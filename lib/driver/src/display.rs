// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What every display driver shares (docs/design/0020-screens-and-virtio-gpu.md, sections 1
//! and 2): answering `pbr.display`, so that each driver only has to say how its own device
//! shows a picture.
//!
//! [`Display`] answers the interface. It keeps the rules (in `pbr_devices::display::State`,
//! which is tested on the host): one client at a time presents, the one that last called
//! `take`; a present names a buffer that exists and rectangles inside it; a screen shows one
//! picture at a time. It also owns what the client was given: the taker's capability, the
//! notification that says a picture is on the screen, and the client's access to each buffer's
//! memory, which it takes away again when the buffer is destroyed. The device itself is behind
//! the [`Backend`] trait, which the driver implements.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use granite::Handle;
use granite::memory::Memory;
use granite::notification::Notification;
use granite::sys::rights;
use pbr_devices::display::{self as rules, Refusal, State};
use pbr_interfaces::pbr_display::{self as display, DisplayError, Mode, Rect, ScreenInfo};

/// The bit of the done notification that says the screens changed.
pub const SCREENS_CHANGED: u64 = 1 << 63;

/// A buffer the backend made.
#[derive(Debug)]
pub struct Made {
    /// The buffer's memory, as a capability the backend derived for this buffer alone, with
    /// the rights to read, write, map, copy, pass on and revoke. [`Display`] gives the client a
    /// copy of it, and revokes it when the buffer is destroyed, which takes the client's copy
    /// and unmaps the memory from the client (note 0006, section 5). So the backend must keep
    /// its own access to the memory through a different capability: the one this was copied
    /// from.
    pub memory: Memory,
    /// Bytes from the start of one row to the start of the next.
    pub stride: u32,
}

/// A display device, as [`Display`] sees it. A trait is a set of functions a type promises to
/// have; each display driver implements this one for its device.
///
/// By the time any of these is called, [`Display`] has checked that the request came from the
/// current taker, that the buffer exists, and that the rectangles are inside it. The backend
/// checks what only it knows: whether the screen exists and is connected, and the mode.
pub trait Backend {
    /// The screens, connected or not, in order of their numbers.
    fn screens(&mut self) -> Vec<ScreenInfo>;

    /// Shows `mode` on `screen`, or turns it off (width 0). Never called while the screen has
    /// a present that isn't done, and the screen's buffers are destroyed straight afterwards.
    fn set_mode(&mut self, screen: u32, mode: Mode) -> Result<(), DisplayError>;

    /// The size in pixels of the picture `screen` is showing: the size its buffers must be.
    /// `NoScreen` if there is no such screen, `NotConnected` if it is off or unplugged.
    fn size(&mut self, screen: u32) -> Result<(u32, u32), DisplayError>;

    /// Makes buffer `id` for `screen` at the size [`Backend::size`] just answered, zeroed.
    fn create(&mut self, screen: u32, id: u32) -> Result<Made, DisplayError>;

    /// Destroys buffer `id`. The client has already lost its access to the memory. It may be
    /// the buffer on the screen, or one whose present isn't done: the backend must then not
    /// report that present finished (see [`Display::finished`]).
    fn destroy(&mut self, id: u32);

    /// Puts the rectangles `changed` of buffer `id` on its screen. Returns `true` if the
    /// picture is on the screen already, `false` if the driver will call
    /// [`Display::finished`] when it is.
    fn present(
        &mut self,
        id: u32,
        buffer: rules::Buffer,
        changed: &[rules::Rect],
    ) -> Result<bool, DisplayError>;
}

/// A client's buffer: what [`Display`] keeps for it.
#[derive(Debug)]
struct Shared {
    /// The capability the client's copy was derived from (see [`Made::memory`]).
    memory: Memory,
}

/// Answers `pbr.display` for one device.
#[derive(Debug)]
pub struct Display<B: Backend> {
    /// The device.
    pub backend: B,
    rules: State,
    buffers: BTreeMap<u32, Shared>,
    /// The current taker's notification. A new one is made for each taker, so that an old
    /// taker hears nothing about the new one's pictures.
    done: Option<Notification>,
    /// Changes whenever the answer to `screens` does.
    generation: u32,
    /// The kernel log with the right to take the screen from the kernel, until that is done.
    log_control: Option<Handle>,
    /// The driver's name, for messages.
    name: &'static str,
}

fn refusal(r: Refusal) -> DisplayError {
    match r {
        Refusal::NoScreen => DisplayError::NoScreen,
        Refusal::NoMemory => DisplayError::NoMemory,
        Refusal::NoSuchBuffer => DisplayError::NoSuchBuffer,
        Refusal::NotTaken => DisplayError::NotTaken,
        Refusal::Busy => DisplayError::Busy,
        Refusal::BadArgument => DisplayError::BadArgument,
    }
}

impl<B: Backend> Display<B> {
    /// A display nobody has taken. `log_control` is the kernel log with `CONTROL`, if the
    /// kernel may be drawing its messages on one of this device's screens.
    pub fn new(name: &'static str, backend: B, log_control: Option<Handle>) -> Display<B> {
        Display {
            backend,
            rules: State::new(),
            buffers: BTreeMap::new(),
            done: None,
            generation: 1,
            log_control,
            name,
        }
    }

    /// A capability with a watched badge has gone (the service's `gone`). If it was the
    /// taker's, its buffers are destroyed and nobody has the screens.
    pub fn gone(&mut self, badge: u64) {
        let was_taker = self.rules.check(badge).is_ok();
        let old = self.rules.gone(badge);
        self.destroy_all(&old);
        if was_taker {
            self.done = None;
        }
    }

    /// The backend has finished putting a picture on `screen`: tell the taker.
    pub fn finished(&mut self, screen: u32) {
        if self.rules.done(screen)
            && let Some(n) = &self.done
        {
            let _ = n.signal(1 << screen);
        }
    }

    /// Something about the screens changed (one was plugged in or out, or resized): tell the
    /// taker to ask again.
    pub fn screens_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(n) = &self.done {
            let _ = n.signal(SCREENS_CHANGED);
        }
    }

    /// Destroys buffers the rules have already forgotten.
    fn destroy_all(&mut self, ids: &[u32]) {
        for id in ids {
            self.destroy(*id);
        }
    }

    fn destroy(&mut self, id: u32) {
        if let Some(shared) = self.buffers.remove(&id) {
            // Revoking destroys every copy made from this capability, wherever it is now, and
            // unmaps the memory from whoever mapped it through one.
            if let Err(e) = shared.memory.0.revoke() {
                granite::println!("{}: can't take buffer {id} back: {e}", self.name);
            }
        }
        self.backend.destroy(id);
    }

    fn answered(&self, r: granite::Result<()>) {
        if let Err(e) = r
            && e != granite::Error::PeerGone
        {
            granite::println!("{}: answer failed: {e}", self.name);
        }
    }
}

impl<B: Backend> display::Server for Display<B> {
    fn screens(&mut self, reply: display::screens::Reply) {
        let screens = self.backend.screens();
        self.answered(reply.ok(&screens, self.generation));
    }

    fn take(&mut self, reply: display::take::Reply) {
        // A fresh notification for this taker; the client's copy can wait on it and pass it
        // on, but not signal it.
        let made = Notification::create()
            .and_then(|n| Ok((n.0.duplicate(rights::WAIT | rights::TRANSFER)?, n)));
        let Ok((copy, done)) = made else {
            return self.answered(reply.err(DisplayError::NoMemory));
        };
        // From now on a client draws: stop the kernel drawing its messages over it (note
        // 0010, section 7). Once is enough, since the kernel never takes the screen back.
        if let Some(log) = self.log_control.take()
            && let Err(e) = granite::device::console_screen_release(&log)
        {
            granite::println!("{}: can't take the screen from the kernel: {e}", self.name);
        }
        let (badge, old) = self.rules.take();
        self.destroy_all(&old);
        self.done = Some(done);
        // The taker's capability is derived from the caller's (note 0013, section 2): it can
        // send and be passed on but not copied, and carries a badge of its own that the
        // kernel watches, so `gone` hears when the client drops it or ends.
        let taker = granite::interface::OutCap::derived(badge, rights::SEND | rights::TRANSFER);
        let r = reply.ok(taker, Notification(copy));
        if r.is_err() {
            // The caller never got the capability: nobody has the screens.
            self.rules.gone(badge);
            self.done = None;
        }
        self.answered(r);
    }

    fn set_mode(&mut self, screen: u32, mode: Mode, reply: display::set_mode::Reply) {
        if let Err(r) = self.rules.check(reply.badge()) {
            return self.answered(reply.err(refusal(r)));
        }
        if self.rules.is_busy(screen) {
            return self.answered(reply.err(DisplayError::Busy));
        }
        match self.backend.set_mode(screen, mode) {
            Ok(()) => {
                // Buffers made for the old size are no use for the new one.
                let old = self.rules.remove_screen(screen);
                self.destroy_all(&old);
                self.answered(reply.ok());
                // `screens` answers differently now (or may): the same signal as for a
                // screen plugged in, so a client has one place to hear of either.
                self.screens_changed();
            }
            Err(e) => self.answered(reply.err(e)),
        }
    }

    fn create_buffer(&mut self, screen: u32, reply: display::create_buffer::Reply) {
        if let Err(r) = self.rules.check(reply.badge()) {
            return self.answered(reply.err(refusal(r)));
        }
        let made = self.backend.size(screen).and_then(|(width, height)| {
            let id = self.rules.add(screen, width, height).map_err(refusal)?;
            match self.backend.create(screen, id) {
                Ok(made) => Ok((id, made)),
                Err(e) => {
                    let _ = self.rules.remove(id);
                    Err(e)
                }
            }
        });
        let (id, made) = match made {
            Ok(m) => m,
            Err(e) => return self.answered(reply.err(e)),
        };
        // The client may map it, read and write it, and pass it on, but not copy it.
        let mask = rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER;
        let copy = made.memory.0.duplicate(mask);
        self.buffers.insert(id, Shared { memory: made.memory });
        match copy {
            Ok(copy) => {
                let r = reply.ok(id, Memory(copy), made.stride);
                if r.is_err() {
                    let _ = self.rules.remove(id);
                    self.destroy(id);
                }
                self.answered(r);
            }
            Err(_) => {
                let _ = self.rules.remove(id);
                self.destroy(id);
                self.answered(reply.err(DisplayError::NoMemory));
            }
        }
    }

    fn destroy_buffer(&mut self, buffer: u32, reply: display::destroy_buffer::Reply) {
        if let Err(r) = self.rules.check(reply.badge()) {
            return self.answered(reply.err(refusal(r)));
        }
        match self.rules.remove(buffer) {
            Ok(_) => {
                self.destroy(buffer);
                self.answered(reply.ok());
            }
            Err(r) => self.answered(reply.err(refusal(r))),
        }
    }

    fn present(&mut self, buffer: u32, changed: Vec<Rect>, reply: display::present::Reply) {
        if let Err(r) = self.rules.check(reply.badge()) {
            return self.answered(reply.err(refusal(r)));
        }
        let changed: Vec<rules::Rect> = changed
            .iter()
            .map(|r| rules::Rect { x: r.x, y: r.y, width: r.width, height: r.height })
            .collect();
        let (b, rects) = match self.rules.present(buffer, &changed) {
            Ok(checked) => checked,
            Err(r) => return self.answered(reply.err(refusal(r))),
        };
        match self.backend.present(buffer, b, &rects) {
            Ok(now) => {
                // Answer first, so the client is waiting for the signal by the time it comes.
                self.answered(reply.ok());
                if now {
                    self.finished(b.screen);
                }
            }
            Err(e) => {
                // Nothing went to the screen, so it isn't busy.
                self.rules.done(b.screen);
                self.answered(reply.err(e));
            }
        }
    }
}
