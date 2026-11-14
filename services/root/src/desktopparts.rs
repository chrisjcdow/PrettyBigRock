// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The self-test of what M6 stage 6 added to the users service, the sandbox manager and the
//! supervisor for the desktop (docs/design/0024-the-session-desktop.md, section 11), run with
//! the sandbox self-test's sessions: apps' own folders, who may call which `pbr.sandbox`
//! operation, cancelling and the limits, a chosen capability whose server never answers,
//! settings, and who may use the power service.
//!
//! The self-test boot has no display server, so nothing here opens a window: `wintest`'s
//! `save` and `load` use only its own folder.

use crate::sandboxtest::{Disk, Session, Users, refused};
use crate::supervisor::{Kind, Supervisor, rule};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::lock::Lock;
use granite::notification::Notification;
use granite::process::{Process, State};
use granite::sys::rights;
use granite::time;
use pbr_interfaces::pbr_power::{self as power_if, PowerError};
use pbr_interfaces::pbr_sandbox::{self as sandbox, SandboxError};
use pbr_interfaces::pbr_users_debug::SandboxKind;

const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;
/// The status `wintest load` ends with when it finds what `save` wrote.
const FOUND: u64 = 10;

pub fn run(sup: &Supervisor, u: &Users, chris: &Session, disk: &Disk) -> Result<String, String> {
    own_folder(chris, disk)?;
    each_kind(u)?;
    limits(u)?;
    never_answers(u)?;
    settings(chris)?;
    power(sup)?;
    Ok(String::from(
        "apps' own folders, who may call what, cancelling and the limits, a chosen capability \
         that never answers, settings, and the power service for the users service alone",
    ))
}

/// Runs `wintest` with `args` in chris's session and waits for it to end: its status.
fn wintest(s: &Session, args: &str) -> Result<u64, String> {
    let id = s.sandbox.prepare("wintest", args).map_err(|e| format!("preparing wintest: {e}"))?;
    let r = s.sandbox.start(id).map_err(|e| format!("starting wintest {args}: {e}"))?;
    ended(&r.process, 60).map_err(|e| format!("wintest {args}: {e}"))
}

/// Waits for `p` to end: its status.
fn ended(p: &Process, secs: u64) -> Result<u64, String> {
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    p.watch(&n, 1).map_err(|e| format!("watching: {e}"))?;
    if matches!(p.info().map(|i| i.state), Ok(State::Running | State::Ending)) {
        let _ = n.wait(time::after_ms(secs * 1000));
    }
    match p.info().map(|i| i.state) {
        Ok(State::Exited(n)) => Ok(n),
        other => Err(format!("ended {other:?}")),
    }
}

/// An app whose manifest says `data` gets `/Home/Apps/APP`, made for it, and nothing of any
/// other app's; a rule can deny it.
fn own_folder(chris: &Session, disk: &Disk) -> Result<(), String> {
    match wintest(chris, "name=Saver save")? {
        0 => {}
        n => return Err(format!("wintest couldn't save in its own folder (status {n})")),
    }
    // Where the user sees it: in their home, under Apps.
    let note = disk.read("Users/chris/Apps/wintest/note.txt")?;
    if note != b"saved before restarting" {
        return Err(format!("wintest's note says {:?}", String::from_utf8_lossy(&note)));
    }
    if wintest(chris, "name=Loader load")? != FOUND {
        return Err("wintest didn't find what it saved in its own folder".into());
    }
    // `deny wintest data`: it gets no folder, so finds nothing.
    let rules = "System/rules/chris.txt";
    let before = disk.read(rules)?;
    let mut denied = String::from_utf8_lossy(&before).into_owned();
    denied.push_str("deny wintest data\n");
    disk.write(rules, &denied)?;
    let r = wintest(chris, "name=Denied load");
    disk.write(rules, core::str::from_utf8(&before).unwrap_or(""))?;
    match r? {
        0 => Ok(()),
        n => Err(format!("wintest found its folder although a rule denies it (status {n})")),
    }
}

/// A `pbr.sandbox` capability for chris's session, as `kind`'s holders have it.
fn sandbox_as(u: &Users, kind: SandboxKind) -> Result<sandbox::Client, String> {
    let h = u.debug.sandbox("chris", kind).map_err(|e| format!("a {kind:?} capability: {e}"))?;
    let mut c = sandbox::Client::new(Endpoint(h));
    c.set_timeout_ms(30_000);
    Ok(c)
}

/// Whether `r` was refused as not allowed (`true`), or let through to be answered (`false`,
/// whatever the answer).
fn not_allowed<T>(r: Result<T, CallError<SandboxError>>) -> bool {
    matches!(r, Err(CallError::Service(SandboxError::NotAllowed)))
}

/// Every operation through each kind of capability: refused exactly where note 0024,
/// section 9's table says.
fn each_kind(u: &Users) -> Result<(), String> {
    use SandboxKind::*;
    for kind in [Trusted, Desktop, Viewer, Picker] {
        let c = sandbox_as(u, kind)?;
        // An endpoint that isn't a file's, to choose (the call is refused or answered before
        // anything is asked of it: launch 9999 doesn't exist).
        let other = Endpoint::create().map_err(|e| format!("{e}"))?;
        let chosen = other.mint(1, CLIENT).map_err(|e| format!("{e}"))?;
        let launches = matches!(kind, Trusted | Desktop);
        let views = matches!(kind, Trusted | Viewer);
        let lists = !matches!(kind, Picker);
        let expect = [
            ("prepare", not_allowed(c.prepare("nothing", "")), !launches),
            ("choose", not_allowed(c.choose(9999, "/Home/x", chosen)), !launches),
            ("start", not_allowed(c.start(9999)), !launches),
            ("cancel", not_allowed(c.cancel(9999)), !launches),
            ("apps", not_allowed(c.apps()), !launches),
            ("list", not_allowed(c.list()), !lists),
            ("details", not_allowed(c.details(9999)), !lists),
            ("stop", not_allowed(c.stop(9999)), !lists),
            ("grants", not_allowed(c.grants(9999, 0)), !views),
            ("revoke", not_allowed(c.revoke(9999, "/Home")), !views),
        ];
        for (op, refused, should) in expect {
            if refused != should {
                return Err(format!(
                    "{op} through a {kind:?} capability was {}refused",
                    if refused { "" } else { "not " }
                ));
            }
        }
    }
    Ok(())
}

/// Four waiting launches for each caller and sixteen in all; `cancel` gives one back.
fn limits(u: &Users) -> Result<(), String> {
    let callers: Vec<sandbox::Client> =
        (0..5).map(|_| sandbox_as(u, SandboxKind::Trusted)).collect::<Result<_, _>>()?;
    let mut waiting: Vec<(usize, u32)> = Vec::new();
    let r = (|| {
        for (i, c) in callers.iter().enumerate().take(4) {
            for _ in 0..4 {
                let id = c.prepare("wintest", "").map_err(|e| format!("preparing: {e}"))?;
                waiting.push((i, id));
            }
            if i == 0 {
                refused(
                    "a fifth launch for one caller",
                    c.prepare("wintest", ""),
                    SandboxError::TooMany,
                )?;
            }
        }
        // Sixteen waiting: a fifth caller can't prepare any.
        refused("a seventeenth launch", callers[4].prepare("wintest", ""), SandboxError::TooMany)?;
        // Another caller's launch can't be cancelled (nor started); one's own can.
        let (_, first) = waiting[0];
        refused("cancelling another's launch", callers[1].cancel(first), SandboxError::NotFound)?;
        refused("starting another's launch", callers[1].start(first), SandboxError::NotFound)?;
        callers[0].cancel(first).map_err(|e| format!("cancelling: {e}"))?;
        waiting.remove(0);
        refused("cancelling twice", callers[0].cancel(first), SandboxError::NotFound)?;
        let id = callers[4].prepare("wintest", "").map_err(|e| format!("after cancelling: {e}"))?;
        waiting.push((4, id));
        Ok::<(), String>(())
    })();
    for (i, id) in waiting {
        let _ = callers[i].cancel(id);
    }
    r
}

/// A chosen capability whose server never answers holds up only that `choose`: the sandbox
/// manager asks it on a helper thread, with a 2-second limit, and answers `list` meanwhile.
fn never_answers(u: &Users) -> Result<(), String> {
    let a = sandbox_as(u, SandboxKind::Trusted)?;
    let b = sandbox_as(u, SandboxKind::Trusted)?;
    // An endpoint this program never receives on: calls to it wait for ever.
    let silent = Endpoint::create().map_err(|e| format!("{e}"))?;
    let chosen = silent.mint(1, CLIENT).map_err(|e| format!("{e}"))?;
    let id = a.prepare("wintest", "").map_err(|e| format!("preparing: {e}"))?;
    let started = time::now();
    // (A thread's closure returns nothing: the result comes back through a shared slot. `Arc`
    // is a shared, counted pointer, and `Lock` lets one thread at a time at what it holds.)
    let slot = Arc::new(Lock::new(None));
    let theirs = slot.clone();
    let choosing = granite::thread::spawn(move || {
        let r = a.choose(id, "/Home/silent", chosen);
        let _ = a.cancel(id);
        *theirs.lock() = Some((r, time::now() - started));
    })
    .map_err(|e| format!("a thread: {e}"))?;
    time::sleep_ms(300);
    let before = time::now();
    b.list().map_err(|e| format!("list while a choose waits: {e}"))?;
    let took = (time::now() - before) / 1_000_000;
    choosing.join().map_err(|e| format!("joining: {e}"))?;
    let (r, waited) = slot.lock().take().ok_or("the choosing thread said nothing")?;
    drop(silent);
    if took > 1_000 {
        return Err(format!("list took {took} ms while a choose was waiting"));
    }
    match r {
        Err(CallError::Service(SandboxError::BadCapability))
            if (1_900_000_000..10_000_000_000).contains(&waited) =>
        {
            Ok(())
        }
        other => Err(format!("choosing a silent capability: {other:?} after {waited} ns")),
    }
}

/// A setting set is read back.
fn settings(chris: &Session) -> Result<(), String> {
    let before = chris.users.settings().map_err(|e| format!("settings: {e}"))?;
    let old = before.iter().find(|s| s.key == "timezone").map(|s| s.value.clone());
    chris.users.set("timezone", "+05:30").map_err(|e| format!("setting the time zone: {e}"))?;
    let after = chris.users.settings().map_err(|e| format!("settings: {e}"))?;
    let got = after.iter().find(|s| s.key == "timezone").map(|s| s.value.clone());
    let _ = chris.users.set("timezone", old.as_deref().unwrap_or("+00:00"));
    if got.as_deref() != Some("+05:30") {
        return Err(format!("the time zone set is read back as {got:?}"));
    }
    refused(
        "a setting that doesn't exist",
        chris.users.set("colour", "blue"),
        pbr_interfaces::pbr_users::UsersError::BadSetting,
    )
}

/// Only the users service may look up `power`, and the power service answers no other badge.
fn power(sup: &Supervisor) -> Result<(), String> {
    for child in [Kind::Devmgr, Kind::Storage, Kind::Console, Kind::Input, Kind::Display] {
        if rule(child, "power").is_some() {
            return Err(format!("{} may look up the power service", child.program()));
        }
    }
    if rule(Kind::Users, "power").is_none() {
        return Err("the users service may not look up the power service".into());
    }
    // Started here without the machine capability, so nothing can be turned off by mistake.
    sup.start(Kind::Power, "")?;
    let r = (|| {
        let mut p = power_if::Client::new(sup.client(Kind::Power, 2, 30)?);
        p.set_timeout_ms(10_000);
        match p.off() {
            Err(CallError::Service(PowerError::NotAllowed)) => {}
            other => return Err(format!("the power service, with another badge: {other:?}")),
        }
        match p.restart() {
            Err(CallError::Service(PowerError::NotAllowed)) => Ok(()),
            other => Err(format!("the power service, with another badge: {other:?}")),
        }
    })();
    sup.retire(Kind::Power)?;
    r
}
