// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The framebuffer driver (docs/design/0011-device-manager-and-drivers.md, section 9): serves
//! `pbr.display` for the screen the firmware set up. It only describes the screen and hands out
//! the memory; clients draw on it themselves, so there is nothing to copy.
//!
//! The kernel draws its own messages on the screen until the display stack takes it over
//! (docs/design/0010-device-access.md, section 7). This driver does that when a client first
//! asks for the framebuffer, since from then on the client draws: it calls
//! `console_screen_release` with the kernel log the device manager gave it, which carries the
//! right `CONTROL` and nothing else. Kernel messages carry on over the serial port, and a kernel
//! panic is still drawn over whatever is on the screen.

#![no_std]
#![no_main]

extern crate alloc;

use granite::device::DeviceMemory;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::message::Incoming;
use granite::sys::rights;
use pbr_driver::{Device, Kind, Service, host_ready, serve};
use pbr_interfaces::pbr_display::{self as display, DisplayError, DisplayInfo, PixelFormat};

granite::main!(main);

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("framebuffer: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("framebuffer: {why}");
            1
        }
    }
}

fn run(mut dev: Device) -> Result<core::convert::Infallible, &'static str> {
    if dev.record.kind != Kind::Framebuffer {
        return Err("not a framebuffer");
    }
    let host = dev.host.take().ok_or("no endpoint to the device manager")?;
    let memory = dev.memory[0].take().ok_or("no screen memory")?;
    let s = dev.record.screen;
    let format = match s.format {
        1 => PixelFormat::Bgrx,
        2 => PixelFormat::Rgbx,
        _ => return Err("an unknown pixel format"),
    };
    // The screen has to fit in the memory it was given.
    let needed = u64::from(s.stride) * u64::from(s.height);
    if s.width == 0
        || s.height == 0
        || u64::from(s.stride) < u64::from(s.width) * 4
        || needed > dev.record.bars[0]
    {
        return Err("the screen doesn't fit its memory");
    }
    let info = DisplayInfo { width: s.width, height: s.height, stride: s.stride, format };
    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    granite::println!("framebuffer: {}x{}, {} bytes a row.", s.width, s.height, s.stride);
    let log_control = dev.log_control.take();
    let mut screen = Screen { info, memory, args: dev.args, log_control };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, display::REQUEST_BUFFER, &mut screen)
}

struct Screen {
    info: DisplayInfo,
    memory: DeviceMemory,
    args: alloc::string::String,
    /// The kernel log with `CONTROL`, until the screen has been taken from the kernel.
    log_control: Option<granite::Handle>,
}

fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("framebuffer: answer failed: {e}");
    }
}

impl display::Server for Screen {
    fn info(&mut self, reply: display::info::Reply) {
        answered(reply.ok(&self.info));
    }

    fn framebuffer(&mut self, reply: display::framebuffer::Reply) {
        // From now on a client draws: stop the kernel drawing over it. Once is enough (the
        // kernel doesn't take the screen back), so the capability goes after use.
        if let Some(log) = self.log_control.take()
            && let Err(e) = granite::device::console_screen_release(&log)
        {
            granite::println!("framebuffer: can't take the screen from the kernel: {e}");
        }
        // Clients may map, read, write and pass it on, but not copy it further.
        let copy = self
            .memory
            .0
            .duplicate(rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER)
            .map(DeviceMemory);
        answered(match copy {
            Ok(m) => reply.ok(m),
            Err(_) => reply.err(DisplayError::NoScreen),
        });
    }
}

impl Service for Screen {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
        } else if let Some((_, reply)) = display::dispatch(self, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, _bits: u64) {}
}
