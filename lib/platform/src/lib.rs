// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Hardware formats that both the loaders and the kernel need: page tables for each
//! architecture, ACPI tables, and device trees. All of it is plain logic, tested on the host;
//! callers supply how to reach physical memory.

#![cfg_attr(not(test), no_std)]

pub mod acpi;
pub mod fdt;
pub mod paging;

use pbr_bootinfo::{MemoryKind, MemoryRegion, PhysRange};

/// The size of a page: 4 KiB on every architecture PrettyBigRock supports.
pub const PAGE: u64 = 4096;

/// The kernel's virtual address (docs/design/0003-boot-information.md).
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Where physical memory is mapped in the kernel's address space (notes 0003 and 0005).
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

/// True for memory the direct map treats as ordinary RAM.
pub fn is_ram(kind: u32) -> bool {
    use MemoryKind::*;
    matches!(
        MemoryRegion { range: PhysRange::NONE, kind, reserved: 0 }.kind(),
        Some(
            Usable
                | AcpiReclaimable
                | AcpiNvs
                | LoaderReclaimable
                | Kernel
                | Module
                | FirmwareRuntime
        )
    )
}

/// Reads physical memory. Loaders read through their identity map, the kernel through its
/// direct map, and tests through a fake.
pub trait PhysMemory {
    /// Copies `buf.len()` bytes starting at physical address `addr` into `buf`. Returns false if
    /// that memory can't be read.
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool;

    /// Reads a byte.
    fn read_u8(&self, addr: u64) -> Option<u8> {
        let mut b = [0; 1];
        self.read(addr, &mut b).then_some(b[0])
    }

    /// Reads a little-endian `u16`.
    fn read_u16(&self, addr: u64) -> Option<u16> {
        let mut b = [0; 2];
        self.read(addr, &mut b).then(|| u16::from_le_bytes(b))
    }

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

    #[test]
    fn ram_kinds() {
        assert!(is_ram(MemoryKind::Kernel as u32));
        assert!(!is_ram(MemoryKind::Framebuffer as u32));
        assert!(!is_ram(12345));
    }
}
