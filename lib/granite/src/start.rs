// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Program start-up (docs/design/0009-programs-and-granite.md, sections 2 and 4): the entry
//! point, the PrettyBigRock ELF note, reading the start-up block, and ending the process when
//! `main` returns or panics. This is the start-up code linked into every program.

use crate::handle::Handle;
use crate::process::role;
use crate::{log, process, sys, thread};
use alloc::string::String;
use alloc::vec::Vec;

/// What a program starts with: its capabilities, by role, and its arguments.
pub struct Start {
    handles: Vec<(u32, u32)>,
    args: String,
}

impl Start {
    /// Takes charge of the capability with role `r` (see `process::role`), if there is one.
    /// The program's own process and the kernel log stay with `granite`, which uses them.
    pub fn take(&mut self, r: u32) -> Option<Handle> {
        if r == role::PROCESS || r == role::LOG {
            return None;
        }
        let i = self.handles.iter().position(|(x, _)| *x == r)?;
        Some(Handle::from_raw(self.handles.remove(i).1))
    }

    /// The raw handle with role `r`, without taking charge of it.
    pub fn peek(&self, r: u32) -> Option<u32> {
        self.handles.iter().find(|(x, _)| *x == r).map(|(_, h)| *h)
    }

    /// Every (role, handle) still in the start-up block, not yet taken.
    pub fn handles(&self) -> &[(u32, u32)] {
        &self.handles
    }

    /// The program's arguments.
    pub fn args(&self) -> &str {
        &self.args
    }
}

/// Declares a program's `main`: `granite::main!(main);` with `fn main(start: granite::Start)
/// -> i32`. The process exits with what `main` returns.
#[macro_export]
macro_rules! main {
    ($f:path) => {
        #[unsafe(no_mangle)]
        fn __granite_main(start: $crate::Start) -> i32 {
            $f(start)
        }
    };
}

unsafe extern "Rust" {
    fn __granite_main(start: Start) -> i32;
}

// The entry point: align the stack as the calling convention expects, and call `entry` with
// the start-up block, the system page and the message page (note 0007, section 3).
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text._start, \"ax\"",
    ".global _start",
    "_start:",
    "and rsp, -16",
    "call {entry}",
    "ud2",
    // The PrettyBigRock note (note 0009, section 2): the interface version this program needs.
    ".section .note.prettybigrock, \"aR\", @note",
    ".balign 4",
    ".long 14",
    ".long 4",
    ".long 1",
    ".ascii \"PrettyBigRock\\0\"",
    ".balign 4",
    ".short {major}, {minor}",
    ".text",
    entry = sym entry,
    major = const sys::VERSION.0,
    minor = const sys::VERSION.1,
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text._start, \"ax\"",
    ".global _start",
    "_start:",
    "mov x9, sp",
    "and x9, x9, #~15",
    "mov sp, x9",
    "bl {entry}",
    "brk #1",
    ".section .note.prettybigrock, \"aR\", %note",
    ".balign 4",
    ".long 14",
    ".long 4",
    ".long 1",
    ".ascii \"PrettyBigRock\\0\"",
    ".balign 4",
    ".short {major}, {minor}",
    ".text",
    entry = sym entry,
    major = const sys::VERSION.0,
    minor = const sys::VERSION.1,
);

/// Reads a little-endian `u32` from the start-up block.
fn word(block: u64, at: u64) -> u32 {
    // SAFETY: the kernel or the launcher mapped the block (a whole page) readable here.
    unsafe { core::ptr::read_volatile((block + at) as *const u32) }
}

extern "C" fn entry(block: u64, sys_page: u64, msg: u64) -> ! {
    // SAFETY: the kernel passes the system page's address.
    unsafe { sys::set_system_page(sys_page) };
    // SAFETY: the main thread's block lives for ever.
    unsafe { thread::install(&raw mut thread::MAIN_TCB, msg) };
    // The start-up block: magic, version, count, where the arguments are, then pairs.
    if word(block, 0) != u32::from_le_bytes(*b"PBRS") || word(block, 4) & 0xFFFF != 1 {
        sys::process_exit(255);
    }
    // (At most 510 pairs fit in the 4 KiB block after its 16-byte header.)
    let count = (word(block, 4) >> 16).min(510);
    let (args_at, args_len) = (word(block, 8).min(4096), word(block, 12).min(4096));
    let pair = |i: u32| (word(block, 16 + 8 * u64::from(i)), word(block, 20 + 8 * u64::from(i)));
    // First the program's own process and the log, which the allocator and `println!` need.
    for i in 0..count {
        match pair(i) {
            (role::PROCESS, h) => process::set_self(h),
            (role::LOG, h) => log::set(h),
            _ => {}
        }
    }
    // Now the allocator works (it needs the program's own process), so every pair can be kept:
    // a program given a big file tree has hundreds.
    let pairs: Vec<(u32, u32)> = (0..count).map(pair).collect();
    let args = if args_at + args_len <= 4096 {
        // SAFETY: inside the mapped block.
        let b = unsafe {
            core::slice::from_raw_parts(
                (block + u64::from(args_at)) as *const u8,
                args_len as usize,
            )
        };
        String::from_utf8_lossy(b).into_owned()
    } else {
        String::new()
    };
    let start = Start { handles: pairs, args };
    // SAFETY: the program declared `__granite_main` with `granite::main!`.
    let code = unsafe { __granite_main(start) };
    sys::process_exit(code as u64)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    crate::println!("panicked: {info}");
    sys::process_exit(101)
}
