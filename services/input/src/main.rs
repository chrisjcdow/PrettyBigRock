// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The input service (docs/design/0021-input-and-keyboard-layouts.md): the one program that
//! reads the keyboards and pointers. It merges them into one stream of events with the user's
//! keyboard layout applied, and serves that as `pbr.seat` to one consumer at a time: the
//! display server, or the console in text mode.
//!
//! Why one place owns them: an app that could open a keyboard itself could read what is
//! typed into another app's window, passwords included. With every device taken by this
//! service, and the seat given only to the consumer the root program names, what is typed
//! reaches an app only if the display server sends it there.
//!
//! The rules are in `lib/seat` (the merge, modifiers, repeats, a full ring) and `lib/keymap`
//! (layouts), both tested on the host. What is left here is plumbing:
//!
//! - **This thread** (the main one) owns the seat. It waits for a call on the endpoint, a
//!   signal that the other threads have queued something, or the time of the next key repeat,
//!   whichever comes first, and puts the events that result in the consumer's ring. Nothing
//!   else touches the seat, so it needs no locks.
//! - **The watcher and the readers** (`devices.rs`) find the devices and read their rings.
//!
//! **Who may do what** is decided by the *badge* on the caller's capability: a number the
//! kernel attaches to a capability when it is made and shows the service with every call,
//! which the caller can't change (note 0006, section 5). The root program makes the
//! capabilities (it answers `pbr.supervisor.lookup("input")`), so it decides who is a consumer
//! and who may only set the layout; this service only checks the number.

#![no_std]
#![no_main]

extern crate alloc;

mod devices;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;
use devices::{Devices, Message, Shared};
use granite::endpoint::{Endpoint, Event as Received};
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::process::role;
use granite::ring::{self, Producer};
use granite::sys::{perm, rights};
use granite::{thread, time};
use pbr_archive::Archive;
use pbr_interfaces::pbr_seat::{self as seat_if, SeatError};
use pbr_interfaces::pbr_supervisor as supervisor;
use pbr_keymap::Layout;
use pbr_seat::event::EVENT_SIZE;
use pbr_seat::{DeviceKind, Event, Outbox, Seat, Sink};

granite::main!(main);

/// The badge of a consumer's capability: the display server's, or the console's. (The root
/// program's lookup table uses the same numbers: `services/root/src/supervisor.rs`.)
const CONSUMER: u64 = 1;
/// The badge of the users service's capability, which may only ask about layouts and set one.
const LAYOUT_SETTER: u64 = 2;

/// Bytes in the consumer's ring: room for about a thousand 32-byte events.
const RING_BYTES: u64 = 32768;
/// While the consumer's ring is full, how often to see whether it has made room.
const RETRY_MS: u64 = 20;
/// The layout used until someone sets another, and when a setting names one that isn't there
/// (note 0021, section 5).
const DEFAULT_LAYOUT: &str = "us";
/// Where layouts are in the start-up archive, and how their files end.
const LAYOUT_FOLDER: &str = "layouts/";
const LAYOUT_ENDING: &str = ".kbd";
/// The most layouts offered (`pbr.seat.layouts` answers with at most this many).
const MAX_LAYOUTS: usize = 32;
/// The US layout, built in: the service can type even if the archive has no layout that can
/// be read. (`include_str!` puts the file's text in the program when it is built.)
const BUILT_IN_US: &str = include_str!("../layouts/us.kbd");

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("input: {e}");
            1
        }
    }
}

/// A layout that was read, under its name.
struct Named {
    name: String,
    layout: Layout,
}

/// Reads every `layouts/NAME.kbd` in the start-up archive. One that can't be read isn't
/// offered, and the log says why (note 0021, section 4). `us` is always there: the archive's
/// if it is good, the built-in one if not.
fn load_layouts(archive: Option<&Archive<'_>>) -> Result<Vec<Named>, String> {
    let mut found: Vec<Named> = Vec::new();
    for entry in archive.into_iter().flat_map(|a| a.entries()) {
        let Some(name) =
            entry.name.strip_prefix(LAYOUT_FOLDER).and_then(|n| n.strip_suffix(LAYOUT_ENDING))
        else {
            continue;
        };
        // A layout's name is what settings files hold: short, and plain enough to be a word
        // in one.
        let plain = !name.is_empty()
            && name.len() <= 32
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !plain {
            granite::println!(
                "input: the layout file {} has a name that can't be used.",
                entry.name
            );
            continue;
        }
        if found.len() >= MAX_LAYOUTS {
            granite::println!("input: too many layouts; {name} is left out.");
            continue;
        }
        match pbr_keymap::parse(entry.data) {
            Ok(layout) => found.push(Named { name: String::from(name), layout }),
            Err(e) => granite::println!("input: the layout {name} can't be used: {e}"),
        }
    }
    if !found.iter().any(|l| l.name == DEFAULT_LAYOUT) {
        let layout = pbr_keymap::parse(BUILT_IN_US.as_bytes())
            .map_err(|e| format!("the built-in US layout can't be read: {e}"))?;
        found.insert(0, Named { name: String::from(DEFAULT_LAYOUT), layout });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    // Role `OWN`: the endpoint this service answers on. Role `PARENT`: `pbr.supervisor`, the
    // root program, which finds the device manager for this service (note 0018, section 2).
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let mut parent = supervisor::Client::new(Endpoint(
        start.take(role::PARENT).ok_or("started without a supervisor")?,
    ));
    parent.set_timeout_ms(10_000);

    // The layouts are read once, at the start. The archive is mapped only for that: what is
    // kept is each layout's table, not its text.
    let layouts = {
        let memory = start.take(role::ARCHIVE).map(Memory);
        let mapping = match &memory {
            Some(m) => Some(m.map(perm::READ).map_err(|e| format!("can't map the archive: {e}"))?),
            None => None,
        };
        // SAFETY: the mapping is readable for its whole length, nothing writes to it, and it
        // outlives `bytes` and the archive made from them (all three end with this block).
        let bytes = mapping
            .as_ref()
            .map(|m| unsafe { core::slice::from_raw_parts(m.as_ptr(), m.len() as usize) });
        let archive = match bytes.map(Archive::parse) {
            Some(Ok(a)) => Some(a),
            Some(Err(e)) => {
                granite::println!(
                    "input: the start-up archive is damaged ({e}); only the US layout."
                );
                None
            }
            None => None,
        };
        load_layouts(archive.as_ref())?
    };
    let current = layouts
        .iter()
        .position(|l| l.name == DEFAULT_LAYOUT)
        .ok_or("the default layout has gone missing")?;
    let names: Vec<&str> = layouts.iter().map(|l| l.name.as_str()).collect();
    granite::println!("input: layouts: {}.", names.join(", "));

    // Bind before anything can signal, so that a signal is never missed.
    let shared = Shared::new()?;
    endpoint.bind(shared.wake()).map_err(|e| format!("binding a notification: {e}"))?;
    let for_watcher = shared.clone();
    // (The watcher never ends, so it is never joined.)
    let _watcher = thread::spawn(move || Devices::new(parent, for_watcher).watch())
        .map_err(|e| format!("starting a thread: {e}"))?;

    let mut service = Service {
        seat: Seat::new(layouts[current].layout.clone()),
        layouts,
        current,
        shared,
        sources: Vec::new(),
        consumer: None,
        outbox: Outbox::new(),
    };
    service.serve(&endpoint)
}

/// The consumer's ring and doorbell, as this service holds them.
///
/// The memory is made once and used for every consumer in turn. The copies a consumer gets
/// are made from `shared_memory` and `shared_bell`, so that revoking those two takes back
/// exactly what the previous consumer was given and leaves this service's own mapping and
/// doorbell working. (Revoking a capability takes away every copy made from it, and every
/// mapping made through those copies: that is what stops the previous consumer's ring.)
struct Channel {
    _memory: Memory,
    mapping: Mapping,
    doorbell: Notification,
    shared_memory: Memory,
    shared_bell: Notification,
}

impl Channel {
    fn new() -> Result<Channel, granite::Error> {
        let memory = Memory::create(RING_BYTES)?;
        let mapping = memory.map(perm::READ_WRITE)?;
        let doorbell = Notification::create()?;
        let shared_memory = Memory(memory.0.duplicate(rights::ALL)?);
        let shared_bell = Notification(doorbell.0.duplicate(rights::ALL)?);
        Ok(Channel { _memory: memory, mapping, doorbell, shared_memory, shared_bell })
    }
}

/// The ring's writing side, as `lib/seat`'s outbox sees it.
struct RingSink<'a> {
    producer: &'a mut Producer,
    /// Set when the ring turns out to be broken. (A `Cell` lets a value be changed through a
    /// shared reference, which `free`, a question, is given.)
    broken: Cell<bool>,
}

impl Sink for RingSink<'_> {
    fn free(&self) -> usize {
        match self.producer.free() {
            Ok(n) => n as usize,
            Err(_) => {
                self.broken.set(true);
                0
            }
        }
    }

    fn push(&mut self, event: &[u8; EVENT_SIZE]) -> bool {
        match self.producer.push(event) {
            Ok(_) => true,
            // `free` said there was room, so any failure is a consumer changing the ring's
            // bookkeeping under us: nothing more is written to it.
            Err(_) => {
                self.broken.set(true);
                false
            }
        }
    }
}

/// Everything the service keeps. Only the main thread touches it.
struct Service {
    seat: Seat,
    layouts: Vec<Named>,
    /// Which of `layouts` is in use.
    current: usize,
    shared: Arc<Shared>,
    /// The open devices: (`devices.rs`'s key, the seat's number for the source).
    sources: Vec<(u32, u32)>,
    /// The ring and its writing side, while there is a consumer.
    consumer: Option<(Channel, Producer)>,
    outbox: Outbox,
}

impl Service {
    /// Answers calls, takes what the readers queued and makes key repeats, for ever.
    fn serve(&mut self, endpoint: &Endpoint) -> Result<(), String> {
        let mut buffer = vec![0u8; seat_if::REQUEST_BUFFER];
        loop {
            // Wake for the next repeat; and, while the consumer's ring is full, soon, to see
            // whether there is room to say what was missed.
            let mut deadline = self.seat.next_repeat().unwrap_or(time::FOREVER);
            if self.outbox.lost() > 0 && self.consumer.is_some() {
                deadline = deadline.min(time::after_ms(RETRY_MS));
            }
            let mut events = Vec::new();
            match endpoint.receive(&mut buffer, deadline) {
                Ok(Received::Call(msg, reply)) => {
                    // What was queued before this call happened before it.
                    self.take_messages(&mut events);
                    self.deliver(&events);
                    events.clear();
                    let unknown = seat_if::dispatch(&mut Calls(self), msg, &buffer, reply);
                    if let Some((_, reply)) = unknown {
                        granite::interface::unknown(reply);
                    }
                }
                Ok(Received::Signal(_)) => self.take_messages(&mut events),
                Ok(Received::Gone(_)) | Err(granite::Error::TimedOut) => {}
                Err(e) => {
                    granite::println!("input: receive failed: {e}");
                    time::sleep_ms(10);
                }
            }
            self.seat.tick(time::now(), &mut events);
            self.deliver(&events);
        }
    }

    /// Handles what the watcher and the readers queued; the events that result go in `out`.
    fn take_messages(&mut self, out: &mut Vec<Event>) {
        for message in self.shared.take() {
            match message {
                Message::Added { key, source, name, device } => {
                    match self.seat.add(source, &name, device, time::now(), out) {
                        Some(number) => {
                            self.sources.push((key, number));
                            granite::println!("input: using {name}.");
                        }
                        None => granite::println!("input: too many devices; {name} is left out."),
                    }
                }
                Message::Raw { key, event } => {
                    // (Events of a device the seat no longer has are dropped here.)
                    if let Some(&(_, number)) = self.sources.iter().find(|s| s.0 == key) {
                        self.seat.raw(number, &event, out);
                    }
                }
                Message::Gone { key } => {
                    if let Some(i) = self.sources.iter().position(|s| s.0 == key) {
                        let (_, number) = self.sources.swap_remove(i);
                        self.seat.remove(number, time::now(), out);
                    }
                }
            }
        }
    }

    /// Puts events in the consumer's ring and rings its doorbell. Without a consumer they
    /// go nowhere: nobody is listening, and the next consumer starts from a reset.
    fn deliver(&mut self, events: &[Event]) {
        let Some((channel, producer)) = &mut self.consumer else { return };
        let mut sink = RingSink { producer, broken: Cell::new(false) };
        let now = time::now();
        let wrote = if events.is_empty() {
            self.outbox.retry(&mut sink, &self.seat, now)
        } else {
            self.outbox.deliver(&mut sink, &self.seat, events, now)
        };
        let broken = sink.broken.get();
        match wrote {
            Ok(true) => {
                let _ = channel.doorbell.signal(1);
            }
            Ok(false) => {}
            Err(_) => {}
        }
        if broken || wrote.is_err() {
            // A ring its consumer has wrecked is no use to anyone. The consumer can take
            // the seat again for a new one.
            granite::println!("input: the consumer's ring is broken; it is dropped.");
            self.consumer = None;
        }
    }

    /// `pbr.seat.take`: gives the caller a new ring and doorbell, taking the seat from
    /// whoever had it.
    fn take(&mut self) -> Result<(Memory, Notification), SeatError> {
        // Keep the memory from the consumer before, or make it the first time.
        let channel = match self.consumer.take() {
            Some((channel, _)) => channel,
            None => Channel::new().map_err(|_| SeatError::NoRing)?,
        };
        // Take back what the previous consumer was given: its mapping of the ring and its
        // doorbell stop working now, before anything new is written.
        let _ = channel.shared_memory.0.revoke();
        let _ = channel.shared_bell.0.revoke();
        // SAFETY: the mapping is `RING_BYTES` long, writable, page aligned, and only the ring
        // lives in it; nobody else holds the memory now that it has been revoked. It is kept
        // beside the producer (in `self.consumer`), so it outlives it.
        let producer = unsafe {
            ring::init(
                channel.mapping.as_mut_ptr(),
                RING_BYTES as usize,
                ring::Kind::Entries,
                EVENT_SIZE,
            )
            .and_then(|_| Producer::new(channel.mapping.as_mut_ptr(), RING_BYTES as usize))
        }
        .map_err(|_| SeatError::NoRing)?;
        // The consumer may read and write the ring's memory (reading moves the ring's "read
        // up to here" mark, which is in the memory) and wait on the doorbell, and pass
        // neither on in a way that outlives the next `take`: copies of copies are revoked
        // with them.
        let memory = channel
            .shared_memory
            .0
            .duplicate(rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER)
            .map_err(|_| SeatError::NoRing)?;
        let bell = channel
            .shared_bell
            .0
            .duplicate(rights::WAIT | rights::TRANSFER)
            .map_err(|_| SeatError::NoRing)?;
        self.consumer = Some((channel, producer));
        // The first thing in a new ring: a reset, and what is held right now.
        if let Some((channel, producer)) = &mut self.consumer {
            let mut sink = RingSink { producer, broken: Cell::new(false) };
            if self.outbox.start(&mut sink, &self.seat, time::now()).is_err() || sink.broken.get() {
                self.consumer = None;
                return Err(SeatError::NoRing);
            }
            let _ = channel.doorbell.signal(1);
        }
        Ok((Memory(memory), Notification(bell)))
    }
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("input: answer failed: {e}");
    }
}

/// `pbr.seat`, answering one call. (`'a` is a *lifetime*: this borrows the service for as long
/// as it exists, so the compiler can check nothing else uses it meanwhile.)
struct Calls<'a>(&'a mut Service);

impl seat_if::Server for Calls<'_> {
    fn take(&mut self, reply: seat_if::take::Reply) {
        // Only a consumer. In particular not the users service, whose capability exists to
        // set the layout: if it could take the seat it could read everything typed.
        if reply.badge() != CONSUMER {
            answered(reply.err(SeatError::NotAllowed));
            return;
        }
        answered(match self.0.take() {
            Ok((ring, doorbell)) => reply.ok(ring, doorbell),
            Err(e) => reply.err(e),
        });
    }

    fn devices(&mut self, reply: seat_if::devices::Reply) {
        if reply.badge() != CONSUMER {
            answered(reply.err(SeatError::NotAllowed));
            return;
        }
        let list: Vec<seat_if::Device> = self
            .0
            .seat
            .devices()
            .map(|d| seat_if::Device {
                id: d.id,
                kind: match d.kind {
                    DeviceKind::Keyboard => seat_if::DeviceKind::Keyboard,
                    DeviceKind::Mouse => seat_if::DeviceKind::Mouse,
                    DeviceKind::Tablet => seat_if::DeviceKind::Tablet,
                },
                name: d.name.clone(),
                device: d.device,
            })
            .collect();
        answered(reply.ok(&list));
    }

    fn layouts(&mut self, reply: seat_if::layouts::Reply) {
        if !matches!(reply.badge(), CONSUMER | LAYOUT_SETTER) {
            answered(reply.err(SeatError::NotAllowed));
            return;
        }
        let list: Vec<seat_if::Layout> = self
            .0
            .layouts
            .iter()
            .map(|l| seat_if::Layout {
                name: l.name.clone(),
                title: String::from(l.layout.title()),
            })
            .collect();
        answered(reply.ok(&self.0.layouts[self.0.current].name, &list));
    }

    fn set_layout(&mut self, name: String, reply: seat_if::set_layout::Reply) {
        // Only the users service, which knows whose session it is and what their settings
        // say (note 0021, section 5). A consumer may not: an app talking the display server
        // into it must not be able to change what the user's keys type.
        if reply.badge() != LAYOUT_SETTER {
            answered(reply.err(SeatError::NotAllowed));
            return;
        }
        let Some(i) = self.0.layouts.iter().position(|l| l.name == name) else {
            answered(reply.err(SeatError::NoSuchLayout));
            return;
        };
        if i != self.0.current {
            self.0.current = i;
            // Between two events: the main thread is here, not in the middle of one.
            self.0.seat.set_layout(self.0.layouts[i].layout.clone());
            granite::println!("input: the layout is now {name}.");
        }
        answered(reply.ok());
    }
}
