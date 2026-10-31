// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The root program (docs/design/0009-programs-and-granite.md, sections 3 and 4): the first
//! program, which the kernel starts with every capability there is. In M3 it has nothing to
//! start yet (the device manager comes in M4), except that with `selftest` on the kernel's
//! command line it runs the program self-test and exits.

#![no_std]
#![no_main]

extern crate alloc;

mod selftest;

use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::role;
use granite::sys::perm;
use granite::time;
use pbr_archive::Archive;

granite::main!(main);

fn main(mut start: granite::Start) -> i32 {
    let Some(h) = start.take(role::ARCHIVE) else {
        granite::println!("The root program was started without the start-up archive.");
        return 1;
    };
    let archive_mem = Memory(h);
    let mapping = match archive_mem.map(perm::READ) {
        Ok(m) => m,
        Err(e) => {
            granite::println!("The root program can't map the start-up archive: {e}");
            return 1;
        }
    };
    // SAFETY: the mapping is readable for its whole length, and nothing writes to it.
    let bytes = unsafe { core::slice::from_raw_parts(mapping.as_ptr(), mapping.len() as usize) };
    let archive = match Archive::parse(bytes) {
        Ok(a) => a,
        Err(e) => {
            granite::println!("The start-up archive is damaged: {e}");
            return 1;
        }
    };
    if start.args().split_whitespace().any(|w| w == "selftest") {
        return selftest::run(&archive);
    }
    granite::println!(
        "The root program is running, with {} files in the start-up archive. There's nothing \
         else to start yet.",
        archive.count()
    );
    // Wait for ever: from M4, this is where the root program starts the device manager.
    if let Ok(n) = Notification::create() {
        loop {
            let _ = n.wait(time::FOREVER);
        }
    }
    0
}
