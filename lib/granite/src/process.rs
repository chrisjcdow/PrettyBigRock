// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Processes, and the program loader (docs/design/0009-programs-and-granite.md, sections 2
//! to 4). Starting a program is done entirely with ordinary system calls, so a launcher decides
//! exactly what the new program starts with.

use crate::handle::Handle;
use crate::memory::Memory;
use crate::notification::Notification;
use crate::sys::{self, Error, Result, num, perm, rights};
use crate::thread::Thread;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use pbr_elf::Program;

static SELF: AtomicU32 = AtomicU32::new(0);

/// This program's handle to its own process.
pub fn self_handle() -> u32 {
    SELF.load(Ordering::Relaxed)
}

#[cfg(target_os = "prettybigrock")]
pub(crate) fn set_self(h: u32) {
    SELF.store(h, Ordering::Relaxed);
}

/// Roles in the start-up block (note 0009, section 4).
pub mod role {
    /// The program's own process.
    pub const PROCESS: u32 = 1;
    /// Its main thread.
    pub const THREAD: u32 = 2;
    /// The kernel log.
    pub const LOG: u32 = 3;
    /// The start-up archive (the root program only).
    pub const ARCHIVE: u32 = 4;
    /// An endpoint to whoever started it.
    pub const PARENT: u32 = 5;
    /// The first role a launcher and its program agree between themselves.
    pub const OWN: u32 = 1000;
}

/// Ends this process with `status`.
pub fn exit(status: u64) -> ! {
    sys::process_exit(status)
}

/// What a faulting thread did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    /// The kind: 1 memory, 2 instruction, 3 alignment, 4 arithmetic, 5 breakpoint, 6 other.
    pub kind: u64,
    /// The address involved.
    pub address: u64,
    /// The instruction's address.
    pub pc: u64,
}

/// Where a process is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Running.
    Running,
    /// Ending: its threads are stopping.
    Ending,
    /// It exited with this status.
    Exited(u64),
    /// It was stopped with this status.
    Stopped(u64),
    /// A thread faulted.
    Faulted(Fault),
}

/// What `process_info` reports.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    /// Where it is in its life.
    pub state: State,
    /// Its budget, in bytes.
    pub budget: u64,
    /// How much of the budget is in use.
    pub used: u64,
    /// Threads that haven't exited.
    pub threads: u64,
    /// The kernel's number for it.
    pub id: u64,
}

/// A process capability.
#[derive(Debug)]
pub struct Process(pub Handle);

impl Process {
    /// A new, empty process named `name`, with a budget of `budget` bytes taken from this
    /// one's, whose threads may have priorities up to `max_priority`.
    pub fn create(name: &str, budget: u64, max_priority: u8) -> Result<Process> {
        let a = [budget, max_priority.into(), name.as_ptr() as u64, name.len() as u64, 0, 0];
        sys::call(num::PROCESS_CREATE, a).map(|(h, _)| Process(Handle::from_raw(h as u32)))
    }

    /// Puts a copy (or with `moving`, the original) of `h` in the process's table, with only
    /// the rights in `mask`. Returns its handle there.
    pub fn give(&self, h: u32, mask: u32, moving: bool) -> Result<u32> {
        let a = [self.0.raw().into(), h.into(), mask.into(), u64::from(moving), 0, 0];
        sys::call(num::PROCESS_GIVE, a).map(|(h, _)| h as u32)
    }

    /// Stops the process with `status`.
    pub fn stop(&self, status: u64) -> Result<()> {
        sys::call(num::PROCESS_STOP, [self.0.raw().into(), status, 0, 0, 0, 0]).map(|_| ())
    }

    /// Asks for `bits` on `n` when the process ends.
    pub fn watch(&self, n: &Notification, bits: u64) -> Result<()> {
        sys::call(num::PROCESS_WATCH, [self.0.raw().into(), n.0.raw().into(), bits, 0, 0, 0])
            .map(|_| ())
    }

    /// Facts about the process.
    pub fn info(&self) -> Result<Info> {
        info(self.0.raw())
    }

    /// Waits for the process to end and returns how.
    pub fn wait(&self) -> Result<State> {
        let n = Notification::create()?;
        self.watch(&n, 1)?;
        n.wait(crate::time::FOREVER)?;
        Ok(self.info()?.state)
    }
}

/// Facts about the process `h`.
pub fn info(h: u32) -> Result<Info> {
    let mut r = [0u8; 128];
    sys::call(num::PROCESS_INFO, [h.into(), r.as_mut_ptr() as u64, 128, 0, 0, 0])?;
    let w = |i: usize| u64::from_le_bytes(r[8 + 8 * i..16 + 8 * i].try_into().unwrap_or([0; 8]));
    let state = match u32::from_le_bytes(r[4..8].try_into().unwrap_or([0; 4])) {
        0 => State::Running,
        1 => State::Ending,
        2 => State::Exited(w(0)),
        3 => State::Stopped(w(0)),
        _ => State::Faulted(Fault { kind: w(1), address: w(2), pc: w(3) }),
    };
    Ok(Info { state, budget: w(4), used: w(5), threads: w(6), id: w(14) })
}

/// Why a program couldn't be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// The program file is wrong.
    Program(pbr_elf::ElfError),
    /// It needs a newer interface than this kernel has.
    NeedsVersion(u16, u16),
    /// A system call failed.
    System(Error),
}

impl From<Error> for SpawnError {
    fn from(e: Error) -> Self {
        SpawnError::System(e)
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Program(e) => write!(f, "{e}"),
            SpawnError::NeedsVersion(a, b) => write!(f, "it needs interface version {a}.{b}"),
            SpawnError::System(e) => write!(f, "{e}"),
        }
    }
}

/// A capability for a new program: (role, handle here, rights to give, moved rather than
/// copied).
pub type Grant = (u32, u32, u32, bool);

/// How to start a program.
pub struct Spawn<'a> {
    /// Its name, for reports.
    pub name: &'a str,
    /// The program file.
    pub program: &'a [u8],
    /// Its budget, in bytes (taken from this program's).
    pub budget: u64,
    /// Its main thread's priority, and the highest its threads may have.
    pub priority: u8,
    /// The capabilities it starts with, besides its own process and main thread.
    pub caps: &'a [Grant],
    /// Give it a copy of this program's kernel log capability.
    pub log: bool,
    /// Its arguments (at most 3 KiB).
    pub args: &'a str,
}

/// A program that has been started.
pub struct Spawned {
    /// Its process.
    pub process: Process,
    /// Its main thread.
    pub thread: Thread,
}

#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = pbr_elf::EM_X86_64;
#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = pbr_elf::EM_AARCH64;

/// Starts a program (note 0009, section 2).
pub fn spawn(s: &Spawn<'_>) -> core::result::Result<Spawned, SpawnError> {
    let prog = Program::parse(s.program, MACHINE).map_err(SpawnError::Program)?;
    let (major, minor) = prog.needs_version;
    if major != sys::VERSION.0 || minor > sys::VERSION.1 {
        return Err(SpawnError::NeedsVersion(major, minor));
    }
    if s.args.len() > 3072 {
        return Err(SpawnError::System(Error::TooBig));
    }
    let process = Process::create(s.name, s.budget, s.priority)?;
    let p = process.0.raw();

    // One memory object holds the whole image. Mapping it once shows where the kernel puts
    // it (somewhere random); then each segment is mapped there with its own permissions.
    let (lo, hi) = prog.span();
    let span = hi - lo;
    let image = Memory::create(span)?;
    let at = image.map_into(p, 0, span, 0, perm::READ)?;
    sys::call(num::MEMORY_UNMAP, [p.into(), at, span, 0, 0, 0])?;
    let base = at.wrapping_sub(lo);
    for seg in prog.segments() {
        let data = prog.segment_data(&seg, base).map_err(SpawnError::Program)?;
        image.write(seg.vaddr - lo, &data)?;
    }
    for seg in prog.segments() {
        let (start, end) = seg.pages();
        let perms = if seg.executable() {
            perm::READ_EXECUTE
        } else if seg.writable() {
            perm::READ_WRITE
        } else {
            perm::READ
        };
        image.map_into(p, start - lo, end - start, base.wrapping_add(start), perms)?;
    }

    // Its stack and start-up block.
    let stack = Memory::create(prog.stack_size)?;
    let stack_at = stack.map_into(p, 0, prog.stack_size, 0, perm::READ_WRITE)?;
    let block = Memory::create(4096)?;
    let block_at = block.map_into(p, 0, 4096, 0, perm::READ)?;

    // Its capabilities.
    let mut entries: Vec<(u32, u32)> = Vec::new();
    entries.push((role::PROCESS, process.give(p, rights::ALL, false)?));
    let entry = base.wrapping_add(prog.entry);
    let a = [p.into(), entry, stack_at + prog.stack_size, block_at, s.priority.into(), 0];
    let (t, _) = sys::call(num::THREAD_CREATE, a)?;
    let thread = Thread(Handle::from_raw(t as u32));
    entries.push((role::THREAD, process.give(thread.0.raw(), rights::ALL, false)?));
    if s.log && crate::log::handle() != 0 {
        entries.push((role::LOG, process.give(crate::log::handle(), rights::ALL, false)?));
    }
    for &(r, h, mask, moving) in s.caps {
        entries.push((r, process.give(h, mask, moving)?));
    }

    // The block: magic, version, count, where the arguments are, then (role, handle) pairs.
    let mut b = Vec::with_capacity(16 + 8 * entries.len() + s.args.len());
    b.extend_from_slice(b"PBRS");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    b.extend_from_slice(&(16 + 8 * entries.len() as u32).to_le_bytes());
    b.extend_from_slice(&(s.args.len() as u32).to_le_bytes());
    for (r, h) in &entries {
        b.extend_from_slice(&r.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
    }
    b.extend_from_slice(s.args.as_bytes());
    if b.len() > 4096 {
        return Err(SpawnError::System(Error::TooBig));
    }
    block.write(0, &b)?;
    thread.start()?;
    Ok(Spawned { process, thread })
}
