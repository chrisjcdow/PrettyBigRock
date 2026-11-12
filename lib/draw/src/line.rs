// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Straight lines of any width (docs/design/0023-draw-window-and-sandstone.md, section 2).
//!
//! **A thin line** (one device pixel wide, which only happens at scale 1) is the classic one:
//! Bresenham's algorithm, exactly as 1.x's `gfx_line` draws it (`classic/kernel/gfx.c`), so
//! the close button's cross looks as it always did.
//!
//! **A thick line** is a *capsule*: every device pixel whose centre is within half the width
//! of the segment between the two end points. That is a rectangle with a half circle on each
//! end, so a line from a point to itself is a round dot.
//!
//! Both are worked out in whole numbers (see `shapes.rs` for doubled coordinates, which the
//! capsule uses), and **both do work in proportion to the clip, never to the line's length**:
//! a line from one end of the number range to the other costs what its visible part costs.

use crate::shapes::{cut, mix};
use crate::{Canvas, Colour, Rect};

impl Canvas<'_> {
    /// Draws a straight line from the middle of one logical pixel to the middle of another,
    /// `width` logical pixels wide, with round ends. A line from a pixel to itself is a dot.
    /// A width of zero or less draws nothing.
    ///
    /// At scale 1 and width 1 this is the classic thin line: one pixel in each column (or
    /// each row, for a steep line). Wider lines are all the pixels whose centre is within
    /// half the width of the segment, so they have no gaps and are the same thickness at every
    /// angle.
    pub fn line(&mut self, from: (i32, i32), to: (i32, i32), width: i32, colour: Colour) {
        let w = i64::from(self.device_len(width));
        if w <= 0 || colour.alpha() == 0 {
            return;
        }
        let a = (i64::from(self.device_x(from.0)), i64::from(self.device_y(from.1)));
        let b = (i64::from(self.device_x(to.0)), i64::from(self.device_y(to.1)));
        if w == 1 {
            self.thin_line(a, b, colour);
        } else {
            self.thick_line(a, b, w, colour);
        }
    }

    /// Bresenham's line between two device pixels, cut to the clip.
    ///
    /// The plain algorithm walks from one end to the other, one pixel along the longer axis at
    /// a time ("the major axis"), and decides at each step whether to move one pixel along the
    /// other too. That decision follows from a running error, and after `k` steps the number
    /// of moves along the other axis has a closed form, which is what lets this jump straight
    /// to the clip instead of walking there. With `n` the length along the major axis and `m`
    /// along the other, the walk has moved `(n/2 + k·m) / n` pixels along the other axis after
    /// `k` steps (whole-number division). The tests compare this with the plain walk on many
    /// lines, so the two cannot differ.
    fn thin_line(&mut self, a: (i64, i64), b: (i64, i64), colour: Colour) {
        let clip = self.device_clip();
        if clip.is_empty() {
            return;
        }
        let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
        // Like 1.x, a tie goes to the x axis.
        let x_major = dx >= dy;
        let (n, m) = if x_major { (dx, dy) } else { (dy, dx) };
        let (step_x, step_y) = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        // The steps `k` (0 to `n`) whose major-axis position is inside the clip: at most as
        // many as the clip is wide (or tall), however long the line is.
        let (start, step, low, high) = if x_major {
            (a.0, step_x, i64::from(clip.x), clip.right())
        } else {
            (a.1, step_y, i64::from(clip.y), clip.bottom())
        };
        let (mut first, mut last) = if step >= 0 {
            (low - start, high - 1 - start)
        } else {
            (start - high + 1, start - low)
        };
        first = first.max(0);
        last = last.min(n);
        for k in first..=last {
            let moved = if n == 0 { 0 } else { (n / 2 + k * m) / n };
            let (x, y) = if x_major {
                (a.0 + step_x * k, a.1 + step_y * moved)
            } else {
                (a.0 + step_x * moved, a.1 + step_y * k)
            };
            self.span(cut(y), cut(x), cut(x + 1), colour);
        }
        let (left, top) = (a.0.min(b.0), a.1.min(b.1));
        self.damaged(Rect::from_edges(left, top, a.0.max(b.0) + 1, a.1.max(b.1) + 1));
    }

    /// A capsule `w` (2 or more) device pixels wide round the segment between two device
    /// pixels, cut to the clip.
    ///
    /// The end points are put in doubled coordinates, at the middle of their logical pixel:
    /// `2·x + scale`. When the width is even but the scale is odd, that middle is the middle of
    /// a *device* pixel, and a line centred on a pixel's middle is an odd number of pixels
    /// thick; so the end points are moved half a pixel up and left, which puts a horizontal
    /// line's edges on pixel boundaries and makes it exactly `w` rows thick, not `w + 1` or
    /// `w - 1`.
    ///
    /// For each row in the clip that the capsule can reach, only the columns that can be in it
    /// are tested: those within `w/2` of the part of the segment that is within `w/2` rows of
    /// this one. A long diagonal line therefore costs about its own pixels, not its bounding
    /// box's.
    fn thick_line(&mut self, a: (i64, i64), b: (i64, i64), w: i64, colour: Colour) {
        let clip = self.device_clip();
        if clip.is_empty() {
            return;
        }
        let scale = i64::from(self.scale());
        let shift = i64::from(w % 2 == 0 && scale % 2 == 1);
        let (ax, ay) = (2 * a.0 + scale - shift, 2 * a.1 + scale - shift);
        let (bx, by) = (2 * b.0 + scale - shift, 2 * b.1 + scale - shift);
        let (dx, dy) = (bx - ax, by - ay);
        let length_squared = dx * dx + dy * dy;
        let (low_x, high_x) = (ax.min(bx), ax.max(bx));
        let (low_y, high_y) = (ay.min(by), ay.max(by));
        // The rows whose doubled centre, 2·py + 1, is within `w` of the segment's rows, cut
        // to the clip.
        let first = ((low_y - w).div_euclid(2)).max(i64::from(clip.y));
        let last = ((high_y + w - 1).div_euclid(2) + 1).min(clip.bottom());
        let (clip_left, clip_right) = (i64::from(clip.x), clip.right());
        // `i128` is a 128-bit whole number: the products below reach about 2¹⁰⁶.
        let reach_squared = i128::from(w) * i128::from(w);
        for py in first..last {
            let centre_y = 2 * py + 1;
            // The part of the segment within `w` (doubled) rows of this row, and the columns
            // it covers, rounded outwards so that none is missed.
            let (from_y, to_y) = (low_y.max(centre_y - w), high_y.min(centre_y + w));
            let (low, high) = if dy == 0 {
                (low_x, high_x)
            } else {
                // The segment's x at height y is ax + (y - ay)·dx/dy. Dividing by a positive
                // number keeps the rounding direction simple.
                let (sign, down) = if dy < 0 { (-1, -dy) } else { (1, dy) };
                let x_at = |y: i64, round_up: bool| {
                    let top = sign * (y - ay) * dx;
                    ax + (if round_up { top + down - 1 } else { top }).div_euclid(down)
                };
                (
                    x_at(from_y, false).min(x_at(to_y, false)),
                    x_at(from_y, true).max(x_at(to_y, true)),
                )
            };
            let from_x = ((low - w).div_euclid(2) - 1).max(clip_left);
            let to_x = ((high + w - 1).div_euclid(2) + 2).min(clip_right);
            let (row, start) = self.row_mut(py as i32, cut(from_x), cut(to_x));
            for (i, pixel) in row.iter_mut().enumerate() {
                let centre_x = 2 * (i64::from(start) + i as i64) + 1;
                let (vx, vy) = (centre_x - ax, centre_y - ay);
                let along = vx * dx + vy * dy;
                let inside = if length_squared == 0 || along <= 0 {
                    i128::from(vx * vx + vy * vy) <= reach_squared
                } else if along >= length_squared {
                    let (ex, ey) = (centre_x - bx, centre_y - by);
                    i128::from(ex * ex + ey * ey) <= reach_squared
                } else {
                    let across = i128::from(vx * dy - vy * dx);
                    across * across <= reach_squared * i128::from(length_squared)
                };
                if inside {
                    mix(colour, pixel);
                }
            }
        }
        self.damaged(Rect::from_edges(
            (low_x - w).div_euclid(2) - 1,
            (low_y - w).div_euclid(2) - 1,
            (high_x + w - 1).div_euclid(2) + 2,
            (high_y + w - 1).div_euclid(2) + 2,
        ));
    }
}
