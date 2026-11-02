// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The 16550 serial port driver (docs/design/0011-device-manager-and-drivers.md, section 9):
//! serves `pbr.serial` for COM2. (COM1 is the kernel's console.)
//!
//! It sets the chip up from scratch every time it starts. Received bytes are collected in the
//! interrupt handler into a buffer of 4096 bytes; bytes that don't fit, and bytes the chip
//! itself lost, are reported to the next `read` as an overrun. Writing waits for the chip, but
//! never for long: it says how many bytes went out.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use granite::device::IoPorts;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::message::Incoming;
use granite::notification::Notification;
use granite::sys::rights;
use granite::{process, time};
use pbr_driver::{Device, Kind, Service, host_ready, serve};
use pbr_interfaces::pbr_serial::{self as serial, SerialError};

granite::main!(main);

/// Register offsets.
mod reg {
    pub const DATA: u64 = 0; // receive and transmit; the divisor's low byte with DLAB
    pub const IER: u64 = 1; // interrupt enable; the divisor's high byte with DLAB
    pub const IIR_FCR: u64 = 2;
    pub const LCR: u64 = 3;
    pub const MCR: u64 = 4;
    pub const LSR: u64 = 5;
    pub const MSR: u64 = 6;
}
const LSR_DATA: u64 = 1 << 0;
const LSR_OVERRUN: u64 = 1 << 1;
const LSR_THR_EMPTY: u64 = 1 << 5;
/// The chip's transmit FIFO holds this many bytes.
const FIFO: usize = 16;
/// The most bytes taken from the chip in one go: more than its FIFO, so a chip that always
/// says it has data (nothing there reads as all ones) can't hold the driver.
const DRAIN: usize = 64;
/// How many received bytes are kept for `read`.
const BUFFER: usize = 4096;
/// How long a write waits for the transmitter, in milliseconds.
const WRITE_WAIT_MS: u64 = 100;

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("serial16550: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("serial16550: {why}");
            3
        }
    }
}

fn run(mut dev: Device) -> Result<core::convert::Infallible, &'static str> {
    if dev.record.kind != Kind::Legacy {
        return Err("not a legacy device");
    }
    let ports = dev.ports.take().ok_or("no ports")?;
    let host = dev.host.take().ok_or("no endpoint to the device manager")?;
    let mut uart = Uart { ports };
    uart.start()?;

    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    let _bound = dev.bind_interrupts(&service).map_err(|e| e.0)?;
    let readable = Notification::create().map_err(|_| "can't make a notification")?;
    let mut port = Port { uart, dev, readable, buffer: VecDeque::new(), overrun: false };
    // Anything that arrived before the interrupt was bound.
    port.drain();
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, serial::REQUEST_BUFFER, &mut port)
}

struct Uart {
    ports: IoPorts,
}

impl Uart {
    fn r(&self, offset: u64) -> Result<u64, &'static str> {
        self.ports.read(offset, 1).map_err(|_| "a port read failed")
    }

    fn w(&self, offset: u64, value: u64) -> Result<(), &'static str> {
        self.ports.write(offset, 1, value).map_err(|_| "a port write failed")
    }

    /// Sets the chip up: 115200 baud, 8 data bits, no parity, one stop bit, FIFOs on, and
    /// an interrupt when a byte arrives.
    fn start(&mut self) -> Result<(), &'static str> {
        self.w(reg::IER, 0)?;
        self.w(reg::LCR, 0x80)?; // the divisor latch
        self.w(reg::DATA, 1)?;
        self.w(reg::IER, 0)?;
        self.w(reg::LCR, 0x03)?;
        self.w(reg::IIR_FCR, 0xC7)?; // enable and clear both FIFOs, 14-byte trigger
        self.w(reg::MCR, 0x0B)?; // DTR, RTS and OUT2 (which lets the chip raise its line)
        // Empty whatever is waiting, and clear the status registers.
        for _ in 0..DRAIN {
            if self.r(reg::LSR)? & LSR_DATA == 0 {
                break;
            }
            self.r(reg::DATA)?;
        }
        self.r(reg::MSR)?;
        self.r(reg::IIR_FCR)?;
        // A chip that isn't there reads as all ones everywhere.
        if self.r(reg::LSR)? == 0xFF && self.r(reg::LCR)? == 0xFF {
            return Err("no serial port answers");
        }
        self.w(reg::IER, 0x01)
    }
}

struct Port {
    uart: Uart,
    dev: Device,
    readable: Notification,
    buffer: VecDeque<u8>,
    overrun: bool,
}

impl Port {
    /// Moves what the chip has received into the buffer.
    fn drain(&mut self) {
        let mut got = false;
        for _ in 0..DRAIN {
            let Ok(status) = self.uart.r(reg::LSR) else { process::exit(3) };
            if status == 0xFF {
                // The chip has gone, or is not a chip.
                granite::println!("serial16550: the port stopped answering");
                process::exit(3);
            }
            if status & LSR_OVERRUN != 0 {
                self.overrun = true;
            }
            if status & LSR_DATA == 0 {
                break;
            }
            let Ok(byte) = self.uart.r(reg::DATA) else { process::exit(3) };
            if self.buffer.len() < BUFFER {
                self.buffer.push_back(byte as u8);
                got = true;
            } else {
                self.overrun = true;
            }
        }
        if got {
            let _ = self.readable.signal(1);
        }
    }
}

fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("serial16550: answer failed: {e}");
    }
}

impl serial::Server for Port {
    fn write(&mut self, data: Vec<u8>, reply: serial::write::Reply) {
        let deadline = time::after_ms(WRITE_WAIT_MS);
        let mut sent = 0;
        'send: while sent < data.len() {
            // The transmitter is empty when the chip says so; then a whole FIFO fits.
            loop {
                match self.uart.r(reg::LSR) {
                    Ok(s) if s & LSR_THR_EMPTY != 0 => break,
                    Ok(_) if time::now() < deadline => core::hint::spin_loop(),
                    Ok(_) => break 'send,
                    Err(_) => process::exit(3),
                }
            }
            for &b in data[sent..].iter().take(FIFO) {
                if self.uart.w(reg::DATA, u64::from(b)).is_err() {
                    process::exit(3);
                }
                sent += 1;
            }
        }
        answered(reply.ok(sent as u32));
    }

    fn read(&mut self, max: u32, reply: serial::read::Reply) {
        self.drain();
        if max as usize > serial::MAX_TRANSFER as usize {
            answered(reply.err(SerialError::BadArgument));
            return;
        }
        if self.overrun {
            self.overrun = false;
            answered(reply.err(SerialError::Overrun));
            return;
        }
        let n = self.buffer.len().min(max as usize);
        let data: Vec<u8> = self.buffer.drain(..n).collect();
        answered(reply.ok(&data));
    }

    fn events(&mut self, reply: serial::events::Reply) {
        match self.readable.0.duplicate(rights::WAIT | rights::TRANSFER) {
            Ok(h) => answered(reply.ok(Notification(h))),
            Err(_) => answered(reply.fail()),
        }
    }
}

impl Service for Port {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.dev.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
        } else if let Some((_, reply)) = serial::dispatch(self, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, bits: u64) {
        if bits & 1 != 0 {
            self.drain();
            self.dev.acknowledge(0);
        }
    }
}
