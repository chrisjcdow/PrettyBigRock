// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What every PrettyBigRock loader shares (docs/design/0004-loaders-and-boot-disk.md).
//!
//! Everything here except [`handover`] is ordinary logic that is tested on the host. The
//! loaders supply the firmware-specific parts: where files come from, and where memory comes
//! from.

#![cfg_attr(not(test), no_std)]

pub mod elf;
#[cfg(not(test))]
pub mod handover;
pub mod memmap;
pub mod tables;

pub use pbr_platform::{
    DIRECT_MAP_BASE, KERNEL_BASE, PAGE, PhysMemory, acpi, align_down, align_up, fdt, paging,
};

/// Physical memory reached through an identity map, as the loaders see it.
pub struct IdentityMapped;

impl PhysMemory for IdentityMapped {
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        if addr == 0 {
            return false;
        }
        // SAFETY: loaders run with all of physical memory identity-mapped (UEFI guarantees
        // this; the BIOS loader maps the first 4 GiB), and callers only pass addresses taken
        // from firmware tables.
        unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, buf.as_mut_ptr(), buf.len()) };
        true
    }
}
