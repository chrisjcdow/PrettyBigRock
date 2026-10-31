// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Kernel objects and capabilities (docs/design/0006-objects-and-capabilities.md).
//!
//! - [`Object`] is a reference to anything the kernel manages.
//! - [`table`] is each process's capability table: handles to (object, rights, badge).
//! - [`caps`] is the kernel-wide derivation tree, and copying, moving, closing and revoking.
//! - The objects themselves: [`process`], [`endpoint`] (with replies and message delivery),
//!   [`notification`], and memory objects in [`crate::mem::object`].

pub mod budget;
pub mod caps;
pub mod endpoint;
pub mod notification;
pub mod process;
pub mod table;

use crate::mem::object::MemoryObject;
use crate::sched::Thread;
use alloc::sync::Arc;

/// The error codes of docs/design/0007-system-calls.md, section 5. Never renumbered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// No such call number.
    InvalidCall = 1,
    /// The handle isn't valid in this process.
    BadHandle = 2,
    /// The handle refers to the wrong kind of object.
    WrongType = 3,
    /// The capability lacks a right the call needs.
    NoRight = 4,
    /// An argument is out of range, or a reserved bit isn't zero.
    InvalidArgument = 5,
    /// Memory the call had to read or write isn't mapped with that permission.
    BadAddress = 6,
    /// The budget can't cover it.
    OutOfMemory = 7,
    /// The deadline passed.
    TimedOut = 8,
    /// The other side of a message has gone.
    PeerGone = 9,
    /// A capability in use was destroyed while the call waited.
    Revoked = 10,
    /// A message, buffer or request exceeds a limit.
    TooBig = 11,
    /// The receiving capability table has no room.
    TableFull = 12,
    /// Something is already there.
    AlreadyExists = 13,
    /// The object isn't in a state that allows this.
    BadState = 14,
    /// The call can't be done here.
    NotSupported = 15,
}

/// Object type numbers (note 0006, section 1). Never reused.
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

/// Rights (note 0006, section 4). Bits 0 to 7 mean the same for every type; 8 to 15 depend
/// on the type; 16 to 31 are reserved.
pub mod rights {
    /// May be copied.
    pub const DUPLICATE: u32 = 1 << 0;
    /// May be sent in a message.
    pub const TRANSFER: u32 = 1 << 1;
    /// May revoke the copies derived from it.
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

    /// The rights every type shares.
    pub const COMMON: u32 = DUPLICATE | TRANSFER | REVOKE | INSPECT;
    /// Every right defined for any type.
    pub const ALL_DEFINED: u32 = 0x07FF;
}

/// The kernel's charges for its own structures, in bytes, rounded up so they're never less
/// than the real cost (note 0006, section 9). Reported by `process_info`.
pub mod charge {
    /// A thread: its 128 KiB kernel stack, its message page, its saved registers and the rest.
    pub const THREAD: u64 = 128 * 1024 + 4096 + 4096 + 2048;
    /// An endpoint.
    pub const ENDPOINT: u64 = 512;
    /// A notification.
    pub const NOTIFICATION: u64 = 256;
    /// A reply capability.
    pub const REPLY: u64 = 128;
    /// A process, besides its threads, table and page tables.
    pub const PROCESS: u64 = 4096 + 1024;
    /// Each slot of a capability table.
    pub const TABLE_SLOT: u64 = 64;
    /// Each page-table page.
    pub const PAGE_TABLE: u64 = 4096;
}

/// A reference to a kernel object.
#[derive(Clone)]
pub enum Object {
    /// A process.
    Process(Arc<process::Process>),
    /// A thread.
    Thread(Arc<Thread>),
    /// A memory object.
    Memory(Arc<MemoryObject>),
    /// An endpoint.
    Endpoint(Arc<endpoint::Endpoint>),
    /// A notification.
    Notification(Arc<notification::Notification>),
    /// A one-time reply capability.
    Reply(Arc<endpoint::Reply>),
    /// The kernel log.
    Log,
}

impl Object {
    /// The object's type number.
    pub fn kind(&self) -> u32 {
        match self {
            Object::Process(_) => kind::PROCESS,
            Object::Thread(_) => kind::THREAD,
            Object::Memory(_) => kind::MEMORY,
            Object::Endpoint(_) => kind::ENDPOINT,
            Object::Notification(_) => kind::NOTIFICATION,
            Object::Reply(_) => kind::REPLY,
            Object::Log => kind::LOG,
        }
    }

    /// Every right that means something for this type: what a newly created object's
    /// capability has.
    pub fn all_rights(&self) -> u32 {
        use rights::*;
        match self {
            Object::Process(_) | Object::Thread(_) => COMMON | MANAGE | STOP | WATCH,
            Object::Memory(_) => COMMON | READ | WRITE | EXECUTE | MAP,
            Object::Endpoint(_) => COMMON | SEND | RECEIVE | ENDPOINT_WATCH,
            Object::Notification(_) => COMMON | SIGNAL | WAIT,
            Object::Reply(_) => 0,
            Object::Log => COMMON | WRITE,
        }
    }

    /// True if `self` and `other` are the same object.
    pub fn same(&self, other: &Object) -> bool {
        match (self, other) {
            (Object::Process(a), Object::Process(b)) => Arc::ptr_eq(a, b),
            (Object::Thread(a), Object::Thread(b)) => Arc::ptr_eq(a, b),
            (Object::Memory(a), Object::Memory(b)) => Arc::ptr_eq(a, b),
            (Object::Endpoint(a), Object::Endpoint(b)) => Arc::ptr_eq(a, b),
            (Object::Notification(a), Object::Notification(b)) => Arc::ptr_eq(a, b),
            (Object::Reply(a), Object::Reply(b)) => Arc::ptr_eq(a, b),
            (Object::Log, Object::Log) => true,
            _ => false,
        }
    }
}

/// Checks a capability has `needed` rights.
pub fn require(rights: u32, needed: u32) -> Result<(), Error> {
    if rights & needed == needed { Ok(()) } else { Err(Error::NoRight) }
}
