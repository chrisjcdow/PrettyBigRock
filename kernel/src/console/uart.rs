// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Serial ports for the early console: memory-mapped ones are the same on every architecture.
//! (The x86 I/O-port 16550 lives in `arch::x86_64`.)

use core::ptr::{read_volatile, write_volatile};

/// An Arm PL011 UART, already set up by the firmware.
pub struct Pl011 {
    base: *mut u32,
}

impl Pl011 {
    /// # Safety
    /// `base` must be the UART's registers, mapped as device memory, used by nothing else.
    pub unsafe fn new(base: u64) -> Self {
        Self { base: base as *mut u32 }
    }

    /// Sends one byte, waiting while the transmit queue is full.
    pub fn write_byte(&mut self, b: u8) {
        // SAFETY: `new`'s caller promised these are the UART's registers: data at 0x00, flags
        // at 0x18 (bit 5: transmit queue full).
        unsafe {
            while read_volatile(self.base.add(0x18 / 4)) & (1 << 5) != 0 {
                core::hint::spin_loop();
            }
            write_volatile(self.base, u32::from(b));
        }
    }
}

/// A memory-mapped 16550-style UART.
pub struct Mmio16550 {
    base: *mut u8,
    stride: usize,
}

impl Mmio16550 {
    /// # Safety
    /// As for [`Pl011::new`]; `stride` is the distance between registers in bytes.
    pub unsafe fn new(base: u64, stride: usize) -> Self {
        Self { base: base as *mut u8, stride: stride.max(1) }
    }

    /// Sends one byte, waiting until the transmitter is ready.
    pub fn write_byte(&mut self, b: u8) {
        // SAFETY: as promised to `new`: register 0 is data, register 5 is line status (bit 5:
        // ready to transmit).
        unsafe {
            while read_volatile(self.base.add(5 * self.stride)) & (1 << 5) == 0 {
                core::hint::spin_loop();
            }
            write_volatile(self.base, b);
        }
    }
}
