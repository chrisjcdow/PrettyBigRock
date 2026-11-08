// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The line editor: the line being typed, and the lines typed before.
//!
//! It works on abstract [`Key`]s, not on key codes: turning what a keyboard sends into keys is
//! [`crate::input`]'s job, and showing the line on the screen is [`crate::view`]'s. That keeps
//! the rules of editing (what Backspace does at the start of a line, how history works) in one
//! place that is tested on its own.
//!
//! The line is kept as a list of `char`s with the cursor as an index into it, because a user
//! thinks in characters, not bytes; but its size is limited in bytes ([`MAX_LINE_BYTES`]),
//! because it travels in a message of limited size (`pbr.terminal.read_line`).

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest line, in bytes of UTF-8 (the limit of `pbr.terminal.read_line`'s answer).
/// Typing more is ignored.
pub const MAX_LINE_BYTES: usize = 4096;

/// How many earlier lines are remembered.
pub const MAX_HISTORY: usize = 32;

/// What a key does to the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Types this character at the cursor. Control characters are ignored.
    Char(char),
    /// Deletes the character before the cursor.
    Backspace,
    /// Deletes the character at the cursor.
    Delete,
    /// Moves the cursor one character left.
    Left,
    /// Moves the cursor one character right.
    Right,
    /// Moves the cursor to the start of the line.
    Home,
    /// Moves the cursor to the end of the line.
    End,
    /// Replaces the line with the one before it in the history.
    Up,
    /// Replaces the line with the one after it in the history, or with what was being typed.
    Down,
    /// Ends the line.
    Enter,
}

/// A line being edited, with the history of earlier ones.
#[derive(Clone, Debug, Default)]
pub struct LineEditor {
    line: Vec<char>,
    /// Where typing goes, from 0 (before the first character) to `line.len()`.
    cursor: usize,
    /// The line's size in bytes of UTF-8, kept so the limit is cheap to check.
    bytes: usize,
    /// Earlier lines, oldest first.
    history: VecDeque<String>,
    /// Which history entry is shown, while Up and Down are moving through them.
    browsing: Option<usize>,
    /// What was being typed when Up was first pressed, for Down to bring back.
    draft: Vec<char>,
}

impl LineEditor {
    /// An editor with an empty line and no history.
    pub fn new() -> LineEditor {
        LineEditor::default()
    }

    /// Starts a new line: forgets what was typed (and any move through the history), but keeps
    /// the history itself.
    pub fn begin(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.bytes = 0;
        self.browsing = None;
        self.draft.clear();
    }

    /// The line so far.
    pub fn chars(&self) -> &[char] {
        &self.line
    }

    /// The line so far as a string.
    pub fn line(&self) -> String {
        self.line.iter().collect()
    }

    /// Where typing goes, as a number of characters from the start of the line.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The earlier lines, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &str> {
        self.history.iter().map(String::as_str)
    }

    /// Applies a key. Returns the finished line when the key was Enter (and the editor is
    /// ready for the next line, keeping the history); `None` for every other key.
    pub fn key(&mut self, key: Key) -> Option<String> {
        match key {
            Key::Char(c) => self.insert(c),
            Key::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.remove(self.cursor);
                }
            }
            Key::Delete => {
                if self.cursor < self.line.len() {
                    self.remove(self.cursor);
                }
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.line.len()),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.line.len(),
            Key::Up => self.up(),
            Key::Down => self.down(),
            Key::Enter => return Some(self.enter()),
        }
        None
    }

    fn insert(&mut self, c: char) {
        if c.is_control() || self.bytes + c.len_utf8() > MAX_LINE_BYTES {
            return;
        }
        self.line.insert(self.cursor, c);
        self.cursor += 1;
        self.bytes += c.len_utf8();
    }

    /// Removes the character at `at`, which must be in the line.
    fn remove(&mut self, at: usize) {
        let c = self.line.remove(at);
        self.bytes -= c.len_utf8();
    }

    /// Shows history entry `index` as the line, with the cursor at its end.
    fn recall(&mut self, index: usize) {
        self.line = self.history[index].chars().collect();
        self.bytes = self.history[index].len();
        self.cursor = self.line.len();
        self.browsing = Some(index);
    }

    fn up(&mut self) {
        let index = match self.browsing {
            _ if self.history.is_empty() => return,
            None => {
                self.draft = core::mem::take(&mut self.line);
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.recall(index);
    }

    fn down(&mut self) {
        match self.browsing {
            None => {}
            Some(i) if i + 1 < self.history.len() => self.recall(i + 1),
            Some(_) => {
                // Past the newest entry: back to what was being typed.
                self.line = core::mem::take(&mut self.draft);
                self.bytes = self.line.iter().map(|c| c.len_utf8()).sum();
                self.cursor = self.line.len();
                self.browsing = None;
            }
        }
    }

    /// Takes the line, remembers it, and gets ready for the next. An empty line, or one the
    /// same as the last, is not remembered.
    fn enter(&mut self) -> String {
        let line = self.line();
        if !line.is_empty() && self.history.back() != Some(&line) {
            if self.history.len() == MAX_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(line.clone());
        }
        self.begin();
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types a string, one `Char` key each.
    fn type_in(e: &mut LineEditor, text: &str) {
        for c in text.chars() {
            assert_eq!(e.key(Key::Char(c)), None);
        }
    }

    fn enter(e: &mut LineEditor) -> String {
        e.key(Key::Enter).expect("Enter ends the line")
    }

    const ALL_KEYS: [Key; 9] = [
        Key::Backspace,
        Key::Delete,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
        Key::Up,
        Key::Down,
        Key::Enter,
    ];

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut e = LineEditor::new();
        type_in(&mut e, "helo");
        e.key(Key::Left);
        type_in(&mut e, "l");
        assert_eq!((e.line().as_str(), e.cursor()), ("hello", 4));
        e.key(Key::Home);
        type_in(&mut e, ">");
        e.key(Key::End);
        type_in(&mut e, "!");
        assert_eq!(enter(&mut e), ">hello!");
        // The next line starts empty.
        assert_eq!((e.line().as_str(), e.cursor()), ("", 0));
    }

    #[test]
    fn backspace_and_delete() {
        let mut e = LineEditor::new();
        type_in(&mut e, "abcd");
        e.key(Key::Backspace);
        assert_eq!(e.line(), "abc");
        e.key(Key::Left);
        e.key(Key::Delete);
        assert_eq!((e.line().as_str(), e.cursor()), ("ab", 2));
        e.key(Key::Delete);
        assert_eq!(e.line(), "ab", "Delete at the end does nothing");
        e.key(Key::Home);
        e.key(Key::Backspace);
        assert_eq!(e.line(), "ab", "Backspace at the start does nothing");
        e.key(Key::Delete);
        assert_eq!((e.line().as_str(), e.cursor()), ("b", 0));
    }

    #[test]
    fn the_cursor_stops_at_both_ends() {
        let mut e = LineEditor::new();
        type_in(&mut e, "ab");
        for _ in 0..5 {
            e.key(Key::Left);
        }
        assert_eq!(e.cursor(), 0);
        for _ in 0..5 {
            e.key(Key::Right);
        }
        assert_eq!(e.cursor(), 2);
        e.key(Key::Home);
        e.key(Key::Home);
        assert_eq!(e.cursor(), 0);
        e.key(Key::End);
        e.key(Key::End);
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn every_key_on_an_empty_line() {
        for key in ALL_KEYS {
            let mut e = LineEditor::new();
            let answer = e.key(key);
            assert_eq!(answer, (key == Key::Enter).then(String::new), "{key:?}");
            assert_eq!((e.line().as_str(), e.cursor()), ("", 0), "{key:?}");
            assert_eq!(e.history().count(), 0, "an empty line is not remembered");
        }
    }

    #[test]
    fn control_characters_are_ignored_and_other_characters_are_not() {
        let mut e = LineEditor::new();
        for c in ['\n', '\r', '\t', '\0', '\x08', '\x7f', '\u{9b}'] {
            e.key(Key::Char(c));
        }
        assert_eq!(e.line(), "");
        type_in(&mut e, "caf\u{e9} \u{1F600}");
        assert_eq!(e.line(), "caf\u{e9} \u{1F600}");
        assert_eq!(e.cursor(), 6, "counted in characters");
        e.key(Key::Backspace);
        e.key(Key::Backspace);
        e.key(Key::Backspace);
        assert_eq!(e.line(), "caf");
        // The byte count follows removals, or the limit below would drift.
        type_in(&mut e, &"x".repeat(MAX_LINE_BYTES - 3));
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
    }

    #[test]
    fn a_line_is_at_most_4096_bytes() {
        let mut e = LineEditor::new();
        type_in(&mut e, &"a".repeat(MAX_LINE_BYTES + 100));
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
        // More is ignored, at the cursor too.
        e.key(Key::Home);
        type_in(&mut e, "bbb");
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
        assert!(e.line().starts_with('a'));
        // A character of several bytes that does not fit is ignored; one byte fewer, and a
        // character that does fit is not.
        e.key(Key::Backspace); // at the start: nothing
        e.key(Key::Delete);
        assert_eq!(e.line().len(), MAX_LINE_BYTES - 1);
        e.key(Key::Char('\u{e9}')); // two bytes: one too many
        assert_eq!(e.line().len(), MAX_LINE_BYTES - 1);
        e.key(Key::Char('b'));
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
        let answer = enter(&mut e);
        assert_eq!(answer.len(), MAX_LINE_BYTES);
        assert!(answer.starts_with('b'));
        // A line of the largest characters stops at the limit as well.
        type_in(&mut e, &"\u{1F600}".repeat(MAX_LINE_BYTES));
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
        assert_eq!(e.chars().len(), MAX_LINE_BYTES / 4);
    }

    #[test]
    fn up_and_down_move_through_the_history() {
        let mut e = LineEditor::new();
        for line in ["one", "two", "three"] {
            type_in(&mut e, line);
            enter(&mut e);
        }
        assert_eq!(e.history().collect::<Vec<_>>(), ["one", "two", "three"]);
        type_in(&mut e, "dra");
        e.key(Key::Up);
        assert_eq!((e.line().as_str(), e.cursor()), ("three", 5));
        e.key(Key::Up);
        assert_eq!(e.line(), "two");
        e.key(Key::Up);
        e.key(Key::Up);
        e.key(Key::Up);
        assert_eq!(e.line(), "one", "stops at the oldest");
        e.key(Key::Down);
        assert_eq!(e.line(), "two");
        e.key(Key::Down);
        e.key(Key::Down);
        assert_eq!((e.line().as_str(), e.cursor()), ("dra", 3), "the line being typed is back");
        e.key(Key::Down);
        assert_eq!(e.line(), "dra", "and Down past it does nothing");
        // Typing goes on from there.
        type_in(&mut e, "ft");
        assert_eq!(enter(&mut e), "draft");
        // A recalled line can be edited and entered.
        e.key(Key::Up);
        e.key(Key::Up);
        assert_eq!(e.line(), "three");
        e.key(Key::Backspace);
        type_in(&mut e, "ee!");
        assert_eq!(enter(&mut e), "threee!");
        assert_eq!(e.history().last(), Some("threee!"));
    }

    #[test]
    fn up_and_down_with_no_history_do_nothing() {
        let mut e = LineEditor::new();
        type_in(&mut e, "abc");
        e.key(Key::Up);
        assert_eq!(e.line(), "abc");
        e.key(Key::Down);
        assert_eq!((e.line().as_str(), e.cursor()), ("abc", 3));
    }

    #[test]
    fn empty_lines_and_repeats_are_not_remembered() {
        let mut e = LineEditor::new();
        for line in ["a", "", "a", "b", "b", "a"] {
            type_in(&mut e, line);
            assert_eq!(enter(&mut e), line);
        }
        assert_eq!(e.history().collect::<Vec<_>>(), ["a", "b", "a"]);
    }

    #[test]
    fn only_32_lines_are_remembered() {
        let mut e = LineEditor::new();
        for i in 0..100 {
            type_in(&mut e, &i.to_string());
            enter(&mut e);
        }
        assert_eq!(e.history().count(), MAX_HISTORY);
        assert_eq!(e.history().next(), Some("68"));
        assert_eq!(e.history().last(), Some("99"));
        for _ in 0..200 {
            e.key(Key::Up);
        }
        assert_eq!(e.line(), "68");
    }

    #[test]
    fn begin_forgets_the_line_but_not_the_history() {
        let mut e = LineEditor::new();
        type_in(&mut e, "old");
        enter(&mut e);
        type_in(&mut e, "half");
        e.key(Key::Up);
        e.begin();
        assert_eq!((e.line().as_str(), e.cursor()), ("", 0));
        e.key(Key::Down);
        assert_eq!(e.line(), "", "the browsing was forgotten, so Down has nowhere to go");
        e.key(Key::Up);
        assert_eq!(e.line(), "old");
    }

    #[test]
    fn a_recalled_long_line_still_counts_its_bytes() {
        let mut e = LineEditor::new();
        type_in(&mut e, &"\u{e9}".repeat(MAX_LINE_BYTES / 2));
        enter(&mut e);
        e.key(Key::Up);
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
        e.key(Key::Char('x'));
        assert_eq!(e.line().len(), MAX_LINE_BYTES, "full, so no more fits");
        e.key(Key::Backspace);
        e.key(Key::Char('x'));
        assert_eq!(e.line().len(), MAX_LINE_BYTES - 1, "one two-byte character became one byte");
        // Down to the draft, which was empty, and the count follows.
        e.key(Key::Down);
        assert_eq!(e.line(), "");
        type_in(&mut e, &"y".repeat(MAX_LINE_BYTES + 1));
        assert_eq!(e.line().len(), MAX_LINE_BYTES);
    }

    /// Any run of keys leaves the editor in a state that holds together.
    #[test]
    fn random_keys_never_break_it() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let mut e = LineEditor::new();
        for _ in 0..20_000 {
            let r = next();
            let key = match r % 12 {
                0..=3 => Key::Char(char::from_u32((next() % 0x500) as u32).unwrap_or('?')),
                4 => Key::Char('\u{1F600}'),
                n => ALL_KEYS[(n as usize - 5) % ALL_KEYS.len()],
            };
            let before_history = e.history().count();
            if let Some(line) = e.key(key) {
                assert!(line.len() <= MAX_LINE_BYTES);
                assert!(!line.chars().any(char::is_control));
                assert!(e.history().count() <= MAX_HISTORY);
                assert!(e.history().count() >= before_history.min(MAX_HISTORY));
            }
            assert!(e.cursor() <= e.chars().len());
            assert!(e.line().len() <= MAX_LINE_BYTES);
        }
    }
}
