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
//! answers. The last command is `shutdown`, which has the storage service save every volume:
//! this half waits for that, then starts a new storage service for the tests that follow. The
//! runner checks the disk afterwards (`tools/check_shell_after.py`).

use crate::sandboxtest::{self, session};
use crate::storagetest;
use crate::supervisor::{Kind, Supervisor};
use alloc::format;
use alloc::string::String;
use granite::interface::CallError;
use granite::time;
use pbr_interfaces::pbr_storage::{StorageError, VolumeState};

/// What the runner waits for before typing (keep it the same as `xtask/src/qemu.rs`).
const READY: &str = "Shell self-test: type commands";
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
        Ok(String::from("the console and the shell, typed at"))
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
