// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The ACPI service (docs/design/0028-acpi.md): it runs the firmware's AML with our own
//! interpreter (`lib/aml`) and answers what only AML can say: which legacy devices the machine
//! has and where, how PCI interrupt pins are wired, and how to turn the machine off. It also
//! watches the power button.
//!
//! **ACPI** is how a PC's firmware describes the machine; its DSDT and SSDTs are programs in a
//! bytecode, AML, which this service loads and runs. The bytecode is the firmware's, trusted in
//! what it does to the machine, but to the interpreter it is untrusted input (note 0028, the
//! problem). So the interpreter is in a service of its own, away from the device manager's DMA
//! capabilities, and can be restarted on its own.
//!
//! The root program starts it, before the device manager (note 0028, section 7), with:
//! - the machine description, which says where the ACPI tables are;
//! - a device memory capability for every physical address, **without** the right to make DMA
//!   memory (the kernel still refuses ordinary memory and its own devices);
//! - on x86-64, every I/O port, and the SCI (the interrupt power-management events raise).
//!
//! It serves `pbr.acpi` on its endpoint: `devices` and `route` to the device manager (badge
//! [`DEVICES`]), `off` and `watch_button` to the power service (badge [`POWER`]).

#![no_std]
#![no_main]

extern crate alloc;

mod events;
mod host;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::device::{DeviceMemory, Interrupts, IoPorts};
use granite::endpoint::{Endpoint, Event};
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::role;
use granite::time;
use pbr_aml::Aml;
use pbr_aml::resource::Resource;
use pbr_interfaces::pbr_acpi::{self as acpi_if, AcpiError};
use pbr_platform::acpi;

granite::main!(main);
// The interpreter recurses as it reads the bytecode; at its nesting limit it needs about
// 1.2 MiB (note 0028, Changes). Four, to be safe.
granite::stack_size!(4 << 20);

/// The badge of the device manager's capability (`services/root`'s `ACPI_DEVICES`).
const DEVICES: u64 = 1;
/// The badge of the power service's (`services/root`'s `ACPI_POWER`).
const POWER: u64 = 2;
/// Where the root program gives the SCI, a one-line slice of the interrupts (x86-64).
const SCI: u32 = 1001;
/// The bit the SCI signals on the service's notification.
const SCI_BIT: u64 = 1;

/// The most devices kept: more than any machine's namespace lists.
const MAX_DEVICES: usize = 256;

struct Service {
    aml: Aml<host::Machine>,
    /// The devices, found once at start.
    devices: Vec<acpi_if::Device>,
    /// The fixed hardware (x86-64), if the FADT describes it.
    fixed: Option<acpi::FadtFixed>,
    /// Who is told when the power button is pressed.
    button: Option<(Notification, u64)>,
}

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("acpi: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let description = Memory(start.take(role::MACHINE).ok_or("no machine description")?);
    let memory = DeviceMemory(start.take(role::DEVICE_MEMORY).ok_or("no device memory")?);
    let ports = start.take(role::IO_PORTS).map(IoPorts);
    let sci = start.take(SCI).map(Interrupts);
    let verbose = start.args().split_whitespace().any(|w| w == "verbose");

    // 1. The tables: where they are, from the machine description the loader made.
    let size = description.size().map_err(|e| format!("{e}"))?.min(4096) as usize;
    let mut page = vec![0u8; size];
    description.read(0, &mut page).map_err(|e| format!("reading the description: {e}"))?;
    let machine =
        pbr_devices::machine::Machine::decode(&page, pbr_devices::machine::FRAMEBUFFER_RECORD)
            .map_err(|e| format!("the machine description: {e:?}"))?;
    if machine.rsdp == 0 {
        return Err("there are no ACPI tables".into());
    }
    let mut host = host::Machine::new(memory, ports);
    host.verbose = verbose;
    let rsdp = machine.rsdp;
    let fadt = acpi::find_table(&host, rsdp, b"FACP").ok_or("no FADT")?;
    let fixed = acpi::fadt_fixed(&host, fadt).filter(|f| !f.hardware_reduced());
    let mut regions = Vec::new();
    if let Some(t) = acpi::find_table(&host, rsdp, b"MCFG") {
        acpi::mcfg(&host, t, |r| regions.push(r));
    }
    host.set_ecam(regions);
    if let Some(f) = &fixed {
        host.set_global_lock(f.facs, (f.pm1a_control, f.pm1b_control));
    }
    let mut addresses: Vec<u64> = Vec::new();
    addresses.extend(acpi::fadt_dsdt(&host, fadt));
    acpi::each_table(&host, rsdp, b"SSDT", |t| addresses.push(t));
    let mut tables: Vec<Vec<u8>> = Vec::new();
    for t in addresses {
        // Nothing the firmware wrote is trusted: no table is read past 4 MiB.
        let len = acpi::table_length(&host, t).unwrap_or(0).min(4 << 20) as usize;
        let mut bytes = vec![0u8; len];
        if len < 36 || !pbr_platform::PhysMemory::read(&host, t, &mut bytes) {
            granite::println!("acpi: can't read the table at {t:#x}.");
            continue;
        }
        tables.push(bytes);
    }

    // 2. ACPI mode, so the power-management registers are ours and the button is an event.
    if let Some(f) = &fixed {
        events::enable_acpi_mode(&host, f);
    }

    // 3. The namespace, and `\_PIC(1)`: the machine's interrupts go through I/O APICs.
    let mut aml = Aml::new(host);
    aml.verbose = verbose;
    let refs: Vec<&[u8]> = tables.iter().map(|t| t.as_slice()).collect();
    let load = aml.load(&refs);
    for s in &load.skipped {
        granite::println!("acpi: skipped {s}.");
    }
    if aml.find(None, "\\_PIC").is_some()
        && let Err(e) = aml.evaluate("\\_PIC", &[pbr_aml::Value::Integer(1)])
    {
        granite::println!("acpi: \\_PIC(1) failed: {e}");
    }

    // 4. The devices: `_INI`, `_REG` and `_STA`, then the list the device manager asks for.
    let init = aml.initialise();
    for f in &init.failed {
        granite::println!("acpi: {f}");
    }
    let devices: Vec<acpi_if::Device> =
        aml.devices().iter().take(MAX_DEVICES).map(describe).collect();
    if verbose {
        for d in &devices {
            granite::println!(
                "acpi: {} {} ports {:?} lines {:?}",
                d.path,
                d.hid,
                d.ports.iter().map(|p| (p.first, p.count)).collect::<Vec<_>>(),
                d.lines.iter().map(|l| l.number).collect::<Vec<_>>()
            );
        }
    }

    // 5. The events M7 doesn't handle off; the power button's on, and the SCI bound.
    let note = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&note).map_err(|e| format!("binding the endpoint: {e}"))?;
    let mut button = "no power button";
    if let Some(f) = &fixed {
        events::quiet(aml.host(), f);
        if f.fixed_power_button() {
            match &sci {
                Some(line) => match line.bind(0, &note, SCI_BIT) {
                    Ok(()) => {
                        events::enable_power_button(aml.host(), f);
                        button = "the power button watched";
                    }
                    Err(e) => granite::println!("acpi: can't bind the SCI: {e}"),
                },
                None => granite::println!("acpi: started without the SCI."),
            }
        } else {
            button = "a control-method power button (not handled in M7)";
        }
    }

    // 6. The summary.
    granite::println!(
        "acpi: {} table{}, {} objects, {} of {} devices present ({} with IDs), {button}.",
        load.tables,
        if load.tables == 1 { "" } else { "s" },
        aml.objects(),
        init.present,
        init.devices,
        devices.len(),
    );

    let mut s = Service { aml, devices, fixed, button: None };
    // `pbr.acpi`'s requests carry no buffer (`REQUEST_BUFFER` is 0); a little room anyway.
    let mut buffer = vec![0u8; 64];
    loop {
        match endpoint.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(msg, reply)) => {
                if let Some((_, reply)) = acpi_if::dispatch(&mut s, msg, &buffer, reply) {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(bits)) => {
                if bits & SCI_BIT != 0 {
                    s.sci();
                    if let Some(line) = &sci {
                        let _ = line.acknowledge(0);
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                granite::println!("acpi: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
    }
}

/// A device as `pbr.acpi` describes it: names cut to fit, resources by kind, at most as many
/// of each as the interface carries.
fn describe(d: &pbr_aml::DeviceInfo) -> acpi_if::Device {
    let cut = |s: &str, n: usize| -> String { s.chars().take(n).collect() };
    let mut out = acpi_if::Device {
        path: cut(&d.path, 128),
        hid: cut(&d.hid, 16),
        compatible: d.compatible.iter().take(4).map(|c| cut(c, 16)).collect(),
        uid: cut(d.uid.as_deref().unwrap_or(""), 16),
        ports: Vec::new(),
        memory: Vec::new(),
        lines: Vec::new(),
    };
    for r in &d.resources {
        match *r {
            Resource::Io { first, count } if out.ports.len() < 8 => {
                out.ports.push(acpi_if::Ports { first, count })
            }
            Resource::Memory { base, length } if out.memory.len() < 8 => {
                out.memory.push(acpi_if::Range { base, length })
            }
            Resource::Interrupt(l) if out.lines.len() < 4 => out.lines.push(acpi_if::Line {
                number: l.number,
                level: l.level,
                active_low: l.active_low,
            }),
            _ => {}
        }
    }
    out
}

impl Service {
    /// The SCI fired: if it was the power button, whoever asked is told.
    fn sci(&mut self) {
        let Some(f) = &self.fixed else { return };
        if events::power_button_pressed(self.aml.host(), f) {
            granite::println!("acpi: the power button was pressed.");
            if let Some((n, bits)) = &self.button {
                let _ = n.signal(*bits);
            }
        }
    }
}

/// Answers, logging a failure other than the caller having gone.
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("acpi: answer failed: {e}");
    }
}

impl acpi_if::Server for Service {
    fn devices(&mut self, first: u32, reply: acpi_if::devices::Reply) {
        if reply.badge() != DEVICES {
            return answered(reply.err(AcpiError::NotAllowed));
        }
        let first = first as usize;
        let page: Vec<acpi_if::Device> =
            self.devices.iter().skip(first).take(16).cloned().collect();
        let next = first + page.len();
        let next = (next < self.devices.len()).then_some(next as u32);
        answered(reply.ok(&page, next));
    }

    fn route(&mut self, segment: u16, bus: u8, device: u8, pin: u8, reply: acpi_if::route::Reply) {
        if reply.badge() != DEVICES {
            return answered(reply.err(AcpiError::NotAllowed));
        }
        match self.aml.route(segment, bus, device, pin) {
            Ok(l) => answered(reply.ok(&acpi_if::Line {
                number: l.number,
                level: l.level,
                active_low: l.active_low,
            })),
            Err(pbr_aml::Route::NotFound) => answered(reply.err(AcpiError::NotFound)),
            Err(pbr_aml::Route::Failed(why)) => {
                granite::println!("acpi: routing {bus:02x}:{device:02x} pin {pin}: {why}");
                answered(reply.err(AcpiError::Failed))
            }
        }
    }

    fn off(&mut self, reply: acpi_if::off::Reply) {
        if reply.badge() != POWER {
            return answered(reply.err(AcpiError::NotAllowed));
        }
        let Some(f) = self.fixed else { return answered(reply.err(AcpiError::NotSupported)) };
        let error = events::off(&mut self.aml, &f);
        answered(reply.err(error));
    }

    fn watch_button(&mut self, note: Notification, bits: u64, reply: acpi_if::watch_button::Reply) {
        if reply.badge() != POWER {
            return answered(reply.err(AcpiError::NotAllowed));
        }
        if !self.fixed.is_some_and(|f| f.fixed_power_button()) {
            return answered(reply.err(AcpiError::NotSupported));
        }
        self.button = Some((note, bits));
        answered(reply.ok());
    }
}
