// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drawing pictures: against a slow model written straight from the rules, against known
//! answers, clipped, damaged, at hostile sizes and in bounded time, and against reference
//! pictures at 1× and 2×.
//!
//! The model works on one destination pixel at a time and, for smooth scaling, looks at every
//! source pixel for every destination pixel, working the overlap out from the definition of
//! the intervals. It shares no code with the library.

mod common;

use common::{PAPER, Sheet};
use pbr_draw::{Canvas, Colour, Damage, MAX_SIDE, Picture, Rect, Turn};
use std::time::Instant;

const MAX: i32 = i32::MAX;
const MIN: i32 = i32::MIN;
const TURNS: [Turn; 4] = [Turn::None, Turn::Right, Turn::Half, Turn::Left];

/// A small random number generator, the same one `damage.rs` uses in its tests.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> i32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n) as i32
    }

    fn pixel(&mut self) -> u32 {
        (self.below(256) as u32) << 24
            | (self.below(256) as u32) << 16
            | (self.below(256) as u32) << 8
            | self.below(256) as u32
    }
}

/// A picture as plain numbers, for the model: the pixels packed with no spare at the row ends.
#[derive(Clone)]
struct Src {
    w: usize,
    h: usize,
    px: Vec<u32>,
    alpha: bool,
}

impl Src {
    /// Random pixels. With alpha, a good share of them are clear or solid.
    fn random(rng: &mut Rng, w: usize, h: usize, alpha: bool) -> Src {
        let px = (0..w * h)
            .map(|_| {
                let p = rng.pixel();
                if !alpha {
                    return p;
                }
                match rng.below(4) {
                    0 => p & 0x00FF_FFFF,
                    1 => p | 0xFF00_0000,
                    _ => p,
                }
            })
            .collect();
        Src { w, h, px, alpha }
    }

    fn at(&self, x: usize, y: usize) -> u32 {
        self.px[y * self.w + x]
    }

    /// The picture after a turn, made pixel by pixel from the rules in the brief.
    fn turned(&self, turn: Turn) -> Src {
        let (w, h) = (self.w, self.h);
        let (tw, th) = if matches!(turn, Turn::Right | Turn::Left) { (h, w) } else { (w, h) };
        let mut px = vec![0; w * h];
        for sy in 0..h {
            for sx in 0..w {
                let (x, y) = match turn {
                    Turn::None => (sx, sy),
                    Turn::Right => (h - 1 - sy, sx),
                    Turn::Half => (w - 1 - sx, h - 1 - sy),
                    Turn::Left => (sy, w - 1 - sx),
                };
                px[y * tw + x] = self.at(sx, sy);
            }
        }
        Src { w: tw, h: th, px, alpha: self.alpha }
    }
}

/// How far the intervals [a0, a1) and [b0, b1) overlap.
fn overlap(a0: i64, a1: i64, b0: i64, b1: i64) -> i64 {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// What the rules say destination pixel (i, j) of a `dw` × `dh` destination shows: a colour
/// to draw over what is there, or `None` for nothing.
fn model_pixel(src: &Src, i: i64, j: i64, dw: i64, dh: i64) -> Option<Colour> {
    let (sw, sh) = (src.w as i64, src.h as i64);
    if dw % sw == 0 && dh % sh == 0 {
        let p = src.at((i / (dw / sw)) as usize, (j / (dh / sh)) as usize);
        return Some(if src.alpha { Colour(p) } else { Colour::solid(p) });
    }
    let n = sw * sh;
    // S: the total weight, each counted as many times as it is solid.
    let (mut s, mut sr, mut sg, mut sb) = (0i64, 0i64, 0i64, 0i64);
    for l in 0..sh {
        // Source row l covers l·dh to (l+1)·dh; destination row j covers j·sh to (j+1)·sh.
        let wy = overlap(l * dh, (l + 1) * dh, j * sh, (j + 1) * sh);
        for k in 0..sw {
            let wx = overlap(k * dw, (k + 1) * dw, i * sw, (i + 1) * sw);
            let p = src.at(k as usize, l as usize);
            let w = wx * wy * if src.alpha { i64::from(p >> 24) } else { 1 };
            s += w;
            sr += w * i64::from(p >> 16 & 0xFF);
            sg += w * i64::from(p >> 8 & 0xFF);
            sb += w * i64::from(p & 0xFF);
        }
    }
    if s == 0 {
        return None;
    }
    let alpha = if src.alpha { (s + n / 2) / n } else { 255 };
    let channel = |sum: i64| ((sum + s / 2) / s) as u32;
    Some(Colour((alpha as u32) << 24 | channel(sr) << 16 | channel(sg) << 8 | channel(sb)))
}

/// Draws `src` into `dest` (logical) on the pixels `bg` (`cw` × `ch` device pixels, scale
/// `scale`), one pixel at a time.
fn model_fit(bg: &mut [u32], cw: i32, ch: i32, scale: i32, src: &Src, dest: Rect) {
    if dest.is_empty() || src.w == 0 || src.h == 0 {
        return;
    }
    let (x0, y0) = (dest.x * scale, dest.y * scale);
    let (dw, dh) = (dest.width * scale, dest.height * scale);
    for py in 0..ch {
        for px in 0..cw {
            if px < x0 || px >= x0 + dw || py < y0 || py >= y0 + dh {
                continue;
            }
            let under = &mut bg[(py * cw + px) as usize];
            if let Some(c) =
                model_pixel(src, (px - x0).into(), (py - y0).into(), dw.into(), dh.into())
            {
                *under = c.over(*under);
            }
        }
    }
}

/// Solid pixels, all different, to draw on.
fn background(w: i32, h: i32) -> Vec<u32> {
    (0..w * h)
        .map(|n| {
            let (x, y) = ((n % w) as u32, (n / w) as u32);
            0xFF00_0000
                | (((x * 5 + y * 3) & 0xFF) << 16)
                | (((x * 7 + 40) & 0xFF) << 8)
                | ((y * 11) & 0xFF)
        })
        .collect()
}

/// Runs `f` with `src` as a [`Picture`], held in memory with `pad` spare pixels at the end of
/// each row (filled with rubbish, which must never show).
fn with_picture<R>(src: &Src, pad: usize, f: impl FnOnce(&Picture) -> R) -> R {
    let stride = src.w + pad;
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut mem: Vec<u32> = (0..stride * src.h).map(|_| rng.pixel()).collect();
    for y in 0..src.h {
        mem[y * stride..y * stride + src.w].copy_from_slice(&src.px[y * src.w..(y + 1) * src.w]);
    }
    let mut picture = Picture::new(&mem, src.w as u32, src.h as u32, stride as u32).unwrap();
    if src.alpha {
        picture = picture.with_alpha();
    }
    f(&picture)
}

/// The same drawing as `model_fit` or the library, as a closure over a canvas of `cw` × `ch`
/// device pixels at `scale`, from `background`.
fn draw(cw: i32, ch: i32, scale: u32, f: impl FnOnce(&mut Canvas)) -> Vec<u32> {
    let mut pixels = background(cw, ch);
    let canvas = Canvas::new(&mut pixels, cw as u32, ch as u32, cw as u32).unwrap();
    f(&mut canvas.with_scale(scale));
    pixels
}

fn assert_same(got: &[u32], want: &[u32], width: i32, what: &str) {
    if let Some(at) = got.iter().zip(want).position(|(g, w)| g != w) {
        let (x, y) = (at as i32 % width, at as i32 / width);
        panic!("{what}: at {x},{y} got {:#010x}, the model says {:#010x}", got[at], want[at]);
    }
}

#[test]
fn a_picture_needs_enough_pixels() {
    let p = vec![0u32; 100];
    assert!(Picture::new(&p, 10, 10, 10).is_some());
    assert!(Picture::new(&p, 10, 11, 10).is_none());
    assert!(Picture::new(&p, 11, 10, 10).is_none(), "the stride is less than the width");
    assert!(Picture::new(&p, 4, 9, 12).is_some(), "the last row needn't have a whole stride");
    assert!(Picture::new(&p, 5, 9, 12).is_none());
    assert!(Picture::new(&p, 100, 1, u32::MAX).is_some(), "one row: any stride");
    assert!(Picture::new(&p, 1, 2, u32::MAX).is_none());
    assert!(Picture::new(&p, MAX_SIDE + 1, 0, MAX_SIDE + 1).is_none());
    assert!(Picture::new(&p, 0, MAX_SIDE + 1, 0).is_none());
    assert!(Picture::new(&p, u32::MAX, u32::MAX, u32::MAX).is_none());
    // Empty pictures are allowed, and draw nothing.
    for (w, h) in [(0, 0), (0, 10), (10, 0)] {
        let empty = Picture::new(&p, w, h, 10).unwrap();
        assert_eq!((empty.width(), empty.height()), (w, h));
        assert_eq!(empty.pixel(0, 0), None);
        let mut canvas = vec![7u32; 100];
        let mut c = Canvas::new(&mut canvas, 10, 10, 10).unwrap();
        c.picture(&empty, 0, 0);
        c.picture_fit(&empty, Rect::new(0, 0, 10, 10));
        for turn in TURNS {
            c.picture_turned(&empty, 0, 0, turn);
        }
        assert!(canvas.iter().all(|&x| x == 7));
    }
    let none: [u32; 0] = [];
    assert!(Picture::new(&none, 0, 0, 0).is_some());
    assert!(Picture::new(&none, 1, 1, 1).is_none());
    // A picture with a side of MAX_SIDE is allowed if the pixels are there.
    let long = vec![0u32; MAX_SIDE as usize];
    assert!(Picture::new(&long, MAX_SIDE, 1, MAX_SIDE).is_some());
}

#[test]
fn looking_at_a_pixel() {
    // 3 × 2 with 2 spare pixels at the end of each row.
    let mem = [
        0x0011_1111,
        0x0022_2222,
        0x0033_3333,
        0xAAAA,
        0xBBBB,
        0x8044_4444,
        0x00FF_5555,
        0x0066_6666,
    ];
    let solid = Picture::new(&mem, 3, 2, 5).unwrap();
    assert_eq!(solid.pixel(0, 0), Some(Colour::solid(0x111111)));
    assert_eq!(solid.pixel(2, 0), Some(Colour::solid(0x333333)));
    assert_eq!(solid.pixel(0, 1), Some(Colour::solid(0x444444)), "the top byte is ignored");
    assert_eq!(solid.pixel(2, 1), Some(Colour::solid(0x666666)));
    for (x, y) in [(3, 0), (0, 2), (-1, 0), (0, -1), (MAX, MAX), (MIN, MIN), (3, 1)] {
        assert_eq!(solid.pixel(x, y), None, "{x},{y}");
    }
    let alpha = solid.with_alpha();
    assert_eq!(alpha.pixel(0, 0), Some(Colour(0x0011_1111)));
    assert_eq!(alpha.pixel(0, 1), Some(Colour(0x8044_4444)));
    assert_eq!(alpha.pixel(1, 1), Some(Colour(0x00FF_5555)));
    assert_eq!(solid.pixel(0, 0), Some(Colour::solid(0x111111)), "the original is unchanged");
}

#[test]
fn a_part_of_a_picture() {
    // 6 × 4, each pixel's value its place: 0xYX.
    let mem: Vec<u32> = (0..4u32).flat_map(|y| (0..6u32).map(move |x| y << 4 | x)).collect();
    let whole = Picture::new(&mem, 6, 4, 6).unwrap();
    let size = |p: &Picture| (p.width(), p.height());
    let value = |p: &Picture, x, y| p.pixel(x, y).map(|c| c.0 & 0xFF);

    let inside = whole.part(Rect::new(2, 1, 3, 2));
    assert_eq!(size(&inside), (3, 2));
    assert_eq!(value(&inside, 0, 0), Some(0x12));
    assert_eq!(value(&inside, 2, 1), Some(0x24));
    assert_eq!(value(&inside, 3, 0), None);
    // Partly outside each edge: only the part inside is kept.
    let left = whole.part(Rect::new(-3, 0, 5, 4));
    assert_eq!(
        (size(&left), value(&left, 0, 0), value(&left, 1, 3)),
        ((2, 4), Some(0), Some(0x31))
    );
    let right = whole.part(Rect::new(4, 0, 10, 4));
    assert_eq!(
        (size(&right), value(&right, 0, 0), value(&right, 1, 3)),
        ((2, 4), Some(4), Some(0x35))
    );
    let top = whole.part(Rect::new(0, -2, 6, 3));
    assert_eq!((size(&top), value(&top, 5, 0)), ((6, 1), Some(5)));
    let bottom = whole.part(Rect::new(0, 3, 6, 100));
    assert_eq!((size(&bottom), value(&bottom, 0, 0)), ((6, 1), Some(0x30)));
    let all = whole.part(Rect::new(MIN, MIN, MAX, MAX));
    assert_eq!(size(&all), (0, 0), "that ends just short of the picture");
    let everything = whole.part(Rect::new(-1, -1, MAX, MAX));
    assert_eq!(size(&everything), (6, 4));
    // Wholly outside, empty, and hostile.
    for area in [
        Rect::new(6, 0, 3, 3),
        Rect::new(0, 4, 3, 3),
        Rect::new(-9, -9, 9, 9),
        Rect::new(2, 2, 0, 3),
        Rect::new(2, 2, 3, -3),
        Rect::EMPTY,
        Rect::new(MAX, MAX, MAX, MAX),
        Rect::new(MIN, 0, 5, 5),
        Rect::new(3, 3, MIN, MIN),
    ] {
        let none = whole.part(area);
        assert_eq!(size(&none), (0, 0), "{area:?}");
        assert_eq!(none.pixel(0, 0), None);
    }
    // A part of a part.
    let inner = inside.part(Rect::new(1, 1, 5, 5));
    assert_eq!(size(&inner), (2, 1));
    assert_eq!((value(&inner, 0, 0), value(&inner, 1, 0)), (Some(0x23), Some(0x24)));
    assert_eq!(size(&inner.part(Rect::new(1, 0, 1, 1))), (1, 1));
    assert_eq!(size(&inner.part(Rect::new(2, 0, 1, 1))), (0, 0));
    // The part keeps the alpha, and keeps the picture's stride (shown here by drawing it).
    let wide: Vec<u32> =
        (0..3u32).flat_map(|y| (0..8u32).map(move |x| 0x8000_0000 | y << 4 | x)).collect();
    let wide = Picture::new(&wide, 5, 3, 8).unwrap().with_alpha();
    let part = wide.part(Rect::new(1, 1, 3, 2));
    assert_eq!(part.pixel(0, 0), Some(Colour(0x8000_0011)));
    assert_eq!(part.pixel(2, 1), Some(Colour(0x8000_0023)));
    let mut canvas = vec![0xFF00_0000u32; 9];
    Canvas::new(&mut canvas, 3, 3, 3).unwrap().picture(&part, 0, 0);
    assert_eq!(canvas[0], Colour(0x8000_0011).over(0xFF00_0000));
    assert_eq!(canvas[5], Colour(0x8000_0023).over(0xFF00_0000));
    assert_eq!(canvas[6], 0xFF00_0000, "only two rows");
}

/// A part of a picture draws as that part would have, taken out and drawn alone.
#[test]
fn drawing_a_part_draws_just_that_part() {
    let mut rng = Rng(77);
    let src = Src::random(&mut rng, 7, 5, true);
    with_picture(&src, 3, |picture| {
        for area in [Rect::new(1, 1, 4, 3), Rect::new(-2, 2, 5, 9), Rect::new(3, 0, 1, 5)] {
            let (x0, y0) = (area.x.max(0) as usize, area.y.max(0) as usize);
            let x1 = ((area.x + area.width) as usize).min(7);
            let y1 = ((area.y + area.height) as usize).min(5);
            let mut px = Vec::new();
            for y in y0..y1 {
                px.extend_from_slice(&src.px[y * 7 + x0..y * 7 + x1]);
            }
            let cut = Src { w: x1 - x0, h: y1 - y0, px, alpha: true };
            let got =
                draw(30, 30, 1, |c| c.picture_fit(&picture.part(area), Rect::new(3, 4, 9, 7)));
            let mut want = background(30, 30);
            model_fit(&mut want, 30, 30, 1, &cut, Rect::new(3, 4, 9, 7));
            assert_same(&got, &want, 30, &format!("{area:?}"));
        }
    });
}

#[test]
fn every_way_of_stretching_agrees_with_the_model() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let source_sizes = [1usize, 2, 3, 5, 7, 12];
    let dest_sizes = [1i32, 2, 3, 4, 6, 7, 9, 12];
    let (cw, ch) = (48, 48);
    let mut cases = 0;
    for &sw in &source_sizes {
        for &sh in &source_sizes {
            for &dw in &dest_sizes {
                for &dh in &dest_sizes {
                    for alpha in [false, true] {
                        let scale = 1 + (cases % 3) as u32;
                        let src = Src::random(&mut rng, sw, sh, alpha);
                        // Partly off each edge, sometimes, and sometimes well inside.
                        let (x, y) = (rng.below(30) - 10, rng.below(30) - 10);
                        let dest = Rect::new(x, y, dw, dh);
                        let got = with_picture(&src, cases % 3, |p| {
                            draw(cw, ch, scale, |c| c.picture_fit(p, dest))
                        });
                        let mut want = background(cw, ch);
                        model_fit(&mut want, cw, ch, scale as i32, &src, dest);
                        let what = format!(
                            "{sw}x{sh} to {dw}x{dh} at {x},{y} scale {scale} alpha {alpha}"
                        );
                        assert_same(&got, &want, cw, &what);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 6 * 6 * 8 * 8 * 2);
}

#[test]
fn plain_pictures_and_turned_ones_agree_with_the_model() {
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let (cw, ch) = (48, 48);
    let mut cases = 0;
    for w in 1..=6usize {
        for h in 1..=6usize {
            for turn in TURNS {
                for alpha in [false, true] {
                    let scale = 1 + (cases % 3) as u32;
                    let src = Src::random(&mut rng, w, h, alpha);
                    let (x, y) = (rng.below(30) - 8, rng.below(30) - 8);
                    let turned = src.turned(turn);
                    let dest = Rect::new(x, y, turned.w as i32, turned.h as i32);
                    let mut want = background(cw, ch);
                    model_fit(&mut want, cw, ch, scale as i32, &turned, dest);
                    let what = format!("{w}x{h} {turn:?} at {x},{y} scale {scale} alpha {alpha}");
                    let got = with_picture(&src, cases % 2, |p| {
                        draw(cw, ch, scale, |c| c.picture_turned(p, x, y, turn))
                    });
                    assert_same(&got, &want, cw, &what);
                    if turn == Turn::None {
                        let got =
                            with_picture(&src, 1, |p| draw(cw, ch, scale, |c| c.picture(p, x, y)));
                        assert_same(&got, &want, cw, &format!("picture, {what}"));
                    }
                    cases += 1;
                }
            }
        }
    }
}

#[test]
fn a_picture_at_scale_1_is_copied_and_at_scale_2_doubled() {
    let mut rng = Rng(5);
    let src = Src::random(&mut rng, 9, 6, false);
    let got = with_picture(&src, 0, |p| {
        let mut pixels = vec![0u32; 12 * 8];
        Canvas::new(&mut pixels, 12, 8, 12).unwrap().picture(p, 2, 1);
        pixels
    });
    for y in 0..8usize {
        for x in 0..12usize {
            let inside = (2..11).contains(&x) && (1..7).contains(&y);
            let want = if inside { 0xFF00_0000 | src.at(x - 2, y - 1) & 0xFFFFFF } else { 0 };
            assert_eq!(got[y * 12 + x], want, "{x},{y}");
        }
    }
    let got2 = with_picture(&src, 0, |p| {
        let mut pixels = vec![0u32; 24 * 16];
        Canvas::new(&mut pixels, 24, 16, 24).unwrap().with_scale(2).picture(p, 2, 1);
        pixels
    });
    for y in 0..16usize {
        for x in 0..24usize {
            assert_eq!(got2[y * 24 + x], got[y / 2 * 12 + x / 2], "{x},{y}");
        }
    }
}

#[test]
fn halving_a_checker_gives_grey_and_clear_pixels_do_not_leak_colour() {
    let checker = [0x0000_0000, 0x00FF_FFFF, 0x00FF_FFFF, 0x0000_0000];
    let mut pixels = [0u32; 1];
    let p = Picture::new(&checker, 2, 2, 2).unwrap();
    Canvas::new(&mut pixels, 1, 1, 1).unwrap().picture_fit(&p, Rect::new(0, 0, 1, 1));
    assert_eq!(pixels[0], 0xFF80_8080, "half of 255, rounded up");
    // A 4 × 4 checker of 2 × 2 blocks shrunk to 2 × 2 keeps the blocks.
    let big: Vec<u32> =
        (0..16u32).map(|n| if (n % 4 / 2 + n / 8) % 2 == 0 { 0 } else { 0xFFFFFF }).collect();
    let p = Picture::new(&big, 4, 4, 4).unwrap();
    let mut out = [0u32; 4];
    Canvas::new(&mut out, 2, 2, 2).unwrap().picture_fit(&p, Rect::new(0, 0, 2, 2));
    assert_eq!(out, [0xFF00_0000, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFF00_0000]);

    // A clear red pixel beside a solid blue one: the average is blue, half as solid.
    let pair = [0x00FF_0000, 0xFF00_00FF];
    let p = Picture::new(&pair, 2, 1, 2).unwrap().with_alpha();
    let mut out = [0xFFFF_FFFFu32];
    Canvas::new(&mut out, 1, 1, 1).unwrap().picture_fit(&p, Rect::new(0, 0, 1, 1));
    assert_eq!(out[0], Colour::argb(128, 0, 0, 255).over(0xFFFF_FFFF));
    // Without alpha the same two pixels are just averaged.
    let p = Picture::new(&pair, 2, 1, 2).unwrap();
    let mut out = [0u32];
    Canvas::new(&mut out, 1, 1, 1).unwrap().picture_fit(&p, Rect::new(0, 0, 1, 1));
    assert_eq!(out[0], 0xFF80_0080);
    // All clear: skipped, so the canvas is left exactly as it was.
    let clear = [0x00FF_0000u32; 6];
    let p = Picture::new(&clear, 3, 2, 3).unwrap().with_alpha();
    let mut out = [0xFFAB_CDEFu32; 2];
    Canvas::new(&mut out, 2, 1, 2).unwrap().picture_fit(&p, Rect::new(0, 0, 2, 1));
    assert_eq!(out, [0xFFAB_CDEF, 0xFFAB_CDEF]);
}

/// A canvas (a sheet) drawn on, made into a picture, drawn on another.
fn turn_through_a_sheet(src: &[u32], w: u32, h: u32, turn: Turn) -> (Vec<u32>, u32, u32) {
    let (tw, th) = if matches!(turn, Turn::Right | Turn::Left) { (h, w) } else { (w, h) };
    let mut sheet = Sheet::new(tw, th, Colour::BLACK);
    let picture = Picture::new(src, w, h, w).unwrap();
    sheet.canvas().picture_turned(&picture, 0, 0, turn);
    (sheet.pixels, tw, th)
}

#[test]
fn turns_undo_each_other() {
    let mut rng = Rng(99);
    let src: Vec<u32> = (0..7 * 4).map(|_| 0xFF00_0000 | rng.pixel()).collect();
    // Four quarter turns to the right (and to the left) give the original back.
    for turn in [Turn::Right, Turn::Left] {
        let (mut pixels, mut w, mut h) = (src.clone(), 7, 4);
        for _ in 0..4 {
            (pixels, w, h) = turn_through_a_sheet(&pixels, w, h, turn);
        }
        assert_eq!((w, h, &pixels), (7, 4, &src), "{turn:?}");
    }
    // Left undoes Right, and the other way round.
    for (first, second) in [(Turn::Right, Turn::Left), (Turn::Left, Turn::Right)] {
        let (a, w, h) = turn_through_a_sheet(&src, 7, 4, first);
        assert_eq!((w, h), (4, 7));
        let (b, w, h) = turn_through_a_sheet(&a, w, h, second);
        assert_eq!((w, h, &b), (7, 4, &src));
    }
    // Half twice, and Half is two quarter turns.
    let (a, w, h) = turn_through_a_sheet(&src, 7, 4, Turn::Half);
    assert_ne!(a, src);
    let (b, ..) = turn_through_a_sheet(&a, w, h, Turn::Half);
    assert_eq!(b, src);
    let (r, w, h) = turn_through_a_sheet(&src, 7, 4, Turn::Right);
    let (rr, ..) = turn_through_a_sheet(&r, w, h, Turn::Right);
    assert_eq!(rr, a);
    // The corners: the top left pixel goes to the top right, bottom right, bottom left.
    let (right, ..) = turn_through_a_sheet(&src, 7, 4, Turn::Right);
    assert_eq!(right[3], src[0]);
    let (left, ..) = turn_through_a_sheet(&src, 7, 4, Turn::Left);
    assert_eq!(left[6 * 4], src[0]);
    assert_eq!(a[7 * 4 - 1], src[0]);
}

/// Some drawing calls of every kind, for the tests of clipping and damage. 60 × 60 device
/// pixels at either scale.
fn scene(c: &mut Canvas, a: &Picture, b: &Picture) {
    c.picture(a, 4, 3);
    c.picture_fit(a, Rect::new(10, 12, 16, 11)); // smooth
    c.picture_fit(b, Rect::new(2, 20, 21, 12)); // see-through, smooth, over the last
    c.picture_fit(b, Rect::new(13, 2, 18, 12)); // whole-number factors
    c.picture_turned(b, 6, 6, Turn::Right);
    c.picture_turned(a, 14, 18, Turn::Left);
    c.picture_turned(b, 3, 3, Turn::Half);
    c.picture_fit(a, Rect::new(-6, -6, 8, 8));
    c.picture_fit(b, Rect::new(20, 20, 40, 40));
}

#[test]
fn clipping_and_sub_canvases_never_change_a_pixel() {
    let mut rng = Rng(31337);
    let a = Src::random(&mut rng, 8, 6, false);
    let b = Src::random(&mut rng, 6, 4, true);
    for scale in [1u32, 2] {
        let s = scale as i32;
        let whole = with_picture(&a, 2, |pa| {
            with_picture(&b, 1, |pb| draw(60, 60, scale, |c| scene(c, pa, pb)))
        });
        let bg = background(60, 60);
        for clip in [
            Rect::new(0, 0, 100, 100),
            Rect::new(5, 5, 11, 9),
            Rect::new(0, 0, 1, 60),
            Rect::new(29, 0, 1, 60),
            Rect::new(0, 17, 60, 1),
            Rect::new(-9, -9, 20, 20),
            Rect::new(12, 10, 500, 500),
            Rect::new(7, 7, 0, 5),
            Rect::new(MAX, MAX, 3, 3),
        ] {
            let cut = with_picture(&a, 2, |pa| {
                with_picture(&b, 1, |pb| {
                    draw(60, 60, scale, |c| scene(&mut c.clipped(clip), pa, pb))
                })
            });
            for y in 0..60i32 {
                for x in 0..60i32 {
                    let (x64, y64, s64) = (i64::from(x), i64::from(y), i64::from(s));
                    let inside = x64 >= i64::from(clip.x) * s64
                        && y64 >= i64::from(clip.y) * s64
                        && x64 < clip.right() * s64
                        && y64 < clip.bottom() * s64
                        && !clip.is_empty();
                    let at = (y * 60 + x) as usize;
                    let want = if inside { whole[at] } else { bg[at] };
                    assert_eq!(cut[at], want, "{clip:?} scale {scale}: {x},{y}");
                }
            }
        }
        // A sub-canvas: its corner is the area's corner.
        let area = Rect::new(4, 5, 17, 13);
        let moved = with_picture(&b, 0, |pb| {
            draw(60, 60, scale, |c| {
                c.sub(area).picture_fit(pb, Rect::new(-2, 1, 11, 9));
                c.sub(area).picture_turned(pb, 3, 2, Turn::Right);
            })
        });
        let direct = with_picture(&b, 0, |pb| {
            draw(60, 60, scale, |c| {
                c.clipped(area).picture_fit(pb, Rect::new(2, 6, 11, 9));
                c.clipped(area).picture_turned(pb, 7, 7, Turn::Right);
            })
        });
        assert_eq!(moved, direct, "scale {scale}");
        assert_ne!(moved, bg);
    }
}

#[test]
fn damage_covers_every_changed_pixel_and_a_call_that_draws_nothing_records_nothing() {
    let mut rng = Rng(4242);
    let a = Src::random(&mut rng, 8, 6, false);
    let b = Src::random(&mut rng, 6, 4, true);
    for scale in [1u32, 2, 3] {
        let (w, h) = (60u32, 60u32);
        let bg = background(60, 60);
        with_picture(&a, 0, |pa| {
            with_picture(&b, 0, |pb| {
                type Call<'a> = Box<dyn Fn(&mut Canvas) + 'a>;
                let calls: Vec<Call> = vec![
                    Box::new(|c| c.picture(pa, 3, 4)),
                    Box::new(|c| c.picture(pb, -2, -1)),
                    Box::new(|c| c.picture_fit(pa, Rect::new(5, 5, 7, 9))),
                    Box::new(|c| c.picture_fit(pb, Rect::new(5, 5, 24, 8))),
                    Box::new(|c| c.picture_turned(pb, 9, 1, Turn::Left)),
                    Box::new(|c| c.picture_turned(pa, 2, 2, Turn::Half)),
                    Box::new(|c| c.picture_fit(pa, Rect::new(-50, -50, 5000, 5000))),
                    Box::new(|c| c.picture_fit(pb, Rect::new(MIN, MIN, MAX, MAX))),
                ];
                for (n, call) in calls.iter().enumerate() {
                    let mut pixels = bg.clone();
                    let mut damage = Damage::new();
                    {
                        let mut canvas = Canvas::new(&mut pixels, w, h, w)
                            .unwrap()
                            .with_scale(scale)
                            .with_damage(&mut damage);
                        call(&mut canvas);
                    }
                    let mut changed = 0;
                    for (at, (g, b)) in pixels.iter().zip(&bg).enumerate() {
                        if g != b {
                            changed += 1;
                            let (x, y) = ((at % 60) as i32, (at / 60) as i32);
                            assert!(
                                damage.rects().iter().any(|r| r.contains(x, y)),
                                "call {n} scale {scale}: {x},{y} changed outside the damage"
                            );
                        }
                    }
                    assert!(damage.bounds().area() <= 3600);
                    assert!(changed > 0 || n == 7, "call {n} should change something");
                }
            })
        });
        // Nothing drawn, nothing recorded.
        let empty = Picture::new(&[], 0, 0, 0).unwrap();
        let one = [0xFF11_2233u32];
        let pic = Picture::new(&one, 1, 1, 1).unwrap();
        let mut pixels = bg.clone();
        let mut damage = Damage::new();
        {
            let mut canvas = Canvas::new(&mut pixels, w, h, w)
                .unwrap()
                .with_scale(scale)
                .with_damage(&mut damage);
            canvas.picture(&empty, 0, 0);
            canvas.picture_fit(&empty, Rect::new(0, 0, 50, 50));
            canvas.picture_turned(&empty, 0, 0, Turn::Right);
            canvas.picture_fit(&pic, Rect::EMPTY);
            canvas.picture_fit(&pic, Rect::new(0, 0, -5, 5));
            canvas.picture_fit(&pic, Rect::new(0, 0, 5, 0));
            canvas.picture(&pic, 100, 0); // wholly outside
            canvas.picture(&pic, 0, -3);
            canvas.picture(&pic, MAX, MAX);
            canvas.picture_turned(&pic, MIN, MIN, Turn::Left);
            canvas.clipped(Rect::EMPTY).picture_fit(&pic, Rect::new(0, 0, 50, 50));
            canvas.clipped(Rect::new(70, 70, 5, 5)).picture_fit(&pic, Rect::new(0, 0, 50, 50));
        }
        assert!(damage.is_empty(), "{:?}", damage.rects());
        assert_eq!(pixels, bg);
    }
}

#[test]
fn a_canvas_used_as_a_picture_afterwards_is_solid() {
    let mut rng = Rng(8);
    let b = Src::random(&mut rng, 7, 5, true);
    let mut pixels = vec![0xFFAB_CDEFu32; 20 * 20];
    with_picture(&b, 0, |p| {
        let mut canvas = Canvas::new(&mut pixels, 20, 20, 20).unwrap();
        canvas.picture(p, 1, 1);
        canvas.picture_fit(p, Rect::new(5, 5, 11, 13));
        canvas.picture_turned(p, 12, 2, Turn::Right);
    });
    assert!(pixels.iter().all(|p| p >> 24 == 0xFF));
    let again = Picture::new(&pixels, 20, 20, 20).unwrap().with_alpha();
    assert!((0..20).all(|y| (0..20).all(|x| again.pixel(x, y).unwrap().alpha() == 255)));
}

#[test]
fn hostile_arguments_never_panic_and_never_leave_the_canvas() {
    let mut rng = Rng(1);
    let src = Src::random(&mut rng, 9, 7, false);
    let see = Src::random(&mut rng, 5, 3, true);
    let long = Src::random(&mut rng, 16384, 1, true);
    let tall = Src::random(&mut rng, 1, 16384, false);
    // A canvas in the middle of a larger sheet of pixels, with spare pixels at the row ends.
    const SIDE: usize = 100;
    for scale in [1u32, 2, 8] {
        let mut mem = vec![0xDEAD_BEEFu32; SIDE * SIDE];
        let start = 10 * SIDE + 10;
        {
            let mut canvas =
                Canvas::new(&mut mem[start..], 64, 64, SIDE as u32).unwrap().with_scale(scale);
            let places = [MIN, MIN + 1, -1, 0, 1, 7, MAX - 1, MAX];
            let sizes = [MIN, -1, 0, 1, 3, MAX, 20_000_000];
            for pic in [&src, &see, &long, &tall] {
                with_picture(pic, 0, |p| {
                    for &x in &places {
                        for &y in &places {
                            canvas.picture(p, x, y);
                            for turn in TURNS {
                                canvas.picture_turned(p, x, y, turn);
                            }
                        }
                    }
                    for &w in &sizes {
                        for &h in &sizes {
                            for &(x, y) in &[(0, 0), (-3, -3), (MIN, MIN), (MAX, 0), (-40, 5)] {
                                canvas.picture_fit(p, Rect::new(x, y, w, h));
                            }
                        }
                    }
                    canvas.picture_fit(p, Rect::new(-5, -5, 20_000_000, 20_000_000));
                    canvas.picture_fit(p, Rect::new(MIN, MIN, MAX, MAX));
                    let mut part = canvas.sub(Rect::new(MAX - 3, 2, MAX, 5));
                    part.picture_fit(p, Rect::new(MIN, MIN, MAX, MAX));
                });
            }
        }
        for (at, &pixel) in mem.iter().enumerate() {
            let (x, y) = (at % SIDE, at / SIDE);
            if (10..74).contains(&x) && (10..74).contains(&y) {
                assert_eq!(pixel >> 24, 0xFF, "scale {scale}: {x},{y} isn't solid");
            } else {
                assert_eq!(pixel, 0xDEAD_BEEF, "scale {scale}: {x},{y} outside the canvas");
            }
        }
    }
}

#[test]
fn work_is_bounded_by_the_clip() {
    let mut rng = Rng(2);
    let long = Src::random(&mut rng, 16384, 1, true);
    let long_solid = Src::random(&mut rng, 16384, 1, false);
    let tall = Src::random(&mut rng, 1, 16384, false);
    let big = Src::random(&mut rng, 500, 500, true);
    let mut pixels = background(64, 64);
    let start = Instant::now();
    let mut calls = 0;
    {
        let mut canvas = Canvas::new(&mut pixels, 64, 64, 64).unwrap().with_scale(8);
        for _ in 0..6 {
            for pic in [&long, &long_solid, &tall, &big] {
                with_picture(pic, 0, |p| {
                    canvas.picture_fit(p, Rect::new(0, 0, 1, 1));
                    canvas.picture_fit(p, Rect::new(0, 0, 3, 5));
                    canvas.picture_fit(p, Rect::new(-1, -1, 20_000_000, 20_000_000));
                    canvas.picture_fit(p, Rect::new(-3, 0, 20_000_000, 1));
                    canvas.picture_fit(p, Rect::new(0, -3, 1, 20_000_000));
                    canvas.picture_fit(p, Rect::new(MIN, MIN, MAX, MAX));
                    canvas.picture(p, -8000, -1);
                    canvas.picture_turned(p, 0, 0, Turn::Right);
                    canvas.picture_turned(p, -100, -1, Turn::Left);
                    canvas.picture_turned(p, -1, 0, Turn::Half);
                    calls += 10;
                });
            }
        }
    }
    let took = start.elapsed();
    assert!(calls >= 100);
    println!("{calls} hostile picture calls took {took:?}");
    assert!(took.as_secs_f64() < 5.0, "{took:?}");
}

// ---- Reference pictures ----

/// The icon's colours, by the numbers the icon is drawn with. 0 is see-through.
const ICON_COLOURS: [u32; 6] =
    [0x0000_0000, 0xFF5A_3B1A, 0xFFC4_8A2C, 0xFFF2_C96B, 0xFFFF_F0B8, 0x5000_0000];

/// A 16 × 16 folder, as numbers into [`ICON_COLOURS`]: 1 the outline, 2 the back of the
/// folder, 3 the front, 4 the light edge along the front's top, 5 a see-through shadow.
const ICON: [&str; 16] = [
    "................",
    "..11111.........",
    ".1222221........",
    "12222222111111..",
    "1444444444444441",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1333333333333331",
    "1111111111111111",
    ".55555555555555.",
];

fn icon() -> Vec<u32> {
    ICON.iter()
        .flat_map(|row| row.bytes().map(|b| ICON_COLOURS[usize::from(b.saturating_sub(b'0'))]))
        .collect()
}

/// A solid picture like a photograph: smooth colour changes in all directions, 64 × 48.
fn photograph() -> Vec<u32> {
    (0..48i32)
        .flat_map(|y| {
            (0..64i32).map(move |x| {
                let (dx, dy) = (x - 40, y - 20);
                let red = x * 255 / 63;
                let green = y * 255 / 47;
                let blue = (255 - (dx * dx + dy * dy) / 5).clamp(0, 255);
                0xFF00_0000 | (red as u32) << 16 | (green as u32) << 8 | blue as u32
            })
        })
        .collect()
}

/// A see-through picture: stripes of two colours fading in from the left, 32 × 32.
fn veil() -> Vec<u32> {
    (0..32u32)
        .flat_map(|y| {
            (0..32u32).map(move |x| {
                let colour = if y / 4 % 2 == 0 { 0x2f6b55 } else { 0xd2577a };
                (x * 255 / 31) << 24 | colour
            })
        })
        .collect()
}

/// The same calls at every scale: 264 × 176 logical pixels.
fn gallery(c: &mut Canvas) {
    let icon_pixels = icon();
    let folder = Picture::new(&icon_pixels, 16, 16, 16).unwrap().with_alpha();
    let photo_pixels = photograph();
    let photo = Picture::new(&photo_pixels, 64, 48, 64).unwrap();
    let veil_pixels = veil();
    let veil = Picture::new(&veil_pixels, 32, 32, 32).unwrap().with_alpha();

    // The folder plain, then at 2× and 3× (crisp blocks), then to sizes that are not whole
    // multiples (smooth), then shrunk.
    c.picture(&folder, 8, 8);
    c.picture_fit(&folder, Rect::new(32, 8, 32, 32));
    c.picture_fit(&folder, Rect::new(72, 8, 48, 48));
    c.picture_fit(&folder, Rect::new(128, 8, 24, 40));
    c.picture_fit(&folder, Rect::new(160, 8, 10, 10));
    c.picture_fit(&folder, Rect::new(176, 8, 8, 8));
    c.picture_fit(&folder, Rect::new(190, 8, 5, 5));
    // Turned each way, and a half-height part of it turned.
    c.picture_turned(&folder, 8, 64, Turn::Right);
    c.picture_turned(&folder, 28, 64, Turn::Half);
    c.picture_turned(&folder, 48, 64, Turn::Left);
    c.picture_turned(&folder.part(Rect::new(0, 0, 16, 8)), 70, 64, Turn::Right);
    c.picture_turned(&folder.part(Rect::new(0, 3, 12, 9)), 90, 64, Turn::Left);
    // A photograph, plain, shrunk and stretched.
    c.picture(&photo, 8, 96);
    c.picture_fit(&photo, Rect::new(80, 96, 29, 21));
    c.picture_fit(&photo, Rect::new(120, 96, 77, 55));
    c.picture_fit(&photo, Rect::new(80, 124, 8, 6));
    // See-through pictures over shapes.
    c.fill_rect(Rect::new(204, 64, 50, 50), Colour::solid(0x2a2e3d));
    c.fill_rect(Rect::new(226, 80, 34, 60), Colour::solid(0xd2577a));
    c.picture(&veil, 210, 70);
    c.picture(&folder, 232, 70);
    c.picture_fit(&folder, Rect::new(214, 120, 45, 45));
    c.picture_fit(&veil, Rect::new(206, 142, 20, 30));
}

#[test]
fn pictures_at_scale_1() {
    let mut sheet = Sheet::new(264, 176, PAPER);
    gallery(&mut sheet.canvas());
    sheet.check("picture-1x");
}

#[test]
fn pictures_at_scale_2() {
    let mut sheet = Sheet::new(528, 352, PAPER);
    gallery(&mut sheet.canvas().with_scale(2));
    sheet.check("picture-2x");
}
