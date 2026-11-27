// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Device memory, interrupts, I/O ports and DMA (docs/design/0010-device-access.md): what a
//! driver needs to drive a device it was given.

use crate::handle::Handle;
use crate::memory::{Mapping, Memory};
use crate::notification::Notification;
use crate::sys::{self, Result, num};

/// The kinds of interrupt range (`interrupt_info`).
pub mod interrupt_kind {
    /// Real interrupt lines.
    pub const WIRED: u32 = 0;
    /// The pool message-signalled interrupts are made from.
    pub const POOL: u32 = 1;
    /// One message-signalled interrupt.
    pub const MSI: u32 = 2;
}

/// Ask for memory below 4 GiB (`DeviceMemory::create_dma`).
pub const BELOW_4G: u64 = 1;

/// A range of device memory: physical addresses that aren't RAM.
#[derive(Debug)]
pub struct DeviceMemory(pub Handle);

impl DeviceMemory {
    /// The part of this range from `offset`, `length` bytes (both multiples of the page size),
    /// with only the rights in `mask`.
    pub fn slice(&self, offset: u64, length: u64, mask: u32) -> Result<DeviceMemory> {
        let a = [self.0.raw().into(), offset, length, mask.into(), 0, 0];
        sys::call(num::DEVICE_MEMORY_SLICE, a)
            .map(|(h, _)| DeviceMemory(Handle::from_raw(h as u32)))
    }

    /// Where the range is: (physical address, length).
    pub fn info(&self) -> Result<(u64, u64)> {
        sys::call(num::DEVICE_MEMORY_INFO, [self.0.raw().into(), 0, 0, 0, 0, 0])
    }

    /// Maps `len` bytes from `offset` into this program, at an address the kernel chooses.
    /// `perms` is from `sys::perm`; add `sys::perm::WRITE_COMBINING` for a framebuffer.
    pub fn map(&self, offset: u64, len: u64, perms: u64) -> Result<Mapping> {
        let me = crate::process::self_handle();
        let a = [me.into(), self.0.raw().into(), offset, len, 0, perms];
        sys::call(num::MEMORY_MAP, a).map(|(at, _)| Mapping::from_raw(at, len))
    }

    /// Maps `len` bytes from `offset` into `process` (a handle with the manage right) at
    /// `addr` (0 to let the kernel choose). Returns where.
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

    /// Creates `size` bytes of physically contiguous memory a device may read and write (needs
    /// the `DMA` right). `flags` is `BELOW_4G` or 0. The memory has the `BUS` right.
    pub fn create_dma(&self, size: u64, flags: u64) -> Result<Memory> {
        let a = [self.0.raw().into(), size, flags, 0, 0, 0];
        sys::call(num::MEMORY_CREATE_DMA, a).map(|(h, _)| Memory(Handle::from_raw(h as u32)))
    }
}

/// The address a device uses for the page of `memory` at `offset` (needs `BUS` on the memory
/// and `DMA` on the device).
pub fn bus_address(memory: &Memory, device: &DeviceMemory, offset: u64) -> Result<u64> {
    let a = [memory.0.raw().into(), device.0.raw().into(), offset, 0, 0, 0];
    sys::call(num::MEMORY_BUS_ADDRESS, a).map(|(addr, _)| addr)
}

/// A range of interrupt numbers.
#[derive(Debug)]
pub struct Interrupts(pub Handle);

impl Interrupts {
    /// The numbers from `first`, `count` of them, with only the rights in `mask`.
    pub fn slice(&self, first: u64, count: u64, mask: u32) -> Result<Interrupts> {
        let a = [self.0.raw().into(), first, count, mask.into(), 0, 0];
        sys::call(num::INTERRUPT_SLICE, a).map(|(h, _)| Interrupts(Handle::from_raw(h as u32)))
    }

    /// What the range is: (kind, how many, first number).
    pub fn info(&self) -> Result<(u32, u32, u64)> {
        sys::call(num::INTERRUPT_INFO, [self.0.raw().into(), 0, 0, 0, 0, 0])
            .map(|(kc, first)| (kc as u32, (kc >> 32) as u32, first))
    }

    /// Delivers interrupt number `index` of the range as `bits` signalled on `n`. The
    /// interrupt stays masked after it fires until [`Interrupts::acknowledge`].
    pub fn bind(&self, index: u64, n: &Notification, bits: u64) -> Result<()> {
        let a = [self.0.raw().into(), index, n.0.raw().into(), bits, 0, 0];
        sys::call(num::INTERRUPT_BIND, a).map(|_| ())
    }

    /// Releases the binding of interrupt `index`.
    pub fn unbind(&self, index: u64) -> Result<()> {
        sys::call(num::INTERRUPT_UNBIND, [self.0.raw().into(), index, 0, 0, 0, 0]).map(|_| ())
    }

    /// Says this process has handled interrupt `index`. Its own binding is no longer owed an
    /// acknowledgement, and once no binding is (several processes may share a level-triggered
    /// line, interface 1.4), the interrupt may fire again. `BAD_STATE` if this process hasn't
    /// bound it.
    pub fn acknowledge(&self, index: u64) -> Result<()> {
        sys::call(num::INTERRUPT_ACKNOWLEDGE, [self.0.raw().into(), index, 0, 0, 0, 0]).map(|_| ())
    }

    /// From a message-signalled pool: a new interrupt.
    pub fn msi_allocate(&self) -> Result<Interrupts> {
        sys::call(num::INTERRUPT_MSI_ALLOCATE, [self.0.raw().into(), 0, 0, 0, 0, 0])
            .map(|(h, _)| Interrupts(Handle::from_raw(h as u32)))
    }

    /// For a message-signalled interrupt: (address, data) to program into the device.
    pub fn msi_info(&self) -> Result<(u64, u64)> {
        sys::call(num::INTERRUPT_MSI_INFO, [self.0.raw().into(), 0, 0, 0, 0, 0])
    }

    /// Sets line `index`'s trigger and polarity (needs `CONFIGURE`): `mode` is
    /// [`LEVEL`](line_mode::LEVEL) and [`ACTIVE_LOW`](line_mode::ACTIVE_LOW) or'd together, or 0
    /// for edge-triggered and active high. Only while nobody has it bound (interface 1.4).
    pub fn configure(&self, index: u64, mode: u64) -> Result<()> {
        sys::call(num::INTERRUPT_CONFIGURE, [self.0.raw().into(), index, mode, 0, 0, 0]).map(|_| ())
    }

    /// Makes interrupt `index` fire by software. Only in self-test runs. Returns true, and
    /// triggers nothing, if it is masked waiting for another sharer's acknowledgement.
    pub fn trigger(&self, index: u64) -> Result<bool> {
        sys::call(num::INTERRUPT_TRIGGER, [self.0.raw().into(), index, 0, 0, 0, 0])
            .map(|(masked, _)| masked != 0)
    }
}

/// The `mode` bits of [`Interrupts::configure`].
pub mod line_mode {
    /// Level-triggered (otherwise edge-triggered).
    pub const LEVEL: u64 = 1 << 0;
    /// Active low (otherwise active high).
    pub const ACTIVE_LOW: u64 = 1 << 1;
}

/// A range of I/O ports (x86-64 only).
#[derive(Debug)]
pub struct IoPorts(pub Handle);

impl IoPorts {
    /// `count` ports from port number `first`, with only the rights in `mask`.
    pub fn slice(&self, first: u64, count: u64, mask: u32) -> Result<IoPorts> {
        let a = [self.0.raw().into(), first, count, mask.into(), 0, 0];
        sys::call(num::IOPORT_SLICE, a).map(|(h, _)| IoPorts(Handle::from_raw(h as u32)))
    }

    /// The range: (first port, how many).
    pub fn info(&self) -> Result<(u64, u64)> {
        sys::call(num::IOPORT_INFO, [self.0.raw().into(), 0, 0, 0, 0, 0])
    }

    /// Reads `width` bytes (1, 2 or 4) from the port `offset` into the range.
    pub fn read(&self, offset: u64, width: u64) -> Result<u64> {
        sys::call(num::IOPORT_IN, [self.0.raw().into(), offset, width, 0, 0, 0]).map(|(v, _)| v)
    }

    /// Writes `width` bytes (1, 2 or 4) to the port `offset` into the range.
    pub fn write(&self, offset: u64, width: u64, value: u64) -> Result<()> {
        sys::call(num::IOPORT_OUT, [self.0.raw().into(), offset, width, value, 0, 0]).map(|_| ())
    }
}

/// Stops the kernel drawing text on the screen (needs `CONTROL` on the kernel log).
pub fn console_screen_release(log: &Handle) -> Result<()> {
    sys::call(num::CONSOLE_SCREEN_RELEASE, [log.raw().into(), 0, 0, 0, 0, 0]).map(|_| ())
}
