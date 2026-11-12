// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A user's settings file, `/System/settings/<user>.txt`
//! (docs/design/0024-the-session-desktop.md, section 5), which the users service reads when a
//! session starts and rewrites when the user changes a setting:
//!
//! ```text
//! layout gb
//! timezone +02:00
//! ```
//!
//! | Key | Meaning |
//! |-----|---------|
//! | `layout NAME` | The keyboard layout (note 0021); `us` if absent |
//! | `timezone ±HH:MM` | The offset from UTC for showing times; `+00:00` if absent |
//!
//! One `key value` per line, in the style of every settings file here (`lines`). **Unknown
//! keys are kept and ignored**: a later version may add some, and an older one that rewrites
//! the file must not lose them. A file that can't be read (not text, a key twice, a known key
//! with a value that makes no sense) is refused whole, and the users service then treats it as
//! empty and says so in the log: a half-read settings file is worse than the defaults.
//!
//! Whether a layout of that name exists is not checked here: only the input service knows
//! which there are. This checks that the value could be a layout's name.

use crate::lines::{self, Limits, ReadError};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// The largest settings file.
pub const MAX_SIZE: usize = 4096;
/// The most settings in a file (`pbr.users.settings` answers with at most this many).
pub const MAX_SETTINGS: usize = 32;
/// The longest key, in bytes (`pbr.users`'s `Setting.key`).
pub const MAX_KEY: usize = 32;
/// The longest value, in bytes (`pbr.users`'s `Setting.value`).
pub const MAX_VALUE: usize = 128;

/// The key of the keyboard layout.
pub const LAYOUT: &str = "layout";
/// The key of the time zone.
pub const TIMEZONE: &str = "timezone";
/// The layout when the file names none.
pub const DEFAULT_LAYOUT: &str = "us";

const LIMITS: Limits = Limits { max_size: MAX_SIZE, max_line: 200, quotes: false };

/// What was wrong with a settings file, or with a setting someone asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    /// The file as a whole can't be read.
    Read(ReadError),
    /// More than [`MAX_SETTINGS`] lines.
    TooMany,
    /// A key that isn't 1 to [`MAX_KEY`] lower-case letters, digits and `-`, starting with a
    /// letter (on this line of a file, or 0 for a setting asked for).
    BadKey(usize),
    /// A value that is empty, longer than [`MAX_VALUE`], or (for a key this version knows) not
    /// what that key takes.
    BadValue(usize),
    /// The same key on two lines.
    Twice(usize),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Read(e) => write!(f, "it can't be read ({e:?})"),
            SettingsError::TooMany => write!(f, "it has more than {MAX_SETTINGS} settings"),
            SettingsError::BadKey(0) => write!(f, "that is not a setting's name"),
            SettingsError::BadKey(n) => write!(f, "line {n}: that is not a setting's name"),
            SettingsError::BadValue(0) => write!(f, "that value can't be used for that setting"),
            SettingsError::BadValue(n) => {
                write!(f, "line {n}: that value can't be used for that setting")
            }
            SettingsError::Twice(n) => write!(f, "line {n}: that setting is given twice"),
        }
    }
}

/// A user's settings: the pairs of the file, in the file's order, unknown keys included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pairs: Vec<(String, String)>,
}

/// Whether `key` could be a setting's name.
fn good_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY
        && key.as_bytes()[0].is_ascii_lowercase()
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` could be a keyboard layout's name: 1 to 32 lower-case letters, digits and
/// `-` (what a layout file may be called, note 0021).
pub fn good_layout_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Reads a time zone, `+HH:MM` or `-HH:MM`, as minutes east of UTC. The sign, both digits of
/// each part and the colon are all required; the hours are at most 14 and the minutes at most
/// 59, and the whole is between -12:00 and +14:00, which covers every zone in use.
pub fn timezone_minutes(value: &str) -> Option<i32> {
    let b = value.as_bytes();
    if b.len() != 6 || b[3] != b':' {
        return None;
    }
    let sign = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let two = |i: usize| {
        (b[i].is_ascii_digit() && b[i + 1].is_ascii_digit())
            .then(|| i32::from(b[i] - b'0') * 10 + i32::from(b[i + 1] - b'0'))
    };
    let (hours, minutes) = (two(1)?, two(4)?);
    if minutes > 59 {
        return None;
    }
    let total = sign * (hours * 60 + minutes);
    (-12 * 60..=14 * 60).contains(&total).then_some(total)
}

/// Whether `value` is one `key` may have. Keys this version doesn't know may have any value
/// of a sensible size: one word or several, printable ASCII.
fn good_value(key: &str, value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_VALUE {
        return false;
    }
    match key {
        LAYOUT => good_layout_name(value),
        TIMEZONE => timezone_minutes(value).is_some(),
        // No `#` (it would be read back as the start of a comment) and nothing but printable
        // ASCII, with no space at either end (which reading back would trim).
        _ => {
            value.bytes().all(|b| (b' '..=b'~').contains(&b) && b != b'#')
                && !value.starts_with(' ')
                && !value.ends_with(' ')
        }
    }
}

impl Settings {
    /// No settings: every default.
    pub fn new() -> Settings {
        Settings::default()
    }

    /// Reads a settings file.
    pub fn parse(text: &[u8]) -> Result<Settings, SettingsError> {
        let lines = lines::read(text, LIMITS).map_err(SettingsError::Read)?;
        if lines.len() > MAX_SETTINGS {
            return Err(SettingsError::TooMany);
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        for line in lines {
            if !good_key(line.key) {
                return Err(SettingsError::BadKey(line.number));
            }
            if pairs.iter().any(|p| p.0 == line.key) {
                return Err(SettingsError::Twice(line.number));
            }
            // Tabs inside a value become spaces, so that what is kept is what `good_value`
            // allows and writing it back gives a file that reads the same.
            let value = line.rest.replace('\t', " ");
            if !good_value(line.key, &value) {
                return Err(SettingsError::BadValue(line.number));
            }
            pairs.push((String::from(line.key), value));
        }
        Ok(Settings { pairs })
    }

    /// The value of `key`, if the file gives one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.iter().find(|p| p.0 == key).map(|p| p.1.as_str())
    }

    /// The keyboard layout: the file's, or `us`.
    pub fn layout(&self) -> &str {
        self.get(LAYOUT).unwrap_or(DEFAULT_LAYOUT)
    }

    /// The time zone in minutes east of UTC: the file's, or 0.
    pub fn timezone(&self) -> i32 {
        self.get(TIMEZONE).and_then(timezone_minutes).unwrap_or(0)
    }

    /// Every pair, in order, unknown keys included.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs.iter().map(|p| (p.0.as_str(), p.1.as_str()))
    }

    /// Sets `key` to `value`, replacing what it was or adding it at the end. Refuses a key or
    /// value a file couldn't hold (the error's line is 0), and a new key when the file is full.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SettingsError> {
        if !good_key(key) {
            return Err(SettingsError::BadKey(0));
        }
        if !good_value(key, value) {
            return Err(SettingsError::BadValue(0));
        }
        match self.pairs.iter_mut().find(|p| p.0 == key) {
            Some(pair) => pair.1 = String::from(value),
            None => {
                if self.pairs.len() >= MAX_SETTINGS {
                    return Err(SettingsError::TooMany);
                }
                self.pairs.push((String::from(key), String::from(value)));
            }
        }
        Ok(())
    }

    /// The file's text for these settings. [`Settings::parse`] of it gives them back.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (key, value) in &self.pairs {
            text.push_str(key);
            text.push(' ');
            text.push_str(value);
            text.push('\n');
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Settings, SettingsError> {
        Settings::parse(text.as_bytes())
    }

    #[test]
    fn the_notes_example_reads() {
        let s = parse("layout gb\ntimezone +02:00\n").unwrap();
        assert_eq!(s.layout(), "gb");
        assert_eq!(s.timezone(), 120);
        assert_eq!(s.pairs().collect::<Vec<_>>(), [("layout", "gb"), ("timezone", "+02:00")]);
    }

    #[test]
    fn an_empty_file_is_every_default() {
        for text in ["", "\n\n", "# nothing set\n", "   \t\r\n"] {
            let s = parse(text).unwrap();
            assert_eq!((s.layout(), s.timezone()), ("us", 0), "{text:?}");
            assert_eq!(s.pairs().count(), 0);
        }
        assert_eq!(Settings::new(), parse("").unwrap());
    }

    #[test]
    fn comments_windows_line_ends_and_spacing_are_fine() {
        let s = parse("# mine\r\n  layout\tus-intl   # for Afrikaans\r\n\r\ntimezone   -03:30\r\n")
            .unwrap();
        assert_eq!(s.layout(), "us-intl");
        assert_eq!(s.timezone(), -210);
    }

    #[test]
    fn unknown_keys_are_kept_and_survive_a_rewrite() {
        let text = "wallpaper mountain at dusk\nlayout gb\nclock-24h yes\n";
        let mut s = parse(text).unwrap();
        assert_eq!(s.get("wallpaper"), Some("mountain at dusk"));
        assert_eq!(s.get("clock-24h"), Some("yes"));
        assert_eq!(s.get("nothing"), None);
        // Changing one setting keeps the others, and their order.
        s.set("layout", "us").unwrap();
        assert_eq!(s.to_text(), "wallpaper mountain at dusk\nlayout us\nclock-24h yes\n");
        // A new one goes at the end.
        s.set("timezone", "+02:00").unwrap();
        assert!(s.to_text().ends_with("clock-24h yes\ntimezone +02:00\n"));
        assert_eq!(parse(&s.to_text()).unwrap(), s);
        // A tab inside a value is read as a space, so the rewrite reads the same.
        let tabbed = parse("note a\tb\n").unwrap();
        assert_eq!(tabbed.get("note"), Some("a b"));
        assert_eq!(parse(&tabbed.to_text()).unwrap(), tabbed);
    }

    #[test]
    fn time_zones_are_checked_exactly() {
        for (text, minutes) in [
            ("+00:00", 0),
            ("-00:00", 0),
            ("+02:00", 120),
            ("+05:45", 345),
            ("-09:30", -570),
            ("+14:00", 840),
            ("-12:00", -720),
        ] {
            assert_eq!(timezone_minutes(text), Some(minutes), "{text}");
        }
        for bad in [
            "",
            "0",
            "02:00",
            "+2:00",
            "+02:0",
            "+0200",
            "+02.00",
            "+02:60",
            "+14:01",
            "-12:01",
            "+24:00",
            " +02:00",
            "+02:00 ",
            "+ab:cd",
            "++2:00",
            "+02:00:00",
            "\u{2212}02:00",
        ] {
            assert_eq!(timezone_minutes(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn layout_names_are_checked() {
        for good in ["us", "gb", "us-intl", "a", "za2", &"x".repeat(32)] {
            assert!(good_layout_name(good), "{good}");
            assert_eq!(parse(&format!("layout {good}\n")).unwrap().layout(), good);
        }
        for bad in ["", "US", "us intl", "us/../x", "us.kbd", "é", &"x".repeat(33)] {
            assert!(!good_layout_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_bad_file_is_refused_whole() {
        let cases: &[(&str, SettingsError)] = &[
            ("layout\n", SettingsError::BadValue(1)),
            ("layout US\n", SettingsError::BadValue(1)),
            ("layout gb us\n", SettingsError::BadValue(1)),
            ("layout gb\ntimezone 2\n", SettingsError::BadValue(2)),
            ("timezone +02:00\n\nlayout gb\nlayout us\n", SettingsError::Twice(4)),
            ("Layout gb\n", SettingsError::BadKey(1)),
            ("9lives yes\n", SettingsError::BadKey(1)),
            ("-x yes\n", SettingsError::BadKey(1)),
            ("a_b yes\n", SettingsError::BadKey(1)),
            (
                "layout gb\nfine yes\nthis-key-is-far-too-long-to-be-a-setting x\n",
                SettingsError::BadKey(3),
            ),
            ("unknown\n", SettingsError::BadValue(1)),
            ("layout g\u{e9}\n", SettingsError::Read(ReadError::NotText)),
            ("layout gb\0\n", SettingsError::Read(ReadError::NotText)),
            ("layout gb\rtimezone +02:00\n", SettingsError::Read(ReadError::NotText)),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(text).as_ref(), Err(expected), "{text:?}");
        }
        assert_eq!(Settings::parse(&[0xFF, 0xFE]), Err(SettingsError::Read(ReadError::NotText)));
        let long_value = format!("note {}\n", "v".repeat(MAX_VALUE + 1));
        assert_eq!(parse(&long_value), Err(SettingsError::BadValue(1)));
        let long_line = format!("note {}\n", "v".repeat(300));
        assert_eq!(parse(&long_line), Err(SettingsError::Read(ReadError::LineTooLong)));
        let big = "# padding\n".repeat(MAX_SIZE / 10 + 1);
        assert_eq!(parse(&big), Err(SettingsError::Read(ReadError::TooLong)));
    }

    #[test]
    fn a_file_holds_32_settings_and_no_more() {
        let lines: Vec<String> = (0..MAX_SETTINGS).map(|i| format!("k{i} v\n")).collect();
        let mut s = parse(&lines.concat()).unwrap();
        assert_eq!(s.pairs().count(), MAX_SETTINGS);
        assert_eq!(parse(&format!("{}extra v\n", lines.concat())), Err(SettingsError::TooMany));
        // Full: an existing key can change, a new one can't be added.
        s.set("k3", "w").unwrap();
        assert_eq!(s.set("layout", "gb"), Err(SettingsError::TooMany));
        assert!(s.to_text().len() <= MAX_SIZE);
    }

    #[test]
    fn set_refuses_what_a_file_could_not_hold() {
        let mut s = Settings::new();
        assert_eq!(s.set("Layout", "gb"), Err(SettingsError::BadKey(0)));
        assert_eq!(s.set("", "gb"), Err(SettingsError::BadKey(0)));
        assert_eq!(s.set("layout", "GB"), Err(SettingsError::BadValue(0)));
        assert_eq!(s.set("layout", ""), Err(SettingsError::BadValue(0)));
        assert_eq!(s.set("timezone", "+2"), Err(SettingsError::BadValue(0)));
        // Values that would read back differently, or not at all.
        for bad in ["a # b", " a", "a ", "a\nb", "a\tb", "caf\u{e9}", &"v".repeat(MAX_VALUE + 1)] {
            assert_eq!(s.set("note", bad), Err(SettingsError::BadValue(0)), "{bad:?}");
        }
        assert_eq!(s, Settings::new(), "nothing was changed by a refused set");
        s.set("note", "two words").unwrap();
        s.set("layout", "gb").unwrap();
        assert_eq!(parse(&s.to_text()).unwrap(), s);
    }

    #[test]
    fn whatever_set_accepts_reads_back_and_damage_never_panics() {
        let mut x = 0x0123_4567_89AB_CDEFu64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let alphabet: Vec<char> = "abz09-_ A#\t\n+:é".chars().collect();
        let word = |next: &mut dyn FnMut() -> u64| -> String {
            (0..next() % 8).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect()
        };
        let mut s = Settings::new();
        for _ in 0..5000 {
            let key = match next() % 4 {
                0 => String::from("layout"),
                1 => String::from("timezone"),
                _ => word(&mut next),
            };
            let value = match next() % 5 {
                0 => String::from("+02:00"),
                1 => String::from("gb"),
                _ => word(&mut next),
            };
            if s.set(&key, &value).is_ok() {
                assert_eq!(parse(&s.to_text()).as_ref(), Ok(&s), "after set({key:?}, {value:?})");
            }
        }
        // Damage to a good file: read or refused, never a panic, and what reads keeps its
        // promises.
        let good = b"layout us-intl\ntimezone +02:00\nwallpaper mountain\n";
        for _ in 0..5000 {
            let mut text = good.to_vec();
            for _ in 0..1 + next() % 3 {
                let at = (next() % text.len() as u64) as usize;
                match next() % 3 {
                    0 => text[at] = next() as u8,
                    1 => {
                        text.remove(at);
                    }
                    _ => text.insert(at, next() as u8),
                }
                if text.is_empty() {
                    text.push(b'\n');
                }
            }
            if let Ok(s) = Settings::parse(&text) {
                assert!(good_layout_name(s.layout()));
                assert!((-720..=840).contains(&s.timezone()));
                assert_eq!(Settings::parse(s.to_text().as_bytes()).as_ref(), Ok(&s));
            }
        }
    }
}
