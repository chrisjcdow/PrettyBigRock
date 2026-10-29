// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! x86-64 (AMD64 / Intel 64).

/// The name used in messages.
pub const NAME: &str = "x86-64";

/// The kernel's entry point. The loader jumps here in 64-bit mode, with paging on, interrupts
/// off, a valid stack, the boot information's physical address in `rdi` and the direct map's
/// base in `rsi` (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64, direct_map_base: u64) -> ! {
    crate::kernel_main(boot_info, direct_map_base)
}

/// Stops this core for good, sleeping until interrupts (which are off, so: for ever).
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this core. It touches no memory and no other state.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Facts about the processor worth reporting at start-up.
pub fn describe(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let mut vendor = [0u8; 12];
    let r = core::arch::x86_64::__cpuid(0);
    vendor[..4].copy_from_slice(&r.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&r.edx.to_le_bytes());
    vendor[8..].copy_from_slice(&r.ecx.to_le_bytes());
    write!(out, "{}", core::str::from_utf8(&vendor).unwrap_or("unknown vendor"))
}

unsafe fn outb(port: u16, v: u8) {
    // SAFETY: the caller promises the port belongs to a device it's driving.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) };
}

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: as for `outb`.
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) };
    v
}

/// A 16550-style UART on I/O ports (COM1 is at 0x3F8).
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
