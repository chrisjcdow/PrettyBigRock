// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Memory objects and mappings (docs/design/0006-objects-and-capabilities.md, section 6).

use crate::handle::Handle;
use crate::sys::{self, Result, num};

/// A memory object.
#[derive(Debug)]
pub struct Memory(pub Handle);

impl Memory {
    /// A new memory object of `size` bytes (rounded up to whole pages), zeroed.
    pub fn create(size: u64) -> Result<Memory> {
        sys::call(num::MEMORY_CREATE, [size, 0, 0, 0, 0, 0])
            .map(|(h, _)| Memory(Handle::from_raw(h as u32)))
    }

    /// Its size in bytes.
    pub fn size(&self) -> Result<u64> {
        sys::call(num::MEMORY_INFO, [self.0.raw().into(), 0, 0, 0, 0, 0]).map(|(s, _)| s)
    }

    /// Copies `buf.len()` bytes from `offset` into `buf`.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let a = [self.0.raw().into(), offset, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0];
        sys::call(num::MEMORY_READ, a).map(|_| ())
    }

    /// Copies `data` into the object at `offset`.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<()> {
        let a = [self.0.raw().into(), offset, data.as_ptr() as u64, data.len() as u64, 0, 0];
        sys::call(num::MEMORY_WRITE, a).map(|_| ())
    }

    /// Maps the whole object into this program at an address the kernel chooses, with
    /// permissions `perms` (see `sys::perm`).
    pub fn map(&self, perms: u64) -> Result<Mapping> {
        let len = self.size()?;
        let addr = self.map_into(crate::process::self_handle(), 0, len, 0, perms)?;
        Ok(Mapping { addr, len })
    }

    /// Maps `len` bytes from `offset` into the process `process` (a handle with the manage
    /// right), at `addr` (0 to let the kernel choose). Returns where.
    pub fn map_into(
        &self,
        process: u32,
        offset: u64,
        len: u64,
        addr: u64,
        perms: u64,
    ) -> Result<u64> {
        let a = [process.into(), self.0.raw().into(), offset, len, addr, perms];
        sys::call(num::MEMORY_MAP, a).map(|(at, _)| at)
    }
}

/// Memory mapped into this program. Unmapped when dropped.
#[derive(Debug)]
pub struct Mapping {
    addr: u64,
    len: u64,
}

impl Mapping {
    /// Where it is.
    pub fn addr(&self) -> u64 {
        self.addr
    }

    /// Its length in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// True if it's empty (never, in practice).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The memory, to read. Another program sharing the object may change it at any moment:
    /// copy before checking (note 0008, section 6).
    pub fn as_ptr(&self) -> *const u8 {
        self.addr as *const u8
    }

    /// The memory, to write.
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.addr as *mut u8
    }

    /// Unmaps it now.
    pub fn unmap(self) -> Result<()> {
        let r = unmap(self.addr, self.len);
        core::mem::forget(self);
        r
    }

    /// Keeps it mapped for ever, returning its address.
    pub fn leak(self) -> u64 {
        let a = self.addr;
        core::mem::forget(self);
        a
    }
}

fn unmap(addr: u64, len: u64) -> Result<()> {
    let a = [crate::process::self_handle().into(), addr, len, 0, 0, 0];
    sys::call(num::MEMORY_UNMAP, a).map(|_| ())
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = unmap(self.addr, self.len);
    }
}

/// Changes the permissions of the mappings exactly covering `[addr, addr + len)` in
/// `process`.
pub fn protect(process: u32, addr: u64, len: u64, perms: u64) -> Result<()> {
    sys::call(num::MEMORY_PROTECT, [process.into(), addr, len, perms, 0, 0]).map(|_| ())
}
