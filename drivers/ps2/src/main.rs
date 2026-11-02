// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PS/2 controller driver (docs/design/0011-device-manager-and-drivers.md, section 9):
//! serves `pbr.input` for the keyboard and mouse on an 8042 controller (x86-64).
//!
//! It starts the controller from scratch every time (note 0012): turns both ports off, empties
//! the output, tests the controller, and turns on translation to scan code set 1, which
//! `pbr_devices::keys` decodes. Anything the devices send is untrusted input to a decoder that
//! can't be wedged by it.

#![no_std]
#![no_main]

extern crate alloc;

use granite::device::IoPorts;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::message::Incoming;
use granite::{process, time};
use pbr_devices::keys::{Mouse, Scan, Set1};
use pbr_driver::input::Input;
use pbr_driver::{Device, Kind, Service, host_ready, serve};
use pbr_interfaces::pbr_input::{self as input, InputKind};

granite::main!(main);

/// Ports, as offsets into the slice the driver was given (which starts at 0x60).
const DATA: u64 = 0;
const STATUS: u64 = 4; // read: status; write: command
const OBF: u64 = 1 << 0; // output buffer full: a byte is waiting
const IBF: u64 = 1 << 1; // input buffer full: the controller isn't ready for a byte
const AUX: u64 = 1 << 5; // the waiting byte came from the mouse
/// The most bytes taken in one go.
const DRAIN: usize = 64;
/// How long to wait for the controller or a device, in milliseconds.
const WAIT_MS: u64 = 200;

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("ps2: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("ps2: {why}");
            3
        }
    }
}

fn run(mut dev: Device) -> Result<core::convert::Infallible, &'static str> {
    if dev.record.kind != Kind::Legacy || dev.irqs.len() != 2 {
        return Err("not a PS/2 controller");
    }
    let ports = dev.ports.take().ok_or("no ports")?;
    let host = dev.host.take().ok_or("no endpoint to the device manager")?;
    let mut ctl = Controller { ports };
    let mouse_present = ctl.start()?;
    granite::println!("ps2: controller started{}.", if mouse_present { "" } else { " (no mouse)" });

    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    let _bound = dev.bind_interrupts(&service).map_err(|e| e.0)?;
    let input = Input::new(InputKind::KeyboardAndMouse, "PS/2 keyboard and mouse")?;
    let mut s = Ps2 { ctl, dev, input, keys: Set1::default(), mouse: Mouse::default(), buttons: 0 };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, 0, &mut s)
}

struct Controller {
    ports: IoPorts,
}

impl Controller {
    fn status(&self) -> Result<u64, &'static str> {
        self.ports.read(STATUS, 1).map_err(|_| "a port read failed")
    }

    fn wait(&self, mut ready: impl FnMut(u64) -> bool) -> Result<bool, &'static str> {
        let deadline = time::after_ms(WAIT_MS);
        loop {
            if ready(self.status()?) {
                return Ok(true);
            }
            if time::now() >= deadline {
                return Ok(false);
            }
            core::hint::spin_loop();
        }
    }

    fn write(&self, offset: u64, value: u64) -> Result<(), &'static str> {
        if !self.wait(|s| s & IBF == 0)? {
            return Err("the controller doesn't take a byte");
        }
        self.ports.write(offset, 1, value).map_err(|_| "a port write failed")
    }

    fn command(&self, c: u64) -> Result<(), &'static str> {
        self.write(STATUS, c)
    }

    /// The next byte from the controller, if one comes soon.
    fn read(&self) -> Result<Option<u8>, &'static str> {
        if !self.wait(|s| s & OBF != 0)? {
            return Ok(None);
        }
        let b = self.ports.read(DATA, 1).map_err(|_| "a port read failed")?;
        Ok(Some(b as u8))
    }

    /// Sends a byte to the keyboard (or, with `mouse`, the mouse) and waits for its
    /// acknowledgement. Other bytes (such as a reset's self-test result) are skipped.
    fn send(&self, mouse: bool, byte: u64) -> Result<bool, &'static str> {
        for _ in 0..3 {
            if mouse {
                self.command(0xD4)?;
            }
            self.write(DATA, byte)?;
            for _ in 0..8 {
                match self.read()? {
                    Some(0xFA) => return Ok(true),
                    Some(0xFE) => break, // resend
                    Some(_) => {}
                    None => return Ok(false),
                }
            }
        }
        Ok(false)
    }

    /// Starts the controller and the devices. Returns whether a mouse answered.
    fn start(&mut self) -> Result<bool, &'static str> {
        self.command(0xAD)?; // keyboard port off
        self.command(0xA7)?; // mouse port off
        for _ in 0..DRAIN {
            if self.status()? & OBF == 0 {
                break;
            }
            self.ports.read(DATA, 1).map_err(|_| "a port read failed")?;
        }
        if self.status()? == 0xFF {
            return Err("no controller answers");
        }
        self.command(0xAA)?; // self-test
        if self.read()? != Some(0x55) {
            return Err("the controller failed its self-test");
        }
        // Interrupts off, clocks on, translation to set 1 on.
        self.command(0x20)?;
        let config = self.read()?.ok_or("no configuration byte")?;
        self.command(0x60)?;
        self.write(DATA, u64::from((config & !0x33) | 0x40))?;
        self.command(0xAE)?;
        self.command(0xA8)?;
        let keyboard = self.send(false, 0xF4)?; // start sending scan codes
        if !keyboard {
            granite::println!("ps2: no keyboard answers");
        }
        let mouse = self.send(true, 0xF6)? && self.send(true, 0xF4)?;
        // Now let both raise interrupts.
        self.command(0x60)?;
        self.write(DATA, u64::from((config & !0x30) | 0x43))?;
        Ok(mouse)
    }
}

struct Ps2 {
    ctl: Controller,
    dev: Device,
    input: Input,
    keys: Set1,
    mouse: Mouse,
    buttons: u8,
}

impl Ps2 {
    fn drain(&mut self) {
        for _ in 0..DRAIN {
            let Ok(status) = self.ctl.status() else { process::exit(3) };
            if status & OBF == 0 {
                break;
            }
            let Ok(byte) = self.ctl.ports.read(DATA, 1) else { process::exit(3) };
            let byte = byte as u8;
            if status & AUX != 0 {
                if let Some(p) = self.mouse.feed(byte) {
                    self.input.motion(p.dx, p.dy);
                    let changed = p.buttons ^ self.buttons;
                    for (bit, code) in [
                        (1, input::BUTTON_LEFT),
                        (2, input::BUTTON_RIGHT),
                        (4, input::BUTTON_MIDDLE),
                    ] {
                        if changed & bit != 0 {
                            self.input.button(code, p.buttons & bit != 0);
                        }
                    }
                    self.buttons = p.buttons;
                }
            } else if let Scan::Key(code, pressed) = self.keys.feed(byte) {
                self.input.key(code, pressed);
            }
        }
    }
}

impl Service for Ps2 {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.dev.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
        } else if let Some((_, reply)) = input::dispatch(&mut self.input, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, bits: u64) {
        self.drain();
        for i in 0..2 {
            if bits & (1 << i) != 0 {
                self.dev.acknowledge(i);
            }
        }
    }
}
