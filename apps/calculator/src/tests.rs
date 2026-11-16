// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Calculator on the host: sums with the pointer and with the keyboard, the keys that work
//! whichever button has the focus, copy and paste, and its look (`tests/pictures/calculator/`).

use super::*;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Key as Named, Modifiers, WindowId};
use std::path::PathBuf;

fn harness() -> Harness<Calculator> {
    Harness::new(Calculator::new(), settings())
}

/// Clicks the keys with these ids, one after another.
fn click(h: &mut Harness<Calculator>, ids: &[&str]) {
    for id in ids {
        h.click_on(id);
    }
}

#[test]
fn sums_with_the_pointer() {
    let mut h = harness();
    assert_eq!(h.app().display(), "0");
    click(&mut h, &["1", "2", "plus", "7"]);
    assert_eq!(h.app().display(), "7");
    assert_eq!(h.app().pending().as_deref(), Some("12 +"));
    click(&mut h, &["equals"]);
    assert_eq!(h.app().display(), "19");
    assert_eq!(h.app().pending(), None);
    // Every other key, once.
    click(&mut h, &["clear", "9", "times", "3", "minus", "4", "equals"]);
    assert_eq!(h.app().display(), "23");
    click(&mut h, &["negate"]);
    assert_eq!(h.app().display(), "-23");
    click(&mut h, &["clear", "5", "0", "percent"]);
    assert_eq!(h.app().display(), "0.5");
    click(&mut h, &["clear", "1", "point", "5", "point", "2", "back"]);
    assert_eq!(h.app().display(), "1.5");
    click(&mut h, &["clear", "8", "divide", "2", "equals"]);
    assert_eq!(h.app().display(), "4");
    click(&mut h, &["clear", "5", "divide", "0", "equals"]);
    assert_eq!(h.app().display(), "Error");
    click(&mut h, &["7"]);
    assert_eq!(h.app().display(), "Error");
    click(&mut h, &["clear", "0"]);
    assert_eq!(h.app().display(), "0");
}

#[test]
fn the_keys_say_what_they_do() {
    let h = harness();
    // The real signs, where 1.3 had `*`, `/` and `-`.
    for (id, label) in [
        ("divide", "\u{f7}"),
        ("times", "\u{d7}"),
        ("minus", "\u{2212}"),
        ("negate", "+/\u{2212}"),
        ("equals", "="),
        ("back", "<"),
    ] {
        let d = h.find(id).unwrap().1;
        assert_eq!(d.what.label, label, "{id}");
        assert_eq!(d.what.role, "button");
        assert!(d.what.enabled);
    }
    assert_eq!(h.find("7").unwrap().1.what.label, "7");
    // 20 keys, 50 by 42 and 8 apart, from 86 down, as in 1.3.
    let seven = h.find("7").unwrap().1.rect;
    assert_eq!((seven.x, seven.y, seven.width, seven.height), (10, 136, 50, 42));
    let equals = h.find("equals").unwrap().1.rect;
    assert_eq!((equals.x, equals.y, equals.width, equals.height), (184, 286, 50, 42));
    let clear = h.find("clear").unwrap().1.rect;
    assert_eq!((clear.x, clear.y), (10, 86));
    let display = h.find("display").unwrap().1.rect;
    assert_eq!((display.x, display.y, display.width, display.height), (10, 10, 224, 64));
}

#[test]
fn the_display_is_described() {
    let mut h = harness();
    let lines = h.describe_lines(WindowId::MAIN);
    assert!(
        lines.contains(&"display \"\" at 10,10 size 224x64 id display value \"0\"".to_string())
    );
    h.type_text("12+7");
    let d = h.find("display").unwrap().1;
    assert_eq!((d.what.label.as_str(), d.what.value.as_str()), ("12 +", "7"));
}

#[test]
fn typing_sums() {
    let mut h = harness();
    h.type_text("9*3-4=");
    assert_eq!(h.app().display(), "23");
    h.key(Named::Escape);
    assert_eq!(h.app().display(), "0");
    h.type_text("12x3=");
    assert_eq!(h.app().display(), "36");
    h.type_text("c2/4\n");
    assert_eq!(h.app().display(), "0.5");
    h.type_text("C7.5%");
    assert_eq!(h.app().display(), "0.075");
    h.type_text("C123");
    h.key(Named::Backspace);
    assert_eq!(h.app().display(), "12");
    h.type_text("+");
    h.type_text("8");
    h.type_text("=");
    assert_eq!(h.app().display(), "20");
    // Keys that are not the calculator's do nothing.
    h.type_text("qz(");
    assert_eq!(h.app().display(), "20");
    // Shift is held for a capital C, and works.
    h.type_text("C");
    assert_eq!(h.app().display(), "0");
}

#[test]
fn keys_work_whichever_button_has_the_focus() {
    let mut h = harness();
    // Tab to the first button (clear), then type: the sums go on.
    h.key(Named::Tab);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("clear"));
    h.type_text("6+4");
    // Enter means `=`, as in 1.3, and not "press the button with the focus".
    h.key(Named::Enter);
    assert_eq!(h.app().display(), "10");
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("clear"));
    // Space presses the button that has the focus, as on any button.
    h.key(Named::Space);
    assert_eq!(h.app().display(), "0");
    // Tab goes on to the next button, and Shift+Tab back.
    h.key(Named::Tab);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("negate"));
    h.key_with(Named::Tab, Modifiers(Modifiers::SHIFT));
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("clear"));
}

/// A key press with these values.
fn press(ch: char, modifiers: Modifiers, repeat: bool) -> pbr_sandstone::KeyPress {
    pbr_sandstone::KeyPress {
        key: Named::Character,
        code: 0,
        ch: Some(ch),
        typed: !modifiers.ctrl() && !modifiers.alt() && !modifiers.meta(),
        modifiers,
        repeat,
    }
}

#[test]
fn held_keys_repeat_digits_and_backspace_but_not_operators() {
    let none = Modifiers::NONE;
    assert_eq!(key_for(&press('7', none, false)), Some(Key::Digit(7)));
    assert_eq!(key_for(&press('7', none, true)), Some(Key::Digit(7)));
    assert_eq!(key_for(&press('+', none, false)), Some(Key::Op(Op::Add)));
    assert_eq!(key_for(&press('+', none, true)), None);
    assert_eq!(key_for(&press('=', none, true)), None);
    // Shift is held to type `+`, `*` and `%` on many keyboards.
    assert_eq!(
        key_for(&press('*', Modifiers(Modifiers::SHIFT), false)),
        Some(Key::Op(Op::Multiply))
    );
    let named = |key, repeat| pbr_sandstone::KeyPress {
        key,
        ch: None,
        typed: false,
        repeat,
        ..press('a', none, false)
    };
    assert_eq!(key_for(&named(Named::Enter, false)), Some(Key::Equals));
    assert_eq!(key_for(&named(Named::Enter, true)), None);
    assert_eq!(key_for(&named(Named::Backspace, true)), Some(Key::Back));
    assert_eq!(key_for(&named(Named::Escape, false)), Some(Key::Clear));
    assert_eq!(key_for(&named(Named::Tab, false)), None);
    assert_eq!(key_for(&named(Named::Space, false)), None);
}

#[test]
fn ctrl_and_alt_keys_are_not_typing() {
    assert_eq!(key_for(&press('c', Modifiers(Modifiers::CTRL), false)), None);
    assert_eq!(key_for(&press('5', Modifiers(Modifiers::ALT), false)), None);
    assert_eq!(key_for(&press('5', Modifiers(Modifiers::META), false)), None);
    let mut h = harness();
    h.type_text("5");
    h.shortcut(Shortcut::ctrl('c'));
    h.shortcut(Shortcut::ctrl('a'));
    assert_eq!(h.app().display(), "5");
}

#[test]
fn copy_and_paste() {
    let mut h = harness();
    h.type_text("12+7=");
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("19"));
    // Paste enters a number, as if typed.
    h.type_text("c");
    h.set_clipboard_text("  42.5\n");
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(h.app().display(), "42.5");
    h.type_text("+8=");
    assert_eq!(h.app().display(), "50.5");
    // What isn't a number is ignored.
    h.set_clipboard_text("hello");
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(h.app().display(), "50.5");
    h.set_clipboard_text("1e5");
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(h.app().display(), "50.5");
    // An error isn't copied.
    h.type_text("c5/0=");
    assert_eq!(h.app().display(), "Error");
    h.set_clipboard_text("keep");
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("keep"));
}

#[test]
fn the_window_is_1_3s_size_and_cannot_be_resized() {
    let s = settings();
    assert_eq!((s.size, s.resizable), ((244, 338), false));
    let h = harness();
    let w = h.window(WindowId::MAIN).unwrap();
    assert_eq!(w.size, (244, 338));
}

#[test]
fn the_number_is_drawn_as_big_as_fits() {
    let d = |s: &str| Display::new(s, None).number_style(204).size;
    assert_eq!(d("0"), 3);
    // 204 wide at 6 dots a character: 11 characters at size 3, 17 at size 2.
    assert_eq!(d("12345678901"), 3);
    assert_eq!(d("123456789012"), 2);
    assert_eq!(d("12345678901234567"), 2);
    assert_eq!(d("123456789012345678"), 1);
    assert_eq!(d("123456789012345.67890123"), 1);
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/calculator")
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "calculator-1x"), (2, "calculator-2x")] {
        let h = Harness::with_scale(Calculator::new(), settings(), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}

#[test]
fn how_it_looks_with_a_sum_started() {
    let mut h = harness();
    h.type_text("12+7");
    h.check_window_picture_in(WindowId::MAIN, &dir(), "calculator-pending-1x");
}

#[test]
fn how_a_long_number_and_an_error_look() {
    let mut h = harness();
    h.type_text("123456789012345");
    h.check_window_picture_in(WindowId::MAIN, &dir(), "calculator-long-1x");
    h.type_text("c5/0=");
    h.check_window_picture_in(WindowId::MAIN, &dir(), "calculator-error-1x");
}
