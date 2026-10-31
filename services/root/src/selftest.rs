// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The program self-test (docs/design/0009-programs-and-granite.md, section 9): M3's "done
//! when", checked from inside running programs. Two programs exchange messages and
//! capabilities, a program can't reach anything it wasn't given, and revocation works.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::memory::Memory;
use granite::message::Outgoing;
use granite::notification::Notification;
use granite::process::{self, Grant, Spawn, Spawned, State};
use granite::sys::{Error, perm, rights};
use granite::time;
use pbr_archive::Archive;
use pbr_test_protocol::{op, role};

const MIB: u64 = 1024 * 1024;
const MEMORY_FAULT: u64 = 1;
const INSTRUCTION_FAULT: u64 = 2;
/// Rights a client's endpoint capability gets.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;

type Outcome = Result<(), String>;

struct Files<'a> {
    echo: &'a [u8],
    probe: &'a [u8],
}

/// Runs every test; returns the process's exit status (0 if all passed).
pub fn run(archive: &Archive<'_>) -> i32 {
    let (Some(echo), Some(probe)) = (archive.find("echo"), archive.find("probe")) else {
        granite::println!("Program self-test FAILED: the test programs aren't in the archive");
        return 1;
    };
    let f = Files { echo: echo.data, probe: probe.data };
    match all(&f) {
        Ok(summary) => {
            granite::println!("Program self-test passed: {summary}");
            0
        }
        Err(e) => {
            granite::println!("Program self-test FAILED: {e}");
            1
        }
    }
}

fn all(f: &Files<'_>) -> Result<String, String> {
    let echo_ep = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;
    let echo = spawn(
        f.echo,
        "echo",
        "threads=4",
        &[(role::ENDPOINT, echo_ep.0.raw(), rights::RECEIVE, false)],
        16 * MIB,
    )?;
    let mut done: Vec<&str> = Vec::new();

    step("messages and capabilities", messages(f, &echo_ep))?;
    done.push("messages and capabilities");
    step("isolation", isolation(f, &echo_ep))?;
    done.push("isolation");
    let faults = faults(f);
    step("faults", faults.as_ref().map(|_| ()).map_err(Clone::clone))?;
    done.push("4 kinds of fault");
    step("revoking an endpoint", revoke_endpoint(f, &echo_ep))?;
    step("revoking memory", revoke_memory(f))?;
    done.push("revocation of endpoints and memory");
    step("the last sender signal", last_sender(f))?;
    done.push("the last sender signal");
    step("a crashing service", crashing_service(f))?;
    done.push("a crashing service");
    step("budgets", budgets(f))?;
    done.push("budgets");
    let calls = load(f, &echo_ep);
    step("load", calls.as_ref().map(|_| ()).map_err(Clone::clone))?;
    done.push("calls on 4 threads to a service with 4 threads");

    echo.process.stop(0).map_err(|e| format!("stopping echo: {e}"))?;
    match wait(&echo, 30)? {
        State::Stopped(0) => {}
        other => return Err(format!("echo ended {other:?} instead of being stopped")),
    }
    Ok(format!("{} ({} calls)", done.join(", "), calls.unwrap_or(0)))
}

fn step(name: &str, r: Outcome) -> Outcome {
    r.map_err(|e| format!("{name}: {e}"))
}

fn spawn(
    file: &[u8],
    name: &str,
    args: &str,
    caps: &[Grant],
    budget: u64,
) -> Result<Spawned, String> {
    process::spawn(&Spawn { name, program: file, budget, priority: 1, caps, log: true, args })
        .map_err(|e| format!("starting {name} ({args}): {e}"))
}

/// Waits up to `secs` seconds for a process to end.
fn wait(s: &Spawned, secs: u64) -> Result<State, String> {
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    s.process.watch(&n, 1).map_err(|e| format!("{e}"))?;
    match n.wait(time::after_ms(secs * 1000)) {
        Ok(_) => s.process.info().map(|i| i.state).map_err(|e| format!("{e}")),
        Err(Error::TimedOut) => {
            let _ = s.process.stop(999);
            Err(format!("still running after {secs} seconds"))
        }
        Err(e) => Err(format!("{e}")),
    }
}

/// Runs a probe and expects it to exit with status 0.
fn probe(f: &Files<'_>, args: &str, caps: &[Grant]) -> Outcome {
    let s = spawn(f.probe, "probe", args, caps, 8 * MIB)?;
    match wait(&s, 60)? {
        State::Exited(0) => Ok(()),
        State::Exited(n) => Err(format!("probe {args}: check {n} failed")),
        other => Err(format!("probe {args} ended {other:?}")),
    }
}

fn mint(ep: &Endpoint, badge: u64) -> Result<Endpoint, String> {
    ep.mint(badge, CLIENT | rights::REVOKE).map_err(|e| format!("minting: {e}"))
}

fn messages(f: &Files<'_>, ep: &Endpoint) -> Outcome {
    let (a, b) = (mint(ep, 11)?, mint(ep, 22)?);
    probe(
        f,
        "messages 11 22",
        &[
            (role::ENDPOINT, a.0.into_raw(), CLIENT, true),
            (role::ENDPOINT2, b.0.into_raw(), CLIENT, true),
        ],
    )
}

fn isolation(f: &Files<'_>, ep: &Endpoint) -> Outcome {
    let m = Memory::create(4096).map_err(|e| format!("{e}"))?;
    let no_send =
        ep.0.duplicate(rights::TRANSFER | rights::DUPLICATE).map_err(|e| format!("{e}"))?;
    probe(
        f,
        "isolation",
        &[
            (
                role::MEMORY_NO_MAP,
                m.0.raw(),
                rights::READ | rights::DUPLICATE | rights::TRANSFER,
                false,
            ),
            (role::NO_SEND, no_send.into_raw(), rights::TRANSFER | rights::DUPLICATE, true),
        ],
    )
}

fn faults(f: &Files<'_>) -> Outcome {
    for (mode, kind) in [
        ("fault-readonly", MEMORY_FAULT),
        ("fault-kernel", MEMORY_FAULT),
        ("fault-null", MEMORY_FAULT),
        ("fault-instruction", INSTRUCTION_FAULT),
    ] {
        let s = spawn(f.probe, "probe", mode, &[], 8 * MIB)?;
        match wait(&s, 60)? {
            State::Faulted(fault) if fault.kind == kind => {}
            other => {
                return Err(format!(
                    "probe {mode} ended {other:?}, not with a fault of kind {kind}"
                ));
            }
        }
    }
    Ok(())
}

fn revoke_endpoint(f: &Files<'_>, ep: &Endpoint) -> Outcome {
    let r = mint(ep, 44)?;
    let ready = Notification::create().map_err(|e| format!("{e}"))?;
    let s = spawn(
        f.probe,
        "probe",
        "revoke-endpoint",
        &[
            (role::ENDPOINT, r.0.raw(), CLIENT, false),
            (role::READY, ready.0.raw(), rights::SIGNAL, false),
        ],
        8 * MIB,
    )?;
    ready.wait(time::after_ms(30_000)).map_err(|e| format!("the probe never got going: {e}"))?;
    r.0.revoke().map_err(|e| format!("revoking: {e}"))?;
    match wait(&s, 60)? {
        State::Exited(0) => {}
        other => return Err(format!("the probe ended {other:?}")),
    }
    // The root program's own capability still works.
    let reply = r.call(&Outgoing::new(op::BADGE, &[]), &mut [], 0, time::after_ms(10_000));
    match reply {
        Ok(m) if m.data() == 44u64.to_le_bytes() => Ok(()),
        Ok(_) => Err("the kept capability has the wrong badge".into()),
        Err(e) => Err(format!("the kept capability stopped working: {e}")),
    }
}

fn revoke_memory(f: &Files<'_>) -> Outcome {
    let m = Memory::create(4096).map_err(|e| format!("{e}"))?;
    let ready = Notification::create().map_err(|e| format!("{e}"))?;
    let s = spawn(
        f.probe,
        "probe",
        "revoke-memory",
        &[
            (role::MEMORY, m.0.raw(), rights::READ | rights::WRITE | rights::MAP, false),
            (role::READY, ready.0.raw(), rights::SIGNAL, false),
        ],
        8 * MIB,
    )?;
    ready.wait(time::after_ms(30_000)).map_err(|e| format!("the probe never got going: {e}"))?;
    m.0.revoke().map_err(|e| format!("revoking: {e}"))?;
    match wait(&s, 60)? {
        State::Faulted(fault) if fault.kind == MEMORY_FAULT => {}
        other => return Err(format!("the probe ended {other:?}, not with a memory fault")),
    }
    // The memory, and what the probe wrote, are still there for the root program.
    let page = m.map(perm::READ).map_err(|e| format!("mapping it again: {e}"))?;
    for i in 0..4096u64 {
        // SAFETY: inside the mapping just made.
        let b = unsafe { core::ptr::read_volatile(page.as_ptr().add(i as usize)) };
        if b != (i * 13) as u8 {
            return Err(format!("byte {i} is {b}, not {}", (i * 13) as u8));
        }
    }
    Ok(())
}

fn last_sender(f: &Files<'_>) -> Outcome {
    let l = Endpoint::create().map_err(|e| format!("{e}"))?;
    let gone = Notification::create().map_err(|e| format!("{e}"))?;
    l.watch(&gone, 1).map_err(|e| format!("watching: {e}"))?;
    let s = mint(&l, 55)?;
    let _receiving = l.0.duplicate(rights::RECEIVE).map_err(|e| format!("{e}"))?;
    drop(l); // now the only sender is `s`
    if gone.wait(time::NOW) != Err(Error::TimedOut) {
        return Err("the signal came while a sender was left".into());
    }
    let ready = Notification::create().map_err(|e| format!("{e}"))?;
    let p = spawn(
        f.probe,
        "probe",
        "sender",
        &[
            (role::ENDPOINT, s.0.into_raw(), CLIENT, true),
            (role::READY, ready.0.raw(), rights::SIGNAL, false),
        ],
        8 * MIB,
    )?;
    ready.wait(time::after_ms(30_000)).map_err(|e| format!("the probe never got going: {e}"))?;
    if gone.wait(time::NOW) != Err(Error::TimedOut) {
        return Err("the signal came while the probe still held its capability".into());
    }
    match wait(&p, 60)? {
        State::Exited(0) => {}
        other => return Err(format!("the probe ended {other:?}")),
    }
    gone.wait(time::after_ms(10_000)).map(|_| ()).map_err(|e| format!("no signal: {e}"))
}

fn crashing_service(f: &Files<'_>) -> Outcome {
    let e = Endpoint::create().map_err(|e| format!("{e}"))?;
    let client = mint(&e, 66)?;
    // The service gets the only capability that can receive.
    let svc = spawn(
        f.echo,
        "echo",
        "threads=1",
        &[(role::ENDPOINT, e.0.into_raw(), rights::RECEIVE, true)],
        4 * MIB,
    )?;
    probe(f, "crash-service", &[(role::ENDPOINT, client.0.into_raw(), CLIENT, true)])?;
    match wait(&svc, 30)? {
        State::Faulted(fault) if fault.kind == MEMORY_FAULT => Ok(()),
        other => Err(format!("the service ended {other:?}, not with a memory fault")),
    }
}

fn budgets(f: &Files<'_>) -> Outcome {
    let me = process::self_handle();
    let used = || process::info(me).map(|i| i.used).map_err(|e| format!("{e}"));
    let budget = 512 * 1024;
    let before = used()?;
    let s = spawn(f.probe, "probe", &format!("budget {budget}"), &[], budget)?;
    let during = used()?;
    if during < before + budget {
        return Err(format!(
            "the child's budget wasn't taken from this one ({before} then {during})"
        ));
    }
    match wait(&s, 60)? {
        State::Exited(0) => {}
        State::Exited(n) => return Err(format!("probe budget: check {n} failed")),
        other => return Err(format!("the probe ended {other:?}")),
    }
    drop(s); // its process and thread capabilities: the last references to its budget
    // Everything comes back: the child's budget and the memory made for it. (This program's
    // own allocator may have taken a little more meanwhile.)
    let until = time::after_ms(5_000);
    loop {
        let after = used()?;
        if after + budget <= during && after <= before + 128 * 1024 {
            return Ok(());
        }
        if time::now() > until {
            return Err(format!(
                "not all memory came back: {before} before, {during} during, {after} after"
            ));
        }
        time::sleep_ms(10);
    }
}

/// Returns how many calls the probe made.
fn load(f: &Files<'_>, ep: &Endpoint) -> Result<u64, String> {
    const THREADS: u64 = 4;
    const CALLS: u64 = 300;
    let mine = mint(ep, 88)?;
    let count = || -> Result<u64, String> {
        let m = mine
            .call(&Outgoing::new(op::COUNT, &[]), &mut [], 0, time::after_ms(10_000))
            .map_err(|e| format!("{e}"))?;
        Ok(u64::from_le_bytes(m.data().try_into().map_err(|_| "a short reply")?))
    };
    let c0 = count()?;
    let theirs = mint(ep, 99)?;
    probe(
        f,
        &format!("load {THREADS} {CALLS}"),
        &[(role::ENDPOINT, theirs.0.into_raw(), CLIENT, true)],
    )?;
    let c1 = count()?;
    // Every call is accounted for: the probe's, and this program's second COUNT.
    if c1 - c0 != THREADS * CALLS + 1 {
        return Err(format!("the service answered {} calls, not {}", c1 - c0 - 1, THREADS * CALLS));
    }
    Ok(THREADS * CALLS)
}
