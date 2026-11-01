// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drivers: the manifests in the start-up archive, and giving a driver exactly its device's
//! capabilities (note 0011, sections 5 and 6).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::device::{DeviceMemory, Interrupts, bus_address};
use granite::endpoint::Endpoint;
use granite::memory::Memory;
use granite::process::{self, Spawn, Spawned};
use granite::sys::perm;
use granite::sys::rights;
use pbr_archive::Archive;
use pbr_devices::manifest::{Class, Manifest};
use pbr_devices::pci::{Address, Bar, BarKind, Function};
use pbr_devices::record::{Kind, MAX_BARS, Message, Record};
use pbr_driver::{ConfigPage, Mmio, role};

/// The BARs larger than this are not given to drivers (graphics apertures and the like are
/// for drivers written for them).
const MAX_BAR: u64 = 256 << 20;
/// The priority drivers run at: above ordinary programs, below interrupt delivery.
pub const PRIORITY: u8 = 2;

/// A driver found in the start-up archive.
pub struct Driver<'a> {
    /// Its name: the part of the file name after `drivers/`.
    pub name: String,
    /// Its manifest.
    pub manifest: Manifest,
    /// Its program.
    pub program: &'a [u8],
}

/// Reads every driver manifest in the archive. Manifests that are wrong, and drivers without a
/// program, are reported and skipped.
pub fn load<'a>(archive: &Archive<'a>) -> Vec<Driver<'a>> {
    let mut out = Vec::new();
    for e in archive.entries() {
        let Some(name) = e.name.strip_prefix("drivers/").and_then(|n| n.strip_suffix(".drv"))
        else {
            continue;
        };
        let Some(program) = archive.find(&format!("drivers/{name}")) else {
            granite::println!("devmgr: {} has no program; skipped.", e.name);
            continue;
        };
        match Manifest::parse(e.data) {
            Ok(manifest) => {
                out.push(Driver { name: String::from(name), manifest, program: program.data })
            }
            Err(err) => granite::println!("devmgr: {} is wrong ({err:?}); skipped.", e.name),
        }
    }
    out
}

/// A device's class as `pbr.devices` names it.
pub fn device_class(c: Class) -> pbr_interfaces::pbr_devices::DeviceClass {
    use pbr_interfaces::pbr_devices::DeviceClass;
    match c {
        Class::Block => DeviceClass::Block,
        Class::Input => DeviceClass::Input,
        Class::Serial => DeviceClass::Serial,
        Class::Display => DeviceClass::Display,
    }
}

/// Everything a driver is started with, made once for a device and kept across restarts (the
/// message-signalled interrupts and DMA memory stay the same, so a restarted driver finds
/// the device's world as it was).
pub struct Plan {
    /// The device record.
    pub record: Memory,
    /// The function's configuration space, as the driver gets it: a copy kept apart from the one
    /// the device manager maps, so that revoking what the driver was given (which takes any
    /// copies it passed on) leaves the device manager's own mapping alone.
    pub config: DeviceMemory,
    /// The capability the mapping below hangs from.
    pub _own_config: DeviceMemory,
    /// The device manager's own mapping of it, made once: mapping and unmapping at every
    /// restart would leave page-table pages behind each time (the kernel keeps a process's
    /// page tables until the process ends).
    pub config_page: ConfigPage,
    /// The BARs given, by index.
    pub bars: Vec<(usize, DeviceMemory)>,
    /// The interrupts given.
    pub msis: Vec<Interrupts>,
    /// The DMA memory, if the manifest asks for any.
    pub dma: Option<Memory>,
}

/// Why a plan couldn't be made.
#[derive(Debug)]
pub struct PlanError(pub String);

fn err<E: core::fmt::Display>(what: &str) -> impl FnOnce(E) -> PlanError + '_ {
    move |e| PlanError(format!("{what}: {e}"))
}

/// The BARs of `f` a driver can be given: memory, whole aligned pages, not enormous.
fn givable(f: &Function) -> impl Iterator<Item = &Bar> {
    f.bars.iter().filter(|b| b.kind != BarKind::Io && b.mappable() && b.size <= MAX_BAR)
}

/// Gets everything for a driver for PCI function `f`: slices its BARs and configuration
/// space, allocates its interrupts and DMA memory, and writes its record.
pub fn plan(
    f: &Function,
    config: DeviceMemory,
    m: &Manifest,
    memory: &DeviceMemory,
    pool: Option<&Interrupts>,
) -> Result<Plan, PlanError> {
    let mut record = Record::new(Kind::Pci);
    record.vendor = f.vendor;
    record.device = f.device;
    (record.class, record.subclass, record.prog_if, record.revision) =
        (f.class, f.subclass, f.prog_if, f.revision);

    let mut bars = Vec::new();
    for b in givable(f) {
        let idx = usize::from(b.index);
        if idx >= MAX_BARS {
            continue;
        }
        // Every right, so that the capability can be copied to the driver (now, and again when
        // the driver is restarted); the driver is given only READ, WRITE and MAP.
        let slice = memory.slice(b.base, b.size, rights::ALL).map_err(err("slicing a BAR"))?;
        record.bars[idx] = b.size;
        bars.push((idx, slice));
    }

    let mut msis = Vec::new();
    for i in 0..m.irqs as usize {
        let pool = pool
            .ok_or_else(|| PlanError("the driver wants interrupts and there is no pool".into()))?;
        let msi = pool.msi_allocate().map_err(err("allocating an interrupt"))?;
        let (address, data) = msi.msi_info().map_err(err("reading an interrupt"))?;
        record.interrupts[i] = Message { address, data: data as u32 };
        msis.push(msi);
    }
    record.interrupt_count = msis.len() as u8;

    let dma = if m.dma != 0 {
        let mem = config.create_dma(m.dma, 0).map_err(err("making DMA memory"))?;
        record.dma_bus =
            bus_address(&mem, &config, 0).map_err(err("finding the DMA memory's address"))?;
        record.dma_size = m.dma;
        Some(mem)
    } else {
        None
    };

    let page = config.map(0, 4096, perm::READ_WRITE).map_err(err("mapping configuration space"))?;
    let own = config;
    let config =
        DeviceMemory(own.0.duplicate(rights::ALL).map_err(err("copying configuration space"))?);
    let config_page = ConfigPage(Mmio::new(page));
    let rec = Memory::create(4096).map_err(err("making the record"))?;
    rec.write(0, &record.encode()).map_err(err("writing the record"))?;
    Ok(Plan { record: rec, config, _own_config: own, config_page, bars, msis, dma })
}

/// Starts the driver for `plan`. `host` is the endpoint drivers report to; the driver's copy is
/// badged with `id`.
pub fn start(
    d: &Driver<'_>,
    plan: &Plan,
    host: &Endpoint,
    id: u32,
    function: Address,
    selftest: bool,
) -> Result<Spawned, PlanError> {
    let name =
        format!("{}@{:02x}:{:02x}.{}", d.name, function.bus, function.device, function.function);
    let badged = host
        .mint(u64::from(id), rights::SEND | rights::TRANSFER)
        .map_err(err("minting the host endpoint"))?;
    let mut grants: Vec<(u32, u32, u32, bool)> = Vec::new();
    grants.push((role::HOST, badged.0.raw(), rights::SEND, true));
    grants.push((
        role::RECORD,
        plan.record.0.raw(),
        rights::READ | rights::MAP | rights::INSPECT,
        false,
    ));
    grants.push((
        role::CONFIG,
        plan.config.0.raw(),
        rights::READ | rights::WRITE | rights::MAP,
        false,
    ));
    for (i, bar) in &plan.bars {
        grants.push((
            role::BAR + *i as u32,
            bar.0.raw(),
            rights::READ | rights::WRITE | rights::MAP,
            false,
        ));
    }
    for (i, msi) in plan.msis.iter().enumerate() {
        grants.push((role::MSI + i as u32, msi.0.raw(), rights::BIND | rights::ACKNOWLEDGE, false));
    }
    if let Some(dma) = &plan.dma {
        grants.push((
            role::DMA,
            dma.0.raw(),
            rights::READ | rights::WRITE | rights::MAP | rights::INSPECT,
            false,
        ));
    }
    // In self-test runs the driver also serves `pbr.driver.debug`.
    let args =
        if selftest { format!("{} selftest", d.manifest.args) } else { d.manifest.args.clone() };
    let spawned = process::spawn(&Spawn {
        name: &name,
        program: d.program,
        budget: d.manifest.budget,
        priority: PRIORITY,
        caps: &grants,
        log: true,
        args: &args,
    });
    spawned.map_err(|e| PlanError(format!("starting {name}: {e}")))
}
