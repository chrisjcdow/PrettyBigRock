// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio GPU driver (docs/design/0020-screens-and-virtio-gpu.md, section 4): serves
//! `pbr.display` for QEMU's `virtio-gpu`, over `pbr-virtio`.
//!
//! A virtio GPU has no screen memory to draw in. It keeps each picture in the host and is sent
//! commands: *copy these pixels from the guest's memory into the picture* and *show the picture
//! on this screen*. So the buffers a client draws in are DMA memory (memory a device reads by
//! itself, without the processor, at an address of its own that the device manager worked out),
//! and presenting a buffer means sending those commands.
//!
//! `pbr_driver::display::Display` answers the interface and keeps its rules; this file is the
//! [`Backend`] it asks to do the work. Everything that can be decided without the hardware is in
//! libraries with host tests: the commands and what the device may answer (`pbr_virtio::gpu`),
//! the screen's description of itself (`pbr_devices::edid`), the modes, merging rectangles and
//! which picture a screen is showing (`pbr_devices::screens`).
//!
//! # Waiting
//!
//! Commands that must have finished before the driver carries on (making and freeing a buffer,
//! turning a screen off, asking what the screens are) are sent and then waited for, polling the
//! queue every millisecond for at most a second. A present is not waited for: its last command
//! carries a *fence*, and the device answers that one only when everything before it is done.
//! That answer arrives as an interrupt, and then the client is told its picture is on the
//! screen. A device that doesn't answer in time, or says anything impossible, is broken: the
//! driver says so, resets it and ends, and the device manager starts it again (note 0012).
//! Nothing in this file panics on what a device does.
//!
//! # Rectangles
//!
//! A present can name 64 rectangles, and each needs a transfer command. Rather than feed the
//! queue as answers arrive, the driver merges them into at most four (their bounding boxes:
//! sending more pixels than were named is allowed, sending fewer is not), so a present is at
//! most seven commands and always fits.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, ReplyCap};
use granite::memory::Memory;
use granite::message::Incoming;
use granite::notification::Notification;
use granite::sys::rights;
use granite::{process, time};
use pbr_devices::display as rules;
use pbr_devices::edid::{self, Edid};
use pbr_devices::msix;
use pbr_devices::pci::{self, command};
use pbr_devices::screens::{
    Choice, Dispose, Shown, bounding_box, choose_mode, connector_name, merge_rects, mode_list,
    picture_bytes, plausible,
};
use pbr_driver::display::{Backend, Display, Made};
use pbr_driver::{Device, Dma, Mmio, Service, host_ready, serve};
use pbr_interfaces::pbr_display::{self as display, DisplayError, Mode, ScreenInfo};
use pbr_virtio::gpu::{self, Command, Completion, EVENT_DISPLAY, Gpu, Response, error_code};
use pbr_virtio::transport::find_layout;

granite::main!(main);

/// How long the driver waits for the device to answer a command it needs the answer to.
const TIMEOUT_MS: u64 = 1_000;
/// The most rectangles a present sends; more are merged.
const MAX_RECTS: usize = 4;
/// The most commands one present sends: a transfer for each rectangle, setting the scanout,
/// freeing the picture the scanout showed before, and the flush with the fence.
const PRESENT_COMMANDS: usize = MAX_RECTS + 3;
// A present must always fit in the commands the device lets be in flight at once.
const _: () = assert!(PRESENT_COMMANDS <= pbr_virtio::gpu::MIN_COMMANDS);
/// The size of the zeros written over a buffer, a piece at a time.
const ZEROS: usize = 64 * 1024;

fn main(mut start: granite::Start) -> i32 {
    let device = match Device::from_start(&mut start) {
        Ok(d) => d,
        Err(e) => {
            granite::println!("virtio-gpu: {e}");
            return 1;
        }
    };
    match run(device) {
        Ok(never) => match never {},
        Err(why) => {
            granite::println!("virtio-gpu: {why}");
            1
        }
    }
}

// A function that can fail returns a `Result`: either `Ok` with its answer or `Err` with a
// reason. `?` after a call that returns one hands back the `Err` from this function at once, so
// the steps below read as a list and stop at the first that goes wrong. This function only
// returns if something did: serving never ends, which is what the `Infallible` (a type with no
// values) in the `Ok` says.
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

    // Interrupts: a device gets the processor's attention by writing a small message (MSI-X)
    // to an address; the table in the device's BAR gets the addresses and numbers the kernel
    // gave us. A present is finished when its interrupt comes, so without one the driver is no
    // use.
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

    // The buffers: DMA memory the device manager made, each with the address the device knows
    // it by and the size the record says (a memory object may be rounded up to whole pages).
    // They are looked at before the device is started, so that a driver that can't use them
    // never touches the device.
    let mut pieces = Vec::new();
    for (i, (memory, bus)) in core::mem::take(&mut dev.buffers).into_iter().enumerate() {
        let size = dev.record.buffers.get(i).map_or(0, |b| b.1);
        if size == 0 || bus.checked_add(size).is_none() || u32::try_from(size).is_err() {
            return Err("a buffer has an address or size the device can't use");
        }
        pieces.push(Piece { memory, bus, size, in_use: false });
    }
    if pieces.is_empty() {
        return Err("no buffers to draw in");
    }

    let service = Endpoint::create().map_err(|_| "can't make an endpoint")?;
    let note = dev.bind_interrupts(&service).map_err(|e| e.0)?;
    let bars = core::mem::replace(&mut dev.bars, [const { None }; 6]);
    let gpu = Gpu::start(layout, bars, dma, bus, vector).map_err(|e| {
        granite::println!("virtio-gpu: the device can't be started: {e:?}");
        "the device can't be started"
    })?;

    let mut backend = VirtioGpu {
        gpu,
        screens: Vec::new(),
        pieces,
        buffers: BTreeMap::new(),
        pending: BTreeMap::new(),
        next_token: 1,
        zeros: vec![0; ZEROS],
        finished: Vec::new(),
        changed: false,
        failed: None,
    };
    if backend.refresh().is_err() {
        if let Some(why) = &backend.failed {
            granite::println!("virtio-gpu: {why}");
        }
        backend.gpu.stop();
        return Err("can't ask the device about its screens");
    }
    granite::println!(
        "virtio-gpu: {} screens ({} connected), {} buffers of {} bytes, {}.",
        backend.screens.len(),
        backend.screens.iter().filter(|s| s.connected()).count(),
        backend.pieces.len(),
        backend.pieces[0].size,
        if backend.gpu.has_edid() { "with EDID" } else { "no EDID" },
    );
    let mut driver = Driver {
        display: Display::new("virtio-gpu", backend, dev.log_control.take()),
        dev,
        _note: note,
    };
    host_ready(host, &service).map_err(|_| "the device manager didn't take the endpoint")?;
    serve(&service, display::REQUEST_BUFFER, &mut driver)
}

/// A piece of the DMA memory buffers are made in.
struct Piece {
    /// The memory. Clients get copies made from a copy of this one (`Made::memory`), so
    /// revoking that copy takes theirs and leaves this alone.
    memory: Memory,
    /// Where the device finds its first byte.
    bus: u64,
    size: u64,
    /// A buffer is made in it.
    in_use: bool,
}

/// A buffer a client has: a piece of memory, and the GPU resource that has it as its backing.
/// The resource's number is the buffer's.
struct Buf {
    piece: usize,
    screen: u32,
    width: u32,
    height: u32,
}

/// A screen: one of the GPU's scanouts.
struct Screen {
    /// The size the device says the scanout has, which is the mode it prefers.
    rect: gpu::Rect,
    /// The device says something is connected.
    enabled: bool,
    /// What the screen said about itself, if it did.
    edid: Option<Edid>,
    /// The size of the pictures it shows now, or `None` if it is off or nothing is connected.
    current: Option<(u32, u32)>,
    /// Which picture it shows.
    shown: Shown,
}

impl Screen {
    /// Something usable is connected: the device says so, and gives a size that could be real.
    fn connected(&self) -> bool {
        self.enabled && plausible(self.rect.width, self.rect.height)
    }

    fn preferred(&self) -> (u32, u32) {
        (self.rect.width, self.rect.height)
    }
}

/// A command in flight that nobody is waiting for.
enum Pending {
    /// A present's last command: when it comes back the picture is on the screen.
    Present { screen: u32, number: u64 },
    /// Any other: nothing to do but check that it worked.
    Quiet(&'static str),
}

/// Why a command did not give an answer.
enum Failure {
    /// The device refused it, with this error code.
    Refused(u32),
    /// The device is broken, or can't be reached. The reason is in [`VirtioGpu::failed`].
    Dead,
}

/// The device, as [`Display`] sees it.
struct VirtioGpu {
    gpu: Gpu<Mmio, Dma>,
    screens: Vec<Screen>,
    pieces: Vec<Piece>,
    buffers: BTreeMap<u32, Buf>,
    /// Commands in flight that nobody is waiting for, by token.
    pending: BTreeMap<u64, Pending>,
    next_token: u64,
    zeros: Vec<u8>,
    /// Screens whose present has finished, for the driver's loop to pass to
    /// [`Display::finished`] (the backend can't call [`Display`], which owns it).
    finished: Vec<u32>,
    /// The screens changed (the display event), for the loop to pass to
    /// [`Display::screens_changed`].
    changed: bool,
    /// Why the driver must end: the device failed. Once set, nothing more is sent.
    failed: Option<String>,
}

impl VirtioGpu {
    /// Gives up on the device, saying why. Only the first reason is kept.
    fn fail(&mut self, why: String) {
        if self.failed.is_none() {
            self.failed = Some(why);
        }
    }

    fn token(&mut self) -> u64 {
        let t = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        t
    }

    /// Takes every command the device has finished. Returns the one with token `wanted`, if it
    /// is among them; the rest are dealt with here.
    fn collect(&mut self, wanted: Option<u64>) -> Option<Completion> {
        let mut found = None;
        loop {
            match self.gpu.poll() {
                Ok(Some(c)) if Some(c.token) == wanted => found = Some(c),
                Ok(Some(c)) => self.handle(c),
                Ok(None) => return found,
                Err(e) => {
                    self.fail(format!("the device said something impossible ({e:?})"));
                    return found;
                }
            }
        }
    }

    /// A command nobody was waiting for has finished.
    fn handle(&mut self, c: Completion) {
        match self.pending.remove(&c.token) {
            Some(Pending::Present { screen, number }) => {
                if let Err(e) = c.result {
                    self.fail(format!("the device refused to show a picture ({:#x})", e.0));
                }
                // Tell the client, unless a newer present has gone to the screen since: that one
                // is what the screen is busy with.
                if self.screens.get(screen as usize).is_some_and(|s| s.shown.is_latest(number)) {
                    self.finished.push(screen);
                }
            }
            Some(Pending::Quiet(what)) => {
                if let Err(e) = c.result {
                    self.fail(format!("the device refused {what} ({:#x})", e.0));
                }
            }
            None => self.fail(String::from("the device answered a command that wasn't sent")),
        }
    }

    /// Waits until at least `n` commands can be sent.
    fn make_room(&mut self, n: usize) -> Result<(), Failure> {
        let deadline = time::after_ms(TIMEOUT_MS);
        loop {
            self.collect(None);
            if self.failed.is_some() {
                return Err(Failure::Dead);
            }
            if self.gpu.free_slots() >= n {
                return Ok(());
            }
            if time::now() >= deadline {
                self.fail(String::from("the device isn't finishing its commands"));
                return Err(Failure::Dead);
            }
            time::sleep_ms(1);
        }
    }

    /// Sends a command and carries on without waiting. `pending` says what to do when it is
    /// done.
    fn send(&mut self, command: Command, pending: Pending) -> Result<(), Failure> {
        if self.failed.is_some() {
            return Err(Failure::Dead);
        }
        let token = self.token();
        self.pending.insert(token, pending);
        if let Err(e) = self.gpu.submit(token, command) {
            self.pending.remove(&token);
            self.fail(format!("can't send {command:?} ({e:?})"));
            return Err(Failure::Dead);
        }
        Ok(())
    }

    /// Sends a command and waits for its answer, polling the queue every millisecond for at
    /// most a second. A time-out is a device failure.
    fn sync(&mut self, command: Command) -> Result<Response, Failure> {
        self.make_room(1)?;
        let token = self.token();
        if let Err(e) = self.gpu.submit(token, command) {
            self.fail(format!("can't send {command:?} ({e:?})"));
            return Err(Failure::Dead);
        }
        let deadline = time::after_ms(TIMEOUT_MS);
        loop {
            // Answers to other commands that come in meanwhile (a present's fence) are
            // recorded, not lost.
            if let Some(c) = self.collect(Some(token)) {
                return c.result.map_err(|e| Failure::Refused(e.0));
            }
            if self.failed.is_some() {
                return Err(Failure::Dead);
            }
            if time::now() >= deadline {
                self.fail(format!("the device didn't answer {command:?}"));
                return Err(Failure::Dead);
            }
            time::sleep_ms(1);
        }
    }

    /// Like [`VirtioGpu::sync`] for a command that has nothing to say but whether it worked. A
    /// refusal is a failure of the device, since the driver only sends commands that are right.
    fn sync_done(&mut self, command: Command) -> Result<(), Failure> {
        match self.sync(command)? {
            Response::Done => Ok(()),
            _ => {
                self.fail(format!("the device answered {command:?} with something else"));
                Err(Failure::Dead)
            }
        }
    }

    /// Like [`VirtioGpu::sync_done`], saying what it was for if the device refuses.
    fn must(&mut self, command: Command, what: &str) -> Result<(), Failure> {
        match self.sync_done(command) {
            Err(Failure::Refused(code)) => {
                self.fail(format!("the device refused {what} ({code:#x})"));
                Err(Failure::Dead)
            }
            other => other,
        }
    }

    /// Asks the device what its screens are, and how they have changed since last asked. Sets
    /// [`VirtioGpu::changed`] if anything the interface reports is different.
    fn refresh(&mut self) -> Result<(), Failure> {
        let info = match self.sync(Command::GetDisplayInfo) {
            Ok(Response::DisplayInfo(info)) => info,
            Ok(_) => {
                self.fail(String::from("the device answered the display information wrongly"));
                return Err(Failure::Dead);
            }
            Err(Failure::Refused(code)) => {
                self.fail(format!("the device refused to describe its screens ({code:#x})"));
                return Err(Failure::Dead);
            }
            Err(Failure::Dead) => return Err(Failure::Dead),
        };
        let first = self.screens.is_empty();
        let mut changed = false;
        for (i, scanout) in info.iter().enumerate() {
            let usable = scanout.enabled && plausible(scanout.rect.width, scanout.rect.height);
            // A screen's description of itself, for a screen that is there. A refusal or a
            // description that makes no sense is just no description.
            let mut edid = None;
            if usable && self.gpu.has_edid() {
                match self.sync(Command::GetEdid { scanout: i as u32 }) {
                    Ok(Response::Edid(bytes)) => edid = edid::parse(&bytes),
                    Ok(_) => {
                        self.fail(String::from("the device answered an EDID wrongly"));
                        return Err(Failure::Dead);
                    }
                    Err(Failure::Refused(_)) => {}
                    Err(Failure::Dead) => return Err(Failure::Dead),
                }
            }
            if first {
                self.screens.push(Screen {
                    rect: scanout.rect,
                    enabled: scanout.enabled,
                    edid,
                    current: usable.then_some((scanout.rect.width, scanout.rect.height)),
                    shown: Shown::new(),
                });
                continue;
            }
            let Some(screen) = self.screens.get_mut(i) else { continue };
            let was = screen.connected();
            // Only a connected screen's size and EDID mean anything.
            changed |=
                was != usable || (usable && (screen.rect != scanout.rect || screen.edid != edid));
            screen.rect = scanout.rect;
            screen.enabled = scanout.enabled;
            screen.edid = edid;
            if !was && usable {
                // Plugged in: it shows pictures of the size it prefers.
                screen.current = Some(screen.preferred());
            } else if was && !usable {
                // Unplugged: it shows nothing, and the picture it was given is no longer on
                // anything.
                screen.current = None;
                let leftover = screen.shown.clear();
                if let Some(resource) = leftover {
                    self.must(Command::Unref { resource }, "freeing a picture")?;
                }
            }
        }
        self.changed |= changed;
        Ok(())
    }

    /// The smallest a buffer can be, which limits the sizes a screen can be given.
    fn buffer_bytes(&self) -> u64 {
        self.pieces.iter().map(|p| p.size).min().unwrap_or(0)
    }

    /// Writes zeros over a piece of buffer memory, so that a new buffer holds nothing of the
    /// last one's pictures, which may have been another client's.
    fn zero(&self, piece: usize) -> granite::Result<()> {
        // `let ... else` takes the value out of the `Some`, or runs the `else` block, which
        // must leave this function.
        let Some(p) = self.pieces.get(piece) else { return Err(granite::Error::InvalidArgument) };
        let mut at = 0;
        while at < p.size {
            let n = (p.size - at).min(self.zeros.len() as u64);
            p.memory.write(at, &self.zeros[..n as usize])?;
            at += n;
        }
        Ok(())
    }

    /// What the device being broken looks like to a client.
    fn dead(&self) -> DisplayError {
        DisplayError::NotConnected
    }

    /// Reacts to an interrupt: finished commands, and the device's events.
    fn interrupt(&mut self) {
        // Reading the status clears it; whatever it says, look at everything, since the status
        // isn't promised to say anything when interrupts are messages.
        let _ = self.gpu.interrupt_status();
        self.collect(None);
        match self.gpu.take_events() {
            Ok(bits) if bits & EVENT_DISPLAY != 0 => {
                // A failure is recorded in `failed` and ends the driver.
                let _ = self.refresh();
            }
            Ok(_) => {}
            Err(e) => self.fail(format!("can't read the device's events ({e:?})")),
        }
    }
}

// `Backend` is a trait, a list of functions a type promises to have; `Display` calls these
// without knowing which device is behind them.
impl Backend for VirtioGpu {
    fn screens(&mut self) -> Vec<ScreenInfo> {
        self.screens
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let connected = s.connected();
                let (width_mm, height_mm, identity) = match &s.edid {
                    Some(e) if connected => (e.width_mm, e.height_mm, e.identity.clone()),
                    _ => (0, 0, String::new()),
                };
                let mode = |(width, height)| Mode { width, height, refresh_mhz: 0 };
                ScreenInfo {
                    screen: i as u32,
                    connector: connector_name(i as u32),
                    identity,
                    connected,
                    width_mm,
                    height_mm,
                    current: s.current.map_or(Mode { width: 0, height: 0, refresh_mhz: 0 }, mode),
                    modes: if connected {
                        mode_list(s.preferred()).into_iter().map(mode).collect()
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect()
    }

    fn set_mode(&mut self, screen: u32, mode: Mode) -> Result<(), DisplayError> {
        if self.failed.is_some() {
            return Err(self.dead());
        }
        let Some(s) = self.screens.get(screen as usize) else { return Err(DisplayError::NoScreen) };
        if !s.connected() {
            return Err(DisplayError::NotConnected);
        }
        match choose_mode(mode.width, mode.height, self.buffer_bytes()) {
            Choice::Bad => Err(DisplayError::BadMode),
            // The new size takes effect with the next present, which names a buffer of that
            // size (`size` answers it from now on).
            Choice::Show(width, height) => {
                self.screens[screen as usize].current = Some((width, height));
                Ok(())
            }
            Choice::Off => {
                let off = Command::SetScanout {
                    scanout: screen,
                    resource: 0,
                    rect: gpu::Rect::default(),
                };
                self.must(off, "turning a screen off").map_err(|_| self.dead())?;
                let s = &mut self.screens[screen as usize];
                s.current = None;
                let leftover = s.shown.clear();
                if let Some(resource) = leftover {
                    self.must(Command::Unref { resource }, "freeing a picture")
                        .map_err(|_| self.dead())?;
                }
                Ok(())
            }
        }
    }

    fn size(&mut self, screen: u32) -> Result<(u32, u32), DisplayError> {
        let Some(s) = self.screens.get(screen as usize) else { return Err(DisplayError::NoScreen) };
        match s.current {
            Some(size) if s.connected() => Ok(size),
            _ => Err(DisplayError::NotConnected),
        }
    }

    fn create(&mut self, screen: u32, id: u32) -> Result<Made, DisplayError> {
        if self.failed.is_some() {
            return Err(self.dead());
        }
        let (width, height) = self.size(screen)?;
        // A picture too big for a buffer, or none free, is a lack of memory.
        let bytes = picture_bytes(width, height).ok_or(DisplayError::NoMemory)?;
        let len = u32::try_from(bytes).map_err(|_| DisplayError::NoMemory)?;
        let piece = self
            .pieces
            .iter()
            .position(|p| !p.in_use && p.size >= bytes)
            .ok_or(DisplayError::NoMemory)?;
        self.zero(piece).map_err(|_| DisplayError::NoMemory)?;
        // The copy `Display` shares with the client and revokes when the buffer is destroyed.
        let mask = rights::READ
            | rights::WRITE
            | rights::MAP
            | rights::DUPLICATE
            | rights::TRANSFER
            | rights::REVOKE;
        let share =
            self.pieces[piece].memory.0.duplicate(mask).map_err(|_| DisplayError::NoMemory)?;
        let bus = self.pieces[piece].bus;

        // The device may be out of memory, which is the client's problem; anything else it
        // refuses is its own.
        let make = Command::CreateResource { resource: id, width, height };
        if let Err(e) = self.sync_done(make) {
            return Err(self.refused(e));
        }
        let attach = Command::AttachBacking { resource: id, address: bus, len };
        if let Err(e) = self.sync_done(attach) {
            // Undo the first half.
            let _ = self.sync_done(Command::Unref { resource: id });
            return Err(self.refused(e));
        }
        self.pieces[piece].in_use = true;
        self.buffers.insert(id, Buf { piece, screen, width, height });
        Ok(Made { memory: Memory(share), stride: width * 4 })
    }

    fn destroy(&mut self, id: u32) {
        let Some(buf) = self.buffers.remove(&id) else { return };
        // The screen keeps showing its last picture: if this is the buffer on the screen, its
        // memory is taken away but the resource stays until the screen has another.
        let dispose = match self.screens.get_mut(buf.screen as usize) {
            Some(s) => s.shown.destroy(id),
            None => Dispose::Free,
        };
        let command = match dispose {
            Dispose::Free => Command::Unref { resource: id },
            Dispose::Detach => Command::DetachBacking { resource: id },
        };
        // Waiting for the answer means every command sent before it is finished too, so the
        // device is no longer reading the buffer's memory when it is free to be used again.
        let _ = self.must(command, "freeing a buffer");
        if let Some(p) = self.pieces.get_mut(buf.piece) {
            p.in_use = false;
        }
    }

    fn present(
        &mut self,
        id: u32,
        _buffer: rules::Buffer,
        changed: &[rules::Rect],
    ) -> Result<bool, DisplayError> {
        if self.failed.is_some() {
            return Err(self.dead());
        }
        let Some(buf) = self.buffers.get(&id) else { return Err(DisplayError::NoSuchBuffer) };
        let (screen, width, height) = (buf.screen, buf.width, buf.height);
        let Some(s) = self.screens.get(screen as usize) else { return Err(DisplayError::NoScreen) };
        if !s.connected() || s.current.is_none() {
            return Err(DisplayError::NotConnected);
        }
        // A buffer of another size than the screen shows now (the screen was unplugged and
        // plugged in again, say) can't be shown.
        if s.current != Some((width, height)) {
            return Err(DisplayError::NoSuchBuffer);
        }
        let rects = merge_rects(changed, MAX_RECTS);
        let Some(bounds) = bounding_box(&rects) else { return Err(DisplayError::BadArgument) };
        self.make_room(PRESENT_COMMANDS).map_err(|_| self.dead())?;

        let plan = self.screens[screen as usize].shown.present(id);
        // Copy what changed into the resource...
        for r in &rects {
            let rect = gpu::Rect { x: r.x, y: r.y, width: r.width, height: r.height };
            // Where in the buffer's memory the rectangle's first pixel is.
            let offset = (u64::from(r.y) * u64::from(width) + u64::from(r.x)) * 4;
            let transfer = Command::Transfer { resource: id, rect, offset };
            self.send(transfer, Pending::Quiet("a copy of pixels")).map_err(|_| self.dead())?;
        }
        // ...show the resource, if the screen isn't showing it already, and let go of the one it
        // showed before if that was only kept for the screen's sake...
        let whole = gpu::Rect { x: 0, y: 0, width, height };
        if plan.set_scanout {
            let set = Command::SetScanout { scanout: screen, resource: id, rect: whole };
            self.send(set, Pending::Quiet("showing a picture")).map_err(|_| self.dead())?;
        }
        if let Some(resource) = plan.free {
            self.send(Command::Unref { resource }, Pending::Quiet("freeing a picture"))
                .map_err(|_| self.dead())?;
        }
        // ...and flush it, with a fence: the answer comes when all of that is done. A picture
        // that has only just been put on the screen is flushed whole, since nothing says what
        // the screen held before.
        let area = if plan.set_scanout {
            whole
        } else {
            gpu::Rect { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height }
        };
        let flush = Command::Flush { resource: id, rect: area, fenced: true };
        let pending = Pending::Present { screen, number: plan.number };
        self.send(flush, pending).map_err(|_| self.dead())?;
        Ok(false)
    }
}

impl VirtioGpu {
    /// What a client is told when the device refused a command it was sent for it: no memory
    /// if the device said so; otherwise the device is at fault, and the driver ends.
    fn refused(&mut self, failure: Failure) -> DisplayError {
        if let Failure::Refused(code) = failure
            && code != error_code::OUT_OF_MEMORY
        {
            self.fail(format!("the device refused to make a buffer ({code:#x})"));
        }
        DisplayError::NoMemory
    }
}

/// The driver: the display, and what it was started with.
struct Driver {
    display: Display<VirtioGpu>,
    dev: Device,
    /// Kept so that the interrupts stay tied to the endpoint.
    _note: Notification,
}

impl Driver {
    /// After anything that may have talked to the device: tells the display what finished and
    /// what changed, and ends the driver if the device failed.
    fn settle(&mut self) {
        let backend = &mut self.display.backend;
        let finished = core::mem::take(&mut backend.finished);
        let changed = core::mem::take(&mut backend.changed);
        let failed = backend.failed.take();
        for screen in finished {
            self.display.finished(screen);
        }
        if changed {
            self.display.screens_changed();
        }
        if let Some(why) = failed {
            granite::println!("virtio-gpu: {why}; ending so that I am restarted");
            self.display.backend.gpu.stop();
            process::exit(1);
        }
    }
}

impl Service for Driver {
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap) {
        if pbr_driver::debug::enabled(&self.dev.args) && pbr_driver::debug::is_debug(&msg) {
            pbr_driver::debug::dispatch(msg, buffer, reply);
            return;
        }
        // Take what has finished first: a lost interrupt then costs a little time, not the
        // screen.
        self.display.backend.collect(None);
        if let Some((_, reply)) = display::dispatch(&mut self.display, msg, buffer, reply) {
            granite::interface::unknown(reply);
        }
        self.settle();
    }

    fn signal(&mut self, _bits: u64) {
        self.display.backend.interrupt();
        self.settle();
        for i in 0..self.dev.irqs.len() {
            self.dev.acknowledge(i);
        }
    }

    fn gone(&mut self, badge: u64) {
        self.display.gone(badge);
        self.settle();
    }
}
