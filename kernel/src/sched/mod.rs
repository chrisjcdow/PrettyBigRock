// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Threads and the scheduler (docs/design/0005-kernel-core.md, section 8).
//!
//! Each core has its own run queue. A thread that's switched away from is put back into a run
//! queue only after the switch has finished saving it, by the next thread to run, as its first
//! action ([`finish_switch`]); otherwise another core could start it before its registers were
//! saved. Waking a thread that's still on its way off a core is handled by the `on_cpu` and
//! `queued` flags, which make sure it's queued exactly once.

use crate::sync::{IrqSpinLock, Level, SpinLock};
use crate::{arch, mem, percpu, time};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Priorities run from 0 (lowest) to 3 (highest).
pub const PRIORITIES: usize = 4;
/// The usual priority.
pub const NORMAL: u8 = 1;
/// Kernel stack size for threads, in pages (128 KiB).
const STACK_PAGES: u64 = 32;
/// A time slice, in timer ticks (10 ms).
const SLICE_TICKS: u32 = 10;

const READY: u8 = 0;
const RUNNING: u8 = 1;
const BLOCKED: u8 = 2;
const EXITED: u8 = 3;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Every thread that's been started and not yet freed, for diagnostics.
static ALL: IrqSpinLock<Vec<alloc::sync::Weak<Thread>>> =
    IrqSpinLock::new(Level::ThreadList, "thread list", Vec::new());

/// A thread.
pub struct Thread {
    /// A number unique to this thread.
    pub id: u64,
    /// A name, for reports.
    pub name: String,
    /// Its priority.
    pub priority: u8,
    sp: UnsafeCell<u64>,
    stack: Option<u64>, // top of a stack from mem::vm::alloc_stack
    state: AtomicU8,
    on_cpu: AtomicBool,
    queued: AtomicBool,
    last_cpu: AtomicU32,
    is_idle: bool,
    entry: SpinLock<Option<Box<dyn FnOnce() + Send>>>,
    exited: WaitQueue,
}

// SAFETY: `sp` is only touched by the core switching to or from the thread, with interrupts
// masked; everything else is atomic or locked.
unsafe impl Sync for Thread {}
// SAFETY: as above.
unsafe impl Send for Thread {}

impl Drop for Thread {
    fn drop(&mut self) {
        if let Some(top) = self.stack {
            mem::vm::free_stack(top, STACK_PAGES);
        }
    }
}

impl Thread {
    /// True once the thread has finished.
    pub fn has_exited(&self) -> bool {
        self.state.load(Ordering::SeqCst) == EXITED
    }
}

/// What happens to the thread being switched away from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// It's still ready to run: put it back in a run queue.
    Requeue,
    /// It's waiting for something; whatever it waits for will wake it.
    Block,
    /// It has finished.
    Exit,
    /// It's this core's idle thread, which never goes in a run queue.
    Idle,
}

struct RunQueue {
    levels: [VecDeque<Arc<Thread>>; PRIORITIES],
    len: usize,
}

impl RunQueue {
    fn pop(&mut self) -> Option<Arc<Thread>> {
        let t = self.levels.iter_mut().rev().find_map(|q| q.pop_front())?;
        self.len -= 1;
        Some(t)
    }
}

/// One core's scheduler state.
pub struct CpuSched {
    rq: IrqSpinLock<RunQueue>,
    timers: IrqSpinLock<Vec<(u64, Arc<Thread>)>>,
    current: UnsafeCell<Option<Arc<Thread>>>,
    idle: UnsafeCell<Option<Arc<Thread>>>,
    prev: UnsafeCell<Option<(Arc<Thread>, Action)>>,
    /// Set when the current thread should give way at the next chance.
    pub need_resched: AtomicBool,
    slice: AtomicU32,
    /// Context switches on this core.
    pub switches: AtomicU64,
}

impl CpuSched {
    /// Empty, for a core that isn't running yet.
    pub fn new() -> Self {
        Self {
            rq: IrqSpinLock::new(
                Level::RunQueue,
                "run queue",
                RunQueue { levels: Default::default(), len: 0 },
            ),
            timers: IrqSpinLock::new(Level::Timers, "timers", Vec::new()),
            current: UnsafeCell::new(None),
            idle: UnsafeCell::new(None),
            prev: UnsafeCell::new(None),
            need_resched: AtomicBool::new(false),
            slice: AtomicU32::new(SLICE_TICKS),
            switches: AtomicU64::new(0),
        }
    }
}

impl Default for CpuSched {
    fn default() -> Self {
        Self::new()
    }
}

/// Makes what's running on this core now into its idle thread. Called once per core.
pub fn init_this_core() {
    let cpu = percpu::this();
    let idle = Arc::new(Thread {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        name: alloc::format!("idle {}", cpu.index),
        priority: 0,
        sp: UnsafeCell::new(0),
        stack: None,
        state: AtomicU8::new(RUNNING),
        on_cpu: AtomicBool::new(true),
        queued: AtomicBool::new(false),
        last_cpu: AtomicU32::new(cpu.index),
        is_idle: true,
        entry: SpinLock::new(None),
        exited: WaitQueue::new(),
    });
    let irq = arch::irq_save();
    // SAFETY: only this core touches its own `current` and `idle`, with interrupts masked.
    unsafe {
        *cpu.sched.idle.get() = Some(idle.clone());
        *cpu.sched.current.get() = Some(idle);
    }
    arch::irq_restore(irq);
}

/// The thread running on this core.
pub fn current() -> Arc<Thread> {
    let irq = arch::irq_save();
    // SAFETY: only this core touches its own `current`, with interrupts masked.
    let t = unsafe { (*percpu::this().sched.current.get()).clone() };
    arch::irq_restore(irq);
    t.expect("no current thread")
}

/// Calls `f` with the current thread's name and number, or ("start-up", 0) before the
/// scheduler runs on this core. Doesn't allocate or lock, so it's safe in a panic.
pub fn with_current_name(f: impl FnOnce(&str, u64)) {
    let Some(cpu) = percpu::try_this() else { return f("start-up", 0) };
    // SAFETY: only this core touches its own `current`; a panic has interrupts masked.
    match unsafe { (*cpu.sched.current.get()).as_ref() } {
        Some(t) => f(&t.name, t.id),
        None => f("start-up", 0),
    }
}

/// Starts a new thread running `f`.
pub fn spawn(name: &str, priority: u8, f: impl FnOnce() + Send + 'static) -> Arc<Thread> {
    let top = mem::vm::alloc_stack(STACK_PAGES).expect("out of memory for a kernel stack");
    let t = Arc::new(Thread {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        name: String::from(name),
        priority: priority.min(PRIORITIES as u8 - 1),
        sp: UnsafeCell::new(0),
        stack: Some(top),
        state: AtomicU8::new(READY),
        on_cpu: AtomicBool::new(false),
        queued: AtomicBool::new(true),
        last_cpu: AtomicU32::new(0),
        is_idle: false,
        entry: SpinLock::new(Some(Box::new(f))),
        exited: WaitQueue::new(),
    });
    // SAFETY: the thread hasn't started, so nothing else uses its stack pointer. The stack
    // holds a pointer to the thread, kept alive by the run queue's reference until it runs.
    unsafe { *t.sp.get() = arch::init_thread_stack(top, Arc::as_ptr(&t) as u64) };
    // The least busy online core.
    let target = percpu::all()
        .iter()
        .filter(|c| c.online.load(Ordering::Acquire))
        .min_by_key(|c| c.sched.rq.lock().len)
        .map(|c| c.index)
        .unwrap_or(0);
    t.last_cpu.store(target, Ordering::Relaxed);
    {
        let mut all = ALL.lock();
        all.retain(|w| w.strong_count() > 0);
        all.push(Arc::downgrade(&t));
    }
    enqueue(target, t.clone());
    t
}

/// Prints the scheduler's state: each core's current thread and queues, and every thread.
/// For diagnosing hangs; it only ever *tries* locks, so it's safe from an interrupt handler.
pub fn dump() {
    use crate::kprintln;
    let state = |t: &Thread| match t.state.load(Ordering::SeqCst) {
        READY => "ready",
        RUNNING => "running",
        BLOCKED => "blocked",
        EXITED => "exited",
        _ => "?",
    };
    let now = time::now_ns();
    for c in percpu::all() {
        // SAFETY: a diagnostic read of another core's `current`; it may be stale, never freed
        // (the core's own reference keeps it alive while it's there).
        let cur = unsafe { (*c.sched.current.get()).as_ref().map(|t| (t.name.clone(), t.id)) };
        let rq = c.sched.rq.try_lock().map(|q| q.len);
        let timers = c.sched.timers.try_lock().map(|t| {
            t.iter()
                .map(|(d, th)| {
                    alloc::format!("{}@{}ms", th.id, (*d as i64 - now as i64) / 1_000_000)
                })
                .collect::<Vec<_>>()
        });
        kprintln!(
            "core {}: online {}, current {:?}, run queue {:?}, timers {:?}, ticks {}, need_resched {}",
            c.index,
            c.online.load(Ordering::Relaxed),
            cur,
            rq,
            timers,
            c.ticks.load(Ordering::Relaxed),
            c.sched.need_resched.load(Ordering::Relaxed)
        );
    }
    if let Some(all) = ALL.try_lock() {
        for t in all.iter().filter_map(|w| w.upgrade()) {
            if t.state.load(Ordering::SeqCst) != EXITED {
                kprintln!(
                    "  thread {} \"{}\": {}, on_cpu {}, queued {}, last core {}",
                    t.id,
                    t.name,
                    state(&t),
                    t.on_cpu.load(Ordering::SeqCst),
                    t.queued.load(Ordering::SeqCst),
                    t.last_cpu.load(Ordering::Relaxed)
                );
            }
        }
    }
}

/// Called (through the architecture's start-up code) when a new thread first runs.
pub extern "C" fn thread_entry(thread: u64) -> ! {
    finish_switch();
    arch::irq_enable();
    // SAFETY: `thread` is the Arc pointer `spawn` put on the stack; this thread's own entry in
    // its core's `current` keeps it alive.
    let t = unsafe { &*(thread as *const Thread) };
    let f = t.entry.lock().take();
    if let Some(f) = f {
        f();
    }
    exit()
}

fn enqueue(cpu: u32, t: Arc<Thread>) {
    let cpus = percpu::all();
    let target = cpus.get(cpu as usize).copied().unwrap_or(cpus[0]);
    {
        let mut rq = target.sched.rq.lock();
        rq.levels[usize::from(t.priority)].push_back(t);
        rq.len += 1;
    }
    let here = percpu::try_this().map(|c| c.index);
    if here != Some(target.index) && target.online.load(Ordering::Acquire) {
        arch::send_ipi(target, arch::Ipi::Reschedule);
    }
}

/// Wakes a thread that's waiting. Does nothing if it isn't.
pub fn wake(t: &Arc<Thread>) {
    if t.state.compare_exchange(BLOCKED, READY, Ordering::SeqCst, Ordering::SeqCst).is_ok()
        && !t.on_cpu.load(Ordering::SeqCst)
        && !t.queued.swap(true, Ordering::SeqCst)
    {
        enqueue(t.last_cpu.load(Ordering::Relaxed), t.clone());
    }
}

/// Takes the next thread to run: from this core's queue, or else from another core's.
fn pick(cpu: &percpu::PerCpu) -> Option<Arc<Thread>> {
    let mut t = cpu.sched.rq.lock().pop();
    if t.is_none() {
        let all = percpu::all();
        for i in 1..all.len() {
            let other = all[(cpu.index as usize + i) % all.len()];
            if let Some(mut rq) = other.sched.rq.try_lock() {
                t = rq.pop();
                if t.is_some() {
                    break;
                }
            }
        }
    }
    let t = t?;
    t.queued.store(false, Ordering::SeqCst);
    t.on_cpu.store(true, Ordering::SeqCst);
    Some(t)
}

/// Switches to the next thread. Interrupts must be masked, and no locks held.
fn schedule(action: Action) {
    let cpu = percpu::this();
    let s = &cpu.sched;
    // SAFETY: only this core touches its own scheduler cells, and interrupts are masked.
    let (cur, idle) = unsafe {
        (
            (*s.current.get()).take().expect("no current thread"),
            (*s.idle.get()).clone().expect("no idle thread"),
        )
    };
    let next = match pick(cpu) {
        Some(n) => n,
        None if action == Action::Requeue && !cur.is_idle => {
            // Nothing else to run: carry on.
            // SAFETY: as above.
            unsafe { *s.current.get() = Some(cur) };
            return;
        }
        None => idle,
    };
    if Arc::ptr_eq(&next, &cur) {
        // SAFETY: as above.
        unsafe { *s.current.get() = Some(cur) };
        return;
    }
    next.state.store(RUNNING, Ordering::SeqCst);
    next.on_cpu.store(true, Ordering::SeqCst);
    next.last_cpu.store(cpu.index, Ordering::Relaxed);
    s.slice.store(SLICE_TICKS, Ordering::Relaxed);
    s.switches.fetch_add(1, Ordering::Relaxed);
    let prev_sp = cur.sp.get();
    let next_sp = next.sp.get();
    // SAFETY: as above. The references move into the per-core cells, so no counted reference is
    // left on the old thread's stack (an exiting thread never comes back to drop it).
    unsafe {
        *s.prev.get() = Some((cur, action));
        *s.current.get() = Some(next);
        arch::switch_context(prev_sp, *next_sp);
    }
    finish_switch();
}

/// Deals with the thread this core just switched away from. Every thread does this first
/// whenever it starts or resumes.
fn finish_switch() {
    let s = &percpu::this().sched;
    // SAFETY: only this core touches its own `prev`, with interrupts masked.
    let Some((prev, action)) = (unsafe { (*s.prev.get()).take() }) else { return };
    match action {
        Action::Requeue => {
            prev.state.store(READY, Ordering::SeqCst);
            prev.on_cpu.store(false, Ordering::SeqCst);
            if !prev.queued.swap(true, Ordering::SeqCst) {
                let cpu = prev.last_cpu.load(Ordering::Relaxed);
                enqueue(cpu, prev);
            }
        }
        Action::Block => {
            prev.on_cpu.store(false, Ordering::SeqCst);
            // Woken while still on its way off this core: queue it now.
            if prev.state.load(Ordering::SeqCst) == READY
                && !prev.queued.swap(true, Ordering::SeqCst)
            {
                let cpu = prev.last_cpu.load(Ordering::Relaxed);
                enqueue(cpu, prev);
            }
        }
        Action::Exit | Action::Idle => prev.on_cpu.store(false, Ordering::SeqCst),
    }
}

/// Gives the core to another ready thread, if there is one.
pub fn yield_now() {
    let irq = arch::irq_save();
    schedule(Action::Requeue);
    arch::irq_restore(irq);
}

/// Sleeps for at least `ns` nanoseconds (to the timer tick's resolution).
pub fn sleep_ns(ns: u64) {
    let deadline = time::now_ns() + ns;
    let irq = arch::irq_save();
    let me = current();
    me.state.store(BLOCKED, Ordering::SeqCst);
    {
        let mut timers = percpu::this().sched.timers.lock();
        let at = timers.partition_point(|(d, _)| *d <= deadline);
        timers.insert(at, (deadline, me));
    }
    schedule(Action::Block);
    arch::irq_restore(irq);
}

/// Ends the current thread.
pub fn exit() -> ! {
    arch::irq_disable();
    let me = current();
    me.state.store(EXITED, Ordering::SeqCst);
    me.exited.wake_all();
    drop(me);
    schedule(Action::Exit);
    unreachable!("an exited thread ran again");
}

/// Waits for `t` to finish.
pub fn join(t: &Thread) {
    t.exited.wait_until(|| t.has_exited());
}

/// Each core's idle loop: run whatever's ready, otherwise wait for an interrupt.
pub fn idle_loop() -> ! {
    loop {
        arch::irq_disable();
        schedule(Action::Idle);
        arch::wait_for_interrupt();
    }
}

/// If set, the time (`time::now_ns`) after which the timer interrupt prints [`dump`] once.
pub static WATCHDOG: AtomicU64 = AtomicU64::new(0);

/// Called by the timer interrupt on each core, with interrupts masked.
pub fn tick() {
    let cpu = percpu::this();
    cpu.ticks.fetch_add(1, Ordering::Relaxed);
    let now = time::now_ns();
    let wd = WATCHDOG.load(Ordering::Relaxed);
    if cpu.index == 0 && wd != 0 && now > wd {
        WATCHDOG.store(0, Ordering::Relaxed);
        crate::kprintln!("Watchdog: still waiting after the deadline. Scheduler state:");
        dump();
    }
    let due: Vec<Arc<Thread>> = {
        let mut timers = cpu.sched.timers.lock();
        let n = timers.partition_point(|(d, _)| *d <= now);
        timers.drain(..n).map(|(_, t)| t).collect()
    };
    for t in &due {
        wake(t);
    }
    let s = &cpu.sched;
    if s.slice.fetch_sub(1, Ordering::Relaxed) <= 1 {
        s.need_resched.store(true, Ordering::Relaxed);
    }
    // An idle core with work waiting should look at it straight away.
    // SAFETY: only this core touches its own `current`, with interrupts masked.
    if unsafe { (*s.current.get()).as_ref().is_some_and(|c| c.is_idle) } && s.rq.lock().len > 0 {
        s.need_resched.store(true, Ordering::Relaxed);
    }
}

/// Called at the end of every interrupt handler, with interrupts masked, before returning to
/// the interrupted code: switches threads if this core's thread should give way.
pub fn interrupt_exit() {
    let Some(cpu) = percpu::try_this() else { return };
    if cpu.sched.need_resched.load(Ordering::Relaxed) && percpu::preemptible() {
        cpu.sched.need_resched.store(false, Ordering::Relaxed);
        // SAFETY: only this core touches its own `current`, with interrupts masked.
        let idle = unsafe { (*cpu.sched.current.get()).as_ref().is_some_and(|c| c.is_idle) };
        schedule(if idle { Action::Idle } else { Action::Requeue });
    }
}

/// A queue of threads waiting for something.
pub struct WaitQueue {
    q: IrqSpinLock<VecDeque<Arc<Thread>>>,
}

impl WaitQueue {
    /// Nobody waiting.
    pub const fn new() -> Self {
        Self { q: IrqSpinLock::new(Level::WaitQueue, "wait queue", VecDeque::new()) }
    }

    /// Waits until `cond` is true. `cond` is checked with the queue locked, so a wake-up between
    /// checking and waiting can't be missed (as long as wakers change the condition first).
    pub fn wait_until(&self, cond: impl Fn() -> bool) {
        loop {
            let irq = arch::irq_save();
            {
                let mut q = self.q.lock();
                if cond() {
                    drop(q);
                    arch::irq_restore(irq);
                    return;
                }
                let me = current();
                me.state.store(BLOCKED, Ordering::SeqCst);
                q.push_back(me);
            }
            schedule(Action::Block);
            arch::irq_restore(irq);
        }
    }

    /// Wakes everyone waiting.
    pub fn wake_all(&self) {
        let waiting: Vec<Arc<Thread>> = self.q.lock().drain(..).collect();
        for t in &waiting {
            wake(t);
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
