// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop boot (`desktoptest` on the kernel's command line;
//! docs/design/0026-apps-m6-tests-and-stages.md, section 6): the display server started as it
//! is for a person, with windowed test programs, while the test runner outside the machine
//! moves the pointer, clicks, types and takes screenshots, and reads what the display server
//! writes about its screens to the serial port (note 0022, section 13).
//!
//! Until the session's desktop and the sandbox manager's part are built (stage 6), the root
//! program stands in for both: it asks the display server for a session as the users service
//! will, and for each test program a window capability as the sandbox manager will, and starts
//! the program with it (at start-up role 1003, note 0024, section 10).
//!
//! The root program and the runner keep in step through the programs: the runner closes a
//! window when it has finished with it, the program ends, and the root program goes on to
//! the next step. Each step it starts is announced with a `Desktop test: ...` line, which is
//! what the runner waits for.

use crate::selftest::{MIB, wait};
use crate::supervisor::{DISPLAY_SESSIONS, Kind, Supervisor};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::machine::{Machine, Power};
use granite::process::{self, Spawn, Spawned, State};
use granite::sys::rights;
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_display_session as session_if;
use pbr_interfaces::pbr_display_sessions as sessions_if;

/// `pbr.window`'s start-up role (note 0024, section 10; `lib/window`'s `ROLE`).
const WINDOW_ROLE: u32 = 1003;
/// How long a step driven by the runner may take: the runner types, clicks and takes
/// screenshots on an emulated machine that may be slow.
const STEP_SECS: u64 = 900;

/// Runs the desktop test; returns the root program's exit status if it fails (if it passes,
/// the machine restarts and it never returns).
pub fn run(archive: &Archive<'static>, archive_mem: u32, start: &mut granite::Start) -> i32 {
    match all(archive, archive_mem, start) {
        Ok(summary) => {
            granite::println!("Desktop test passed: {summary}");
            // As the self-test does, the boot ends by restarting the machine; the runner has
            // told QEMU to stop instead, and checks that it did for that reason.
            let Some(h) = start.take(granite::process::role::POWER) else {
                granite::println!("Desktop test FAILED: restarting: no machine capability");
                return 1;
            };
            let e = Machine(h).power(Power::Restart);
            granite::println!("Desktop test FAILED: restarting: {e}");
            1
        }
        Err(e) => {
            granite::println!("Desktop test FAILED: {e}");
            1
        }
    }
}

/// A session of the display server's, as the users service will hold it.
struct Session {
    session: session_if::Client,
}

/// Asks the display server (the one running now) for a session for the test user, and shows
/// it.
fn session(sup: &Supervisor) -> Result<Session, String> {
    let mut sessions = sessions_if::Client::new(sup.client(Kind::Display, DISPLAY_SESSIONS, 60)?);
    sessions.set_timeout_ms(10_000);
    let started = sessions.start("test").map_err(|e| format!("starting a session: {e}"))?;
    sessions.activate(started.number).map_err(|e| format!("showing the session: {e}"))?;
    // A wrong number is refused.
    match sessions.activate(started.number + 100) {
        Err(CallError::Service(sessions_if::SessionsError::BadSession)) => {}
        other => return Err(format!("activating a session that isn't there: {other:?}")),
    }
    let mut session = session_if::Client::new(started.session);
    session.set_timeout_ms(10_000);
    Ok(Session { session })
}

/// The flags `pbr.display.session.client` takes (`ClientFlags`).
const DESKTOP: u32 = 2;

/// What the sandbox manager would tell the display server about an app.
struct App<'a> {
    /// Its number.
    number: u32,
    /// Its manifest's title.
    title: &'a str,
    /// `ClientFlags`.
    flags: u32,
    /// Launched by the user just now, so that its first window takes the focus.
    by_user: bool,
}

impl Session {
    /// Starts program `name` from the start-up archive with `args`, with a window capability
    /// for it as `app`.
    fn launch(
        &self,
        archive: &Archive<'static>,
        name: &str,
        args: &str,
        app: App<'_>,
    ) -> Result<Spawned, String> {
        let App { number: app, title, flags, by_user } = app;
        let launched = if by_user { time::now() } else { 0 };
        let client: Endpoint = self
            .session
            .client(app, title, session_if::ClientFlags(flags), launched)
            .map_err(|e| format!("a window capability for {title}: {e}"))?;
        let program = archive.find(name).ok_or(format!("{name} isn't in the archive"))?;
        process::spawn(&Spawn {
            name,
            program: program.data,
            budget: 16 * MIB,
            priority: 1,
            // Moved to it, and only to call with: the app can't copy it or pass it on.
            caps: &[(WINDOW_ROLE, client.0.into_raw(), rights::SEND, true)],
            log: true,
            args,
            tied: false,
            file_tree: &[],
        })
        .map_err(|e| format!("starting {title}: {e}"))
    }
}

/// Waits for a program to end by itself with `status`.
fn ended(s: &Spawned, title: &str, status: u64, secs: u64) -> Result<(), String> {
    match wait(s, secs) {
        Ok(State::Exited(n)) if n == status => Ok(()),
        Ok(other) => Err(format!("{title} ended {other:?}, not with status {status}")),
        Err(e) => Err(format!("{title}: {e}")),
    }
}

fn all(
    archive: &Archive<'static>,
    archive_mem: u32,
    start: &granite::Start,
) -> Result<String, String> {
    let sup = Supervisor::new(*archive, archive_mem, start, false)?;
    sup.start(Kind::Devmgr, "")?;
    sup.start(Kind::Input, "")?;
    sup.start(Kind::Display, "describe")?;
    let mut done: Vec<&str> = Vec::new();

    // Step 1: a desktop and two windows, which the runner drives (note 0022, section 13).
    let s = session(&sup)?;
    let desk = s.launch(
        archive,
        "wintest",
        "name=desk desktop",
        App { number: 1, title: "Desktop", flags: DESKTOP, by_user: false },
    )?;
    let alpha = s.launch(
        archive,
        "wintest",
        "name=Alpha colour=c04040 size=300x200",
        App { number: 2, title: "Alpha", flags: 0, by_user: true },
    )?;
    // Beta is launched a little later, so the two windows open in a known order.
    time::sleep_ms(500);
    let beta = s.launch(
        archive,
        "wintest",
        "name=Beta colour=4060c0 size=300x200 resizable",
        App { number: 3, title: "Beta", flags: 0, by_user: true },
    )?;
    granite::println!("Desktop test: drive the windows");
    // The runner's last step closes Alpha with its close button.
    ended(&alpha, "Alpha", 0, STEP_SECS)?;
    done.push("windows drawn where they open, focus, keys, the pointer, the clipboard");

    // Step 2: a program that stops answering, and a hostile one.
    let hung = s.launch(
        archive,
        "wintest",
        "name=Hung colour=808080 size=200x120 hang",
        App { number: 4, title: "Hung", flags: 0, by_user: true },
    )?;
    let probe = s.launch(
        archive,
        "windowprobe",
        "",
        App { number: 5, title: "Probe", flags: 0, by_user: false },
    )?;
    granite::println!("Desktop test: the hung window");
    ended(&probe, "windowprobe", 0, STEP_SECS)?;
    done.push("a hostile app refused");
    // The runner sees the hung window marked, types into Beta, and closes it with Ctrl+W.
    ended(&beta, "Beta", 0, STEP_SECS)?;
    done.push("a hung app marked not responding while the rest carry on");

    // Step 3: the display server crashes. Its clients hear it has gone (the desktop test
    // program ends with status 3 when it does); the supervisor starts it again.
    let generation = sup.generation(Kind::Display);
    sup.crash(Kind::Display)?;
    sup.wait_started(Kind::Display, generation, 60)?;
    ended(&desk, "the desktop", 3, 60)?;
    // The hung program never reads its events: it is stopped, as the sandbox manager will stop
    // every app with windows when the display server goes (note 0022, section 11).
    hung.process.stop(0).map_err(|e| format!("stopping Hung: {e}"))?;
    wait(&hung, 30)?;
    // A new session, the desktop again, and a window the runner closes with Ctrl+W.
    let s = session(&sup)?;
    let desk = s.launch(
        archive,
        "wintest",
        "name=desk desktop",
        App { number: 1, title: "Desktop", flags: DESKTOP, by_user: false },
    )?;
    let again = s.launch(
        archive,
        "wintest",
        "name=Again colour=40a040 size=200x120",
        App { number: 6, title: "Again", flags: 0, by_user: true },
    )?;
    granite::println!("Desktop test: after the restart");
    ended(&again, "Again", 0, STEP_SECS)?;
    desk.process.stop(0).map_err(|e| format!("stopping the desktop: {e}"))?;
    wait(&desk, 30)?;
    done.push("the display server crashed and came back");
    Ok(done.join(", "))
}
