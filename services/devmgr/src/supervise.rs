// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Watching the drivers, and starting one again when it ends
//! (docs/design/0012-driver-restart-and-m4-tests.md, sections 1 to 4).
//!
//! A driver is not supposed to end, so any end is a crash: a fault, an exit, or a stop. What
//! happens then, in order: record it; revoke what the driver may have passed on; switch the
//! device off, so that nothing is in flight; zero the DMA memory (which the device manager
//! keeps, so the device only ever wrote into memory that was its own); decide, from the
//! device's history, whether and when to restart; start the new driver with the same
//! capabilities.

use crate::drivers::{self, Driver, Plan};
use crate::{REGISTRY, Registry};
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::notification::Notification;
use granite::process::State;
use granite::time;
use pbr_devices::msix;
use pbr_devices::pci::{self, command};
use pbr_interfaces::pbr_devices::DeviceState;
use pbr_supervise::Decision;

/// How long to wait at most for a notification. The kernel tells us when a process ends; this
/// only makes sure a missed wake-up can't leave a crash unnoticed for long.
const POLL_MS: u64 = 500;

/// The clock, in milliseconds.
pub fn now_ms() -> u64 {
    time::now() / 1_000_000
}

/// True if the driver's process is still running.
pub fn alive(p: &granite::process::Spawned) -> bool {
    p.process.info().is_ok_and(|i| i.state == State::Running)
}

/// Watches the drivers for ever.
pub fn run(drivers: &[Driver<'_>], host: &Endpoint, selftest: bool) -> ! {
    let n = match Notification::create() {
        Ok(n) => n,
        Err(e) => {
            granite::println!("devmgr: can't watch the drivers: {e}");
            granite::process::exit(1);
        }
    };
    for e in REGISTRY.lock().devices.iter() {
        if let Some(p) = &e.process {
            let _ = p.process.watch(&n, 1);
        }
    }
    loop {
        for id in ended() {
            crashed(id);
        }
        let now = now_ms();
        for id in due(now) {
            restart(id, drivers, host, selftest, &n);
        }
        let wait = next_due().map_or(POLL_MS, |t| t.saturating_sub(now_ms()).min(POLL_MS));
        // Timing out is how the loop gets round again.
        let _ = n.wait(time::after_ms(wait));
    }
}

/// The devices whose driver has ended.
fn ended() -> Vec<u32> {
    let g = REGISTRY.lock();
    g.devices
        .iter()
        .filter(|e| {
            e.process
                .as_ref()
                .is_some_and(|p| p.process.info().is_ok_and(|i| i.state != State::Running))
        })
        .map(|e| e.id)
        .collect()
}

/// The devices whose restart is due.
fn due(now: u64) -> Vec<u32> {
    let g = REGISTRY.lock();
    g.devices.iter().filter(|e| e.due.is_some_and(|t| t <= now)).map(|e| e.id).collect()
}

fn next_due() -> Option<u64> {
    REGISTRY.lock().devices.iter().filter_map(|e| e.due).min()
}

fn entry(g: &mut Registry, id: u32) -> Option<&mut crate::Entry> {
    g.devices.iter_mut().find(|e| e.id == id)
}

/// The driver for device `id` has ended.
fn crashed(id: u32) {
    let t_detected = now_ms();
    // Take what the driver had out, so that nothing is held while the slow parts happen. A
    // client that asks meanwhile is told the device isn't ready.
    let (process, plan, name) = {
        let mut g = REGISTRY.lock();
        let Some(e) = entry(&mut g, id) else { return };
        e.state = DeviceState::Restarting;
        e.service = None;
        (e.process.take(), e.plan.take(), e.name.clone())
    };
    // 1. Record it, once its threads have all stopped (a fault ends a process by stopping the
    // other threads, which takes a moment, and a new driver mustn't start before the old one
    // has let go of the device).
    if let Some(p) = &process {
        let t0 = time::now();
        let mut state = p.process.info().map(|i| i.state);
        while matches!(state, Ok(State::Ending)) && time::now() - t0 < 2_000_000_000 {
            time::sleep_ms(1);
            state = p.process.info().map(|i| i.state);
        }
        match state {
            Ok(s) => granite::println!("devmgr: {name} ended: {s:?}."),
            Err(e) => granite::println!("devmgr: {name} ended ({e})."),
        }
    }
    drop(process);
    let mut plan = plan;
    if let Some(plan) = &mut plan {
        // 2. Revoke every copy the driver may have passed on.
        revoke(plan);
        // 3. Switch the device off.
        quiet(plan);
        // 4. Zero the DMA memory, to be used again.
        if let Some(dma) = &plan.dma {
            zero(dma);
        }
    }
    // 5. Decide.
    let now = now_ms();
    granite::println!("devmgr: {name} made safe in {} ms.", now - t_detected);
    let mut g = REGISTRY.lock();
    let Some(e) = entry(&mut g, id) else { return };
    e.plan = plan;
    if e.plan.is_none() {
        e.state = DeviceState::Disabled;
        return;
    }
    match e.history.crashed(now) {
        Decision::Restart { delay_ms } => {
            e.restarts += 1;
            e.due = Some(now + delay_ms);
        }
        Decision::GiveUp => {
            e.state = DeviceState::Disabled;
            e.due = None;
            granite::println!("devmgr: {name} crashed too often; it is left alone.");
        }
    }
}

/// Removes the copies of everything the plan holds (the driver's own table went when it ended;
/// this takes back any it passed on).
fn revoke(plan: &Plan) {
    let _ = plan.record.0.revoke();
    if let Some(pci) = &plan.pci {
        let _ = pci.config.0.revoke();
    }
    for p in [&plan.ports, &plan.ports2].into_iter().flatten() {
        let _ = p.0.revoke();
    }
    for (_, b) in &plan.bars {
        let _ = b.0.revoke();
    }
    for m in &plan.irqs {
        let _ = m.0.revoke();
    }
    if let Some(d) = &plan.dma {
        let _ = d.0.revoke();
    }
}

/// Stops a PCI function from doing DMA or raising interrupts.
fn quiet(plan: &mut Plan) {
    // Legacy devices have no bus mastering to switch off, and the line is masked when the
    // driver's binding goes with its process.
    let Some(pci) = &mut plan.pci else { return };
    let cfg = &mut pci.page;
    // MSI-X off first, then the function's bus-mastering and decoding.
    let (caps, _) = pci::capabilities(cfg);
    if let Some(info) = msix::info(cfg, &caps) {
        msix::disable(cfg, &info);
    }
    pci::update_command(cfg, 0, command::BUS_MASTER | command::MEMORY | command::IO);
}

/// Fills the memory with zeros.
fn zero(mem: &granite::memory::Memory) {
    let Ok(size) = mem.size() else { return };
    let block = vec![0u8; 16384];
    let mut at = 0;
    while at < size {
        let n = (size - at).min(block.len() as u64) as usize;
        if mem.write(at, &block[..n]).is_err() {
            granite::println!("devmgr: can't zero a device's DMA memory.");
            return;
        }
        at += n as u64;
    }
}

/// Starts the driver for device `id` again.
fn restart(id: u32, drivers: &[Driver<'_>], host: &Endpoint, selftest: bool, n: &Notification) {
    let (plan, driver, function, name) = {
        let mut g = REGISTRY.lock();
        let Some(e) = entry(&mut g, id) else { return };
        e.due = None;
        (e.plan.take(), e.driver, e.label.clone(), e.name.clone())
    };
    let result = match (&plan, drivers.get(driver)) {
        (Some(plan), Some(d)) => drivers::start(d, plan, host, id, &function, selftest),
        _ => Err(drivers::PlanError("nothing to start it with".into())),
    };
    let mut g = REGISTRY.lock();
    let Some(e) = entry(&mut g, id) else { return };
    e.plan = plan;
    match result {
        Ok(s) => {
            let _ = s.process.watch(n, 1);
            e.history.started(now_ms());
            e.started_at = now_ms();
            e.process = Some(s);
            let (used, limit) = granite::process::info(granite::process::self_handle())
                .map_or((0, 0), |i| (i.used, i.budget));
            granite::println!(
                "devmgr: {name} started again ({} restarts; memory {used} of {limit}).",
                e.restarts
            );
        }
        Err(why) => {
            e.state = DeviceState::Disabled;
            granite::println!("devmgr: {name} can't be started again: {}", why.0);
        }
    }
}
