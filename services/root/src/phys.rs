// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading the ACPI tables (for the power service), which are in physical memory the kernel
//! doesn't manage: through the device memory capability, one page at a time. (The same as the
//! device manager's `phys.rs`: two small copies, rather than a library for one struct.)

use alloc::collections::BTreeMap;
use core::cell::RefCell;
use core::ptr::read_volatile;
use granite::device::DeviceMemory;
use granite::memory::Mapping;
use granite::sys::{perm, rights};
use pbr_platform::PhysMemory;

/// The most pages kept mapped at once.
const MAX_PAGES: usize = 64;

/// Physical memory read through device memory.
pub struct PhysReader<'a> {
    memory: &'a DeviceMemory,
    pages: RefCell<BTreeMap<u64, Mapping>>,
}

impl<'a> PhysReader<'a> {
    /// A reader over `memory` (the capability covering all physical addresses).
    pub fn new(memory: &'a DeviceMemory) -> Self {
        PhysReader { memory, pages: RefCell::new(BTreeMap::new()) }
    }

    /// The address the page at `page` is mapped at, mapping it if needed.
    fn page(&self, page: u64) -> Option<u64> {
        let mut pages = self.pages.borrow_mut();
        if let Some(m) = pages.get(&page) {
            return Some(m.addr());
        }
        if pages.len() >= MAX_PAGES {
            pages.clear();
        }
        let slice = self.memory.slice(page, 4096, rights::READ | rights::MAP).ok()?;
        let mapping = slice.map(0, 4096, perm::READ).ok()?;
        let at = mapping.addr();
        pages.insert(page, mapping);
        Some(at)
    }
}

impl PhysMemory for PhysReader<'_> {
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        let mut done = 0usize;
        while done < buf.len() {
            let Some(a) = addr.checked_add(done as u64) else { return false };
            let (page, within) = (a & !0xFFF, (a & 0xFFF) as usize);
            let n = (4096 - within).min(buf.len() - done);
            let Some(at) = self.page(page) else { return false };
            for i in 0..n {
                // SAFETY: inside the page mapped at `at` (`within + n <= 4096`). Bytes are read one
                // at a time: device memory may not allow wider or unaligned accesses.
                buf[done + i] = unsafe { read_volatile((at as *const u8).add(within + i)) };
            }
            done += n;
        }
        true
    }
}
