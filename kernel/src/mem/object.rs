// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Memory objects (docs/design/0006-objects-and-capabilities.md, section 6): a whole number of
//! pages that can be mapped into processes. In M3 every page is allocated and zeroed when the
//! object is made, and charged to its creator's budget.

use super::{PAGE, phys_ptr, pmm};
use crate::obj::Error;
use crate::obj::budget::{Budget, Charge};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The largest memory object, in bytes.
pub const MAX_SIZE: u64 = 4 << 30;

/// A set of pages.
pub struct MemoryObject {
    pages: Vec<u64>,
    _charge: Option<Charge>,
}

impl MemoryObject {
    /// A new object of `size` bytes (rounded up to whole pages), zeroed, charged to `budget`.
    pub fn new(size: u64, budget: &Arc<Budget>) -> Result<Arc<Self>, Error> {
        if size == 0 {
            return Err(Error::InvalidArgument);
        }
        if size > MAX_SIZE {
            return Err(Error::TooBig);
        }
        let count = size.div_ceil(PAGE);
        let charge = budget.charge(count * PAGE)?;
        Self::allocate(count, Some(charge))
    }

    /// An object the kernel itself owns, charged to nobody (the system pages).
    pub fn kernel(pages: u64) -> Result<Arc<Self>, Error> {
        Self::allocate(pages, None)
    }

    fn allocate(count: u64, charge: Option<Charge>) -> Result<Arc<Self>, Error> {
        let mut pages = Vec::new();
        pages.try_reserve_exact(count as usize).map_err(|_| Error::OutOfMemory)?;
        for _ in 0..count {
            match pmm::alloc_page() {
                Some(p) => pages.push(p),
                None => {
                    for p in pages {
                        pmm::free(p, 1);
                    }
                    return Err(Error::OutOfMemory);
                }
            }
        }
        Ok(Arc::new(MemoryObject { pages, _charge: charge }))
    }

    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.pages.len() as u64 * PAGE
    }

    /// The physical address of page `i`.
    pub fn page(&self, i: u64) -> Option<u64> {
        self.pages.get(i as usize).copied()
    }

    fn check(&self, offset: u64, len: u64) -> Result<(), Error> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.size() { Err(Error::InvalidArgument) } else { Ok(()) }
    }

    /// Copies `buf.len()` bytes from `offset` into `buf`.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.check(offset, buf.len() as u64)?;
        let mut done = 0usize;
        while done < buf.len() {
            let at = offset + done as u64;
            let in_page = (PAGE - at % PAGE).min((buf.len() - done) as u64) as usize;
            let src = self.pages[(at / PAGE) as usize] + at % PAGE;
            // SAFETY: the page is this object's, reached through the direct map, and the range
            // stays inside it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_ptr::<u8>(src),
                    buf[done..].as_mut_ptr(),
                    in_page,
                )
            };
            done += in_page;
        }
        Ok(())
    }

    /// Copies `data` into the object at `offset`.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), Error> {
        self.check(offset, data.len() as u64)?;
        let mut done = 0usize;
        while done < data.len() {
            let at = offset + done as u64;
            let in_page = (PAGE - at % PAGE).min((data.len() - done) as u64) as usize;
            let dst = self.pages[(at / PAGE) as usize] + at % PAGE;
            // SAFETY: as for `read`.
            unsafe {
                core::ptr::copy_nonoverlapping(data[done..].as_ptr(), phys_ptr::<u8>(dst), in_page)
            };
            done += in_page;
        }
        Ok(())
    }
}

impl Drop for MemoryObject {
    fn drop(&mut self) {
        for &p in &self.pages {
            pmm::free(p, 1);
        }
    }
}
