// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The shell self-test (docs/design/0018-supervision-shell-and-m5-tests.md, section 7, test 8):
//! the console and the text shell, used the way a person would, with the keyboard.
//!
//! This half, in the root program, starts the console and the users service as a normal start
//! would. The users service gives the console's terminal to the first administrator's session
//! (`chris`, on the GPT test disk's system volume), whose sandbox manager starts the shell on
//! it. Once the shell is running, this says so on the serial port, and the test runner on the
//! other side (`xtask/src/qemu.rs`, `SHELL_COMMANDS`) types commands through QEMU's control
//! socket, one each time the console shows a prompt. The console is told it is a self-test, so
//! it writes everything it shows to the kernel log as well, which is how the runner reads the
//! answers.
//!
//! Since M6 the console gets its keys from the input service (note 0021), so every command
//! typed here has come through it. Three things in the conversation are about that: the
//! runner sets the keyboard layout to `gb` and types a line with its keys, then to `us-intl`
//! and types accents with its dead keys; and between the two, once `gb` is set, this half
//! crashes the input service. The console must take the seat again from the new one, and the
//! users service must tell the new one the layout, or the line typed next comes out wrong.
//!
//! The last command is `shutdown`, which has the storage service save every volume:
//! this half waits for that, then starts a new storage service for the tests that follow. The
//! runner checks the disk afterwards (`tools/check_shell_after.py`).

use crate::sandboxtest::{self, session};
use crate::storagetest;
use crate::supervisor::{Kind, SEAT_CONSUMER, Supervisor};
use alloc::format;
use alloc::string::String;
use granite::interface::CallError;
use granite::time;
use pbr_interfaces::pbr_seat as seat;
use pbr_interfaces::pbr_storage::{StorageError, VolumeState};

/// What the runner waits for before typing (keep it the same as `xtask/src/qemu.rs`).
const READY: &str = "Shell self-test: type commands";
/// What the runner waits for before it types the line after `set layout gb` (the same in
/// `xtask/src/qemu.rs`).
const INPUT_RESTARTED: &str = "Shell self-test: the input service was restarted";
/// How long the runner may take over all its commands. Typing is slow under emulation, and
/// `shutdown` saves every volume.
const TYPING_SECS: u64 = 400;
/// How long the shell is given to show that the shutdown has finished.
const SHOWN_MS: u64 = 10_000;

/// Waits up to `secs` seconds for `f` to give something.
fn within<T>(secs: u64, what: &str, mut f: impl FnMut() -> Option<T>) -> Result<T, String> {
    let end = time::after_ms(secs * 1000);
    loop {
        if let Some(v) = f() {
            return Ok(v);
        }
        if time::now() > end {
            return Err(format!("{what}: not within {secs} seconds"));
        }
        time::sleep_ms(200);
    }
}

/// Waits for the runner to have set chris's layout to `gb` (the users service has then saved
/// it and told the input service), then crashes the input service and waits until its
/// replacement is running with that layout again, which only the users service can have
/// set. Then tells the runner to go on typing.
fn input_crash(sup: &Supervisor, u: &sandboxtest::Users) -> Result<(), String> {
    let layout_is_gb = |c: &seat::Client| c.layouts().is_ok_and(|l| l.current == "gb");
    // A consumer's capability may ask which layout is in use (and may not change it).
    let before = seat::Client::new(sup.client(Kind::Input, SEAT_CONSUMER, 30)?);
    within(TYPING_SECS, "chris's layout being set to gb", || {
        let s = session(u, "chris").ok()?;
        let set = s.users.settings().ok()?;
        (set.iter().any(|x| x.key == "layout" && x.value == "gb") && layout_is_gb(&before))
            .then_some(())
    })?;
    drop(before);
    let generation = sup.generation(Kind::Input);
    sup.crash(Kind::Input)?;
    sup.wait_started(Kind::Input, generation, 60)?;
    let after = seat::Client::new(sup.client(Kind::Input, SEAT_CONSUMER, 30)?);
    within(60, "the new input service being told the layout", || {
        layout_is_gb(&after).then_some(())
    })?;
    // And for it to have a keyboard again, and the console the seat: the console is told by
    // the kernel that the old one went and asks for the new one at once, then once a second.
    within(60, "the new input service finding a keyboard", || {
        after.devices().ok()?.iter().any(|d| d.kind == seat::DeviceKind::Keyboard).then_some(())
    })?;
    time::sleep_ms(3_000);
    granite::println!("{INPUT_RESTARTED}");
    Ok(())
}

/// The shell self-test. Returns a short summary.
pub fn run(sup: &Supervisor) -> Result<String, String> {
    sup.start(Kind::Console, "")?;
    let r = sandboxtest::start(sup, "").and_then(|u| {
        // The shell is running in chris's session (which may have been started again once
        // the console answered, to be given the terminal).
        within(120, "the shell running in chris's session", || {
            let s = session(&u, "chris").ok()?;
            let apps = s.sandbox.list().ok()?;
            apps.iter().any(|a| a.app == "shell" && a.running && a.trusted).then_some(())
        })?;
        granite::println!("{READY}");
        input_crash(sup, &u)?;
        // The runner's last command is `shutdown`: the storage service then refuses to open
        // volumes (it still lists them).
        let storage = storagetest::connect(sup)?;
        let volume = storage.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
        let volume = volume.first().ok_or("no volumes")?.id;
        within(TYPING_SECS, "the shell's shutdown", || {
            matches!(storage.admin.open(volume), Err(CallError::Service(StorageError::ShutDown)))
                .then_some(())
        })?;
        // Then for it to finish (asking for a shutdown that is under way answers `NotReady`; one
        // that has finished, `Ok`), and for the shell to show the user it has: its answer comes
        // straight after, but nothing here can see the screen, so give it a few seconds before
        // the session is ended.
        within(60, "the storage service to finish shutting down", || {
            storage.admin.shutdown().is_ok().then_some(())
        })?;
        time::sleep_ms(SHOWN_MS);
        drop(storage);
        // The shell carries on afterwards, waiting for the next command.
        within(30, "the shell still running after shutdown", || {
            let s = session(&u, "chris").ok()?;
            let apps = s.sandbox.list().ok()?;
            apps.iter().any(|a| a.app == "shell" && a.running).then_some(())
        })?;
        Ok(String::from(
            "the console and the shell, typed at through the input service, with three \
             layouts and the input service crashed on the way",
        ))
    });
    sup.retire(Kind::Users)?;
    sup.retire(Kind::Console)?;
    // A storage service that has shut down stays that way, so the tests after this one get a
    // new one, which opens the volumes again.
    sup.retire(Kind::Storage)?;
    sup.start(Kind::Storage, "")?;
    // The tests after this one expect the volumes ready, as they are after a start (on ARM
    // under emulation, finding every disk again takes the new service well over 30 seconds).
    let storage = storagetest::connect(sup)?;
    within(120, "the new storage service's volumes", || {
        let list = storage.admin.list().ok()?;
        (!list.is_empty() && list.iter().all(|v| v.state != VolumeState::Starting)).then_some(())
    })?;
    r
}
