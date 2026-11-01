// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio disk driver (docs/design/0011-device-manager-and-drivers.md, sections 7 and 9):
//! serves `pbr.block` for a virtio block device, over `pbr-virtio`.
//!
//! It starts the device from reset every time, takes one request at a time, and treats a device
//! that lies or stops answering as broken: it fails the request and ends, and the device
//! manager starts it again (note 0012).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::message::Incoming;
use granite::notification::Notification;
use granite::{process, time};
use pbr_devices::msix;
use pbr_devices::pci::{self, command};
use pbr_driver::{Device, Dma, Mmio, Service, host_ready, serve};
use pbr_interfaces::pbr_block::{self as block, BlockError, BlockInfo};
use pbr_virtio::blk::{self, Block, Done, Request};
use pbr_virtio::transport::find_layout;

granite::main!(main);

/// How long a request may take before the device is given up on.
const TIMEOUT_MS: u64 = 5_000;
/// How long to wait for an interrupt before looking at the queue anyway (an interrupt that
/// went missing must not hang the driver).
const POLL_MS: u64 = 20;

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("virtio-blk: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("virtio-blk: {why}");
            1
        }
    }
}

fn run(mut dev: Device) -> Result<core::convert::Infallible, &'static str> {
    let mut cfg = dev.config.take().ok_or("no configuration space")?;
    let host = dev.host.take().ok_or("no endpoint to the device manager")?;
    let dma = dev.dma.take().ok_or("no DMA memory")?;
    let bus = dma.bus();

    // The function: respond to memory accesses, be allowed to do DMA and write interrupt
    // messages, and not use the older interrupt lines.
    pci::update_command(&mut cfg, command::MEMORY | command::BUS_MASTER | command::INTX_DISABLE, 0);
    let (caps, sound) = pci::capabilities(&mut cfg);
    if !sound {
        return Err("the capability list is damaged");
    }
    let sizes = dev.record.bars;
    let layout =
        find_layout(&mut cfg, &caps, &sizes).map_err(|_| "the virtio structures are unusable")?;

    // Interrupts: the MSI-X table in its BAR gets the messages the kernel gave us.
    let vector = match msix::info(&mut cfg, &caps) {
        Some(info) if !dev.irqs.is_empty() => {
            let size = sizes.get(usize::from(info.bar)).copied().unwrap_or(0);
            msix::check(&info, size).map_err(|_| "the MSI-X table is unusable")?;
            let table = dev.bars[usize::from(info.bar)]
                .as_ref()
                .ok_or("the MSI-X table's BAR wasn't given")?;
            let messages: Vec<_> = dev.irqs.iter().map(|i| i.message).collect();
            msix::enable(&mut cfg, &info, table, &messages).map_err(|_| "can't enable MSI-X")?;
            Some(0)
        }
        _ => None, // no interrupts: the queue is polled
    };

    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    let note = dev.bind_interrupts(&service).map_err(|e| e.0)?;
    let bars = core::mem::replace(&mut dev.bars, [const { None }; 6]);
    let blk = Block::start(layout, bars, dma, bus, vector).map_err(|e| {
        granite::println!("virtio-blk: the device can't be started: {e:?}");
        "the device can't be started"
    })?;
    granite::println!(
        "virtio-blk: {} sectors, {}, {}",
        blk.sectors(),
        if blk.read_only() { "read-only" } else { "read-write" },
        if vector.is_some() { "interrupts" } else { "polled" },
    );
    let mut disk = Disk { blk, note, dev };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, block::REQUEST_BUFFER, &mut disk)
}

struct Disk {
    blk: Block<Mmio, Dma>,
    note: Notification,
    dev: Device,
}

/// What a finished request turned out to be.
enum Outcome {
    Done(Done),
    /// The request failed but the device is fine.
    Failed(BlockError),
    /// The device is broken: answer, then end so the device manager restarts the driver.
    Broken(&'static str),
}

fn map_error(e: blk::Error) -> Outcome {
    match e {
        blk::Error::OutOfRange => Outcome::Failed(BlockError::OutOfRange),
        blk::Error::ReadOnly => Outcome::Failed(BlockError::ReadOnly),
        blk::Error::BadArgument => Outcome::Failed(BlockError::BadArgument),
        blk::Error::Io => Outcome::Failed(BlockError::DeviceError),
        blk::Error::Busy | blk::Error::Idle => Outcome::Broken("a request was lost track of"),
        blk::Error::Queue(_) | blk::Error::BadCompletion => Outcome::Broken("the device lied"),
        _ => Outcome::Broken("the device failed"),
    }
}

impl Disk {
    /// Submits a request and waits for it, polling the queue as interrupts arrive.
    fn perform(&mut self, r: Request<'_>) -> Outcome {
        if let Err(e) = self.blk.submit(r) {
            return map_error(e);
        }
        let deadline = time::after_ms(TIMEOUT_MS);
        loop {
            match self.blk.poll() {
                Ok(Some(done)) => return Outcome::Done(done),
                Ok(None) => {}
                Err(e) => return map_error(e),
            }
            if time::now() >= deadline {
                return Outcome::Broken("the device didn't answer");
            }
            // Wake for the interrupt, or after a short while in case it never comes.
            let _ = self.note.wait(time::after_ms(POLL_MS));
            for i in 0..self.dev.irqs.len() {
                self.dev.acknowledge(i);
            }
        }
    }

    /// Splits an outcome into what to tell the caller and whether the device is broken.
    fn judge(outcome: Outcome) -> (Result<Done, BlockError>, Option<&'static str>) {
        match outcome {
            Outcome::Done(d) => (Ok(d), None),
            Outcome::Failed(e) => (Err(e), None),
            Outcome::Broken(why) => (Err(BlockError::DeviceError), Some(why)),
        }
    }

    /// If the device is broken, ends the driver (after the caller has been answered).
    fn end_if_broken(&mut self, broken: Option<&'static str>) {
        if let Some(why) = broken {
            granite::println!("virtio-blk: {why}; ending so that I am restarted");
            self.blk.stop();
            process::exit(3);
        }
    }
}

/// Reports a failed answer (the caller may have gone, which is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("virtio-blk: answer failed: {e}");
    }
}

impl block::Server for Disk {
    fn info(&mut self, reply: block::info::Reply) {
        let info = BlockInfo {
            sector_size: blk::SECTOR as u32,
            sector_count: self.blk.sectors(),
            read_only: self.blk.read_only(),
        };
        answered(reply.ok(&info));
    }

    fn read(&mut self, sector: u64, count: u32, reply: block::read::Reply) {
        let (result, broken) = Self::judge(self.perform(Request::Read { sector, count }));
        answered(match result {
            Ok(Done::Read { bytes }) => {
                let mut data = vec![0u8; bytes];
                self.blk.data(&mut data);
                reply.ok(&data)
            }
            Ok(_) => reply.fail(),
            Err(e) => reply.err(e),
        });
        self.end_if_broken(broken);
    }

    fn write(&mut self, sector: u64, data: Vec<u8>, reply: block::write::Reply) {
        let (result, broken) = Self::judge(self.perform(Request::Write { sector, data: &data }));
        answered(match result {
            Ok(Done::Written) => reply.ok(),
            Ok(_) => reply.fail(),
            Err(e) => reply.err(e),
        });
        self.end_if_broken(broken);
    }

    fn flush(&mut self, reply: block::flush::Reply) {
        if !self.blk.has_flush() {
            answered(reply.ok());
            return;
        }
        let (result, broken) = Self::judge(self.perform(Request::Flush));
        answered(match result {
            Ok(Done::Flushed) => reply.ok(),
            Ok(_) => reply.fail(),
            Err(e) => reply.err(e),
        });
        self.end_if_broken(broken);
    }
}

impl Service for Disk {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if let Some((_, reply)) = block::dispatch(self, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, _bits: u64) {
        // An interrupt with no request in flight: nothing to do but let it fire again.
        for i in 0..self.dev.irqs.len() {
            self.dev.acknowledge(i);
        }
    }
}
