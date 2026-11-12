// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! From the seat's events to the line editor's [`Key`]s.
//!
//! The console gets its keys from the input service (`pbr.seat`,
//! docs/design/0021-input-and-keyboard-layouts.md): a ring of 32-byte events, which `lib/seat`
//! describes. By the time an event arrives the hard work is done: the input service has
//! applied the user's keyboard layout (so a key press already carries the characters it
//! types, `£` or `ê` as much as `a`), kept track of Shift and the other modifiers, and made
//! the repeats of a key that is held. What is left is small and has no memory:
//!
//! - a press or a repeat of an editing key (Enter, Backspace, Delete, the arrows, Home, End)
//!   is that [`Key`];
//! - a press or a repeat that types is a [`Key::Char`] for each character, at most two (a
//!   dead key followed by a letter it doesn't combine with types the accent and the letter);
//! - everything else is nothing: releases, the "still held" events after a reset, keys held
//!   with Ctrl, Alt or the Windows key (those are shortcuts, and the console has none),
//!   pointers, and events that make no sense.
//!
//! Until M6 the console read keyboards itself and this file held the US layout's Shift rules.

use crate::editor::Key;
use pbr_devices::keys::key;
use pbr_seat::event::{Event, kind, value};

pub use pbr_seat::event::EVENT_SIZE;

/// The keys one event from the seat's ring is: none, one or two, in order.
///
/// An array of two `Option`s rather than a list, so that the console's key thread never
/// allocates memory for a key. `into_iter().flatten()` walks the ones that are there.
pub fn keys(event: &[u8; EVENT_SIZE]) -> [Option<Key>; 2] {
    let e = Event::decode(event);
    if e.kind != kind::KEY || !matches!(e.value, value::PRESSED | value::REPEAT) {
        return [None, None];
    }
    // Editing keys are known by where they are, in every layout, and work whatever
    // modifiers are held. (With Num Lock off the input service reports the keypad's keys as
    // the arrows and editing keys they are, so they need nothing here.)
    let editing = match e.code {
        key::ENTER | key::KP_ENTER => Some(Key::Enter),
        key::BACKSPACE => Some(Key::Backspace),
        key::DELETE => Some(Key::Delete),
        key::LEFT => Some(Key::Left),
        key::RIGHT => Some(Key::Right),
        key::UP => Some(Key::Up),
        key::DOWN => Some(Key::Down),
        key::HOME => Some(Key::Home),
        key::END => Some(Key::End),
        _ => None,
    };
    if editing.is_some() {
        return [editing, None];
    }
    if !e.types() {
        return [None, None];
    }
    let mut chars = e.chars().map(Key::Char);
    [chars.next(), chars.next()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pbr_seat::event::mods;

    fn event(code: u16, value: i32, mods: u16, first: char, second: char) -> [u8; EVENT_SIZE] {
        let number = |c: char| if c == '\0' { 0 } else { c as u32 };
        Event {
            kind: kind::KEY,
            device: 1,
            code,
            mods,
            value,
            first: number(first),
            second: number(second),
            time: 5,
        }
        .encode()
    }

    fn typed(events: &[[u8; EVENT_SIZE]]) -> Vec<Key> {
        events.iter().flat_map(|e| keys(e).into_iter().flatten()).collect()
    }

    #[test]
    fn a_press_types_its_characters_and_a_release_nothing() {
        let down = event(key::A, value::PRESSED, 0, 'a', '\0');
        let up = event(key::A, value::RELEASED, 0, '\0', '\0');
        assert_eq!(typed(&[down, up]), [Key::Char('a')]);
        // Characters from any layout, not only ASCII.
        let pound = event(key::D3, value::PRESSED, mods::SHIFT, '£', '\0');
        let e_hat = event(key::E, value::PRESSED, 0, 'ê', '\0');
        assert_eq!(typed(&[pound, e_hat]), [Key::Char('£'), Key::Char('ê')]);
        // A dead key and a letter it doesn't combine with: two characters in one event.
        let both = event(key::X, value::PRESSED, 0, '^', 'x');
        assert_eq!(typed(&[both]), [Key::Char('^'), Key::Char('x')]);
    }

    #[test]
    fn repeats_type_and_held_keys_do_not() {
        let repeat = event(key::A, value::REPEAT, 0, 'a', '\0');
        assert_eq!(typed(&[repeat, repeat]), [Key::Char('a'), Key::Char('a')]);
        // "Still held" after a reset stands for a press that was acted on already.
        assert_eq!(typed(&[event(key::A, value::HELD, 0, '\0', '\0')]), []);
        assert_eq!(typed(&[event(key::ENTER, value::HELD, 0, '\0', '\0')]), []);
        // A value that is none of those.
        assert_eq!(typed(&[event(key::A, 9, 0, 'a', '\0')]), []);
    }

    #[test]
    fn editing_keys_are_known_by_their_place() {
        let cases = [
            (key::ENTER, Key::Enter),
            (key::KP_ENTER, Key::Enter),
            (key::BACKSPACE, Key::Backspace),
            (key::DELETE, Key::Delete),
            (key::LEFT, Key::Left),
            (key::RIGHT, Key::Right),
            (key::UP, Key::Up),
            (key::DOWN, Key::Down),
            (key::HOME, Key::Home),
            (key::END, Key::End),
        ];
        for (code, expected) in cases {
            assert_eq!(typed(&[event(code, value::PRESSED, 0, '\0', '\0')]), [expected]);
            assert_eq!(typed(&[event(code, value::REPEAT, 0, '\0', '\0')]), [expected]);
            assert_eq!(typed(&[event(code, value::RELEASED, 0, '\0', '\0')]), []);
            // They work with a modifier held (Shift+Backspace is still Backspace).
            assert_eq!(
                typed(&[event(code, value::PRESSED, mods::SHIFT | mods::CTRL, '\0', '\0')]),
                [expected]
            );
        }
        // Keys that neither type nor edit a line are nothing.
        for code in [key::F1, key::ESCAPE, key::TAB, key::PAGE_UP, key::LEFT_SHIFT, key::CAPS_LOCK]
        {
            assert_eq!(typed(&[event(code, value::PRESSED, 0, '\0', '\0')]), []);
        }
    }

    #[test]
    fn shortcuts_do_not_type() {
        // Ctrl+S carries `s` so that programs can tell which shortcut it is; it isn't typed.
        for held in [mods::CTRL, mods::ALT, mods::META, mods::CTRL | mods::SHIFT] {
            assert_eq!(typed(&[event(key::S, value::PRESSED, held, 's', '\0')]), []);
        }
        // Shift, AltGr and the locks choose the character; they don't stop it.
        let held = mods::SHIFT | mods::ALTGR | mods::CAPS_LOCK | mods::NUM_LOCK;
        assert_eq!(typed(&[event(key::E, value::PRESSED, held, 'É', '\0')]), [Key::Char('É')]);
    }

    #[test]
    fn nothing_in_the_ring_can_type_what_is_not_a_character() {
        // Other kinds of event, whatever their fields hold.
        for k in [
            kind::LOST,
            kind::MOVE,
            kind::BUTTON,
            kind::SCROLL,
            kind::POSITION,
            kind::RESET,
            kind::DEVICES,
            99,
        ] {
            let e = Event {
                kind: k,
                code: key::A,
                value: value::PRESSED,
                first: 'a' as u32,
                ..Event::default()
            };
            assert_eq!(typed(&[e.encode()]), []);
        }
        // Numbers that are no character, and control characters.
        let odd = |first: u32, second: u32| {
            Event {
                kind: kind::KEY,
                code: key::A,
                value: value::PRESSED,
                first,
                second,
                ..Event::default()
            }
            .encode()
        };
        assert_eq!(typed(&[odd(0xD800, 0x11_0000), odd(0x1B, 0x7F), odd(0, 0)]), []);
        assert_eq!(typed(&[odd(u32::MAX, 'b' as u32)]), [Key::Char('b')]);
        // Random bytes: never a panic, never a control character.
        let mut x = 0x1234_5678_9ABC_DEF1u64;
        for _ in 0..20_000 {
            let mut e = [0u8; EVENT_SIZE];
            for b in &mut e {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
            e[0] = 1;
            e[1] = 0;
            for k in keys(&e).into_iter().flatten() {
                if let Key::Char(c) = k {
                    assert!(!c.is_control());
                }
            }
        }
    }
}
