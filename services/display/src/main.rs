// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The display server (docs/design/0022-display-server.md): the one program that draws on
//! the screens in a graphical session. Each app's windows are pictures the app draws in memory
//! this program lends it; this program puts them together, with their title bars, on every
//! screen, and sends each key and click to the one window it is for.
//!
//! The rules are in `lib/compositor`, tested on the host: what each client may do, where input
//! goes, stacking and the focus, the clipboard, and drawing. What is left here is plumbing:
//!
//! - **This thread** (the main one) owns everything: the windows, the screens' buffers, the
//!   clients' rings. It waits for a call, a signal from the other threads, or the time of the
//!   next thing due (a picture, a ping), whichever comes first. Nothing else touches its state,
//!   so it needs no locks, and it never waits for an app: it calls none, and reads no memory an
//!   app made except through the kernel's `memory_read`, which fails cleanly.
//! - **The watcher, the seat thread and the done threads** (`devices.rs`) find the screens and
//!   the seat and pass on what they hear.
//!
//! **Three interfaces on one endpoint.** Who is calling is told by the *badge* on their
//! capability, a number the kernel attaches and the caller can't change (note 0006, section
//! 5): the users service's capability (from the root program's lookup table) serves
//! `pbr.display.sessions`; each session's capability, derived from it, serves
//! `pbr.display.session`; each app's capability, derived from its session's, serves
//! `pbr.window`. The badges of the derived ones are *watched*: when the last copy goes (the app
//! ended, or the sandbox manager took it back), the kernel says so, and the app's windows go.

#![no_std]
#![no_main]

extern crate alloc;

mod clients;
mod devices;
mod screens;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use clients::{Clients, RingTrouble};
use devices::{Message, Shared, Watcher};
use granite::endpoint::{Endpoint, Event as Received};
use granite::interface::OutCap;
use granite::memory::Memory;
use granite::process::role;
use granite::sys::rights;
use granite::{thread, time};
use pbr_compositor::{ClientId, Desk, Out, Place, WindowId};
use pbr_interfaces::pbr_display_session::{self as session_if, SessionError};
use pbr_interfaces::pbr_display_sessions::{self as sessions_if, SessionsError};
use pbr_interfaces::pbr_supervisor as supervisor;
use pbr_interfaces::pbr_window::{self as window_if, ClipForm, WindowError};
use screens::{Displays, REFRESH};

granite::main!(main);

/// The badge of the users service's capability (the root program's lookup table gives it):
/// `pbr.display.sessions`.
const SESSIONS: u64 = 1;
/// The badges of session capabilities: this, with the session's number.
const SESSION: u64 = 1 << 56;
/// The badges of client capabilities: this, with the client's number.
const CLIENT: u64 = 2 << 56;
/// The bits that say which kind of badge.
const KIND: u64 = 0xFF << 56;

/// How long a wait may be while some client is owed a "lost" event: soon, to see if it has
/// made room.
const RETRY: u64 = 20_000_000;
/// How often the displays' drivers are checked on.
const CHECK: u64 = 1_000_000_000;

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("display: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    // With `selftest` or `describe`, the screens are described in the kernel log after each
    // change, for the boot tests (note 0022, section 13).
    let describe = start.args().split_whitespace().any(|w| w == "selftest" || w == "describe");
    // Role `OWN`: the endpoint this service answers on. Role `PARENT`: `pbr.supervisor`, the
    // root program, which finds the device manager and the input service for it.
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let mut parent = supervisor::Client::new(Endpoint(
        start.take(role::PARENT).ok_or("started without a supervisor")?,
    ));
    parent.set_timeout_ms(10_000);
    let shared = Shared::new()?;
    // Bind before anything can signal, so that a signal is never missed.
    endpoint.bind(shared.wake()).map_err(|e| format!("binding a notification: {e}"))?;
    let watcher = Watcher::new(parent, shared.clone())?;
    // (The watcher never ends, so it is never joined.)
    thread::spawn(move || watcher.watch()).map_err(|e| format!("starting a thread: {e}"))?;
    granite::println!("display: running.");
    let mut service = Service {
        desk: Desk::new(),
        clients: Clients::default(),
        displays: Displays::default(),
        shared,
        describe,
        view: 0,
        described: 0,
        checked: 0,
    };
    service.serve(&endpoint)
}

/// Everything the display server keeps. Only the main thread touches it.
struct Service {
    desk: Desk,
    clients: Clients,
    displays: Displays,
    shared: Arc<Shared>,
    describe: bool,
    /// How many descriptions have been written, and the desk's change count at the last.
    view: u64,
    described: u64,
    /// When the displays were last asked whether their drivers are still there.
    checked: u64,
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("display: answer failed: {e}");
    }
}

impl Service {
    fn serve(&mut self, endpoint: &Endpoint) -> Result<(), String> {
        let size = window_if::REQUEST_BUFFER
            .max(sessions_if::REQUEST_BUFFER)
            .max(session_if::REQUEST_BUFFER)
            .max(64);
        let mut buffer = vec![0u8; size];
        let mut next_tick = 0u64;
        loop {
            let now = time::now();
            // Wake for the next picture due, ping due, or frame tick.
            let mut deadline = next_tick;
            if let Some(t) = self.displays.next_draw() {
                deadline = deadline.min(t);
            }
            if self.desk.frames_wanted() {
                deadline = deadline.min(now.saturating_add(REFRESH));
            }
            if self.clients.owe_losses() {
                deadline = deadline.min(now.saturating_add(RETRY));
            }
            deadline = deadline.min(self.checked.saturating_add(CHECK));
            match endpoint.receive(&mut buffer, deadline.max(now)) {
                Ok(Received::Call(msg, reply)) => {
                    self.take_messages();
                    let badge = msg.badge;
                    let unknown = match badge & KIND {
                        0 if badge == SESSIONS => {
                            sessions_if::dispatch(&mut SessionsCalls(self), msg, &buffer, reply)
                        }
                        SESSION => session_if::dispatch(
                            &mut SessionCalls(self, (badge & !KIND) as u32),
                            msg,
                            &buffer,
                            reply,
                        ),
                        CLIENT => window_if::dispatch(
                            &mut WindowCalls(self, (badge & !KIND) as ClientId),
                            msg,
                            &buffer,
                            reply,
                        ),
                        _ => Some((msg, reply)),
                    };
                    if let Some((_, reply)) = unknown {
                        granite::interface::unknown(reply);
                    }
                }
                Ok(Received::Signal(_)) => self.take_messages(),
                Ok(Received::Gone(badge)) => self.gone(badge),
                Err(granite::Error::TimedOut) => {}
                Err(e) => {
                    granite::println!("display: receive failed: {e}");
                    time::sleep_ms(10);
                }
            }
            let now = time::now();
            next_tick = self.desk.tick(now);
            // A driver that crashed while nothing was being drawn is noticed here: its
            // capabilities fail, and the watcher finds the restarted one.
            if now >= self.checked.saturating_add(CHECK) {
                self.checked = now;
                self.lost_displays();
            }
            self.after(now);
        }
    }

    /// What follows anything happening: the screens drawn where they changed, frame events,
    /// events into rings, buffers let go of, and the description written.
    fn after(&mut self, now: u64) {
        if self.desk.has_damage() {
            let d = self.desk.take_damage();
            self.displays.damage(d.as_deref());
        }
        if self.displays.draw(&self.desk, &self.clients, now) {
            self.lost_displays();
        }
        self.displays.unstick(now);
        // Frame events for windows whose screens are up to date.
        self.send_frames(now);
        for out in self.desk.take_out() {
            match out {
                Out::Event(c, e) => self.clients.deliver(c, &e),
                Out::Free { window, generation } => self.clients.free(window, generation),
            }
        }
        self.clients.ring(now);
        if self.describe && self.desk.changes() != self.described {
            self.described = self.desk.changes();
            self.view += 1;
            for line in self.desk.describe(self.view) {
                granite::println!("{line}");
            }
        }
    }

    fn send_frames(&mut self, now: u64) {
        if !self.desk.frames_wanted() {
            return;
        }
        // The busy screens' parts of the space, worked out before the desk is changed.
        let space = self.desk.space().clone();
        let busy: Vec<pbr_draw::Rect> = space
            .screens()
            .iter()
            .filter_map(|s| space.shows(s.number).map(|m| m.rect()))
            .filter(|r| self.displays.busy_over(&self.desk, *r))
            .collect();
        self.desk.send_frames(now, &|r| busy.iter().any(|b| !b.intersect(&r).is_empty()));
    }

    /// Takes what the other threads queued.
    fn take_messages(&mut self) {
        let now = time::now();
        let mut screens_changed = false;
        for m in self.shared.take() {
            match m {
                Message::Display(taken) => {
                    let device = taken.device;
                    match self.displays.add(*taken) {
                        Ok(()) => {
                            granite::println!("display: using display {device}.");
                            screens_changed = true;
                        }
                        Err(_) => self.shared.release(device),
                    }
                }
                Message::Done { device, bits } => {
                    if self.displays.done(device, bits) {
                        screens_changed = true;
                        if !self.displays.outputs().iter().any(|o| o.device == device) {
                            self.shared.release(device);
                        }
                    }
                }
                Message::Seat(events) => {
                    for e in &events {
                        self.desk.seat(e, now);
                    }
                }
                Message::Tablets(t) => {
                    if self.describe {
                        granite::println!("display: {} tablets.", t.len());
                    }
                    self.desk.set_tablets(t);
                }
            }
        }
        if screens_changed {
            let outputs = self.displays.outputs();
            self.desk.set_outputs(&outputs, now);
            self.displays.damage(None);
        }
    }

    /// Displays whose drivers went: let go of them; the watcher finds them again.
    fn lost_displays(&mut self) {
        let gone = self.displays.drop_lost();
        if gone.is_empty() {
            return;
        }
        for device in gone {
            granite::println!("display: display {device} has gone; looking for it again.");
            self.shared.release(device);
        }
        let outputs = self.displays.outputs();
        self.desk.set_outputs(&outputs, time::now());
    }

    /// A watched badge's last capability has gone.
    fn gone(&mut self, badge: u64) {
        let now = time::now();
        match badge & KIND {
            SESSION => self.desk.end_session((badge & !KIND) as u32, now),
            CLIENT => {
                let c = (badge & !KIND) as ClientId;
                self.desk.remove_client(c, now);
                self.clients.remove(c);
            }
            _ => {}
        }
    }
}

// ---- pbr.display.sessions -------------------------------------------------------------

struct SessionsCalls<'a>(&'a mut Service);

impl sessions_if::Server for SessionsCalls<'_> {
    fn start(&mut self, user: String, reply: sessions_if::start::Reply) {
        let Some(n) = self.0.desk.start_session(&user) else {
            return answered(reply.err(SessionsError::TooMany));
        };
        // Derived from the users service's capability, so revoking that takes every session's;
        // watched, so `gone` hears when the session ends. The sandbox manager may copy it (one
        // copy per app it launches, so it can take one app's windows back by revoking that
        // copy) and pass it on.
        let rights = rights::SEND | rights::TRANSFER | rights::DUPLICATE | rights::REVOKE;
        let r = reply.ok(OutCap::derived(SESSION | u64::from(n), rights), n);
        if r.is_err() {
            self.0.desk.end_session(n, time::now());
        }
        answered(r);
    }

    fn activate(&mut self, number: u32, reply: sessions_if::activate::Reply) {
        if self.0.desk.activate(number) {
            answered(reply.ok());
        } else {
            answered(reply.err(SessionsError::BadSession));
        }
    }

    fn farewell(&mut self, restarting: bool, reply: sessions_if::farewell::Reply) {
        self.0.desk.farewell(restarting);
        answered(reply.ok());
    }

    fn screens(&mut self, reply: sessions_if::screens::Reply) {
        let list: Vec<sessions_if::ScreenSummary> = self
            .0
            .desk
            .space()
            .screens()
            .iter()
            .take(16)
            .map(|s| sessions_if::ScreenSummary {
                connector: s.output.connector.clone(),
                width: s.output.width,
                height: s.output.height,
                scale: s.place.scale,
            })
            .collect();
        answered(reply.ok(&list));
    }
}

// ---- pbr.display.session --------------------------------------------------------------

struct SessionCalls<'a>(&'a mut Service, u32);

impl session_if::Server for SessionCalls<'_> {
    fn client(
        &mut self,
        app: u32,
        title: String,
        flags: session_if::ClientFlags,
        launched: u64,
        reply: session_if::client::Reply,
    ) {
        let Some(c) = self.0.desk.add_client(self.1, app, &title, flags.0, launched) else {
            return answered(reply.err(SessionError::TooMany));
        };
        self.0.clients.add(c);
        // Derived from the session's capability (revoked with it) and watched. It can call and
        // be passed on (the sandbox manager gives it to the app), not copied: the display
        // server hears when the app is done with it.
        let r = reply.ok(OutCap::derived(CLIENT | u64::from(c), rights::SEND | rights::TRANSFER));
        if r.is_err() {
            self.0.desk.remove_client(c, time::now());
            self.0.clients.remove(c);
        }
        answered(r);
    }
}

// ---- pbr.window -----------------------------------------------------------------------

struct WindowCalls<'a>(&'a mut Service, ClientId);

impl window_if::Server for WindowCalls<'_> {
    fn events(
        &mut self,
        doorbell: granite::notification::Notification,
        reply: window_if::events::Reply,
    ) {
        match self.0.clients.events(self.1, doorbell) {
            Ok(ring) => answered(reply.ok(ring)),
            Err(RingTrouble::Twice) => answered(reply.err(WindowError::BadArgument)),
            Err(RingTrouble::NoMemory) => answered(reply.err(WindowError::NoMemory)),
        }
    }

    fn create(
        &mut self,
        kind: window_if::WindowKind,
        parent: u32,
        title: String,
        size: window_if::Size,
        flags: window_if::WindowFlags,
        at: window_if::Point,
        serial: u32,
        reply: window_if::create::Reply,
    ) {
        let now = time::now();
        let r = self.0.desk.create(
            self.1,
            kind,
            parent,
            &title,
            (size.width, size.height),
            flags,
            (at.x, at.y),
            serial,
            now,
        );
        answered(match r {
            Ok(w) => reply.ok(w),
            Err(e) => reply.err(e),
        });
    }

    fn destroy(&mut self, window: u32, reply: window_if::destroy::Reply) {
        let r = self.0.desk.destroy(self.1, window, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn set_title(&mut self, window: u32, title: String, reply: window_if::set_title::Reply) {
        let r = self.0.desk.set_title(self.1, window, &title);
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn set_limits(
        &mut self,
        window: u32,
        min: window_if::Size,
        max: window_if::Size,
        reply: window_if::set_limits::Reply,
    ) {
        let r = self.0.desk.set_limits(
            self.1,
            window,
            (min.width, min.height),
            (max.width, max.height),
        );
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn set_cursor(
        &mut self,
        window: u32,
        cursor: window_if::Cursor,
        reply: window_if::set_cursor::Reply,
    ) {
        let r = self.0.desk.set_cursor(self.1, window, cursor);
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn buffers(&mut self, window: u32, serial: u32, reply: window_if::buffers::Reply) {
        let made = match self.0.desk.buffers(self.1, window, serial) {
            Ok(m) => m,
            Err(e) => return answered(reply.err(e)),
        };
        let id = WindowId { client: self.1, number: window };
        let b = made.buffers;
        match self.0.clients.buffers(id, b, made.reused) {
            Ok(memory) => answered(reply.ok(memory, b.width, b.height, b.stride)),
            Err(_) => {
                // The display server's own memory ran out (its budget is the limit on all
                // sessions' windows together).
                if !made.reused {
                    self.0.desk.buffers_failed(self.1, window, b.generation);
                }
                answered(reply.err(WindowError::NoMemory));
            }
        }
    }

    fn commit(
        &mut self,
        window: u32,
        frame: u32,
        changed: Vec<window_if::Rect>,
        reply: window_if::commit::Reply,
    ) {
        let rects: Vec<(i32, i32, u32, u32)> =
            changed.iter().map(|r| (r.x, r.y, r.width, r.height)).collect();
        let r = self.0.desk.commit(self.1, window, frame, &rects, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn frame(&mut self, window: u32, reply: window_if::frame::Reply) {
        let r = self.0.desk.want_frame(self.1, window);
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn pong(&mut self, serial: u32, reply: window_if::pong::Reply) {
        let r = self.0.desk.pong(self.1, serial, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn start_move(&mut self, window: u32, serial: u32, reply: window_if::start_move::Reply) {
        let r = self.0.desk.start_move(self.1, window, serial, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn start_resize(
        &mut self,
        window: u32,
        edge: window_if::Edge,
        serial: u32,
        reply: window_if::start_resize::Reply,
    ) {
        let r = self.0.desk.start_resize(self.1, window, u32::from(edge), serial, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn minimise(&mut self, window: u32, reply: window_if::minimise::Reply) {
        let r = self.0.desk.minimise(self.1, window, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn maximise(&mut self, window: u32, on: bool, reply: window_if::maximise::Reply) {
        let r = self.0.desk.maximise(self.1, window, on, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn copy(
        &mut self,
        serial: u32,
        form: ClipForm,
        data: Memory,
        length: u64,
        width: u32,
        height: u32,
        reply: window_if::copy::Reply,
    ) {
        let now = time::now();
        let picture = form == ClipForm::Picture;
        if let Err(e) =
            self.0.desk.check_copy(self.1, serial, picture, length, (width, height), now)
        {
            return answered(reply.err(e));
        }
        // Copied with the kernel's `memory_read`, never by mapping the app's memory: if the
        // app revokes it meanwhile the read fails, and nothing here faults.
        let mut bytes = vec![0u8; length as usize];
        for (i, chunk) in bytes.chunks_mut(65536).enumerate() {
            if data.read((i * 65536) as u64, chunk).is_err() {
                return answered(reply.err(WindowError::BadArgument));
            }
        }
        let size = picture.then_some((width, height));
        let r = self.0.desk.copied(self.1, serial, size, bytes, now);
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn paste(&mut self, serial: u32, form: ClipForm, reply: window_if::paste::Reply) {
        let now = time::now();
        let (data, w, h) = match self.0.desk.paste(self.1, serial, form == ClipForm::Picture, now) {
            Ok((d, w, h)) => (Vec::from(d), w, h),
            Err(e) => return answered(reply.err(e)),
        };
        // A new memory object with a copy, which the app may only read.
        let made = || -> granite::Result<Memory> {
            let m = Memory::create((data.len() as u64).max(1))?;
            m.write(0, &data)?;
            Ok(Memory(m.0.duplicate(rights::READ | rights::MAP | rights::TRANSFER)?))
        };
        answered(match made() {
            Ok(m) => reply.ok(m, data.len() as u64, w, h),
            Err(_) => reply.err(WindowError::NoMemory),
        });
    }

    fn screens(&mut self, reply: window_if::screens::Reply) {
        let list = match self.0.desk.screens_for(self.1) {
            Ok(l) => l,
            Err(e) => return answered(reply.err(e)),
        };
        let places: Vec<window_if::ScreenPlace> = list
            .iter()
            .take(16)
            .map(|(s, (w, h))| window_if::ScreenPlace {
                screen: s.number,
                identity: s.output.key(),
                x: s.place.x,
                y: s.place.y,
                scale: s.place.scale,
                rotation: s.place.rotation,
                mirrors: s.place.mirrors,
                main: s.place.main,
                size: window_if::Size { width: *w as u32, height: *h as u32 },
            })
            .collect();
        answered(reply.ok(&places));
    }

    fn arrange(&mut self, screens: Vec<window_if::ScreenPlace>, reply: window_if::arrange::Reply) {
        let places: Vec<(u32, Place)> = screens
            .iter()
            .map(|p| {
                (
                    p.screen,
                    Place {
                        x: p.x,
                        y: p.y,
                        scale: p.scale,
                        rotation: p.rotation,
                        mirrors: p.mirrors,
                        main: p.main,
                    },
                )
            })
            .collect();
        let r = self.0.desk.arrange(self.1, &places, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn windows(&mut self, reply: window_if::windows::Reply) {
        let list = match self.0.desk.window_list(self.1) {
            Ok(l) => l,
            Err(e) => return answered(reply.err(e)),
        };
        let infos: Vec<window_if::WindowInfo> = list
            .into_iter()
            .map(|w| window_if::WindowInfo {
                window: w.window,
                app: w.app,
                title: w.title,
                minimised: w.minimised,
                focused: w.focused,
                attention: w.attention,
                not_responding: w.not_responding,
            })
            .collect();
        answered(reply.ok(&infos));
    }

    fn act(&mut self, window: u64, action: u32, reply: window_if::act::Reply) {
        let r = self.0.desk.act(self.1, window, action, time::now());
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn pointer(
        &mut self,
        speed: u32,
        tablets: Vec<window_if::TabletTie>,
        reply: window_if::pointer::Reply,
    ) {
        let ties = tablets.into_iter().map(|t| (t.name, t.screen)).collect();
        let r = self.0.desk.set_pointer(self.1, speed, ties);
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn check_serial(
        &mut self,
        app: u32,
        window: u32,
        serial: u32,
        reply: window_if::check_serial::Reply,
    ) {
        let r = self.0.desk.check_serial(self.1, app, window, serial, time::now());
        answered(match r {
            Ok(b) => reply.ok(b),
            Err(e) => reply.err(e),
        });
    }

    fn create_over(
        &mut self,
        app: u32,
        window: u32,
        title: String,
        size: window_if::Size,
        flags: window_if::WindowFlags,
        reply: window_if::create_over::Reply,
    ) {
        let r = self.0.desk.create_over(
            self.1,
            app,
            window,
            &title,
            (size.width, size.height),
            flags,
            time::now(),
        );
        answered(match r {
            Ok(w) => reply.ok(w),
            Err(e) => reply.err(e),
        });
    }
}
