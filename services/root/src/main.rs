// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The root program (docs/design/0009-programs-and-granite.md, sections 3 and 4): the first
//! program, which the kernel starts with every capability there is. It starts the system
//! services and supervises them (`supervisor`, note 0018), and with `selftest` on the kernel's
//! command line it runs the program self-test and exits.

#![no_std]
#![no_main]

extern crate alloc;

mod blocktest;
mod classictest;
mod crashtest;
mod devtest;
mod displaytest;
mod drivertest;
mod fattest;
mod filestest;
mod kernel12;
mod kernel13;
mod sandboxtest;
mod seattest;
mod selftest;
mod shelltest;
mod storagetest;
mod supervisetest;
mod supervisor;

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
    let len = mapping.len() as usize;
    let at = mapping.leak();
    // SAFETY: the mapping is readable for `len` bytes, nothing writes to it, and `leak` keeps
    // it mapped for the rest of the root program's life (which is the system's), so a
    // `'static` slice of it stays valid. The supervisor's thread needs it for as long.
    let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(at as *const u8, len) };
    let archive = match Archive::parse(bytes) {
        Ok(a) => a,
        Err(e) => {
            granite::println!("The start-up archive is damaged: {e}");
            return 1;
        }
    };
    if start.args().split_whitespace().any(|w| w == "selftest") {
        return selftest::run(&archive, archive_mem.0.raw(), &mut start);
    }
    granite::println!(
        "The root program is running, with {} files in the start-up archive.",
        archive.count()
    );
    // The system services, in the order they need each other (note 0018, section 1). Each
    // finds the ones it uses through the supervisor, which starts them again if they end.
    let sup = match supervisor::Supervisor::new(archive, archive_mem.0.raw(), &start, false) {
        Ok(s) => s,
        Err(e) => {
            granite::println!("The root program can't supervise: {e}");
            return 1;
        }
    };
    use supervisor::Kind;
    for kind in [Kind::Devmgr, Kind::Storage, Kind::Input, Kind::Console, Kind::Users] {
        if let Err(e) = sup.start(kind, "") {
            granite::println!("The root program can't start {}: {e}", kind.program());
        }
    }
    // Wait for ever.
    if let Ok(n) = Notification::create() {
        loop {
            let _ = n.wait(time::FOREVER);
        }
    }
    0
}
