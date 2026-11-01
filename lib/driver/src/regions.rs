// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Mapped device memory and DMA memory behind the [`Registers`] trait.

use core::ptr::{read_volatile, write_volatile};
use granite::memory::Mapping;
use pbr_devices::pci::Config;
use pbr_devices::regs::Registers;

/// Checks an access of `width` bytes at `offset` in a block of `len`.
#[track_caller]
fn check(offset: usize, width: usize, len: usize) {
    assert!(
        offset.is_multiple_of(width) && offset.checked_add(width).is_some_and(|e| e <= len),
        "device access of {width} bytes at {offset:#x} outside or misaligned in a block of {len:#x}",
    );
}

macro_rules! volatile_methods {
    () => {
        fn len(&self) -> usize {
            self.mapping.len() as usize
        }
        fn read8(&self, offset: usize) -> u8 {
            check(offset, 1, self.len());
            // SAFETY: inside the mapping (just checked), which stays mapped while `self` lives.
            unsafe { read_volatile(self.mapping.as_ptr().add(offset)) }
        }
        fn read16(&self, offset: usize) -> u16 {
            check(offset, 2, self.len());
            // SAFETY: as for `read8`; aligned (just checked).
            unsafe { read_volatile(self.mapping.as_ptr().add(offset).cast::<u16>()) }
        }
        fn read32(&self, offset: usize) -> u32 {
            check(offset, 4, self.len());
            // SAFETY: as for `read16`.
            unsafe { read_volatile(self.mapping.as_ptr().add(offset).cast::<u32>()) }
        }
        fn read64(&self, offset: usize) -> u64 {
            check(offset, 8, self.len());
            // SAFETY: as for `read16`.
            unsafe { read_volatile(self.mapping.as_ptr().add(offset).cast::<u64>()) }
        }
        fn write8(&self, offset: usize, value: u8) {
            check(offset, 1, self.len());
            // SAFETY: as for `read8`; the mapping is writable.
            unsafe { write_volatile(self.mapping.as_mut_ptr().add(offset), value) }
        }
        fn write16(&self, offset: usize, value: u16) {
            check(offset, 2, self.len());
            // SAFETY: as for `write8`; aligned.
            unsafe { write_volatile(self.mapping.as_mut_ptr().add(offset).cast::<u16>(), value) }
        }
        fn write32(&self, offset: usize, value: u32) {
            check(offset, 4, self.len());
            // SAFETY: as for `write16`.
            unsafe { write_volatile(self.mapping.as_mut_ptr().add(offset).cast::<u32>(), value) }
        }
        fn write64(&self, offset: usize, value: u64) {
            check(offset, 8, self.len());
            // SAFETY: as for `write16`.
            unsafe { write_volatile(self.mapping.as_mut_ptr().add(offset).cast::<u64>(), value) }
        }
    };
}

/// Mapped device registers (a BAR, or a function's configuration space).
#[derive(Debug)]
pub struct Mmio {
    mapping: Mapping,
}

impl Mmio {
    /// Wraps a mapping of device memory.
    pub fn new(mapping: Mapping) -> Mmio {
        Mmio { mapping }
    }
}

impl Registers for Mmio {
    volatile_methods!();
}

/// The driver's DMA memory: ordinary (cached) memory the device reads and writes too, and the
/// bus address the device uses for its first byte.
#[derive(Debug)]
pub struct Dma {
    mapping: Mapping,
    bus: u64,
}

impl Dma {
    /// Wraps a mapping of DMA memory whose first byte the device reaches at `bus`.
    pub fn new(mapping: Mapping, bus: u64) -> Dma {
        Dma { mapping, bus }
    }

    /// The bus address of the first byte.
    pub fn bus(&self) -> u64 {
        self.bus
    }
}

impl Registers for Dma {
    volatile_methods!();

    fn read_bytes(&self, offset: usize, out: &mut [u8]) {
        self.copy_out(offset, out)
    }

    fn write_bytes(&self, offset: usize, data: &[u8]) {
        self.copy_in(offset, data)
    }
}

impl Dma {
    /// Copies bytes out of the memory (a plain copy: the memory is ordinary RAM).
    pub fn copy_out(&self, offset: usize, out: &mut [u8]) {
        assert!(
            offset.checked_add(out.len()).is_some_and(|e| e <= self.len()),
            "DMA copy out of range"
        );
        // SAFETY: the range is inside the mapping (just checked), and `out` is a different
        // allocation from the mapping.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.mapping.as_ptr().add(offset),
                out.as_mut_ptr(),
                out.len(),
            )
        }
    }

    /// Copies bytes into the memory.
    pub fn copy_in(&self, offset: usize, data: &[u8]) {
        assert!(
            offset.checked_add(data.len()).is_some_and(|e| e <= self.len()),
            "DMA copy out of range"
        );
        // SAFETY: as for `copy_out`; the mapping is writable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.mapping.as_mut_ptr().add(offset),
                data.len(),
            )
        }
    }
}

/// A function's 4 KiB of configuration space, as a [`Config`].
#[derive(Debug)]
pub struct ConfigPage(pub Mmio);

impl Config for ConfigPage {
    fn read(&mut self, offset: u16) -> u32 {
        if usize::from(offset) + 4 > self.0.len() {
            return u32::MAX;
        }
        self.0.read32(usize::from(offset))
    }

    fn write(&mut self, offset: u16, value: u32) {
        if usize::from(offset) + 4 <= self.0.len() {
            self.0.write32(usize::from(offset), value);
        }
    }
}
