// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Pictures: copying, stretching, shrinking and turning pixels that belong to someone else onto
//! a canvas (docs/design/0023-draw-window-and-sandstone.md, section 2).
//!
//! A [`Picture`] is pixels that are only looked at, never changed: an icon, a window's frame
//! that the display server is composing, or another canvas's pixels. Three calls draw one:
//! [`Canvas::picture`] (each picture pixel fills one logical pixel), [`Canvas::picture_turned`]
//! (the same after a quarter-turn rotation) and [`Canvas::picture_fit`] (stretched or shrunk
//! to fill a rectangle).
//!
//! **Two ways of scaling.** Drawing a picture into a rectangle of another size has to decide
//! which picture pixels each canvas pixel shows:
//! - **Nearest** is used when the rectangle's size is a whole number of times the picture's,
//!   across and down (including 1 time, a plain copy). Every picture pixel simply becomes a
//!   block of canvas pixels of one colour, so the result is crisp, with no new colours. It is
//!   also the common case (an icon at 2x, a window copied one for one), so it is the fast one.
//! - **Smooth** is used otherwise. Imagine the picture stretched over the rectangle: each canvas
//!   pixel then covers a piece of the picture, partly covering some picture pixels. Its colour
//!   is the average of those, each counted in proportion to how much of it the canvas pixel
//!   covers (its *weight*). Averaging by area is the right way to shrink (it never skips a thin
//!   line) and gives soft edges when stretching. All the arithmetic is on whole numbers, so
//!   the same picture comes out the same on every processor, and a tiny rounding rule
//!   (add half, then divide) makes the average round to the nearest value.
//!
//! **Alpha.** A picture made `with_alpha` has a *straight* alpha in its top byte, like
//! [`Colour`]. Averaging weighs each pixel's colour by how solid it is, so a clear pixel's
//! (meaningless) colour never leaks in, and the averaged pixel is mixed over the canvas with
//! [`Colour::over`], once. A picture without alpha ignores its top byte and is always solid.
//!
//! **Why the work is bounded.** Every loop here runs over the canvas pixels that are inside
//! the clip, never over the whole destination rectangle (which may be millions of pixels wide,
//! when the arguments are hostile), and for each of those only over the picture pixels that
//! it covers. A turned picture is never copied: each canvas pixel works out which picture
//! pixel it shows.

use crate::{Canvas, Colour, MAX_SIDE, Rect};

/// A picture to draw from: pixels that belong to someone else, looked at but never changed.
///
/// The `'a` is a Rust *lifetime*: it says the picture borrows its pixels from somewhere else
/// and can't outlive them. `&'a [u32]` is a shared borrow, which many pictures (and parts of
/// pictures) can hold at once, since nobody can change the pixels while they are borrowed.
/// `#[derive(Clone, Copy)]` makes a picture as cheap to pass around as a few numbers: it is
/// only a window onto the pixels, not a copy of them.
#[derive(Clone, Copy, Debug)]
pub struct Picture<'a> {
    /// Starts at the picture's top left pixel.
    pixels: &'a [u32],
    width: i32,
    height: i32,
    /// How many pixels from the start of one row to the start of the next.
    stride: usize,
    /// Whether the top byte of a pixel means alpha.
    alpha: bool,
}

impl<'a> Picture<'a> {
    /// A solid picture on `pixels`: `height` rows of `width` pixels (`0x__RRGGBB`; the top byte
    /// is ignored), each row starting `stride` pixels after the one before.
    ///
    /// `None` if `pixels` is too short to hold that, if `stride` is less than `width`, or if
    /// either side is longer than [`MAX_SIDE`] (the checks [`Canvas::new`] makes). `Option` is
    /// Rust's "maybe a value": `Some(picture)` or `None`. A picture may be empty (no width or
    /// no height): it draws nothing.
    pub fn new(pixels: &'a [u32], width: u32, height: u32, stride: u32) -> Option<Picture<'a>> {
        if width > MAX_SIDE || height > MAX_SIDE || stride < width {
            return None;
        }
        // The last row needs only its `width` pixels, not a whole stride. 64-bit, because
        // `stride` can be anything.
        let needed = if width == 0 || height == 0 {
            0
        } else {
            u64::from(stride) * u64::from(height - 1) + u64::from(width)
        };
        if needed > pixels.len() as u64 {
            return None;
        }
        Some(Picture {
            pixels,
            width: width as i32,
            height: height as i32,
            // `needed` fits in the slice, so `stride` fits in a `usize` unless there is only
            // one row, when it is never used.
            stride: usize::try_from(stride).unwrap_or(usize::MAX),
            alpha: false,
        })
    }

    /// The same picture with its top byte meaning alpha (`0xAARRGGBB`, straight alpha, as
    /// [`Colour`]): see-through where it says so. `self` without `&` takes the picture and
    /// gives back a changed one (it is `Copy`, so the original is still usable too).
    pub fn with_alpha(mut self) -> Picture<'a> {
        self.alpha = true;
        self
    }

    /// How many pixels wide.
    pub fn width(&self) -> u32 {
        self.width as u32
    }

    /// How many pixels high.
    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// The part of the picture inside `area` (in the picture's own pixels), as a picture. The
    /// part of `area` outside the picture is left out, so the result may be smaller, or empty.
    /// It keeps the picture's alpha.
    pub fn part(&self, area: Rect) -> Picture<'a> {
        let inside = Rect::new(0, 0, self.width, self.height).intersect(&area);
        if inside.is_empty() {
            return Picture { pixels: &[], width: 0, height: 0, stride: 0, alpha: self.alpha };
        }
        // Copying the reference out first keeps the long lifetime `'a`: borrowing from `self`
        // would only last as long as `self` does.
        let pixels: &'a [u32] = self.pixels;
        // `inside` is inside the picture, so the start is before the end of the pixels (and
        // the rows after it are still all there).
        let start = inside.y as usize * self.stride + inside.x as usize;
        Picture {
            pixels: &pixels[start..],
            width: inside.width,
            height: inside.height,
            stride: self.stride,
            alpha: self.alpha,
        }
    }

    /// The pixel at (`x`, `y`) as a colour (alpha 255 for a solid picture), or `None` outside.
    pub fn pixel(&self, x: i32, y: i32) -> Option<Colour> {
        if !Rect::new(0, 0, self.width, self.height).contains(x, y) {
            return None;
        }
        let pixel = self.pixels[y as usize * self.stride + x as usize];
        Some(if self.alpha { Colour(pixel) } else { Colour::solid(pixel) })
    }
}

/// A quarter-turn rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not turned.
    None,
    /// A quarter turn clockwise: the top of the picture ends up on the right.
    Right,
    /// A half turn: upside down.
    Half,
    /// A quarter turn anticlockwise: the top of the picture ends up on the left.
    Left,
}

/// A picture as it will be drawn, possibly turned: its size after turning, and how to find the
/// pixel at any place in the turned picture without making a turned copy.
///
/// For a place (x, y) in the turned picture, the pixel is at index `base + x * across + y *
/// down` in the original's pixels. Each turn is a different choice of those three numbers
/// (signed: turning runs through the pixels backwards in some direction).
struct View<'p> {
    pixels: &'p [u32],
    width: i64,
    height: i64,
    base: i64,
    across: i64,
    down: i64,
    alpha: bool,
}

impl<'p> View<'p> {
    fn new(picture: &Picture<'p>, turn: Turn) -> View<'p> {
        let (w, h) = (i64::from(picture.width), i64::from(picture.height));
        let stride = i64::try_from(picture.stride).unwrap_or(i64::MAX / 0x10000);
        // `match` picks one branch by the value of `turn` (and the compiler checks that every
        // case is covered). Each arm is: the turned picture's size, then where its (0, 0) is
        // in the original and how far one step right and one step down move through it. For
        // a quarter turn to the right the original's pixel (sx, sy) lands at (h - 1 - sy, sx),
        // so the turned picture's (x, y) shows (sx, sy) = (y, h - 1 - x); the others likewise.
        let (width, height, base, across, down) = match turn {
            Turn::None => (w, h, 0, 1, stride),
            Turn::Right => (h, w, (h - 1) * stride, -stride, 1),
            Turn::Half => (w, h, (h - 1) * stride + w - 1, -1, -stride),
            Turn::Left => (h, w, w - 1, stride, -1),
        };
        View { pixels: picture.pixels, width, height, base, across, down, alpha: picture.alpha }
    }

    /// The raw pixel at (x, y) of the turned picture, which must be inside it.
    #[inline]
    fn at(&self, x: i64, y: i64) -> u32 {
        self.pixels[(self.base + x * self.across + y * self.down) as usize]
    }

    /// The pixel at (x, y) as a colour (solid, if the picture has no alpha).
    #[inline]
    fn colour(&self, x: i64, y: i64) -> Colour {
        let pixel = self.at(x, y);
        if self.alpha { Colour(pixel) } else { Colour(0xFF00_0000 | (pixel & 0x00FF_FFFF)) }
    }
}

/// Draws one source pixel `p` onto the canvas pixel `d`.
#[inline]
fn put(d: &mut u32, p: u32, alpha: bool) {
    if !alpha {
        // The canvas is always solid, whatever the picture's spare byte says.
        *d = 0xFF00_0000 | (p & 0x00FF_FFFF);
        return;
    }
    match p >> 24 {
        0 => {}
        255 => *d = p,
        _ => *d = Colour(p).over(*d),
    }
}

/// Where the edges of the canvas pixels fall on the picture's pixels, stepping from one canvas
/// pixel to the next without dividing every time.
///
/// For the stretched picture, canvas pixel `i` starts at `i * from` in units where a picture
/// pixel is `to` long (`from` is the picture's size along this direction, `to` the
/// destination's). `index` is that start divided by `to`, rounded down (the picture pixel the
/// edge is in) and `rest` is what is left over, so `rest == 0` means the edge is exactly on a
/// picture pixel's edge.
#[derive(Clone, Copy)]
struct Edge {
    index: u64,
    rest: u64,
}

impl Edge {
    /// The edge at the start of canvas pixel `i`. One division, once per row or column.
    fn at(i: u64, from: u64, to: u64) -> Edge {
        Edge { index: i * from / to, rest: i * from % to }
    }

    /// Moves on to the next canvas pixel's edge.
    fn step(&mut self, from: u64, to: u64) {
        self.rest += from;
        if self.rest >= to {
            self.index += self.rest / to;
            self.rest %= to;
        }
    }
}

/// How far two intervals overlap: the interval of picture pixel `k` (from `k * to` to `(k + 1)
/// * to`) and that of canvas pixel `i` (from `i * from` to `(i + 1) * from`). A whole number,
/// and for one `i` the overlaps with all the `k` add up to `from`.
#[inline]
fn overlap(k: u64, i: u64, from: u64, to: u64) -> u64 {
    let low = (k * to).max(i * from);
    let high = ((k + 1) * to).min((i + 1) * from);
    high.saturating_sub(low)
}

impl Canvas<'_> {
    /// Draws a picture with its top left corner at (`x`, `y`): each of its pixels fills one
    /// logical pixel (so a block of scale x scale pixels, when the canvas's scale is more than 1).
    ///
    /// Only the part inside the clip is drawn. A picture with alpha is mixed over what is
    /// there, once per pixel; a solid one replaces it.
    pub fn picture(&mut self, picture: &Picture, x: i32, y: i32) {
        self.picture_fit(picture, Rect::new(x, y, picture.width, picture.height));
    }

    /// Draws a picture turned by `turn`, with the turned picture's top left corner at (`x`,
    /// `y`). The turned picture is 'h' wide and 'w' high for a quarter turn, if the picture
    /// is w wide and h high.
    pub fn picture_turned(&mut self, picture: &Picture, x: i32, y: i32, turn: Turn) {
        let view = View::new(picture, turn);
        // The sizes are at most `MAX_SIDE`, so they fit an `i32`.
        let dest = Rect::new(x, y, view.width as i32, view.height as i32);
        self.draw_view(&view, dest);
    }

    /// Draws a picture stretched or shrunk to fill `dest` (in logical pixels). Whole-number
    /// factors are drawn crisp (each picture pixel a block); other sizes are averaged by area
    /// (see the top of this file). Nothing is drawn if the picture or `dest` is empty.
    pub fn picture_fit(&mut self, picture: &Picture, dest: Rect) {
        self.draw_view(&View::new(picture, Turn::None), dest);
    }

    /// The one place pictures are drawn, turned or not.
    fn draw_view(&mut self, view: &View, dest: Rect) {
        if view.width == 0 || view.height == 0 {
            return;
        }
        let device = self.to_device(dest);
        // Only the pixels of the destination that are inside the clip are worked out: this
        // is what keeps a destination of millions of pixels as cheap as the clip is small.
        let visible = device.intersect(&self.device_clip());
        if visible.is_empty() {
            return;
        }
        let (dw, dh) = (i64::from(device.width), i64::from(device.height));
        if dw % view.width == 0 && dh % view.height == 0 {
            self.draw_nearest(view, device, visible, dw / view.width, dh / view.height);
        } else {
            self.draw_smooth(view, device, visible);
        }
        self.damaged(visible);
    }

    /// Whole-number factors: device pixel (`x0 + i`, `y0 + j`) shows picture pixel
    /// (`i / fx`, `j / fy`). `device` is the whole destination, `visible` the part to draw.
    fn draw_nearest(&mut self, view: &View, device: Rect, visible: Rect, fx: i64, fy: i64) {
        for y in visible.y..visible.y + visible.height {
            let sy = i64::from(y - device.y) / fy;
            if fx == 1 {
                // One for one. `row_mut` gives the part of the row that is inside the clip
                // (all of `visible`'s, which is inside it) and the column it starts at.
                let (row, start) = self.row_mut(y, visible.x, visible.x + visible.width);
                let first = i64::from(start - device.x);
                copy_row(row, view, first, sy);
            } else {
                // Each picture pixel is a run of `fx` device pixels in this row: draw the
                // runs that are at least partly visible. `span` mixes or fills a run and cuts
                // it to the clip.
                let first = i64::from(visible.x - device.x) / fx;
                let last = (i64::from(visible.x + visible.width - 1 - device.x)) / fx;
                for sx in first..=last {
                    let from = i64::from(device.x) + sx * fx;
                    // These are within a few times `DEVICE_LIMIT`, so they fit in an `i32`.
                    self.span(y, from as i32, (from + fx) as i32, view.colour(sx, sy));
                }
            }
        }
    }

    /// Any other size: each device pixel is the average of the picture pixels it covers,
    /// weighted by how much of each it covers (see the top of this file).
    fn draw_smooth(&mut self, view: &View, device: Rect, visible: Rect) {
        // All of these are at most 2^25, and the sums below at most 2^28 * 255 * 255.
        let (sw, sh) = (view.width as u64, view.height as u64);
        let (dw, dh) = (device.width as u64, device.height as u64);
        let total = sw * sh;
        let (i0, j0) = ((visible.x - device.x) as u64, (visible.y - device.y) as u64);
        let mut rows = Edge::at(j0, sh, dh);
        for y in visible.y..visible.y + visible.height {
            let j = (y - device.y) as u64;
            // The picture rows from this device row's top edge to its bottom edge (the last
            // one only partly, unless the edge falls exactly between two).
            let mut next = rows;
            next.step(sh, dh);
            let (ky0, ky1) = (rows.index, next.index + u64::from(next.rest > 0));
            rows = next;

            let (row, start) = self.row_mut(y, visible.x, visible.x + visible.width);
            let mut cols = Edge::at(i0 + (start - visible.x) as u64, sw, dw);
            for (n, d) in row.iter_mut().enumerate() {
                let i = i0 + (start - visible.x) as u64 + n as u64;
                let mut next = cols;
                next.step(sw, dw);
                let (kx0, kx1) = (cols.index, next.index + u64::from(next.rest > 0));
                cols = next;

                if kx1 - kx0 == 1 && ky1 - ky0 == 1 {
                    // Inside one picture pixel: its colour exactly, with no sums.
                    let colour = view.colour(kx0 as i64, ky0 as i64);
                    put(d, colour.0, view.alpha);
                    continue;
                }
                let (mut sum_w, mut sum_r, mut sum_g, mut sum_b) = (0u64, 0u64, 0u64, 0u64);
                for ky in ky0..ky1 {
                    let wy = overlap(ky, j, sh, dh);
                    for kx in kx0..kx1 {
                        let p = view.at(kx as i64, ky as i64);
                        // A pixel's weight: how much of the device pixel it covers, times how
                        // solid it is. A picture without alpha is solid everywhere.
                        let mut w = wy * overlap(kx, i, sw, dw);
                        if view.alpha {
                            w *= u64::from(p >> 24);
                        }
                        sum_w += w;
                        sum_r += w * u64::from((p >> 16) & 0xFF);
                        sum_g += w * u64::from((p >> 8) & 0xFF);
                        sum_b += w * u64::from(p & 0xFF);
                    }
                }
                if !view.alpha {
                    // `sum_w` is `total` here. Adding half of it before dividing rounds to
                    // the nearest.
                    let avg = |sum: u64| (sum + total / 2) / total;
                    *d = 0xFF00_0000
                        | (avg(sum_r) as u32) << 16
                        | (avg(sum_g) as u32) << 8
                        | avg(sum_b) as u32;
                } else if sum_w != 0 {
                    // Colours are averaged by how solid each pixel is; the alpha by area.
                    let avg = |sum: u64| ((sum + sum_w / 2) / sum_w) as u32;
                    let alpha = ((sum_w + total / 2) / total) as u32;
                    let colour =
                        Colour(alpha << 24 | avg(sum_r) << 16 | avg(sum_g) << 8 | avg(sum_b));
                    *d = colour.over(*d);
                }
            }
        }
    }
}

/// Draws one row of a picture one for one onto `row`: the pixels at (`first`, `y`) onwards.
fn copy_row(row: &mut [u32], view: &View, first: i64, y: i64) {
    if row.is_empty() {
        return;
    }
    if view.across == 1 {
        // An unturned picture: its pixels in this row are next to each other, so they can
        // be taken as a slice and copied with a plain loop the compiler turns into vector
        // instructions. The slice is in the picture because the row is inside the destination.
        let at = (view.base + first + y * view.down) as usize;
        let source = &view.pixels[at..at + row.len()];
        if view.alpha {
            row.iter_mut().zip(source).for_each(|(d, &p)| put(d, p, true));
        } else {
            row.iter_mut().zip(source).for_each(|(d, &p)| *d = 0xFF00_0000 | (p & 0x00FF_FFFF));
        }
    } else {
        for (n, d) in row.iter_mut().enumerate() {
            put(d, view.at(first + n as i64, y), view.alpha);
        }
    }
}
