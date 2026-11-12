// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests of the seat and the outbox together, with the real layout files and a clock the
//! tests move by hand (docs/design/0021-input-and-keyboard-layouts.md, section 9).

use crate::event::{EVENT_SIZE, Event, kind, mods, value};
use crate::outbox::{Broken, Outbox, Sink};
use crate::seat::{
    DeviceKind, MAX_DEVICES, RAW_SIZE, REPEAT_DELAY_NS, REPEAT_PERIOD_NS, Seat, Source,
};
use pbr_devices::keys::key;
use pbr_keymap::Layout;

const US: &str = include_str!("../../../services/input/layouts/us.kbd");
const GB: &str = include_str!("../../../services/input/layouts/gb.kbd");
const US_INTL: &str = include_str!("../../../services/input/layouts/us-intl.kbd");

fn layout(text: &str) -> Layout {
    pbr_keymap::parse(text.as_bytes()).expect("the layout file is good")
}

/// A driver's event.
fn raw(what: u16, code: u16, amount: i32, time: u64) -> [u8; RAW_SIZE] {
    let mut e = [0u8; RAW_SIZE];
    e[0..2].copy_from_slice(&what.to_le_bytes());
    e[2..4].copy_from_slice(&code.to_le_bytes());
    e[4..8].copy_from_slice(&amount.to_le_bytes());
    e[8..16].copy_from_slice(&time.to_le_bytes());
    e
}

/// A seat with one keyboard, and helpers that press and release keys on it at a time the
/// test sets.
struct Typist {
    seat: Seat,
    keyboard: u32,
    now: u64,
}

impl Typist {
    fn new(text: &str) -> Typist {
        let mut seat = Seat::new(layout(text));
        let mut out = Vec::new();
        let keyboard = seat.add(Source::Keyboard, "Test keyboard", 7, 0, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, kind::DEVICES);
        Typist { seat, keyboard, now: 1_000 }
    }

    fn feed(&mut self, source: u32, what: u16, code: u16, amount: i32) -> Vec<Event> {
        let mut out = Vec::new();
        self.seat.raw(source, &raw(what, code, amount, self.now), &mut out);
        out
    }

    fn press(&mut self, code: u16) -> Vec<Event> {
        self.feed(self.keyboard, kind::KEY, code, value::PRESSED)
    }

    fn release(&mut self, code: u16) -> Vec<Event> {
        self.feed(self.keyboard, kind::KEY, code, value::RELEASED)
    }

    /// Presses and releases a key; the characters the press typed.
    fn tap(&mut self, code: u16) -> String {
        let down = self.press(code);
        self.release(code);
        typed(&down)
    }

    /// Presses a key with others held round it.
    fn chord(&mut self, held: &[u16], code: u16) -> String {
        for h in held {
            self.press(*h);
        }
        let s = self.tap(code);
        for h in held.iter().rev() {
            self.release(*h);
        }
        s
    }

    /// Moves the clock on and makes the repeats that fall due, a tick at a time as the input
    /// service would.
    fn wait(&mut self, ns: u64) -> Vec<Event> {
        let end = self.now + ns;
        let mut out = Vec::new();
        while let Some(due) = self.seat.next_repeat().filter(|d| *d <= end) {
            self.now = due;
            self.seat.tick(due, &mut out);
        }
        self.now = end;
        out
    }
}

/// The characters a list of events types: those of presses and repeats that type.
fn typed(events: &[Event]) -> String {
    events.iter().filter(|e| e.types()).flat_map(|e| e.chars()).collect()
}

fn keys(events: &[Event]) -> Vec<(u16, i32)> {
    events.iter().filter(|e| e.kind == kind::KEY).map(|e| (e.code, e.value)).collect()
}

// ------------------------------------------------------------------------------------------
// Keys and characters

#[test]
fn keys_type_what_the_layout_says() {
    let mut t = Typist::new(US);
    assert_eq!(t.tap(key::H), "h");
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::H), "H");
    assert_eq!(t.chord(&[key::RIGHT_SHIFT], key::D3), "#");
    assert_eq!(t.tap(key::SPACE), " ");
    // Keys that type nothing carry no characters.
    for code in [key::ENTER, key::F1, key::LEFT, key::BACKSPACE, key::ESCAPE, key::TAB] {
        let down = t.press(code);
        assert_eq!((down.len(), down[0].first, down[0].second), (1, 0, 0), "{code}");
        t.release(code);
    }
    let mut t = Typist::new(GB);
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::D3), "£");
    assert_eq!(t.chord(&[key::RIGHT_ALT], key::D4), "€");
    assert_eq!(t.tap(key::K102ND), "\\");
}

#[test]
fn a_press_and_its_release_carry_the_device_the_time_and_the_modifiers() {
    let mut t = Typist::new(US);
    let id = t.seat.devices().next().unwrap().id;
    assert_ne!(id, 0);
    t.now = 5_000;
    let shift = t.press(key::LEFT_SHIFT);
    assert_eq!(shift.len(), 1);
    let e = shift[0];
    assert_eq!(
        (e.kind, e.device, e.code, e.value, e.time),
        (kind::KEY, id, key::LEFT_SHIFT, 1, 5_000)
    );
    // "Modifiers held after this event": Shift's own press already shows Shift.
    assert_eq!(e.mods, mods::SHIFT | mods::NUM_LOCK);
    t.now = 6_000;
    let a = t.press(key::A)[0];
    assert_eq!(
        (a.first, a.second, a.mods, a.time),
        ('A' as u32, 0, mods::SHIFT | mods::NUM_LOCK, 6_000)
    );
    let up = t.release(key::A)[0];
    assert_eq!((up.value, up.first, up.second), (value::RELEASED, 0, 0));
    let up = t.release(key::LEFT_SHIFT)[0];
    assert_eq!(up.mods, mods::NUM_LOCK, "Shift's release no longer shows Shift");
}

#[test]
fn every_modifier_has_its_bit_and_right_alt_depends_on_the_layout() {
    for (text, right_alt) in [(US, mods::ALT), (GB, mods::ALTGR), (US_INTL, mods::ALTGR)] {
        let mut t = Typist::new(text);
        for (code, bit) in [
            (key::LEFT_SHIFT, mods::SHIFT),
            (key::RIGHT_SHIFT, mods::SHIFT),
            (key::LEFT_CTRL, mods::CTRL),
            (key::RIGHT_CTRL, mods::CTRL),
            (key::LEFT_ALT, mods::ALT),
            (key::RIGHT_ALT, right_alt),
            (key::LEFT_META, mods::META),
            (key::RIGHT_META, mods::META),
        ] {
            assert_eq!(t.press(code)[0].mods, bit | mods::NUM_LOCK, "{code}");
            assert_eq!(t.release(code)[0].mods, mods::NUM_LOCK, "{code}");
        }
        // Both Shift keys held: letting one go leaves Shift held.
        t.press(key::LEFT_SHIFT);
        t.press(key::RIGHT_SHIFT);
        assert_eq!(t.release(key::LEFT_SHIFT)[0].mods & mods::SHIFT, mods::SHIFT);
        assert_eq!(t.release(key::RIGHT_SHIFT)[0].mods & mods::SHIFT, 0);
    }
}

#[test]
fn with_ctrl_alt_or_meta_the_characters_are_given_but_do_not_type() {
    let mut t = Typist::new(US);
    for held in [key::LEFT_CTRL, key::LEFT_ALT, key::RIGHT_ALT, key::LEFT_META] {
        t.press(held);
        let s = t.press(key::S)[0];
        assert_eq!(s.first, 's' as u32, "{held}: the shortcut's letter");
        assert!(!s.types(), "{held}");
        t.release(key::S);
        t.release(held);
    }
    // On a layout with AltGr, the right Alt key chooses a character and it does type.
    let mut t = Typist::new(GB);
    t.press(key::RIGHT_ALT);
    assert!(t.press(key::E)[0].types());
}

#[test]
fn caps_lock_and_num_lock_are_toggles_kept_for_the_whole_seat() {
    let mut t = Typist::new(US);
    let mut out = Vec::new();
    let second = t.seat.add(Source::Keyboard, "Second", 8, 0, &mut out).unwrap();
    // Caps Lock on the first keyboard changes what the second types.
    let caps = t.press(key::CAPS_LOCK)[0];
    assert_eq!(caps.mods, mods::CAPS_LOCK | mods::NUM_LOCK);
    t.release(key::CAPS_LOCK);
    let a = t.feed(second, kind::KEY, key::A, value::PRESSED);
    assert_eq!(typed(&a), "A");
    assert_eq!(a[0].mods, mods::CAPS_LOCK | mods::NUM_LOCK);
    t.feed(second, kind::KEY, key::A, value::RELEASED);
    // Caps Lock doesn't change digits, and Shift undoes it for letters.
    assert_eq!(t.tap(key::D1), "1");
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::A), "a");
    // Holding it down doesn't toggle again (the keyboard's own repeats are dropped).
    t.press(key::CAPS_LOCK);
    assert!(t.feed(t.keyboard, kind::KEY, key::CAPS_LOCK, value::REPEAT).is_empty());
    t.release(key::CAPS_LOCK);
    assert_eq!(t.tap(key::A), "a", "the second press turned it off");
}

#[test]
fn the_keypad_types_digits_with_num_lock_and_edits_without() {
    let mut t = Typist::new(US);
    assert_eq!(t.tap(key::KP_7), "7");
    assert_eq!(t.tap(key::KP_PERIOD), ".");
    let off = t.press(key::NUM_LOCK)[0];
    assert_eq!(off.mods & mods::NUM_LOCK, 0);
    t.release(key::NUM_LOCK);
    // Off: the key is reported as the editing key it is, press and release, and types nothing.
    let down = t.press(key::KP_7);
    assert_eq!(keys(&down), [(key::HOME, value::PRESSED)]);
    assert_eq!(typed(&down), "");
    assert_eq!(keys(&t.release(key::KP_7)), [(key::HOME, value::RELEASED)]);
    assert_eq!(keys(&t.press(key::KP_4)), [(key::LEFT, value::PRESSED)]);
    // The keys that are the same either way.
    assert_eq!(t.tap(key::KP_PLUS), "+");
    assert_eq!(keys(&t.press(key::KP_5)), [(key::KP_5, value::PRESSED)]);
    t.release(key::KP_5);
    t.release(key::KP_4);
    // Num Lock turned on while a keypad key is held: its release still names the key its
    // press did.
    t.press(key::KP_1);
    t.tap(key::NUM_LOCK);
    assert_eq!(keys(&t.release(key::KP_1)), [(key::END, value::RELEASED)]);
    assert_eq!(t.tap(key::KP_1), "1");
}

// ------------------------------------------------------------------------------------------
// Dead keys

#[test]
fn dead_keys_combine_with_the_next_letter() {
    let mut t = Typist::new(US_INTL);
    // Circumflex is Shift+6, diaeresis Shift+apostrophe, acute the apostrophe alone.
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::D6), "", "a dead key types nothing by itself");
    assert_eq!(t.tap(key::E), "ê");
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::APOSTROPHE), "");
    assert_eq!(t.tap(key::E), "ë");
    assert_eq!(t.tap(key::APOSTROPHE), "");
    assert_eq!(t.chord(&[key::LEFT_SHIFT], key::O), "Ó", "Shift pressed in between is no key");
    // A letter it doesn't combine with: the accent, then the letter, in one event.
    t.chord(&[key::LEFT_SHIFT], key::D6);
    let x = t.press(key::X);
    assert_eq!((x[0].first, x[0].second), ('^' as u32, 'x' as u32));
    t.release(key::X);
    // Space gives the accent; the dead key twice gives it once.
    t.tap(key::APOSTROPHE);
    assert_eq!(t.tap(key::SPACE), "'");
    t.tap(key::APOSTROPHE);
    assert_eq!(t.tap(key::APOSTROPHE), "'");
    assert_eq!(t.tap(key::A), "a", "nothing is left waiting");
}

#[test]
fn a_waiting_dead_key_is_dropped_by_editing_keys_and_a_change_of_layout() {
    let mut t = Typist::new(US_INTL);
    for code in [key::BACKSPACE, key::LEFT, key::ENTER, key::ESCAPE, key::F5] {
        t.tap(key::APOSTROPHE);
        t.tap(code);
        assert_eq!(t.tap(key::E), "e", "after {code}");
    }
    // Modifiers and locks leave it waiting.
    t.tap(key::APOSTROPHE);
    t.tap(key::LEFT_CTRL);
    t.tap(key::CAPS_LOCK);
    assert_eq!(t.tap(key::E), "É");
    t.tap(key::CAPS_LOCK);
    // A shortcut in the middle neither uses the dead key up nor is changed by it.
    t.tap(key::APOSTROPHE);
    t.press(key::LEFT_CTRL);
    let e = t.press(key::E)[0];
    assert_eq!((e.first, e.second), ('e' as u32, 0));
    t.release(key::E);
    // A dead key pressed as a shortcut carries its accent.
    let quote = t.press(key::APOSTROPHE)[0];
    assert_eq!(quote.first, '\'' as u32);
    t.release(key::APOSTROPHE);
    t.release(key::LEFT_CTRL);
    assert_eq!(t.tap(key::E), "é", "the first dead key was still waiting");
    // The layout changing drops it (note 0021, section 5).
    t.tap(key::APOSTROPHE);
    t.seat.set_layout(layout(US));
    assert_eq!(t.tap(key::E), "e");
    assert_eq!(t.tap(key::APOSTROPHE), "'");
}

// ------------------------------------------------------------------------------------------
// Repeat

#[test]
fn a_held_key_repeats_after_half_a_second_thirty_times_a_second() {
    let mut t = Typist::new(US);
    t.now = 1_000_000;
    t.press(key::A);
    assert_eq!(t.seat.next_repeat(), Some(1_000_000 + REPEAT_DELAY_NS));
    assert!(t.wait(REPEAT_DELAY_NS - 1).is_empty(), "nothing before the delay is up");
    let first = t.wait(1);
    assert_eq!(first.len(), 1);
    assert_eq!(
        (first[0].value, first[0].first, first[0].time),
        (value::REPEAT, 'a' as u32, 1_000_000 + REPEAT_DELAY_NS)
    );
    // One second more: 30 repeats, evenly spaced.
    let second = t.wait(1_000_000_000);
    assert_eq!(second.len(), 30);
    for pair in second.windows(2) {
        assert_eq!(pair[1].time - pair[0].time, REPEAT_PERIOD_NS);
    }
    assert_eq!(typed(&second), "a".repeat(30));
    // Released: it stops.
    t.release(key::A);
    assert_eq!(t.seat.next_repeat(), None);
    assert!(t.wait(2_000_000_000).is_empty());
}

#[test]
fn the_last_key_pressed_is_the_one_that_repeats() {
    let mut t = Typist::new(US);
    t.press(key::A);
    t.wait(REPEAT_DELAY_NS / 2);
    t.press(key::B);
    // B's own half second starts at its press.
    assert!(t.wait(REPEAT_DELAY_NS - 1).is_empty());
    assert_eq!(typed(&t.wait(1)), "b");
    assert_eq!(typed(&t.wait(REPEAT_PERIOD_NS)), "b");
    // Letting go of A, which isn't repeating, changes nothing.
    t.release(key::A);
    assert_eq!(typed(&t.wait(REPEAT_PERIOD_NS)), "b");
    // Letting go of B stops the repeating.
    t.release(key::B);
    assert_eq!(t.seat.next_repeat(), None);
    // And when the last key pressed is let go while an earlier one is still down, the
    // earlier one doesn't start again: nothing repeats until another press.
    t.press(key::A);
    t.wait(10);
    t.press(key::B);
    t.release(key::B);
    assert_eq!(t.seat.next_repeat(), None);
    assert!(t.wait(2 * REPEAT_DELAY_NS).is_empty());
}

#[test]
fn modifiers_never_repeat_and_do_not_interrupt_a_repeat() {
    let mut t = Typist::new(US);
    for code in [
        key::LEFT_SHIFT,
        key::RIGHT_CTRL,
        key::LEFT_ALT,
        key::RIGHT_META,
        key::CAPS_LOCK,
        key::F1,
        key::ESCAPE,
        key::INSERT,
    ] {
        t.press(code);
        assert_eq!(t.seat.next_repeat(), None, "{code}");
        assert!(t.wait(3 * REPEAT_DELAY_NS).is_empty(), "{code}");
        t.release(code);
    }
    // (The loop left Caps Lock on: off again.)
    t.tap(key::CAPS_LOCK);
    // Hold `a`, then press Shift while it repeats: it goes on, as `A`.
    t.press(key::A);
    assert_eq!(typed(&t.wait(REPEAT_DELAY_NS)), "a");
    t.press(key::LEFT_SHIFT);
    assert_eq!(typed(&t.wait(REPEAT_PERIOD_NS)), "A");
    t.release(key::LEFT_SHIFT);
    assert_eq!(typed(&t.wait(REPEAT_PERIOD_NS)), "a");
    // A key that neither types nor edits takes the repeating away without repeating itself.
    t.press(key::F1);
    assert_eq!(t.seat.next_repeat(), None);
}

#[test]
fn editing_keys_repeat_and_dead_keys_do_not() {
    let mut t = Typist::new(US_INTL);
    for code in [
        key::BACKSPACE,
        key::DELETE,
        key::ENTER,
        key::KP_ENTER,
        key::TAB,
        key::LEFT,
        key::RIGHT,
        key::UP,
        key::DOWN,
        key::HOME,
        key::END,
        key::PAGE_UP,
        key::PAGE_DOWN,
    ] {
        t.press(code);
        let r = t.wait(REPEAT_DELAY_NS + REPEAT_PERIOD_NS);
        assert_eq!(keys(&r), [(code, value::REPEAT), (code, value::REPEAT)], "{code}");
        t.release(code);
    }
    t.press(key::APOSTROPHE);
    assert_eq!(t.seat.next_repeat(), None, "a dead key types nothing, so it doesn't repeat");
    t.release(key::APOSTROPHE);
    t.tap(key::SPACE);
    // The keypad with Num Lock off repeats as the editing key it is.
    t.tap(key::NUM_LOCK);
    t.press(key::KP_4);
    assert_eq!(keys(&t.wait(REPEAT_DELAY_NS)), [(key::LEFT, value::REPEAT)]);
}

#[test]
fn the_keyboards_own_repeats_are_dropped() {
    let mut t = Typist::new(US);
    t.press(key::A);
    // A PS/2 keyboard sends the press again and again; a driver marks those as repeats, and
    // one that didn't would send presses of a key that is down.
    assert!(t.feed(t.keyboard, kind::KEY, key::A, value::REPEAT).is_empty());
    assert!(t.feed(t.keyboard, kind::KEY, key::A, value::PRESSED).is_empty());
    // They don't put the seat's own repeat off either.
    assert_eq!(typed(&t.wait(REPEAT_DELAY_NS)), "a");
    // A "repeat" of a key that isn't down (its press was missed) counts as its press.
    let b = t.feed(t.keyboard, kind::KEY, key::B, value::REPEAT);
    assert_eq!(keys(&b), [(key::B, value::PRESSED)]);
    // A release of a key that isn't down is nothing.
    assert!(t.release(key::C).is_empty());
}

#[test]
fn a_late_tick_makes_one_repeat_not_a_burst() {
    let mut t = Typist::new(US);
    t.now = 0;
    t.press(key::A);
    let mut out = Vec::new();
    // The service was busy for ten seconds.
    t.seat.tick(10_000_000_000, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].time, 10_000_000_000);
    assert_eq!(t.seat.next_repeat(), Some(10_000_000_000 + REPEAT_PERIOD_NS));
    // A tick that comes early does nothing.
    t.seat.tick(10_000_000_001, &mut out);
    assert_eq!(out.len(), 1);
}

// ------------------------------------------------------------------------------------------
// Several devices, losses and devices going

#[test]
fn each_keyboard_has_its_own_shift() {
    let mut t = Typist::new(US);
    let mut out = Vec::new();
    let second = t.seat.add(Source::Keyboard, "Second", 8, 0, &mut out).unwrap();
    let ids: Vec<u16> = t.seat.devices().map(|d| d.id).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    t.press(key::LEFT_SHIFT);
    let a = t.feed(second, kind::KEY, key::A, value::PRESSED);
    assert_eq!(typed(&a), "a", "Shift on the first keyboard isn't Shift on the second");
    assert_eq!(a[0].device, ids[1]);
    assert_eq!(a[0].mods & mods::SHIFT, 0);
    assert_eq!(typed(&t.press(key::B)), "B");
}

#[test]
fn pointers_pass_through_with_the_keyboards_modifiers() {
    let mut t = Typist::new(US);
    let mut out = Vec::new();
    let mouse = t.seat.add(Source::Mouse, "Mouse", 9, 0, &mut out).unwrap();
    let tablet = t.seat.add(Source::Tablet, "Tablet", 10, 0, &mut out).unwrap();
    let id = |t: &Typist, kind: DeviceKind| t.seat.devices().find(|d| d.kind == kind).unwrap().id;
    let (mouse_id, tablet_id) = (id(&t, DeviceKind::Mouse), id(&t, DeviceKind::Tablet));
    t.press(key::LEFT_CTRL);
    let moved = t.feed(mouse, kind::MOVE, 1, -5);
    assert_eq!(moved.len(), 1);
    assert_eq!(
        (moved[0].kind, moved[0].device, moved[0].code, moved[0].value),
        (kind::MOVE, mouse_id, 1, -5)
    );
    assert_eq!(moved[0].mods, mods::CTRL | mods::NUM_LOCK, "Ctrl+click needs Ctrl on the click");
    let wheel = t.feed(mouse, kind::SCROLL, 4, 2);
    assert_eq!((wheel[0].kind, wheel[0].code, wheel[0].value), (kind::SCROLL, 4, 2));
    // A button: once down, once up; a second "down" is nothing.
    assert_eq!(t.feed(mouse, kind::BUTTON, 1, value::PRESSED).len(), 1);
    assert!(t.feed(mouse, kind::BUTTON, 1, value::PRESSED).is_empty());
    assert_eq!(t.feed(mouse, kind::BUTTON, 1, value::RELEASED).len(), 1);
    assert!(t.feed(mouse, kind::BUTTON, 1, value::RELEASED).is_empty());
    assert!(t.feed(mouse, kind::BUTTON, 0, value::PRESSED).is_empty());
    assert!(t.feed(mouse, kind::BUTTON, 9, value::PRESSED).is_empty());
    // A tablet's place: packed into one number by the driver, two fields here.
    let packed = (40000u32 | (65535u32 << 16)) as i32;
    let place = t.feed(tablet, kind::POSITION, 0, packed);
    assert_eq!(place[0].position(), Some((40000, 65535)));
    assert_eq!(place[0].device, tablet_id);
    // Events a device can't send are skipped: a key from a mouse, a move from a keyboard, a
    // position from a mouse.
    assert!(t.feed(mouse, kind::KEY, key::A, value::PRESSED).is_empty());
    assert!(t.feed(t.keyboard, kind::MOVE, 1, 3).is_empty());
    assert!(t.feed(mouse, kind::POSITION, 0, 5).is_empty());
    // And so are unknown kinds, codes and values, and sources that don't exist.
    assert!(t.feed(t.keyboard, 99, 1, 1).is_empty());
    assert!(t.feed(t.keyboard, kind::KEY, 0, value::PRESSED).is_empty());
    assert!(t.feed(t.keyboard, kind::KEY, 128, value::PRESSED).is_empty());
    assert!(t.feed(t.keyboard, kind::KEY, key::A, 7).is_empty());
    assert!(t.feed(12345, kind::KEY, key::A, value::PRESSED).is_empty());
}

#[test]
fn a_keyboard_and_mouse_in_one_is_two_devices() {
    let mut seat = Seat::new(layout(US));
    let mut out = Vec::new();
    let both =
        seat.add(Source::KeyboardAndMouse, "PS/2 keyboard and mouse", 3, 0, &mut out).unwrap();
    let kinds: Vec<DeviceKind> = seat.devices().map(|d| d.kind).collect();
    assert_eq!(kinds, [DeviceKind::Keyboard, DeviceKind::Mouse]);
    assert!(seat.devices().all(|d| d.device == 3 && d.name == "PS/2 keyboard and mouse"));
    let ids: Vec<u16> = seat.devices().map(|d| d.id).collect();
    out.clear();
    seat.raw(both, &raw(kind::KEY, key::A, 1, 1), &mut out);
    seat.raw(both, &raw(kind::MOVE, 2, 4, 2), &mut out);
    assert_eq!((out[0].device, out[1].device), (ids[0], ids[1]));
    // Removing the source removes both.
    out.clear();
    seat.remove(both, 3, &mut out);
    assert_eq!(seat.devices().count(), 0);
    assert_eq!(out.first().map(|e| e.kind), Some(kind::RESET));
    assert_eq!(out.last().map(|e| e.kind), Some(kind::DEVICES));
}

#[test]
fn a_key_released_during_a_loss_does_not_stay_held() {
    let mut t = Typist::new(US);
    let mut out = Vec::new();
    let second = t.seat.add(Source::Keyboard, "Second", 8, 0, &mut out).unwrap();
    let ids: Vec<u16> = t.seat.devices().map(|d| d.id).collect();
    t.press(key::LEFT_SHIFT);
    t.press(key::A);
    t.feed(second, kind::KEY, key::LEFT_CTRL, value::PRESSED);
    // The first keyboard's driver says it dropped events: the releases of Shift and A may be
    // among them.
    let after = t.feed(t.keyboard, kind::LOST, 0, 5);
    assert_eq!(after[0].kind, kind::RESET);
    // What is held on the other keyboard is said again, as held, not as a new press.
    assert_eq!(after.len(), 2);
    assert_eq!(
        (after[1].device, after[1].code, after[1].value),
        (ids[1], key::LEFT_CTRL, value::HELD)
    );
    assert!(!after[1].types());
    // Nothing of the first keyboard is held, and its repeat has stopped.
    assert_eq!(t.seat.next_repeat(), None);
    assert!(t.wait(2 * REPEAT_DELAY_NS).is_empty());
    assert_eq!(typed(&t.press(key::B)), "b", "Shift is no longer held");
    // The releases that were not lost after all arrive late and are nothing.
    assert!(t.release(key::A).is_empty());
    assert!(t.release(key::LEFT_SHIFT).is_empty());
}

#[test]
fn a_device_going_lets_go_of_its_keys_and_tells_the_consumer() {
    let mut t = Typist::new(US);
    let mut out = Vec::new();
    let mouse = t.seat.add(Source::Mouse, "Mouse", 9, 0, &mut out).unwrap();
    t.feed(mouse, kind::BUTTON, 2, value::PRESSED);
    t.press(key::A);
    assert!(t.seat.next_repeat().is_some());
    let old_id = t.seat.devices().find(|d| d.kind == DeviceKind::Keyboard).unwrap().id;
    out.clear();
    t.seat.remove(t.keyboard, 77, &mut out);
    let kinds: Vec<u16> = out.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [kind::RESET, kind::BUTTON, kind::DEVICES]);
    assert_eq!((out[1].code, out[1].value, out[1].time), (2, value::HELD, 77));
    assert_eq!(t.seat.next_repeat(), None);
    assert_eq!(t.seat.devices().count(), 1);
    // Its number means nothing now.
    assert!(t.press(key::B).is_empty());
    let mut again = Vec::new();
    t.seat.remove(t.keyboard, 78, &mut again);
    assert!(again.is_empty(), "removing it twice does nothing");
    // The same keyboard coming back is a new device with a new id.
    let back = t.seat.add(Source::Keyboard, "Test keyboard", 7, 79, &mut again).unwrap();
    assert_ne!(back, t.keyboard);
    let new_id = t.seat.devices().find(|d| d.kind == DeviceKind::Keyboard).unwrap().id;
    assert_ne!(new_id, old_id);
}

#[test]
fn the_seat_is_full_at_32_devices_and_names_are_cut_safely() {
    let mut seat = Seat::new(layout(US));
    let mut out = Vec::new();
    for i in 0..MAX_DEVICES - 1 {
        assert!(seat.add(Source::Mouse, "m", i as u32, 0, &mut out).is_some());
    }
    // One place left: a keyboard and mouse in one needs two.
    assert_eq!(seat.add(Source::KeyboardAndMouse, "both", 0, 0, &mut out), None);
    assert!(seat.add(Source::Keyboard, "k", 0, 0, &mut out).is_some());
    assert_eq!(seat.add(Source::Keyboard, "k", 0, 0, &mut out), None);
    assert_eq!(seat.devices().count(), MAX_DEVICES);
    let mut ids: Vec<u16> = seat.devices().map(|d| d.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), MAX_DEVICES, "no id twice");
    assert!(!ids.contains(&0));
    // A name longer than 64 bytes is cut at a character, never inside one.
    let mut seat = Seat::new(layout(US));
    let long = "é".repeat(40); // 80 bytes
    seat.add(Source::Keyboard, &long, 0, 0, &mut out).unwrap();
    assert_eq!(seat.devices().next().unwrap().name, "é".repeat(32));
    let odd = format!("a{}", "é".repeat(40));
    seat.add(Source::Keyboard, &odd, 0, 0, &mut out).unwrap();
    assert_eq!(seat.devices().nth(1).unwrap().name.len(), 63);
}

#[test]
fn nothing_a_driver_sends_can_upset_the_seat() {
    // Random bytes as events, from every kind of source, with devices coming and going: no
    // panic, every id an event carries is a device's or 0, and no key is typed by an event
    // that says it is only held.
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut seat = Seat::new(layout(US_INTL));
    let mut sources: Vec<u32> = Vec::new();
    let mut now = 0u64;
    for round in 0..20_000 {
        let mut out = Vec::new();
        now += next() % 50_000_000;
        match next() % 40 {
            0 => {
                let source =
                    [Source::Keyboard, Source::Mouse, Source::KeyboardAndMouse, Source::Tablet]
                        [(next() % 4) as usize];
                if let Some(s) = seat.add(source, "device", round, now, &mut out) {
                    sources.push(s);
                }
            }
            1 if !sources.is_empty() => {
                let s = sources.swap_remove((next() % sources.len() as u64) as usize);
                seat.remove(s, now, &mut out);
            }
            2 => seat.tick(now, &mut out),
            _ => {
                let mut e = [0u8; RAW_SIZE];
                for b in &mut e {
                    *b = next() as u8;
                }
                // Mostly plausible kinds, codes and values, so that state builds up.
                if next() % 4 != 0 {
                    e[0] = (next() % 7) as u8;
                    e[1] = 0;
                    e[2] = (next() % 110) as u8;
                    e[3] = 0;
                    e[4..8].copy_from_slice(&((next() % 4) as i32).to_le_bytes());
                }
                let source = if sources.is_empty() || next() % 50 == 0 {
                    next() as u32
                } else {
                    sources[(next() % sources.len() as u64) as usize]
                };
                seat.raw(source, &e, &mut out);
            }
        }
        let ids: Vec<u16> = seat.devices().map(|d| d.id).collect();
        for e in &out {
            assert!(e.device == 0 || ids.contains(&e.device), "round {round}: {e:?}");
            assert!(e.kind <= kind::DEVICES);
            if e.value == value::HELD {
                assert_eq!((e.first, e.second), (0, 0));
            }
            // Whatever is in the character fields of a key event is a character.
            if e.kind == kind::KEY {
                for c in [e.first, e.second] {
                    assert!(c == 0 || char::from_u32(c).is_some_and(|c| !c.is_control()));
                }
            }
        }
        assert!(seat.devices().count() <= MAX_DEVICES);
    }
}

// ------------------------------------------------------------------------------------------
// The outbox

/// A pretend ring with room for `room` events.
struct Ring {
    room: usize,
    events: Vec<Event>,
    broken: bool,
}

impl Ring {
    fn new(room: usize) -> Ring {
        Ring { room, events: Vec::new(), broken: false }
    }

    /// The consumer reads everything.
    fn drain(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.events)
    }
}

impl Sink for Ring {
    fn free(&self) -> usize {
        self.room - self.events.len()
    }

    fn push(&mut self, event: &[u8; EVENT_SIZE]) -> bool {
        assert!(self.events.len() < self.room, "pushed into a full ring");
        if self.broken {
            return false;
        }
        self.events.push(Event::decode(event));
        true
    }
}

#[test]
fn a_new_consumer_is_told_what_is_held() {
    let mut t = Typist::new(US);
    t.press(key::LEFT_SHIFT);
    let mut ring = Ring::new(16);
    let mut outbox = Outbox::new();
    outbox.start(&mut ring, &t.seat, 9).unwrap();
    let first = ring.drain();
    assert_eq!(first[0].kind, kind::RESET);
    assert_eq!((first[1].code, first[1].value), (key::LEFT_SHIFT, value::HELD));
    assert_eq!(first.len(), 2);
    // Then events flow as they come.
    let a = t.press(key::A);
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &a, 10), Ok(true));
    assert_eq!(typed(&ring.drain()), "A");
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &[], 11), Ok(false), "nothing to say");
}

#[test]
fn a_full_ring_drops_events_then_owns_up_and_says_where_things_stand() {
    let mut t = Typist::new(US);
    let mut ring = Ring::new(4);
    let mut outbox = Outbox::new();
    outbox.start(&mut ring, &t.seat, 0).unwrap();
    ring.drain();
    // The consumer stops reading. Six events: four fit.
    let mut burst = Vec::new();
    for code in [key::A, key::B, key::C] {
        burst.extend(t.press(code));
        burst.extend(t.release(code));
    }
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &burst, 1), Ok(true));
    assert_eq!(ring.events.len(), 4);
    assert_eq!(outbox.lost(), 2);
    // More happens while it is full: Shift goes down and stays down, D is typed.
    let shift = t.press(key::LEFT_SHIFT);
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &shift, 2), Ok(false));
    let d = t.press(key::D);
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &d, 3), Ok(false));
    assert_eq!(outbox.lost(), 4);
    // The consumer reads one event: room for one is not room for the catching-up (a lost
    // event, a reset, and the two keys held), so nothing is written yet.
    ring.events.remove(0);
    assert_eq!(outbox.retry(&mut ring, &t.seat, 4), Ok(false));
    assert_eq!(ring.events.len(), 3);
    // It reads the rest.
    ring.drain();
    let up = t.release(key::D);
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &up, 5), Ok(true));
    let caught = ring.drain();
    assert_eq!(caught[0].kind, kind::LOST);
    assert_eq!(caught[0].value, 5, "two, Shift, D's press and D's release");
    assert_eq!(caught[1].kind, kind::RESET);
    // Shift is still held; D, released while the ring was full, is not.
    assert_eq!(keys(&caught[2..]), [(key::LEFT_SHIFT, value::HELD)]);
    assert_eq!(outbox.lost(), 0);
    // And then things flow again.
    let e = t.press(key::E);
    assert_eq!(outbox.deliver(&mut ring, &t.seat, &e, 6), Ok(true));
    assert_eq!(typed(&ring.drain()), "E");
    assert_eq!(outbox.retry(&mut ring, &t.seat, 7), Ok(false));
}

#[test]
fn a_new_consumer_does_not_inherit_the_last_ones_losses_and_a_broken_ring_is_reported() {
    let mut t = Typist::new(US);
    let mut ring = Ring::new(1);
    let mut outbox = Outbox::new();
    outbox.start(&mut ring, &t.seat, 0).unwrap();
    let burst: Vec<Event> = [t.press(key::A), t.release(key::A)].concat();
    outbox.deliver(&mut ring, &t.seat, &burst, 1).unwrap();
    assert!(outbox.lost() > 0);
    let mut new_ring = Ring::new(8);
    outbox.start(&mut new_ring, &t.seat, 2).unwrap();
    assert_eq!(outbox.lost(), 0);
    assert_eq!(new_ring.drain().iter().map(|e| e.kind).collect::<Vec<_>>(), [kind::RESET]);
    new_ring.broken = true;
    let a = t.press(key::A);
    assert_eq!(outbox.deliver(&mut new_ring, &t.seat, &a, 3), Err(Broken));
}
