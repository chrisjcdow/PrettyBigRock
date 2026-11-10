// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The tests of kernel interface 1.2 (docs/design/0013-kernel-1.2.md; note 0018, section 7,
//! test 1), run by the root program as part of the program self-test:
//! - **watched badges**: the kernel tells a service when the last capability carrying one of
//!   its badges has gone, after closes, copies and revocation, in the right order with
//!   notifications and messages, and gives the record's memory back;
//! - **derived replies**: a service answers with a capability that is a child of the one the
//!   caller used, so revoking the caller's capability takes it too, while closing doesn't;
//! - **tied processes**: a process ends with the process that created it, and its creator's
//!   end is only reported once it has ended;
//! - **the wall clock**, and the interface version.
//!
//! Each check that fails says which, in words; nothing here panics.

use crate::selftest::{Files, MIB, Outcome};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, Event};
use granite::memory::Memory;
use granite::message::{CapSend, Outgoing};
use granite::notification::Notification;
use granite::process::{self, Process, Spawn, State, TIED_STATUS};
use granite::sys::{self, Error, num, rights};
use granite::{Handle, thread, time};
use pbr_test_protocol::{op, role};

/// The kernel's charge for a notification, which a watched badge's record costs (note 0006,
/// section 9; `process_info` lists it, but the test states what the note promises).
const RECORD_CHARGE: u64 = 256;
/// And for an endpoint.
const ENDPOINT_CHARGE: u64 = 512;

/// Runs every kernel 1.2 test.
pub fn run(f: &Files<'_>, echo: &Endpoint) -> Outcome {
    version()?;
    watched().map_err(|e| format!("watched badges: {e}"))?;
    derived(echo).map_err(|e| format!("derived replies: {e}"))?;
    tied(f).map_err(|e| format!("tied processes: {e}"))?;
    wall_clock()
}

fn version() -> Outcome {
    // Later minor versions only add to 1.2 (note 0007, section 7); `kernel13` checks for 1.3.
    match sys::version() {
        (1, minor) if minor >= 2 => Ok(()),
        (a, b) => Err(format!("the kernel says it is interface version {a}.{b}, not 1.2 or later")),
    }
}

fn wall_clock() -> Outcome {
    // 1 January 2026 and 1 January 2100, in seconds since 1970. QEMU's clock is the host's.
    let now = time::wall_seconds();
    if (1_767_225_600..4_102_444_800).contains(&now) {
        Ok(())
    } else {
        Err(format!("the wall clock says {now} seconds since 1970"))
    }
}

/// How much of this program's budget is in use.
fn used() -> Result<u64, String> {
    process::info(process::self_handle()).map(|i| i.used).map_err(|e| format!("{e}"))
}

/// What one receive on `ep` gets, without waiting more than `ms`.
fn next(ep: &Endpoint, ms: u64) -> Result<Event, Error> {
    let deadline = if ms == 0 { time::NOW } else { time::after_ms(ms) };
    ep.receive(&mut [], deadline)
}

/// Expects a gone notice for `badge`, and nothing else, straight away.
fn expect_gone(ep: &Endpoint, badge: u64, what: &str) -> Outcome {
    match next(ep, 0) {
        Ok(Event::Gone(b)) if b == badge => Ok(()),
        Ok(Event::Gone(b)) => Err(format!("{what}: a gone notice for badge {b}, not {badge}")),
        Ok(_) => Err(format!("{what}: something other than a gone notice")),
        Err(e) => Err(format!("{what}: no gone notice ({e})")),
    }
}

/// Expects nothing to be waiting.
fn expect_nothing(ep: &Endpoint, what: &str) -> Outcome {
    match next(ep, 0) {
        Err(Error::TimedOut) => Ok(()),
        Ok(Event::Gone(b)) => Err(format!("{what}: an early gone notice for badge {b}")),
        Ok(_) => Err(format!("{what}: something arrived")),
        Err(e) => Err(format!("{what}: receiving failed ({e})")),
    }
}

fn watched() -> Outcome {
    let e = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;

    // Refusals: badge 0, no receive right, an already badged capability.
    if e.mint_watched(0, rights::ALL).err() != Some(Error::InvalidArgument) {
        return Err("badge 0 wasn't refused".into());
    }
    let no_receive =
        Endpoint(e.0.duplicate(rights::ALL & !rights::RECEIVE).map_err(|e| format!("{e}"))?);
    if no_receive.mint_watched(5, rights::ALL).err() != Some(Error::NoRight) {
        return Err("a capability without the receive right could watch a badge".into());
    }
    let badged = e.mint(3, rights::ALL).map_err(|e| format!("{e}"))?;
    if badged.mint_watched(4, rights::ALL).err() != Some(Error::BadState) {
        return Err("a badged capability was re-badged".into());
    }
    expect_nothing(&e, "before anything was watched")?;

    // Copies share one record: only the last one going gives the notice.
    let w = e.mint_watched(7, rights::ALL).map_err(|e| format!("{e}"))?;
    let w2 = w.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?;
    drop(w);
    expect_nothing(&e, "after closing one of two copies")?;
    drop(w2);
    expect_gone(&e, 7, "after closing the last copy")?;

    // Revoking destroys the copies but keeps the original; the notice waits for it.
    let x = e.mint_watched(9, rights::ALL).map_err(|e| format!("{e}"))?;
    let y = x.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?;
    x.0.revoke().map_err(|e| format!("revoking: {e}"))?;
    if y.info().err() != Some(Error::BadHandle) {
        return Err("a revoked copy still worked".into());
    }
    drop(y);
    expect_nothing(&e, "after revoking the copies of a capability that's still held")?;
    drop(x);
    expect_gone(&e, 9, "after closing the revoked capability's original")?;

    // Order: notification bits, then gone notices, then calls.
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    e.bind(&n).map_err(|e| format!("binding: {e}"))?;
    let client = e.mint(11, rights::SEND).map_err(|e| format!("{e}"))?;
    let caller = thread::spawn(move || {
        let _ = client.call(&Outgoing::new(1, &[]), &mut [], 0, time::after_ms(30_000));
    })
    .map_err(|e| format!("starting a thread: {e}"))?;
    time::sleep_ms(300); // give the caller time to be waiting
    n.signal(0x10).map_err(|e| format!("{e}"))?;
    let w = e.mint_watched(12, rights::ALL).map_err(|e| format!("{e}"))?;
    drop(w);
    match next(&e, 0) {
        Ok(Event::Signal(0x10)) => {}
        _ => return Err("the notification's bits didn't come first".into()),
    }
    expect_gone(&e, 12, "after the notification's bits")?;
    match next(&e, 30_000) {
        Ok(Event::Call(_, reply)) => {
            reply.reply(&Outgoing::new(0, &[]), &[]).map_err(|e| format!("replying: {e}"))?
        }
        _ => return Err("the call didn't come after the gone notice".into()),
    }
    caller.join().map_err(|e| format!("joining the caller: {e}"))?;

    // The record is charged to whoever minted, until its notice is received.
    let w = e.mint_watched(20, rights::ALL).map_err(|e| format!("{e}"))?;
    drop(w);
    let before = used()?;
    expect_gone(&e, 20, "for the charge check")?;
    let after = used()?;
    if before.checked_sub(after) != Some(RECORD_CHARGE) {
        return Err(format!(
            "receiving the notice gave back {} bytes, not {RECORD_CHARGE}",
            before as i64 - after as i64
        ));
    }

    // When nobody can receive any more, queued notices are dropped and their charges given
    // back (with the endpoint's own).
    let e2 = Endpoint::create().map_err(|e| format!("{e}"))?;
    let w = e2.mint_watched(30, rights::ALL).map_err(|e| format!("{e}"))?;
    drop(w);
    let before = used()?;
    drop(e2);
    let after = used()?;
    if before.checked_sub(after) != Some(RECORD_CHARGE + ENDPOINT_CHARGE) {
        return Err(format!(
            "closing the endpoint with a notice queued gave back {} bytes, not {}",
            before as i64 - after as i64,
            RECORD_CHARGE + ENDPOINT_CHARGE
        ));
    }
    Ok(())
}

/// Asks echo, through `ep`, for a capability derived from `ep` with `badge` and `mask`.
fn derive(ep: &Endpoint, badge: u64, mask: u32) -> Result<Endpoint, Error> {
    let mut data = [0u8; 12];
    data[..8].copy_from_slice(&badge.to_le_bytes());
    data[8..].copy_from_slice(&mask.to_le_bytes());
    let mut r = ep.call(&Outgoing::new(op::DERIVE, &data), &mut [], 0, time::after_ms(10_000))?;
    r.take_cap(0).map(Endpoint).ok_or(Error::BadState)
}

/// The badge echo sees on calls through `ep`.
fn badge_of(ep: &Endpoint) -> Result<u64, Error> {
    let r = ep.call(&Outgoing::new(op::BADGE, &[]), &mut [], 0, time::after_ms(10_000))?;
    Ok(r.data().get(..8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]))))
}

/// How many gone notices echo has had, and the last badge.
fn gone_count(ep: &Endpoint) -> Result<(u64, u64), String> {
    let r = ep
        .call(&Outgoing::new(op::GONE, &[]), &mut [], 0, time::after_ms(10_000))
        .map_err(|e| format!("asking echo for its gone notices: {e}"))?;
    let d = r.data();
    let w = |i: usize| {
        d.get(i..i + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
    };
    Ok((w(0), w(8)))
}

/// Waits (up to 10 seconds) for echo to have had `count` gone notices, the last for `badge`.
fn wait_gone(ep: &Endpoint, count: u64, badge: u64, what: &str) -> Outcome {
    let deadline = time::after_ms(10_000);
    loop {
        let (c, last) = gone_count(ep)?;
        if c >= count {
            return if c == count && last == badge {
                Ok(())
            } else {
                Err(format!("{what}: echo had {c} gone notices, the last for {last}"))
            };
        }
        if time::now() > deadline {
            return Err(format!("{what}: echo was never told the capability had gone"));
        }
        time::sleep_ms(20);
    }
}

/// Turns an error into words, saying what was being done.
fn fail(what: &'static str) -> impl Fn(Error) -> String {
    move |e| format!("{what}: {e}")
}

fn derived(echo: &Endpoint) -> Outcome {
    let full = rights::SEND | rights::DUPLICATE | rights::TRANSFER | rights::REVOKE;
    // An unbadged parent the root program can revoke, and a badged capability under it.
    let parent = Endpoint(echo.0.duplicate(full).map_err(fail("copying"))?);
    let asker = parent.mint(60, full).map_err(fail("minting"))?;
    let (gone0, _) = gone_count(&asker)?;

    // The derived capability has the caller's rights, masked, and its own badge.
    let d = derive(&asker, 61, rights::SEND | rights::DUPLICATE).map_err(fail("deriving"))?;
    match d.0.info() {
        Ok((_, r)) if r == rights::SEND | rights::DUPLICATE => {}
        other => return Err(format!("the derived capability is {other:?}")),
    }
    if badge_of(&d) != Ok(61) {
        return Err("the derived capability doesn't carry its badge".into());
    }

    // Revoking the capability a call came through revokes what was derived from it (and
    // nothing else), and echo is told. `middle` is a copy of `asker`; revoking it destroys
    // everything derived from it: `d2`.
    let middle = Endpoint(asker.0.duplicate(full).map_err(fail("copying"))?);
    let d2 = derive(&middle, 62, rights::SEND).map_err(fail("deriving"))?;
    middle.0.revoke().map_err(fail("revoking"))?;
    if badge_of(&d2) != Err(Error::BadHandle) {
        return Err("the derived capability survived revoking the one it came through".into());
    }
    drop(d2);
    drop(middle);
    wait_gone(&asker, gone0 + 1, 62, "after revoking")?;
    if badge_of(&d) != Ok(61) {
        return Err("revoking a sibling took the first derived capability too".into());
    }

    // Closing the caller's capability doesn't: the derived one moves up to its parent, so
    // revoking further up still reaches it.
    let d3 = derive(&asker, 63, rights::SEND).map_err(fail("deriving"))?;
    drop(asker);
    if badge_of(&d3) != Ok(63) {
        return Err("closing the capability it came through closed the derived one".into());
    }
    parent.0.revoke().map_err(fail("revoking the parent"))?;
    if badge_of(&d3) != Err(Error::BadHandle) || badge_of(&d) != Err(Error::BadHandle) {
        return Err("revoking further up didn't reach the derived capabilities".into());
    }
    drop((d, d3));
    // Two gone notices (61 and 63), in either order; the count says both came.
    let deadline = time::after_ms(10_000);
    while gone_count(&parent)?.0 < gone0 + 3 {
        if time::now() > deadline {
            return Err("echo wasn't told about every revoked derived capability".into());
        }
        time::sleep_ms(20);
    }

    // Refusals: badge 0 (the reply fails, so the caller is told the service has gone), and
    // a derived capability in a call.
    let asker = parent.mint(64, full).map_err(fail("minting"))?;
    if derive(&asker, 0, rights::SEND).err() != Some(Error::PeerGone) {
        return Err("a derived capability with badge 0 wasn't refused".into());
    }
    let caps = [CapSend::derived(65, rights::SEND)];
    let m = Outgoing { label: op::ECHO, data: &[], caps: &caps };
    if asker.call(&m, &mut [], 0, time::after_ms(10_000)).err() != Some(Error::InvalidArgument) {
        return Err("a derived capability in a call wasn't refused".into());
    }
    Ok(())
}

/// A process capability the probe sent, and whether it was tied.
struct Reported {
    tied: bool,
    level: u64,
    process: Process,
}

/// Receives `count` reports from probes in `tied` mode.
fn reports(ep: &Endpoint, count: usize) -> Result<Vec<Reported>, String> {
    let mut out = Vec::new();
    while out.len() < count {
        match next(ep, 60_000) {
            Ok(Event::Call(mut msg, reply)) => {
                let d = msg.data();
                let tied = d.first() == Some(&1);
                let level =
                    d.get(1..9).map_or(99, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
                let h: Option<Handle> = msg.take_cap(0);
                reply.reply(&Outgoing::new(0, &[]), &[]).map_err(|e| format!("replying: {e}"))?;
                let process = Process(h.ok_or("a report without a process capability")?);
                out.push(Reported { tied, level, process });
            }
            Ok(_) => {}
            Err(e) => return Err(format!("waiting for the probes' reports: {e}")),
        }
    }
    Ok(out)
}

fn state(p: &Process) -> Result<State, String> {
    p.info().map(|i| i.state).map_err(|e| format!("{e}"))
}

/// Waits up to 30 seconds for `p` to end, by its watch notification.
fn wait_end(p: &Process) -> Result<State, String> {
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    p.watch(&n, 1).map_err(|e| format!("watching: {e}"))?;
    n.wait(time::after_ms(30_000)).map_err(|e| format!("waiting for a process to end: {e}"))?;
    state(p)
}

fn tied(f: &Files<'_>) -> Outcome {
    // Bits above `TIED` are refused.
    let a = [MIB, 1 | 1 << 9, b"x".as_ptr() as u64, 1, 0, 0];
    if sys::call(num::PROCESS_CREATE, a).err() != Some(Error::InvalidArgument) {
        return Err("an unknown process_create flag wasn't refused".into());
    }

    let report = Endpoint::create().map_err(|e| format!("{e}"))?;
    let program = Memory::create(f.probe.len() as u64).map_err(|e| format!("{e}"))?;
    program.write(0, f.probe).map_err(|e| format!("{e}"))?;
    let start = |args: &str| {
        let caps = [
            (role::PROGRAM, program.0.raw(), rights::ALL & !rights::WRITE, false),
            (
                role::REPORT,
                report.0.raw(),
                rights::SEND | rights::DUPLICATE | rights::TRANSFER,
                false,
            ),
        ];
        process::spawn(&Spawn {
            name: "probe",
            program: f.probe,
            budget: 64 * MIB,
            priority: 1,
            caps: &caps,
            log: true,
            args,
            tied: false,
            file_tree: &[],
        })
        .map_err(|e| format!("starting probe {args}: {e}"))
    };

    // Stopping a process stops its tied children, and theirs; not its untied one. Its end is
    // reported only once theirs are complete.
    let top = start("tied 2 untied")?;
    let got = reports(&report, 3)?;
    let find = |tied: bool, level: u64| {
        got.iter().find(|r| r.tied == tied && r.level == level).ok_or("a probe didn't report")
    };
    let (child, grandchild, untied) = (find(true, 1)?, find(true, 0)?, find(false, 0)?);
    for r in [child, grandchild, untied] {
        if state(&r.process)? != State::Running {
            return Err("a child wasn't running before its creator was stopped".into());
        }
    }
    top.process.stop(77).map_err(|e| format!("stopping: {e}"))?;
    match wait_end(&top.process)? {
        State::Stopped(77) => {}
        other => return Err(format!("the creator ended {other:?}")),
    }
    // Its end was reported, so its tied children's ends must already be complete.
    for (r, what) in [(child, "its tied child"), (grandchild, "its tied child's tied child")] {
        match state(&r.process)? {
            State::Stopped(TIED_STATUS) => {}
            other => {
                return Err(format!("when its creator's end was reported, {what} was {other:?}"));
            }
        }
    }
    if state(&untied.process)? != State::Running {
        return Err("an untied child ended with its creator".into());
    }
    untied.process.stop(0).map_err(|e| format!("{e}"))?;
    wait_end(&untied.process)?;

    // A creator that ends by a fault takes its tied child with it too.
    let top = start("tied 1 fault")?;
    let got = reports(&report, 1)?;
    match wait_end(&top.process)? {
        State::Faulted(_) => {}
        other => return Err(format!("the faulting creator ended {other:?}")),
    }
    match state(&got[0].process)? {
        State::Stopped(TIED_STATUS) => Ok(()),
        other => Err(format!("a faulting creator's tied child was {other:?}")),
    }
}
