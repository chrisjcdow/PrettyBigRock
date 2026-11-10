// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The tests of kernel interface 1.3 (docs/design/0019-kernel-1.3.md, section 5), run by the
//! root program as part of the program self-test:
//! - **receiver watches**: a client is signalled when nobody can receive on the endpoint it
//!   calls any more (its service has gone): once, at the right moment, and only if the
//!   capability the watch was set through is still the client's;
//! - **the machine capability**: who may turn the machine off or restart it, and what is
//!   refused. (That restarting really restarts is shown at the very end of the self-test, by
//!   doing it: see `selftest::run`.)
//!
//! Each check that fails says which, in words; nothing here panics.

use crate::selftest::{Files, MIB, Outcome, spawn};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use granite::endpoint::{Endpoint, Event};
use granite::message::{CapSend, Outgoing};
use granite::notification::Notification;
use granite::process;
use granite::sys::{self, Error, kind, num, rights};
use granite::{thread, time};
use pbr_test_protocol::{op, role};

/// The kernel's charge for a receiver watch: what a notification costs (note 0019, section 2).
const WATCH_CHARGE: u64 = 256;
/// What a client's capability to a service has.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;

/// Runs every kernel 1.3 test. `power` is the root program's machine capability (its raw
/// handle, which stays the root program's).
pub fn run(f: &Files<'_>, echo: &Endpoint, power: Option<u32>) -> Outcome {
    match sys::version() {
        (1, 3) => {}
        (a, b) => return Err(format!("the kernel says it is interface version {a}.{b}, not 1.3")),
    }
    watches(f, echo).map_err(|e| format!("receiver watches: {e}"))?;
    let power = power.ok_or("the root program wasn't given the machine capability")?;
    machine(power).map_err(|e| format!("the machine capability: {e}"))
}

/// How much of this program's budget is in use.
fn used() -> Result<u64, String> {
    process::info(process::self_handle()).map(|i| i.used).map_err(|e| format!("{e}"))
}

/// A new notification. (`map_err` turns the kernel's error into the text these tests report.)
fn notification() -> Result<Notification, String> {
    Notification::create().map_err(|e| format!("creating a notification: {e}"))
}

/// Expects exactly `bits` to be set on `n` already, and takes them.
fn expect_bits(n: &Notification, bits: u64, what: &str) -> Outcome {
    match n.wait(time::NOW) {
        Ok(b) if b == bits => Ok(()),
        Ok(b) => Err(format!("{what}: the bits were {b:#x}, not {bits:#x}")),
        Err(e) => Err(format!("{what}: no signal ({e})")),
    }
}

/// Expects nothing to be set on `n`.
fn expect_quiet(n: &Notification, what: &str) -> Outcome {
    match n.wait(time::NOW) {
        Err(Error::TimedOut) => Ok(()),
        Ok(b) => Err(format!("{what}: bits {b:#x} were signalled")),
        Err(e) => Err(format!("{what}: waiting failed ({e})")),
    }
}

/// A new endpoint and a client's capability to it (badge 1).
fn service() -> Result<(Endpoint, Endpoint), String> {
    let e = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;
    let c = e.mint(1, CLIENT).map_err(|e| format!("minting: {e}"))?;
    Ok((e, c))
}

fn watches(f: &Files<'_>, echo: &Endpoint) -> Outcome {
    let n = notification()?;

    // Refusals: no send right, a notification that can't be signalled, the wrong types.
    let (e, c) = service()?;
    let no_send = Endpoint(e.0.duplicate(rights::ALL & !rights::SEND).map_err(|e| format!("{e}"))?);
    if no_send.watch_receiver(&n, 1).err() != Some(Error::NoRight) {
        return Err("a capability without the send right could watch".into());
    }
    let no_signal =
        Notification(n.0.duplicate(rights::ALL & !rights::SIGNAL).map_err(|e| format!("{e}"))?);
    if c.watch_receiver(&no_signal, 1).err() != Some(Error::NoRight) {
        return Err("a notification without the signal right was accepted".into());
    }
    let a = [n.0.raw().into(), n.0.raw().into(), 1, 0, 0, 0];
    if sys::call(num::ENDPOINT_WATCH_RECEIVER, a).err() != Some(Error::WrongType) {
        return Err("watching a notification as if it were an endpoint wasn't refused".into());
    }
    let a = [c.0.raw().into(), c.0.raw().into(), 1, 0, 0, 0];
    if sys::call(num::ENDPOINT_WATCH_RECEIVER, a).err() != Some(Error::WrongType) {
        return Err("an endpoint given as the notification wasn't refused".into());
    }
    expect_quiet(&n, "after the refusals")?;
    // (`no_send` can still receive, so it has to go before the next check's "last receiver".)
    drop((no_send, no_signal));

    // It fires when the last capability that can receive goes, not before; and its record
    // is charged to whoever set it until then.
    let before = used()?;
    c.watch_receiver(&n, 0x4).map_err(|e| format!("setting a watch: {e}"))?;
    if used()?.checked_sub(before) != Some(WATCH_CHARGE) {
        return Err(format!("a watch wasn't charged {WATCH_CHARGE} bytes"));
    }
    let second = e.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?;
    drop(e);
    expect_quiet(&n, "with one of two receiving capabilities closed")?;
    let charged = used()?;
    drop(second);
    expect_bits(&n, 0x4, "after the last receiving capability was closed")?;
    if charged.checked_sub(used()?) != Some(WATCH_CHARGE) {
        return Err("a watch's charge wasn't given back when it fired".into());
    }
    // Once: nothing more comes, and the client's calls fail as they always did.
    expect_quiet(&n, "after a watch fired")?;
    if c.call(&Outgoing::new(1, &[]), &mut [], 0, time::NOW).err() != Some(Error::PeerGone) {
        return Err("a call to an endpoint nobody can receive on didn't fail with PeerGone".into());
    }
    // On an endpoint that already has no receiver, it fires at once, and keeps no record.
    let before = used()?;
    c.watch_receiver(&n, 0x8).map_err(|e| format!("watching a dead endpoint: {e}"))?;
    expect_bits(&n, 0x8, "watching an endpoint that already has no receiver")?;
    if used()? != before {
        return Err("a watch that fired at once kept its charge".into());
    }
    drop(c);

    // Setting it again through the same capability replaces it: one record, the new bits.
    let (e, c) = service()?;
    let before = used()?;
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    c.watch_receiver(&n, 0x2).map_err(|e| format!("replacing a watch: {e}"))?;
    if used()?.checked_sub(before) != Some(WATCH_CHARGE) {
        return Err("replacing a watch left two records charged".into());
    }
    // Several clients can each watch the same endpoint, through their own capabilities.
    let c2 = e.mint(2, CLIENT).map_err(|e| format!("{e}"))?;
    let c3 = e.mint(3, CLIENT).map_err(|e| format!("{e}"))?;
    c2.watch_receiver(&n, 0x10).map_err(|e| format!("{e}"))?;
    c3.watch_receiver(&n, 0x20).map_err(|e| format!("{e}"))?;
    drop(e);
    expect_bits(&n, 0x32, "three clients watching one endpoint")?;
    drop((c, c2, c3));

    // The watch belongs to the capability it was set through. Closing that capability
    // removes it (and gives its charge back); a copy made from it has none.
    let (e, c) = service()?;
    let copy = Endpoint(c.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?);
    let before = used()?;
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    drop(c);
    if used()? != before {
        return Err("closing a capability didn't give its watch's charge back".into());
    }
    drop(e);
    expect_quiet(&n, "after the watching capability was closed (a copy of it remains)")?;
    drop(copy);

    // Revoking it removes it too. Here `parent` is revoked, which destroys `c`, made from it.
    let e = Endpoint::create().map_err(|e| format!("{e}"))?;
    let parent = e.mint(1, CLIENT | rights::REVOKE).map_err(|e| format!("{e}"))?;
    let c = Endpoint(parent.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?);
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    parent.0.revoke().map_err(|e| format!("revoking: {e}"))?;
    drop(e);
    expect_quiet(&n, "after the watching capability was revoked")?;
    drop((c, parent));

    // And so does moving it: the echo service is sent the capability and sends it straight
    // back (the same capability, by the kernel's reckoning, in a new place twice over).
    let (e, c) = service()?;
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    let before = used()?;
    let caps = [CapSend::moved(c.0, CLIENT)];
    let m = Outgoing { label: op::ECHO, data: &[], caps: &caps };
    let mut r = echo
        .call(&m, &mut [], 0, time::after_ms(10_000))
        .map_err(|e| format!("sending a capability to echo and back: {e}"))?;
    let back = Endpoint(r.take_cap(0).ok_or("echo didn't send the capability back")?);
    if before.checked_sub(used()?) != Some(WATCH_CHARGE) {
        return Err("moving a capability didn't give its watch's charge back".into());
    }
    drop(e);
    expect_quiet(&n, "after the watching capability was moved away and back")?;
    // The capability itself still works as one: a new watch through it fires at once.
    back.watch_receiver(&n, 0x40).map_err(|e| format!("{e}"))?;
    expect_bits(&n, 0x40, "a new watch through the moved capability")?;
    drop(back);

    // Receivers going by revocation count like any other: with three capabilities that can
    // receive (`e`, `r` made from it, `r2` made from `r`), revoking `r` destroys only `r2`.
    // The watch fires when the last of them goes, however it goes.
    let (e, c) = service()?;
    let r = e.0.duplicate(rights::ALL).map_err(|e| format!("{e}"))?;
    let r2 = r.duplicate(rights::ALL).map_err(|e| format!("{e}"))?;
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    r.revoke().map_err(|e| format!("revoking: {e}"))?;
    if r2.info().err() != Some(Error::BadHandle) {
        return Err("a revoked receiving capability still worked".into());
    }
    expect_quiet(&n, "after one receiving capability was revoked")?;
    drop((r2, r));
    expect_quiet(&n, "with the original receiving capability still held")?;
    drop(e);
    expect_bits(&n, 0x1, "after revoking and closing every receiving capability")?;
    drop(c);

    // A call waiting when the last receiver goes fails with PeerGone, and the watch fires.
    let (e, c) = service()?;
    let caller_cap = e.mint(9, CLIENT).map_err(|e| format!("{e}"))?;
    c.watch_receiver(&n, 0x1).map_err(|e| format!("{e}"))?;
    // The caller's thread says how its call ended through a shared flag. `Arc` is a
    // reference-counted pointer two threads can both hold; an atomic is a value they can both
    // change safely; `move` gives the new thread its own `Arc` and the capability.
    let ended = Arc::new(AtomicU32::new(0));
    let flag = ended.clone();
    let caller = thread::spawn(move || {
        let r = caller_cap.call(&Outgoing::new(1, &[]), &mut [], 0, time::after_ms(30_000));
        let how = match r.err() {
            Some(Error::PeerGone) => 1,
            Some(_) => 2,
            None => 3,
        };
        flag.store(how, Ordering::SeqCst);
    })
    .map_err(|e| format!("starting a thread: {e}"))?;
    time::sleep_ms(300); // give the caller time to be waiting
    drop(e);
    caller.join().map_err(|e| format!("joining the caller: {e}"))?;
    match ended.load(Ordering::SeqCst) {
        1 => {}
        2 => return Err("a waiting call failed, but not with PeerGone".into()),
        _ => return Err("a waiting call was answered though nobody could receive it".into()),
    }
    expect_bits(&n, 0x1, "with a call waiting when the last receiver went")?;
    drop(c);

    // A service that is itself a client binds the notification to its own endpoint, and the
    // news arrives in its receive loop.
    let (e, c) = service()?;
    let mine = Endpoint::create().map_err(|e| format!("{e}"))?;
    let bound = notification()?;
    mine.bind(&bound).map_err(|e| format!("binding: {e}"))?;
    c.watch_receiver(&bound, 0x80).map_err(|e| format!("{e}"))?;
    drop(e);
    match mine.receive(&mut [], time::NOW) {
        Ok(Event::Signal(0x80)) => {}
        _ => return Err("a watch's bits didn't arrive through a bound notification".into()),
    }
    drop(c);

    in_another_process(f, &n)
}

/// The case the call exists for: the service is another process, and it ends.
fn in_another_process(f: &Files<'_>, n: &Notification) -> Outcome {
    let (e, c) = service()?;
    let server =
        spawn(f.echo, "echo", "", &[(role::ENDPOINT, e.0.raw(), rights::RECEIVE, false)], 8 * MIB)?;
    // The root program's own capability goes, so only the new process can receive. (Closing
    // a capability leaves the ones made from it working: note 0006, section 5.)
    drop(e);
    c.watch_receiver(n, 0x100).map_err(|e| format!("{e}"))?;
    // The service is there and answering.
    let r = c
        .call(&Outgoing::new(op::BADGE, &[]), &mut [], 0, time::after_ms(30_000))
        .map_err(|e| format!("calling the new service: {e}"))?;
    if r.data().get(..8) != Some(&1u64.to_le_bytes()[..]) {
        return Err("the new service didn't see the client's badge".into());
    }
    expect_quiet(n, "while the service was running")?;
    server.process.stop(0).map_err(|e| format!("stopping the service: {e}"))?;
    // Waits for the signal itself this time: the process's capabilities go as it ends.
    match n.wait(time::after_ms(30_000)) {
        Ok(0x100) => Ok(()),
        other => Err(format!("after the service was stopped, the watch gave {other:?}")),
    }
}

/// `machine_power` through the raw handle `h` with `action`: the error it came back with.
/// (It only ever comes back with an error.)
fn power(h: u32, action: u64) -> Option<Error> {
    sys::call(num::MACHINE_POWER, [h.into(), action, 0, 0, 0, 0]).err()
}

fn machine(h: u32) -> Outcome {
    // What the root program was given: the machine, with the power right.
    let a = [h.into(), 0, 0, 0, 0, 0];
    let (k, r) = sys::call(num::HANDLE_INFO, a).map_err(|e| format!("handle_info: {e}"))?;
    if k != u64::from(kind::MACHINE) || r as u32 & rights::POWER == 0 {
        return Err(format!("start-up role 13 is type {k} with rights {r:#x}"));
    }
    // Refusals. None of these may stop anything: the tests that follow run on every core.
    let a = [h.into(), u64::from(rights::ALL & !rights::POWER), 0, 0, 0, 0];
    let (weak, _) = sys::call(num::HANDLE_DUPLICATE, a).map_err(|e| format!("copying: {e}"))?;
    let weak = granite::Handle::from_raw(weak as u32);
    for action in [1, 2] {
        if power(weak.raw(), action) != Some(Error::NoRight) {
            return Err(format!("action {action} without the power right wasn't refused"));
        }
    }
    drop(weak);
    let n = notification()?;
    if power(n.0.raw(), 2) != Some(Error::WrongType) {
        return Err("a notification passed as the machine wasn't refused".into());
    }
    if power(0, 2) != Some(Error::BadHandle) {
        return Err("handle 0 as the machine wasn't refused".into());
    }
    for action in [0, 3, u64::MAX] {
        if power(h, action) != Some(Error::InvalidArgument) {
            return Err(format!("action {action} wasn't refused"));
        }
    }
    // The kernel can't turn a PC off (the power service does, note 0024), and says so before
    // it stops anything. On ARM64 it can, so asking would end the test there and then.
    if cfg!(target_arch = "x86_64") && power(h, 1) != Some(Error::NotSupported) {
        return Err("turning off on x86-64 didn't answer NotSupported".into());
    }
    Ok(())
}
