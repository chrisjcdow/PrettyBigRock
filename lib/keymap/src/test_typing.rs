// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Caps Lock, Num Lock, the keypad's editing keys, codes that are no key, and dead keys.

use crate::test_layouts::{INTL_DEAD, code, gb_layout, intl_layout, load, mods, us_layout};
use crate::{Composer, DeadKey, Layout, Mods, Typed, caps_pair, keypad_editing, parse};
use pbr_devices::keys::{ALL, key};

fn caps(shift: bool, altgr: bool) -> Mods {
    Mods { shift, altgr, caps_lock: true, num_lock: true }
}

#[test]
fn caps_pairs() {
    for (l, u) in [('a', 'A'), ('é', 'É'), ('ÿ', 'Ÿ'), ('z', 'Z'), ('ä', 'Ä'), ('ø', 'Ø')] {
        assert!(caps_pair(l, u), "{l} {u}");
        assert!(!caps_pair(u, l), "{u} {l}");
        assert!(!caps_pair(l, l));
    }
    // Greek and Cyrillic.
    assert!(caps_pair('α', 'Α'));
    assert!(caps_pair('ж', 'Ж'));
    assert!(caps_pair('ё', 'Ё'));
    assert!(caps_pair('ế', 'Ế'));
    // `ß` becomes SS, two letters, so it pairs with nothing.
    assert!(!caps_pair('ß', 'S'));
    assert!(!caps_pair('ß', 'ẞ'));
    assert!(!caps_pair('ß', '§'));
    for (a, b) in [('1', '!'), ('a', 'B'), ('a', '1'), ('-', '_'), ('[', '{'), ('a', 'a')] {
        assert!(!caps_pair(a, b), "{a} {b}");
    }
    // Outside the blocks: Japanese and Armenian have cases or none, but are not swapped.
    assert!(!caps_pair('ա', 'Ա'));
    assert!(!caps_pair('ⅰ', 'Ⅰ'));
    // Dead keys are not letters.
    assert!(!caps_pair(' ', ' '));
}

/// Surprising pairs that `core` gives: `µ` (micro sign) has the Greek capital `Μ`, and the
/// long s `ſ` has `S`. They are real case pairs, but nobody expects a keyboard to swap them.
#[test]
fn core_pairs_that_might_surprise() {
    assert!(caps_pair('µ', 'Μ'));
    assert!(caps_pair('ſ', 'S'));
    assert!(caps_pair('ı', 'I'));
}

#[test]
fn caps_lock_swaps_letters_and_nothing_else() {
    for layout in [us_layout(), gb_layout(), intl_layout()] {
        for c in 'a'..='z' {
            let k = code(&c.to_ascii_uppercase().to_string());
            let up = c.to_ascii_uppercase();
            assert_eq!(layout.key(k, caps(false, false)), Typed::Char(up));
            assert_eq!(layout.key(k, caps(true, false)), Typed::Char(c));
        }
        // Digits, punctuation, space and the key beside Shift are unchanged.
        for name in [
            "1",
            "2",
            "3",
            "4",
            "5",
            "0",
            "MINUS",
            "EQUALS",
            "GRAVE",
            "LEFT_BRACKET",
            "RIGHT_BRACKET",
            "BACKSLASH",
            "SEMICOLON",
            "COMMA",
            "PERIOD",
            "SLASH",
            "SPACE",
            "102ND",
        ] {
            for (shift, altgr) in [(false, false), (true, false)] {
                assert_eq!(
                    layout.key(code(name), caps(shift, altgr)),
                    layout.key(code(name), mods(shift, altgr)),
                    "{name}"
                );
            }
        }
    }
    let us = us_layout();
    assert_eq!(us.key(key::D1, caps(false, false)), Typed::Char('1'));
    assert_eq!(us.key(key::D1, caps(true, false)), Typed::Char('!'));
}

#[test]
fn caps_lock_on_the_altgr_levels() {
    for layout in [gb_layout(), intl_layout()] {
        assert_eq!(layout.key(key::A, caps(false, true)), Typed::Char('Á'));
        assert_eq!(layout.key(key::A, caps(true, true)), Typed::Char('á'));
        assert_eq!(layout.key(key::E, caps(false, true)), Typed::Char('É'));
        assert_eq!(layout.key(key::U, caps(true, true)), Typed::Char('ú'));
        // AltGr plain still follows Shift for the plain letter.
        assert_eq!(layout.key(key::A, caps(false, false)), Typed::Char('A'));
    }
    let l = intl_layout();
    assert_eq!(l.key(code("Q"), caps(false, true)), Typed::Char('Ä'));
    assert_eq!(l.key(code("W"), caps(true, true)), Typed::Char('å'));
    assert_eq!(l.key(code("Z"), caps(false, true)), Typed::Char('Æ'));
    assert_eq!(l.key(code("N"), caps(false, true)), Typed::Char('Ñ'));
    assert_eq!(l.key(code("COMMA"), caps(false, true)), Typed::Char('Ç'));
    assert_eq!(l.key(code("COMMA"), caps(true, true)), Typed::Char('ç'));
    // The sharp s and section sign are not a letter pair; the micro sign has no capital here.
    assert_eq!(l.key(code("S"), caps(false, true)), Typed::Char('ß'));
    assert_eq!(l.key(code("S"), caps(true, true)), Typed::Char('§'));
    assert_eq!(l.key(code("M"), caps(false, true)), Typed::Char('µ'));
    assert_eq!(l.key(code("M"), caps(true, true)), Typed::Nothing);
    // `r` has only a plain AltGr level, so there is no pair.
    assert_eq!(l.key(code("R"), caps(false, true)), Typed::Char('®'));
    // Dead keys are not swapped.
    assert_eq!(
        l.key(key::APOSTROPHE, caps(false, false)),
        l.key(key::APOSTROPHE, mods(false, false))
    );
}

#[test]
fn altgr_is_ignored_without_an_altgr_line() {
    let us = us_layout();
    for c in 0..120 {
        for shift in [false, true] {
            assert_eq!(us.key(c, mods(shift, true)), us.key(c, mods(shift, false)));
        }
    }
    let l = load("title \"T\"\nkey A a A\n");
    assert_eq!(l.key(key::A, caps(true, true)), Typed::Char('a'));
}

#[test]
fn num_lock_decides_the_keypad_digits() {
    for layout in [us_layout(), gb_layout(), intl_layout()] {
        let digits = [
            ("KP_0", '0'),
            ("KP_1", '1'),
            ("KP_2", '2'),
            ("KP_3", '3'),
            ("KP_4", '4'),
            ("KP_5", '5'),
            ("KP_6", '6'),
            ("KP_7", '7'),
            ("KP_8", '8'),
            ("KP_9", '9'),
            ("KP_PERIOD", '.'),
        ];
        for (name, c) in digits {
            for (shift, altgr, caps_lock) in
                [(false, false, false), (true, false, false), (false, false, true)]
            {
                let on = Mods { shift, altgr, caps_lock, num_lock: true };
                let off = Mods { num_lock: false, ..on };
                assert_eq!(layout.key(code(name), on), Typed::Char(c), "{name}");
                assert_eq!(layout.key(code(name), off), Typed::Nothing, "{name}");
            }
        }
        // The operators do not care.
        for (name, c) in
            [("KP_PLUS", '+'), ("KP_MINUS", '-'), ("KP_MULTIPLY", '*'), ("KP_DIVIDE", '/')]
        {
            for num_lock in [false, true] {
                let m = Mods { num_lock, ..Mods::default() };
                assert_eq!(layout.key(code(name), m), Typed::Char(c), "{name}");
            }
        }
    }
}

#[test]
fn caps_lock_never_applies_to_the_keypad() {
    let l = load("title \"T\"\naltgr\nkey KP_7 a A\nkey KP_PLUS b B\nkey KP_DIVIDE c C\n");
    for shift in [false, true] {
        let m = Mods { shift, altgr: false, caps_lock: true, num_lock: true };
        let plain = Mods { caps_lock: false, ..m };
        for k in [key::KP_7, key::KP_PLUS, key::KP_DIVIDE] {
            assert_eq!(l.key(k, m), l.key(k, plain));
        }
    }
    assert_eq!(l.key(key::KP_7, caps(false, false)), Typed::Char('a'));
    assert_eq!(l.key(key::KP_7, Mods { num_lock: false, ..caps(false, false) }), Typed::Nothing);
}

#[test]
fn the_keypad_is_the_editing_keys_without_num_lock() {
    let table = [
        (key::KP_7, key::HOME),
        (key::KP_8, key::UP),
        (key::KP_9, key::PAGE_UP),
        (key::KP_4, key::LEFT),
        (key::KP_6, key::RIGHT),
        (key::KP_1, key::END),
        (key::KP_2, key::DOWN),
        (key::KP_3, key::PAGE_DOWN),
        (key::KP_0, key::INSERT),
        (key::KP_PERIOD, key::DELETE),
    ];
    for &(name, k) in ALL {
        let want = table.iter().find(|(from, _)| *from == k).map(|&(_, to)| to);
        assert_eq!(keypad_editing(k), want, "{name}");
    }
    assert_eq!(keypad_editing(key::KP_5), None);
    for c in [0, 106, 1000, u16::MAX] {
        assert_eq!(keypad_editing(c), None);
    }
}

#[test]
fn codes_that_are_no_key_type_nothing() {
    for layout in [us_layout(), gb_layout(), intl_layout()] {
        for c in [0, 106, 107, 255, 256, 1000, u16::MAX - 1, u16::MAX] {
            for bits in 0..16u8 {
                let m = Mods {
                    shift: bits & 1 != 0,
                    altgr: bits & 2 != 0,
                    caps_lock: bits & 4 != 0,
                    num_lock: bits & 8 != 0,
                };
                assert_eq!(layout.key(c, m), Typed::Nothing, "{c}");
            }
        }
    }
}

// ---- The composer ----

/// The dead key of `intl` with this accent, found by pressing the keys that have it.
fn dead(layout: &Layout, accent: char) -> DeadKey {
    for &(_, k) in ALL {
        for level in 0..4 {
            if let Typed::Dead(d) = layout.key(k, mods(level % 2 == 1, level >= 2))
                && layout.accent(d) == accent
            {
                return d;
            }
        }
    }
    panic!("no dead key {accent:?}");
}

fn feed(c: &mut Composer, l: &Layout, t: Typed) -> Vec<char> {
    c.feed(l, t).into_iter().flatten().collect()
}

#[test]
fn the_dead_keys_are_on_the_right_keys() {
    let l = intl_layout();
    let acc = |k, shift| match l.key(k, mods(shift, false)) {
        Typed::Dead(d) => l.accent(d),
        other => panic!("{other:?}"),
    };
    assert_eq!(acc(key::APOSTROPHE, false), '\'');
    assert_eq!(acc(key::APOSTROPHE, true), '"');
    assert_eq!(acc(key::GRAVE, false), '`');
    assert_eq!(acc(key::GRAVE, true), '~');
    assert_eq!(acc(key::D6, true), '^');
    assert_eq!(l.key(key::D6, mods(false, false)), Typed::Char('6'));
}

#[test]
fn nothing_waiting() {
    let l = intl_layout();
    let mut c = Composer::new();
    assert!(!c.waiting());
    assert_eq!(feed(&mut c, &l, Typed::Nothing), vec![]);
    assert!(!c.waiting());
    assert_eq!(feed(&mut c, &l, Typed::Char('x')), vec!['x']);
    assert!(!c.waiting());
    let d = dead(&l, '\'');
    assert_eq!(feed(&mut c, &l, Typed::Dead(d)), vec![]);
    assert!(c.waiting());
}

#[test]
fn every_row_of_the_table_for_every_dead_key() {
    let l = intl_layout();
    for (_, accent, pairs) in INTL_DEAD {
        let d = dead(&l, accent);
        let mut c = Composer::new();
        let press = |c: &mut Composer| {
            assert_eq!(feed(c, &l, Typed::Dead(d)), vec![]);
            assert!(c.waiting());
        };
        // Each base combines, and nothing waits afterwards.
        for &(base, result) in pairs {
            press(&mut c);
            assert_eq!(feed(&mut c, &l, Typed::Char(base)), vec![result], "{accent} {base}");
            assert!(!c.waiting());
        }
        // A letter that does not combine: the accent, then the letter.
        for other in ['z', 'x', 'Z', '1', 'ß', 'ä', '-'] {
            if pairs.iter().any(|&(b, _)| b == other) {
                continue;
            }
            press(&mut c);
            assert_eq!(feed(&mut c, &l, Typed::Char(other)), vec![accent, other]);
            assert!(!c.waiting());
        }
        // Space types the accent.
        press(&mut c);
        assert_eq!(feed(&mut c, &l, Typed::Char(' ')), vec![accent]);
        assert!(!c.waiting());
        // The same dead key twice types the accent once.
        press(&mut c);
        assert_eq!(feed(&mut c, &l, Typed::Dead(d)), vec![accent]);
        assert!(!c.waiting());
        // A key that types nothing (Shift) does not disturb it.
        press(&mut c);
        assert_eq!(feed(&mut c, &l, Typed::Nothing), vec![]);
        assert_eq!(feed(&mut c, &l, Typed::Nothing), vec![]);
        assert!(c.waiting());
        let (base, result) = pairs[0];
        assert_eq!(feed(&mut c, &l, Typed::Char(base)), vec![result]);
        assert!(!c.waiting());
        // Reset forgets it.
        press(&mut c);
        c.reset();
        assert!(!c.waiting());
        assert_eq!(feed(&mut c, &l, Typed::Char(base)), vec![base]);
        // Another dead key: the first one's accent now, the second waits.
        for (_, other_accent, other_pairs) in INTL_DEAD {
            if other_accent == accent {
                continue;
            }
            let e = dead(&l, other_accent);
            press(&mut c);
            assert_eq!(feed(&mut c, &l, Typed::Dead(e)), vec![accent]);
            assert!(c.waiting());
            let (b, r) = other_pairs[0];
            assert_eq!(feed(&mut c, &l, Typed::Char(b)), vec![r]);
            assert!(!c.waiting());
        }
    }
}

#[test]
fn spelling_afrikaans() {
    let l = intl_layout();
    let mut c = Composer::new();
    let mut typed = String::new();
    let mut press = |t: Typed| typed.extend(c.feed(&l, t).into_iter().flatten());
    let key = |k, shift| l.key(k, mods(shift, false));
    // "Afrikaanse" has none, but "ê", "ë", "ô", "û", "î", "ï" do.
    for (k, shift) in [
        (code("6"), true),
        (code("E"), false),
        (code("APOSTROPHE"), true),
        (code("E"), false),
        (code("6"), true),
        (code("O"), false),
        (code("6"), true),
        (code("U"), false),
        (code("6"), true),
        (code("I"), false),
        (code("APOSTROPHE"), true),
        (code("I"), false),
    ] {
        press(key(k, shift));
    }
    assert_eq!(typed, "êëôûîï");
}

#[test]
fn dead_keys_do_nothing_on_layouts_without_them() {
    let l = us_layout();
    let mut c = Composer::new();
    for &(_, k) in ALL {
        for level in 0..4 {
            let t = l.key(k, mods(level % 2 == 1, level >= 2));
            let out = feed(&mut c, &l, t);
            assert_eq!(out.len(), usize::from(matches!(t, Typed::Char(_))));
            assert!(!c.waiting());
        }
    }
}

#[test]
fn a_dead_key_from_another_layout_does_not_panic() {
    let l = us_layout();
    let foreign = DeadKey(7);
    assert_eq!(l.accent(foreign), ' ');
    assert_eq!(l.combine(foreign, 'a'), None);
    let mut c = Composer::new();
    assert_eq!(feed(&mut c, &l, Typed::Dead(foreign)), vec![]);
    assert_eq!(feed(&mut c, &l, Typed::Char('a')), vec![' ', 'a']);
    assert!(parse(b"").is_err());
}
