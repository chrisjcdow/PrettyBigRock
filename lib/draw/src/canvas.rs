// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The canvas: the pixels being drawn on, and the rules every drawing call obeys.

use crate::{Colour, Damage, Rect};

/// The biggest canvas, in pixels each way. Far bigger than any screen; it keeps the arithmetic
/// here from overflowing.
pub const MAX_SIDE: u32 = 16_384;

/// The biggest scale.
pub const MAX_SCALE: u32 = 8;

/// Positions in the canvas's own pixels are cut to this far from its corner, either way,
/// before any arithmetic is done with them. It is a thousand times [`MAX_SIDE`], so nothing
/// that could be seen is changed, and it leaves room for the shapes' arithmetic: twice a
/// position squared still fits easily in 64 bits.
pub(crate) const DEVICE_LIMIT: i64 = 1 << 24;

/// Something to draw on: a view of 32-bit pixels, with the rules every drawing call obeys.
///
/// **The pixels** are `Bgrx`, as screens' buffers and windows' frames are (notes 0020 and
/// 0022): each is a `u32` holding `0x__RRGGBB`. As bytes in memory that is blue first, then
/// green, red and a spare byte, since both our processors store a number's lowest byte first
/// (*little-endian*). A canvas is always solid: drawing a see-through colour mixes it with
/// what is there (see [`Colour::over`]) and the spare byte is set to 255, so that a canvas
/// used later as a picture is solid too.
///
/// **Logical pixels and the scale.** Drawing calls take positions and sizes in *logical
/// pixels*, and the canvas multiplies them by its scale (1 on an ordinary screen, 2 on a dense
/// one), so the same calls draw the same picture twice as big, with each shape as sharp as the
/// real pixels allow. The real pixels are called *device pixels* here.
///
/// **The clip.** Nothing is ever drawn outside the canvas's *clip rectangle*. Positions and
/// sizes may be anything at all (negative, huge, nonsense): only the part inside the clip is
/// drawn, and nothing panics.
///
/// **Damage.** A canvas made [`Canvas::with_damage`] records every rectangle it changes (in
/// device pixels), for `present` and `commit`.
///
/// The `'a` is a Rust *lifetime*: it says the canvas borrows its pixels (and its damage list)
/// from somewhere else and can't outlive them. While the canvas exists nothing else can touch
/// those pixels, which the compiler checks; that is what `&mut` (a unique borrow) means.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    /// How many pixels from the start of one row to the start of the next.
    stride: usize,
    /// Its size in device pixels.
    width: i32,
    height: i32,
    scale: i32,
    /// Where logical (0, 0) is, in device pixels from the top left corner.
    origin: (i32, i32),
    /// In device pixels; always inside the canvas.
    clip: Rect,
    /// `Option` is Rust's "maybe a value": `Some(list)` or `None`.
    damage: Option<&'a mut Damage>,
}

impl<'a> Canvas<'a> {
    /// A canvas on `pixels`: `height` rows of `width` pixels, each row starting `stride`
    /// pixels after the one before (rows may have spare pixels at the end, which are never
    /// touched). Its scale is 1 and its clip is all of it.
    ///
    /// `None` if `pixels` is too short to hold that, if `stride` is less than `width`, or if
    /// either side is longer than [`MAX_SIDE`]. A canvas may be empty (no width or no height):
    /// nothing drawn on it shows.
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32, stride: u32) -> Option<Canvas<'a>> {
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
        Some(Canvas {
            pixels,
            // `needed` fits in the slice, so `stride` fits in a `usize` unless there is only
            // one row, when it is never used.
            stride: usize::try_from(stride).unwrap_or(usize::MAX),
            width: width as i32,
            height: height as i32,
            scale: 1,
            origin: (0, 0),
            clip: Rect::new(0, 0, width as i32, height as i32),
            damage: None,
        })
    }

    /// The same canvas with another scale: 1 to [`MAX_SCALE`] (anything else is brought into
    /// that range). `self` without `&` means this takes the canvas and gives back a changed
    /// one, so calls can be chained: `Canvas::new(..)?.with_scale(2)`.
    pub fn with_scale(mut self, scale: u32) -> Canvas<'a> {
        self.scale = scale.clamp(1, MAX_SCALE) as i32;
        self
    }

    /// Every pixel at once, for code that works each one out itself (the desktop's wallpaper):
    /// the pixels, the width and height in device pixels, and the stride. It ignores the clip
    /// and the origin, so it is only for a whole canvas of one's own; the whole canvas counts as
    /// changed.
    pub fn whole(&mut self) -> (&mut [u32], u32, u32, u32) {
        let all = Rect::new(0, 0, self.width, self.height);
        if let Some(d) = self.damage.as_deref_mut() {
            d.add(all);
        }
        let stride = u32::try_from(self.stride).unwrap_or(u32::MAX);
        (&mut *self.pixels, self.width as u32, self.height as u32, stride)
    }

    /// The same canvas, recording every rectangle it changes into `damage`, in device pixels.
    pub fn with_damage(mut self, damage: &'a mut Damage) -> Canvas<'a> {
        self.damage = Some(damage);
        self
    }

    /// How many device pixels a logical pixel is, each way.
    pub fn scale(&self) -> u32 {
        self.scale as u32
    }

    /// Its size in device pixels: (width, height).
    pub fn device_size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    /// The part that can be drawn on, in logical pixels: the smallest rectangle of whole
    /// logical pixels that holds the clip. Code that draws a lot (a long list, a big picture)
    /// can use it to leave out what wouldn't show.
    pub fn clip_bounds(&self) -> Rect {
        if self.clip.is_empty() {
            return Rect::EMPTY;
        }
        let (s, (ox, oy)) = (i64::from(self.scale), self.origin);
        let c = &self.clip;
        // `div_euclid` rounds down for negative numbers too; adding `s - 1` first rounds up.
        Rect::from_edges(
            (i64::from(c.x) - i64::from(ox)).div_euclid(s),
            (i64::from(c.y) - i64::from(oy)).div_euclid(s),
            (c.right() - i64::from(ox) + s - 1).div_euclid(s),
            (c.bottom() - i64::from(oy) + s - 1).div_euclid(s),
        )
    }

    /// A canvas on part of this one: `area` (in this canvas's logical pixels) becomes the new
    /// canvas's whole world. Its (0, 0) is `area`'s corner and it can't draw outside `area`,
    /// nor outside this canvas's clip. The scale and the damage list carry over.
    ///
    /// This is how a window is divided among its parts: each draws from (0, 0) and can't
    /// spill onto its neighbours. The new canvas borrows this one (`'_` is the lifetime of
    /// that borrow), so this one can't be used until the new one is finished with.
    pub fn sub(&mut self, area: Rect) -> Canvas<'_> {
        let device = self.to_device(area);
        let origin = (self.device_x(area.x), self.device_y(area.y));
        Canvas {
            // `&mut *` borrows again from a borrow ("reborrowing"), for the shorter lifetime.
            pixels: &mut *self.pixels,
            stride: self.stride,
            width: self.width,
            height: self.height,
            scale: self.scale,
            origin,
            clip: self.clip.intersect(&device),
            // `as_deref_mut` reborrows what is inside the `Option`, if anything.
            damage: self.damage.as_deref_mut(),
        }
    }

    /// A canvas with the same (0, 0) as this one that can only draw inside `area` (and this
    /// canvas's clip).
    pub fn clipped(&mut self, area: Rect) -> Canvas<'_> {
        let origin = self.origin;
        let mut sub = self.sub(area);
        sub.origin = origin;
        sub
    }

    /// The colour of the pixel at a logical position (of its top left device pixel, when the
    /// scale is more than 1), or `None` outside the canvas. The clip doesn't matter here: it
    /// limits drawing, not looking.
    pub fn pixel(&self, x: i32, y: i32) -> Option<Colour> {
        let (x, y) = (self.device_x(x), self.device_y(y));
        if !Rect::new(0, 0, self.width, self.height).contains(x, y) {
            return None;
        }
        Some(Colour::solid(self.pixels[y as usize * self.stride + x as usize]))
    }

    /// Fills everything that can be drawn on with `colour`.
    pub fn fill(&mut self, colour: Colour) {
        self.fill_device(self.clip, colour);
    }

    /// Fills a rectangle with `colour`.
    pub fn fill_rect(&mut self, rect: Rect, colour: Colour) {
        self.fill_device(self.to_device(rect), colour);
    }

    /// Draws the outline of a rectangle: a line `thickness` wide just inside its edges. A
    /// thickness of zero or less draws nothing; one that leaves no hole fills the rectangle.
    pub fn stroke_rect(&mut self, rect: Rect, thickness: i32, colour: Colour) {
        if rect.is_empty() || thickness <= 0 {
            return;
        }
        let t = thickness;
        if i64::from(t) * 2 >= i64::from(rect.width.min(rect.height)) {
            return self.fill_rect(rect, colour);
        }
        let Rect { x, y, width, .. } = rect;
        // Four bars that don't overlap, so that a see-through colour isn't drawn twice at
        // the corners. The arithmetic can't overflow: `t` is less than half of each side.
        self.fill_rect(Rect::new(x, y, width, t), colour);
        self.fill_rect(
            Rect::from_edges(x.into(), rect.bottom() - i64::from(t), rect.right(), rect.bottom()),
            colour,
        );
        let (top, bottom) = (i64::from(y) + i64::from(t), rect.bottom() - i64::from(t));
        self.fill_rect(
            Rect::from_edges(x.into(), top, i64::from(x) + i64::from(t), bottom),
            colour,
        );
        self.fill_rect(
            Rect::from_edges(rect.right() - i64::from(t), top, rect.right(), bottom),
            colour,
        );
    }

    // ---- What the rest of this library draws with. Everything below is in device pixels. ----

    /// The device column of a logical x, cut to [`DEVICE_LIMIT`] either side of the corner.
    pub(crate) fn device_x(&self, x: i32) -> i32 {
        let at = i64::from(self.origin.0) + i64::from(x) * i64::from(self.scale);
        at.clamp(-DEVICE_LIMIT, DEVICE_LIMIT) as i32
    }

    /// The device row of a logical y, cut the same way.
    pub(crate) fn device_y(&self, y: i32) -> i32 {
        let at = i64::from(self.origin.1) + i64::from(y) * i64::from(self.scale);
        at.clamp(-DEVICE_LIMIT, DEVICE_LIMIT) as i32
    }

    /// A logical length in device pixels, cut to [`DEVICE_LIMIT`] (and to its negative).
    pub(crate) fn device_len(&self, length: i32) -> i32 {
        (i64::from(length) * i64::from(self.scale)).clamp(-DEVICE_LIMIT, DEVICE_LIMIT) as i32
    }

    /// A logical rectangle in device pixels (not clipped; each edge cut to [`DEVICE_LIMIT`]).
    pub(crate) fn to_device(&self, rect: Rect) -> Rect {
        if rect.is_empty() {
            return Rect::EMPTY;
        }
        let (s, limit) = (i64::from(self.scale), DEVICE_LIMIT);
        let edge = |origin: i32, at: i64| (i64::from(origin) + at * s).clamp(-limit, limit);
        Rect::from_edges(
            edge(self.origin.0, rect.x.into()),
            edge(self.origin.1, rect.y.into()),
            edge(self.origin.0, rect.right()),
            edge(self.origin.1, rect.bottom()),
        )
    }

    /// The clip rectangle, in device pixels. Always inside the canvas.
    pub(crate) fn device_clip(&self) -> Rect {
        self.clip
    }

    /// The pixels of row `y` from column `x0` up to (not including) `x1`, cut to the clip:
    /// perhaps fewer than asked for, perhaps none. The second value is the column the slice
    /// starts at. Changing them does **not** record damage: call [`Canvas::damaged`] once for
    /// the whole shape.
    pub(crate) fn row_mut(&mut self, y: i32, x0: i32, x1: i32) -> (&mut [u32], i32) {
        let c = &self.clip;
        if c.is_empty() || y < c.y || i64::from(y) >= c.bottom() {
            return (&mut [], x0);
        }
        let (x0, x1) = (i64::from(x0).max(c.x.into()), i64::from(x1).min(c.right()));
        if x0 >= x1 {
            return (&mut [], x0 as i32);
        }
        // The clip is inside the canvas, so the row and both columns are too, and `new`
        // checked that the slice holds every pixel of the canvas.
        let start = y as usize * self.stride;
        (&mut self.pixels[start + x0 as usize..start + x1 as usize], x0 as i32)
    }

    /// Draws `colour` on row `y` from column `x0` up to (not including) `x1`, inside the clip.
    /// Does **not** record damage.
    #[inline]
    pub(crate) fn span(&mut self, y: i32, x0: i32, x1: i32, colour: Colour) {
        let (row, _) = self.row_mut(y, x0, x1);
        match colour.alpha() {
            0 => {}
            255 => row.fill(colour.0),
            _ => row.iter_mut().for_each(|p| *p = colour.over(*p)),
        }
    }

    /// The pixel at a device position, as it is stored, or `None` outside the clip.
    pub(crate) fn device_pixel(&self, x: i32, y: i32) -> Option<u32> {
        if !self.clip.contains(x, y) {
            return None;
        }
        Some(self.pixels[y as usize * self.stride + x as usize])
    }

    /// Records that the pixels of `rect` (in device pixels) may have changed. The part
    /// outside the clip is left out, since nothing there can have.
    pub(crate) fn damaged(&mut self, rect: Rect) {
        let rect = rect.intersect(&self.clip);
        // `if let` runs its block only when the `Option` holds something.
        if let Some(damage) = self.damage.as_deref_mut() {
            damage.add(rect);
        }
    }

    /// Fills a device rectangle, inside the clip, and records the damage.
    pub(crate) fn fill_device(&mut self, rect: Rect, colour: Colour) {
        let rect = rect.intersect(&self.clip);
        if rect.is_empty() || colour.alpha() == 0 {
            return;
        }
        for y in rect.y..rect.y + rect.height {
            self.span(y, rect.x, rect.x + rect.width, colour);
        }
        self.damaged(rect);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    const INK: Colour = Colour::solid(0x112233);

    /// The pixels as rows of text: `#` for [`INK`], `.` for anything untouched (0), `?` else.
    fn show(pixels: &[u32], width: usize, stride: usize) -> Vec<String> {
        pixels
            .chunks(stride)
            .map(|row| {
                row[..width.min(row.len())]
                    .iter()
                    .map(|&p| match p {
                        p if p == INK.0 => '#',
                        0 => '.',
                        _ => '?',
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_canvas_needs_enough_pixels() {
        let mut p = vec![0u32; 100];
        assert!(Canvas::new(&mut p, 10, 10, 10).is_some());
        assert!(Canvas::new(&mut p, 10, 11, 10).is_none());
        assert!(Canvas::new(&mut p, 11, 10, 10).is_none(), "the stride is less than the width");
        // The last row needn't have a whole stride.
        assert!(Canvas::new(&mut p, 4, 9, 12).is_some());
        assert!(Canvas::new(&mut p, 5, 9, 12).is_none());
        assert!(Canvas::new(&mut p, 100, 1, u32::MAX).is_some(), "one row: any stride");
        assert!(Canvas::new(&mut p, 1, 2, u32::MAX).is_none());
        assert!(Canvas::new(&mut p, MAX_SIDE + 1, 0, MAX_SIDE + 1).is_none());
        assert!(Canvas::new(&mut p, 0, MAX_SIDE + 1, 0).is_none());
        assert!(Canvas::new(&mut p, u32::MAX, u32::MAX, u32::MAX).is_none());
        // Empty canvases are allowed, and nothing can be drawn on them.
        for (w, h) in [(0, 0), (0, 10), (10, 0)] {
            let mut c = Canvas::new(&mut p, w, h, 10).unwrap();
            c.fill(INK);
            c.fill_rect(Rect::new(-5, -5, 100, 100), INK);
            assert_eq!(c.pixel(0, 0), None);
            assert_eq!(c.clip_bounds(), Rect::EMPTY);
        }
        assert!(p.iter().all(|&x| x == 0));
        let mut none: [u32; 0] = [];
        assert!(Canvas::new(&mut none, 0, 0, 0).is_some());
        assert!(Canvas::new(&mut none, 1, 1, 1).is_none());
    }

    #[test]
    fn filling_is_cut_to_the_canvas() {
        let mut p = vec![0u32; 6 * 4];
        let mut c = Canvas::new(&mut p, 6, 4, 6).unwrap();
        c.fill_rect(Rect::new(1, 1, 3, 2), INK);
        c.fill_rect(Rect::new(4, -9, 50, 11), INK); // off the top and the right
        c.fill_rect(Rect::new(-3, 3, 4, 100), INK); // off the left and the bottom
        c.fill_rect(Rect::new(6, 0, 5, 5), INK); // wholly outside
        c.fill_rect(Rect::new(0, 0, -5, 5), INK); // empty
        assert_eq!(c.pixel(1, 1), Some(INK));
        assert_eq!(c.pixel(0, 0), Some(Colour::solid(0)));
        assert_eq!(c.pixel(6, 0), None);
        assert_eq!(c.pixel(0, -1), None);
        assert_eq!(show(&p, 6, 6), ["....##", ".#####", ".###..", "#....."]);
    }

    #[test]
    fn spare_pixels_at_the_ends_of_rows_are_never_touched() {
        let mut p = vec![0u32; 8 * 3];
        let mut c = Canvas::new(&mut p, 5, 3, 8).unwrap();
        c.fill(INK);
        c.fill_rect(Rect::new(-100, -100, 1000, 1000), INK);
        assert_eq!(show(&p, 8, 8), ["#####...", "#####...", "#####..."]);
    }

    #[test]
    fn hostile_positions_and_sizes_draw_what_is_inside_and_never_panic() {
        let (max, min) = (i32::MAX, i32::MIN);
        for scale in [1, 2, 8] {
            let mut p = vec![0u32; 16 * 16];
            let mut c = Canvas::new(&mut p, 16, 16, 16).unwrap().with_scale(scale);
            for r in [
                Rect::new(max, max, max, max),
                Rect::new(min, min, 5, 5),
                Rect::new(max - 2, 0, max, 5),
                Rect::new(0, 0, min, min),
                Rect::new(min, 3, 10, max),
                Rect::new(7, min, max, 10),
            ] {
                c.fill_rect(r, INK);
                c.stroke_rect(r, 3, INK);
                c.stroke_rect(r, max, INK);
                c.stroke_rect(r, min, INK);
                c.sub(r).fill(INK);
                c.clipped(r).fill(INK);
                let _ = c.sub(r).sub(r).clip_bounds();
            }
            let _ = (c.pixel(max, max), c.pixel(min, min));
            assert!(p.iter().all(|&x| x == 0), "none of those reaches the canvas");
            // And ones that do reach it fill exactly what is inside.
            let mut c = Canvas::new(&mut p, 16, 16, 16).unwrap().with_scale(scale);
            c.fill_rect(Rect::new(min, min, max, max), INK);
            assert!(p.iter().all(|&x| x == 0), "it ends just short of (0, 0)");
            let mut c = Canvas::new(&mut p, 16, 16, 16).unwrap().with_scale(scale);
            c.fill_rect(Rect::new(-1, -1, max, max), INK);
            assert!(p.iter().all(|&x| x == INK.0));
        }
    }

    #[test]
    fn see_through_colours_mix_and_the_spare_byte_becomes_255() {
        let mut p = vec![0x00FF_FFFFu32; 4];
        let mut c = Canvas::new(&mut p, 2, 2, 2).unwrap();
        c.fill_rect(Rect::new(0, 0, 1, 1), Colour(0x8000_0000));
        c.fill_rect(Rect::new(1, 0, 1, 1), Colour(0x0012_3456)); // clear: nothing at all
        c.fill_rect(Rect::new(0, 1, 1, 1), Colour::solid(0x123456));
        assert_eq!(p, [0xFF7F_7F7F, 0x00FF_FFFF, 0xFF12_3456, 0x00FF_FFFF]);
    }

    #[test]
    fn the_scale_multiplies_positions_and_sizes() {
        let mut p = vec![0u32; 8 * 6];
        let mut c = Canvas::new(&mut p, 8, 6, 8).unwrap().with_scale(2);
        assert_eq!((c.scale(), c.device_size()), (2, (8, 6)));
        assert_eq!(c.clip_bounds(), Rect::new(0, 0, 4, 3));
        c.fill_rect(Rect::new(1, 1, 2, 1), INK);
        assert_eq!(c.pixel(1, 1), Some(INK));
        assert_eq!(c.pixel(3, 1), Some(Colour::solid(0)));
        assert_eq!(c.pixel(4, 0), None);
        let rows = show(&p, 8, 8);
        assert_eq!(rows, ["........", "........", "..####..", "..####..", "........", "........"]);
        // Scales outside 1 to MAX_SCALE are brought inside.
        let c = Canvas::new(&mut p, 8, 6, 8).unwrap();
        assert_eq!(c.with_scale(0).scale(), 1);
        let c = Canvas::new(&mut p, 8, 6, 8).unwrap();
        assert_eq!(c.with_scale(u32::MAX).scale(), MAX_SCALE);
        // A canvas that isn't a whole number of logical pixels: the last one is partly there.
        let mut q = vec![0u32; 5 * 5];
        let mut c = Canvas::new(&mut q, 5, 5, 5).unwrap().with_scale(2);
        assert_eq!(c.clip_bounds(), Rect::new(0, 0, 3, 3));
        c.fill_rect(Rect::new(2, 2, 1, 1), INK);
        assert_eq!(show(&q, 5, 5)[4], "....#");
    }

    #[test]
    fn stroke_rect_draws_a_frame_and_each_pixel_once() {
        let mut p = vec![0u32; 7 * 6];
        let mut c = Canvas::new(&mut p, 7, 6, 7).unwrap();
        c.stroke_rect(Rect::new(1, 1, 5, 4), 1, INK);
        assert_eq!(
            show(&p, 7, 7),
            [".......", ".#####.", ".#...#.", ".#...#.", ".#####.", "......."]
        );
        // With a see-through colour, no pixel is mixed twice: every one comes out the same.
        let mut p = vec![0xFFFF_FFFFu32; 9 * 9];
        let mut c = Canvas::new(&mut p, 9, 9, 9).unwrap();
        c.stroke_rect(Rect::new(0, 0, 9, 9), 2, Colour(0x8000_0000));
        assert!(p.iter().all(|&x| x == 0xFF7F_7F7F || x == 0xFFFF_FFFF));
        assert_eq!(p.iter().filter(|&&x| x == 0xFFFF_FFFF).count(), 25);
        // No hole left: it is filled, once.
        for thickness in [3, 4, 100, i32::MAX] {
            let mut p = vec![0xFFFF_FFFFu32; 36];
            let mut c = Canvas::new(&mut p, 6, 6, 6).unwrap();
            c.stroke_rect(Rect::new(0, 0, 6, 6), thickness, Colour(0x8000_0000));
            assert!(p.iter().all(|&x| x == 0xFF7F_7F7F), "{thickness}");
        }
        let mut p = vec![0u32; 36];
        let mut c = Canvas::new(&mut p, 6, 6, 6).unwrap();
        c.stroke_rect(Rect::new(0, 0, 6, 6), 0, INK);
        c.stroke_rect(Rect::new(0, 0, 6, 6), -1, INK);
        c.stroke_rect(Rect::new(0, 0, 0, 6), 1, INK);
        assert!(p.iter().all(|&x| x == 0));
        // At scale 2 the frame is twice as thick.
        let mut p = vec![0u32; 12 * 12];
        let mut c = Canvas::new(&mut p, 12, 12, 12).unwrap().with_scale(2);
        c.stroke_rect(Rect::new(0, 0, 6, 6), 1, INK);
        let rows = show(&p, 12, 12);
        assert_eq!(rows[1], "############");
        assert_eq!(rows[2], "##........##");
        assert_eq!(rows[10], "############");
    }

    #[test]
    fn a_sub_canvas_has_its_own_corner_and_cannot_draw_outside_its_area() {
        let mut p = vec![0u32; 8 * 6];
        let mut c = Canvas::new(&mut p, 8, 6, 8).unwrap();
        {
            let mut part = c.sub(Rect::new(2, 1, 4, 3));
            assert_eq!(part.clip_bounds(), Rect::new(0, 0, 4, 3));
            part.fill_rect(Rect::new(-1, -1, 3, 3), INK); // only its corner shows
            part.fill_rect(Rect::new(3, 2, 10, 10), INK);
            assert_eq!(part.pixel(0, 0), Some(INK));
            assert_eq!(part.pixel(-2, -1), Some(Colour::solid(0)), "looking isn't clipped");
            // A part of a part.
            let mut inner = part.sub(Rect::new(1, 2, 100, 100));
            assert_eq!(inner.clip_bounds(), Rect::new(0, 0, 3, 1));
            inner.fill(INK);
        }
        assert_eq!(
            show(&p, 8, 8),
            ["........", "..##....", "..##....", "...###..", "........", "........"]
        );
        // `clipped` keeps the corner where it was.
        let mut p = vec![0u32; 8 * 2];
        let mut c = Canvas::new(&mut p, 8, 2, 8).unwrap();
        let mut part = c.clipped(Rect::new(2, 0, 3, 1));
        assert_eq!(part.clip_bounds(), Rect::new(2, 0, 3, 1));
        part.fill_rect(Rect::new(0, 0, 4, 2), INK);
        assert_eq!(show(&p, 8, 8), ["..##....", "........"]);
        // An area outside the canvas leaves nothing to draw on.
        let mut c = Canvas::new(&mut p, 8, 2, 8).unwrap();
        let mut part = c.sub(Rect::new(8, 0, 4, 4));
        part.fill(INK);
        assert_eq!(part.clip_bounds(), Rect::EMPTY);
        assert_eq!(show(&p, 8, 8), ["..##....", "........"]);
    }

    #[test]
    fn sub_canvases_at_scale_two() {
        let mut p = vec![0u32; 8 * 4];
        let mut c = Canvas::new(&mut p, 8, 4, 8).unwrap().with_scale(2);
        let mut part = c.sub(Rect::new(1, 1, 2, 1));
        assert_eq!(part.scale(), 2);
        part.fill_rect(Rect::new(1, 0, 5, 5), INK);
        assert_eq!(show(&p, 8, 8), ["........", "........", "....##..", "....##.."]);
    }

    #[test]
    fn damage_is_recorded_in_device_pixels_and_only_inside_the_clip() {
        let mut p = vec![0u32; 100 * 100];
        let mut damage = Damage::new();
        {
            let mut c =
                Canvas::new(&mut p, 100, 100, 100).unwrap().with_scale(2).with_damage(&mut damage);
            c.fill_rect(Rect::new(40, 40, 100, 5), INK); // cut at the right edge
            c.fill_rect(Rect::new(200, 200, 5, 5), INK); // outside: no damage
            c.fill_rect(Rect::new(0, 0, 5, 5), Colour::CLEAR); // clear: nothing changed
            let mut part = c.sub(Rect::new(0, 0, 3, 3));
            part.fill(INK); // sub-canvases record into the same list
        }
        let mut rects = damage.rects().to_vec();
        rects.sort_by_key(|r| r.x);
        assert_eq!(rects, [Rect::new(0, 0, 6, 6), Rect::new(80, 80, 20, 10)]);
        // A frame's four bars stay four rectangles, so the middle isn't shown again.
        let mut damage = Damage::new();
        let mut c = Canvas::new(&mut p, 100, 100, 100).unwrap().with_damage(&mut damage);
        c.stroke_rect(Rect::new(0, 0, 100, 100), 2, INK);
        assert_eq!(damage.rects().len(), 4);
        assert_eq!(damage.bounds(), Rect::new(0, 0, 100, 100));
    }

    #[test]
    fn what_the_library_draws_with_obeys_the_clip() {
        let mut p = vec![0u32; 6 * 3];
        let mut c = Canvas::new(&mut p, 6, 3, 6).unwrap();
        let mut part = c.clipped(Rect::new(1, 1, 4, 1));
        assert_eq!(part.device_clip(), Rect::new(1, 1, 4, 1));
        for y in -2..5 {
            part.span(y, -100, 100, INK);
            part.span(y, 3, 3, INK);
            part.span(y, 4, 2, INK);
        }
        part.span(1, i32::MIN, i32::MAX, INK);
        assert_eq!(part.row_mut(1, 0, 3), (&mut [INK.0, INK.0][..], 1));
        assert_eq!(part.row_mut(1, 5, 9).0.len(), 0);
        assert_eq!(part.row_mut(0, 0, 9).0.len(), 0);
        assert_eq!(part.device_pixel(1, 1), Some(INK.0));
        assert_eq!(part.device_pixel(0, 1), None);
        assert_eq!(part.device_pixel(1, 0), None);
        assert_eq!(show(&p, 6, 6), ["......", ".####.", "......"]);
        // The conversions are cut to the limit, not wrapped.
        let c = Canvas::new(&mut p, 6, 3, 6).unwrap().with_scale(8);
        assert_eq!(c.device_x(i32::MAX), DEVICE_LIMIT as i32);
        assert_eq!(c.device_y(i32::MIN), -DEVICE_LIMIT as i32);
        assert_eq!(c.device_len(i32::MIN), -DEVICE_LIMIT as i32);
        assert_eq!(c.device_len(3), 24);
        let all = c.to_device(Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX));
        assert_eq!((all.x, all.right()), (-DEVICE_LIMIT as i32, -8));
    }
}
