// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Memory management (docs/design/0005-kernel-core.md, sections 2 to 4): physical pages, the
//! kernel's page tables and virtual areas, and the heap.

pub mod heap;
pub mod object;
pub mod pmm;
pub mod space;
pub mod vm;

use core::sync::atomic::{AtomicU64, Ordering};
pub use pbr_platform::PAGE;

static DIRECT_MAP: AtomicU64 = AtomicU64::new(0);

/// Records where the direct map is (from the loader). Called once, first thing.
pub fn set_direct_map(base: u64) {
    DIRECT_MAP.store(base, Ordering::Relaxed);
}

/// The virtual address of physical address `phys`, through the direct map.
pub fn phys_to_virt(phys: u64) -> u64 {
    DIRECT_MAP.load(Ordering::Relaxed) + phys
}

/// A pointer to physical memory, through the direct map.
pub fn phys_ptr<T>(phys: u64) -> *mut T {
    phys_to_virt(phys) as *mut T
}

/// Reads physical memory through the direct map, for the ACPI and device-tree readers.
pub struct DirectMapped;

impl pbr_platform::PhysMemory for DirectMapped {
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        if addr == 0 {
            return false;
        }
        // SAFETY: callers only pass addresses from firmware tables, which are in memory the
        // direct map covers (note 0003).
        unsafe {
            core::ptr::copy_nonoverlapping(phys_ptr::<u8>(addr), buf.as_mut_ptr(), buf.len())
        };
        true
    }
}
