// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The whole terminal, without the hardware: a grid, the line being read (if any), and the
//! mirror. The console service has one of these and does only what needs hardware around it:
//! it feeds it text and keys, and draws what changed.
//!
//! Keeping all of this out of the service means the rules that matter (text shown above a
//! line being edited, keys dropped when nobody is reading, exactly which lines the mirror
//! logs and when) are tested on the host.

use crate::editor::{Key, LineEditor};
use crate::grid::{Changes, Grid};
use crate::mirror::Mirror;
use crate::view::{EditView, split_prompt};
use alloc::string::String;
use alloc::vec::Vec;

/// A line is already being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// A line being read: how it is shown, and the prompt (for the mirror).
#[derive(Debug)]
struct Reading {
    view: EditView,
    /// The prompt as shown on the line being edited, without any lines before it.
    prompt: String,
}

/// A text terminal: what is on the screen, and at most one line being read.
#[derive(Debug)]
pub struct Terminal {
    grid: Grid,
    editor: LineEditor,
    reading: Option<Reading>,
    mirror: Option<Mirror>,
}

impl Terminal {
    /// A blank terminal of `columns` by `rows` characters (see [`Grid::new`] for what is
    /// refused). With `mirror`, it makes the log records of [`crate::mirror`].
    pub fn new(columns: usize, rows: usize, mirror: bool) -> Option<Terminal> {
        Some(Terminal {
            grid: Grid::new(columns, rows)?,
            editor: LineEditor::new(),
            reading: None,
            mirror: mirror.then(Mirror::new),
        })
    }

    /// The screen's contents.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// What changed on the grid since the last call (see [`Changes`]).
    pub fn take_changes(&mut self) -> Changes {
        self.grid.take_changes()
    }

    /// Marks the whole grid as changed, for drawing it on a new screen.
    pub fn mark_all_dirty(&mut self) {
        self.grid.mark_all_dirty();
    }

    /// Whether a line is being read.
    pub fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Where the cursor is shown, (column, row), while a line is being read.
    pub fn caret(&self) -> Option<(usize, usize)> {
        self.reading.as_ref().map(|r| r.view.caret())
    }

    /// Shows `text` (see [`Grid::write_str`]). While a line is being read it appears above
    /// the line: the line is taken off, the text written, and the line shown again after it.
    pub fn write(&mut self, text: &str) {
        if let Some(m) = &mut self.mirror {
            m.text(text);
        }
        match &mut self.reading {
            Some(r) => {
                r.view.suspend(&mut self.grid);
                self.grid.write_str(text);
                r.view.resume(&mut self.grid, &self.editor);
            }
            None => self.grid.write_str(text),
        }
    }

    /// Starts reading a line: shows `prompt` and an empty line to edit. [`Busy`] if one is
    /// being read already.
    pub fn begin_read(&mut self, prompt: &str) -> Result<(), Busy> {
        if self.reading.is_some() {
            return Err(Busy);
        }
        let (text, shown) = split_prompt(prompt);
        if let Some(m) = &mut self.mirror {
            m.text(text);
            m.read_started(shown);
        }
        self.editor.begin();
        let view = EditView::begin(&mut self.grid, prompt, &self.editor);
        let prompt = shown.chars().map(crate::grid::printable).collect();
        self.reading = Some(Reading { view, prompt });
        Ok(())
    }

    /// A key was typed. While a line is being read it edits the line, and the finished line
    /// is the answer when the key was Enter (the screen has moved to a new row by then).
    /// When no line is being read the key is dropped.
    pub fn key(&mut self, key: Key) -> Option<String> {
        let mut reading = self.reading.take()?;
        match self.editor.key(key) {
            None => {
                reading.view.redraw(&mut self.grid, &self.editor);
                self.reading = Some(reading);
                None
            }
            Some(line) => {
                reading.view.finish(&mut self.grid);
                if let Some(m) = &mut self.mirror {
                    m.line_entered(&reading.prompt, &line);
                }
                Some(line)
            }
        }
    }

    /// Gives up reading a line (the one waiting for it has gone): what was typed stays on the
    /// screen, and the cursor moves to a new row. Nothing is mirrored; the line was not
    /// entered.
    pub fn abandon_read(&mut self) {
        if let Some(reading) = self.reading.take() {
            reading.view.finish(&mut self.grid);
            self.editor.begin();
        }
    }

    /// Empties the screen. A line being read is shown again at the top.
    pub fn clear(&mut self) {
        if let Some(m) = &mut self.mirror {
            m.cleared();
        }
        self.grid.clear();
        if let Some(r) = &mut self.reading {
            r.view.resume(&mut self.grid, &self.editor);
        }
    }

    /// Moves to a screen of another size. The text on the old one is not carried over (the
    /// new screen starts blank, with all of it to be drawn); a line being read is shown again
    /// at the top. `false`, and nothing changed, if the size is refused (see [`Grid::new`]).
    pub fn resize(&mut self, columns: usize, rows: usize) -> bool {
        let Some(grid) = Grid::new(columns, rows) else { return false };
        self.grid = grid;
        if let Some(r) = &mut self.reading {
            r.view = EditView::start(&mut self.grid, &r.prompt, &self.editor);
        }
        true
    }

    /// The mirror's records made so far, which are forgotten (always none without a mirror).
    pub fn take_mirror(&mut self) -> Vec<String> {
        self.mirror.as_mut().map(Mirror::take).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(columns: usize, rows: usize) -> Terminal {
        Terminal::new(columns, rows, true).unwrap()
    }

    fn rows(t: &Terminal) -> Vec<String> {
        (0..t.grid().rows()).map(|r| t.grid().row(r).iter().collect()).collect()
    }

    fn type_in(t: &mut Terminal, text: &str) {
        for c in text.chars() {
            assert_eq!(t.key(Key::Char(c)), None);
        }
    }

    #[test]
    fn a_shell_session() {
        let mut t = terminal(12, 5);
        t.write("Welcome\n");
        t.begin_read("$ ").unwrap();
        assert!(t.is_reading());
        type_in(&mut t, "echo hi");
        assert_eq!(t.key(Key::Enter), Some(String::from("echo hi")));
        assert!(!t.is_reading());
        t.write("hi\n");
        t.begin_read("$ ").unwrap();
        assert_eq!(
            rows(&t),
            ["Welcome     ", "$ echo hi   ", "hi          ", "$           ", "            "]
        );
        assert_eq!(t.caret(), Some((2, 3)));
        assert_eq!(
            t.take_mirror(),
            [
                "[console] Welcome",
                "[console:read] $ ",
                "[console] $ echo hi",
                "[console] hi",
                "[console:read] $ "
            ]
        );
    }

    #[test]
    fn a_second_read_is_busy_and_changes_nothing() {
        let mut t = terminal(10, 3);
        t.begin_read("a> ").unwrap();
        type_in(&mut t, "x");
        assert_eq!(t.begin_read("b> "), Err(Busy));
        assert_eq!(rows(&t)[0], "a> x      ");
        assert_eq!(t.take_mirror(), ["[console:read] a> "]);
        assert_eq!(t.key(Key::Enter), Some(String::from("x")));
        assert!(t.begin_read("b> ").is_ok());
    }

    #[test]
    fn keys_are_dropped_when_nobody_reads() {
        let mut t = terminal(10, 3);
        for key in [Key::Char('a'), Key::Enter, Key::Up, Key::Backspace] {
            assert_eq!(t.key(key), None);
        }
        assert_eq!(rows(&t), ["          "; 3]);
        assert!(t.take_changes().dirty_rows().count() == 3, "only the first, new-grid changes");
        assert!(t.take_changes().is_empty());
        // And they do not wait for the next read to be typed.
        t.begin_read("> ").unwrap();
        assert_eq!(t.key(Key::Enter), Some(String::new()));
    }

    #[test]
    fn text_written_while_reading_goes_above_the_line() {
        let mut t = terminal(10, 4);
        t.begin_read("> ").unwrap();
        type_in(&mut t, "ab");
        t.write("event\n");
        assert_eq!(rows(&t), ["event     ", "> ab      ", "          ", "          "]);
        assert_eq!(t.caret(), Some((4, 1)));
        t.write("a");
        t.write("b\n");
        assert_eq!(rows(&t)[..3], ["event     ", "ab        ", "> ab      "]);
        type_in(&mut t, "c");
        assert_eq!(t.key(Key::Enter), Some(String::from("abc")));
        assert_eq!(
            t.take_mirror(),
            ["[console:read] > ", "[console] event", "[console] ab", "[console] > abc"]
        );
    }

    #[test]
    fn history_survives_between_reads() {
        let mut t = terminal(10, 6);
        for line in ["first", "second"] {
            t.begin_read("> ").unwrap();
            type_in(&mut t, line);
            t.key(Key::Enter);
        }
        t.begin_read("> ").unwrap();
        t.key(Key::Up);
        assert_eq!(t.grid().row(2).iter().collect::<String>(), "> second  ");
        t.key(Key::Up);
        assert_eq!(t.grid().row(2).iter().collect::<String>(), "> first   ");
        assert_eq!(t.key(Key::Enter), Some(String::from("first")));
    }

    #[test]
    fn a_prompt_with_lines_before_it() {
        let mut t = terminal(10, 4);
        t.begin_read("Hello\n> ").unwrap();
        assert_eq!(rows(&t)[..2], ["Hello     ", ">         "]);
        type_in(&mut t, "y");
        assert_eq!(t.key(Key::Enter), Some(String::from("y")));
        assert_eq!(t.take_mirror(), ["[console] Hello", "[console:read] > ", "[console] > y"]);
    }

    #[test]
    fn clearing_while_reading_moves_the_line_to_the_top() {
        let mut t = terminal(10, 4);
        t.write("one\ntwo\n");
        t.begin_read("> ").unwrap();
        type_in(&mut t, "ab");
        t.clear();
        assert_eq!(rows(&t), ["> ab      ", "          ", "          ", "          "]);
        assert_eq!(t.caret(), Some((4, 0)));
        t.key(Key::Left);
        assert_eq!(t.caret(), Some((3, 0)));
        assert_eq!(
            t.take_mirror(),
            ["[console] one", "[console] two", "[console:read] > ", "[console:clear]"]
        );
        // Clearing when idle, too.
        assert_eq!(t.key(Key::Enter), Some(String::from("ab")));
        t.clear();
        assert_eq!(rows(&t), ["          "; 4]);
        assert_eq!(t.grid().cursor(), (0, 0));
        assert_eq!(t.take_mirror(), ["[console] > ab", "[console:clear]"]);
    }

    #[test]
    fn a_new_screen_of_another_size() {
        let mut t = terminal(10, 4);
        t.write("old\n");
        t.begin_read("> ").unwrap();
        type_in(&mut t, "ab");
        t.take_changes();
        assert!(!t.resize(0, 5));
        assert_eq!(t.grid().columns(), 10, "a refused size changes nothing");
        assert!(t.resize(6, 2));
        assert_eq!(rows(&t), ["> ab  ", "      "]);
        assert_eq!(t.caret(), Some((4, 0)));
        assert_eq!(t.take_changes().dirty_rows().count(), 2, "all of it is new");
        // Editing goes on.
        type_in(&mut t, "cdef");
        assert_eq!(t.key(Key::Enter), Some(String::from("abcdef")));
        // And a resize with no read going.
        assert!(t.resize(3, 3));
        assert_eq!(t.caret(), None);
    }

    #[test]
    fn abandoning_a_read_leaves_the_text_and_goes_on() {
        let mut t = terminal(10, 4);
        t.begin_read("> ").unwrap();
        type_in(&mut t, "half");
        t.abandon_read();
        assert!(!t.is_reading());
        assert_eq!(rows(&t)[0], "> half    ");
        assert_eq!(t.grid().cursor(), (0, 1));
        t.abandon_read(); // nothing to abandon
        t.begin_read("> ").unwrap();
        assert_eq!(t.key(Key::Enter), Some(String::new()), "the half line was forgotten");
        assert_eq!(t.take_mirror(), ["[console:read] > ", "[console:read] > ", "[console] > "]);
    }

    #[test]
    fn without_a_mirror_there_are_no_records() {
        let mut t = Terminal::new(10, 3, false).unwrap();
        t.write("x\n");
        t.begin_read("> ").unwrap();
        t.key(Key::Enter);
        t.clear();
        assert!(t.take_mirror().is_empty());
        assert!(Terminal::new(0, 3, false).is_none());
    }

    #[test]
    fn hostile_use_on_a_tiny_screen() {
        let mut t = terminal(1, 1);
        t.write(&"abc\n".repeat(50));
        t.begin_read(&"p".repeat(256)).unwrap();
        for c in "a long line of text that does not fit".chars() {
            t.key(Key::Char(c));
            let (col, row) = t.caret().unwrap();
            assert_eq!((col, row), (0, 0));
        }
        t.write("more\n");
        t.clear();
        t.resize(2, 1);
        t.key(Key::Home);
        assert!(t.key(Key::Enter).unwrap().starts_with("a long"));
        // A write of nothing but newlines.
        t.write(&"\n".repeat(10_000));
        assert!(!t.take_mirror().is_empty());
    }
}
