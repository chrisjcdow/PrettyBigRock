// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading what a driver was started with (note 0011, section 6).

use crate::regions::{ConfigPage, Dma, Mmio};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use granite::Handle;
use granite::Start;
use granite::device::{DeviceMemory, Interrupts, IoPorts};
use granite::endpoint::Endpoint;
use granite::memory::Memory;
use granite::notification::Notification;
use granite::sys::perm;
use pbr_devices::record::{self, Kind, MAX_BARS, MAX_INTERRUPTS, Message, Record};

/// The roles the device manager gives a driver's capabilities (note 0011, section 6).
pub mod role {
    /// An endpoint to the device manager.
    pub const HOST: u32 = 5;
    /// The device record.
    pub const RECORD: u32 = 1000;
    /// The function's configuration space.
    pub const CONFIG: u32 = 1001;
    /// BAR 0; BAR *i* is this plus *i*.
    pub const BAR: u32 = 1010;
    /// Message-signalled interrupt 0; interrupt *i* is this plus *i*.
    pub const MSI: u32 = 1020;
    /// The DMA memory.
    pub const DMA: u32 = 1030;
    /// I/O ports.
    pub const PORTS: u32 = 1040;
    /// A second range of I/O ports, for a device whose ports aren't together.
    pub const PORTS_2: u32 = 1060;
    /// Legacy interrupt 0; interrupt *i* is this plus *i*.
    pub const LEGACY_IRQ: u32 = 1041;
    /// The kernel log with the right to take over the screen.
    pub const LOG_CONTROL: u32 = 1050;
}

/// Why a driver couldn't be set up from what it was given.
#[derive(Debug, PartialEq, Eq)]
pub struct StartError(pub &'static str);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A device interrupt the driver can bind: the capability and what to program into the device.
#[derive(Debug)]
pub struct Irq {
    /// The interrupt.
    pub interrupts: Interrupts,
    /// The address and data for the device (for a legacy interrupt: address 0, data the
    /// number).
    pub message: Message,
}

/// Everything a driver was given, mapped and checked.
#[derive(Debug)]
pub struct Device {
    /// What the device manager says the device is.
    pub record: Record,
    /// The function's configuration space (PCI only).
    pub config: Option<ConfigPage>,
    /// The BARs it was given, by index.
    pub bars: [Option<Mmio>; MAX_BARS],
    /// The BARs of a framebuffer, by index: kept as capabilities and not mapped, because the
    /// display driver passes the memory on and never draws itself.
    pub memory: [Option<DeviceMemory>; MAX_BARS],
    /// The interrupts, in order.
    pub irqs: Vec<Irq>,
    /// The DMA memory.
    pub dma: Option<Dma>,
    /// I/O ports (legacy devices).
    pub ports: Option<IoPorts>,
    /// A second range of I/O ports.
    pub ports2: Option<IoPorts>,
    /// An endpoint to the device manager.
    pub host: Option<Endpoint>,
    /// The kernel log with `CONTROL` (the display driver only).
    pub log_control: Option<Handle>,
    /// The arguments from the manifest.
    pub args: String,
}

fn bad<T>(what: &'static str) -> Result<T, StartError> {
    Err(StartError(what))
}

impl Device {
    /// Reads the start-up block. Takes charge of the capabilities it names.
    pub fn from_start(start: &mut Start) -> Result<Device, StartError> {
        let Some(h) = start.take(role::RECORD) else { return bad("no device record") };
        let mut bytes = [0u8; record::SIZE];
        if Memory(h).read(0, &mut bytes).is_err() {
            return bad("the device record can't be read");
        }
        let record =
            Record::decode(&bytes).map_err(|_| StartError("the device record is damaged"))?;

        let config = match start.take(role::CONFIG) {
            Some(h) => {
                let m = DeviceMemory(h).map(0, 4096, perm::READ_WRITE);
                Some(ConfigPage(Mmio::new(
                    m.map_err(|_| StartError("can't map configuration space"))?,
                )))
            }
            None if record.kind == Kind::Pci => return bad("no configuration space"),
            None => None,
        };

        let mut bars: [Option<Mmio>; MAX_BARS] = [const { None }; MAX_BARS];
        let mut memory: [Option<DeviceMemory>; MAX_BARS] = [const { None }; MAX_BARS];
        for (i, slot) in bars.iter_mut().enumerate() {
            let size = record.bars[i];
            if size == 0 {
                continue;
            }
            let Some(h) = start.take(role::BAR + i as u32) else { return bad("a BAR is missing") };
            if size < 4096 || !size.is_multiple_of(4096) {
                return bad("a BAR has a size that can't be mapped");
            }
            if record.kind == Kind::Framebuffer {
                memory[i] = Some(DeviceMemory(h));
                continue;
            }
            let m = DeviceMemory(h)
                .map(0, size, perm::READ_WRITE)
                .map_err(|_| StartError("can't map a BAR"))?;
            *slot = Some(Mmio::new(m));
        }

        let count = usize::from(record.interrupt_count);
        if count > MAX_INTERRUPTS {
            return bad("too many interrupts");
        }
        let mut irqs = Vec::new();
        for i in 0..count {
            let r = if record.kind == Kind::Pci { role::MSI } else { role::LEGACY_IRQ } + i as u32;
            let Some(h) = start.take(r) else { return bad("an interrupt is missing") };
            irqs.push(Irq { interrupts: Interrupts(h), message: record.interrupts[i] });
        }

        let dma = if record.dma_size != 0 {
            let Some(h) = start.take(role::DMA) else { return bad("the DMA memory is missing") };
            if record.dma_size > (4 << 20)
                || !record.dma_size.is_multiple_of(4096)
                || !record.dma_bus.is_multiple_of(4096)
            {
                return bad("the DMA memory has an impossible size or address");
            }
            let mem = Memory(h);
            match mem.size() {
                Ok(s) if s >= record.dma_size => {}
                _ => return bad("the DMA memory is smaller than the record says"),
            }
            let m =
                mem.map(perm::READ_WRITE).map_err(|_| StartError("can't map the DMA memory"))?;
            Some(Dma::new(m, record.dma_bus))
        } else {
            None
        };

        Ok(Device {
            record,
            config,
            bars,
            memory,
            irqs,
            dma,
            ports: start.take(role::PORTS).map(IoPorts),
            ports2: start.take(role::PORTS_2).map(IoPorts),
            host: start.take(role::HOST).map(Endpoint),
            log_control: start.take(role::LOG_CONTROL),
            args: String::from(start.args()),
        })
    }

    /// Makes a notification, ties it to `endpoint` (so receiving there also returns interrupts)
    /// and binds each interrupt to it: interrupt *i* sets bit *i*.
    pub fn bind_interrupts(&self, endpoint: &Endpoint) -> Result<Notification, StartError> {
        let n = Notification::create().map_err(|_| StartError("can't make a notification"))?;
        endpoint.bind(&n).map_err(|_| StartError("can't tie the notification to the endpoint"))?;
        for (i, irq) in self.irqs.iter().enumerate() {
            irq.interrupts
                .bind(0, &n, 1 << i)
                .map_err(|_| StartError("can't bind an interrupt"))?;
        }
        Ok(n)
    }

    /// Says interrupt *i* has been handled, so it may fire again.
    pub fn acknowledge(&self, i: usize) {
        if let Some(irq) = self.irqs.get(i) {
            let _ = irq.interrupts.acknowledge(0);
        }
    }
}
