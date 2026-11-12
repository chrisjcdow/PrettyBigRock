// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `pbr.seat`'s events: the 32 bytes the input service puts in its consumer's ring
//! (`interfaces/pbr.seat.pbi` has the table). Both sides use this one description: the input
//! service to write them, the console and the display server to read them.
//!
//! The numbers below are the interface file's; a test compares them.

/// The size of one event in the ring, in bytes.
pub const EVENT_SIZE: usize = 32;

/// Event kinds (`EVENT_*` in `pbr.seat.pbi`).
#[allow(missing_docs)] // each is described in the interface file
pub mod kind {
    pub const LOST: u16 = 0;
    pub const KEY: u16 = 1;
    pub const MOVE: u16 = 2;
    pub const BUTTON: u16 = 3;
    pub const SCROLL: u16 = 4;
    pub const POSITION: u16 = 5;
    pub const RESET: u16 = 6;
    pub const DEVICES: u16 = 7;
}

/// Values of key and button events (`VALUE_*` in `pbr.input.pbi`, and `VALUE_HELD` in
/// `pbr.seat.pbi`).
#[allow(missing_docs)]
pub mod value {
    pub const RELEASED: i32 = 0;
    pub const PRESSED: i32 = 1;
    pub const REPEAT: i32 = 2;
    /// Still held, after a reset: stands for a press the consumer has seen already or missed.
    pub const HELD: i32 = 3;
}

/// Modifier bits (`MOD_*` in `pbr.seat.pbi`).
#[allow(missing_docs)]
pub mod mods {
    pub const SHIFT: u16 = 1;
    pub const CTRL: u16 = 2;
    pub const ALT: u16 = 4;
    pub const ALTGR: u16 = 8;
    pub const META: u16 = 16;
    pub const CAPS_LOCK: u16 = 32;
    pub const NUM_LOCK: u16 = 64;
}

/// One event, as fields. [`Event::encode`] and [`Event::decode`] turn it into the ring's bytes
/// and back.
///
/// `#[derive(...)]` asks the compiler to write the obvious code for copying, printing and
/// comparing the type; `Default` makes one with every field 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// What happened: one of [`kind`].
    pub kind: u16,
    /// The device it came from (the `id` in `pbr.seat.devices`); 0 when it is about the whole
    /// seat (a reset, a loss, a change of devices).
    pub device: u16,
    /// The key (a `pbr.input` key code), the button, or the axis.
    pub code: u16,
    /// The modifiers held after this event: bits of [`mods`].
    pub mods: u16,
    /// For keys and buttons one of [`value`]; an amount for movement and scrolling; x for a
    /// position; the number dropped for a loss.
    pub value: i32,
    /// For keys: the first character typed, 0 if none. For positions: y.
    pub first: u32,
    /// For keys: a second character typed, 0 if none.
    pub second: u32,
    /// When it happened: nanoseconds on the monotonic clock.
    pub time: u64,
}

impl Event {
    /// The event as the ring holds it: little-endian fields at the offsets the interface file
    /// gives, with the reserved bytes 0.
    pub fn encode(&self) -> [u8; EVENT_SIZE] {
        let mut e = [0u8; EVENT_SIZE];
        e[0..2].copy_from_slice(&self.kind.to_le_bytes());
        e[2..4].copy_from_slice(&self.device.to_le_bytes());
        e[4..6].copy_from_slice(&self.code.to_le_bytes());
        e[6..8].copy_from_slice(&self.mods.to_le_bytes());
        e[8..12].copy_from_slice(&self.value.to_le_bytes());
        e[12..16].copy_from_slice(&self.first.to_le_bytes());
        e[16..20].copy_from_slice(&self.second.to_le_bytes());
        e[24..32].copy_from_slice(&self.time.to_le_bytes());
        e
    }

    /// Reads an event from the ring's bytes. Every pattern of bytes is some event: what the
    /// fields mean is for the reader to check (an unknown kind is skipped, [`Event::chars`]
    /// drops numbers that are no character), so a consumer never has to trust the ring.
    pub fn decode(e: &[u8; EVENT_SIZE]) -> Event {
        let u16_at = |i: usize| u16::from_le_bytes([e[i], e[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
        let mut time = [0u8; 8];
        time.copy_from_slice(&e[24..32]);
        Event {
            kind: u16_at(0),
            device: u16_at(2),
            code: u16_at(4),
            mods: u16_at(6),
            value: u32_at(8) as i32,
            first: u32_at(12),
            second: u32_at(16),
            time: u64::from_le_bytes(time),
        }
    }

    /// The characters a key event types, in order: none, one or two. Empty for every other
    /// kind of event, and for numbers that aren't characters or are control characters.
    ///
    /// The caller still has to look at the modifiers: with Ctrl, Alt or Meta held the
    /// characters say which key it was (Ctrl+S carries `s`), and are not to be typed
    /// ([`Event::types`]).
    pub fn chars(&self) -> impl Iterator<Item = char> + use<> {
        let both = if self.kind == kind::KEY { [self.first, self.second] } else { [0, 0] };
        both.into_iter().filter_map(char::from_u32).filter(|c| !c.is_control())
    }

    /// Whether this is a key press or repeat whose characters are to be typed: no Ctrl, Alt or
    /// Meta is held. (AltGr and Shift choose the character; they don't stop it typing.)
    pub fn types(&self) -> bool {
        self.kind == kind::KEY
            && matches!(self.value, value::PRESSED | value::REPEAT)
            && self.mods & (mods::CTRL | mods::ALT | mods::META) == 0
    }

    /// Where an absolute pointer is: (x, y), each 0 to 65535 across the device's surface.
    /// `None` for other events, and for a position outside that range.
    pub fn position(&self) -> Option<(u16, u16)> {
        if self.kind != kind::POSITION {
            return None;
        }
        Some((u16::try_from(self.value).ok()?, u16::try_from(self.first).ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value of `const NAME: u32 = N;` in an interface file.
    fn constant(src: &str, name: &str) -> i64 {
        let line = src
            .lines()
            .find_map(|l| l.strip_prefix(&format!("const {name}: u32 = ")))
            .unwrap_or_else(|| panic!("{name} is not in the interface file"));
        line.trim_end_matches(';').parse().unwrap()
    }

    #[test]
    fn the_constants_are_the_interface_files() {
        let seat = include_str!("../../../interfaces/pbr.seat.pbi");
        let input = include_str!("../../../interfaces/pbr.input.pbi");
        assert_eq!(EVENT_SIZE as i64, constant(seat, "EVENT_SIZE"));
        for (name, n) in [
            ("EVENT_LOST", kind::LOST),
            ("EVENT_KEY", kind::KEY),
            ("EVENT_MOVE", kind::MOVE),
            ("EVENT_BUTTON", kind::BUTTON),
            ("EVENT_SCROLL", kind::SCROLL),
            ("EVENT_POSITION", kind::POSITION),
            ("EVENT_RESET", kind::RESET),
            ("EVENT_DEVICES", kind::DEVICES),
            ("MOD_SHIFT", mods::SHIFT),
            ("MOD_CTRL", mods::CTRL),
            ("MOD_ALT", mods::ALT),
            ("MOD_ALTGR", mods::ALTGR),
            ("MOD_META", mods::META),
            ("MOD_CAPS_LOCK", mods::CAPS_LOCK),
            ("MOD_NUM_LOCK", mods::NUM_LOCK),
        ] {
            assert_eq!(i64::from(n), constant(seat, name), "{name}");
        }
        assert_eq!(i64::from(value::HELD), constant(seat, "VALUE_HELD"));
        assert_eq!(i64::from(value::RELEASED), constant(input, "VALUE_RELEASED"));
        assert_eq!(i64::from(value::PRESSED), constant(input, "VALUE_PRESSED"));
        assert_eq!(i64::from(value::REPEAT), constant(input, "VALUE_REPEAT"));
        // The kinds the drivers' events share with the seat's have the same numbers, which the
        // input service relies on when it passes pointer events through.
        for (name, n) in [
            ("EVENT_LOST", kind::LOST),
            ("EVENT_KEY", kind::KEY),
            ("EVENT_MOVE", kind::MOVE),
            ("EVENT_BUTTON", kind::BUTTON),
            ("EVENT_SCROLL", kind::SCROLL),
            ("EVENT_POSITION", kind::POSITION),
        ] {
            assert_eq!(i64::from(n), constant(input, name), "{name} in pbr.input");
        }
    }

    #[test]
    fn an_event_is_laid_out_as_the_interface_file_says() {
        let e = Event {
            kind: 0x0102,
            device: 0x0304,
            code: 0x0506,
            mods: 0x0708,
            value: 0x090A_0B0C,
            first: 0x0D0E_0F10,
            second: 0x1112_1314,
            time: 0x1516_1718_191A_1B1C,
        };
        let b = e.encode();
        assert_eq!(b[0..2], [0x02, 0x01]);
        assert_eq!(b[2..4], [0x04, 0x03]);
        assert_eq!(b[4..6], [0x06, 0x05]);
        assert_eq!(b[6..8], [0x08, 0x07]);
        assert_eq!(b[8..12], [0x0C, 0x0B, 0x0A, 0x09]);
        assert_eq!(b[12..16], [0x10, 0x0F, 0x0E, 0x0D]);
        assert_eq!(b[16..20], [0x14, 0x13, 0x12, 0x11]);
        assert_eq!(b[20..24], [0, 0, 0, 0], "reserved");
        assert_eq!(b[24..32], [0x1C, 0x1B, 0x1A, 0x19, 0x18, 0x17, 0x16, 0x15]);
        assert_eq!(Event::decode(&b), e);
    }

    #[test]
    fn every_event_survives_the_ring_and_negative_amounts_stay_negative() {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..2000 {
            let e = Event {
                kind: next() as u16,
                device: next() as u16,
                code: next() as u16,
                mods: next() as u16,
                value: next() as i32,
                first: next() as u32,
                second: next() as u32,
                time: next(),
            };
            assert_eq!(Event::decode(&e.encode()), e);
        }
        let moved = Event { kind: kind::MOVE, value: -7, ..Event::default() };
        assert_eq!(Event::decode(&moved.encode()).value, -7);
    }

    #[test]
    fn characters_come_only_from_key_events_and_only_real_ones() {
        let key = |first: u32, second: u32| Event {
            kind: kind::KEY,
            value: value::PRESSED,
            first,
            second,
            ..Event::default()
        };
        let chars = |e: Event| e.chars().collect::<Vec<char>>();
        assert_eq!(chars(key('a' as u32, 0)), ['a']);
        assert_eq!(chars(key('^' as u32, 'x' as u32)), ['^', 'x']);
        assert_eq!(chars(key(0xEA, 0)), ['ê']);
        assert_eq!(chars(key(0, 0)), []);
        // Not characters at all (a surrogate, a number past the last character), and control
        // characters, which nothing types.
        assert_eq!(chars(key(0xD800, 0x11_0000)), []);
        assert_eq!(chars(key(0x1B, 0x7F)), []);
        assert_eq!(chars(key(u32::MAX, 'b' as u32)), ['b']);
        // A position's y is in the same field as a key's first character.
        let place = Event { kind: kind::POSITION, value: 5, first: 'a' as u32, ..Event::default() };
        assert_eq!(chars(place), []);
    }

    #[test]
    fn typing_needs_a_press_without_ctrl_alt_or_meta() {
        let key =
            |value: i32, mods: u16| Event { kind: kind::KEY, value, mods, ..Event::default() };
        assert!(key(value::PRESSED, 0).types());
        assert!(key(value::REPEAT, mods::SHIFT | mods::ALTGR | mods::CAPS_LOCK).types());
        assert!(!key(value::RELEASED, 0).types());
        assert!(!key(value::HELD, 0).types());
        for held in [mods::CTRL, mods::ALT, mods::META] {
            assert!(!key(value::PRESSED, held).types());
        }
        assert!(!Event { kind: kind::BUTTON, value: value::PRESSED, ..Event::default() }.types());
    }

    #[test]
    fn positions_are_checked() {
        let place =
            |x: i32, y: u32| Event { kind: kind::POSITION, value: x, first: y, ..Event::default() };
        assert_eq!(place(0, 0).position(), Some((0, 0)));
        assert_eq!(place(65535, 65535).position(), Some((65535, 65535)));
        assert_eq!(place(65536, 0).position(), None);
        assert_eq!(place(-1, 0).position(), None);
        assert_eq!(place(0, 65536).position(), None);
        assert_eq!(Event { kind: kind::MOVE, value: 3, ..Event::default() }.position(), None);
    }
}
