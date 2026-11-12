// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The three layout files, checked against tables written out here from the standard layouts.
//! The tables are a second statement of each layout, typed independently of the files.

use crate::{DeadKey, Layout, Mods, Typed, parse};
use pbr_devices::keys::{ALL, key};

pub(crate) const US_FILE: &str = include_str!("../../../services/input/layouts/us.kbd");
pub(crate) const GB_FILE: &str = include_str!("../../../services/input/layouts/gb.kbd");
pub(crate) const INTL_FILE: &str = include_str!("../../../services/input/layouts/us-intl.kbd");

pub(crate) fn load(text: &str) -> Layout {
    match parse(text.as_bytes()) {
        Ok(layout) => layout,
        Err(e) => panic!("a layout file was refused: {e}"),
    }
}

pub(crate) fn us_layout() -> Layout {
    load(US_FILE)
}
pub(crate) fn gb_layout() -> Layout {
    load(GB_FILE)
}
pub(crate) fn intl_layout() -> Layout {
    load(INTL_FILE)
}

pub(crate) fn code(name: &str) -> u16 {
    ALL.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no key {name}")).1
}

pub(crate) fn mods(shift: bool, altgr: bool) -> Mods {
    Mods { shift, altgr, caps_lock: false, num_lock: true }
}

/// In the tables below, these stand for the five dead keys; `\0` stands for "types nothing".
pub(crate) const ACUTE: char = '\u{1}';
pub(crate) const GRAVE: char = '\u{2}';
pub(crate) const CIRCUMFLEX: char = '\u{3}';
pub(crate) const DIAERESIS: char = '\u{4}';
pub(crate) const TILDE: char = '\u{5}';

/// What each dead key does: its marker, the accent it types alone, and its pairs.
/// A dead key's marker, its accent, and its pairs.
type DeadRow = (char, char, &'static [(char, char)]);

pub(crate) const INTL_DEAD: [DeadRow; 5] = [
    (
        ACUTE,
        '\'',
        &[
            ('a', 'á'),
            ('A', 'Á'),
            ('e', 'é'),
            ('E', 'É'),
            ('i', 'í'),
            ('I', 'Í'),
            ('o', 'ó'),
            ('O', 'Ó'),
            ('u', 'ú'),
            ('U', 'Ú'),
            ('y', 'ý'),
            ('Y', 'Ý'),
            ('c', 'ç'),
            ('C', 'Ç'),
            (' ', '\''),
        ],
    ),
    (
        GRAVE,
        '`',
        &[
            ('a', 'à'),
            ('A', 'À'),
            ('e', 'è'),
            ('E', 'È'),
            ('i', 'ì'),
            ('I', 'Ì'),
            ('o', 'ò'),
            ('O', 'Ò'),
            ('u', 'ù'),
            ('U', 'Ù'),
            (' ', '`'),
        ],
    ),
    (
        CIRCUMFLEX,
        '^',
        &[
            ('a', 'â'),
            ('A', 'Â'),
            ('e', 'ê'),
            ('E', 'Ê'),
            ('i', 'î'),
            ('I', 'Î'),
            ('o', 'ô'),
            ('O', 'Ô'),
            ('u', 'û'),
            ('U', 'Û'),
            (' ', '^'),
        ],
    ),
    (
        DIAERESIS,
        '"',
        &[
            ('a', 'ä'),
            ('A', 'Ä'),
            ('e', 'ë'),
            ('E', 'Ë'),
            ('i', 'ï'),
            ('I', 'Ï'),
            ('o', 'ö'),
            ('O', 'Ö'),
            ('u', 'ü'),
            ('U', 'Ü'),
            ('y', 'ÿ'),
            ('Y', 'Ÿ'),
            (' ', '"'),
        ],
    ),
    (
        TILDE,
        '~',
        &[('a', 'ã'), ('A', 'Ã'), ('n', 'ñ'), ('N', 'Ñ'), ('o', 'õ'), ('O', 'Õ'), (' ', '~')],
    ),
];

/// The keypad, the same on all three layouts: plain and Shift (Num Lock on).
const KEYPAD: &[(&str, &str)] = &[
    ("KP_0", "00"),
    ("KP_1", "11"),
    ("KP_2", "22"),
    ("KP_3", "33"),
    ("KP_4", "44"),
    ("KP_5", "55"),
    ("KP_6", "66"),
    ("KP_7", "77"),
    ("KP_8", "88"),
    ("KP_9", "99"),
    ("KP_PERIOD", ".."),
    ("KP_PLUS", "++"),
    ("KP_MINUS", "--"),
    ("KP_MULTIPLY", "**"),
    ("KP_DIVIDE", "//"),
];

/// The keys that are not letters, on the US layout: each level in turn (plain, Shift, AltGr,
/// AltGr with Shift); a shorter string leaves the rest typing nothing.
const US_OTHERS: &[(&str, &str)] = &[
    ("GRAVE", "`~"),
    ("1", "1!"),
    ("2", "2@"),
    ("3", "3#"),
    ("4", "4$"),
    ("5", "5%"),
    ("6", "6^"),
    ("7", "7&"),
    ("8", "8*"),
    ("9", "9("),
    ("0", "0)"),
    ("MINUS", "-_"),
    ("EQUALS", "=+"),
    ("LEFT_BRACKET", "[{"),
    ("RIGHT_BRACKET", "]}"),
    ("BACKSLASH", "\\|"),
    ("SEMICOLON", ";:"),
    ("APOSTROPHE", "'\""),
    ("COMMA", ",<"),
    ("PERIOD", ".>"),
    ("SLASH", "/?"),
    ("102ND", "\\|"),
    ("SPACE", "  "),
];

const GB_OTHERS: &[(&str, &str)] = &[
    ("GRAVE", "`¬¦"),
    ("1", "1!"),
    ("2", "2\""),
    ("3", "3£"),
    ("4", "4$€"),
    ("5", "5%"),
    ("6", "6^"),
    ("7", "7&"),
    ("8", "8*"),
    ("9", "9("),
    ("0", "0)"),
    ("MINUS", "-_"),
    ("EQUALS", "=+"),
    ("LEFT_BRACKET", "[{"),
    ("RIGHT_BRACKET", "]}"),
    ("BACKSLASH", "#~"),
    ("SEMICOLON", ";:"),
    ("APOSTROPHE", "'@"),
    ("COMMA", ",<"),
    ("PERIOD", ".>"),
    ("SLASH", "/?"),
    ("102ND", "\\|"),
    ("SPACE", "  "),
];

/// Letters that type something on AltGr, on the UK layout.
const GB_LETTERS: &[(&str, &str)] =
    &[("A", "aAáÁ"), ("E", "eEéÉ"), ("I", "iIíÍ"), ("O", "oOóÓ"), ("U", "uUúÚ")];

const INTL_OTHERS: &[(&str, &str)] = &[
    ("GRAVE", "\u{2}\u{5}"),
    ("1", "1!¡¹"),
    ("2", "2@²"),
    ("3", "3#³"),
    ("4", "4$¤£"),
    ("5", "5%€"),
    ("6", "6\u{3}¼"),
    ("7", "7&½"),
    ("8", "8*¾"),
    ("9", "9(‘"),
    ("0", "0)’"),
    ("MINUS", "-_¥"),
    ("EQUALS", "=+×÷"),
    ("LEFT_BRACKET", "[{«"),
    ("RIGHT_BRACKET", "]}»"),
    ("BACKSLASH", "\\|¬¦"),
    ("SEMICOLON", ";:¶°"),
    ("APOSTROPHE", "\u{1}\u{4}´¨"),
    ("COMMA", ",<çÇ"),
    ("PERIOD", ".>"),
    ("SLASH", "/?¿"),
    ("102ND", "\\|"),
    ("SPACE", "  "),
];

const INTL_LETTERS: &[(&str, &str)] = &[
    ("A", "aAáÁ"),
    ("C", "cC©¢"),
    ("D", "dDðÐ"),
    ("E", "eEéÉ"),
    ("I", "iIíÍ"),
    ("L", "lLøØ"),
    ("M", "mMµ"),
    ("N", "nNñÑ"),
    ("O", "oOóÓ"),
    ("P", "pPöÖ"),
    ("Q", "qQäÄ"),
    ("R", "rR®"),
    ("S", "sSß§"),
    ("T", "tTþÞ"),
    ("U", "uUúÚ"),
    ("W", "wWåÅ"),
    ("Y", "yYüÜ"),
    ("Z", "zZæÆ"),
];

/// Whether `want` (one entry of a table above) is what `typed` is on `layout`.
fn matches(layout: &Layout, typed: Typed, want: char) -> bool {
    let accent_of = |marker: char| INTL_DEAD.iter().find(|d| d.0 == marker).map(|d| d.1);
    match typed {
        Typed::Nothing => want == '\0',
        Typed::Char(c) => c == want,
        Typed::Dead(d) => accent_of(want) == Some(layout.accent(d)),
    }
}

/// Checks every key code at all four levels against the tables: the keys listed type what is
/// listed, the letters not listed type themselves and their capitals, and every other key
/// types nothing.
fn check(layout: &Layout, others: &[(&str, &str)], letters: &[(&str, &str)]) {
    let table = |name: &str| -> Option<Vec<char>> {
        let row = others.iter().chain(KEYPAD).chain(letters).find(|(n, _)| *n == name);
        if let Some((_, levels)) = row {
            let mut v: Vec<char> = levels.chars().collect();
            v.resize(4, '\0');
            return Some(v);
        }
        let mut chars = name.chars();
        let letter = chars.next().filter(|c| c.is_ascii_uppercase() && chars.next().is_none());
        letter.map(|c| vec![c.to_ascii_lowercase(), c, '\0', '\0'])
    };
    for &(name, code) in ALL {
        let want = table(name).unwrap_or(vec!['\0'; 4]);
        for (level, &wanted) in want.iter().enumerate() {
            let (shift, altgr) = (level % 2 == 1, level >= 2);
            let typed = layout.key(code, mods(shift, altgr));
            if !layout.has_altgr() && altgr {
                // Without an AltGr key the modifier is ignored.
                assert_eq!(typed, layout.key(code, mods(shift, false)), "{name}");
                continue;
            }
            assert!(
                matches(layout, typed, wanted),
                "{name} at level {level} typed {typed:?}, want {wanted:?}"
            );
        }
    }
}

/// Checks a dead key table against `INTL_DEAD`, in both directions.
fn check_dead(layout: &Layout, marker: char, accent: char, pairs: &[(char, char)]) {
    let find = |m: char| {
        let mut all = (1..=ALL.len() as u16).flat_map(|c| (0..4).map(move |l| (c, l)));
        all.find_map(|(c, l)| match layout.key(c, mods(l % 2 == 1, l >= 2)) {
            Typed::Dead(d) if layout.accent(d) == INTL_DEAD.iter().find(|x| x.0 == m)?.1 => Some(d),
            _ => None,
        })
    };
    let dead = find(marker).expect("a key types it");
    assert_eq!(layout.accent(dead), accent);
    for &(base, result) in pairs {
        assert_eq!(layout.combine(dead, base), Some(result), "{accent:?} then {base:?}");
    }
    // Nothing else combines: every ASCII character and every character the table gives.
    let listed = |c: char| pairs.iter().any(|&(b, _)| b == c);
    for c in (' '..='~').chain(['é', '\u{a0}', 'ß']) {
        if !listed(c) {
            assert_eq!(layout.combine(dead, c), None, "{accent:?} then {c:?}");
        }
    }
}

#[test]
fn us_types_what_the_us_layout_types() {
    let layout = us_layout();
    assert_eq!(layout.title(), "English (US)");
    assert!(!layout.has_altgr());
    check(&layout, US_OTHERS, &[]);
}

/// M5's US layout, exactly as `lib/devices` had it when the console read keyboards itself.
/// It is kept here, and only here, as the second statement of what `us.kbd` must type: the
/// table moved into the file in M6 (note 0021, section 4), and this test is what says the move
/// changed nothing.
fn m5_us(code: u16, shift: bool) -> Option<char> {
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

/// Every code against M5's table, which the file replaces (the keypad and the key beside the
/// left Shift, which M5's table lacks, aside).
#[test]
fn us_agrees_with_the_built_in_table() {
    let layout = us_layout();
    for &(name, code) in ALL {
        if name.starts_with("KP_") || name == "102ND" {
            continue;
        }
        for shift in [false, true] {
            let typed = layout.key(code, mods(shift, false));
            let want = m5_us(code, shift).map_or(Typed::Nothing, Typed::Char);
            assert_eq!(typed, want, "{name} shift={shift}");
        }
    }
}

#[test]
fn gb_types_what_the_uk_layout_types() {
    let layout = gb_layout();
    assert_eq!(layout.title(), "English (UK)");
    assert!(layout.has_altgr());
    check(&layout, GB_OTHERS, GB_LETTERS);
    // The pound sign is where the hash is on a US keyboard, and the euro is AltGr+4.
    assert_eq!(layout.key(key::D3, mods(true, false)), Typed::Char('£'));
    assert_eq!(layout.key(key::D4, mods(false, true)), Typed::Char('€'));
    assert_eq!(layout.key(key::K102ND, mods(true, false)), Typed::Char('|'));
}

#[test]
fn us_intl_types_what_the_international_layout_types() {
    let layout = intl_layout();
    assert_eq!(layout.title(), "English (US, international, with dead keys)");
    assert!(layout.has_altgr());
    check(&layout, INTL_OTHERS, INTL_LETTERS);
}

#[test]
fn us_intl_dead_keys_have_the_right_tables() {
    let layout = intl_layout();
    assert_eq!(layout.dead.len(), 5);
    for (marker, accent, pairs) in INTL_DEAD {
        check_dead(&layout, marker, accent, pairs);
    }
}

#[test]
fn the_files_have_the_standard_licence_lines() {
    for text in [US_FILE, GB_FILE, INTL_FILE] {
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("# SPDX-License-Identifier: GPL-3.0-or-later"));
        assert_eq!(lines.next(), Some("# Copyright (C) 2026 Christopher Cameron-Dow"));
    }
}

/// The three layouts mention the same keys: every typing key, and nothing else.
#[test]
fn every_layout_covers_every_typing_key() {
    for layout in [us_layout(), gb_layout(), intl_layout()] {
        for &(name, code) in ALL {
            let typing = (1..=26).contains(&code)
                || (27..=36).contains(&code)
                || (41..=52).contains(&code)
                || (84..=99).contains(&code);
            let any = (0..4).any(|l| layout.key(code, mods(l % 2 == 1, l >= 2)) != Typed::Nothing);
            assert_eq!(any, typing, "{name}");
        }
    }
}

/// Every character a layout can type, dead-key results included, is in the font.
#[test]
fn every_character_has_a_glyph() {
    for layout in [us_layout(), gb_layout(), intl_layout()] {
        let mut chars = Vec::new();
        for code in 0..=ALL.len() as u16 + 1 {
            for level in 0..4 {
                for caps_lock in [false, true] {
                    let m = Mods { caps_lock, ..mods(level % 2 == 1, level >= 2) };
                    if let Typed::Char(c) = layout.key(code, m) {
                        chars.push(c);
                    }
                }
            }
        }
        for table in &layout.dead {
            for &(base, result) in &table.pairs {
                chars.extend([base, result]);
            }
        }
        assert!(chars.len() > 100);
        for c in chars {
            assert!(pbr_font::lookup(c).is_some(), "{c:?} (U+{:04X}) has no glyph", u32::from(c));
        }
    }
}

#[test]
fn dead_keys_of_one_layout_are_distinct() {
    let layout = intl_layout();
    let apostrophe = layout.key(key::APOSTROPHE, mods(false, false));
    let quote = layout.key(key::APOSTROPHE, mods(true, false));
    assert!(matches!((apostrophe, quote), (Typed::Dead(a), Typed::Dead(b)) if a != b));
    assert_ne!(DeadKey(0), DeadKey(1));
}
