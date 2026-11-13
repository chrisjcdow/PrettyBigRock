// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop boot (`desktoptest` on the kernel's command line;
//! docs/design/0026-apps-m6-tests-and-stages.md, section 6): the system started as it is for a
//! person, with the users service, the sandbox manager and the desktop app, while the test
//! runner outside the machine moves the pointer, clicks, types and takes screenshots, and reads
//! what the display server writes about its screens to the serial port (note 0022, section 13).
//!
//! This thread is the rest of the test, inside: through `pbr.users.debug` it gets the user's
//! session's `pbr.sandbox`, as a trusted app has it, and launches the test programs as apps
//! (`tests/programs/wintest` and the hostile `windowprobe`), as the shell's `run` would; and it
//! checks what the desktop holds. The root program and the runner keep in step through the
//! programs: the runner closes a window when it is done with it, the program ends, and this
//! thread starts the next step. Each step it starts is announced with a `Desktop test: ...`
//! line, which is what the runner waits for.
//!
//! The boot is run twice on the same disks (note 0024, section 11). The first time, after the
//! windows, it saves a note in an app's own folder, and the display server is crashed past its
//! restart policy so that the console takes its place and the shell appears (falling back); the
//! runner restarts the machine from the shell. The second time, the note is found, and the
//! runner shuts the machine down from the start menu. With `text` too, the console is started
//! instead of the display server, and the runner turns the machine off from the shell.

use crate::sandboxtest::DEBUG;
use crate::supervisor::{Kind, Supervisor};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::notification::Notification;
use granite::process::{Process, State};
use granite::time;
use pbr_interfaces::pbr_sandbox::{self as sandbox, AppServices, FileRights, GrantReason};
use pbr_interfaces::pbr_users_debug as users_debug;

/// How long a step driven by the runner may take: the runner types, clicks and takes
/// screenshots on an emulated machine that may be slow.
const STEP_SECS: u64 = 900;
/// The status `wintest load` ends with when it found what the first boot saved.
const FOUND: u64 = 10;

/// Runs the desktop test beside the system; returns only if it fails (the runner ends the boot
/// by restarting or turning off the machine).
pub fn run(sup: &Supervisor, text: bool) -> i32 {
    let result = if text { text_mode() } else { all(sup) };
    match result {
        Ok(summary) => {
            granite::println!("Desktop test passed: {summary}");
            // The system carries on as it would for a person, until the runner ends it.
            if let Ok(n) = Notification::create() {
                loop {
                    let _ = n.wait(time::FOREVER);
                }
            }
            1
        }
        Err(e) => {
            granite::println!("Desktop test FAILED: {e}");
            1
        }
    }
}

/// In text mode there is no display server: the session gets the console's terminal, and its
/// sandbox manager starts the shell, which the runner types into.
fn text_mode() -> Result<String, String> {
    granite::println!("Desktop test: text mode");
    Ok(String::from("text mode"))
}

/// The session as a trusted app sees it.
struct Session {
    debug: users_debug::Client,
    sandbox: sandbox::Client,
}

impl Session {
    /// The test user's session's `pbr.sandbox`, waiting up to `secs` seconds for the session
    /// to be running with its desktop (after a restart, its sandbox manager is new).
    fn find(sup: &Supervisor, secs: u64) -> Result<Session, String> {
        let mut debug = users_debug::Client::new(sup.client(Kind::Users, DEBUG, 60)?);
        debug.set_timeout_ms(30_000);
        let end = time::after_ms(secs * 1000);
        loop {
            if let Ok(caps) = debug.session("chris") {
                let mut sandbox = sandbox::Client::new(Endpoint(caps.sandbox));
                sandbox.set_timeout_ms(30_000);
                let s = Session { debug, sandbox };
                if s.app("desktop").is_some() {
                    return Ok(s);
                }
                debug = s.debug;
            }
            if time::now() > end {
                return Err(format!("the desktop wasn't running after {secs} seconds"));
            }
            time::sleep_ms(200);
        }
    }

    /// The running app called `name`, if there is one: its number.
    fn app(&self, name: &str) -> Option<u32> {
        let list = self.sandbox.list().ok()?;
        list.iter().find(|a| a.app == name && a.running).map(|a| a.id)
    }

    /// Launches `app` with `args`.
    fn launch(&self, app: &str, args: &str) -> Result<(u32, Process), String> {
        let launch =
            self.sandbox.prepare(app, args).map_err(|e| format!("preparing {app}: {e}"))?;
        let started = self.sandbox.start(launch).map_err(|e| format!("starting {app}: {e}"))?;
        Ok((started.app, started.process))
    }
}

/// Waits for `p` to end, by itself, with `status` (`None`: however it ends).
fn ended(p: &Process, title: &str, status: Option<u64>, secs: u64) -> Result<State, String> {
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    p.watch(&n, 1).map_err(|e| format!("watching {title}: {e}"))?;
    if matches!(p.info().map(|i| i.state), Ok(State::Running | State::Ending)) {
        n.wait(time::after_ms(secs * 1000))
            .map_err(|e| format!("{title} didn't end in {secs} seconds: {e}"))?;
    }
    let state = p.info().map(|i| i.state).map_err(|e| format!("{title}: {e}"))?;
    match (state, status) {
        (State::Exited(n), Some(s)) if n == s => Ok(state),
        (_, None) => Ok(state),
        (other, Some(s)) => Err(format!("{title} ended {other:?}, not with status {s}")),
    }
}

/// What the desktop holds is what note 0024, section 6 says: its windows, its own folder, a
/// `pbr.sandbox` and a `pbr.users`, and none of the user's files.
fn check_desktop(s: &Session) -> Result<(), String> {
    let id = s.app("desktop").ok_or("no desktop")?;
    let d = s.sandbox.details(id).map_err(|e| format!("the desktop's details: {e}"))?;
    let wanted = AppServices::WINDOWS.0
        | AppServices::DATA.0
        | AppServices::SANDBOX.0
        | AppServices::USERS.0;
    if d.services.0 != wanted || d.trusted || d.system || d.title != "Desktop" {
        return Err(format!("the desktop holds {d:?}"));
    }
    let grants = s.sandbox.grants(id, 0).map_err(|e| format!("the desktop's grants: {e}"))?;
    let rw = FileRights::READ.0 | FileRights::WRITE.0;
    match grants.grants.as_slice() {
        [g] if g.path == "/Home/Apps/desktop"
            && g.rights.0 == rw
            && g.reason == GrantReason::Manifest => {}
        other => return Err(format!("the desktop's files are {other:?}")),
    }
    Ok(())
}

fn all(sup: &Supervisor) -> Result<String, String> {
    let mut done: Vec<&str> = Vec::new();
    let s = Session::find(sup, 300)?;
    check_desktop(&s)?;
    done.push("the desktop started by the sandbox manager, holding only what it should");

    // Which boot this is: a note saved by the first is found by the second.
    let (_, p) = s.launch("wintest", "name=Saved load")?;
    if ended(&p, "wintest load", None, 120)? == State::Exited(FOUND) {
        granite::println!("Desktop test: after restarting");
        done.push("what was saved before restarting is still there");
        return Ok(done.join(", "));
    }

    // Step 1: two windows, which the runner drives (note 0022, section 13).
    let (_, alpha) = s.launch("wintest", "name=Alpha colour=c04040 size=300x200")?;
    // Beta is launched a little later, so the two windows open in a known order.
    time::sleep_ms(500);
    let (_, beta) = s.launch("wintest", "name=Beta colour=4060c0 size=300x200 resizable")?;
    granite::println!("Desktop test: drive the windows");
    // The runner's last step closes Alpha with its close button.
    ended(&alpha, "Alpha", Some(0), STEP_SECS)?;
    done.push("windows drawn where they open, focus, keys, the pointer, the clipboard");

    // Step 2: a program that stops answering, and a hostile one.
    let (_, hung) = s.launch("wintest", "name=Hung colour=808080 size=200x120 hang")?;
    let (_, probe) = s.launch("windowprobe", "")?;
    granite::println!("Desktop test: the hung window");
    ended(&probe, "windowprobe", Some(0), STEP_SECS)?;
    done.push("a hostile app refused");
    // The runner sees the hung window marked, types into Beta, and closes it with Ctrl+W, then
    // stops the hung app from its window menu, which the desktop passes to the sandbox manager.
    ended(&beta, "Beta", Some(0), STEP_SECS)?;
    done.push("a hung app marked not responding while the rest carry on");
    ended(&hung, "Hung", None, STEP_SECS)?;
    done.push("an app stopped from its window menu");

    // Step 3: the display server crashes. The sandbox manager stops the apps with windows (the
    // desktop among them), the users service starts the session again with the new display
    // server, and its sandbox manager starts the desktop again.
    let generation = sup.generation(Kind::Display);
    sup.crash(Kind::Display)?;
    sup.wait_started(Kind::Display, generation, 60)?;
    let s = Session::find(sup, 300)?;
    check_desktop(&s)?;
    let (_, again) = s.launch("wintest", "name=Again colour=40a040 size=200x120")?;
    granite::println!("Desktop test: after the restart");
    ended(&again, "Again", Some(0), STEP_SECS)?;
    done.push("the display server crashed and came back, with the desktop");

    // Step 4: the start menu. The runner opens it with the Windows key and launches the test
    // window from it; it appears focused, and the runner closes it.
    granite::println!("Desktop test: the start menu");
    let end = time::after_ms(STEP_SECS * 1000);
    let launched = loop {
        if let Some(id) = s.app("wintest") {
            break id;
        }
        if time::now() > end {
            return Err("nothing was launched from the start menu".into());
        }
        time::sleep_ms(200);
    };
    while s.sandbox.list().is_ok_and(|l| l.iter().any(|a| a.id == launched)) {
        if time::now() > end {
            return Err("the window launched from the start menu wasn't closed".into());
        }
        time::sleep_ms(200);
    }
    done.push("an app launched from the start menu");

    // A note in an app's own folder, for the second boot to find.
    let (_, saver) = s.launch("wintest", "name=Saver save")?;
    ended(&saver, "wintest save", Some(0), 120)?;

    // Step 5: the display server ends again and again, until it is given up on: the console
    // takes its place, the users service starts the session again on its terminal, and the
    // shell appears, which the runner types into (note 0024, section 8).
    granite::println!("Desktop test: falling back");
    for _ in 0..8 {
        let generation = sup.generation(Kind::Display);
        if sup.crash(Kind::Display).is_err() {
            break;
        }
        if sup.wait_started(Kind::Display, generation, 10).is_err() {
            break;
        }
    }
    sup.wait_started(Kind::Console, 0, 60)?;
    done.push("the console in the display server's place");
    Ok(done.join(", "))
}
