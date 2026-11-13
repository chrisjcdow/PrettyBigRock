// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The connection to the display server: the system calls, on top of the plain computation in
//! the other modules (docs/design/0023-draw-window-and-sandstone.md, section 4).
//!
//! Two OS ideas appear here. A **capability** is an unforgeable handle to something another
//! program (or the kernel) owns; the app's `pbr.window` capability is its only way to ask the
//! display server for anything (note 0006). A **notification** is a set of 64 flag bits that
//! one thread can set and another can wait on, which is how the display server rings the app's
//! "doorbell" without the app having to poll.

use crate::clip;
use crate::error::Error;
use crate::event::{Event, Incoming, PointerKind, Win, decode};
use crate::frames::{self, Frames};
use crate::timers::TimerQueue;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::ring::{Consumer, RingError};
use granite::sys::{perm, rights};
use granite::time;
use pbr_draw::{Canvas, Damage};
use pbr_interfaces::pbr_window::{self as wire, ClipForm, Cursor, Edge, Point, Size};

pub use wire::{WindowFlags, WindowKind};

/// The start-up role the app's `pbr.window` capability arrives at (note 0024).
pub const ROLE: u32 = 1003;

/// Bit 0 is the display server's doorbell. Bit 1: the display server has gone (the receiver
/// watch).
const BIT_GONE: u64 = 2;
/// Bits 8 to 63 are the app's.
const APP_BITS: u64 = !0xFF;
/// How long one call to the display server may take. The display server answers at once,
/// unless it is busy with a display driver itself (setting a screen up on an emulated machine
/// has taken ten seconds); an app shouldn't give up on it for that.
const CALL_MS: u64 = 30_000;
/// How long `draw` waits for a frame to come back.
const RELEASE_MS: u64 = 30_000;
/// The most events taken from the ring in one go, so that a flood cannot keep the app here.
const DRAIN_MAX: u32 = 1024;

/// A window's last configure.
#[derive(Clone, Copy, Debug)]
struct Configure {
    size: (u32, u32),
    scale: u32,
}

/// A window's two frames, mapped.
struct Buffers {
    /// Kept so the memory is not freed while it is mapped.
    _memory: Memory,
    mapping: Mapping,
    /// Device pixels.
    width: u32,
    height: u32,
    /// Pixels (not bytes) from one row to the next.
    stride: usize,
    /// The logical size and scale these were made for.
    logical: (u32, u32),
    scale: u32,
}

impl Buffers {
    /// Maps `memory` and checks that it really holds two frames of the size it claims. The
    /// numbers come from another program, so none of them is trusted.
    fn new(
        memory: Memory,
        (width, height, stride_bytes): (u32, u32, u32),
        logical: (u32, u32),
        scale: u32,
    ) -> Result<Buffers, Error> {
        if stride_bytes % 4 != 0 || stride_bytes / 4 < width || width > 16_384 || height > 16_384 {
            return Err(Error::Broken);
        }
        let mapping = memory.map(perm::READ_WRITE)?;
        let frame_bytes = u64::from(height) * u64::from(stride_bytes);
        if frame_bytes * 2 > mapping.len() || mapping.addr() % 4 != 0 {
            return Err(Error::Broken);
        }
        Ok(Buffers {
            _memory: memory,
            mapping,
            width,
            height,
            stride: (stride_bytes / 4) as usize,
            logical,
            scale,
        })
    }

    fn frame_len(&self) -> usize {
        self.height as usize * self.stride
    }

    /// Frame `draw` to draw in, and the other frame to copy from.
    fn split(&mut self, draw: usize) -> (&mut [u32], &[u32]) {
        let len = self.frame_len();
        let base = self.mapping.as_mut_ptr() as *mut u32;
        let (d, s) = if draw == 0 { (0, len) } else { (len, 0) };
        // SAFETY: `new` checked that the mapping holds two frames of `len` pixels, starting
        // on a page (so aligned for `u32`), and the two ranges do not overlap. The display
        // server maps these frames read only, and it is only given the one the app has
        // committed, so nothing else writes to what we draw in. The borrow of `self` keeps
        // the mapping alive for as long as the slices are.
        unsafe {
            (
                core::slice::from_raw_parts_mut(base.add(d), len),
                core::slice::from_raw_parts(base.add(s), len),
            )
        }
    }
}

/// What the connection knows about one window.
struct WindowState {
    number: u32,
    configure: Option<Configure>,
    buffers: Option<Buffers>,
    frames: Frames,
    drawn: bool,
    /// The last pointer position in the window, 1/256 logical pixels (button and scroll
    /// events do not carry one).
    pointer: (i32, i32),
}

/// An app's connection to the display server.
pub struct Connection {
    client: wire::Client,
    /// Everything the app waits for: bit 0 events, bit 1 the server gone, 8 to 63 the app's.
    wake: Notification,
    ring: Consumer,
    /// Kept so the ring stays mapped as long as `ring` reads it.
    _ring_map: Mapping,
    _ring_memory: Memory,
    queue: VecDeque<Event>,
    timers: TimerQueue,
    windows: Vec<WindowState>,
    /// App bits signalled but not yet given out.
    app_bits: u64,
    /// The display server has gone (or its ring is broken): `Disconnected` is next.
    gone: bool,
    /// `Disconnected` has been given.
    finished: bool,
    last_error: Option<Error>,
}

impl Connection {
    /// Takes the app's `pbr.window` capability from its start-up block (role [`ROLE`]).
    #[cfg(target_os = "prettybigrock")]
    pub fn from_start(start: &mut granite::Start) -> Result<Connection, Error> {
        let handle = start.take(ROLE).ok_or(Error::NoCapability)?;
        Connection::new(Endpoint(handle))
    }

    /// Connects through `endpoint`: asks for the event ring, and watches for the display
    /// server going.
    pub fn new(endpoint: Endpoint) -> Result<Connection, Error> {
        let mut client = wire::Client::new(endpoint);
        client.set_timeout_ms(CALL_MS);
        let wake = Notification::create()?;
        // Watch first, so that a display server that is already gone is noticed.
        client.endpoint().watch_receiver(&wake, BIT_GONE)?;
        let doorbell = Notification(wake.0.duplicate(rights::SIGNAL | rights::TRANSFER)?);
        let memory = client.events(doorbell)?;
        let mapping = memory.map(perm::READ_WRITE)?;
        // SAFETY: the mapping is readable and writable for its whole length and starts on a
        // page (aligned to 64, as a ring needs); it is kept in the same struct as the
        // consumer, so it outlives it. The ring's contents are the display server's to write
        // and `Consumer` copes with any of them.
        let ring = unsafe { Consumer::new(mapping.as_mut_ptr(), mapping.len() as usize) }
            .map_err(|_| Error::Broken)?;
        Ok(Connection {
            client,
            wake,
            ring,
            _ring_map: mapping,
            _ring_memory: memory,
            queue: VecDeque::new(),
            timers: TimerQueue::new(),
            windows: Vec::new(),
            app_bits: 0,
            gone: false,
            finished: false,
            last_error: None,
        })
    }

    /// The raw interface, for a call this library has no method for.
    pub fn client(&self) -> &wire::Client {
        &self.client
    }

    /// The memory the event ring lives in, for a test program that wants to attack it.
    pub fn ring_memory(&self) -> &Memory {
        &self._ring_memory
    }

    /// A copy of the wake-up notification that can only signal, for other threads: signal
    /// any of bits 8 to 63 to get an [`Event::Notified`].
    pub fn notifier(&self) -> Result<Notification, Error> {
        Ok(Notification(self.wake.0.duplicate(rights::SIGNAL)?))
    }

    /// The last error met while handling events inside the library (asking for buffers after
    /// a configure, say), which `next_event` could not return. Taking it clears it.
    pub fn take_error(&mut self) -> Option<Error> {
        self.last_error.take()
    }

    // ---- windows ----

    /// A normal window with no parent.
    pub fn create(
        &mut self,
        kind: WindowKind,
        title: &str,
        size: (u32, u32),
        flags: WindowFlags,
    ) -> Result<Win, Error> {
        self.create_child(kind, Win(0), title, size, flags, (0, 0), 0)
    }

    /// A window with a parent (a dialog, or a popup placed at `at` in its parent, with the
    /// serial of the input event that asked for it).
    #[allow(clippy::too_many_arguments)] // the interface's own operation takes all of these
    pub fn create_child(
        &mut self,
        kind: WindowKind,
        parent: Win,
        title: &str,
        size: (u32, u32),
        flags: WindowFlags,
        at: (i32, i32),
        serial: u32,
    ) -> Result<Win, Error> {
        let size = Size { width: size.0, height: size.1 };
        let at = Point { x: at.0, y: at.1 };
        let n = self.client.create(
            kind,
            parent.0,
            frames::clip_title(title),
            &size,
            flags,
            &at,
            serial,
        )?;
        self.windows.retain(|w| w.number != n);
        self.windows.push(WindowState {
            number: n,
            configure: None,
            buffers: None,
            frames: Frames::fresh(),
            drawn: false,
            pointer: (0, 0),
        });
        Ok(Win(n))
    }

    /// Destroys a window.
    pub fn destroy(&mut self, w: Win) -> Result<(), Error> {
        self.client.destroy(w.0)?;
        self.windows.retain(|s| s.number != w.0);
        Ok(())
    }

    /// Sets the app's part of the title.
    pub fn set_title(&self, w: Win, title: &str) -> Result<(), Error> {
        Ok(self.client.set_title(w.0, frames::clip_title(title))?)
    }

    /// The smallest and largest size the user may make the window (0 for no limit).
    pub fn set_limits(&self, w: Win, min: (u32, u32), max: (u32, u32)) -> Result<(), Error> {
        let (min, max) =
            (Size { width: min.0, height: min.1 }, Size { width: max.0, height: max.1 });
        Ok(self.client.set_limits(w.0, &min, &max)?)
    }

    /// Chooses the pointer's shape over the window.
    pub fn set_cursor(&self, w: Win, cursor: Cursor) -> Result<(), Error> {
        Ok(self.client.set_cursor(w.0, cursor)?)
    }

    /// Minimises the window.
    pub fn minimise(&self, w: Win) -> Result<(), Error> {
        Ok(self.client.minimise(w.0)?)
    }

    /// Maximises the window, or restores it.
    pub fn maximise(&self, w: Win, on: bool) -> Result<(), Error> {
        Ok(self.client.maximise(w.0, on)?)
    }

    /// Starts the user moving the window; `serial` is of a press in it in the last second.
    pub fn start_move(&self, w: Win, serial: u32) -> Result<(), Error> {
        Ok(self.client.start_move(w.0, serial)?)
    }

    /// Starts the user resizing the window by `edge`.
    pub fn start_resize(&self, w: Win, edge: Edge, serial: u32) -> Result<(), Error> {
        Ok(self.client.start_resize(w.0, edge, serial)?)
    }

    /// Asks for a [`Event::Frame`], a good time to draw again.
    pub fn request_frame(&self, w: Win) -> Result<(), Error> {
        Ok(self.client.frame(w.0)?)
    }

    /// Answers a ping. `next_event` does this itself; this is for the hostile test program.
    pub fn pong(&self, serial: u32) -> Result<(), Error> {
        Ok(self.client.pong(serial)?)
    }

    /// The window's last size (logical pixels) and scale, once it has been configured.
    pub fn size_of(&self, w: Win) -> Option<((u32, u32), u32)> {
        self.window(w.0)?.configure.map(|c| (c.size, c.scale))
    }

    /// Whether the window has buffers to draw in.
    pub fn has_buffers(&self, w: Win) -> bool {
        self.window(w.0).is_some_and(|s| s.buffers.is_some())
    }

    /// Whether the window has been drawn since its last configure.
    pub fn drawn_since_configure(&self, w: Win) -> bool {
        self.window(w.0).is_some_and(|s| s.drawn)
    }

    fn window(&self, n: u32) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.number == n)
    }

    fn window_mut(&mut self, n: u32) -> Option<&mut WindowState> {
        self.windows.iter_mut().find(|w| w.number == n)
    }

    // ---- drawing ----

    /// Draws in the window and commits the picture.
    ///
    /// `f` is given a canvas over the frame the app holds, at the window's scale, so it draws
    /// in logical pixels. It returns the rectangles it changed, in logical pixels; an empty
    /// [`Damage`] means all of the window. Returns `Ok(false)` if the window has no buffers
    /// yet (no configure has come, or asking for buffers failed).
    ///
    /// If both frames are with the display server this waits (up to thirty seconds) for one to
    /// come back, queueing other events for `next_event`.
    pub fn draw(
        &mut self,
        w: Win,
        f: impl FnOnce(&mut Canvas<'_>) -> Damage,
    ) -> Result<bool, Error> {
        if self.window(w.0).is_none() {
            return Err(Error::UnknownWindow);
        }
        if !self.has_buffers(w) {
            return Ok(false);
        }
        let n = self.wait_for_frame(w)?;
        let s = self.window_mut(w.0).ok_or(Error::UnknownWindow)?;
        let (plan_rects, all, nothing) = match s.frames.copy_before_drawing() {
            frames::CopyPlan::Nothing => (Vec::new(), false, true),
            frames::CopyPlan::All => (Vec::new(), true, false),
            frames::CopyPlan::Rects(r) => (r.to_vec(), false, false),
        };
        let size = s.configure.map_or((0, 0), |c| c.size);
        let b = s.buffers.as_mut().ok_or(Error::Broken)?;
        let factor = frames::pixel_scale(b.scale);
        let (width, height, stride) = (b.width, b.height, b.stride);
        let damage = {
            let (dst, src) = b.split(n);
            let plan = if nothing {
                frames::CopyPlan::Nothing
            } else if all {
                frames::CopyPlan::All
            } else {
                frames::CopyPlan::Rects(&plan_rects)
            };
            frames::apply_copy(plan, dst, src, (width, height, stride), factor);
            let mut canvas = Canvas::new(dst, width, height, stride as u32)
                .ok_or(Error::Broken)?
                .with_scale(factor);
            f(&mut canvas)
        };
        let rects = frames::wire_rects(&damage, size);
        self.client.commit(w.0, n as u32, &rects)?;
        if let Some(s) = self.window_mut(w.0) {
            s.frames.committed(n, &frames::plain_rects(&rects));
            s.drawn = true;
        }
        Ok(true)
    }

    /// Waits until the window has a frame to draw in, and says which.
    fn wait_for_frame(&mut self, w: Win) -> Result<usize, Error> {
        let give_up = time::after_ms(RELEASE_MS);
        loop {
            self.drain_ring();
            let s = self.window(w.0).ok_or(Error::UnknownWindow)?;
            if s.buffers.is_none() {
                return Err(Error::Broken);
            }
            if let Some(n) = s.frames.next() {
                return Ok(n);
            }
            if self.gone {
                return Err(Error::Disconnected);
            }
            if time::now() >= give_up {
                return Err(Error::TimedOut);
            }
            self.wait_once(give_up);
        }
    }

    // ---- events ----

    /// Sets timer `id` to give an [`Event::Timer`] at `at` (monotonic nanoseconds, as
    /// `granite::time::now` counts). Setting an id again replaces it.
    pub fn set_timer(&mut self, id: u32, at: u64) {
        self.timers.set(id, at);
    }

    /// Cancels a timer.
    pub fn cancel_timer(&mut self, id: u32) {
        self.timers.cancel(id);
    }

    /// The next event for the app, waiting until `deadline` (monotonic nanoseconds; use
    /// `granite::time::NOW` to only look and `FOREVER` to wait). Returns `None` when the
    /// deadline passes with nothing to report, and for ever after `Disconnected`.
    ///
    /// In order: events already queued, events in the ring, timers that have come, then the
    /// app's own bits. Pings and released frames are dealt with here and never reach the app.
    pub fn next_event(&mut self, deadline: u64) -> Option<Event> {
        loop {
            if let Some(e) = self.queue.pop_front() {
                return Some(e);
            }
            if self.finished {
                return None;
            }
            self.drain_ring();
            if !self.queue.is_empty() {
                continue;
            }
            if self.gone {
                self.finished = true;
                return Some(Event::Disconnected);
            }
            let now = time::now();
            if let Some(id) = self.timers.pop_due(now) {
                return Some(Event::Timer(id));
            }
            if self.app_bits != 0 {
                return Some(Event::Notified(core::mem::take(&mut self.app_bits)));
            }
            if now >= deadline {
                return None;
            }
            let until = self.timers.next().map_or(deadline, |t| t.min(deadline));
            if until > now {
                self.wait_once(until);
            }
        }
    }

    /// Calls `handler` with each event until it returns false, or until after `Disconnected`.
    /// Timers set with [`Connection::set_timer`] wake it on time.
    pub fn run(&mut self, mut handler: impl FnMut(&mut Connection, Event) -> bool) {
        loop {
            let deadline = self.timers.next().unwrap_or(time::FOREVER);
            match self.next_event(deadline) {
                Some(e) => {
                    let last = matches!(e, Event::Disconnected);
                    if !handler(self, e) || last {
                        return;
                    }
                }
                None if self.finished => return,
                None => {}
            }
        }
    }

    /// Waits on the wake-up notification until `until`, and notes what it says.
    fn wait_once(&mut self, until: u64) {
        match self.wake.wait(until) {
            Ok(bits) => {
                if bits & BIT_GONE != 0 {
                    self.gone = true;
                }
                // `BIT_EVENTS` needs nothing here: the ring is always read next.
                self.app_bits |= bits & APP_BITS;
            }
            Err(granite::Error::TimedOut) => {}
            // The notification itself failing is not something an app can recover from.
            Err(_) => self.gone = true,
        }
    }

    /// Reads the ring until it is empty, dealing with each event.
    fn drain_ring(&mut self) {
        let mut entry = [0u8; 32];
        for _ in 0..DRAIN_MAX {
            match self.ring.pop(&mut entry) {
                Ok(_) => {
                    if let Some(incoming) = decode(&entry) {
                        self.handle(incoming);
                    }
                }
                Err(RingError::WouldBlock) => return,
                Err(_) => {
                    self.gone = true;
                    return;
                }
            }
        }
    }

    fn handle(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Ping { serial } => {
                // If this fails the server is gone, and the watch will say so.
                let _ = self.client.pong(serial);
            }
            Incoming::Released { window, frame } => {
                if let Some(s) = self.window_mut(window.0) {
                    s.frames.released(frame);
                }
            }
            Incoming::App(Event::Lost(n)) => {
                // Released events may have been among those lost, and an app waiting for one
                // would wait for ever. Taking every frame as back is the safe guess: at worst
                // the picture tears once.
                for s in &mut self.windows {
                    s.frames.released(0);
                    s.frames.released(1);
                }
                self.queue.push_back(Event::Lost(n));
            }
            Incoming::App(Event::Configure {
                window,
                size,
                scale,
                focused,
                maximised,
                minimised,
                shown,
                serial,
            }) => {
                if self.window(window.0).is_none() {
                    return;
                }
                self.configure(window, size, scale, serial);
                self.queue.push_back(Event::Configure {
                    window,
                    size,
                    scale,
                    focused,
                    maximised,
                    minimised,
                    shown,
                    serial,
                });
            }
            Incoming::App(Event::Pointer(mut p)) => {
                if let Some(s) = self.window_mut(p.window.0) {
                    match p.what {
                        PointerKind::Moved => s.pointer = (p.raw_x, p.raw_y),
                        _ => {
                            p.raw_x = s.pointer.0;
                            p.raw_y = s.pointer.1;
                            p.x = p.raw_x >> 8;
                            p.y = p.raw_y >> 8;
                        }
                    }
                }
                self.queue.push_back(Event::Pointer(p));
            }
            Incoming::App(e) => self.queue.push_back(e),
        }
    }

    /// A configure came: remember it, and get new buffers if the size or scale changed.
    fn configure(&mut self, window: Win, size: (u32, u32), scale: u32, serial: u32) {
        let Some(s) = self.window(window.0) else { return };
        let same = s.buffers.as_ref().is_some_and(|b| b.logical == size && b.scale == scale);
        let result =
            if same {
                None
            } else {
                Some(self.client.buffers(window.0, serial).map_err(Error::from).and_then(|r| {
                    Buffers::new(r.memory, (r.width, r.height, r.stride), size, scale)
                }))
            };
        let Some(s) = self.window_mut(window.0) else { return };
        s.configure = Some(Configure { size, scale });
        s.drawn = false;
        match result {
            None => {}
            Some(Ok(b)) => {
                s.buffers = Some(b);
                s.frames = Frames::fresh();
            }
            Some(Err(e)) => {
                // Drawing at the wrong size would be worse than not drawing.
                s.buffers = None;
                self.last_error = Some(e);
            }
        }
    }

    // ---- the clipboard ----

    /// Puts text on the clipboard; `serial` is of a key press in the last second.
    pub fn copy_text(&self, serial: u32, text: &str) -> Result<(), Error> {
        let len = text.len() as u64;
        if len > wire::MAX_CLIPBOARD {
            return Err(Error::TooBig);
        }
        let memory = Memory::create(len.max(1))?;
        if !text.is_empty() {
            memory.write(0, text.as_bytes())?;
        }
        Ok(self.client.copy(serial, ClipForm::Text, memory, len, 0, 0)?)
    }

    /// Puts a picture of `width` by `height` pixels (`Bgrx`, rows of exactly `width`) on the
    /// clipboard.
    pub fn copy_picture(
        &self,
        serial: u32,
        width: u32,
        height: u32,
        pixels: &[u32],
    ) -> Result<(), Error> {
        let count = u64::from(width) * u64::from(height);
        if count == 0 || count > pixels.len() as u64 {
            return Err(Error::BadArgument);
        }
        let len = count * 4;
        if len > wire::MAX_CLIPBOARD {
            return Err(Error::TooBig);
        }
        let bytes = clip::pixels_to_bytes(&pixels[..count as usize]);
        let memory = Memory::create(len)?;
        memory.write(0, &bytes)?;
        Ok(self.client.copy(serial, ClipForm::Picture, memory, len, width, height)?)
    }

    /// The clipboard's text; `serial` is of a key press in the last second.
    pub fn paste_text(&self, serial: u32) -> Result<String, Error> {
        let r = self.client.paste(serial, ClipForm::Text)?;
        let bytes = read_all(&r.data, r.length)?;
        clip::text_from_bytes(bytes).ok_or(Error::NotText)
    }

    /// The clipboard's picture: width, height and `Bgrx` pixels.
    pub fn paste_picture(&self, serial: u32) -> Result<(u32, u32, Vec<u32>), Error> {
        let r = self.client.paste(serial, ClipForm::Picture)?;
        if r.length != u64::from(r.width) * u64::from(r.height) * 4 {
            return Err(Error::Protocol);
        }
        let bytes = read_all(&r.data, r.length)?;
        Ok((r.width, r.height, clip::bytes_to_pixels(&bytes)))
    }
}

/// Reads `length` bytes of a memory object the display server lent, refusing a length that
/// is too big or more than the object holds.
fn read_all(memory: &Memory, length: u64) -> Result<Vec<u8>, Error> {
    if length > wire::MAX_CLIPBOARD || length > memory.size()? {
        return Err(Error::Protocol);
    }
    let mut bytes = alloc::vec![0u8; length as usize];
    if length > 0 {
        memory.read(0, &mut bytes)?;
    }
    Ok(bytes)
}
