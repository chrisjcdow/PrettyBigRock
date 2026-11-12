// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Rounded rectangles, ellipses, gradients and shadows
//! (docs/design/0023-draw-window-and-sandstone.md, section 2).
//!
//! **The rule for every shape here: a device pixel is in the shape if its centre is.** There is
//! no anti-aliasing (no half-covered pixels shown as in-between colours): a pixel is in or it
//! is out. That keeps the look crisp like the bitmap font, makes pictures comparable exactly,
//! and gives flood fill clean edges.
//!
//! **Doubled coordinates.** Device pixel (`px`, `py`) has its centre at (`px` + ½, `py` + ½).
//! To stay in whole numbers, the formulas work in *doubled* coordinates, where that centre is
//! (2·`px` + 1, 2·`py` + 1) and a rectangle's edge `x0` is 2·`x0`. Distances come out doubled
//! too, and every comparison is between whole numbers.
//!
//! **Shapes are worked out in device pixels**, after the scale is applied, so a circle at scale
//! 2 is a true circle of twice the radius and not a small one with doubled pixels.
//!
//! **The work is bounded by the clip, never by the shape.** Arguments can be anything (a shape
//! a million pixels wide, a radius in the millions). So every loop here runs only over the rows
//! inside the clip, and for each row works out *directly* (with an integer square root) which
//! columns are in the shape, instead of testing columns one by one or walking round the
//! outline. A huge shape drawn on a small canvas costs what a small shape costs.
//!
//! Each call mixes every pixel of its shape exactly once (so a see-through shape has one even
//! tone, even where its parts meet) and records its damage once.

use crate::{Canvas, Colour, Rect};
use alloc::vec::Vec;

/// The most a shadow's blur is allowed to be, in device pixels. It bounds the table of
/// fade-out values made for each call.
const MAX_BLUR: i64 = 1024;

/// The whole number square root of `n`, rounded down: the biggest `r` with `r * r <= n`. Exact
/// for every `n`, and uses no floating point (note 0023: the same pixels on every processor).
///
/// This is the classic digit-by-digit method: it builds the root one binary digit at a time,
/// from the highest, by asking whether the next digit fits. It takes at most 32 rounds.
pub(crate) fn isqrt(n: u64) -> u64 {
    // `bit` is the highest power of four that is at most `n`; `root` is what has been found.
    let (mut rest, mut root, mut bit) = (n, 0u64, 1u64 << 62);
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if rest >= root + bit {
            rest -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// The square root of `n` rounded *up*: the smallest `r` with `r * r >= n`.
fn isqrt_up(n: u64) -> u64 {
    let root = isqrt(n);
    if root * root < n { root + 1 } else { root }
}

/// An `i64` as an `i32`, stopping at the ends of the `i32` range. Every position worked out
/// here is already far inside it; this makes that obvious to the compiler and to the reader.
pub(crate) fn cut(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Draws `colour` over one stored pixel, as [`Canvas::span`] does for a run: a clear colour
/// leaves the pixel alone (even its spare byte), a solid one replaces it.
#[inline]
pub(crate) fn mix(colour: Colour, pixel: &mut u32) {
    if colour.alpha() != 0 {
        *pixel = colour.over(*pixel);
    }
}

/// A rectangle's edges as (left, top, right, bottom), in 64 bits so the sums below cannot
/// overflow.
fn edges(r: &Rect) -> (i64, i64, i64, i64) {
    (i64::from(r.x), i64::from(r.y), r.right(), r.bottom())
}

/// How far in, from the left and from the right, the rounded corners pull the rectangle's edge
/// on row `py`: the number of pixels of that row, at each end, that are not in the shape.
///
/// `r` is the corner radius (0 to half the shorter side). A pixel in a corner square is in the
/// shape when its centre is no farther than `r` from the corner circle's centre, which in
/// doubled coordinates is `dx² + dy² <= (2r)²`. For this row `dy` is fixed, so `dx` must be at
/// most the square root of what is left, and `dx` is odd in the left half (twice a whole
/// number, minus one), which gives the column directly.
fn corner_inset(py: i64, y0: i64, y1: i64, r: i64) -> i64 {
    if r == 0 {
        return 0;
    }
    // Twice the distance from this row's centre up or down to the corner circles' centres'
    // row, or 0 for rows between the two corners. Top and bottom are mirror images: the
    // distance below the bottom circles' centre row is worked out the same way.
    let centre = 2 * py + 1;
    let dy = (2 * (y0 + r) - centre).max(centre - 2 * (y1 - r)).max(0);
    if dy == 0 {
        return 0;
    }
    let left = (4 * r * r - dy * dy).max(0);
    // The farthest odd `dx` that fits is `isqrt(left)`; it covers `(that + 1) / 2` pixels.
    r - (isqrt(left as u64) as i64 + 1) / 2
}

/// The columns of the ellipse that just fits in `r` (a device rectangle), on row `py`: the
/// first, and the one past the last. Empty (`from >= to`) if the row misses the ellipse.
///
/// The rule is `dx²·h² + dy²·w² <= w²·h²` in doubled coordinates, with `w` and `h` the ellipse's
/// size. For this row `dy` is fixed, so `dx²` must be at most `w²·(h² - dy²) / h²`, rounded
/// down (`dx` is a whole number, so that loses nothing), and `dx` itself at most that
/// number's square root. The products are up to 2¹⁰⁰, which is why this uses `i128`, Rust's
/// 128-bit whole number.
fn ellipse_span(r: &Rect, py: i64) -> (i64, i64) {
    let (x0, y0, w, h) = (i64::from(r.x), i64::from(r.y), i64::from(r.width), i64::from(r.height));
    let dy = 2 * py + 1 - (2 * y0 + h);
    let room = i128::from(h * h - dy * dy).max(0);
    let dx_squared = i128::from(w) * i128::from(w) * room / i128::from(h * h);
    // At most w², so it fits in 64 bits.
    let reach = isqrt(dx_squared as u64) as i64;
    let centre = 2 * x0 + w;
    // The columns whose doubled centre, 2·px + 1, is within `reach` of the ellipse's centre.
    // `div_euclid` divides rounding down, for negative numbers too.
    ((centre - reach).div_euclid(2), (centre + reach + 1).div_euclid(2))
}

impl Canvas<'_> {
    // `impl Canvas<'_>` adds methods to `Canvas`; the `'_` says "whatever lifetime the canvas
    // has", since these methods do not care.

    /// Fills a rectangle with rounded corners. `radius` is cut to half the shorter side; a
    /// radius of zero or less gives square corners, as [`Canvas::fill_rect`].
    ///
    /// A pixel is left out only if it is in one of the four corner squares and its centre is
    /// farther than the radius from the corner circle's centre, so the shape is the same in
    /// all four corners, mirrored, and the same left to right and top to bottom.
    pub fn fill_round_rect(&mut self, rect: Rect, radius: i32, colour: Colour) {
        let outer = self.to_device(rect);
        let r = self.corner_radius(&outer, radius);
        self.fill_round_device(outer, r, colour);
    }

    /// Draws the outline of a rounded rectangle: a line `thickness` wide just inside its edge.
    /// A thickness of zero or less draws nothing; one that leaves no hole fills the shape.
    ///
    /// The outline is the pixels of [`Canvas::fill_round_rect`] that are not in the smaller
    /// rounded rectangle inside it, whose corners have the same centres, so the line has the
    /// same thickness all the way round.
    pub fn stroke_round_rect(&mut self, rect: Rect, radius: i32, thickness: i32, colour: Colour) {
        let outer = self.to_device(rect);
        let r = self.corner_radius(&outer, radius);
        let t = i64::from(self.device_len(thickness));
        self.stroke_round_device(outer, r, t, colour);
    }

    /// Fills the ellipse that just fits inside `rect` (a circle, if `rect` is square).
    ///
    /// For a square of even side this is exactly the rounded rectangle whose radius is half
    /// the side.
    pub fn fill_ellipse(&mut self, rect: Rect, colour: Colour) {
        let outer = self.to_device(rect);
        let Some((first, last)) = self.visible_rows(&outer, colour) else {
            return;
        };
        for py in first..last {
            let (from, to) = ellipse_span(&outer, py);
            self.span(py as i32, cut(from), cut(to), colour);
        }
        self.damaged(outer);
    }

    /// Draws the outline of the ellipse that just fits inside `rect`, `thickness` wide, inside
    /// it. A thickness of zero or less draws nothing; one that leaves no hole fills the
    /// ellipse.
    pub fn stroke_ellipse(&mut self, rect: Rect, thickness: i32, colour: Colour) {
        let outer = self.to_device(rect);
        let t = i64::from(self.device_len(thickness));
        if t <= 0 {
            return;
        }
        let (x0, y0, x1, y1) = edges(&outer);
        let inner = Rect::from_edges(x0 + t, y0 + t, x1 - t, y1 - t);
        if inner.is_empty() {
            return self.fill_ellipse(rect, colour);
        }
        let Some((first, last)) = self.visible_rows(&outer, colour) else {
            return;
        };
        let (_, inner_top, _, inner_bottom) = edges(&inner);
        for py in first..last {
            let (from, to) = ellipse_span(&outer, py);
            if (inner_top..inner_bottom).contains(&py) {
                let (hole_from, hole_to) = ellipse_span(&inner, py);
                if hole_from < hole_to {
                    // The hole is inside the ellipse, so the clamps change nothing; they are
                    // there so that a mistake could never draw outside it.
                    self.span(py as i32, cut(from), cut(hole_from.clamp(from, to)), colour);
                    self.span(py as i32, cut(hole_to.clamp(from, to)), cut(to), colour);
                    continue;
                }
            }
            self.span(py as i32, cut(from), cut(to), colour);
        }
        self.damaged(outer);
    }

    /// Fills a rectangle with colours running from `top` (its first row) to `bottom` (its last).
    /// The colours may be see-through; they are mixed with what is there.
    ///
    /// Each row's colour is [`Colour::between`] counted from the rectangle's real first row,
    /// even where the clip cuts the top off, so clipping never changes a colour. A one-row
    /// rectangle is all `top`.
    pub fn gradient_down(&mut self, rect: Rect, top: Colour, bottom: Colour) {
        let whole = self.to_device(rect);
        let area = whole.intersect(&self.device_clip());
        if area.is_empty() || (top.alpha() == 0 && bottom.alpha() == 0) {
            return;
        }
        let steps = whole.height - 1;
        for y in area.y..cut(area.bottom()) {
            let colour = Colour::between(top, bottom, y - whole.y, steps);
            self.span(y, area.x, cut(area.right()), colour);
        }
        self.damaged(whole);
    }

    /// Fills a rectangle with colours running from `left` (its first column) to `right` (its
    /// last), as [`Canvas::gradient_down`] does from top to bottom.
    pub fn gradient_across(&mut self, rect: Rect, left: Colour, right: Colour) {
        let whole = self.to_device(rect);
        let area = whole.intersect(&self.device_clip());
        if area.is_empty() || (left.alpha() == 0 && right.alpha() == 0) {
            return;
        }
        // Every row has the same colours, so they are worked out once. At most one entry per
        // column of the clip, so this is as big as the canvas is wide, never as big as the
        // rectangle.
        let steps = whole.width - 1;
        let colours: Vec<Colour> = (area.x..cut(area.right()))
            .map(|x| Colour::between(left, right, x - whole.x, steps))
            .collect();
        for y in area.y..cut(area.bottom()) {
            let (row, _) = self.row_mut(y, area.x, cut(area.right()));
            for (pixel, colour) in row.iter_mut().zip(&colours) {
                mix(*colour, pixel);
            }
        }
        self.damaged(whole);
    }

    /// Draws a soft shadow: the shape of a rounded rectangle (see
    /// [`Canvas::fill_round_rect`], with the same `radius`), fading out over `blur` pixels
    /// outside it. Nothing is drawn farther than `blur` from the shape. A blur of zero or less
    /// is just [`Canvas::fill_round_rect`]; one over 1024 device pixels is cut to that.
    ///
    /// For each pixel, `e` is twice the distance from the pixel's centre to the rounded
    /// rectangle, rounded up to a whole number (0 for every pixel that
    /// [`Canvas::fill_round_rect`] fills, and for no other: rounding *down* would also give 0
    /// to some pixels just outside a corner). With `A` the colour's alpha and `B` the blur in
    /// device pixels, a pixel is drawn with the colour's alpha replaced by
    /// `(A·(2B - e)² + (2B)²/2) / (2B)²`, using whole-number division: all of `A` at the
    /// shape's edge, falling smoothly (as a parabola) to nothing at `e = 2B`.
    pub fn shadow(&mut self, rect: Rect, radius: i32, blur: i32, colour: Colour) {
        let outer = self.to_device(rect);
        let r = self.corner_radius(&outer, radius);
        let b = i64::from(self.device_len(blur)).min(MAX_BLUR);
        if b <= 0 {
            return self.fill_round_device(outer, r, colour);
        }
        let alpha = u64::from(colour.alpha());
        let (x0, y0, x1, y1) = edges(&outer);
        let reach = Rect::from_edges(x0 - b, y0 - b, x1 + b, y1 + b);
        let clip = self.device_clip();
        let seen = reach.intersect(&clip);
        if outer.is_empty() || alpha == 0 || seen.is_empty() {
            return;
        }
        // The alpha for each `e` from 0 to 2B - 1, worked out once. `u64` so that nothing
        // here can overflow: 255 · (2048)² is far below 2⁶⁴.
        let two_b = 2 * b as u64;
        let table: Vec<u8> = (0..two_b)
            .map(|e| {
                ((alpha * (two_b - e) * (two_b - e) + two_b * two_b / 2) / (two_b * two_b)) as u8
            })
            .collect();
        // A pixel is drawn only if `e < 2B`: if its doubled distance from the corner circle's
        // centre, rounded up, is below `2R + 2B`, that is at most this.
        let most = 2 * r + 2 * b - 1;
        for py in i64::from(seen.y)..seen.bottom() {
            let centre = 2 * py + 1;
            let qy = (2 * (y0 + r) - centre).max(centre - 2 * (y1 - r)).max(0);
            if qy > most {
                continue;
            }
            // The columns between the corner circles' centres all have the same `e`, so one
            // run draws them (a straight stretch of edge needs no square root at all).
            let straight = (qy - 2 * r).max(0) as usize;
            if let Some(&a) = table.get(straight) {
                self.span(py as i32, cut(x0 + r), cut(x1 - r), colour.with_alpha(a));
            }
            // Beyond them, to the left and to the right, `e` depends on the distance across
            // too, and each pixel needs a square root. Only the `k` columns on each side that
            // can reach `e < 2B` are visited, and only those inside the clip.
            let across = isqrt((most * most - qy * qy) as u64) as i64;
            let k = (across + 1) / 2;
            let (left_edge, right_edge) = (x0 + r, x1 - r);
            let (row, from) = self.row_mut(py as i32, cut(left_edge - k), cut(left_edge));
            for (i, pixel) in row.iter_mut().enumerate() {
                // The pixel's centre is `t` pixels left of the circle's centre, so twice that
                // distance is `2t - 1` (the centre of the pixel next to it is half a pixel away).
                let t = left_edge - (i64::from(from) + i as i64);
                mix_shadow(&table, r, qy, t, colour, pixel);
            }
            let (row, from) = self.row_mut(py as i32, cut(right_edge), cut(right_edge + k));
            for (i, pixel) in row.iter_mut().enumerate() {
                let t = i64::from(from) + i as i64 - right_edge + 1;
                mix_shadow(&table, r, qy, t, colour, pixel);
            }
        }
        self.damaged(reach);
    }

    /// The corner radius to use for a device rectangle: the logical `radius` in device pixels,
    /// brought into 0 to half the shorter side, so that the corners never overlap.
    fn corner_radius(&self, outer: &Rect, radius: i32) -> i64 {
        let most = i64::from(outer.width.min(outer.height).max(0) / 2);
        i64::from(self.device_len(radius)).clamp(0, most)
    }

    /// The first row and the one past the last that are worth drawing for a shape inside
    /// `outer`: the rows it shares with the clip. `None` if there are none, or the colour is
    /// clear, so that a call that draws nothing records no damage.
    fn visible_rows(&self, outer: &Rect, colour: Colour) -> Option<(i64, i64)> {
        let area = outer.intersect(&self.device_clip());
        if area.is_empty() || colour.alpha() == 0 {
            return None;
        }
        Some((i64::from(area.y), area.bottom()))
    }

    /// Fills a rounded rectangle given in device pixels, with a radius in device pixels that
    /// is already in range.
    fn fill_round_device(&mut self, outer: Rect, r: i64, colour: Colour) {
        let Some((first, last)) = self.visible_rows(&outer, colour) else {
            return;
        };
        let (x0, y0, x1, y1) = edges(&outer);
        for py in first..last {
            let inset = corner_inset(py, y0, y1, r);
            self.span(py as i32, cut(x0 + inset), cut(x1 - inset), colour);
        }
        self.damaged(outer);
    }

    /// Draws the outline of a rounded rectangle given in device pixels, `t` thick: the
    /// rectangle minus the smaller one inside it. Both are worked out a row at a time, and
    /// each row is up to two runs, the part left of the hole and the part right of it.
    fn stroke_round_device(&mut self, outer: Rect, r: i64, t: i64, colour: Colour) {
        if t <= 0 {
            return;
        }
        let (x0, y0, x1, y1) = edges(&outer);
        let inner = Rect::from_edges(x0 + t, y0 + t, x1 - t, y1 - t);
        if inner.is_empty() {
            return self.fill_round_device(outer, r, colour);
        }
        let Some((first, last)) = self.visible_rows(&outer, colour) else {
            return;
        };
        let (ix0, iy0, ix1, iy1) = edges(&inner);
        // The hole's corners have the same centres as the outline's: radius `r - t`.
        let inner_r = (r - t).max(0);
        for py in first..last {
            let inset = corner_inset(py, y0, y1, r);
            let (from, to) = (x0 + inset, x1 - inset);
            if (iy0..iy1).contains(&py) {
                let hole = corner_inset(py, iy0, iy1, inner_r);
                // The hole is inside the outline, so the clamps change nothing; they are
                // there so that a mistake could never draw outside it.
                self.span(py as i32, cut(from), cut((ix0 + hole).clamp(from, to)), colour);
                self.span(py as i32, cut((ix1 - hole).clamp(from, to)), cut(to), colour);
            } else {
                self.span(py as i32, cut(from), cut(to), colour);
            }
        }
        self.damaged(outer);
    }
}

/// Mixes the shadow's colour into one pixel that is `t` pixels (1 or more) across from the
/// nearer corner circle's centre column and has doubled distance `qy` from its centre row.
fn mix_shadow(table: &[u8], r: i64, qy: i64, t: i64, colour: Colour, pixel: &mut u32) {
    let qx = 2 * t - 1;
    let distance = isqrt_up((qx * qx + qy * qy) as u64) as i64;
    // `e` is zero inside the rounded rectangle; `get` gives `None` past the end of the table
    // (where nothing is drawn), so a wrong count could never read outside it.
    if let Some(&alpha) = table.get((distance - 2 * r).max(0) as usize) {
        mix(colour.with_alpha(alpha), pixel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_exact_round_down() {
        for n in 0..70_000u64 {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
            assert_eq!(isqrt_up(n), if r * r == n { r } else { r + 1 }, "{n}");
        }
        // Either side of every square, from small ones up to the biggest.
        for i in (0..=u64::from(u32::MAX))
            .step_by(997)
            .chain(u64::from(u32::MAX) - 5..=u64::from(u32::MAX))
        {
            let square = i * i;
            assert_eq!(isqrt(square), i);
            if i > 0 {
                assert_eq!(isqrt(square - 1), i - 1);
            }
            if i < u64::from(u32::MAX) {
                assert_eq!(isqrt(square + 2 * i), i, "the last number before the next square");
                assert_eq!(isqrt(square + 2 * i + 1), i + 1);
            }
        }
        assert_eq!(isqrt(u64::MAX), u64::from(u32::MAX));
        assert_eq!(isqrt(u64::MAX - 1), u64::from(u32::MAX));
        assert_eq!(isqrt(1 << 63), 3_037_000_499);
        assert_eq!(isqrt_up(u64::MAX), 1 << 32);
        assert_eq!(isqrt_up(0), 0);
    }
}
