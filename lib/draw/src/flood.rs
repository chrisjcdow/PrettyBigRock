// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Flood fill: a paint program's bucket
//! (docs/design/0023-draw-window-and-sandstone.md, section 2).
//!
//! **The rule.** The seed is a device pixel. Every pixel inside the clip that has the seed's
//! colour and can be reached from the seed through pixels of that colour, stepping up, down,
//! left or right (not diagonally), is set to the new colour. The spare byte of a stored pixel
//! is ignored when colours are compared.
//!
//! **The method** is a scanline fill: it fills a whole run of a row at once, then looks along
//! the rows above and below for runs to fill next. It keeps those still to do in a `Vec`
//! (a growable array) used as a stack, not in recursion: a recursive fill on a big picture
//! would use up the program's stack, which is small and fixed. The stack holds a few entries
//! per run, never one per pixel.
//!
//! The work is bounded by the clip: a pixel outside it is never read, written or reached.

use crate::{Canvas, Colour, Rect};
use alloc::vec;

/// The part of a stored pixel that is its colour; the top byte is spare.
const COLOUR_BITS: u32 = 0x00FF_FFFF;

impl Canvas<'_> {
    /// Fills the area of one colour around a point with another colour, as a paint program's
    /// bucket does. The point is at the position (`x`, `y`); if that is outside the clip,
    /// nothing happens. The area is every pixel inside the clip of the same colour that can be
    /// reached by steps up, down, left and right through pixels of that colour.
    ///
    /// `colour` is mixed with the old colour once, so a see-through fill gives the whole area
    /// one even tone. If the result would be the colour that is there already (which includes a
    /// clear colour), nothing happens.
    pub fn flood_fill(&mut self, x: i32, y: i32, colour: Colour) {
        let (seed_x, seed_y) = (self.device_x(x), self.device_y(y));
        let Some(seed) = self.device_pixel(seed_x, seed_y) else {
            return;
        };
        let old = seed & COLOUR_BITS;
        let new = colour.over(old);
        if new & COLOUR_BITS == old {
            return;
        }
        let clip = self.device_clip();
        let (clip_left, clip_right) = (clip.x, clip.right() as i32);
        // The pixels changed lie in this box, which is the damage.
        let (mut left, mut top, mut right, mut bottom) = (seed_x, seed_y, seed_x, seed_y);
        let mut todo = vec![(seed_x, seed_y)];
        // `while let` repeats as long as `pop` gives something.
        while let Some((x, y)) = todo.pop() {
            let (row, start) = self.row_mut(y, clip_left, clip_right);
            let at = (x - start) as usize;
            if row[at] & COLOUR_BITS != old {
                // Filled already, by a run that was found in the meantime.
                continue;
            }
            let (mut from, mut to) = (at, at);
            while from > 0 && row[from - 1] & COLOUR_BITS == old {
                from -= 1;
            }
            while to + 1 < row.len() && row[to + 1] & COLOUR_BITS == old {
                to += 1;
            }
            row[from..=to].fill(new);
            let (first, last) = (start + from as i32, start + to as i32);
            (left, right) = (left.min(first), right.max(last));
            (top, bottom) = (top.min(y), bottom.max(y));
            // Look at the row above and the row below, along the run just filled. Each stretch
            // of old colour found starts a new run to fill; the fill then goes on to its ends,
            // which may stick out past this run on either side.
            for next in [y - 1, y + 1] {
                if next < clip.y || i64::from(next) >= clip.bottom() {
                    continue;
                }
                let (above, _) = self.row_mut(next, clip_left, clip_right);
                let mut inside = false;
                for (i, &pixel) in above.iter().enumerate().take(to + 1).skip(from) {
                    let matches = pixel & COLOUR_BITS == old;
                    if matches && !inside {
                        todo.push((start + i as i32, next));
                    }
                    inside = matches;
                }
            }
        }
        self.damaged(Rect::from_edges(
            left.into(),
            top.into(),
            i64::from(right) + 1,
            i64::from(bottom) + 1,
        ));
    }
}
