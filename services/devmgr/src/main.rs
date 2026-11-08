// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The device manager (docs/design/0011-device-manager-and-drivers.md): finds the devices,
//! starts a driver for each, keeps the list of what is running, and hands clients the
//! endpoint of the driver for a device.
//!
//! It is started by the root program, which gives it the start-up archive and the device access
//! capabilities (note 0010, section 8). Its own endpoint (role 1000) is where clients call
//! `pbr.devices`; drivers report to a second endpoint it makes, with `pbr.driver.host`. The root
//! program supervises it (note 0018): it restarts the device manager if it ends, and keeps a copy
//! of every driver's DMA memory, through `pbr.supervisor` at role 5, until the next device
//! manager has switched the devices off. Its drivers are *tied* to it (note 0013), so they end
//! with it.

#![no_std]
#![no_main]

extern crate alloc;

mod drivers;
mod ecam;
mod phys;
mod supervise;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use granite::Handle;
use granite::device::{DeviceMemory, Interrupts, IoPorts};
use granite::endpoint::{Endpoint, ReplyCap};
use granite::lock::Lock;
use granite::memory::Memory;
use granite::message::Incoming;
use granite::process::{Spawned, role};
use granite::sys::{kind, perm, rights};
use granite::thread;
use pbr_archive::Archive;
use pbr_devices::machine::Machine;
use pbr_devices::manifest::{Class, Legacy, Manifest, Target, choose};
use pbr_devices::pci;
use pbr_driver::{ConfigPage, Mmio, Service, serve};
use pbr_interfaces::pbr_devices::{self as devices, DeviceInfo, DeviceState, DevicesError};
use pbr_interfaces::pbr_devices_debug::{self as devices_debug, DebugError};
use pbr_interfaces::pbr_driver_host::{self as host, HostError};
use pbr_interfaces::pbr_supervisor as supervisor;
use pbr_platform::acpi;
use pbr_supervise::History;

granite::main!(main);

/// The role the root program gives the device manager's public endpoint.
const PUBLIC: u32 = role::OWN;
/// The role the root program gives the device manager its kernel log with `CONTROL` at, to be
/// passed on to the display driver only (note 0010, section 7).
const LOG_CONTROL: u32 = 1001;
/// The most devices listed to a client (what `pbr.devices.list` can carry).
const MAX_LISTED: usize = 64;

/// A device with a driver.
struct Entry {
    id: u32,
    name: String,
    class: Class,
    state: DeviceState,
    restarts: u32,
    /// The driver's endpoint, once it has said it is ready.
    service: Option<Handle>,
    /// The driver's process.
    process: Option<Spawned>,
    /// What the driver was started with, kept for as long as the device exists: it keeps the
    /// interrupts and DMA memory alive, and a restarted driver is given the same ones.
    plan: Option<drivers::Plan>,
    /// Which driver (in the list `drivers::load` made) serves it.
    driver: usize,
    /// Where it is, for naming it (`00:03.0`, `com2`).
    label: String,
    /// When it crashed before.
    history: History,
    /// When (milliseconds on the clock) the next start is due, while it is waiting.
    due: Option<u64>,
    /// When the driver was last started (milliseconds on the clock).
    started_at: u64,
}

struct Registry {
    devices: Vec<Entry>,
}

static REGISTRY: Lock<Registry> = Lock::new(Registry { devices: Vec::new() });

/// How many PCI functions this instance switched off at start, and when it had finished
/// (nanoseconds on the kernel's clock), for `pbr.devices.debug.quieted`.
static QUIETED: Lock<(u32, u64)> = Lock::new((0, 0));

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(why) => {
            granite::println!("devmgr: {why}");
            1
        }
    }
}

fn take(start: &mut granite::Start, r: u32, what: &str) -> Result<Handle, String> {
    start.take(r).ok_or_else(|| alloc::format!("started without {what}"))
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let selftest = start.args().split_whitespace().any(|w| w == "selftest");
    let archive_mem = Memory(take(start, role::ARCHIVE, "the start-up archive")?);
    let memory = DeviceMemory(take(start, role::DEVICE_MEMORY, "device memory")?);
    let machine_mem = Memory(take(start, role::MACHINE, "the machine description")?);
    let pool = start.take(role::MSI_POOL).map(Interrupts);
    let io_ports = start.take(role::IO_PORTS).map(IoPorts);
    let lines = start.take(role::INTERRUPTS).map(Interrupts);
    let public = Endpoint(take(start, PUBLIC, "an endpoint to serve")?);
    let log_control = start.take(LOG_CONTROL);
    // Its supervisor (the root program), which keeps copies of DMA memory for it.
    let keeper = start.take(role::PARENT).map(|h| {
        let mut c = supervisor::Client::new(Endpoint(h));
        c.set_timeout_ms(10_000);
        c
    });

    let mapping =
        archive_mem.map(perm::READ).map_err(|e| alloc::format!("can't map the archive: {e}"))?;
    // SAFETY: the mapping is readable for its whole length and nothing writes to it.
    let bytes = unsafe { core::slice::from_raw_parts(mapping.as_ptr(), mapping.len() as usize) };
    let archive =
        Archive::parse(bytes).map_err(|e| alloc::format!("the archive is damaged: {e}"))?;
    let drivers = drivers::load(&archive);
    let manifests: Vec<Manifest> = drivers.iter().map(|d| d.manifest.clone()).collect();
    granite::println!("devmgr: {} drivers known.", drivers.len());

    let mut page =
        alloc::vec![0u8; machine_mem.size().map_err(|e| alloc::format!("{e}"))?.min(4096) as usize];
    machine_mem
        .read(0, &mut page)
        .map_err(|e| alloc::format!("can't read the machine description: {e}"))?;
    let machine = Machine::decode(&page, pbr_devices::machine::FRAMEBUFFER_RECORD)
        .map_err(|e| alloc::format!("the machine description is wrong: {e:?}"))?;
    granite::println!(
        "devmgr: machine: ACPI root {:#x}, {} interrupts, {} framebuffers.",
        machine.rsdp,
        machine.interrupts,
        machine.framebuffers
    );

    // Drivers report here.
    let host_ep = Endpoint::create().map_err(|e| alloc::format!("{e}"))?;
    let mut next_id = 1u32;

    // PCI Express. First every function a driver is chosen for is found and switched off,
    // before any driver starts (note 0018, section 4): if an earlier device manager died, its
    // devices might still be doing DMA into memory the supervisor is keeping for them. Only then
    // is that memory released. (Functions no driver is chosen for were never given to anyone,
    // and are left alone: one of them may be the display the kernel's own console is on.)
    let reader = phys::PhysReader::new(&memory);
    let mut regions = Vec::new();
    if machine.rsdp != 0
        && let Some(t) = acpi::find_table(&reader, machine.rsdp, b"MCFG")
    {
        acpi::mcfg(&reader, t, |r| regions.push(r));
    }
    if regions.is_empty() {
        granite::println!("devmgr: no PCI Express configuration space (no MCFG table).");
    }
    let mut found = Vec::new();
    for region in regions.into_iter().filter(|r| r.segment == 0) {
        let mut ecam = ecam::Ecam::new(&memory, region);
        let scan = pci::scan(&mut ecam, region.start_bus, region.end_bus);
        for p in &scan.problems {
            let a = p.addr;
            granite::println!(
                "devmgr: {:02x}:{:02x}.{}: {:?}",
                a.bus,
                a.device,
                a.function,
                p.kind
            );
        }
        for f in scan.functions {
            if f.class == 6 {
                continue; // bridges are followed by the scan, and need no driver
            }
            let a = f.addr;
            let target = Target::Pci {
                vendor: f.vendor,
                device: f.device,
                class: f.class,
                subclass: f.subclass,
                prog_if: f.prog_if,
            };
            let label = alloc::format!(
                "{:02x}:{:02x}.{} {:04x}:{:04x} (class {:02x}{:02x})",
                a.bus,
                a.device,
                a.function,
                f.vendor,
                f.device,
                f.class,
                f.subclass
            );
            let Some(i) = choose(&manifests, &target) else {
                granite::println!("devmgr: {label}: no driver.");
                continue;
            };
            granite::println!("devmgr: {label}: {}.", drivers[i].name);
            let Some(config) = ecam.function(a) else {
                granite::println!("devmgr: {label}: can't get its configuration space.");
                continue;
            };
            // The device manager's own mapping of the function's configuration space, made once
            // and kept with the device (`drivers::PciPart`).
            let page = match config.map(0, 4096, perm::READ_WRITE) {
                Ok(m) => ConfigPage(Mmio::new(m)),
                Err(e) => {
                    granite::println!("devmgr: {label}: can't map its configuration space: {e}");
                    continue;
                }
            };
            let mut page = page;
            pci::quiet(&mut page);
            found.push((f, i, config, page));
        }
    }
    *QUIETED.lock() = (found.len() as u32, granite::time::now());
    if let Some(s) = &keeper {
        match s.release_kept() {
            Ok(()) => granite::println!(
                "devmgr: {} devices switched off; what an earlier device manager left is released.",
                found.len()
            ),
            Err(e) => granite::println!("devmgr: can't release what was kept: {e}"),
        }
    }
    for (f, i, config, page) in found {
        let d = &drivers[i];
        let a = f.addr;
        let id = next_id;
        next_id += 1;
        let plan =
            drivers::plan(&f, config, page, &d.manifest, &memory, pool.as_ref(), keeper.as_ref())
                .map_err(|e| e.0);
        let label = alloc::format!("{:02x}:{:02x}.{}", a.bus, a.device, a.function);
        add(d, i, label, plan, &host_ep, id, selftest);
    }

    // The devices that aren't on PCI (note 0011, section 4).
    if let (Some(io), Some(lines)) = (&io_ports, &lines) {
        if com2_present(io) {
            legacy(
                &drivers,
                &manifests,
                Legacy::Com2,
                drivers::COM2,
                io,
                lines,
                &host_ep,
                &mut next_id,
                selftest,
            );
        } else {
            granite::println!("devmgr: com2: not present.");
        }
        if machine.flags & pbr_devices::machine::flags::PS2 != 0 {
            legacy(
                &drivers,
                &manifests,
                Legacy::Ps2,
                drivers::PS2,
                io,
                lines,
                &host_ep,
                &mut next_id,
                selftest,
            );
        } else {
            granite::println!("devmgr: ps2: no controller.");
        }
    }
    // One screen is served; more would need a way to pick between them.
    if let Some(screen) = machine.screens.first() {
        if let Some(i) = choose(&manifests, &Target::Legacy(Legacy::Framebuffer)) {
            let id = next_id;
            granite::println!(
                "devmgr: framebuffer ({}x{}): {}.",
                screen.width,
                screen.height,
                drivers[i].name
            );
            let plan =
                drivers::plan_framebuffer(screen, &memory, log_control.as_ref()).map_err(|e| e.0);
            add(&drivers[i], i, String::from("fb0"), plan, &host_ep, id, selftest);
        } else {
            granite::println!("devmgr: framebuffer: no driver.");
        }
    }

    // Serve drivers on one thread and clients on another; this one watches the drivers.
    let host_ep = Arc::new(host_ep);
    let for_host = host_ep.clone();
    thread::spawn(move || serve(&for_host, 0, &mut Host))
        .map_err(|e| alloc::format!("can't start a thread: {e}"))?;
    thread::spawn(move || serve(&public, devices::REQUEST_BUFFER, &mut Public { selftest }))
        .map_err(|e| alloc::format!("can't start a thread: {e}"))?;
    granite::println!("devmgr: ready.");
    supervise::run(&drivers, &host_ep, selftest)
}

/// Starts a driver for a device whose plan is `plan`, and lists the device.
fn add(
    d: &drivers::Driver<'_>,
    driver: usize,
    label: String,
    plan: Result<drivers::Plan, String>,
    host_ep: &Endpoint,
    id: u32,
    selftest: bool,
) {
    let mut history = History::new();
    history.started(supervise::now_ms());
    let started = plan.and_then(|plan| {
        drivers::start(d, &plan, host_ep, id, &label, selftest).map(|s| (s, plan)).map_err(|e| e.0)
    });
    let (state, process, plan) = match started {
        Ok((s, p)) => (DeviceState::Starting, Some(s), Some(p)),
        Err(why) => {
            granite::println!("devmgr: {label}: {why}");
            (DeviceState::Disabled, None, None)
        }
    };
    REGISTRY.lock().devices.push(Entry {
        id,
        name: d.name.clone(),
        class: d.manifest.class,
        state,
        restarts: 0,
        service: None,
        process,
        plan,
        driver,
        label,
        history,
        due: None,
        started_at: supervise::now_ms(),
    });
}

/// Finds a driver for a legacy device and starts it.
#[allow(clippy::too_many_arguments)]
fn legacy(
    drivers: &[drivers::Driver<'_>],
    manifests: &[Manifest],
    which: Legacy,
    spec: drivers::LegacySpec,
    io: &IoPorts,
    lines: &Interrupts,
    host_ep: &Endpoint,
    next_id: &mut u32,
    selftest: bool,
) {
    let label = match which {
        Legacy::Com2 => "com2",
        Legacy::Ps2 => "ps2",
        Legacy::Framebuffer => "framebuffer",
    };
    let Some(i) = choose(manifests, &Target::Legacy(which)) else {
        granite::println!("devmgr: {label}: no driver.");
        return;
    };
    granite::println!("devmgr: {label}: {}.", drivers[i].name);
    let id = *next_id;
    *next_id += 1;
    let plan = drivers::plan_legacy(spec, io, lines).map_err(|e| e.0);
    add(&drivers[i], i, String::from(label), plan, host_ep, id, selftest);
}

/// Whether the second serial port exists: a byte written with the chip in loopback mode comes
/// back from the receive register. (Nothing at the ports reads as 0xFF, so it can't.)
fn com2_present(io: &IoPorts) -> bool {
    let Ok(p) = io.slice(drivers::COM2.port, drivers::COM2.ports, rights::ALL) else {
        return false;
    };
    const MCR: u64 = 4;
    const LOOPBACK: u64 = 0x10;
    let ok = (|| -> granite::Result<bool> {
        p.write(MCR, 1, LOOPBACK | 0x0F)?;
        let mut ok = true;
        for byte in [0xAEu64, 0x51] {
            p.write(0, 1, byte)?;
            let mut got = None;
            for _ in 0..1000 {
                if p.read(5, 1)? & 1 != 0 {
                    got = Some(p.read(0, 1)?);
                    break;
                }
            }
            ok &= got == Some(byte);
        }
        p.write(MCR, 1, 0)?;
        Ok(ok)
    })();
    ok.unwrap_or(false)
}

/// Reports a failed answer (the caller may have gone).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("devmgr: answer failed: {e}");
    }
}

/// What clients call: `pbr.devices` (and, in self-test runs, `pbr.devices.debug`).
struct Public {
    selftest: bool,
}

impl Service for Public {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        // Both interfaces are version 1, so the debug one is told apart by its operation
        // numbers; `devices::dispatch` would answer them as unknown.
        let debug = msg.label >> 16 == u32::from(devices_debug::VERSION)
            && matches!(
                (msg.label & 0xFFFF) as u16,
                devices_debug::stop::OP | devices_debug::forgive::OP | devices_debug::quieted::OP
            );
        if self.selftest && debug {
            if let Some((_, reply)) = devices_debug::dispatch(self, msg, buffer, reply) {
                granite::interface::unknown(reply);
            }
        } else if let Some((_, reply)) = devices::dispatch(self, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }
    fn signal(&mut self, _bits: u64) {}
}

impl devices::Server for Public {
    fn list(&mut self, reply: devices::list::Reply) {
        let infos: Vec<DeviceInfo> = REGISTRY
            .lock()
            .devices
            .iter()
            .take(MAX_LISTED)
            .map(|e| DeviceInfo {
                id: e.id,
                kind: drivers::device_class(e.class),
                state: e.state,
                name: e.name.clone(),
                restarts: e.restarts,
            })
            .collect();
        answered(reply.ok(&infos));
    }

    fn open(&mut self, id: u32, reply: devices::open::Reply) {
        let result = {
            let g = REGISTRY.lock();
            match g.devices.iter().find(|e| e.id == id) {
                None => Err(DevicesError::NoSuchDevice),
                Some(e) if e.state == DeviceState::Disabled => Err(DevicesError::Disabled),
                // A driver that has ended but hasn't been noticed yet is not ready either.
                Some(e) if !e.process.as_ref().is_some_and(supervise::alive) => {
                    Err(DevicesError::NotReady)
                }
                Some(e) => match (&e.service, e.state) {
                    (Some(h), DeviceState::Running) => h
                        .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE)
                        .map_err(|_| DevicesError::NotReady),
                    _ => Err(DevicesError::NotReady),
                },
            }
        };
        answered(match result {
            Ok(h) => reply.ok(h),
            Err(e) => reply.err(e),
        });
    }
}

impl devices_debug::Server for Public {
    fn quieted(&mut self, reply: devices_debug::quieted::Reply) {
        let (functions, at) = *QUIETED.lock();
        answered(reply.ok(functions, at));
    }

    fn stop(&mut self, id: u32, reply: devices_debug::stop::Reply) {
        let ok = {
            let g = REGISTRY.lock();
            g.devices
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.process.as_ref())
                .is_some_and(|p| p.process.stop(9).is_ok())
        };
        answered(if ok { reply.ok() } else { reply.err(DebugError::NoSuchDevice) });
    }

    fn forgive(&mut self, id: u32, reply: devices_debug::forgive::Reply) {
        let ok = {
            let mut g = REGISTRY.lock();
            g.devices.iter_mut().find(|e| e.id == id).map(|e| e.history.forgive()).is_some()
        };
        answered(if ok { reply.ok() } else { reply.err(DebugError::NoSuchDevice) });
    }
}

/// What drivers call: `pbr.driver.host`. The caller is known by the badge on its endpoint.
struct Host;

impl Service for Host {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if let Some((_, reply)) = host::dispatch(self, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }
    fn signal(&mut self, _bits: u64) {}
}

impl host::Server for Host {
    fn ready(&mut self, service: Handle, reply: host::ready::Reply) {
        let id = reply.badge() as u32;
        // What arrives must be an endpoint that can be sent to.
        let sound = granite::interface::check_kind(&service, kind::ENDPOINT, true).is_ok();
        let result = {
            let mut g = REGISTRY.lock();
            match g.devices.iter_mut().find(|e| e.id == id) {
                Some(e)
                    if sound
                        && matches!(e.state, DeviceState::Starting | DeviceState::Restarting) =>
                {
                    e.service = Some(service);
                    e.state = DeviceState::Running;
                    granite::println!(
                        "devmgr: {} is running ({} ms after it started).",
                        e.name,
                        supervise::now_ms().saturating_sub(e.started_at)
                    );
                    Ok(())
                }
                _ => Err(HostError::BadState),
            }
        };
        answered(match result {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }
}
