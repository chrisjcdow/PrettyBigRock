// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The early console: where the kernel's messages go before there are any drivers. Every
//! message goes to the serial port (if there is one) and to the first screen (if there is one).

mod fb;
mod font;
mod uart;

use crate::sync::SpinLock;
use core::fmt::{self, Write};
use pbr_bootinfo::BootInfo;

enum Serial {
    None,
    Pl011(uart::Pl011),
    Mmio16550(uart::Mmio16550),
    #[cfg(target_arch = "x86_64")]
    Port16550(crate::arch::Port16550),
}

impl Serial {
    fn write_byte(&mut self, b: u8) {
        match self {
            Serial::None => {}
            Serial::Pl011(u) => u.write_byte(b),
            Serial::Mmio16550(u) => u.write_byte(b),
            #[cfg(target_arch = "x86_64")]
            Serial::Port16550(u) => u.write_byte(b),
        }
    }
}

struct Console {
    serial: Serial,
    screen: Option<fb::FbConsole>,
}

// SAFETY: the console's raw pointers are to device memory that only the console uses, and the
// lock around it lets one core at a time near it.
unsafe impl Send for Console {}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.serial.write_byte(b'\r');
            }
            self.serial.write_byte(b);
        }
        if let Some(screen) = &mut self.screen {
            screen.write_str(s);
        }
        Ok(())
    }
}

static CONSOLE: SpinLock<Console> = SpinLock::new(Console { serial: Serial::None, screen: None });

/// Sets up the console from the boot information.
///
/// # Safety
/// Call once, early, with the boot information the loader gave; `direct_map_base` must be
/// where the loader mapped physical memory.
pub unsafe fn init(info: &BootInfo, direct_map_base: u64) {
    let ec = info.early_console;
    let serial = match ec.kind {
        // SAFETY: the loader described this UART and mapped it in the direct map as device
        // memory; nothing else uses it.
        3 => Serial::Pl011(unsafe { uart::Pl011::new(direct_map_base + ec.address) }),
        2 => {
            let base = direct_map_base + ec.address;
            // SAFETY: as for the PL011.
            Serial::Mmio16550(unsafe { uart::Mmio16550::new(base, ec.register_width as usize) })
        }
        #[cfg(target_arch = "x86_64")]
        1 => crate::arch::Port16550::new(ec.address as u16).map_or(Serial::None, Serial::Port16550),
        _ => Serial::None,
    };
    let screen = info.framebuffers().first().and_then(|f| {
        // SAFETY: the loader mapped every framebuffer in the direct map, writable.
        unsafe { fb::FbConsole::new(*f, direct_map_base + f.memory.base) }
    });
    let mut c = CONSOLE.lock();
    c.serial = serial;
    c.screen = screen;
}

/// Used by [`kprint!`] and [`kprintln!`].
pub fn print(args: fmt::Arguments<'_>) {
    let _ = CONSOLE.lock().write_fmt(args);
}

/// Prints to the console.
#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

/// Prints a line to the console.
#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::console::print(format_args!("{}\n", format_args!($($arg)*))) };
}
