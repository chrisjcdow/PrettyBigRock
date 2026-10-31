// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Programs' threads: what a thread has besides its kernel side, starting it in user mode,
//! and what happens when it faults, is stopped or exits (docs/design/0006 sections 2 and 7,
//! 0007 sections 3 and 9).

use crate::mem::object::MemoryObject;
use crate::mem::space::{USER_END, perm};
use crate::obj::budget::Charge;
use crate::obj::endpoint::Phase;
use crate::obj::notification::Notification;
use crate::obj::process::{Fault, Process, Status};
use crate::obj::{Error, caps, charge};
use crate::sched::{self, Thread};
use crate::sync::SpinLock;
use crate::{arch, mem};
use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// What a program's thread has besides its kernel side.
pub struct UserThread {
    /// Its process.
    pub process: Arc<Process>,
    /// Its message page.
    pub msg: Arc<MemoryObject>,
    /// Where its message page is mapped.
    pub msg_addr: u64,
    /// Its part in a message exchange.
    pub ipc: SpinLock<Phase>,
    /// Set when it's started.
    pub started: AtomicBool,
    regs: arch::UserRegs,
    entry: (u64, u64, u64),
    watchers: SpinLock<Vec<(Arc<Notification>, u64)>>,
    exited: AtomicBool,
    _charge: Charge,
}

impl UserThread {
    /// Saves the program's registers that a thread switch doesn't (floating point and vector
    /// registers, the thread pointer). Called as the thread is switched away from.
    pub fn save_registers(&self) {
        arch::save_user(&self.regs);
    }

    /// Restores what [`UserThread::save_registers`] saved. Called as the thread resumes.
    pub fn load_registers(&self) {
        arch::load_user(&self.regs);
    }

    /// Sets the program's thread pointer (`FS` base on x86-64, `TPIDR_EL0` on AArch64).
    pub fn set_thread_pointer(&self, value: u64) {
        arch::set_thread_pointer(&self.regs, value);
    }

    /// Asks for `bits` on `n` when the thread exits (at once if it has).
    pub fn watch(&self, n: Arc<Notification>, bits: u64) {
        if self.exited.load(Ordering::SeqCst) {
            n.signal(bits);
            return;
        }
        self.watchers.lock().push((n.clone(), bits));
        if self.exited.load(Ordering::SeqCst) {
            n.signal(bits);
        }
    }
}

/// Makes a thread in `process`, not yet started, that will begin at `pc` with stack pointer
/// `sp` and `arg` as its argument (note 0007, section 3).
pub fn create_thread(
    process: &Arc<Process>,
    pc: u64,
    sp: u64,
    arg: u64,
    priority: u8,
) -> Result<Arc<Thread>, Error> {
    if pc >= USER_END || sp > USER_END || priority > process.max_priority {
        return Err(Error::InvalidArgument);
    }
    let c = process.budget.charge(charge::THREAD)?;
    let msg = MemoryObject::kernel(1)?;
    let msg_addr = caps::map_kernel(process, msg.clone(), 0, perm::READ | perm::WRITE)?;
    let undo = |p: &Arc<Process>| {
        p.space.lock().remove(msg_addr);
    };
    let regs = match arch::UserRegs::new() {
        Some(r) => r,
        None => {
            undo(process);
            return Err(Error::OutOfMemory);
        }
    };
    let user = Box::new(UserThread {
        process: process.clone(),
        msg,
        msg_addr,
        ipc: SpinLock::new(Phase::Idle),
        started: AtomicBool::new(false),
        regs,
        entry: (pc, sp, arg),
        watchers: SpinLock::new(Vec::new()),
        exited: AtomicBool::new(false),
        _charge: c,
    });
    let name = format!("{}#{}", process.name, process.thread_count() + 1);
    let Some(t) = sched::create(&name, priority, enter, Some(user)) else {
        undo(process);
        return Err(Error::OutOfMemory);
    };
    if let Err(e) = process.add_thread(t.clone()) {
        undo(process);
        return Err(e);
    }
    Ok(t)
}

/// A program thread's first code in the kernel: goes to user mode.
fn enter() {
    let me = sched::current();
    let u = me.user.as_ref().expect("a program thread");
    let (pc, sp, arg) = u.entry;
    let (sys, msg) = (u.process.syspage, u.msg_addr);
    drop(me); // nothing on this stack may keep the thread alive once it runs
    arch::enter_user(pc, sp, arg, sys, msg)
}

/// Ends the current program thread.
pub fn exit_thread() -> ! {
    arch::irq_enable();
    let me = sched::current();
    if let Some(u) = &me.user
        && !u.exited.swap(true, Ordering::SeqCst)
    {
        let w: Vec<(Arc<Notification>, u64)> = core::mem::take(&mut *u.watchers.lock());
        for (n, bits) in w {
            n.signal(bits);
        }
        u.process.thread_exited(&me);
    }
    drop(me);
    sched::exit()
}

/// The current program thread did something illegal: its process ends, and so does it.
pub fn fault(kind: u64, address: u64, pc: u64) -> ! {
    arch::irq_enable();
    let me = sched::current();
    if let Some(u) = &me.user {
        u.process.end(Status::Fault(Fault { kind, address, pc }));
    }
    drop(me);
    exit_thread()
}

/// Called just before returning to a program, with interrupts masked: a thread that has been
/// asked to stop exits instead.
pub fn before_return() {
    let me = sched::current();
    let stop = me.stop.load(Ordering::SeqCst);
    drop(me);
    if stop {
        exit_thread();
    }
}

/// The page size, for the system call layer.
pub const PAGE: u64 = mem::PAGE;
