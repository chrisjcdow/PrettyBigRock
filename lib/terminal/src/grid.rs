// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The text grid: the characters on the screen, a cursor, and a record of what changed.
//!
//! The console service draws this grid on the framebuffer. Keeping the grid separate from the
//! drawing has two uses: all the rules about wrapping, scrolling and tabs can be tested on the
//! host, and the console can redraw only the rows that changed instead of the whole screen.
//!
//! Nothing here panics, whatever text it is given. A [`Vec`] is Rust's growable array; every
//! position is checked against the grid's size before it is used.

use alloc::vec;
use alloc::vec::Vec;

/// The most cells a grid may have (about 4 million, a screen far bigger than any we have), so
/// that a corrupt screen size cannot ask for all of memory.
pub const MAX_CELLS: usize = 1 << 22;

/// Tab stops are every this many columns.
pub const TAB_STOP: usize = 8;

/// The character that is drawn for `c`: itself if it is printable ASCII (space to `~`), `?`
/// for anything else, because the font has no other glyphs (note 0018, section 5).
pub fn printable(c: char) -> char {
    if (' '..='~').contains(&c) { c } else { '?' }
}

/// What changed in a [`Grid`] since the last [`Grid::take_changes`].
///
/// A console that keeps the previous picture on the screen can bring it up to date by first
/// moving its pixels up by [`Changes::scrolled`] rows, if that is not 0, and then redrawing
/// the rows in [`Changes::dirty_rows`]. (The dirty rows are counted after the scroll, as the
/// grid now is.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changes {
    /// How many rows the grid scrolled up by (never more than the grid's rows). The rows it
    /// scrolled in at the bottom are among the dirty ones.
    pub scrolled: usize,
    dirty: Vec<bool>,
}

impl Changes {
    /// The numbers of the rows to redraw, from the top. (`impl Iterator` means "something that
    /// can be looped over", without saying exactly what.)
    pub fn dirty_rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty.iter().enumerate().filter(|(_, d)| **d).map(|(row, _)| row)
    }

    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.scrolled == 0 && !self.dirty.contains(&true)
    }
}

/// A screen of text: `columns` by `rows` characters, and where the next character goes.
#[derive(Clone, Debug)]
pub struct Grid {
    columns: usize,
    rows: usize,
    cells: Vec<char>,
    /// Where the next character goes. `col` can be `columns`: the row is full, and the next
    /// character starts a new row. (Waiting to wrap until a character arrives, instead of
    /// wrapping at once, means a line of exactly `columns` characters followed by a newline
    /// does not leave a blank row.)
    col: usize,
    row: usize,
    dirty: Vec<bool>,
    scrolled: usize,
}

impl Grid {
    /// A blank grid, with the cursor at the top left. `None` if it would have no cells (a
    /// screen with no room for text) or too many ([`MAX_CELLS`]).
    pub fn new(columns: usize, rows: usize) -> Option<Grid> {
        let cells = columns.checked_mul(rows)?;
        if cells == 0 || cells > MAX_CELLS {
            return None;
        }
        Some(Grid {
            columns,
            rows,
            cells: vec![' '; cells],
            col: 0,
            row: 0,
            // Everything is new to a console that has not drawn it yet.
            dirty: vec![true; rows],
            scrolled: 0,
        })
    }

    /// Characters across.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Rows down.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Where the next character goes: (column, row), counted from 0 at the top left. The
    /// column is `columns()` when the row is full (see the field's note).
    pub fn cursor(&self) -> (usize, usize) {
        (self.col, self.row)
    }

    /// Moves the cursor. The row is held to the last row, and the column to `columns()`.
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        self.col = col.min(self.columns);
        self.row = row.min(self.rows - 1);
    }

    /// The character at (`col`, `row`), or a space outside the grid.
    pub fn cell(&self, col: usize, row: usize) -> char {
        if col < self.columns && row < self.rows {
            self.cells[row * self.columns + col]
        } else {
            ' '
        }
    }

    /// The characters of one row (empty for a row outside the grid).
    pub fn row(&self, row: usize) -> &[char] {
        if row >= self.rows {
            return &[];
        }
        &self.cells[row * self.columns..(row + 1) * self.columns]
    }

    /// Puts `c` (drawn as [`printable`] says) at (`col`, `row`) without moving the cursor.
    /// Does nothing outside the grid. The row is only marked changed if the cell was.
    pub fn put_at(&mut self, col: usize, row: usize, c: char) {
        if col >= self.columns || row >= self.rows {
            return;
        }
        let c = printable(c);
        let cell = &mut self.cells[row * self.columns + col];
        if *cell != c {
            *cell = c;
            self.dirty[row] = true;
        }
    }

    /// If the cursor is at the end of a full row, moves it to the start of the next one
    /// (scrolling if that is past the bottom), as writing another character would.
    pub fn wrap_pending(&mut self) {
        if self.col >= self.columns {
            self.newline();
        }
    }

    /// Moves the cursor to the start of the next row, scrolling up one row if it was on the
    /// last.
    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.scroll_up(1);
        }
    }

    /// Writes `text` at the cursor.
    /// - `\n` starts a new row; `\r` goes back to the start of the row.
    /// - `\t` moves to the next multiple of [`TAB_STOP`] (or the end of the row), filling the
    ///   cells it passes with spaces. In a full row it does nothing.
    /// - A character that would go past the right edge starts a new row first; at the bottom
    ///   the top row goes (the grid scrolls up).
    /// - Anything that is not printable ASCII is written as `?`.
    pub fn write_str(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => self.col = 0,
                '\t' => {
                    let stop = ((self.col / TAB_STOP + 1) * TAB_STOP).min(self.columns);
                    while self.col < stop {
                        self.put_at(self.col, self.row, ' ');
                        self.col += 1;
                    }
                }
                c => {
                    self.wrap_pending();
                    self.put_at(self.col, self.row, c);
                    self.col += 1;
                }
            }
        }
    }

    /// Moves every row up by `n` (at most all of them), dropping the top ones and adding blank
    /// rows at the bottom. The cursor does not move. A console that keeps the screen's pixels
    /// can move them too, instead of redrawing (see [`Changes`]).
    pub fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.rows);
        if n == 0 {
            return;
        }
        let keep = (self.rows - n) * self.columns;
        self.cells.copy_within(n * self.columns.., 0);
        self.cells[keep..].fill(' ');
        // A row that was waiting to be redrawn still is, in its new place.
        self.dirty.copy_within(n.., 0);
        self.dirty[self.rows - n..].fill(true);
        self.scrolled = (self.scrolled + n).min(self.rows);
    }

    /// Empties the grid and puts the cursor at the top left.
    pub fn clear(&mut self) {
        self.cells.fill(' ');
        self.col = 0;
        self.row = 0;
        self.dirty.fill(true);
        // Whatever was scrolled is gone anyway: everything is dirty.
        self.scrolled = 0;
    }

    /// Marks every row as changed, for a console that has to draw it all (on a new screen).
    pub fn mark_all_dirty(&mut self) {
        self.dirty.fill(true);
        self.scrolled = 0;
    }

    /// What changed since the last call (or since the grid was made), and forgets it.
    pub fn take_changes(&mut self) -> Changes {
        let dirty = core::mem::replace(&mut self.dirty, vec![false; self.rows]);
        Changes { scrolled: core::mem::take(&mut self.scrolled), dirty }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    /// The grid as text: one string per row, trailing spaces left as they are.
    fn rows(g: &Grid) -> Vec<String> {
        (0..g.rows()).map(|r| g.row(r).iter().collect()).collect()
    }

    fn grid(columns: usize, rows: usize) -> Grid {
        Grid::new(columns, rows).unwrap()
    }

    /// A small deterministic random number generator (xorshift64*).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
    }

    #[test]
    fn an_empty_grid_is_refused() {
        assert!(Grid::new(0, 0).is_none());
        assert!(Grid::new(0, 5).is_none());
        assert!(Grid::new(5, 0).is_none());
        assert!(Grid::new(usize::MAX, 2).is_none());
        assert!(Grid::new(MAX_CELLS + 1, 1).is_none());
        assert!(Grid::new(MAX_CELLS, 1).is_some());
        assert!(Grid::new(1, 1).is_some());
    }

    #[test]
    fn text_goes_where_the_cursor_is() {
        let mut g = grid(6, 3);
        g.write_str("ab\ncd");
        assert_eq!(rows(&g), ["ab    ", "cd    ", "      "]);
        assert_eq!(g.cursor(), (2, 1));
        g.write_str("\r!");
        assert_eq!(g.row(1).iter().collect::<String>(), "!d    ");
        assert_eq!(g.cell(0, 1), '!');
        assert_eq!(g.cell(99, 99), ' ');
    }

    #[test]
    fn long_lines_wrap_and_a_full_row_waits_for_the_next_character() {
        let mut g = grid(4, 3);
        g.write_str("abcd");
        assert_eq!(g.cursor(), (4, 0), "the row is full but the cursor waits");
        g.write_str("\n");
        assert_eq!(g.cursor(), (0, 1), "so a newline right after does not leave a blank row");
        g.write_str("efghi");
        assert_eq!(rows(&g), ["abcd", "efgh", "i   "]);
        assert_eq!(g.cursor(), (1, 2));
    }

    #[test]
    fn the_top_row_goes_at_the_bottom() {
        let mut g = grid(3, 2);
        g.write_str("aaa\nbbb\nccc");
        assert_eq!(rows(&g), ["bbb", "ccc"]);
        assert_eq!(g.cursor(), (3, 1));
        g.write_str("d");
        assert_eq!(rows(&g), ["ccc", "d  "]);
        assert_eq!(g.cursor(), (1, 1));
    }

    #[test]
    fn tabs_go_to_the_next_multiple_of_eight() {
        let mut g = grid(20, 2);
        g.write_str("a\tb\tc");
        assert_eq!(g.row(0).iter().collect::<String>(), "a       b       c   ");
        // At a tab stop, a tab goes to the next one.
        g.write_str("\n12345678\tx");
        assert_eq!(g.row(1).iter().collect::<String>(), "12345678        x   ");
        // Near the edge a tab stops at the edge, and in a full row it does nothing.
        let mut g = grid(10, 2);
        g.write_str("123456789\t");
        assert_eq!(g.cursor(), (10, 0));
        g.write_str("\t");
        assert_eq!(g.cursor(), (10, 0));
        g.write_str("z");
        assert_eq!(g.cell(0, 1), 'z');
        // It overwrites with spaces, not leaving old text behind.
        let mut g = grid(10, 1);
        g.write_str("abcdefghij\r\t");
        assert_eq!(g.row(0).iter().collect::<String>(), "        ij");
    }

    #[test]
    fn carriage_returns_overwrite() {
        let mut g = grid(5, 2);
        g.write_str("hello\rjy");
        assert_eq!(rows(&g), ["jyllo", "     "]);
        g.write_str("\r\r\r");
        assert_eq!(g.cursor(), (0, 0));
    }

    #[test]
    fn anything_else_is_a_question_mark() {
        let mut g = grid(12, 1);
        g.write_str("caf\u{e9} \u{1F600}\x08\x7f\0~");
        assert_eq!(g.row(0).iter().collect::<String>(), "caf? ????~  ");
        assert_eq!(printable('a'), 'a');
        assert_eq!(printable('\u{e9}'), '?');
        assert_eq!(printable('\n'), '?');
    }

    #[test]
    fn a_one_by_one_grid_works() {
        let mut g = grid(1, 1);
        g.write_str("abc\n\n\nd");
        assert_eq!(g.row(0), ['d']);
        g.write_str("\te\r\t");
        assert_eq!(g.row(0), [' ']);
        g.write_str("\u{e9}");
        assert_eq!(g.cursor(), (1, 0));
        g.scroll_up(5);
        g.clear();
        assert_eq!(g.cursor(), (0, 0));
        g.set_cursor(9, 9);
        assert_eq!(g.cursor(), (1, 0));
    }

    #[test]
    fn only_newlines() {
        let mut g = grid(3, 4);
        g.write_str(&"\n".repeat(1000));
        assert_eq!(rows(&g), ["   "; 4]);
        assert_eq!(g.cursor(), (0, 3));
        let mut g = grid(80, 25);
        g.write_str("x");
        g.write_str(&"\n".repeat(25));
        assert_eq!(g.cell(0, 0), ' ');
    }

    #[test]
    fn scrolling_by_hand() {
        let mut g = grid(2, 4);
        g.write_str("aabbccdd");
        g.scroll_up(1);
        assert_eq!(rows(&g), ["bb", "cc", "dd", "  "]);
        g.scroll_up(0);
        assert_eq!(rows(&g), ["bb", "cc", "dd", "  "]);
        g.scroll_up(3);
        assert_eq!(rows(&g), ["  "; 4]);
        g.write_str("zz");
        g.scroll_up(usize::MAX);
        assert_eq!(rows(&g), ["  "; 4]);
    }

    #[test]
    fn it_remembers_which_rows_changed() {
        let mut g = grid(4, 4);
        // A new grid is all new.
        let c = g.take_changes();
        assert_eq!(c.dirty_rows().collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert!(g.take_changes().is_empty());
        g.write_str("ab\n\ncd");
        let c = g.take_changes();
        assert_eq!(c.scrolled, 0);
        assert_eq!(c.dirty_rows().collect::<Vec<_>>(), [0, 2]);
        // Writing what is there already changes nothing.
        g.set_cursor(0, 0);
        g.write_str("ab");
        g.put_at(0, 2, 'c');
        assert!(g.take_changes().is_empty());
        // Moving the cursor and writing nothing changes nothing either.
        g.set_cursor(3, 3);
        g.write_str("");
        assert!(g.take_changes().is_empty());
        g.mark_all_dirty();
        assert_eq!(g.take_changes().dirty_rows().count(), 4);
        g.clear();
        let c = g.take_changes();
        assert_eq!((c.scrolled, c.dirty_rows().count()), (0, 4));
    }

    #[test]
    fn scrolling_is_recorded_and_moves_pending_changes_up() {
        let mut g = grid(2, 4);
        g.write_str("aabbcc");
        g.take_changes();
        g.set_cursor(0, 1);
        g.write_str("B"); // row 1 changes...
        g.scroll_up(1); // ...then moves to row 0
        let c = g.take_changes();
        assert_eq!(c.scrolled, 1);
        assert_eq!(c.dirty_rows().collect::<Vec<_>>(), [0, 3]);
        g.scroll_up(2);
        g.scroll_up(3);
        let c = g.take_changes();
        assert_eq!(c.scrolled, 4, "never more than the grid has");
        assert_eq!(c.dirty_rows().count(), 4);
    }

    /// The console's way of keeping the screen up to date, run against a random mess of text
    /// and tabs and scrolling: the picture it keeps is always the grid.
    #[test]
    fn moving_the_old_picture_and_redrawing_the_dirty_rows_is_enough() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let mut g = grid(7, 5);
        let mut screen: Vec<Vec<char>> = (0..5).map(|_| vec![' '; 7]).collect();
        let alphabet: Vec<char> = "abc \n\t\r\u{e9}".chars().collect();
        for _ in 0..2000 {
            let n = rng.next() % 12;
            let text: String =
                (0..n).map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize]).collect();
            g.write_str(&text);
            match rng.next() % 9 {
                0 => g.scroll_up((rng.next() % 7) as usize),
                1 => g.clear(),
                2 => g.put_at((rng.next() % 9) as usize, (rng.next() % 7) as usize, 'Q'),
                _ => {}
            }
            if rng.next().is_multiple_of(3) {
                let changes = g.take_changes();
                screen.rotate_left(changes.scrolled);
                for row in changes.dirty_rows() {
                    screen[row] = g.row(row).to_vec();
                }
                for (r, drawn) in screen.iter().enumerate() {
                    assert_eq!(drawn.as_slice(), g.row(r), "row {r}");
                }
            }
        }
    }

    /// Whatever it is given, nothing panics and the cursor stays in the grid.
    #[test]
    fn random_text_and_moves_never_panic() {
        let mut rng = Rng(42);
        for (columns, rows) in [(1, 1), (1, 9), (9, 1), (3, 3), (80, 25)] {
            let mut g = grid(columns, rows);
            for _ in 0..3000 {
                match rng.next() % 6 {
                    0 => g.set_cursor(rng.next() as usize, rng.next() as usize),
                    1 => g.scroll_up(rng.next() as usize % 40),
                    2 => g.put_at(rng.next() as usize % 100, rng.next() as usize % 100, '\n'),
                    3 => {
                        g.take_changes();
                    }
                    _ => {
                        let text: String = (0..rng.next() % 20)
                            .map(|_| char::from_u32((rng.next() % 0x300) as u32).unwrap_or('x'))
                            .collect();
                        g.write_str(&text);
                    }
                }
                let (col, row) = g.cursor();
                assert!(col <= columns && row < rows);
            }
        }
    }
}
