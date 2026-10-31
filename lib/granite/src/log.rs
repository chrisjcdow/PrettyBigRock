// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Writing to the kernel log, with [`println!`](crate::println). A program can only do this if
//! it was given the kernel log capability (start-up role 3).

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering};

static LOG: AtomicU32 = AtomicU32::new(0);

/// Uses `handle` for the kernel log from now on.
pub fn set(handle: u32) {
    LOG.store(handle, Ordering::Relaxed);
}

/// The kernel log handle, or 0 if there isn't one.
pub fn handle() -> u32 {
    LOG.load(Ordering::Relaxed)
}

/// Writes one line (at most 1024 bytes are kept).
pub fn write_line(args: fmt::Arguments<'_>) {
    let h = LOG.load(Ordering::Relaxed);
    if h == 0 {
        return;
    }
    let mut line = Line { buf: [0; 1024], len: 0 };
    let _ = line.write_fmt(args);
    let text = core::str::from_utf8(&line.buf[..line.len]).unwrap_or("(unprintable)");
    let _ = crate::sys::log_write(h, text);
}

struct Line {
    buf: [u8; 1024],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            let mut b = [0; 4];
            let e = c.encode_utf8(&mut b);
            if self.len + e.len() > self.buf.len() {
                return Ok(());
            }
            self.buf[self.len..self.len + e.len()].copy_from_slice(e.as_bytes());
            self.len += e.len();
        }
        Ok(())
    }
}

/// Writes a line to the kernel log, like `std::println!`.
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::log::write_line(format_args!($($arg)*)) };
}
