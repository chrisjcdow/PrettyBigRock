// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The early console: where the kernel's messages go before there are any drivers. Every
//! message goes to the serial port (if there is one) and to the first screen (if there is one).

mod fb;
mod font;
mod uart;

use crate::sync::{IrqSpinLock, Level};
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
    /// Set when the display stack has taken the screen: text is no longer drawn on it, except
    /// a panic's (note 0010, section 7).
    screen_released: bool,
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
        if !self.screen_released
            && let Some(screen) = &mut self.screen
        {
            screen.write_str(s);
        }
        Ok(())
    }
}

static CONSOLE: IrqSpinLock<Console> = IrqSpinLock::new(
    Level::Console,
    "console",
    Console { serial: Serial::None, screen: None, screen_released: false },
);

static UART_KIND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static UART_ADDRESS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The physical address and length of the early console's registers, if it's memory-mapped:
/// no program may be given them (docs/design/0010-device-access.md, section 1).
pub fn uart_registers() -> Option<(u64, u64)> {
    use core::sync::atomic::Ordering::Relaxed;
    matches!(UART_KIND.load(Relaxed), 2 | 3).then(|| (UART_ADDRESS.load(Relaxed), 0x1000))
}

/// The I/O port of the early console, if it's a port.
#[cfg(target_arch = "x86_64")]
pub fn uart_port() -> Option<u16> {
    use core::sync::atomic::Ordering::Relaxed;
    (UART_KIND.load(Relaxed) == 1).then(|| UART_ADDRESS.load(Relaxed) as u16)
}

/// Sets up the console from the boot information.
///
/// # Safety
/// Call once, early, with the boot information the loader gave; `direct_map_base` must be
/// where the loader mapped physical memory.
pub unsafe fn init(info: &BootInfo, direct_map_base: u64) {
    let ec = info.early_console;
    UART_KIND.store(u64::from(ec.kind), core::sync::atomic::Ordering::Relaxed);
    UART_ADDRESS.store(ec.address, core::sync::atomic::Ordering::Relaxed);
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

/// Prints even if the console is locked: for reporting a panic, which may have happened while
/// this core was printing.
pub fn print_panic(args: fmt::Arguments<'_>) {
    for _ in 0..10_000_000 {
        if let Some(mut c) = CONSOLE.try_lock() {
            c.screen_released = false; // a panic is drawn over whatever is on the screen
            let _ = c.write_fmt(args);
            return;
        }
        core::hint::spin_loop();
    }
    // SAFETY: the system is stopping; whoever held the console will never release it.
    unsafe { CONSOLE.force_unlock() };
    let mut c = CONSOLE.lock();
    c.screen_released = false;
    let _ = c.write_fmt(args);
}

/// Stops the kernel drawing text on the screen; serial output carries on.
pub fn release_screen() {
    CONSOLE.lock().screen_released = true;
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
