// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio input driver (docs/design/0011-device-manager-and-drivers.md, sections 7 and 9):
//! serves `pbr.input` for a virtio keyboard, mouse or tablet, over `pbr-virtio`.
//!
//! It starts the device from reset every time. A device that lies makes the driver end, and the
//! device manager starts it again (note 0012).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::message::Incoming;
use granite::process;
use pbr_devices::keys;
use pbr_devices::msix;
use pbr_devices::pci::{self, command};
use pbr_driver::input::Input;
use pbr_driver::{Device, Dma, Mmio, Service, host_ready, serve};
use pbr_interfaces::pbr_input::{self as input, InputKind};
use pbr_virtio::input::{self as vinput, Input as Hardware, Kind, Tablet};
use pbr_virtio::transport::find_layout;

granite::main!(main);

/// The most events taken from the device in one go.
const BATCH: usize = 1024;
/// Linux event types and codes.
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
/// "That is the whole report": what the device has sent so far happened together.
const SYN_REPORT: u16 = 0;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("virtio-input: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("virtio-input: {why}");
            3
        }
    }
}

fn run(mut dev: Device) -> Result<core::convert::Infallible, &'static str> {
    let mut cfg = dev.config.take().ok_or("no configuration space")?;
    let host = dev.host.take().ok_or("no endpoint to the device manager")?;
    let dma = dev.dma.take().ok_or("no DMA memory")?;
    let bus = dma.bus();

    // Respond to memory accesses, be allowed to do DMA and write interrupt messages, and not
    // use the older interrupt lines.
    pci::update_command(&mut cfg, command::MEMORY | command::BUS_MASTER | command::INTX_DISABLE, 0);
    let (caps, sound) = pci::capabilities(&mut cfg);
    if !sound {
        return Err("the capability list is damaged");
    }
    let sizes = dev.record.bars;
    let layout =
        find_layout(&mut cfg, &caps, &sizes).map_err(|_| "the virtio structures are unusable")?;

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
        _ => return Err("no interrupts to use"),
    };

    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    let _note = dev.bind_interrupts(&service).map_err(|e| e.0)?;
    let bars = core::mem::replace(&mut dev.bars, [const { None }; 6]);
    let hw = Hardware::start(layout, bars, dma, bus, vector).map_err(|e| {
        granite::println!("virtio-input: the device can't be started: {e:?}");
        "the device can't be started"
    })?;
    let kind = match hw.kind() {
        Kind::Keyboard => InputKind::Keyboard,
        Kind::Mouse => InputKind::Mouse,
        Kind::Tablet => InputKind::Tablet,
    };
    let tablet = hw.abs_ranges().map(Tablet::new);
    granite::println!("virtio-input: {:?} \"{}\"", hw.kind(), hw.name());
    let input = Input::new(kind, hw.name())?;
    let mut s = Hid { hw, input, tablet, dev };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, 0, &mut s)
}

struct Hid {
    hw: Hardware<Mmio, Dma>,
    input: Input,
    /// For a tablet, the latest position of each axis. Absolute positions are not posted as they
    /// arrive: X and Y come as separate events and mean something only together, at the end of
    /// the report.
    tablet: Option<Tablet>,
    dev: Device,
}

impl Hid {
    fn drain(&mut self) {
        for _ in 0..BATCH {
            match self.hw.poll() {
                Ok(Some(e)) => self.translate(e),
                Ok(None) => return,
                Err(e) => {
                    granite::println!(
                        "virtio-input: the device lied ({e:?}); ending so that I am restarted"
                    );
                    self.hw.stop();
                    process::exit(3);
                }
            }
        }
    }

    /// Posts the tablet's position if it has moved since the last one.
    fn flush_position(&mut self) {
        if let Some((x, y)) = self.tablet.as_mut().and_then(Tablet::take) {
            self.input.position(x, y);
        }
    }

    /// Turns a Linux event into a PrettyBigRock one. Events with no equivalent are dropped.
    fn translate(&mut self, e: vinput::Event) {
        match (e.kind, e.code) {
            // A button or the wheel on a tablet happens where the pointer is now. The device
            // sends the movement before the click in the same report, but the end-of-report
            // event comes after both, so the position goes first, ahead of the click.
            (EV_KEY, BTN_LEFT) => {
                self.flush_position();
                self.input.button(input::BUTTON_LEFT, e.value != 0);
            }
            (EV_KEY, BTN_RIGHT) => {
                self.flush_position();
                self.input.button(input::BUTTON_RIGHT, e.value != 0);
            }
            (EV_KEY, BTN_MIDDLE) => {
                self.flush_position();
                self.input.button(input::BUTTON_MIDDLE, e.value != 0);
            }
            (EV_KEY, code) => {
                if let Some(k) = keys::from_evdev(code) {
                    self.input.key(k, e.value != 0);
                }
            }
            (EV_REL, REL_X) => self.input.motion(e.value, 0),
            (EV_REL, REL_Y) => self.input.motion(0, e.value),
            (EV_REL, REL_WHEEL) => {
                self.flush_position();
                self.input.wheel(e.value);
            }
            (EV_REL, REL_HWHEEL) => {
                self.flush_position();
                self.input.hwheel(e.value);
            }
            (EV_ABS, code) => {
                if let Some(t) = &mut self.tablet {
                    t.abs(code, e.value);
                }
            }
            (EV_SYN, SYN_REPORT) => self.flush_position(),
            _ => {}
        }
    }
}

impl Service for Hid {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.dev.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
        } else if let Some((_, reply)) = input::dispatch(&mut self.input, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, _bits: u64) {
        self.hw.interrupt_status();
        self.drain();
        for i in 0..self.dev.irqs.len() {
            self.dev.acknowledge(i);
        }
    }
}
