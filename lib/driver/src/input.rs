// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What an input driver shares: the `pbr.input` service. The driver makes events and calls
//! [`Input::post`]; clients open the device and read them from a ring (note 0011, section 8).
//!
//! One client has the device at a time. Opening again takes it over: the ring and doorbell the
//! previous client held are revoked, so a client that crashed can never lock everyone out.
//! If the client falls behind and the ring fills, new events are dropped, counted, and the
//! next event that fits is a "lost" event carrying the count.

use alloc::string::String;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::ring::{self, Producer, RingError};
use granite::sys::{perm, rights};
use granite::time;
use pbr_interfaces::pbr_input::{self as input, InputError, InputKind};

/// Bytes in the ring: 16-byte entries, 512 of them.
const RING_BYTES: u64 = 16384;
/// Bytes in one event.
pub const EVENT_SIZE: usize = 16;

/// The `pbr.input` service for one device.
pub struct Input {
    kind: InputKind,
    name: String,
    /// Where the driver's own mapping hangs from. (Revoking a capability takes mappings made
    /// through it, so the copies clients get hang from the pair below instead.)
    _memory: Memory,
    mapping: Mapping,
    doorbell: Notification,
    /// The copies clients get are made from these, so that revoking them takes back exactly
    /// those and leaves the driver's own mapping and doorbell alone.
    shared_memory: Memory,
    shared_bell: Notification,
    producer: Option<Producer>,
    lost: u32,
    /// Which keys are down, by code, so that a second "pressed" is reported as a repeat.
    down: [u64; 2],
}

impl Input {
    /// Makes the service for a device of `kind` called `name` (at most 64 bytes).
    pub fn new(kind: InputKind, name: &str) -> Result<Input, &'static str> {
        let memory = Memory::create(RING_BYTES).map_err(|_| "can't make the ring")?;
        let mapping = memory.map(perm::READ_WRITE).map_err(|_| "can't map the ring")?;
        let doorbell = Notification::create().map_err(|_| "can't make the doorbell")?;
        let copy =
            |h: &granite::Handle| h.duplicate(rights::ALL).map_err(|_| "can't copy a handle");
        let shared_memory = Memory(copy(&memory.0)?);
        let shared_bell = Notification(copy(&doorbell.0)?);
        let mut name = String::from(name);
        name.truncate(64);
        while !name.is_char_boundary(name.len()) {
            name.pop();
        }
        Ok(Input {
            kind,
            name,
            _memory: memory,
            mapping,
            doorbell,
            shared_memory,
            shared_bell,
            producer: None,
            lost: 0,
            down: [0; 2],
        })
    }

    /// A key changed: `pressed` or released. A key already down that is pressed again is
    /// reported as a repeat.
    pub fn key(&mut self, code: u16, pressed: bool) {
        let (word, bit) = (usize::from(code / 64), 1u64 << (code % 64));
        let Some(w) = self.down.get_mut(word) else { return };
        let value = match (pressed, *w & bit != 0) {
            (true, false) => input::VALUE_PRESSED,
            (true, true) => input::VALUE_REPEAT,
            (false, true) => input::VALUE_RELEASED,
            (false, false) => return, // a release for a key that wasn't down
        };
        if pressed {
            *w |= bit;
        } else {
            *w &= !bit;
        }
        self.post(input::EVENT_KEY as u16, code, value as i32);
    }

    /// The pointer moved by (`dx`, `dy`) pixels, down being positive.
    pub fn motion(&mut self, dx: i32, dy: i32) {
        if dx != 0 {
            self.post(input::EVENT_MOVE as u16, input::AXIS_X as u16, dx);
        }
        if dy != 0 {
            self.post(input::EVENT_MOVE as u16, input::AXIS_Y as u16, dy);
        }
    }

    /// A mouse button (`input::BUTTON_*`) went down or up.
    pub fn button(&mut self, code: u32, pressed: bool) {
        let value = if pressed { input::VALUE_PRESSED } else { input::VALUE_RELEASED };
        self.post(input::EVENT_BUTTON as u16, code as u16, value as i32);
    }

    /// The wheel turned by `amount` notches (positive away from the user).
    pub fn wheel(&mut self, amount: i32) {
        if amount != 0 {
            self.post(input::EVENT_SCROLL as u16, input::AXIS_WHEEL as u16, amount);
        }
    }

    /// Adds an event to the ring, if a client has the device open, and rings the doorbell.
    pub fn post(&mut self, kind: u16, code: u16, value: i32) {
        let Some(p) = &mut self.producer else { return };
        let now = time::now();
        let mut ok = true;
        // Report what was lost first, if there is room for it and for this event.
        if self.lost > 0 && p.free().is_ok_and(|f| f >= 2) {
            ok &= push(p, 0, 0, self.lost as i32, now);
            self.lost = 0;
        }
        if p.free().is_ok_and(|f| f > 0) {
            ok &= push(p, kind, code, value, now);
        } else {
            self.lost = self.lost.saturating_add(1);
        }
        if !ok {
            // A ring the client has wrecked is no use to anyone: stop writing to it.
            self.producer = None;
        }
        let _ = self.doorbell.signal(1);
    }
}

fn push(p: &mut Producer, kind: u16, code: u16, value: i32, time: u64) -> bool {
    let mut e = [0u8; EVENT_SIZE];
    e[0..2].copy_from_slice(&kind.to_le_bytes());
    e[2..4].copy_from_slice(&code.to_le_bytes());
    e[4..8].copy_from_slice(&value.to_le_bytes());
    e[8..16].copy_from_slice(&time.to_le_bytes());
    !matches!(p.push(&e), Err(RingError::Broken | RingError::Invalid))
}

fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("input: answer failed: {e}");
    }
}

impl input::Server for Input {
    fn info(&mut self, reply: input::info::Reply) {
        answered(reply.ok(self.kind, &self.name));
    }

    fn open(&mut self, reply: input::open::Reply) {
        // Take the device back from whoever had it.
        self.producer = None;
        let _ = self.shared_memory.0.revoke();
        let _ = self.shared_bell.0.revoke();
        // SAFETY: the mapping is `RING_BYTES` long, writable, page aligned, and only the ring
        // lives in it; nobody else holds the memory now that it has been revoked.
        let started = unsafe {
            ring::init(
                self.mapping.as_mut_ptr(),
                RING_BYTES as usize,
                ring::Kind::Entries,
                EVENT_SIZE,
            )
            .and_then(|_| Producer::new(self.mapping.as_mut_ptr(), RING_BYTES as usize))
        };
        let Ok(producer) = started else {
            answered(reply.err(InputError::NoRing));
            return;
        };
        let given = (
            self.shared_memory
                .0
                .duplicate(rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER),
            self.shared_bell.0.duplicate(rights::WAIT | rights::TRANSFER),
        );
        let (Ok(memory), Ok(bell)) = given else {
            answered(reply.err(InputError::NoRing));
            return;
        };
        self.producer = Some(producer);
        self.lost = 0;
        answered(reply.ok(Memory(memory), Notification(bell)));
    }
}
