// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The seat: every keyboard and pointer merged into one stream of events
//! (docs/design/0021-input-and-keyboard-layouts.md, sections 1 and 2).
//!
//! A driver says *which key* went down or up, by where it is on the keyboard. The seat turns
//! that into what `pbr.seat` promises: the characters the key types on the user's layout
//! (`lib/keymap`), the modifiers held, key repeats made here so that every keyboard repeats the
//! same way, and which device each event came from.
//!
//! It is plain computation, so that it can be tested on the host: the input service
//! (`services/input`) feeds it the drivers' 16-byte events ([`Seat::raw`]) and the time
//! ([`Seat::tick`]), and puts the [`Event`]s it makes in the consumer's ring. Nothing here
//! trusts a driver: any 16 bytes are some event or are skipped, and nothing panics.
//!
//! **What is kept per device and what for the whole seat.** Which keys and buttons are down,
//! and so which of Shift, Ctrl, Alt and Meta apply, is each device's own: Shift held on one
//! keyboard doesn't change what another types. Caps Lock and Num Lock belong to the seat: they
//! are states, not held keys, and a person with two keyboards has one of each. A dead key
//! waiting for its letter is the seat's too.

use crate::event::{Event, kind, mods, value};
use alloc::string::String;
use alloc::vec::Vec;
use pbr_devices::keys::key;
use pbr_keymap::{Composer, Layout, Mods, Typed, keypad_editing};

/// How long a key is held before it starts repeating.
pub const REPEAT_DELAY_NS: u64 = 500_000_000;
/// The time between repeats: 30 a second.
pub const REPEAT_PERIOD_NS: u64 = 1_000_000_000 / 30;
/// The most devices a seat has (`pbr.seat.devices` answers with at most this many).
pub const MAX_DEVICES: usize = 32;
/// The longest device name kept, in bytes (`pbr.seat`'s `Device.name`).
pub const MAX_NAME: usize = 64;

/// The size of a driver's event (`pbr.input`'s `EVENT_SIZE`).
pub const RAW_SIZE: usize = 16;

/// The highest key code a keyboard's state has room for. (`pbr.input` numbers keys from 1;
/// 105 is the highest so far.)
const MAX_KEY: u16 = 127;
/// The highest button code kept track of.
const MAX_BUTTON: u16 = 8;

/// What a driver's device is (`pbr.input`'s `InputKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A keyboard.
    Keyboard,
    /// A mouse: it says how far it moved.
    Mouse,
    /// One device that is both, such as the PS/2 controller. The seat shows it as two.
    KeyboardAndMouse,
    /// An absolute pointer: it says where it is.
    Tablet,
}

/// What one device of the seat is (`pbr.seat`'s `DeviceKind`; touch screens come later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// A keyboard.
    Keyboard,
    /// A mouse.
    Mouse,
    /// A tablet.
    Tablet,
}

/// One device of the seat, as `pbr.seat.devices` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The seat's number for it, which events carry. Never 0.
    pub id: u16,
    /// What it is.
    pub kind: DeviceKind,
    /// Its driver's name for it.
    pub name: String,
    /// The device manager's number for it.
    pub device: u32,
}

/// A device and what is held on it.
#[derive(Clone, Debug)]
struct Device {
    /// Which of the input service's sources it belongs to.
    source: u32,
    info: DeviceInfo,
    /// The keys that are down: bit `code` of this 128-bit set.
    keys: u128,
    /// Keys that were reported under another code than the keyboard sent (a keypad key
    /// pressed with Num Lock off): (the keyboard's code, the code reported). Their release
    /// carries the same code as their press did, whatever Num Lock is by then.
    renamed: Vec<(u16, u16)>,
    /// The buttons that are down: bit `code`.
    buttons: u16,
}

impl Device {
    fn down(&self, code: u16) -> bool {
        code <= MAX_KEY && self.keys & (1u128 << code) != 0
    }

    /// The code a held key was reported under.
    fn reported(&self, code: u16) -> u16 {
        self.renamed.iter().find(|r| r.0 == code).map_or(code, |r| r.1)
    }
}

/// The key that is repeating.
#[derive(Clone, Copy, Debug)]
struct Repeat {
    /// The device it is held on.
    device: u16,
    /// The keyboard's code for it.
    code: u16,
    /// When the next repeat is due.
    due: u64,
}

/// The seat. See the module's description.
#[derive(Clone, Debug)]
pub struct Seat {
    layout: Layout,
    devices: Vec<Device>,
    /// The number the next source gets.
    next_source: u32,
    /// The id the next device gets, if it is free.
    next_id: u16,
    caps_lock: bool,
    num_lock: bool,
    composer: Composer,
    repeat: Option<Repeat>,
}

/// Whether a key is one of the eight modifier keys.
fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        key::LEFT_SHIFT
            | key::RIGHT_SHIFT
            | key::LEFT_CTRL
            | key::RIGHT_CTRL
            | key::LEFT_ALT
            | key::RIGHT_ALT
            | key::LEFT_META
            | key::RIGHT_META
    )
}

/// Whether a key that types nothing still repeats when held: the keys that edit and move
/// (note 0021, section 2).
fn repeats_without_typing(code: u16) -> bool {
    matches!(
        code,
        key::BACKSPACE
            | key::DELETE
            | key::ENTER
            | key::KP_ENTER
            | key::TAB
            | key::LEFT
            | key::RIGHT
            | key::UP
            | key::DOWN
            | key::HOME
            | key::END
            | key::PAGE_UP
            | key::PAGE_DOWN
    )
}

impl Seat {
    /// A seat with no devices, using `layout`. Num Lock starts on (the keypad types digits,
    /// as PCs start) and Caps Lock off.
    pub fn new(layout: Layout) -> Seat {
        Seat {
            layout,
            devices: Vec::new(),
            next_source: 1,
            next_id: 1,
            caps_lock: false,
            num_lock: true,
            composer: Composer::new(),
            repeat: None,
        }
    }

    /// The devices there are now.
    pub fn devices(&self) -> impl Iterator<Item = &DeviceInfo> {
        self.devices.iter().map(|d| &d.info)
    }

    /// Changes the layout, between two events. A dead key that was waiting is dropped: it
    /// belonged to the old layout (note 0021, section 5).
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.composer.reset();
    }

    /// An id no device has: they are handed out in turn, so that one isn't used again soon
    /// after its device went, and never 0.
    fn fresh_id(&mut self) -> u16 {
        loop {
            let id = self.next_id;
            self.next_id = if id == u16::MAX { 1 } else { id + 1 };
            if !self.devices.iter().any(|d| d.info.id == id) {
                return id;
            }
        }
    }

    /// Adds a source: a driver's device, which is one device of the seat or (a keyboard and
    /// mouse in one) two. Returns the number to give [`Seat::raw`] and [`Seat::remove`] for
    /// it, or `None` if the seat is full. Tells the consumer the devices changed.
    pub fn add(
        &mut self,
        source: Source,
        name: &str,
        device: u32,
        now: u64,
        out: &mut Vec<Event>,
    ) -> Option<u32> {
        let kinds: &[DeviceKind] = match source {
            Source::Keyboard => &[DeviceKind::Keyboard],
            Source::Mouse => &[DeviceKind::Mouse],
            Source::KeyboardAndMouse => &[DeviceKind::Keyboard, DeviceKind::Mouse],
            Source::Tablet => &[DeviceKind::Tablet],
        };
        if self.devices.len() + kinds.len() > MAX_DEVICES {
            return None;
        }
        let number = self.next_source;
        self.next_source = self.next_source.wrapping_add(1).max(1);
        // The name is cut to what `pbr.seat` carries, at a character boundary (UTF-8
        // characters are one to four bytes, and a string must not end inside one).
        let mut end = name.len().min(MAX_NAME);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        for &kind in kinds {
            let id = self.fresh_id();
            self.devices.push(Device {
                source: number,
                info: DeviceInfo { id, kind, name: String::from(&name[..end]), device },
                keys: 0,
                renamed: Vec::new(),
                buttons: 0,
            });
        }
        out.push(Event {
            kind: kind::DEVICES,
            time: now,
            mods: self.all_mods(),
            ..Event::default()
        });
        Some(number)
    }

    /// Removes a source (its driver ended, or the device manager did). Whatever was held on
    /// it is let go: the consumer is sent a reset, then what is still held elsewhere, then
    /// that the devices changed. Nothing happens for a number that isn't a source.
    pub fn remove(&mut self, source: u32, now: u64, out: &mut Vec<Event>) {
        let before = self.devices.len();
        let gone: Vec<u16> =
            self.devices.iter().filter(|d| d.source == source).map(|d| d.info.id).collect();
        self.devices.retain(|d| d.source != source);
        if self.devices.len() == before {
            return;
        }
        if self.repeat.is_some_and(|r| gone.contains(&r.device)) {
            self.repeat = None;
        }
        self.composer.reset();
        self.held(now, out);
        out.push(Event {
            kind: kind::DEVICES,
            time: now,
            mods: self.all_mods(),
            ..Event::default()
        });
    }

    /// The modifier bits for what is held on one device, with the seat's locks.
    fn mods_of(&self, d: &Device) -> u16 {
        let mut m = 0;
        if d.down(key::LEFT_SHIFT) || d.down(key::RIGHT_SHIFT) {
            m |= mods::SHIFT;
        }
        if d.down(key::LEFT_CTRL) || d.down(key::RIGHT_CTRL) {
            m |= mods::CTRL;
        }
        // The right Alt key is AltGr on layouts that say so, and an Alt key on the others.
        let right_alt = if self.layout.has_altgr() { mods::ALTGR } else { mods::ALT };
        if d.down(key::LEFT_ALT) {
            m |= mods::ALT;
        }
        if d.down(key::RIGHT_ALT) {
            m |= right_alt;
        }
        if d.down(key::LEFT_META) || d.down(key::RIGHT_META) {
            m |= mods::META;
        }
        m | self.locks()
    }

    fn locks(&self) -> u16 {
        (if self.caps_lock { mods::CAPS_LOCK } else { 0 })
            | (if self.num_lock { mods::NUM_LOCK } else { 0 })
    }

    /// The modifiers held on any keyboard, with the locks: what pointer events carry, since a
    /// mouse has no modifier keys of its own and Ctrl+click means Ctrl on whichever keyboard.
    fn all_mods(&self) -> u16 {
        self.devices.iter().fold(self.locks(), |m, d| m | self.mods_of(d))
    }

    /// Tells the consumer where things stand: a reset ("assume nothing is held"), then an
    /// event with the value [`value::HELD`] for every key and button that is down. Sent first
    /// to a new consumer, and whenever events may have been missed.
    pub fn held(&self, now: u64, out: &mut Vec<Event>) {
        out.push(Event { kind: kind::RESET, time: now, mods: self.all_mods(), ..Event::default() });
        for d in &self.devices {
            let m = self.mods_of(d);
            for code in 1..=MAX_KEY {
                if d.down(code) {
                    out.push(Event {
                        kind: kind::KEY,
                        device: d.info.id,
                        code: d.reported(code),
                        mods: m,
                        value: value::HELD,
                        time: now,
                        ..Event::default()
                    });
                }
            }
            for code in 1..=MAX_BUTTON {
                if d.buttons & (1 << code) != 0 {
                    out.push(Event {
                        kind: kind::BUTTON,
                        device: d.info.id,
                        code,
                        mods: self.all_mods(),
                        value: value::HELD,
                        time: now,
                        ..Event::default()
                    });
                }
            }
        }
    }

    /// The device of `source` that key events (`keyboard`) or pointer events belong to.
    fn find(&self, source: u32, keyboard: bool) -> Option<usize> {
        self.devices
            .iter()
            .position(|d| d.source == source && (d.info.kind == DeviceKind::Keyboard) == keyboard)
    }

    /// Takes one event from a driver's ring (16 bytes, `pbr.input`): what it becomes for the
    /// consumer is added to `out`. Events from a number that isn't a source, of a kind the
    /// device can't send (a key from a mouse), or that make no sense are skipped.
    pub fn raw(&mut self, source: u32, raw: &[u8; RAW_SIZE], out: &mut Vec<Event>) {
        let what = u16::from_le_bytes([raw[0], raw[1]]);
        let code = u16::from_le_bytes([raw[2], raw[3]]);
        let amount = i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let mut time = [0u8; 8];
        time.copy_from_slice(&raw[8..16]);
        let time = u64::from_le_bytes(time);
        match what {
            kind::LOST => self.source_lost(source, time, out),
            kind::KEY => {
                if let Some(i) = self.find(source, true) {
                    self.key(i, code, amount, time, out);
                }
            }
            kind::MOVE | kind::SCROLL => {
                if let Some(i) = self.find(source, false) {
                    out.push(Event {
                        kind: what,
                        device: self.devices[i].info.id,
                        code,
                        mods: self.all_mods(),
                        value: amount,
                        time,
                        ..Event::default()
                    });
                }
            }
            kind::BUTTON => {
                if let Some(i) = self.find(source, false) {
                    self.button(i, code, amount, time, out);
                }
            }
            kind::POSITION => {
                // A driver's event has no room for two numbers, so it packs them: x in the
                // low 16 bits of the value and y in the high 16. The seat's events have room.
                if let Some(i) = self.find(source, false)
                    && self.devices[i].info.kind == DeviceKind::Tablet
                {
                    let packed = amount as u32;
                    out.push(Event {
                        kind: kind::POSITION,
                        device: self.devices[i].info.id,
                        mods: self.all_mods(),
                        value: (packed & 0xFFFF) as i32,
                        first: packed >> 16,
                        time,
                        ..Event::default()
                    });
                }
            }
            _ => {}
        }
    }

    /// A driver dropped events because the input service fell behind: a release may be among
    /// them, so nothing on that source is assumed held any more. The consumer is told the
    /// same way as when a device goes.
    fn source_lost(&mut self, source: u32, now: u64, out: &mut Vec<Event>) {
        let mut any = false;
        for d in self.devices.iter_mut().filter(|d| d.source == source) {
            d.keys = 0;
            d.renamed.clear();
            d.buttons = 0;
            any = true;
            if self.repeat.is_some_and(|r| r.device == d.info.id) {
                self.repeat = None;
            }
        }
        if any {
            self.composer.reset();
            self.held(now, out);
        }
    }

    fn button(&mut self, i: usize, code: u16, amount: i32, time: u64, out: &mut Vec<Event>) {
        if code == 0 || code > MAX_BUTTON {
            return;
        }
        let d = &mut self.devices[i];
        let bit = 1u16 << code;
        match amount {
            value::PRESSED if d.buttons & bit == 0 => d.buttons |= bit,
            value::RELEASED if d.buttons & bit != 0 => d.buttons &= !bit,
            // A second press of a button that is down, a release of one that isn't, or a
            // value that is neither.
            _ => return,
        }
        let id = d.info.id;
        out.push(Event {
            kind: kind::BUTTON,
            device: id,
            code,
            mods: self.all_mods(),
            value: amount,
            time,
            ..Event::default()
        });
    }

    /// What the key `code` of device `i` types now, as (the two character fields of its
    /// event, whether it is a key that types a character). `reported` is the code the event
    /// will carry, which differs from `code` for a keypad key acting as an editing key.
    ///
    /// While Ctrl, Alt or Meta is held the characters only say which key it was (for
    /// shortcuts), so a waiting dead key is left alone, and a dead key gives its accent.
    fn typed(&mut self, i: usize, code: u16, reported: u16) -> ([u32; 2], bool) {
        let m = self.mods_of(&self.devices[i]);
        // A keypad key that was reported as an editing key types nothing.
        let typed = if reported != code {
            Typed::Nothing
        } else {
            self.layout.key(
                code,
                Mods {
                    shift: m & mods::SHIFT != 0,
                    altgr: m & mods::ALTGR != 0,
                    caps_lock: self.caps_lock,
                    num_lock: self.num_lock,
                },
            )
        };
        let chars = if m & (mods::CTRL | mods::ALT | mods::META) != 0 {
            match typed {
                Typed::Nothing => [None, None],
                Typed::Char(c) => [Some(c), None],
                Typed::Dead(d) => [Some(self.layout.accent(d)), None],
            }
        } else if typed == Typed::Nothing {
            // A key that types nothing but isn't a modifier or a lock (an arrow, Backspace,
            // F1) ends a waiting dead key: the accent was meant for a letter that never came.
            if !is_modifier(code) && code != key::CAPS_LOCK && code != key::NUM_LOCK {
                self.composer.reset();
            }
            [None, None]
        } else {
            self.composer.feed(&self.layout, typed)
        };
        // `map_or` turns "no character" into 0, which is how an event says so.
        (
            [chars[0].map_or(0, u32::from), chars[1].map_or(0, u32::from)],
            matches!(typed, Typed::Char(_)),
        )
    }

    fn key(&mut self, i: usize, code: u16, amount: i32, time: u64, out: &mut Vec<Event>) {
        if code == 0 || code > MAX_KEY {
            return;
        }
        let bit = 1u128 << code;
        let was_down = self.devices[i].keys & bit != 0;
        let id = self.devices[i].info.id;
        match amount {
            value::RELEASED => {
                if !was_down {
                    return;
                }
                let d = &mut self.devices[i];
                d.keys &= !bit;
                let reported = d.reported(code);
                d.renamed.retain(|r| r.0 != code);
                if self.repeat.is_some_and(|r| r.device == id && r.code == code) {
                    self.repeat = None;
                }
                out.push(Event {
                    kind: kind::KEY,
                    device: id,
                    code: reported,
                    mods: self.mods_of(&self.devices[i]),
                    value: value::RELEASED,
                    time,
                    ..Event::default()
                });
            }
            value::PRESSED | value::REPEAT => {
                // A press of a key that is down is the keyboard's own repeat (PS/2 keyboards
                // make them, virtio ones sometimes): dropped, because repeats are made here,
                // the same for every keyboard.
                if was_down {
                    return;
                }
                self.devices[i].keys |= bit;
                match code {
                    key::CAPS_LOCK => self.caps_lock = !self.caps_lock,
                    key::NUM_LOCK => self.num_lock = !self.num_lock,
                    _ => {}
                }
                // With Num Lock off the keypad's digits are editing keys, and are reported
                // as those keys, so that no consumer needs to know about the keypad.
                let reported = match keypad_editing(code) {
                    Some(editing) if !self.num_lock => {
                        self.devices[i].renamed.push((code, editing));
                        editing
                    }
                    _ => code,
                };
                let (chars, types) = self.typed(i, code, reported);
                out.push(Event {
                    kind: kind::KEY,
                    device: id,
                    code: reported,
                    mods: self.mods_of(&self.devices[i]),
                    value: value::PRESSED,
                    first: chars[0],
                    second: chars[1],
                    time,
                });
                // One key repeats at a time, the last one pressed; a modifier going down
                // doesn't interrupt it (hold `a`, press Shift, and it goes on as `A`).
                if types || repeats_without_typing(reported) {
                    self.repeat = Some(Repeat {
                        device: id,
                        code,
                        due: time.saturating_add(REPEAT_DELAY_NS),
                    });
                } else if !is_modifier(code) {
                    self.repeat = None;
                }
            }
            _ => {}
        }
    }

    /// When the next repeat is due, if a key is repeating: the input service wakes then and
    /// calls [`Seat::tick`].
    pub fn next_repeat(&self) -> Option<u64> {
        self.repeat.map(|r| r.due)
    }

    /// Makes the repeat that is due at `now`, if one is. At most one for each call: after a
    /// long gap (the service was busy) the key carries on from now, it doesn't catch up with a
    /// burst.
    pub fn tick(&mut self, now: u64, out: &mut Vec<Event>) {
        let Some(r) = self.repeat else { return };
        if r.due > now {
            return;
        }
        let Some(i) = self.devices.iter().position(|d| d.info.id == r.device) else {
            self.repeat = None;
            return;
        };
        if !self.devices[i].down(r.code) {
            self.repeat = None;
            return;
        }
        let reported = self.devices[i].reported(r.code);
        let (chars, _) = self.typed(i, r.code, reported);
        out.push(Event {
            kind: kind::KEY,
            device: r.device,
            code: reported,
            mods: self.mods_of(&self.devices[i]),
            value: value::REPEAT,
            first: chars[0],
            second: chars[1],
            time: now,
        });
        let next = r.due.saturating_add(REPEAT_PERIOD_NS);
        let due = if next <= now { now.saturating_add(REPEAT_PERIOD_NS) } else { next };
        self.repeat = Some(Repeat { due, ..r });
    }
}
