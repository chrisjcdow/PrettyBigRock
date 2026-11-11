// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The framebuffer driver (docs/design/0020-screens-and-virtio-gpu.md, section 3): serves
//! `pbr.display` for the one screen the firmware set up.
//!
//! A *framebuffer* is the screen's own memory: whatever is written to it appears on the
//! screen. Clients don't get that memory. They draw in *buffers*, ordinary memory this driver
//! makes and shares with them, and *present* a buffer, saying which rectangles changed; the
//! driver copies those rectangles into the framebuffer, converting each pixel to the screen's
//! own format on the way. That is one copy more than drawing straight on the screen, and it
//! buys three things: the client never sees a half-drawn picture on the screen, it needn't
//! know the screen's pixel format (which can be three bytes a pixel), and this driver looks
//! the same to its client as one for a device with no framebuffer at all (virtio-gpu).
//!
//! `pbr_driver::display::Display` answers the interface and keeps its rules; this file is the
//! [`Backend`] it asks to do the work. Which bytes to copy is worked out, and checked, by
//! `pbr_devices::display::spans`, which is tested on the host; the copying here is the one
//! place that needs `unsafe`.
//!
//! The kernel draws its own messages on the screen until a client takes it (`Display` sees to
//! that, note 0010, section 7). Kernel messages carry on over the serial port, and a kernel
//! panic is still drawn over whatever is on the screen.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::memory::{Mapping, Memory};
use granite::message::Incoming;
use granite::sys::{perm, rights};
use pbr_devices::display::{self as rules, Format, Geometry, convert_row, spans};
use pbr_driver::display::{Backend, Display, Made};
use pbr_driver::{Device, Kind, Service, host_ready, serve};
use pbr_interfaces::pbr_display::{self as display, DisplayError, Mode, ScreenInfo};

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
    let format = Format::from_record(s.format).ok_or("an unknown pixel format")?;
    let geometry = Geometry { width: s.width, height: s.height, stride: s.stride, format };
    // The screen has to fit in the memory it was given.
    let size = dev.record.bars[0];
    if geometry.bytes().is_none_or(|bytes| bytes > size) {
        return Err("the screen doesn't fit its memory");
    }
    // Write-combining: the processor gathers writes to the screen into big bursts, which is
    // much faster for drawing. Reading such memory back is very slow, and nothing here does.
    let map = memory
        .map(0, size, perm::READ_WRITE | perm::WRITE_COMBINING)
        .map_err(|_| "can't map the screen's memory")?;
    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    granite::println!(
        "framebuffer: {}x{}, {} bytes a pixel, {} bytes a row.",
        s.width,
        s.height,
        format.bytes(),
        s.stride
    );
    let screen = Screen {
        geometry,
        map,
        buffers: BTreeMap::new(),
        row: vec![0; s.width as usize * 4],
        converted: vec![0; s.width as usize * format.bytes()],
    };
    let mut driver = Driver {
        display: Display::new("framebuffer", screen, dev.log_control.take()),
        args: dev.args,
    };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, display::REQUEST_BUFFER, &mut driver)
}

/// A buffer: ordinary memory, mapped here so that it can be copied to the screen.
struct Buffer {
    /// The memory. Clients' capabilities are copies of a copy of this one (`Made::memory`),
    /// so revoking that copy takes theirs and leaves this and its mapping alone.
    _memory: Memory,
    /// Mapped read-only: the driver never writes in a client's buffer.
    map: Mapping,
}

/// The firmware's screen.
struct Screen {
    geometry: Geometry,
    /// The framebuffer.
    map: Mapping,
    buffers: BTreeMap<u32, Buffer>,
    /// One row of a buffer, copied out of the shared memory before it is converted (the
    /// client can still write in the buffer, and code must never look twice at memory that
    /// can change under it and assume it saw the same thing).
    row: Vec<u8>,
    /// The same row in the screen's format.
    converted: Vec<u8>,
}

impl Screen {
    /// A buffer's bytes from one row to the next: four a pixel, with no gaps.
    fn stride(&self) -> u32 {
        self.geometry.width * 4
    }

    fn mode(&self) -> Mode {
        Mode { width: self.geometry.width, height: self.geometry.height, refresh_mhz: 0 }
    }
}

impl Backend for Screen {
    fn screens(&mut self) -> Vec<ScreenInfo> {
        vec![ScreenInfo {
            screen: 0,
            connector: String::from("Firmware"),
            identity: String::new(),
            connected: true,
            width_mm: 0,
            height_mm: 0,
            current: self.mode(),
            modes: vec![self.mode()],
        }]
    }

    fn set_mode(&mut self, screen: u32, mode: Mode) -> Result<(), DisplayError> {
        if screen != 0 {
            return Err(DisplayError::NoScreen);
        }
        // The firmware chose the mode, and only firmware could change it.
        let now = self.mode();
        if mode.width == now.width && mode.height == now.height {
            Ok(())
        } else {
            Err(DisplayError::BadMode)
        }
    }

    fn size(&mut self, screen: u32) -> Result<(u32, u32), DisplayError> {
        if screen != 0 {
            return Err(DisplayError::NoScreen);
        }
        Ok((self.geometry.width, self.geometry.height))
    }

    fn create(&mut self, _screen: u32, id: u32) -> Result<Made, DisplayError> {
        // Ordinary memory, charged to this driver's budget (its manifest says how much), so
        // the kernel refuses when a client asks for more buffers than that allows. `run`
        // checked the screen's size makes sense, so this can't overflow.
        let bytes = u64::from(self.stride()) * u64::from(self.geometry.height);
        let made = || -> granite::Result<(Memory, Mapping, Memory)> {
            let memory = Memory::create(bytes)?;
            let map = memory.map(perm::READ)?;
            let mask = rights::READ
                | rights::WRITE
                | rights::MAP
                | rights::DUPLICATE
                | rights::TRANSFER
                | rights::REVOKE;
            let share = Memory(memory.0.duplicate(mask)?);
            Ok((memory, map, share))
        };
        let (memory, map, share) = made().map_err(|_| DisplayError::NoMemory)?;
        self.buffers.insert(id, Buffer { _memory: memory, map });
        Ok(Made { memory: share, stride: self.stride() })
    }

    fn destroy(&mut self, id: u32) {
        // Dropping the buffer unmaps it and closes its memory, which frees it: the clients'
        // copies were revoked before this was called.
        self.buffers.remove(&id);
    }

    fn present(
        &mut self,
        id: u32,
        _buffer: rules::Buffer,
        changed: &[rules::Rect],
    ) -> Result<bool, DisplayError> {
        let Some(buffer) = self.buffers.get(&id) else { return Err(DisplayError::NoSuchBuffer) };
        let (stride, format) = (self.geometry.width * 4, self.geometry.format);
        for r in changed {
            let Some(rows) = spans(stride, buffer.map.len(), self.geometry, self.map.len(), *r)
            else {
                return Err(DisplayError::BadArgument);
            };
            for span in rows {
                let (from, to) = (span.pixels * 4, span.pixels * format.bytes());
                // `spans` never gives more pixels than the screen is wide, which is how long
                // these two were made; cutting them with `get_mut` checks that again.
                let (Some(row), Some(converted)) =
                    (self.row.get_mut(..from), self.converted.get_mut(..to))
                else {
                    return Err(DisplayError::BadArgument);
                };
                // SAFETY: `spans` checked that the `from` bytes at `span.from` are inside the
                // buffer's mapping, which is readable and lives as long as `buffer`; `row` is
                // this program's own memory, `from` bytes long, and not part of the mapping.
                // The client may be writing the buffer at the same time; a raw copy (not a
                // Rust reference to the shared memory) is allowed to race with that, and at
                // worst copies a half-drawn row.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        buffer.map.as_ptr().add(span.from),
                        row.as_mut_ptr(),
                        from,
                    );
                }
                let written = convert_row(row, format, converted);
                // SAFETY: `spans` checked that the `to` bytes at `span.to` are inside the
                // screen's mapping, which is writable and lives as long as `self`; `written`
                // is at most `to` (`converted`'s length), and `converted` is this program's
                // own memory.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        converted.as_ptr(),
                        self.map.as_mut_ptr().add(span.to),
                        written,
                    );
                }
            }
        }
        // The picture is on the screen already.
        Ok(true)
    }
}

/// The driver: the display, and the arguments it was started with.
struct Driver {
    display: Display<Screen>,
    args: String,
}

impl Service for Driver {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
        } else if let Some((_, reply)) = display::dispatch(&mut self.display, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
    }

    fn signal(&mut self, _bits: u64) {}

    fn gone(&mut self, badge: u64) {
        self.display.gone(badge);
    }
}
