// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Text: UTF-8 strings drawn in the system's bitmap font
//! (docs/design/0023-draw-window-and-sandstone.md, sections 2 and 3).
//!
//! A *glyph* is the picture of one character, and a *bitmap font* stores each glyph as a small
//! grid of dots (here 5 across and 11 down, one byte a row), so that drawing text is only
//! filling in the dots that are set. The font itself, and what its rows mean, is in
//! `lib/font`. A dot is drawn as a square of `size` x `size` logical pixels, so text can be
//! made bigger by whole numbers (2 for headings) and stays sharp. No two characters differ
//! in width: every `char` of the string takes one cell, the same width, which is what
//! makes measuring a string a multiplication. There is no kerning (moving letters closer),
//! no combining of accents (the font has ready-made accented letters), and no line breaking:
//! a new line is the caller's job, and `'\n'` is a character the font lacks, like any other.
//!
//! **A character the font lacks** is drawn as a box, so that missing letters are seen rather
//! than silently left out.
//!
//! **Why the work is bounded.** A string may be a million characters long and the canvas
//! 64 pixels wide. Characters wholly left of the clip are skipped without being looked at
//! (their number is worked out by arithmetic, the cells being all one width), drawing stops
//! at the first character right of the clip, and a character's rows and dots are only turned
//! into pixels inside the clip. So the cost is one cheap step for each character of the string
//! (counting them, to say how wide the text is) plus work in proportion to what is visible.

use crate::{Canvas, Colour, Rect};
use pbr_font::{ABOVE, Glyph, HEIGHT, WIDTH};

/// The biggest size: a dot is this many logical pixels, each way.
const MAX_SIZE: u32 = 16;

/// The glyph drawn for a character the font lacks: the outline of a rectangle as wide as the
/// font, from the row `ABOVE` (where the capital letters start) down seven rows (to where
/// they end, before the descenders).
const BOX: Glyph = box_glyph();

/// Makes [`BOX`]. A `const fn` can be run by the compiler, so that the box is part of the
/// program and costs nothing to make. (`while` rather than `for`, which a `const fn` can't use.)
const fn box_glyph() -> Glyph {
    let full = (1u8 << WIDTH) - 1;
    let sides = (1u8 << (WIDTH - 1)) | 1;
    let (top, bottom) = (ABOVE, ABOVE + 6);
    let mut glyph = [0u8; HEIGHT];
    let mut row = top;
    while row <= bottom {
        glyph[row] = if row == top || row == bottom { full } else { sides };
        row += 1;
    }
    glyph
}

/// How text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    /// How many logical pixels each dot of the font is, each way: 1 for ordinary text, 2 for
    /// headings. (Brought within 1 to 16 when used, so that any number may be given.)
    pub size: u32,
    /// Bold: every dot is drawn twice, the second one dot to the right, as 1.x's bold was.
    pub bold: bool,
}

impl TextStyle {
    /// Ordinary text.
    pub const PLAIN: TextStyle = TextStyle { size: 1, bold: false };
    /// Bold text.
    pub const BOLD: TextStyle = TextStyle { size: 1, bold: true };

    /// This style at another size.
    pub const fn sized(self, size: u32) -> TextStyle {
        TextStyle { size, bold: self.bold }
    }

    /// The size, brought within 1 to 16. (`u32::clamp` can't be used in a `const fn`.)
    const fn dot(self) -> i32 {
        if self.size < 1 {
            1
        } else if self.size > MAX_SIZE {
            MAX_SIZE as i32
        } else {
            self.size as i32
        }
    }

    /// Logical pixels from one character to the next: 5 dots and one of space between (and
    /// one more if bold), times the size.
    pub const fn advance(self) -> i32 {
        (WIDTH as i32 + 1 + self.bold as i32) * self.dot()
    }

    /// Logical pixels from one line of text to the next: the font's height in dots, times the
    /// size.
    pub const fn line_height(self) -> i32 {
        HEIGHT as i32 * self.dot()
    }

    /// How wide `text` is when drawn: its number of characters (not bytes) times
    /// [`TextStyle::advance`], stopping at `i32::MAX`.
    pub fn width(self, text: &str) -> i32 {
        // 64-bit, so that the product of a long string and the advance can't wrap round.
        let characters = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        characters.saturating_mul(i64::from(self.advance())).min(i64::from(i32::MAX)) as i32
    }
}

impl Canvas<'_> {
    /// Draws `text` with the top left corner of its first character's cell at (`x`, `y`), and
    /// answers the x just past its last character, so that more text can follow it.
    ///
    /// Only what is inside the clip is drawn. A see-through colour is mixed once for each
    /// pixel, even where a bold dot overlaps its twin.
    pub fn text(&mut self, x: i32, y: i32, text: &str, colour: Colour, style: TextStyle) -> i32 {
        // The answer doesn't depend on what is visible.
        let end = x.saturating_add(style.width(text));
        let clip = self.clip_bounds();
        if text.is_empty() || colour.alpha() == 0 || clip.is_empty() {
            return end;
        }
        let (size, advance) = (style.dot(), i64::from(style.advance()));
        let height = i64::from(style.line_height());
        if i64::from(y) >= clip.bottom() || i64::from(y) + height <= i64::from(clip.y) {
            return end;
        }
        // Every cell is `advance` wide, so the first cell that reaches into the clip is found
        // by arithmetic, and `skip` steps over the characters before it (a fast scan that
        // doesn't draw anything). `div_euclid` rounds down for negative numbers too.
        let first = (i64::from(clip.x) - i64::from(x)).div_euclid(advance).max(0);
        let first_x = i64::from(x) + first * advance;
        let mut cell_x = first_x;
        // `skip` takes a `usize`; one too big to hold is past the end of any string anyway.
        let skipped = usize::try_from(first).unwrap_or(usize::MAX);
        for c in text.chars().skip(skipped) {
            if cell_x >= clip.right() {
                break;
            }
            // `lookup` gives an `Option`: `Some(glyph)`, or `None` if the font lacks `c`.
            // `unwrap_or` takes the glyph, or else the box.
            let glyph = pbr_font::lookup(c).unwrap_or(&BOX);
            // The cell starts before the clip's right edge and ends after its left one, and
            // the clip is small, so these fit in an `i32`.
            self.draw_glyph(glyph, cell_x as i32, y, size, style.bold, colour);
            cell_x += advance;
        }
        // One rectangle for all the cells visited (the clip cuts it).
        if cell_x > first_x {
            let cells = Rect::from_edges(first_x, i64::from(y), cell_x, i64::from(y) + height);
            self.damaged(self.to_device(cells));
        }
        end
    }

    /// Draws one glyph with the top left corner of its cell at logical (`cx`, `cy`). Does
    /// **not** record damage.
    fn draw_glyph(
        &mut self,
        glyph: &Glyph,
        cx: i32,
        cy: i32,
        size: i32,
        bold: bool,
        colour: Colour,
    ) {
        let clip = self.device_clip();
        let columns = if bold { WIDTH + 1 } else { WIDTH };
        for (row, &dots) in glyph.iter().enumerate() {
            if dots == 0 {
                continue;
            }
            // Bold: the dots again, one to the right, as one number of `columns` bits, so
            // that where the copies overlap the dot is there once, and is drawn once.
            let dots = u32::from(dots);
            let dots = if bold { dots << 1 | dots } else { dots };
            let top_edge = cy.saturating_add(row as i32 * size);
            // The device rows this row of dots covers, cut to the clip: only those are drawn.
            let top = self.device_y(top_edge).max(clip.y);
            let bottom = self.device_y(top_edge.saturating_add(size)).min(clip.bottom() as i32);
            if top >= bottom {
                continue;
            }
            // Runs of dots next to each other are drawn as one span in each device row.
            let set = |column: usize| dots >> (columns - 1 - column) & 1 == 1;
            let mut column = 0;
            while column < columns {
                if !set(column) {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < columns && set(column) {
                    column += 1;
                }
                let left = self.device_x(cx.saturating_add(start as i32 * size));
                let right = self.device_x(cx.saturating_add(column as i32 * size));
                for y in top..bottom {
                    self.span(y, left, right, colour);
                }
            }
        }
    }
}
