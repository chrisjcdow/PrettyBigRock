// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Where things are inside a text editor: the margin with the line numbers, the text, the scroll
//! bars, and the arithmetic that turns a place in the text into pixels and back
//! (`classic/kernel/notepad.c`, `np_draw`, is the look this follows).
//!
//! All of it is in *logical pixels* (the canvas multiplies by the window's scale) and uses whole
//! numbers only, so pictures are the same on every processor. Numbers that depend on the size
//! of the text are worked out in 64 bits and brought back into 32, because a text may have
//! far more lines than a window has pixels.

use super::content::{Content, Pos, cell_width};
use crate::geometry::{Point, Size};
use crate::widget::scrollbar::{self, THICKNESS};
use alloc::string::String;

/// How far apart lines are (1.3's `LINE_H`).
pub(crate) const LINE_H: i32 = 12;
/// Room above the first line.
pub(crate) const TOP: i32 = 7;
/// Room below the last line (when scrolled to the end).
pub(crate) const BOTTOM: i32 = 5;
/// Room left of the text, and right of the longest line; and left of the line numbers.
pub(crate) const PAD: i32 = 8;
/// How wide the little extra is that shows a selected line break.
pub(crate) const BREAK_W: i32 = 4;
/// The size an editor takes when nothing says how big it should be.
pub(crate) const DEFAULT_SIZE: Size = Size::new(200, 100);
/// How many characters of room the cursor is given before the edge when it is scrolled to.
const MARGIN_CHARS: i32 = 3;
/// Digits a line number always has room for (1.3 printed them as `%3d`).
const MIN_DIGITS: usize = 3;

/// `n * per`, kept within what an `i32` can hold with room to add to it.
pub(crate) fn times(n: usize, per: i32) -> i32 {
    (n as i64).saturating_mul(i64::from(per)).min(i64::from(i32::MAX / 4)) as i32
}

/// How many decimal digits `n` has.
pub(crate) fn digits(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// The parts of an editor's rectangle, relative to its top left corner: what [`Geo::new`] works
/// out from the size, the text's size and whether there are line numbers. It is kept in the
/// widget's state by layout, so that events (which are not given the theme) can use it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Geo {
    /// The whole widget.
    pub size: Size,
    /// How wide the margin is, 0 if there are no line numbers.
    pub gutter: i32,
    /// Where the text's room starts across: the margin and the line after it.
    pub left: i32,
    /// The room for text: across and down.
    pub view_w: i32,
    pub view_h: i32,
    /// How big the text is, with its room around it.
    pub content_w: i32,
    pub content_h: i32,
    /// Which scroll bars there are.
    pub vbar: bool,
    pub hbar: bool,
    /// How wide a character is.
    pub adv: i32,
}

impl Geo {
    /// The layout of an editor `size` big showing `lines` lines of which the widest is
    /// `widest` columns.
    pub fn new(size: Size, numbers: bool, lines: usize, widest: usize, adv: i32) -> Geo {
        let adv = adv.max(1);
        let gutter = if numbers { PAD + times(digits(lines).max(MIN_DIGITS), adv) + 10 } else { 0 };
        let left = if numbers { gutter + 1 } else { 0 };
        let avail_w = (size.width - left).max(0);
        let avail_h = size.height.max(0);
        let content_w = PAD.saturating_add(times(widest, adv)).saturating_add(PAD);
        let content_h = TOP.saturating_add(times(lines, LINE_H)).saturating_add(BOTTOM);
        // A bar takes room from the other direction, which may then need a bar of its own.
        let (mut vbar, mut hbar) = (false, false);
        for _ in 0..3 {
            vbar = content_h > avail_h - if hbar { THICKNESS } else { 0 };
            hbar = content_w > avail_w - if vbar { THICKNESS } else { 0 };
        }
        Geo {
            size,
            gutter,
            left,
            view_w: (avail_w - if vbar { THICKNESS } else { 0 }).max(0),
            view_h: (avail_h - if hbar { THICKNESS } else { 0 }).max(0),
            content_w,
            content_h,
            vbar,
            hbar,
            adv,
        }
    }

    /// The largest scrolled distances: across, then down.
    pub fn max_scroll(&self) -> (i32, i32) {
        (
            scrollbar::max_offset(self.content_w, self.view_w),
            scrollbar::max_offset(self.content_h, self.view_h),
        )
    }

    /// `scroll` brought within what there is to scroll.
    pub fn clamp(&self, scroll: (i32, i32)) -> (i32, i32) {
        let (mx, my) = self.max_scroll();
        (scroll.0.clamp(0, mx), scroll.1.clamp(0, my))
    }

    /// The vertical bar's track, given where the widget's corner is.
    pub fn vtrack(&self, o: Point) -> pbr_draw::Rect {
        pbr_draw::Rect::new(o.x + self.size.width - THICKNESS, o.y, THICKNESS, self.view_h)
    }

    /// The horizontal bar's track.
    pub fn htrack(&self, o: Point) -> pbr_draw::Rect {
        pbr_draw::Rect::new(
            o.x + self.left,
            o.y + self.size.height - THICKNESS,
            self.view_w,
            THICKNESS,
        )
    }

    /// The room for text.
    pub fn text_rect(&self, o: Point) -> pbr_draw::Rect {
        pbr_draw::Rect::new(o.x + self.left, o.y, self.view_w, self.view_h)
    }

    /// The lines that show at least in part when scrolled `scroll_y`, out of `lines`.
    pub fn rows(&self, scroll_y: i32, lines: usize) -> core::ops::Range<usize> {
        let first = i64::from(scroll_y - TOP).div_euclid(i64::from(LINE_H)).max(0);
        let end =
            (i64::from(scroll_y) + i64::from(self.view_h) - i64::from(TOP) + i64::from(LINE_H) - 1)
                .div_euclid(i64::from(LINE_H))
                .clamp(0, lines as i64);
        (first.min(end) as usize)..(end as usize)
    }

    /// How many lines fit in the room for text (what Page Up and Down move by).
    pub fn page(&self) -> usize {
        (self.view_h / LINE_H).max(1) as usize
    }

    /// Where the top of line `n`'s characters is, scrolled `scroll_y`, from the widget's top.
    pub fn line_y(&self, n: usize, scroll_y: i32) -> i32 {
        TOP.saturating_add(times(n, LINE_H)).saturating_sub(scroll_y)
    }

    /// The scroll that shows the cursor at `at` in `content`, from `scroll`, going no further
    /// than it must (with a line and a few characters of room round it, where there is room).
    pub fn reveal(&self, content: &Content, at: Pos, scroll: (i32, i32)) -> (i32, i32) {
        let (mut sx, mut sy) = scroll;
        let cell_top = TOP.saturating_add(times(at.0, LINE_H));
        let room = LINE_H.min((self.view_h - LINE_H).max(0) / 2);
        if cell_top - room < sy {
            sy = cell_top - room;
        } else if cell_top + LINE_H + room > sy.saturating_add(self.view_h) {
            sy = cell_top + LINE_H + room - self.view_h;
        }
        let col = super::content::display_column(content.line(at.0), at.1);
        let x = PAD.saturating_add(times(col, self.adv));
        let room = (MARGIN_CHARS * self.adv).min((self.view_w - self.adv).max(0) / 2);
        if x - 1 - room < sx {
            sx = x - 1 - room;
        } else if x + 2 + room > sx.saturating_add(self.view_w) {
            sx = x + 2 + room - self.view_w;
        }
        self.clamp((sx, sy))
    }

    /// The place in the text under the point `p` of the window, when the widget's corner is at
    /// `o` and the text is scrolled `scroll`. Points outside the text are taken to the nearest
    /// line and column.
    pub fn position_at(&self, content: &Content, o: Point, scroll: (i32, i32), p: Point) -> Pos {
        let y = i64::from(p.y) - i64::from(o.y) + i64::from(scroll.1) - i64::from(TOP) + 1;
        let line = y.div_euclid(i64::from(LINE_H)).clamp(0, content.line_count() as i64 - 1);
        let line = line as usize;
        let x = i64::from(p.x) - i64::from(o.x) - i64::from(self.left) + i64::from(scroll.0)
            - i64::from(PAD);
        (line, column_at(content.line(line), x, i64::from(self.adv)))
    }
}

/// The character column nearest to `x` pixels from the start of `line`: the click's half of
/// a character decides whether the cursor goes before or after it.
pub(crate) fn column_at(line: &str, x: i64, adv: i64) -> usize {
    if x <= 0 {
        return 0;
    }
    let mut at = 0usize;
    for (i, c) in line.chars().enumerate() {
        let w = cell_width(c, at);
        let middle = (at as i64 * 2 + w as i64) * adv / 2;
        if x < middle {
            return i;
        }
        at += w;
    }
    line.chars().count()
}

/// The cells `first..first + cells` of `line` as text, a tab shown as spaces: what is drawn of
/// a line, so that a very long line costs only what is visible of it (and a little to get
/// there). Cell `first` is its first character.
pub(crate) fn visible_text(line: &str, first: usize, cells: usize) -> String {
    let mut out = String::new();
    let mut at = 0usize;
    for c in line.chars() {
        if at >= first + cells {
            break;
        }
        let w = cell_width(c, at);
        if at + w > first {
            if c == '\t' {
                // The part of a tab inside the range.
                let from = at.max(first);
                let to = (at + w).min(first + cells);
                out.extend(core::iter::repeat_n(' ', to - from));
            } else {
                out.push(c);
            }
        }
        at += w;
    }
    out
}
