// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drivers: the manifests in the start-up archive, and giving a driver exactly its device's
//! capabilities (note 0011, sections 5 and 6).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::Handle;
use granite::device::{DeviceMemory, Interrupts, IoPorts, bus_address};
use granite::endpoint::Endpoint;
use granite::memory::Memory;
use granite::process::{self, Spawn, Spawned};
use granite::sys::rights;
use pbr_archive::Archive;
use pbr_devices::machine::Screen;
use pbr_devices::manifest::{Class, Manifest};
use pbr_devices::pci::{Bar, BarKind, Function};
use pbr_devices::record::{Kind, MAX_BARS, MAX_BUFFERS, Message, Record, Screen as Screen2};
use pbr_driver::{ConfigPage, role};
use pbr_interfaces::pbr_supervisor as supervisor;

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

/// A PCI function's configuration space, in the two forms the device manager needs.
pub struct PciPart {
    /// The configuration space as the driver gets it: a copy kept apart from the one the device
    /// manager maps, so that revoking what the driver was given (which takes any copies it
    /// passed on) leaves the device manager's own mapping alone.
    pub config: DeviceMemory,
    /// The capability the mapping below hangs from.
    pub _own_config: DeviceMemory,
    /// The device manager's own mapping of it, made once: mapping and unmapping at every
    /// restart would leave page-table pages behind each time.
    pub page: ConfigPage,
}

/// Everything a driver is started with, made once for a device and kept across restarts (the
/// interrupts and DMA memory stay the same, so a restarted driver finds the device's world as
/// it was).
pub struct Plan {
    /// The device record.
    pub record: Memory,
    /// What sort of device it is.
    pub kind: Kind,
    /// The configuration space (PCI only).
    pub pci: Option<PciPart>,
    /// The BARs given, by index (for a framebuffer: its memory, as BAR 0).
    pub bars: Vec<(usize, DeviceMemory)>,
    /// The interrupts given (message-signalled for PCI, wired lines for legacy devices).
    pub irqs: Vec<Interrupts>,
    /// The DMA memory, if the manifest asks for any: the copy drivers are given theirs from,
    /// and the one that is revoked when a driver ends, to take back whatever it passed on.
    pub dma: Option<Memory>,
    /// The DMA memory's original capability. It is never revoked: the copy the supervisor keeps
    /// (note 0018, section 4) was made from it, and revoking destroys every copy made from a
    /// capability, so revoking this one would take the supervisor's with it.
    #[expect(dead_code, reason = "kept so that the memory lives; nothing needs to use it")]
    pub dma_root: Option<Memory>,
    /// A display driver's buffers (note 0020, section 4): more DMA memory, each its own memory
    /// object so that the driver can share one with its client. Unlike `dma` these are not
    /// revoked when the driver ends: its client has them mapped and may be drawing, and memory
    /// that vanished under it would end the client too. They only ever hold pictures.
    pub buffers: Vec<Memory>,
    /// I/O ports (legacy devices).
    pub ports: Option<IoPorts>,
    /// A second range of I/O ports, for a device whose ports aren't together.
    pub ports2: Option<IoPorts>,
    /// The kernel log with only the right `CONTROL`, for the firmware's framebuffer: whoever
    /// drives the screen must be able to stop the kernel drawing its text on it
    /// (`console_screen_release`, note 0010 section 7). Nothing else is given it.
    pub log_control: Option<Handle>,
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
/// space, allocates its interrupts and DMA memory, and writes its record. `page` is the device
/// manager's own mapping of the configuration space `config`. A copy of the DMA memory goes to
/// `keeper` (the supervisor) to keep, so that it outlives this device manager if it dies
/// (note 0018, section 4). `log_control` is the device manager's kernel log with `CONTROL`;
/// a display driver gets a copy with only that right.
#[allow(clippy::too_many_arguments)] // each is a different root capability or its mapping
pub fn plan(
    f: &Function,
    config: DeviceMemory,
    page: ConfigPage,
    m: &Manifest,
    memory: &DeviceMemory,
    pool: Option<&Interrupts>,
    keeper: Option<&supervisor::Client>,
    log_control: Option<&Handle>,
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

    // Makes a piece of DMA memory, has the supervisor keep a copy, and finds its bus address
    // (the address the device uses for it).
    let make = |size: u64| -> Result<(Memory, u64), PlanError> {
        let mem = config.create_dma(size, 0).map_err(err("making DMA memory"))?;
        let bus = bus_address(&mem, &config, 0).map_err(err("finding the DMA memory's address"))?;
        if let Some(k) = keeper {
            // Any rights will do to keep it alive; it has to be transferable to be sent.
            let copy = mem
                .0
                .duplicate(rights::INSPECT | rights::TRANSFER)
                .map_err(err("copying DMA memory"))?;
            k.keep(Memory(copy)).map_err(|e| PlanError(format!("having DMA memory kept: {e}")))?;
        }
        Ok((mem, bus))
    };
    let (dma, dma_root) = if m.dma != 0 {
        let (root, bus) = make(m.dma)?;
        (record.dma_bus, record.dma_size) = (bus, m.dma);
        let share = root.0.duplicate(rights::ALL).map_err(err("copying DMA memory"))?;
        (Some(Memory(share)), Some(root))
    } else {
        (None, None)
    };
    let mut buffers = Vec::new();
    let (count, each) = m.buffers;
    for i in 0..(count as usize).min(MAX_BUFFERS) {
        let (mem, bus) = make(each)?;
        record.buffers[i] = (bus, each);
        buffers.push(mem);
    }
    record.buffer_count = buffers.len() as u8;

    let own = config;
    let config =
        DeviceMemory(own.0.duplicate(rights::ALL).map_err(err("copying configuration space"))?);
    let part = PciPart { config, _own_config: own, page };
    let rec = write_record(&record)?;
    Ok(Plan {
        record: rec,
        kind: Kind::Pci,
        pci: Some(part),
        bars,
        irqs: msis,
        dma,
        dma_root,
        buffers,
        ports: None,
        ports2: None,
        log_control: if m.class == Class::Display { copy_log_control(log_control) } else { None },
    })
}

/// A copy of the kernel log with the right to take the screen from the kernel, for a display
/// driver's plan. Without it the driver still works; the kernel just keeps drawing its text
/// on the screen too.
fn copy_log_control(log_control: Option<&Handle>) -> Option<Handle> {
    log_control.and_then(|h| {
        // (`DUPLICATE` and `TRANSFER` too, so that `start` can give the driver a copy: the driver's
        // copy has only `CONTROL`.)
        h.duplicate(rights::CONTROL | rights::DUPLICATE | rights::TRANSFER)
            .map_err(|e| granite::println!("devmgr: copying the kernel log for the screen: {e}"))
            .ok()
    })
}

fn write_record(record: &Record) -> Result<Memory, PlanError> {
    let rec = Memory::create(4096).map_err(err("making the record"))?;
    rec.write(0, &record.encode()).map_err(err("writing the record"))?;
    Ok(rec)
}

/// The legacy devices and what each is given (note 0011, section 4): I/O ports, and wired
/// interrupt lines numbered as the ISA numbered them.
#[derive(Clone, Copy)]
pub struct LegacySpec {
    /// First port.
    pub port: u64,
    /// How many ports.
    pub ports: u64,
    /// A second range (first port, how many), for a device whose ports aren't together.
    pub more: Option<(u64, u64)>,
    /// The interrupt lines.
    pub irqs: &'static [u64],
}

/// The second serial port.
pub const COM2: LegacySpec = LegacySpec { port: 0x2F8, ports: 8, more: None, irqs: &[3] };
/// The 8042: the data port 0x60 and the command and status port 0x64 (one capability each:
/// the ports between are not all the controller's, and the kernel keeps 0x61), the keyboard on
/// line 1 and the mouse on line 12.
pub const PS2: LegacySpec =
    LegacySpec { port: 0x60, ports: 1, more: Some((0x64, 1)), irqs: &[1, 12] };

/// Makes the plan for a legacy device from the root capabilities.
pub fn plan_legacy(
    spec: LegacySpec,
    ports: &IoPorts,
    lines: &Interrupts,
) -> Result<Plan, PlanError> {
    let mut record = Record::new(Kind::Legacy);
    let slice = ports.slice(spec.port, spec.ports, rights::ALL).map_err(err("slicing ports"))?;
    let (_, _, first) = lines.info().map_err(err("reading the interrupt range"))?;
    let mut irqs = Vec::new();
    for (i, line) in spec.irqs.iter().enumerate() {
        let one = lines.slice(first + line, 1, rights::ALL).map_err(err("slicing a line"))?;
        record.interrupts[i] = Message { address: 0, data: (first + line) as u32 };
        irqs.push(one);
    }
    record.interrupt_count = irqs.len() as u8;
    let ports2 = match spec.more {
        Some((first, count)) => {
            Some(ports.slice(first, count, rights::ALL).map_err(err("slicing more ports"))?)
        }
        None => None,
    };
    Ok(Plan {
        record: write_record(&record)?,
        kind: Kind::Legacy,
        pci: None,
        bars: Vec::new(),
        irqs,
        dma: None,
        dma_root: None,
        buffers: Vec::new(),
        ports: Some(slice),
        ports2,
        log_control: None,
    })
}

/// Makes the plan for the firmware's framebuffer `s`. `log_control` is the device manager's
/// kernel log with `CONTROL`, if it was given one; the driver gets a copy with only that right.
pub fn plan_framebuffer(
    s: &Screen,
    memory: &DeviceMemory,
    log_control: Option<&Handle>,
) -> Result<Plan, PlanError> {
    let format =
        s.format().ok_or_else(|| PlanError("the screen's format is unsupported".into()))?;
    let start = s.base & !4095;
    let size = (s.base - start + s.len).next_multiple_of(4096);
    if s.base != start {
        return Err(PlanError("the framebuffer isn't page aligned".into()));
    }
    let mut record = Record::new(Kind::Framebuffer);
    record.bars[0] = size;
    record.screen = Screen2 { width: s.width, height: s.height, stride: s.stride, format };
    let slice = memory.slice(start, size, rights::ALL).map_err(err("slicing the framebuffer"))?;
    let log_control = copy_log_control(log_control);
    Ok(Plan {
        record: write_record(&record)?,
        kind: Kind::Framebuffer,
        pci: None,
        bars: alloc::vec![(0, slice)],
        irqs: Vec::new(),
        dma: None,
        dma_root: None,
        buffers: Vec::new(),
        ports: None,
        ports2: None,
        log_control,
    })
}

/// Starts the driver for `plan`. `host` is the endpoint drivers report to; the driver's copy is
/// badged with `id`.
pub fn start(
    d: &Driver<'_>,
    plan: &Plan,
    host: &Endpoint,
    id: u32,
    label: &str,
    selftest: bool,
) -> Result<Spawned, PlanError> {
    let name = format!("{}@{label}", d.name);
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
    if let Some(pci) = &plan.pci {
        grants.push((
            role::CONFIG,
            pci.config.0.raw(),
            rights::READ | rights::WRITE | rights::MAP,
            false,
        ));
    }
    // A display driver passes its memory on to clients, so it may copy it.
    let pass_on =
        if plan.kind == Kind::Framebuffer { rights::DUPLICATE | rights::TRANSFER } else { 0 };
    for (i, bar) in &plan.bars {
        grants.push((
            role::BAR + *i as u32,
            bar.0.raw(),
            rights::READ | rights::WRITE | rights::MAP | pass_on,
            false,
        ));
    }
    let irq_role = if plan.kind == Kind::Pci { role::MSI } else { role::LEGACY_IRQ };
    for (i, irq) in plan.irqs.iter().enumerate() {
        grants.push((irq_role + i as u32, irq.0.raw(), rights::BIND | rights::ACKNOWLEDGE, false));
    }
    if let Some(ports) = &plan.ports {
        grants.push((role::PORTS, ports.0.raw(), rights::READ | rights::WRITE, false));
    }
    if let Some(ports) = &plan.ports2 {
        grants.push((role::PORTS_2, ports.0.raw(), rights::READ | rights::WRITE, false));
    }
    if let Some(dma) = &plan.dma {
        grants.push((
            role::DMA,
            dma.0.raw(),
            rights::READ | rights::WRITE | rights::MAP | rights::INSPECT,
            false,
        ));
    }
    // A display driver shares each buffer with its client, so it may copy and pass them on.
    for (i, buffer) in plan.buffers.iter().enumerate() {
        grants.push((
            role::BUFFER + i as u32,
            buffer.0.raw(),
            rights::READ
                | rights::WRITE
                | rights::MAP
                | rights::INSPECT
                | rights::DUPLICATE
                | rights::TRANSFER
                | rights::REVOKE,
            false,
        ));
    }
    // Only a display driver may take the screen from the kernel.
    if let Some(log) = &plan.log_control
        && d.manifest.class == Class::Display
    {
        grants.push((role::LOG_CONTROL, log.raw(), rights::CONTROL, false));
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
        // Tied (note 0013): if the device manager ends, its drivers end first.
        tied: true,
        file_tree: &[],
    });
    spawned.map_err(|e| PlanError(format!("starting {name}: {e}")))
}
