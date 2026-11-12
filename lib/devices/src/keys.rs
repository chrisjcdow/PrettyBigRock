// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! PrettyBigRock's key codes, as `interfaces/pbr.input.pbi` numbers them, and the tables that
//! turn what keyboards send into them. A test checks the numbers against the interface file.

/// The key codes: a name and a number each.
#[allow(missing_docs)] // each is named for its key; the interface file documents them
pub mod key {
    pub const A: u16 = 1;
    pub const B: u16 = 2;
    pub const C: u16 = 3;
    pub const D: u16 = 4;
    pub const E: u16 = 5;
    pub const F: u16 = 6;
    pub const G: u16 = 7;
    pub const H: u16 = 8;
    pub const I: u16 = 9;
    pub const J: u16 = 10;
    pub const K: u16 = 11;
    pub const L: u16 = 12;
    pub const M: u16 = 13;
    pub const N: u16 = 14;
    pub const O: u16 = 15;
    pub const P: u16 = 16;
    pub const Q: u16 = 17;
    pub const R: u16 = 18;
    pub const S: u16 = 19;
    pub const T: u16 = 20;
    pub const U: u16 = 21;
    pub const V: u16 = 22;
    pub const W: u16 = 23;
    pub const X: u16 = 24;
    pub const Y: u16 = 25;
    pub const Z: u16 = 26;
    pub const D1: u16 = 27;
    pub const D2: u16 = 28;
    pub const D3: u16 = 29;
    pub const D4: u16 = 30;
    pub const D5: u16 = 31;
    pub const D6: u16 = 32;
    pub const D7: u16 = 33;
    pub const D8: u16 = 34;
    pub const D9: u16 = 35;
    pub const D0: u16 = 36;
    pub const ENTER: u16 = 37;
    pub const ESCAPE: u16 = 38;
    pub const BACKSPACE: u16 = 39;
    pub const TAB: u16 = 40;
    pub const SPACE: u16 = 41;
    pub const MINUS: u16 = 42;
    pub const EQUALS: u16 = 43;
    pub const LEFT_BRACKET: u16 = 44;
    pub const RIGHT_BRACKET: u16 = 45;
    pub const BACKSLASH: u16 = 46;
    pub const SEMICOLON: u16 = 47;
    pub const APOSTROPHE: u16 = 48;
    pub const GRAVE: u16 = 49;
    pub const COMMA: u16 = 50;
    pub const PERIOD: u16 = 51;
    pub const SLASH: u16 = 52;
    pub const CAPS_LOCK: u16 = 53;
    pub const F1: u16 = 54;
    pub const F2: u16 = 55;
    pub const F3: u16 = 56;
    pub const F4: u16 = 57;
    pub const F5: u16 = 58;
    pub const F6: u16 = 59;
    pub const F7: u16 = 60;
    pub const F8: u16 = 61;
    pub const F9: u16 = 62;
    pub const F10: u16 = 63;
    pub const F11: u16 = 64;
    pub const F12: u16 = 65;
    pub const INSERT: u16 = 66;
    pub const DELETE: u16 = 67;
    pub const HOME: u16 = 68;
    pub const END: u16 = 69;
    pub const PAGE_UP: u16 = 70;
    pub const PAGE_DOWN: u16 = 71;
    pub const RIGHT: u16 = 72;
    pub const LEFT: u16 = 73;
    pub const DOWN: u16 = 74;
    pub const UP: u16 = 75;
    pub const LEFT_SHIFT: u16 = 76;
    pub const RIGHT_SHIFT: u16 = 77;
    pub const LEFT_CTRL: u16 = 78;
    pub const RIGHT_CTRL: u16 = 79;
    pub const LEFT_ALT: u16 = 80;
    pub const RIGHT_ALT: u16 = 81;
    pub const LEFT_META: u16 = 82;
    pub const RIGHT_META: u16 = 83;
    pub const K102ND: u16 = 84;
    pub const KP_0: u16 = 85;
    pub const KP_1: u16 = 86;
    pub const KP_2: u16 = 87;
    pub const KP_3: u16 = 88;
    pub const KP_4: u16 = 89;
    pub const KP_5: u16 = 90;
    pub const KP_6: u16 = 91;
    pub const KP_7: u16 = 92;
    pub const KP_8: u16 = 93;
    pub const KP_9: u16 = 94;
    pub const KP_PERIOD: u16 = 95;
    pub const KP_PLUS: u16 = 96;
    pub const KP_MINUS: u16 = 97;
    pub const KP_MULTIPLY: u16 = 98;
    pub const KP_DIVIDE: u16 = 99;
    pub const KP_ENTER: u16 = 100;
    pub const NUM_LOCK: u16 = 101;
    pub const SCROLL_LOCK: u16 = 102;
    pub const PRINT_SCREEN: u16 = 103;
    pub const PAUSE: u16 = 104;
    pub const MENU: u16 = 105;
}

/// Every key code with its name, in numeric order (the names are the interface file's).
pub const ALL: &[(&str, u16)] = &[
    ("A", key::A),
    ("B", key::B),
    ("C", key::C),
    ("D", key::D),
    ("E", key::E),
    ("F", key::F),
    ("G", key::G),
    ("H", key::H),
    ("I", key::I),
    ("J", key::J),
    ("K", key::K),
    ("L", key::L),
    ("M", key::M),
    ("N", key::N),
    ("O", key::O),
    ("P", key::P),
    ("Q", key::Q),
    ("R", key::R),
    ("S", key::S),
    ("T", key::T),
    ("U", key::U),
    ("V", key::V),
    ("W", key::W),
    ("X", key::X),
    ("Y", key::Y),
    ("Z", key::Z),
    ("1", key::D1),
    ("2", key::D2),
    ("3", key::D3),
    ("4", key::D4),
    ("5", key::D5),
    ("6", key::D6),
    ("7", key::D7),
    ("8", key::D8),
    ("9", key::D9),
    ("0", key::D0),
    ("ENTER", key::ENTER),
    ("ESCAPE", key::ESCAPE),
    ("BACKSPACE", key::BACKSPACE),
    ("TAB", key::TAB),
    ("SPACE", key::SPACE),
    ("MINUS", key::MINUS),
    ("EQUALS", key::EQUALS),
    ("LEFT_BRACKET", key::LEFT_BRACKET),
    ("RIGHT_BRACKET", key::RIGHT_BRACKET),
    ("BACKSLASH", key::BACKSLASH),
    ("SEMICOLON", key::SEMICOLON),
    ("APOSTROPHE", key::APOSTROPHE),
    ("GRAVE", key::GRAVE),
    ("COMMA", key::COMMA),
    ("PERIOD", key::PERIOD),
    ("SLASH", key::SLASH),
    ("CAPS_LOCK", key::CAPS_LOCK),
    ("F1", key::F1),
    ("F2", key::F2),
    ("F3", key::F3),
    ("F4", key::F4),
    ("F5", key::F5),
    ("F6", key::F6),
    ("F7", key::F7),
    ("F8", key::F8),
    ("F9", key::F9),
    ("F10", key::F10),
    ("F11", key::F11),
    ("F12", key::F12),
    ("INSERT", key::INSERT),
    ("DELETE", key::DELETE),
    ("HOME", key::HOME),
    ("END", key::END),
    ("PAGE_UP", key::PAGE_UP),
    ("PAGE_DOWN", key::PAGE_DOWN),
    ("RIGHT", key::RIGHT),
    ("LEFT", key::LEFT),
    ("DOWN", key::DOWN),
    ("UP", key::UP),
    ("LEFT_SHIFT", key::LEFT_SHIFT),
    ("RIGHT_SHIFT", key::RIGHT_SHIFT),
    ("LEFT_CTRL", key::LEFT_CTRL),
    ("RIGHT_CTRL", key::RIGHT_CTRL),
    ("LEFT_ALT", key::LEFT_ALT),
    ("RIGHT_ALT", key::RIGHT_ALT),
    ("LEFT_META", key::LEFT_META),
    ("RIGHT_META", key::RIGHT_META),
    ("102ND", key::K102ND),
    ("KP_0", key::KP_0),
    ("KP_1", key::KP_1),
    ("KP_2", key::KP_2),
    ("KP_3", key::KP_3),
    ("KP_4", key::KP_4),
    ("KP_5", key::KP_5),
    ("KP_6", key::KP_6),
    ("KP_7", key::KP_7),
    ("KP_8", key::KP_8),
    ("KP_9", key::KP_9),
    ("KP_PERIOD", key::KP_PERIOD),
    ("KP_PLUS", key::KP_PLUS),
    ("KP_MINUS", key::KP_MINUS),
    ("KP_MULTIPLY", key::KP_MULTIPLY),
    ("KP_DIVIDE", key::KP_DIVIDE),
    ("KP_ENTER", key::KP_ENTER),
    ("NUM_LOCK", key::NUM_LOCK),
    ("SCROLL_LOCK", key::SCROLL_LOCK),
    ("PRINT_SCREEN", key::PRINT_SCREEN),
    ("PAUSE", key::PAUSE),
    ("MENU", key::MENU),
];

/// Linux input event codes (what virtio-input devices send) and the key each is.
pub const EVDEV: &[(u16, u16)] = &[
    (1, key::ESCAPE),
    (2, key::D1),
    (3, key::D2),
    (4, key::D3),
    (5, key::D4),
    (6, key::D5),
    (7, key::D6),
    (8, key::D7),
    (9, key::D8),
    (10, key::D9),
    (11, key::D0),
    (12, key::MINUS),
    (13, key::EQUALS),
    (14, key::BACKSPACE),
    (15, key::TAB),
    (16, key::Q),
    (17, key::W),
    (18, key::E),
    (19, key::R),
    (20, key::T),
    (21, key::Y),
    (22, key::U),
    (23, key::I),
    (24, key::O),
    (25, key::P),
    (26, key::LEFT_BRACKET),
    (27, key::RIGHT_BRACKET),
    (28, key::ENTER),
    (29, key::LEFT_CTRL),
    (30, key::A),
    (31, key::S),
    (32, key::D),
    (33, key::F),
    (34, key::G),
    (35, key::H),
    (36, key::J),
    (37, key::K),
    (38, key::L),
    (39, key::SEMICOLON),
    (40, key::APOSTROPHE),
    (41, key::GRAVE),
    (42, key::LEFT_SHIFT),
    (43, key::BACKSLASH),
    (44, key::Z),
    (45, key::X),
    (46, key::C),
    (47, key::V),
    (48, key::B),
    (49, key::N),
    (50, key::M),
    (51, key::COMMA),
    (52, key::PERIOD),
    (53, key::SLASH),
    (54, key::RIGHT_SHIFT),
    (56, key::LEFT_ALT),
    (57, key::SPACE),
    (58, key::CAPS_LOCK),
    (59, key::F1),
    (60, key::F2),
    (61, key::F3),
    (62, key::F4),
    (63, key::F5),
    (64, key::F6),
    (65, key::F7),
    (66, key::F8),
    (67, key::F9),
    (68, key::F10),
    (87, key::F11),
    (88, key::F12),
    (97, key::RIGHT_CTRL),
    (100, key::RIGHT_ALT),
    (102, key::HOME),
    (103, key::UP),
    (104, key::PAGE_UP),
    (105, key::LEFT),
    (106, key::RIGHT),
    (107, key::END),
    (108, key::DOWN),
    (109, key::PAGE_DOWN),
    (110, key::INSERT),
    (111, key::DELETE),
    (125, key::LEFT_META),
    (126, key::RIGHT_META),
    // The keypad, the locks and the rest (note 0021, section 4).
    (55, key::KP_MULTIPLY),
    (69, key::NUM_LOCK),
    (70, key::SCROLL_LOCK),
    (71, key::KP_7),
    (72, key::KP_8),
    (73, key::KP_9),
    (74, key::KP_MINUS),
    (75, key::KP_4),
    (76, key::KP_5),
    (77, key::KP_6),
    (78, key::KP_PLUS),
    (79, key::KP_1),
    (80, key::KP_2),
    (81, key::KP_3),
    (82, key::KP_0),
    (83, key::KP_PERIOD),
    (86, key::K102ND),
    (96, key::KP_ENTER),
    (98, key::KP_DIVIDE),
    (99, key::PRINT_SCREEN),
    (119, key::PAUSE),
    (127, key::MENU),
];

/// Set 1 scan codes of keys that send one byte (the make code; the break code has bit 7 set).
/// The main block of a keyboard is numbered the same as in [`EVDEV`].
pub const SET1: &[(u8, u16)] = &[
    (0x01, key::ESCAPE),
    (0x02, key::D1),
    (0x03, key::D2),
    (0x04, key::D3),
    (0x05, key::D4),
    (0x06, key::D5),
    (0x07, key::D6),
    (0x08, key::D7),
    (0x09, key::D8),
    (0x0a, key::D9),
    (0x0b, key::D0),
    (0x0c, key::MINUS),
    (0x0d, key::EQUALS),
    (0x0e, key::BACKSPACE),
    (0x0f, key::TAB),
    (0x10, key::Q),
    (0x11, key::W),
    (0x12, key::E),
    (0x13, key::R),
    (0x14, key::T),
    (0x15, key::Y),
    (0x16, key::U),
    (0x17, key::I),
    (0x18, key::O),
    (0x19, key::P),
    (0x1a, key::LEFT_BRACKET),
    (0x1b, key::RIGHT_BRACKET),
    (0x1c, key::ENTER),
    (0x1d, key::LEFT_CTRL),
    (0x1e, key::A),
    (0x1f, key::S),
    (0x20, key::D),
    (0x21, key::F),
    (0x22, key::G),
    (0x23, key::H),
    (0x24, key::J),
    (0x25, key::K),
    (0x26, key::L),
    (0x27, key::SEMICOLON),
    (0x28, key::APOSTROPHE),
    (0x29, key::GRAVE),
    (0x2a, key::LEFT_SHIFT),
    (0x2b, key::BACKSLASH),
    (0x2c, key::Z),
    (0x2d, key::X),
    (0x2e, key::C),
    (0x2f, key::V),
    (0x30, key::B),
    (0x31, key::N),
    (0x32, key::M),
    (0x33, key::COMMA),
    (0x34, key::PERIOD),
    (0x35, key::SLASH),
    (0x36, key::RIGHT_SHIFT),
    (0x38, key::LEFT_ALT),
    (0x39, key::SPACE),
    (0x3a, key::CAPS_LOCK),
    (0x3b, key::F1),
    (0x3c, key::F2),
    (0x3d, key::F3),
    (0x3e, key::F4),
    (0x3f, key::F5),
    (0x40, key::F6),
    (0x41, key::F7),
    (0x42, key::F8),
    (0x43, key::F9),
    (0x44, key::F10),
    (0x57, key::F11),
    (0x58, key::F12),
    // The keypad, the locks and the key beside the left Shift.
    (0x37, key::KP_MULTIPLY),
    (0x45, key::NUM_LOCK),
    (0x46, key::SCROLL_LOCK),
    (0x47, key::KP_7),
    (0x48, key::KP_8),
    (0x49, key::KP_9),
    (0x4a, key::KP_MINUS),
    (0x4b, key::KP_4),
    (0x4c, key::KP_5),
    (0x4d, key::KP_6),
    (0x4e, key::KP_PLUS),
    (0x4f, key::KP_1),
    (0x50, key::KP_2),
    (0x51, key::KP_3),
    (0x52, key::KP_0),
    (0x53, key::KP_PERIOD),
    (0x56, key::K102ND),
];

/// Set 1 scan codes of keys that send `0xE0` first.
pub const SET1_EXTENDED: &[(u8, u16)] = &[
    (0x1d, key::RIGHT_CTRL),
    (0x38, key::RIGHT_ALT),
    (0x47, key::HOME),
    (0x48, key::UP),
    (0x49, key::PAGE_UP),
    (0x4b, key::LEFT),
    (0x4d, key::RIGHT),
    (0x4f, key::END),
    (0x50, key::DOWN),
    (0x51, key::PAGE_DOWN),
    (0x52, key::INSERT),
    (0x53, key::DELETE),
    (0x5b, key::LEFT_META),
    (0x5c, key::RIGHT_META),
    (0x1c, key::KP_ENTER),
    (0x35, key::KP_DIVIDE),
    // Print Screen sends E0 2A E0 37: the first pair is a pretend Shift, which is in no table
    // and so is dropped; the second is the key.
    (0x37, key::PRINT_SCREEN),
    (0x5d, key::MENU),
];

/// The key a Linux input event code is, if it is one PrettyBigRock numbers.
pub fn from_evdev(code: u16) -> Option<u16> {
    EVDEV.iter().find(|(c, _)| *c == code).map(|(_, k)| *k)
}

/// The character a key types on a US keyboard, with Shift held (`shift`) or not. `None` for
/// keys that type no character (Enter, Tab, the arrows, Shift itself, the function keys) and
/// for codes that aren't keys at all.
///
/// This is the whole of the "layout" for now: a keyboard sends *which key* was pressed, by its
/// position (note 0011, section 8), and a layout says what that position types. Other layouts
/// come with the input service (note 0018, open questions). Caps Lock isn't handled.
pub fn us(code: u16, shift: bool) -> Option<char> {
    // The letters are numbered in order, so their characters are worked out, not listed.
    if (key::A..=key::Z).contains(&code) {
        let letter = b'a' + (code - key::A) as u8;
        return Some(char::from(if shift { letter.to_ascii_uppercase() } else { letter }));
    }
    // Everything else is a pair: what the key types plain, and with Shift.
    let (plain, shifted) = match code {
        key::D1 => ('1', '!'),
        key::D2 => ('2', '@'),
        key::D3 => ('3', '#'),
        key::D4 => ('4', '$'),
        key::D5 => ('5', '%'),
        key::D6 => ('6', '^'),
        key::D7 => ('7', '&'),
        key::D8 => ('8', '*'),
        key::D9 => ('9', '('),
        key::D0 => ('0', ')'),
        key::SPACE => (' ', ' '),
        key::MINUS => ('-', '_'),
        key::EQUALS => ('=', '+'),
        key::LEFT_BRACKET => ('[', '{'),
        key::RIGHT_BRACKET => (']', '}'),
        key::BACKSLASH => ('\\', '|'),
        key::SEMICOLON => (';', ':'),
        key::APOSTROPHE => ('\'', '"'),
        key::GRAVE => ('`', '~'),
        key::COMMA => (',', '<'),
        key::PERIOD => ('.', '>'),
        key::SLASH => ('/', '?'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

/// One step of the set 1 decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    /// More bytes are needed (a prefix), or the byte means nothing here.
    None,
    /// A key went down (`true`) or up.
    Key(u16, bool),
}

/// Turns a stream of set 1 scan code bytes into key presses and releases. It keeps no state
/// that a wrong byte can wedge: an unknown byte ends any prefix.
#[derive(Clone, Copy, Debug, Default)]
pub struct Set1 {
    /// The prefix byte the next byte belongs to: `0xE0`, `0xE1`, or 0 for none.
    prefix: u8,
    /// How many bytes have come since an `0xE1`.
    after_e1: u8,
}

impl Set1 {
    /// Takes the next byte.
    pub fn feed(&mut self, byte: u8) -> Scan {
        if self.prefix == 0xE1 {
            // Pause is the one key that starts with E1: it sends E1 1D 45 when it goes down
            // and E1 9D C5 straight after (it never says when it is let go). The byte after
            // the E1 is a pretend Ctrl; the one after that is the key, with bit 7 set for the
            // release like any other.
            self.after_e1 += 1;
            if self.after_e1 < 2 {
                return Scan::None;
            }
            self.prefix = 0;
            return if byte & 0x7F == 0x45 {
                Scan::Key(key::PAUSE, byte & 0x80 == 0)
            } else {
                Scan::None
            };
        }
        match (self.prefix, byte) {
            (_, 0xE0) => {
                self.prefix = 0xE0;
                Scan::None
            }
            (_, 0xE1) => {
                self.prefix = 0xE1;
                self.after_e1 = 0;
                Scan::None
            }
            (p, b) => {
                self.prefix = 0;
                let table = if p == 0xE0 { SET1_EXTENDED } else { SET1 };
                let make = b & 0x7F;
                match table.iter().find(|(c, _)| *c == make) {
                    Some((_, k)) => Scan::Key(*k, b & 0x80 == 0),
                    None => Scan::None,
                }
            }
        }
    }
}

/// A mouse movement or button change, from a three-byte PS/2 packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePacket {
    /// Movement to the right.
    pub dx: i32,
    /// Movement down (PS/2 counts up as positive, so this is the packet's value negated).
    pub dy: i32,
    /// Bit 0 left, bit 1 right, bit 2 middle.
    pub buttons: u8,
}

/// Collects PS/2 mouse bytes into packets. Bit 3 of the first byte is always set, which is how
/// it finds its place again after a lost byte.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mouse {
    bytes: [u8; 3],
    have: usize,
}

impl Mouse {
    /// Takes the next byte.
    pub fn feed(&mut self, byte: u8) -> Option<MousePacket> {
        if self.have == 0 && byte & 0x08 == 0 {
            return None; // not the start of a packet
        }
        self.bytes[self.have] = byte;
        self.have += 1;
        if self.have < 3 {
            return None;
        }
        self.have = 0;
        let [flags, x, y] = self.bytes;
        if flags & 0xC0 != 0 {
            return None; // the movement overflowed: the packet is nonsense
        }
        let extend = |v: u8, negative: bool| i32::from(v) - if negative { 256 } else { 0 };
        Some(MousePacket {
            dx: extend(x, flags & 0x10 != 0),
            dy: -extend(y, flags & 0x20 != 0),
            buttons: flags & 7,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers here are the interface file's.
    #[test]
    fn the_codes_match_the_interface_file() {
        let src = include_str!("../../../interfaces/pbr.input.pbi");
        let mut seen = 0;
        for line in src.lines() {
            let Some(rest) = line.strip_prefix("const KEY_") else { continue };
            let (name, value) = rest.split_once(": u32 = ").expect("a constant");
            let value: u16 = value.trim_end_matches(';').parse().expect("a number");
            assert!(ALL.contains(&(name, value)), "KEY_{name} = {value} is not in the table");
            seen += 1;
        }
        assert_eq!(seen, ALL.len());
        assert!(ALL.windows(2).all(|w| w[0].1 + 1 == w[1].1), "numbers in order, none missing");
    }

    #[test]
    fn the_tables_have_no_duplicates_and_cover_a_keyboard() {
        for (i, a) in EVDEV.iter().enumerate() {
            assert!(EVDEV.iter().skip(i + 1).all(|b| a.0 != b.0 && a.1 != b.1), "{a:?}");
        }
        // Every key can be typed on a set 1 keyboard, and every key is a Linux key too.
        for (name, k) in ALL {
            // (Pause has no entry: it is a sequence of its own, which `Set1::feed` knows.)
            let set1 = *k == key::PAUSE || SET1.iter().chain(SET1_EXTENDED).any(|(_, x)| x == k);
            assert!(set1, "{name} has no set 1 scan code");
            assert!(EVDEV.iter().any(|(_, x)| x == k), "{name} has no Linux code");
        }
        // Plain and extended codes don't clash within themselves.
        for t in [SET1, SET1_EXTENDED] {
            for (i, a) in t.iter().enumerate() {
                assert!(t.iter().skip(i + 1).all(|b| a.0 != b.0), "{a:?}");
            }
        }
    }

    #[test]
    fn the_us_layout_types_letters_digits_and_punctuation() {
        for (i, c) in ('a'..='z').enumerate() {
            let code = key::A + i as u16;
            assert_eq!(us(code, false), Some(c));
            assert_eq!(us(code, true), Some(c.to_ascii_uppercase()));
        }
        let others = [
            (key::D1, '1', '!'),
            (key::D2, '2', '@'),
            (key::D3, '3', '#'),
            (key::D4, '4', '$'),
            (key::D5, '5', '%'),
            (key::D6, '6', '^'),
            (key::D7, '7', '&'),
            (key::D8, '8', '*'),
            (key::D9, '9', '('),
            (key::D0, '0', ')'),
            (key::SPACE, ' ', ' '),
            (key::MINUS, '-', '_'),
            (key::EQUALS, '=', '+'),
            (key::LEFT_BRACKET, '[', '{'),
            (key::RIGHT_BRACKET, ']', '}'),
            (key::BACKSLASH, '\\', '|'),
            (key::SEMICOLON, ';', ':'),
            (key::APOSTROPHE, '\'', '"'),
            (key::GRAVE, '`', '~'),
            (key::COMMA, ',', '<'),
            (key::PERIOD, '.', '>'),
            (key::SLASH, '/', '?'),
        ];
        for (code, plain, shifted) in others {
            assert_eq!((us(code, false), us(code, true)), (Some(plain), Some(shifted)));
        }
    }

    /// Every code in the table either types a printable character (a different one for each
    /// key and shift state, except that both states of Space are a space) or types nothing in
    /// both states.
    #[test]
    fn the_us_layout_covers_the_whole_table() {
        let mut seen = std::collections::BTreeSet::new();
        let mut typing = 0;
        for (name, code) in ALL {
            match (us(*code, false), us(*code, true)) {
                (None, None) => {}
                (Some(plain), Some(shifted)) => {
                    typing += 1;
                    for c in [plain, shifted] {
                        assert!(c.is_ascii_graphic() || c == ' ', "{name} types {c:?}");
                    }
                    assert!(seen.insert(plain), "{name} types {plain:?} twice over");
                    if *code != key::SPACE {
                        assert!(seen.insert(shifted), "{name} types {shifted:?} twice over");
                    }
                }
                other => panic!("{name} types only in one state: {other:?}"),
            }
        }
        // 26 letters, 10 digits, Space and 11 punctuation keys.
        assert_eq!(typing, 48);
        // Together they can type every printable ASCII character.
        for c in ' '..='~' {
            assert!(seen.contains(&c), "{c:?} can't be typed");
        }
        for code in [
            key::ENTER,
            key::ESCAPE,
            key::BACKSPACE,
            key::TAB,
            key::CAPS_LOCK,
            key::F1,
            key::F12,
            key::DELETE,
            key::UP,
            key::LEFT_SHIFT,
            key::RIGHT_META,
        ] {
            assert_eq!((us(code, false), us(code, true)), (None, None), "{code}");
        }
    }

    #[test]
    fn the_us_layout_ignores_codes_that_are_not_keys() {
        for code in [0, 84, 85, 255, 256, 1000, u16::MAX - 1, u16::MAX] {
            assert_eq!((us(code, false), us(code, true)), (None, None), "{code}");
        }
    }

    #[test]
    fn set1_decodes_presses_releases_and_prefixes() {
        let mut d = Set1::default();
        assert_eq!(d.feed(0x1E), Scan::Key(key::A, true));
        assert_eq!(d.feed(0x9E), Scan::Key(key::A, false));
        assert_eq!(d.feed(0xE0), Scan::None);
        assert_eq!(d.feed(0x48), Scan::Key(key::UP, true));
        assert_eq!(d.feed(0xE0), Scan::None);
        assert_eq!(d.feed(0xC8), Scan::Key(key::UP, false));
        // The prefix applies to one byte only: plain 0x48 is the keypad's 8, not the up arrow.
        assert_eq!(d.feed(0x48), Scan::Key(key::KP_8, true));
        assert_eq!(d.feed(0xC8), Scan::Key(key::KP_8, false));
        // Pause sends its press and its release one after the other.
        let pause: Vec<Scan> =
            [0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5].into_iter().map(|b| d.feed(b)).collect();
        assert_eq!(
            pause,
            [
                Scan::None,
                Scan::None,
                Scan::Key(key::PAUSE, true),
                Scan::None,
                Scan::None,
                Scan::Key(key::PAUSE, false)
            ]
        );
        // Anything else after E1 is dropped, two bytes of it, and the decoder carries on.
        for b in [0xE1, 0x1E, 0x1E] {
            assert_eq!(d.feed(b), Scan::None);
        }
        // Print Screen (E0 2A E0 37): the pretend Shift is dropped, the key is kept.
        let print: Vec<Scan> = [0xE0, 0x2A, 0xE0, 0x37].into_iter().map(|b| d.feed(b)).collect();
        assert_eq!(print, [Scan::None, Scan::None, Scan::None, Scan::Key(key::PRINT_SCREEN, true)]);
        // The keypad's Enter and slash are extended; its star is not.
        assert_eq!(d.feed(0xE0), Scan::None);
        assert_eq!(d.feed(0x1C), Scan::Key(key::KP_ENTER, true));
        assert_eq!(d.feed(0xE0), Scan::None);
        assert_eq!(d.feed(0x35), Scan::Key(key::KP_DIVIDE, true));
        assert_eq!(d.feed(0x37), Scan::Key(key::KP_MULTIPLY, true));
        assert_eq!(d.feed(0x39), Scan::Key(key::SPACE, true));
        // Unknown bytes are ignored and cancel a prefix.
        assert_eq!(d.feed(0xE0), Scan::None);
        assert_eq!(d.feed(0x7F), Scan::None);
        assert_eq!(d.feed(0x1E), Scan::Key(key::A, true));
    }

    #[test]
    fn the_mouse_decoder_makes_packets_and_resynchronises() {
        let mut m = Mouse::default();
        assert_eq!(m.feed(0x09), None);
        assert_eq!(m.feed(5), None);
        // Left button down, moved right 5 and (PS/2 up is positive) up 3: down is negative.
        assert_eq!(m.feed(3), Some(MousePacket { dx: 5, dy: -3, buttons: 1 }));
        // Negative movement: dx = -2, dy = +4 (down).
        m.feed(0x08 | 0x10 | 0x20);
        m.feed(0xFE);
        let p = m.feed(0xFC).unwrap();
        assert_eq!((p.dx, p.dy, p.buttons), (-2, 4, 0));
        // A byte with bit 3 clear can't start a packet: it is dropped.
        assert_eq!(m.feed(0x00), None);
        assert_eq!(m.feed(0x0A), None);
        m.feed(0);
        assert_eq!(m.feed(0), Some(MousePacket { dx: 0, dy: 0, buttons: 2 }));
        // An overflowing packet is discarded.
        m.feed(0x48);
        m.feed(1);
        assert_eq!(m.feed(1), None);
    }
}
