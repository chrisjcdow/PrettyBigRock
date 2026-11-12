// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The reader for `.kbd` layout files (note 0021, section 4).
//!
//! It has its own line reader rather than `lib/manifest`'s, because the keys of a layout type
//! `#` and `"` and letters outside ASCII, which that reader gives other meanings or refuses.
//! Here a comment is a line that *starts* with `#`, and nothing else is a comment.
//!
//! Like every reader of a file that decides what happens when someone presses a key, it
//! refuses the whole file for anything it does not understand.

use crate::error::{ParseError, Problem};
use crate::layout::{DeadTable, LEVELS, Layout};
use crate::{DeadKey, MAX_LINE, MAX_SIZE, Typed};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use pbr_devices::keys::{ALL, key};

/// The most dead keys a layout may have.
const MAX_DEAD: usize = 16;
/// The most pairs one dead key may have.
const MAX_PAIRS: usize = 64;
/// The longest title, in bytes.
const MAX_TITLE: usize = 64;
/// The longest dead key name.
const MAX_DEAD_NAME: usize = 16;

/// Reads a layout file.
///
/// `&[u8]` is a borrowed view of bytes: the function reads them without taking them over. The
/// answer is a `Result`: either the layout (`Ok`) or the reason for refusing it (`Err`).
pub fn parse(text: &[u8]) -> Result<Layout, ParseError> {
    if text.len() > MAX_SIZE {
        return Err(ParseError { line: 0, problem: Problem::FileTooLong });
    }
    let mut state = State::default();
    // Splitting at every `\n` also gives the (possibly empty) piece after the last one, which
    // is how a file with no final newline still has its last line read.
    let pieces: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    for (i, &raw) in pieces.iter().enumerate() {
        let number = i + 1;
        let fail = |problem| ParseError { line: number, problem };
        // A `\r` ends a line only as part of `\r\n`, so the last piece, which no `\n`
        // follows, keeps its `\r` (and is refused for it).
        let raw = if i + 1 < pieces.len() { raw.strip_suffix(b"\r").unwrap_or(raw) } else { raw };
        let line = check_text(raw).map_err(fail)?;
        state.line(number, line).map_err(fail)?;
    }
    state.finish()
}

/// Checks that one line is short, free of control characters and UTF-8, and gives it as text.
fn check_text(raw: &[u8]) -> Result<&str, Problem> {
    if raw.len() > MAX_LINE {
        return Err(Problem::LineTooLong);
    }
    // Tab is the one control character allowed. A `\r` is only allowed at the very end of a
    // line (already removed), so one that is left is a stray.
    if raw.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7F) {
        return Err(Problem::ControlCharacter);
    }
    core::str::from_utf8(raw).map_err(|_| Problem::NotUtf8)
}

/// What one value of a `key` line says, before dead keys are matched to their tables.
#[derive(Clone, Debug)]
enum Value {
    Nothing,
    Char(char),
    Dead(String),
}

/// A `key` line, kept until the whole file has been read.
struct PendingKey {
    line: usize,
    code: u16,
    levels: [Value; LEVELS],
    /// Whether it gave a value for the AltGr levels.
    uses_altgr: bool,
}

/// A `dead` line, kept the same way.
struct PendingDead {
    line: usize,
    name: String,
    pairs: Vec<(char, char)>,
}

/// Everything read so far.
#[derive(Default)]
struct State {
    title: Option<String>,
    altgr: bool,
    keys: Vec<PendingKey>,
    dead: Vec<PendingDead>,
}

impl State {
    /// Takes one line of text.
    fn line(&mut self, number: usize, line: &str) -> Result<(), Problem> {
        let line = line.trim_matches([' ', '\t']);
        // A comment is a whole line. After the first word, `#` is just a character.
        if line.is_empty() || line.starts_with('#') {
            return Ok(());
        }
        let (word, rest) = match line.find([' ', '\t']) {
            Some(at) => (&line[..at], line[at..].trim_start_matches([' ', '\t'])),
            None => (line, ""),
        };
        match word {
            "title" => self.title_line(rest),
            "altgr" => {
                if !rest.is_empty() {
                    Err(Problem::AltgrWithWords)
                } else if self.altgr {
                    Err(Problem::AltgrTwice)
                } else {
                    self.altgr = true;
                    Ok(())
                }
            }
            "key" => self.key_line(number, rest),
            "dead" => self.dead_line(number, rest),
            _ => Err(Problem::UnknownWord),
        }
    }

    fn title_line(&mut self, rest: &str) -> Result<(), Problem> {
        if self.title.is_some() {
            return Err(Problem::TitleTwice);
        }
        let inner =
            rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')).ok_or(Problem::BadTitle)?;
        let good = (1..=MAX_TITLE).contains(&inner.len())
            && !inner.contains('"')
            && !inner.chars().any(char::is_control)
            && !inner.trim().is_empty();
        if !good {
            return Err(Problem::BadTitle);
        }
        self.title = Some(inner.to_string());
        Ok(())
    }

    fn key_line(&mut self, number: usize, rest: &str) -> Result<(), Problem> {
        let mut words = rest.split([' ', '\t']).filter(|w| !w.is_empty());
        let name = words.next().ok_or(Problem::NoKeyName)?;
        let code = ALL
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, code)| code)
            .ok_or(Problem::UnknownKey)?;
        if !is_typing_key(code) {
            return Err(Problem::ControlKey);
        }
        if self.keys.iter().any(|k| k.code == code) {
            return Err(Problem::KeyTwice);
        }
        let mut given = Vec::new();
        for word in words {
            if given.len() == LEVELS {
                return Err(Problem::WrongValueCount);
            }
            given.push(value(word).ok_or(Problem::BadValue)?);
        }
        if given.is_empty() {
            return Err(Problem::WrongValueCount);
        }
        let uses_altgr = given.len() > 2;
        // Shift left out means the same as plain; the AltGr levels left out type nothing.
        let at = |i: usize| given.get(i).cloned();
        let normal = at(0).unwrap_or(Value::Nothing);
        let levels = [
            normal.clone(),
            at(1).unwrap_or(normal.clone()),
            at(2).unwrap_or(Value::Nothing),
            at(3).unwrap_or(Value::Nothing),
        ];
        if levels.iter().all(|v| matches!(v, Value::Nothing)) {
            return Err(Problem::KeyTypesNothing);
        }
        self.keys.push(PendingKey { line: number, code, levels, uses_altgr });
        Ok(())
    }

    fn dead_line(&mut self, number: usize, rest: &str) -> Result<(), Problem> {
        let mut words = rest.split([' ', '\t']).filter(|w| !w.is_empty());
        let name = words.next().filter(|n| good_dead_name(n)).ok_or(Problem::BadDeadName)?;
        if self.dead.iter().any(|d| d.name == name) {
            return Err(Problem::DeadTwice);
        }
        if self.dead.len() == MAX_DEAD {
            return Err(Problem::TooManyDead);
        }
        let words: Vec<&str> = words.collect();
        if !words.len().is_multiple_of(2) {
            return Err(Problem::OddDeadWords);
        }
        if words.len() / 2 > MAX_PAIRS {
            return Err(Problem::TooManyPairs);
        }
        let mut pairs: Vec<(char, char)> = Vec::new();
        for pair in words.chunks_exact(2) {
            let base = if pair[0] == "space" { Some(' ') } else { one_char(pair[0]) };
            let base = base.ok_or(Problem::BadDeadCharacter)?;
            let result = one_char(pair[1]).ok_or(Problem::BadDeadCharacter)?;
            if pairs.iter().any(|&(b, _)| b == base) {
                return Err(Problem::DeadBaseTwice);
            }
            pairs.push((base, result));
        }
        if !pairs.iter().any(|&(b, _)| b == ' ') {
            return Err(Problem::DeadNeedsSpace);
        }
        self.dead.push(PendingDead { line: number, name: name.to_string(), pairs });
        Ok(())
    }

    /// The checks that need the whole file, then the layout itself.
    fn finish(self) -> Result<Layout, ParseError> {
        // Every problem found here has a line; the earliest one is reported.
        let mut found: Vec<(usize, Problem)> = Vec::new();
        let mut used = vec![false; self.dead.len()];
        let mut slots = vec![[Typed::Nothing; LEVELS]; usize::from(highest_code()) + 1];
        for k in &self.keys {
            if k.uses_altgr && !self.altgr {
                found.push((k.line, Problem::AltgrWithoutLine));
            }
            for (level, v) in k.levels.iter().enumerate() {
                slots[usize::from(k.code)][level] = match v {
                    Value::Nothing => Typed::Nothing,
                    Value::Char(c) => Typed::Char(*c),
                    Value::Dead(name) => match self.dead.iter().position(|d| d.name == *name) {
                        Some(i) => {
                            used[i] = true;
                            // At most 16 tables, so the index fits in a byte.
                            Typed::Dead(DeadKey(i as u8))
                        }
                        None => {
                            found.push((k.line, Problem::UnknownDead));
                            Typed::Nothing
                        }
                    },
                };
            }
        }
        for (d, used) in self.dead.iter().zip(&used) {
            if !used {
                found.push((d.line, Problem::UnusedDead));
            }
        }
        if let Some(&(line, problem)) = found.iter().min_by_key(|(line, _)| *line) {
            return Err(ParseError { line, problem });
        }
        let Some(title) = self.title else {
            return Err(ParseError { line: 0, problem: Problem::TitleMissing });
        };
        let dead = self.dead.into_iter().map(|d| DeadTable { name: d.name, pairs: d.pairs });
        Ok(Layout { title, altgr: self.altgr, slots, dead: dead.collect() })
    }
}

/// The largest key code there is.
fn highest_code() -> u16 {
    ALL.iter().map(|&(_, code)| code).max().unwrap_or(0)
}

/// Whether a key is one that layouts write: letters, digits, Space, the punctuation keys, the
/// key beside the left Shift, and the keypad's digits and operators. Enter, Tab, Shift, the
/// arrows and the like are the same on every layout and are not in the files.
fn is_typing_key(code: u16) -> bool {
    (key::A..=key::Z).contains(&code)
        || (key::D1..=key::D0).contains(&code)
        || (key::SPACE..=key::SLASH).contains(&code)
        || code == key::K102ND
        || (key::KP_0..=key::KP_DIVIDE).contains(&code)
}

/// A dead key's name: 1 to 16 lower-case ASCII letters.
fn good_dead_name(name: &str) -> bool {
    (1..=MAX_DEAD_NAME).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase())
}

/// A word that is exactly one character, which is not a control character or a space.
fn one_char(word: &str) -> Option<char> {
    let mut chars = word.chars();
    let c = chars.next()?;
    (chars.next().is_none() && !c.is_control() && !c.is_whitespace()).then_some(c)
}

/// One value of a `key` line.
fn value(word: &str) -> Option<Value> {
    match word {
        "space" => Some(Value::Char(' ')),
        "none" => Some(Value::Nothing),
        _ => match word.strip_prefix("dead:") {
            Some(name) => good_dead_name(name).then(|| Value::Dead(name.to_string())),
            None => one_char(word).map(Value::Char),
        },
    }
}
