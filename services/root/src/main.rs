// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The root program (docs/design/0009-programs-and-granite.md, sections 3 and 4): the first
//! program, which the kernel starts with every capability there is. It starts the system
//! services and supervises them (`supervisor`, note 0018). With `selftest` on the kernel's
//! command line it runs the program self-test and exits; with `desktoptest` it starts the
//! system as for a person and runs the desktop boot's test beside it (`desktoptest`, note
//! 0026, section 6).
//!
//! The desktop is the default: the display server is started, unless `text` is on the command
//! line, which starts the console instead (docs/design/0024-the-session-desktop.md, section 1:
//! for recovery, or a machine whose screen the display server can't use yet).

#![no_std]
#![no_main]

extern crate alloc;

mod blocktest;
mod classictest;
mod crashtest;
mod desktopparts;
mod desktoptest;
mod devtest;
mod displaytest;
mod drivertest;
mod fattest;
mod filestest;
mod kernel12;
mod kernel13;
#[cfg(target_arch = "x86_64")]
mod phys;
mod power;
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
    let text = start.args().split_whitespace().any(|w| w == "text");
    granite::println!(
        "The root program is running, with {} files in the start-up archive{}.",
        archive.count(),
        if text { ", in text mode" } else { "" }
    );
    let test = start.args().split_whitespace().any(|w| w == "desktoptest");
    let sup = match start_system(&archive, archive_mem.0.raw(), &start, text, test) {
        Ok(s) => s,
        Err(e) => {
            granite::println!("The root program can't supervise: {e}");
            return 1;
        }
    };
    if test {
        return desktoptest::run(&sup, text);
    }
    drop(sup);
    // Wait for ever.
    if let Ok(n) = Notification::create() {
        loop {
            let _ = n.wait(time::FOREVER);
        }
    }
    0
}

/// Starts the system services, in the order they need each other (note 0018, section 1; note
/// 0024, section 1): the display server, or the console in text mode. Each finds the ones it
/// uses through the supervisor, which starts them again if they end. In the desktop boot
/// (`test`), the display server describes its screens to the serial port and the users service
/// serves `pbr.users.debug` to the root program (note 0026, section 6).
pub fn start_system(
    archive: &Archive<'static>,
    archive_mem: u32,
    start: &granite::Start,
    text: bool,
    test: bool,
) -> Result<supervisor::Supervisor, alloc::string::String> {
    use supervisor::Kind;
    let sup = supervisor::Supervisor::new(*archive, archive_mem, start, false)?;
    let (caps, args) = power::caps(start);
    sup.set_power(caps, args);
    if test {
        sup.set_fallback_args("selftest");
    }
    let screen = if text { Kind::Console } else { Kind::Display };
    let (describe, users) = if test { ("describe", "desktoptest") } else { ("", "") };
    for (kind, args) in [
        (Kind::Devmgr, ""),
        (Kind::Storage, ""),
        (Kind::Input, ""),
        // (The console mirrors the screen to the serial port in the test, for the runner.)
        (
            screen,
            if !test {
                ""
            } else if text {
                "selftest"
            } else {
                describe
            },
        ),
        (Kind::Power, ""),
        (Kind::Users, users),
    ] {
        if let Err(e) = sup.start(kind, args) {
            granite::println!("The root program can't start {}: {e}", kind.program());
        }
    }
    Ok(sup)
}
