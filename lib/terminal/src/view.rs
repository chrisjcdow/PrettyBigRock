// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Showing the line editor on the grid: the prompt and the line being typed, after whatever
//! text is already there.
//!
//! The editor ([`crate::editor`]) knows the line; the grid ([`crate::grid`]) knows the screen.
//! This is the part in between, and it is where the fiddly cases live: a line longer than a
//! row wraps; one that reaches the bottom scrolls the screen; text written by someone else
//! while the line is being edited must appear *above* it, not on top of it.
//!
//! Positions are worked out as one number counting cells along the grid, row after row (the
//! "linear" position: `row * columns + column`). A line that wraps is then just a run of
//! consecutive cells, whatever the column it starts in.

use crate::editor::LineEditor;
use crate::grid::Grid;
use alloc::vec::Vec;

/// The prompt and line of one `read_line`, as painted on a grid.
#[derive(Clone, Debug)]
pub struct EditView {
    /// The prompt as it is shown: printable characters only, and without any lines before
    /// its last `\n` (those are ordinary text, written when the view began).
    prompt: Vec<char>,
    /// The linear position of the prompt's first cell. It is negative if the line is so long
    /// that its beginning has scrolled off the top of the screen.
    start: isize,
    /// How many cells (prompt and line) were painted last time.
    painted: usize,
    /// Where the cursor is shown, from the last paint: (column, row).
    caret: (usize, usize),
}

impl EditView {
    /// Starts editing at the grid's cursor and paints the prompt and line. Anything in
    /// `prompt` up to its last `\n` is written to the grid as ordinary text first; the rest
    /// is the prompt that is repainted after every edit.
    pub fn begin(grid: &mut Grid, prompt: &str, editor: &LineEditor) -> EditView {
        let (text, shown) = split_prompt(prompt);
        grid.write_str(text);
        EditView::start(grid, shown, editor)
    }

    /// Like [`EditView::begin`] for a prompt that has no `\n` in it (the rest of one that
    /// had): starts at the grid's cursor and paints.
    pub fn start(grid: &mut Grid, prompt: &str, editor: &LineEditor) -> EditView {
        let mut view = EditView {
            prompt: prompt.chars().map(crate::grid::printable).collect(),
            start: 0,
            painted: 0,
            caret: (0, 0),
        };
        view.start_at_cursor(grid);
        view.redraw(grid, editor);
        view
    }

    /// Where the cursor is shown on the grid, as (column, row). It is always inside the grid.
    pub fn caret(&self) -> (usize, usize) {
        self.caret
    }

    /// Makes the first prompt cell the one the grid's cursor is on (the next row's first, if
    /// the cursor is at the end of a full row), and forgets what was painted.
    fn start_at_cursor(&mut self, grid: &mut Grid) {
        grid.wrap_pending();
        let (col, row) = grid.cursor();
        self.start = (row * grid.columns() + col) as isize;
        self.painted = 0;
    }

    /// Paints the prompt and line again after an edit, on top of what was painted before:
    /// cells that are no longer part of it are blanked, and the grid is scrolled if the line
    /// has grown past the bottom. The grid's cursor ends up after the last character.
    pub fn redraw(&mut self, grid: &mut Grid, editor: &LineEditor) {
        let columns = grid.columns() as isize;
        let total = columns * grid.rows() as isize;
        let line = editor.chars();
        let length = self.prompt.len() + line.len();
        let caret = (self.prompt.len() + editor.cursor()) as isize;

        // Scroll just far enough for the whole line to fit, except that the cursor must stay
        // on the screen even if the end of the line cannot: its row, and no rows above that,
        // are all that can ever be scrolled away.
        let last = (length as isize - 1).max(caret);
        let end = self.start + last;
        let to_fit = if end >= total { (end - total) / columns + 1 } else { 0 };
        let to_keep_caret = (self.start + caret).max(0) / columns;
        let scroll = to_fit.min(to_keep_caret);
        grid.scroll_up(scroll as usize);
        self.start -= scroll * columns;

        // Blank what was painted before and is not painted now, then paint.
        let first_unused = self.start + length as isize;
        for linear in first_unused..self.start + self.painted as isize {
            put_linear(grid, linear, ' ');
        }
        for (i, c) in self.prompt.iter().chain(line).enumerate() {
            put_linear(grid, self.start + i as isize, *c);
        }
        self.painted = length;

        let caret_at = (self.start + caret).clamp(0, total - 1);
        self.caret = ((caret_at % columns) as usize, (caret_at / columns) as usize);
        self.move_grid_cursor_to_end(grid);
    }

    /// Puts the grid's cursor after the last painted cell, where text written next goes.
    fn move_grid_cursor_to_end(&self, grid: &mut Grid) {
        let columns = grid.columns() as isize;
        let total = columns * grid.rows() as isize;
        let end = (self.start + self.painted as isize).clamp(0, total);
        if self.painted > 0 && end > 0 && end % columns == 0 {
            // The last row painted is full: stay at its end, as writing text would.
            grid.set_cursor(grid.columns(), (end / columns - 1) as usize);
        } else {
            grid.set_cursor((end % columns) as usize, (end / columns) as usize);
        }
    }

    /// Takes the prompt and line off the grid and puts the grid's cursor where the prompt
    /// began, so that text can be written there. Call [`EditView::resume`] afterwards.
    pub fn suspend(&mut self, grid: &mut Grid) {
        for linear in self.start..self.start + self.painted as isize {
            put_linear(grid, linear, ' ');
        }
        self.painted = 0;
        let columns = grid.columns() as isize;
        let at = self.start.clamp(0, columns * grid.rows() as isize - 1);
        grid.set_cursor((at % columns) as usize, (at / columns) as usize);
    }

    /// Paints the prompt and line again after the grid's cursor, wherever text written since
    /// [`EditView::suspend`] left it. Also the way to carry on after the grid was cleared,
    /// when the prompt goes back to the top.
    pub fn resume(&mut self, grid: &mut Grid, editor: &LineEditor) {
        self.start_at_cursor(grid);
        self.redraw(grid, editor);
    }

    /// Ends editing: leaves the prompt and line on the grid and moves to the next row.
    pub fn finish(self, grid: &mut Grid) {
        self.move_grid_cursor_to_end(grid);
        grid.write_str("\n");
    }
}

/// Puts `c` at linear position `linear` if that is inside the grid.
fn put_linear(grid: &mut Grid, linear: isize, c: char) {
    let columns = grid.columns() as isize;
    if linear >= 0 && linear < columns * grid.rows() as isize {
        grid.put_at((linear % columns) as usize, (linear / columns) as usize, c);
    }
}

/// Splits a prompt into the text up to and including its last `\n` and the rest.
pub fn split_prompt(prompt: &str) -> (&str, &str) {
    match prompt.rfind('\n') {
        Some(i) => prompt.split_at(i + 1),
        None => ("", prompt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Key;
    use alloc::string::String;

    fn rows(g: &Grid) -> Vec<String> {
        (0..g.rows()).map(|r| g.row(r).iter().collect()).collect()
    }

    fn type_in(e: &mut LineEditor, text: &str) {
        for c in text.chars() {
            e.key(Key::Char(c));
        }
    }

    #[test]
    fn the_prompt_and_line_follow_the_text_already_there() {
        let mut g = Grid::new(10, 4).unwrap();
        g.write_str("hi\n");
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        assert_eq!(rows(&g)[..2], ["hi        ", ">         "]);
        assert_eq!(v.caret(), (2, 1));
        type_in(&mut e, "ls");
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g)[1], "> ls      ");
        assert_eq!((v.caret(), g.cursor()), ((4, 1), (4, 1)));
        e.key(Key::Left);
        v.redraw(&mut g, &e);
        assert_eq!((v.caret(), g.cursor()), ((3, 1), (4, 1)), "the caret moves, the end does not");
        e.key(Key::Backspace);
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g)[1], "> s       ", "what was painted past the end is blanked");
        assert_eq!((v.caret(), g.cursor()), ((2, 1), (3, 1)));
        let line = e.key(Key::Enter).unwrap();
        assert_eq!(line, "s");
        v.finish(&mut g);
        assert_eq!(g.cursor(), (0, 2));
        assert_eq!(rows(&g)[1], "> s       ", "and stays on the screen");
    }

    #[test]
    fn a_prompt_after_unfinished_text_follows_it() {
        let mut g = Grid::new(10, 3).unwrap();
        g.write_str("abc");
        let e = LineEditor::new();
        let v = EditView::begin(&mut g, "$ ", &e);
        assert_eq!(rows(&g)[0], "abc$      ");
        assert_eq!(v.caret(), (5, 0));
    }

    #[test]
    fn a_prompt_starting_with_a_full_row_starts_on_the_next() {
        let mut g = Grid::new(4, 3).unwrap();
        g.write_str("abcd");
        let e = LineEditor::new();
        let v = EditView::begin(&mut g, "> ", &e);
        assert_eq!(rows(&g), ["abcd", ">   ", "    "]);
        assert_eq!(v.caret(), (2, 1));
    }

    #[test]
    fn lines_before_the_last_newline_of_a_prompt_are_text() {
        let mut g = Grid::new(10, 4).unwrap();
        let e = LineEditor::new();
        let v = EditView::begin(&mut g, "one\ntwo\n> ", &e);
        assert_eq!(rows(&g)[..3], ["one       ", "two       ", ">         "]);
        assert_eq!(v.caret(), (2, 2));
        assert_eq!(split_prompt("a\nb"), ("a\n", "b"));
        assert_eq!(split_prompt("ab"), ("", "ab"));
        assert_eq!(split_prompt("ab\n"), ("ab\n", ""));
        assert_eq!(split_prompt(""), ("", ""));
        // Characters the font cannot draw are shown as the font shows them.
        let mut g = Grid::new(6, 1).unwrap();
        EditView::begin(&mut g, "\u{4e2d}\t>", &LineEditor::new());
        assert_eq!(rows(&g), ["??>   "]);
    }

    #[test]
    fn a_long_line_wraps_and_the_caret_follows() {
        let mut g = Grid::new(5, 4).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        type_in(&mut e, "abcdefgh");
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g), ["> abc", "defgh", "     ", "     "]);
        // The caret is at the start of the next row: that is where typing goes.
        assert_eq!(v.caret(), (0, 2));
        e.key(Key::Home);
        v.redraw(&mut g, &e);
        assert_eq!(v.caret(), (2, 0));
        // The line's cursor after "abc": the first cell of the second row.
        for _ in 0..3 {
            e.key(Key::Right);
        }
        v.redraw(&mut g, &e);
        assert_eq!(v.caret(), (0, 1));
        assert_eq!(g.cursor(), (5, 1), "text written next would continue the full row");
    }

    #[test]
    fn a_line_that_reaches_the_bottom_scrolls_the_screen() {
        let mut g = Grid::new(4, 3).unwrap();
        g.write_str("old1\nold2\n");
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        assert_eq!(rows(&g), ["old1", "old2", ">   "]);
        g.take_changes();
        type_in(&mut e, "abcdef"); // 8 cells: two rows from row 2
        v.redraw(&mut g, &e);
        // Eight cells, and the caret on the cell after them: three rows from the prompt's.
        assert_eq!(rows(&g), ["> ab", "cdef", "    "]);
        assert_eq!(v.caret(), (0, 2));
        type_in(&mut e, "g");
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g), ["> ab", "cdef", "g   "]);
        assert_eq!(v.caret(), (1, 2));
        assert_eq!(g.take_changes().scrolled, 2, "the old text went off the top");
        // Back up: the screen does not scroll back, and the end is blanked.
        e.key(Key::Backspace);
        e.key(Key::Backspace);
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g), ["> ab", "cde ", "    "]);
        let line = e.key(Key::Enter).unwrap();
        assert_eq!(line, "abcde");
        v.finish(&mut g);
        assert_eq!(g.cursor(), (0, 2), "the next row");
        assert_eq!(rows(&g), ["> ab", "cde ", "    "]);
    }

    #[test]
    fn text_written_while_editing_goes_above_the_line() {
        let mut g = Grid::new(8, 4).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        type_in(&mut e, "ab");
        v.redraw(&mut g, &e);
        e.key(Key::Left);
        v.redraw(&mut g, &e);
        v.suspend(&mut g);
        assert_eq!(rows(&g)[0], "        ");
        assert_eq!(g.cursor(), (0, 0));
        g.write_str("news\n");
        v.resume(&mut g, &e);
        assert_eq!(rows(&g), ["news    ", "> ab    ", "        ", "        "]);
        assert_eq!(v.caret(), (3, 1), "the caret is where it was in the line");
        // Text with no newline: the prompt follows it.
        v.suspend(&mut g);
        g.write_str("more");
        v.resume(&mut g, &e);
        assert_eq!(rows(&g)[1], "more> ab");
        // Typing goes on as before.
        e.key(Key::End);
        type_in(&mut e, "c");
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g)[1], "more> ab");
        assert_eq!(rows(&g)[2], "c       ");
    }

    #[test]
    fn writing_enough_while_editing_scrolls_the_line_up_with_it() {
        let mut g = Grid::new(6, 3).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        type_in(&mut e, "x");
        v.redraw(&mut g, &e);
        v.suspend(&mut g);
        g.write_str("1\n2\n3\n4\n");
        v.resume(&mut g, &e);
        assert_eq!(rows(&g), ["3     ", "4     ", "> x   "]);
        assert_eq!(v.caret(), (3, 2));
    }

    #[test]
    fn clearing_puts_the_prompt_at_the_top() {
        let mut g = Grid::new(6, 3).unwrap();
        g.write_str("a\nb\n");
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        type_in(&mut e, "hey");
        v.redraw(&mut g, &e);
        g.clear();
        v.resume(&mut g, &e);
        assert_eq!(rows(&g), ["> hey ", "      ", "      "]);
        assert_eq!(v.caret(), (5, 0));
    }

    #[test]
    fn the_end_of_a_full_screen_is_handled() {
        // Five cells and a caret after them just fit.
        let mut g = Grid::new(3, 2).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, ">", &e);
        type_in(&mut e, "abcd");
        v.redraw(&mut g, &e);
        assert_eq!(rows(&g), [">ab", "cd "]);
        assert_eq!(v.caret(), (2, 1));
        type_in(&mut e, "e");
        v.redraw(&mut g, &e);
        // Six cells fill the grid, and the caret needs a seventh: the screen scrolls, and the
        // start of the line, now off the top, is lost.
        assert_eq!(rows(&g), ["cde", "   "]);
        assert_eq!(v.caret(), (0, 1));
        assert_eq!(g.cursor(), (3, 0), "text written next continues the full row");
    }

    /// A line longer than the whole screen: the caret stays on the screen, and nothing panics.
    #[test]
    fn a_line_taller_than_the_screen() {
        let mut g = Grid::new(4, 2).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        type_in(&mut e, &"abcdefghijklmnopqrstuvwxyz".repeat(3));
        v.redraw(&mut g, &e);
        let (col, row) = v.caret();
        assert!(col < 4 && row < 2);
        for key in [Key::Home, Key::Left, Key::End, Key::Left, Key::Left, Key::Home] {
            e.key(key);
            v.redraw(&mut g, &e);
            let (col, row) = v.caret();
            assert!(col < 4 && row < 2, "{key:?}");
        }
        v.suspend(&mut g);
        g.write_str("x");
        v.resume(&mut g, &e);
        e.key(Key::Enter);
        v.finish(&mut g);
    }

    #[test]
    fn a_one_by_one_screen() {
        let mut g = Grid::new(1, 1).unwrap();
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        for key in [Key::Char('a'), Key::Char('b'), Key::Left, Key::Backspace, Key::Delete] {
            e.key(key);
            v.redraw(&mut g, &e);
            assert_eq!(v.caret(), (0, 0));
        }
        v.suspend(&mut g);
        v.resume(&mut g, &e);
        v.finish(&mut g);
    }

    #[test]
    fn nothing_but_the_editors_cells_is_touched() {
        let mut g = Grid::new(10, 5).unwrap();
        g.write_str("keep me\nand me\n");
        let mut e = LineEditor::new();
        let mut v = EditView::begin(&mut g, "> ", &e);
        for c in "hello world, this is long".chars() {
            e.key(Key::Char(c));
            v.redraw(&mut g, &e);
        }
        for _ in 0..30 {
            e.key(Key::Backspace);
            v.redraw(&mut g, &e);
        }
        assert_eq!(
            rows(&g),
            ["keep me   ", "and me    ", ">         ", "          ", "          "]
        );
        assert_eq!(g.take_changes().scrolled, 0);
    }

    /// Hostile use: any mix of keys, writes, clears and finishes, on grids of any shape, never
    /// panics and always has the caret on the screen.
    #[test]
    fn random_use_never_panics() {
        let mut seed = 77u64;
        let mut next = move || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for (columns, rows) in [(1, 1), (1, 3), (3, 1), (2, 2), (5, 3), (20, 6)] {
            let mut g = Grid::new(columns, rows).unwrap();
            let mut e = LineEditor::new();
            let mut v = EditView::begin(&mut g, "p> ", &e);
            for _ in 0..4000 {
                match next() % 12 {
                    0 => {
                        v.suspend(&mut g);
                        g.write_str(
                            ["a", "bc\n", "\n\n", "\t", "defghijkl"][(next() % 5) as usize],
                        );
                        v.resume(&mut g, &e);
                    }
                    1 => {
                        g.clear();
                        v.resume(&mut g, &e);
                    }
                    2 => {
                        e.key(Key::Enter);
                        v.finish(&mut g);
                        v = EditView::begin(
                            &mut g,
                            if next() % 2 == 0 { "" } else { "a\nb> " },
                            &e,
                        );
                    }
                    3..=6 => {
                        e.key(Key::Char((b'a' + (next() % 26) as u8) as char));
                        v.redraw(&mut g, &e);
                    }
                    n => {
                        let keys = [
                            Key::Backspace,
                            Key::Delete,
                            Key::Left,
                            Key::Right,
                            Key::Home,
                            Key::End,
                        ];
                        e.key(keys[n as usize % keys.len()]);
                        v.redraw(&mut g, &e);
                    }
                }
                let (col, row) = v.caret();
                assert!(col < columns && row < rows, "{columns}x{rows}: caret {col},{row}");
                let (col, row) = g.cursor();
                assert!(col <= columns && row < rows);
            }
        }
    }
}
