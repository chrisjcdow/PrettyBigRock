// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning what a keyboard sends into editor keys.
//!
//! An input driver puts 16-byte events in a ring (docs/design/0011-device-manager-and-drivers.md,
//! section 8, and `interfaces/pbr.input.pbi`). A keyboard event says *which key* went down or
//! up, by its position on the keyboard, not which character it types; [`keys::us`] knows what
//! the US layout types, and this decoder keeps track of Shift, which is why a press needs more
//! than the event to be understood.
//!
//! One [`KeyDecoder`] is kept for each keyboard: Shift held on one keyboard does not apply to
//! another.

use crate::editor::Key;
use pbr_devices::keys::{self, key};

/// The size of an event in the ring, in bytes.
pub const EVENT_SIZE: usize = 16;

/// Event kinds (`EVENT_*` in `pbr.input.pbi`; a test checks the numbers).
const EVENT_LOST: u16 = 0;
const EVENT_KEY: u16 = 1;

/// Values of key events (`VALUE_*` in `pbr.input.pbi`).
const VALUE_RELEASED: i32 = 0;
const VALUE_PRESSED: i32 = 1;
const VALUE_REPEAT: i32 = 2;

/// Decodes one keyboard's events.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyDecoder {
    left_shift: bool,
    right_shift: bool,
}

impl KeyDecoder {
    /// A decoder for a keyboard with no key held.
    pub fn new() -> KeyDecoder {
        KeyDecoder::default()
    }

    /// Whether either Shift key is held.
    pub fn shift(&self) -> bool {
        self.left_shift || self.right_shift
    }

    /// Takes the next event from the ring: the editor key it makes, if any.
    ///
    /// - A press or a repeat of a key that types a character or edits is a key; a repeat is
    ///   just another press, which is what holding a key down is for.
    /// - Releases only matter for Shift.
    /// - A marker that events were lost (kind 0) means a Shift release may have been lost with
    ///   them, so Shift is let go rather than risk it staying down.
    /// - Anything else (mouse events, unknown kinds, a value that is none of released, pressed
    ///   or repeated) is skipped.
    pub fn feed(&mut self, event: &[u8; EVENT_SIZE]) -> Option<Key> {
        let kind = u16::from_le_bytes([event[0], event[1]]);
        let code = u16::from_le_bytes([event[2], event[3]]);
        let value = i32::from_le_bytes([event[4], event[5], event[6], event[7]]);
        match kind {
            EVENT_LOST => {
                *self = KeyDecoder::default();
                return None;
            }
            EVENT_KEY => {}
            _ => return None,
        }
        let down = match value {
            VALUE_PRESSED | VALUE_REPEAT => true,
            VALUE_RELEASED => false,
            _ => return None,
        };
        match code {
            key::LEFT_SHIFT => self.left_shift = down,
            key::RIGHT_SHIFT => self.right_shift = down,
            _ if !down => {}
            key::ENTER => return Some(Key::Enter),
            key::BACKSPACE => return Some(Key::Backspace),
            key::DELETE => return Some(Key::Delete),
            key::LEFT => return Some(Key::Left),
            key::RIGHT => return Some(Key::Right),
            key::UP => return Some(Key::Up),
            key::DOWN => return Some(Key::Down),
            key::HOME => return Some(Key::Home),
            key::END => return Some(Key::End),
            _ => return keys::us(code, self.shift()).map(Key::Char),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
        let mut e = [0xAA; EVENT_SIZE]; // the time, which is not looked at
        e[0..2].copy_from_slice(&kind.to_le_bytes());
        e[2..4].copy_from_slice(&code.to_le_bytes());
        e[4..8].copy_from_slice(&value.to_le_bytes());
        e
    }

    fn press(d: &mut KeyDecoder, code: u16) -> Option<Key> {
        d.feed(&event(1, code, 1))
    }

    fn release(d: &mut KeyDecoder, code: u16) -> Option<Key> {
        d.feed(&event(1, code, 0))
    }

    #[test]
    fn the_constants_are_the_interface_files() {
        let src = include_str!("../../../interfaces/pbr.input.pbi");
        let value = |name: &str| -> i64 {
            let line = src
                .lines()
                .find_map(|l| l.strip_prefix(&format!("const {name}: u32 = ")))
                .unwrap_or_else(|| panic!("{name} is not in the interface file"));
            line.trim_end_matches(';').parse().unwrap()
        };
        assert_eq!(i64::from(EVENT_LOST), value("EVENT_LOST"));
        assert_eq!(i64::from(EVENT_KEY), value("EVENT_KEY"));
        assert_eq!(i64::from(VALUE_RELEASED), value("VALUE_RELEASED"));
        assert_eq!(i64::from(VALUE_PRESSED), value("VALUE_PRESSED"));
        assert_eq!(i64::from(VALUE_REPEAT), value("VALUE_REPEAT"));
        assert_eq!(EVENT_SIZE as i64, value("EVENT_SIZE"));
    }

    #[test]
    fn keys_type_characters_and_edit() {
        let mut d = KeyDecoder::new();
        assert_eq!(press(&mut d, key::H), Some(Key::Char('h')));
        assert_eq!(press(&mut d, key::D1), Some(Key::Char('1')));
        assert_eq!(press(&mut d, key::SPACE), Some(Key::Char(' ')));
        assert_eq!(press(&mut d, key::SLASH), Some(Key::Char('/')));
        for (code, k) in [
            (key::ENTER, Key::Enter),
            (key::BACKSPACE, Key::Backspace),
            (key::DELETE, Key::Delete),
            (key::LEFT, Key::Left),
            (key::RIGHT, Key::Right),
            (key::UP, Key::Up),
            (key::DOWN, Key::Down),
            (key::HOME, Key::Home),
            (key::END, Key::End),
        ] {
            assert_eq!(press(&mut d, code), Some(k));
            assert_eq!(release(&mut d, code), None, "a release is nothing");
        }
        for code in [key::TAB, key::ESCAPE, key::F1, key::PAGE_UP, key::LEFT_CTRL, key::CAPS_LOCK] {
            assert_eq!(press(&mut d, code), None);
        }
    }

    #[test]
    fn a_repeat_is_another_press() {
        let mut d = KeyDecoder::new();
        assert_eq!(d.feed(&event(1, key::A, 2)), Some(Key::Char('a')));
        assert_eq!(d.feed(&event(1, key::BACKSPACE, 2)), Some(Key::Backspace));
    }

    #[test]
    fn shift_applies_while_either_is_held() {
        let mut d = KeyDecoder::new();
        assert_eq!(press(&mut d, key::LEFT_SHIFT), None);
        assert_eq!(press(&mut d, key::A), Some(Key::Char('A')));
        assert_eq!(press(&mut d, key::D2), Some(Key::Char('@')));
        assert_eq!(d.feed(&event(1, key::LEFT_SHIFT, 2)), None, "a repeat of Shift");
        assert!(d.shift());
        // Both held, then one let go: still shifted.
        press(&mut d, key::RIGHT_SHIFT);
        release(&mut d, key::LEFT_SHIFT);
        assert_eq!(press(&mut d, key::B), Some(Key::Char('B')));
        release(&mut d, key::RIGHT_SHIFT);
        assert!(!d.shift());
        assert_eq!(press(&mut d, key::B), Some(Key::Char('b')));
        // Shift does not change the editing keys.
        press(&mut d, key::RIGHT_SHIFT);
        assert_eq!(press(&mut d, key::LEFT), Some(Key::Left));
    }

    #[test]
    fn a_lost_event_marker_lets_go_of_shift() {
        let mut d = KeyDecoder::new();
        press(&mut d, key::LEFT_SHIFT);
        press(&mut d, key::RIGHT_SHIFT);
        assert_eq!(d.feed(&event(0, 0, 17)), None);
        assert!(!d.shift());
        assert_eq!(press(&mut d, key::A), Some(Key::Char('a')));
    }

    #[test]
    fn other_events_are_skipped() {
        let mut d = KeyDecoder::new();
        press(&mut d, key::LEFT_SHIFT);
        // Mouse movement, buttons and scrolling, and kinds nobody has defined: even when their
        // numbers look like a key's.
        for kind in [2, 3, 4, 5, 100, u16::MAX] {
            assert_eq!(d.feed(&event(kind, key::A, 1)), None, "kind {kind}");
        }
        // Key values that are neither released, pressed nor repeated.
        for value in [3, -1, 100, i32::MIN, i32::MAX] {
            assert_eq!(d.feed(&event(1, key::A, value)), None, "value {value}");
            assert_eq!(d.feed(&event(1, key::LEFT_SHIFT, value)), None);
        }
        assert!(d.shift(), "none of that let go of Shift");
        // Codes that are not keys.
        for code in [0, 84, 1000, u16::MAX] {
            assert_eq!(press(&mut d, code), None, "code {code}");
        }
    }

    /// Whatever bytes a driver (or a stale ring) holds, decoding never panics.
    #[test]
    fn random_events_are_harmless() {
        let mut seed = 5u64;
        let mut d = KeyDecoder::new();
        for _ in 0..50_000 {
            let mut e = [0u8; EVENT_SIZE];
            for b in &mut e {
                seed ^= seed >> 12;
                seed ^= seed << 25;
                seed ^= seed >> 27;
                *b = (seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8;
            }
            // Make real-looking kinds, codes and values common.
            e[1] = 0;
            e[0] &= 3;
            e[3] = 0;
            e[2] %= 90;
            e[4] &= 3;
            e[5..8].fill(0);
            let _ = d.feed(&e);
        }
    }
}
