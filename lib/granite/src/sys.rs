// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The system calls (docs/design/0007-system-calls.md), made through the system page, and
//! the constants that go with them. Most programs use the friendlier types elsewhere in
//! `granite`; these are the building blocks.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

/// An error from the kernel (note 0007, section 5), or [`Error::Unknown`] for a code this
/// version of `granite` doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such call.
    InvalidCall,
    /// The handle isn't valid here.
    BadHandle,
    /// The handle refers to the wrong kind of object.
    WrongType,
    /// The capability lacks a right the call needs.
    NoRight,
    /// An argument is out of range, or a reserved bit isn't zero.
    InvalidArgument,
    /// Memory the call had to read or write isn't mapped that way.
    BadAddress,
    /// The budget can't cover it.
    OutOfMemory,
    /// The deadline passed.
    TimedOut,
    /// The other side has gone.
    PeerGone,
    /// A capability in use was destroyed while the call waited.
    Revoked,
    /// Something exceeds a limit.
    TooBig,
    /// The other side's capability table is full.
    TableFull,
    /// Something is already there.
    AlreadyExists,
    /// The object isn't in a state that allows this.
    BadState,
    /// Can't be done here.
    NotSupported,
    /// An error code this version of `granite` doesn't know.
    Unknown(u64),
}

impl Error {
    /// The error for a kernel status code (not 0).
    pub fn from_code(code: u64) -> Error {
        match code {
            1 => Error::InvalidCall,
            2 => Error::BadHandle,
            3 => Error::WrongType,
            4 => Error::NoRight,
            5 => Error::InvalidArgument,
            6 => Error::BadAddress,
            7 => Error::OutOfMemory,
            8 => Error::TimedOut,
            9 => Error::PeerGone,
            10 => Error::Revoked,
            11 => Error::TooBig,
            12 => Error::TableFull,
            13 => Error::AlreadyExists,
            14 => Error::BadState,
            15 => Error::NotSupported,
            other => Error::Unknown(other),
        }
    }

    /// The kernel's code for this error.
    pub fn code(self) -> u64 {
        match self {
            Error::InvalidCall => 1,
            Error::BadHandle => 2,
            Error::WrongType => 3,
            Error::NoRight => 4,
            Error::InvalidArgument => 5,
            Error::BadAddress => 6,
            Error::OutOfMemory => 7,
            Error::TimedOut => 8,
            Error::PeerGone => 9,
            Error::Revoked => 10,
            Error::TooBig => 11,
            Error::TableFull => 12,
            Error::AlreadyExists => 13,
            Error::BadState => 14,
            Error::NotSupported => 15,
            Error::Unknown(c) => c,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidCall => f.write_str("no such system call"),
            Error::BadHandle => f.write_str("not a valid handle"),
            Error::WrongType => f.write_str("the wrong kind of object"),
            Error::NoRight => f.write_str("not allowed"),
            Error::InvalidArgument => f.write_str("an invalid argument"),
            Error::BadAddress => f.write_str("a bad address"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::TimedOut => f.write_str("timed out"),
            Error::PeerGone => f.write_str("the other side has gone"),
            Error::Revoked => f.write_str("revoked"),
            Error::TooBig => f.write_str("too big"),
            Error::TableFull => f.write_str("no room for more capabilities"),
            Error::AlreadyExists => f.write_str("already exists"),
            Error::BadState => f.write_str("not possible now"),
            Error::NotSupported => f.write_str("not supported"),
            Error::Unknown(c) => write!(f, "error {c}"),
        }
    }
}

/// A result with a kernel [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

/// Object types (note 0006, section 1).
pub mod kind {
    /// A process.
    pub const PROCESS: u32 = 1;
    /// A thread.
    pub const THREAD: u32 = 2;
    /// A memory object.
    pub const MEMORY: u32 = 3;
    /// An endpoint.
    pub const ENDPOINT: u32 = 4;
    /// A notification.
    pub const NOTIFICATION: u32 = 5;
    /// A reply capability.
    pub const REPLY: u32 = 6;
    /// The kernel log.
    pub const LOG: u32 = 7;
}

/// Rights (note 0006, section 4).
pub mod rights {
    /// May be copied.
    pub const DUPLICATE: u32 = 1 << 0;
    /// May be sent in a message.
    pub const TRANSFER: u32 = 1 << 1;
    /// May revoke its copies.
    pub const REVOKE: u32 = 1 << 2;
    /// Memory: read, or map readable.
    pub const READ: u32 = 1 << 3;
    /// Memory: write, or map writable. The kernel log: write to it.
    pub const WRITE: u32 = 1 << 4;
    /// Memory: map executable.
    pub const EXECUTE: u32 = 1 << 5;
    /// Memory: map at all.
    pub const MAP: u32 = 1 << 6;
    /// Read details about the object.
    pub const INSPECT: u32 = 1 << 7;
    /// Process or thread: build or start it.
    pub const MANAGE: u32 = 1 << 8;
    /// Process or thread: stop it.
    pub const STOP: u32 = 1 << 9;
    /// Process or thread: be told when it ends.
    pub const WATCH: u32 = 1 << 10;
    /// Endpoint: send to it.
    pub const SEND: u32 = 1 << 8;
    /// Endpoint: receive from it.
    pub const RECEIVE: u32 = 1 << 9;
    /// Endpoint: be told when its last sender goes.
    pub const ENDPOINT_WATCH: u32 = 1 << 10;
    /// Notification: signal it.
    pub const SIGNAL: u32 = 1 << 8;
    /// Notification: wait on it.
    pub const WAIT: u32 = 1 << 9;
    /// Every right.
    pub const ALL: u32 = 0x07FF;
}

/// Mapping permissions (note 0007, section 6).
pub mod perm {
    /// Readable.
    pub const READ: u64 = 1;
    /// Readable and writable.
    pub const READ_WRITE: u64 = 3;
    /// Readable and executable.
    pub const READ_EXECUTE: u64 = 5;
}

/// System call numbers (note 0007, section 6).
#[allow(missing_docs)]
pub mod num {
    pub const VERSION: u64 = 1;
    pub const CLOCK_NOW: u64 = 2;
    pub const HANDLE_CLOSE: u64 = 10;
    pub const HANDLE_DUPLICATE: u64 = 11;
    pub const HANDLE_REVOKE: u64 = 12;
    pub const HANDLE_INFO: u64 = 13;
    pub const PROCESS_CREATE: u64 = 20;
    pub const PROCESS_GIVE: u64 = 21;
    pub const PROCESS_STOP: u64 = 22;
    pub const PROCESS_INFO: u64 = 23;
    pub const PROCESS_WATCH: u64 = 24;
    pub const PROCESS_EXIT: u64 = 25;
    pub const MEMORY_CREATE: u64 = 30;
    pub const MEMORY_MAP: u64 = 31;
    pub const MEMORY_UNMAP: u64 = 32;
    pub const MEMORY_PROTECT: u64 = 33;
    pub const MEMORY_READ: u64 = 34;
    pub const MEMORY_WRITE: u64 = 35;
    pub const MEMORY_INFO: u64 = 36;
    pub const THREAD_CREATE: u64 = 40;
    pub const THREAD_START: u64 = 41;
    pub const THREAD_EXIT: u64 = 42;
    pub const THREAD_STOP: u64 = 43;
    pub const THREAD_YIELD: u64 = 44;
    pub const THREAD_SLEEP: u64 = 45;
    pub const THREAD_WATCH: u64 = 46;
    pub const THREAD_SET_POINTER: u64 = 47;
    pub const ENDPOINT_CREATE: u64 = 50;
    pub const ENDPOINT_MINT: u64 = 51;
    pub const ENDPOINT_CALL: u64 = 52;
    pub const ENDPOINT_RECEIVE: u64 = 53;
    pub const ENDPOINT_REPLY: u64 = 54;
    pub const ENDPOINT_REPLY_RECEIVE: u64 = 55;
    pub const ENDPOINT_BIND: u64 = 56;
    pub const ENDPOINT_WATCH: u64 = 57;
    pub const NOTIFICATION_CREATE: u64 = 60;
    pub const NOTIFICATION_SIGNAL: u64 = 61;
    pub const NOTIFICATION_WAIT: u64 = 62;
    pub const LOG_WRITE: u64 = 70;
}

/// The interface version this `granite` is built for, recorded in every program's
/// PrettyBigRock note.
pub const VERSION: (u16, u16) = (1, 0);

static CALL: AtomicU64 = AtomicU64::new(0);
static CLOCK: AtomicU64 = AtomicU64::new(0);

/// Records where the system page is. Called once, at start-up, before any system call.
///
/// # Safety
/// `page` must be the system page's address, as the kernel passed it.
pub unsafe fn set_system_page(page: u64) {
    // SAFETY: the kernel maps the system page readable; its header is note 0007's.
    let (call, clock) = unsafe {
        (
            core::ptr::read_volatile((page + 16) as *const u64),
            core::ptr::read_volatile((page + 24) as *const u64),
        )
    };
    CALL.store(page + call, Ordering::Release);
    CLOCK.store(page + clock, Ordering::Release);
}

/// Makes system call `n` with six arguments. Returns (status, result 1, result 2).
#[cfg(target_arch = "x86_64")]
pub fn raw(n: u64, a: [u64; 6]) -> (u64, u64, u64) {
    let entry = CALL.load(Ordering::Acquire);
    let (status, r1, r2);
    // SAFETY: the system page's `call` follows note 0007's convention: it changes only rax,
    // rcx, r11, rdx and rsi, and the kernel checks every argument.
    unsafe {
        core::arch::asm!(
            "call {entry}",
            entry = in(reg) entry,
            inlateout("rax") n => status,
            inlateout("rdi") a[0] => _,
            inlateout("rsi") a[1] => r2,
            inlateout("rdx") a[2] => r1,
            in("r10") a[3],
            in("r8") a[4],
            in("r9") a[5],
            out("rcx") _,
            out("r11") _,
        );
    }
    (status, r1, r2)
}

/// Makes system call `n` with six arguments. Returns (status, result 1, result 2).
#[cfg(target_arch = "aarch64")]
pub fn raw(n: u64, a: [u64; 6]) -> (u64, u64, u64) {
    let entry = CALL.load(Ordering::Acquire);
    let (status, r1, r2);
    // SAFETY: the system page's `call` follows note 0007's convention: it changes only x0 to
    // x2, x8, x16 and x17 (and x30, which `blr` sets).
    unsafe {
        core::arch::asm!(
            "blr {entry}",
            entry = in(reg) entry,
            inlateout("x8") n => _,
            inlateout("x0") a[0] => status,
            inlateout("x1") a[1] => r1,
            inlateout("x2") a[2] => r2,
            in("x3") a[3],
            in("x4") a[4],
            in("x5") a[5],
            out("x16") _,
            out("x17") _,
            out("x30") _,
        );
    }
    (status, r1, r2)
}

/// Makes system call `n`, turning a non-zero status into an [`Error`].
pub fn call(n: u64, a: [u64; 6]) -> Result<(u64, u64)> {
    let (s, r1, r2) = raw(n, a);
    if s == 0 { Ok((r1, r2)) } else { Err(Error::from_code(s)) }
}

/// The monotonic clock, in nanoseconds since start-up, from the system page.
pub fn clock_now() -> u64 {
    let f = CLOCK.load(Ordering::Acquire);
    // SAFETY: the system page's `clock_now` is an ordinary C-convention function (note 0007).
    let clock: extern "C" fn() -> u64 = unsafe { core::mem::transmute(f as usize) };
    clock()
}

/// The kernel's interface version: (major, minor).
pub fn version() -> (u64, u64) {
    call(num::VERSION, [0; 6]).unwrap_or((0, 0))
}

/// Writes `text` to the kernel log through `log` (a handle with the write right).
pub fn log_write(log: u32, text: &str) -> Result<()> {
    call(num::LOG_WRITE, [log.into(), text.as_ptr() as u64, text.len() as u64, 0, 0, 0]).map(|_| ())
}

/// Ends the whole process with `status`.
pub fn process_exit(status: u64) -> ! {
    let _ = raw(num::PROCESS_EXIT, [status, 0, 0, 0, 0, 0]);
    // The kernel never returns from this; if it somehow did, don't run on.
    loop {
        let _ = raw(num::THREAD_EXIT, [0; 6]);
    }
}

/// Ends the calling thread.
pub fn thread_exit() -> ! {
    loop {
        let _ = raw(num::THREAD_EXIT, [0; 6]);
    }
}
