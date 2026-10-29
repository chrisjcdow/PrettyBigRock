// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What every PrettyBigRock loader shares (docs/design/0004-loaders-and-boot-disk.md).
//!
//! Everything here except [`handover`] is ordinary logic that is tested on the host. The
//! loaders supply the firmware-specific parts: where files come from, and where memory comes
//! from.

#![cfg_attr(not(test), no_std)]

pub mod acpi;
pub mod elf;
pub mod fdt;
#[cfg(not(test))]
pub mod handover;
pub mod memmap;
pub mod paging;

/// The size of a page: 4 KiB on every architecture PrettyBigRock supports.
pub const PAGE: u64 = 4096;

/// The kernel's virtual address (docs/design/0003-boot-information.md).
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Where every loader puts the direct map of physical memory.
pub const DIRECT_MAP_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Rounds `x` down to a multiple of `align` (a power of two).
pub const fn align_down(x: u64, align: u64) -> u64 {
    x & !(align - 1)
}

/// Rounds `x` up to a multiple of `align` (a power of two), saturating at the top.
pub const fn align_up(x: u64, align: u64) -> u64 {
    match x.checked_add(align - 1) {
        Some(v) => v & !(align - 1),
        None => !(align - 1),
    }
}

/// Reads physical memory. Loaders run with physical memory identity-mapped, so theirs simply
/// reads through a pointer; tests use a fake.
pub trait PhysMemory {
    /// Copies `buf.len()` bytes starting at physical address `addr` into `buf`. Returns false if
    /// that memory can't be read.
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool;

    /// Reads a little-endian `u32`.
    fn read_u32(&self, addr: u64) -> Option<u32> {
        let mut b = [0; 4];
        self.read(addr, &mut b).then(|| u32::from_le_bytes(b))
    }

    /// Reads a little-endian `u64`.
    fn read_u64(&self, addr: u64) -> Option<u64> {
        let mut b = [0; 8];
        self.read(addr, &mut b).then(|| u64::from_le_bytes(b))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment() {
        assert_eq!(align_down(0x1234, PAGE), 0x1000);
        assert_eq!(align_up(0x1234, PAGE), 0x2000);
        assert_eq!(align_up(0x2000, PAGE), 0x2000);
        assert_eq!(align_up(u64::MAX - 5, PAGE), !(PAGE - 1));
    }
}
