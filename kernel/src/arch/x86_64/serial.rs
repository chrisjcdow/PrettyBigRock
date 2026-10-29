// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A 16550-style UART on I/O ports, for the early console (COM1 is at 0x3F8).

unsafe fn outb(port: u16, v: u8) {
    // SAFETY: the caller promises the port belongs to the UART.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) };
}

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: as for `outb`.
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) };
    v
}

/// A 16550-style UART on I/O ports.
pub struct Port16550 {
    port: u16,
}

impl Port16550 {
    /// Sets the UART up at 115200 baud, 8 data bits, no parity, one stop bit. Returns `None`
    /// if there's no UART at `port` (its scratch register doesn't hold a value).
    pub fn new(port: u16) -> Option<Self> {
        // SAFETY: these are the standard 16550 registers at the port the loader reported.
        unsafe {
            outb(port + 7, 0xA5);
            if inb(port + 7) != 0xA5 {
                return None;
            }
            outb(port + 1, 0x00); // no interrupts
            outb(port + 3, 0x80); // set the speed...
            outb(port, 0x01); // ...divisor 1: 115200 baud
            outb(port + 1, 0x00);
            outb(port + 3, 0x03); // 8N1
            outb(port + 2, 0xC7); // FIFOs on, cleared
            outb(port + 4, 0x03); // DTR and RTS
        }
        Some(Self { port })
    }

    /// Sends one byte, waiting until the transmitter is ready.
    pub fn write_byte(&mut self, b: u8) {
        // SAFETY: `new` found a UART at this port.
        unsafe {
            for _ in 0..100_000 {
                if inb(self.port + 5) & 0x20 != 0 {
                    break;
                }
            }
            outb(self.port, b);
        }
    }
}
