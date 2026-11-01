// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The device manager (docs/design/0011-device-manager-and-drivers.md): finds the devices,
//! starts a driver for each, keeps the list of what is running, and hands clients the
//! endpoint of the driver for a device.
//!
//! It is started by the root program, which gives it the start-up archive and the device access
//! capabilities (note 0010, section 8). Its own endpoint (role 1000) is where clients call
//! `pbr.devices`; drivers report to a second endpoint it makes, with `pbr.driver.host`.

#![no_std]
#![no_main]

extern crate alloc;

mod drivers;
mod ecam;
mod phys;

use alloc::string::String;
use alloc::vec::Vec;
use granite::Handle;
use granite::device::{DeviceMemory, Interrupts};
use granite::endpoint::{Endpoint, ReplyCap};
use granite::lock::Lock;
use granite::memory::Memory;
use granite::message::Incoming;
use granite::process::{Spawned, role};
use granite::sys::{kind, perm, rights};
use granite::thread;
use pbr_archive::Archive;
use pbr_devices::machine::Machine;
use pbr_devices::manifest::{Class, Manifest, Target, choose};
use pbr_devices::pci::{self, Function};
use pbr_driver::{Service, serve};
use pbr_interfaces::pbr_devices::{self as devices, DeviceInfo, DeviceState, DevicesError};
use pbr_interfaces::pbr_driver_host::{self as host, HostError};
use pbr_platform::acpi;

granite::main!(main);

/// The role the root program gives the device manager's public endpoint.
const PUBLIC: u32 = role::OWN;
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
    /// The driver's process (used by restart, Stage 3).
    #[allow(dead_code)]
    process: Option<Spawned>,
    /// What the driver was started with, kept for as long as the device exists: it keeps the
    /// interrupts and DMA memory alive, and a restarted driver is given the same ones.
    #[allow(dead_code)]
    plan: Option<drivers::Plan>,
}

struct Registry {
    devices: Vec<Entry>,
}

static REGISTRY: Lock<Registry> = Lock::new(Registry { devices: Vec::new() });

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
    let archive_mem = Memory(take(start, role::ARCHIVE, "the start-up archive")?);
    let memory = DeviceMemory(take(start, role::DEVICE_MEMORY, "device memory")?);
    let machine_mem = Memory(take(start, role::MACHINE, "the machine description")?);
    let pool = start.take(role::MSI_POOL).map(Interrupts);
    let public = Endpoint(take(start, PUBLIC, "an endpoint to serve")?);

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
    let machine = Machine::decode(&page, 0)
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

    // PCI Express.
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
        for f in &scan.functions {
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
            let d = &drivers[i];
            granite::println!("devmgr: {label}: {}.", d.name);
            let Some(config) = ecam.function(a) else {
                granite::println!("devmgr: {label}: can't get its configuration space.");
                continue;
            };
            let id = next_id;
            next_id += 1;
            let started = start_pci(d, f, config, &memory, pool.as_ref(), &host_ep, id);
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
            });
        }
    }

    // Serve drivers on one thread and clients on this one.
    thread::spawn(move || serve(&host_ep, 0, &mut Host))
        .map_err(|e| alloc::format!("can't start a thread: {e}"))?;
    granite::println!("devmgr: ready.");
    serve(&public, devices::REQUEST_BUFFER, &mut Public)
}

fn start_pci(
    d: &drivers::Driver<'_>,
    f: &Function,
    config: DeviceMemory,
    memory: &DeviceMemory,
    pool: Option<&Interrupts>,
    host_ep: &Endpoint,
    id: u32,
) -> Result<(Spawned, drivers::Plan), String> {
    let plan = drivers::plan(f, config, &d.manifest, memory, pool).map_err(|e| e.0)?;
    let spawned = drivers::start(d, &plan, host_ep, id, f.addr).map_err(|e| e.0)?;
    Ok((spawned, plan))
}

/// Reports a failed answer (the caller may have gone).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("devmgr: answer failed: {e}");
    }
}

/// What clients call: `pbr.devices`.
struct Public;

impl Service for Public {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if let Some((_, reply)) = devices::dispatch(self, msg, buffer, reply) {
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
                Some(e) => match (&e.service, e.state) {
                    (Some(h), DeviceState::Running) => h
                        .duplicate(rights::SEND | rights::TRANSFER)
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
                Some(e) if sound && e.state == DeviceState::Starting => {
                    e.service = Some(service);
                    e.state = DeviceState::Running;
                    granite::println!("devmgr: {} is running.", e.name);
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
