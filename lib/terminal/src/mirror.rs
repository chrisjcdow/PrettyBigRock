// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The mirror: in self-test runs, everything the console shows is also written to the kernel
//! log, one line at a time, because the test runner cannot see the screen. It reads the
//! kernel's serial output instead (docs/design/0018-supervision-shell-and-m5-tests.md,
//! sections 5 and 7). The formats here are what that test looks for, so they do not change
//! without changing the test.
//!
//! | Record | What it means |
//! |--------|---------------|
//! | `[console] <text>` | a complete line of text that was shown |
//! | `[console:read] <prompt>` | a `read_line` started waiting |
//! | `[console] <prompt><line>` | that line was entered |
//! | `[console:clear]` | the screen was cleared |
//!
//! This produces the lines; the console service prints them. Text is mirrored as it was
//! written, before the screen wraps it into rows, and what the screen draws as `?` is `?` here
//! too. Typing is not mirrored key by key, only the finished line.

use alloc::string::String;
use alloc::vec::Vec;

/// The longest line the mirror will hold back waiting for its `\n`. Longer text is cut into
/// lines of this length, so that text with no newline cannot use up all memory.
pub const MAX_LINE: usize = 4096;

/// What the mirror shows for `c` in a line of text: `None` for a carriage return (it only
/// moves the screen's cursor, and would make a log line end oddly), a tab as it is, any other
/// character the screen can draw as it is, and `?` for anything else, as on the screen.
fn shown(c: char) -> Option<char> {
    match c {
        '\r' => None,
        '\t' => Some('\t'),
        c => Some(crate::grid::printable(c)),
    }
}

/// `text` as the mirror shows it, with no line breaks: `\n` is shown as `?` (a prompt or a
/// typed line has none).
fn single_line(text: &str) -> String {
    text.chars().filter_map(shown).collect()
}

/// Collects what the screen shows and turns it into log lines.
#[derive(Clone, Debug, Default)]
pub struct Mirror {
    /// The start of a line whose `\n` has not come yet.
    partial: String,
    /// Finished lines, waiting to be taken.
    lines: Vec<String>,
}

impl Mirror {
    /// A mirror with nothing in it.
    pub fn new() -> Mirror {
        Mirror::default()
    }

    /// Text that was written to the screen. Complete lines (ended by `\n`) become records; the
    /// rest is held until its `\n` comes, or [`Mirror::flush`].
    pub fn text(&mut self, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                self.end_line();
            } else if let Some(c) = shown(c) {
                // Only a character that does not fit cuts the line, so a line of exactly
                // `MAX_LINE` characters followed by `\n` is one line.
                if self.partial.len() >= MAX_LINE {
                    self.end_line();
                }
                self.partial.push(c);
            }
        }
    }

    /// Makes a record of the held-back partial line, if there is one.
    pub fn flush(&mut self) {
        if !self.partial.is_empty() {
            self.end_line();
        }
    }

    fn end_line(&mut self) {
        let line = core::mem::take(&mut self.partial);
        self.lines.push(alloc::format!("[console] {line}"));
    }

    /// A `read_line` with this prompt started waiting. (Anything held back goes first, so the
    /// record of the prompt follows the text before it.)
    pub fn read_started(&mut self, prompt: &str) {
        self.flush();
        self.lines.push(alloc::format!("[console:read] {}", single_line(prompt)));
    }

    /// The line was entered. Text written without a newline while it was being typed is
    /// logged first, as a line of its own.
    pub fn line_entered(&mut self, prompt: &str, line: &str) {
        self.flush();
        self.lines.push(alloc::format!("[console] {}{}", single_line(prompt), single_line(line)));
    }

    /// The screen was cleared. (Anything held back is logged first: it is about to be gone.)
    pub fn cleared(&mut self) {
        self.flush();
        self.lines.push(String::from("[console:clear]"));
    }

    /// The records made so far, which are forgotten.
    pub fn take(&mut self) -> Vec<String> {
        core::mem::take(&mut self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(m: &mut Mirror) -> Vec<String> {
        m.take()
    }

    #[test]
    fn complete_lines_are_records() {
        let mut m = Mirror::new();
        m.text("one\ntwo\n");
        assert_eq!(taken(&mut m), ["[console] one", "[console] two"]);
        assert!(taken(&mut m).is_empty(), "taken once");
        // Blank lines are lines.
        m.text("\n\nx\n");
        assert_eq!(taken(&mut m), ["[console] ", "[console] ", "[console] x"]);
    }

    #[test]
    fn a_partial_line_waits_for_its_newline() {
        let mut m = Mirror::new();
        m.text("par");
        assert!(taken(&mut m).is_empty());
        m.text("tial\nnext");
        assert_eq!(taken(&mut m), ["[console] partial"]);
        m.text(" line\n");
        assert_eq!(taken(&mut m), ["[console] next line"]);
    }

    #[test]
    fn a_read_flushes_what_came_before_it() {
        let mut m = Mirror::new();
        m.text("held");
        m.read_started("> ");
        m.line_entered("> ", "ls -a");
        assert_eq!(taken(&mut m), ["[console] held", "[console:read] > ", "[console] > ls -a"]);
        // Nothing is left to flush.
        m.flush();
        assert!(taken(&mut m).is_empty());
    }

    #[test]
    fn text_written_during_a_read_comes_before_the_entered_line() {
        let mut m = Mirror::new();
        m.read_started("$ ");
        m.text("news\nhalf");
        m.line_entered("$ ", "x");
        assert_eq!(
            taken(&mut m),
            ["[console:read] $ ", "[console] news", "[console] half", "[console] $ x"]
        );
    }

    #[test]
    fn clearing_logs_what_it_wipes_first() {
        let mut m = Mirror::new();
        m.text("a\nb");
        m.cleared();
        m.cleared();
        assert_eq!(
            taken(&mut m),
            ["[console] a", "[console] b", "[console:clear]", "[console:clear]"]
        );
    }

    #[test]
    fn what_the_screen_cannot_draw_is_a_question_mark() {
        let mut m = Mirror::new();
        m.text("caf\u{4e2d}\r\nt\tb\x08\0\u{1F600}\n");
        m.read_started("\u{4e2d}> ");
        m.line_entered("\u{4e2d}> ", "a\nb\r");
        assert_eq!(
            taken(&mut m),
            ["[console] caf?", "[console] t\tb???", "[console:read] ?> ", "[console] ?> a?b"]
        );
    }

    #[test]
    fn a_line_without_a_newline_is_cut_at_a_limit() {
        let mut m = Mirror::new();
        m.text(&"x".repeat(MAX_LINE * 2 + 5));
        let lines = taken(&mut m);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.len() == "[console] ".len() + MAX_LINE));
        m.flush();
        assert_eq!(taken(&mut m), ["[console] xxxxx"]);
        // Exactly at the limit, with the newline after it, is one line.
        m.text(&("y".repeat(MAX_LINE) + "\n"));
        assert_eq!(taken(&mut m).len(), 1);
    }
}
