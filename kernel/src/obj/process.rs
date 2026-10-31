// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Processes (docs/design/0006-objects-and-capabilities.md, section 2): an address space, one
//! capability table, threads and a budget.
//!
//! A process ends when it exits, its last thread exits, a thread faults, or someone with the
//! stop right stops it. Its threads are told to stop; once none is left, the reaper (a kernel
//! thread) closes its capabilities, removes its mappings, records how it ended and tells its
//! watchers.

use super::budget::{Budget, Charge};
use super::notification::Notification;
use super::table::Table;
use super::{Error, charge};
use crate::mem::space::{Mapping, Space, perm};
use crate::sched::{self, Thread, WaitQueue};
use crate::sync::{Once, SpinLock};
use crate::{kprintln, syspage};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Why a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// It exited (or its last thread did), with this status.
    Exited(u64),
    /// It was stopped by someone else, with this status.
    Stopped(u64),
    /// A thread did something illegal.
    Fault(Fault),
}

/// What a faulting thread did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    /// What kind of fault (see [`fault_kind`]).
    pub kind: u64,
    /// The address involved, if any.
    pub address: u64,
    /// The instruction's address.
    pub pc: u64,
}

/// Kinds of fault, as `process_info` reports them.
pub mod fault_kind {
    /// Touched memory that isn't mapped, or isn't mapped that way.
    pub const MEMORY: u64 = 1;
    /// An instruction that doesn't exist or isn't allowed.
    pub const INSTRUCTION: u64 = 2;
    /// A misaligned stack or code address.
    pub const ALIGNMENT: u64 = 3;
    /// A division error or other arithmetic fault.
    pub const ARITHMETIC: u64 = 4;
    /// A breakpoint, with nothing to catch it.
    pub const BREAKPOINT: u64 = 5;
    /// Anything else.
    pub const OTHER: u64 = 6;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Ending(Status),
    Ended(Status),
}

/// A process.
pub struct Process {
    /// A number for reports.
    pub id: u64,
    /// Its name, for reports.
    pub name: String,
    /// Its address space.
    pub space: SpinLock<Space>,
    root: u64,
    /// Its capabilities.
    pub table: SpinLock<Table>,
    /// Its memory budget.
    pub budget: Arc<Budget>,
    /// The highest priority its threads may have.
    pub max_priority: u8,
    /// Where its system code page is (the system data page follows it).
    pub syspage: u64,
    state: SpinLock<State>,
    threads: SpinLock<Vec<Arc<Thread>>>,
    watchers: SpinLock<Vec<(Arc<Notification>, u64)>>,
    /// Capabilities to this process held by other processes.
    external: AtomicU64,
    /// Set once any thread has been started.
    started: AtomicBool,
    thread_count: AtomicU64,
    _charge: Charge,
}

impl Process {
    /// A new, empty process with budget `budget`. Its system pages are mapped; nothing else.
    pub fn new(name: &str, budget: Arc<Budget>, max_priority: u8) -> Result<Arc<Self>, Error> {
        let c = budget.charge(charge::PROCESS)?;
        let table = Table::new(budget.charge(0)?);
        let mut space = Space::new(budget.charge(0)?)?;
        let (code, data) = syspage::pages();
        let at = space.choose(2 * crate::mem::PAGE)?;
        for (i, (mem, perms)) in
            [(code, perm::READ | perm::EXECUTE), (data, perm::READ)].into_iter().enumerate()
        {
            space.map(Mapping {
                start: at + i as u64 * crate::mem::PAGE,
                len: crate::mem::PAGE,
                mem,
                offset: 0,
                perms,
                max: perms,
                node: None,
            })?;
        }
        let root = space.root();
        Ok(Arc::new(Process {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: String::from(name),
            space: SpinLock::new(space),
            root,
            table: SpinLock::new(table),
            budget,
            max_priority,
            syspage: at,
            state: SpinLock::new(State::Running),
            threads: SpinLock::new(Vec::new()),
            watchers: SpinLock::new(Vec::new()),
            external: AtomicU64::new(0),
            started: AtomicBool::new(false),
            thread_count: AtomicU64::new(0),
            _charge: c,
        }))
    }

    /// The root of its page tables.
    pub fn root(&self) -> u64 {
        self.root
    }

    /// True until it starts to end.
    pub fn running(&self) -> bool {
        *self.state.lock() == State::Running
    }

    /// How it ended, once it has.
    pub fn status(&self) -> Option<Status> {
        match *self.state.lock() {
            State::Ended(s) => Some(s),
            _ => None,
        }
    }

    /// Adds a thread made for this process (not yet started). Fails if the process is ending.
    pub fn add_thread(&self, t: Arc<Thread>) -> Result<(), Error> {
        let st = self.state.lock();
        if *st != State::Running {
            return Err(Error::BadState);
        }
        self.threads.lock().push(t);
        self.thread_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Starts one of its threads.
    pub fn start_thread(&self, t: &Arc<Thread>) -> Result<(), Error> {
        let st = self.state.lock();
        if *st != State::Running {
            return Err(Error::BadState);
        }
        let u = t.user.as_ref().ok_or(Error::WrongType)?;
        if u.started.swap(true, Ordering::SeqCst) {
            return Err(Error::BadState);
        }
        self.started.store(true, Ordering::SeqCst);
        drop(st);
        sched::start(t);
        Ok(())
    }

    /// The number of its threads that haven't exited.
    pub fn thread_count(&self) -> u64 {
        self.threads.lock().len() as u64
    }

    /// Starts ending the process with `status`, unless it's already ending.
    pub fn end(self: &Arc<Self>, status: Status) {
        {
            let mut st = self.state.lock();
            if *st != State::Running {
                return;
            }
            *st = State::Ending(status);
        }
        if let Status::Fault(f) = status {
            kprintln!(
                "Program \"{}\" stopped: {} at {:#x} (instruction at {:#x}).",
                self.name,
                describe_fault(f.kind),
                f.address,
                f.pc
            );
        }
        // Threads that never started can go at once; the rest are told to stop.
        let empty = {
            let mut ts = self.threads.lock();
            ts.retain(|t| t.user.as_ref().is_some_and(|u| u.started.load(Ordering::SeqCst)));
            for t in ts.iter() {
                t.stop.store(true, Ordering::SeqCst);
                sched::kick(t);
            }
            ts.is_empty()
        };
        if empty {
            reap(self.clone());
        }
    }

    /// Called by each of its threads as it exits.
    pub fn thread_exited(self: &Arc<Self>, t: &Arc<Thread>) {
        let empty = {
            let mut ts = self.threads.lock();
            ts.retain(|x| !Arc::ptr_eq(x, t));
            ts.is_empty()
        };
        if empty {
            // The last thread leaving ends the process, if nothing else already has. If
            // something has, it left the finishing to the last thread out: this one.
            let ending = matches!(*self.state.lock(), State::Ending(_));
            if ending {
                reap(self.clone());
            } else {
                self.end(Status::Exited(0));
            }
        }
    }

    /// Asks for `bits` to be signalled on `n` when the process ends (at once if it has).
    pub fn watch(&self, n: Arc<Notification>, bits: u64) {
        let ended = matches!(*self.state.lock(), State::Ended(_));
        if ended {
            n.signal(bits);
        } else {
            self.watchers.lock().push((n, bits));
            // It may have ended in between; signalling twice is harmless.
            if matches!(*self.state.lock(), State::Ended(_)) {
                self.signal_watchers();
            }
        }
    }

    fn signal_watchers(&self) {
        let w: Vec<(Arc<Notification>, u64)> = core::mem::take(&mut *self.watchers.lock());
        for (n, bits) in w {
            n.signal(bits);
        }
    }

    /// A capability to this process was put in `owner`'s table.
    pub fn cap_added(&self, owner: &Process) {
        if !core::ptr::eq(self, owner) {
            self.external.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A capability to this process left `owner`'s table. A process that never started a
    /// thread, and that nobody else can reach any more, can never run: it ends.
    pub fn cap_removed(self: &Arc<Self>, owner: &Process) {
        if !core::ptr::eq(&**self, owner)
            && self.external.fetch_sub(1, Ordering::SeqCst) == 1
            && !self.started.load(Ordering::SeqCst)
        {
            self.end(Status::Exited(0));
        }
    }

    /// The record `process_info` writes (note 0007): 128 bytes, little-endian.
    pub fn info(&self) -> [u8; 128] {
        let mut out = [0u8; 128];
        let (state, status, f) = match *self.state.lock() {
            State::Running => (0u32, 0, None),
            State::Ending(_) => (1, 0, None),
            State::Ended(Status::Exited(s)) => (2, s, None),
            State::Ended(Status::Stopped(s)) => (3, s, None),
            State::Ended(Status::Fault(f)) => (4, 0, Some(f)),
        };
        let words: [u64; 15] = [
            status,
            f.map_or(0, |f| f.kind),
            f.map_or(0, |f| f.address),
            f.map_or(0, |f| f.pc),
            self.budget.limit(),
            self.budget.used(),
            self.thread_count(),
            charge::THREAD,
            charge::ENDPOINT,
            charge::NOTIFICATION,
            charge::REPLY,
            charge::PROCESS,
            charge::TABLE_SLOT,
            charge::PAGE_TABLE,
            self.id,
        ];
        out[0..4].copy_from_slice(&1u32.to_le_bytes());
        out[4..8].copy_from_slice(&state.to_le_bytes());
        for (i, w) in words.iter().enumerate() {
            out[8 + i * 8..16 + i * 8].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// Words for a fault kind, for the kernel log.
pub fn describe_fault(kind: u64) -> &'static str {
    match kind {
        fault_kind::MEMORY => "a memory access it isn't allowed",
        fault_kind::INSTRUCTION => "an illegal instruction",
        fault_kind::ALIGNMENT => "a misaligned address",
        fault_kind::ARITHMETIC => "an arithmetic error",
        fault_kind::BREAKPOINT => "a breakpoint",
        _ => "a fault",
    }
}

// ------------------------------------------------------------------------------------------
// The reaper: finishes ending processes, in a kernel thread of its own, so it never runs
// with anyone's locks held.

static REAP: SpinLock<VecDeque<Arc<Process>>> = SpinLock::new(VecDeque::new());
static REAP_WAIT: WaitQueue = WaitQueue::new();
static REAPER: Once<()> = Once::new();

/// Starts the reaper thread. Called once, before any process exists.
pub fn start_reaper() {
    REAPER.set(());
    sched::spawn("reaper", sched::NORMAL + 1, || {
        loop {
            REAP_WAIT.wait_until(|| !REAP.lock().is_empty());
            let next = REAP.lock().pop_front();
            if let Some(p) = next {
                finish(&p);
            }
        }
    });
}

fn reap(p: Arc<Process>) {
    REAP.lock().push_back(p);
    REAP_WAIT.wake_all();
}

/// Closes everything the process held, and records how it ended.
fn finish(p: &Arc<Process>) {
    let status = match *p.state.lock() {
        State::Ending(s) => s,
        _ => return,
    };
    super::caps::teardown(p);
    *p.state.lock() = State::Ended(status);
    p.signal_watchers();
}
