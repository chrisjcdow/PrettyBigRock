// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The wallpaper: Table Mountain at dusk, from 1.3's `make_wallpaper` (`classic/kernel/wm.c`),
//! step for step.
//!
//! Everything is worked out as fractions of the screen's size, so the same picture comes out
//! at any size. The "random" stars and glints are not random: 1.3's little generator
//! (`krand`) is started from the same seed every time, so they fall in the same places, and
//! the picture is the same on every boot. The numbers must be drawn from the generator in
//! exactly 1.3's order, or every star would move.
//!
//! Integer arithmetic only (no `f32`), as `lib/draw` does: the same pixels on every processor.
//! A pixel here is a `u32` holding `0x00RRGGBB`.

use crate::taskbar;

/// The seed 1.3 used (the day it was made: 28 October 2026).
const SEED: u32 = 20261028;

/// The biggest screen side this will draw. Far past any real screen; it keeps the arithmetic
/// below inside `i32` (a number that is 32 bits wide, with a sign).
const MAX_SIDE: u32 = 16_384;

/// The mountain's outline: (x, height) pairs, both in thousandths of the screen's width and
/// height, from left to right. The flat top is the long stretch at 400.
const RIDGE: [(i32, i32); 26] = [
    (0, 90),
    (60, 120),
    (130, 190),
    (180, 270),
    (205, 330),
    (220, 345),
    (240, 318),
    (265, 330),
    (290, 372),
    (315, 396),
    (332, 402),
    (598, 400),
    (618, 382),
    (650, 322),
    (680, 252),
    (710, 205),
    (735, 214),
    (758, 258),
    (772, 276),
    (788, 270),
    (805, 236),
    (830, 182),
    (870, 142),
    (920, 120),
    (970, 102),
    (1000, 96),
];

/// The faint range behind it, in the same units.
const FAR_RIDGE: [(i32, i32); 11] = [
    (0, 150),
    (90, 170),
    (160, 150),
    (230, 175),
    (330, 140),
    (450, 160),
    (560, 130),
    (700, 155),
    (820, 120),
    (900, 140),
    (1000, 115),
];

/// Draws the wallpaper for a screen `width` x `height` device pixels into `pixels` (rows
/// `stride` pixels apart, each pixel `0x00RRGGBB`). `scale` is device pixels per logical pixel
/// (1 or 2; anything else is brought within 1 to 4): only the taskbar's height, which the
/// sea's reflections stop above, depends on it.
///
/// If `pixels` is too short for that size, or a side is over 16384, or the stride is less than
/// the width, nothing is drawn (there is nowhere to draw it).
pub fn draw(pixels: &mut [u32], width: u32, height: u32, stride: u32, scale: u32) {
    let needed = if width == 0 || height == 0 {
        0
    } else {
        u64::from(stride) * u64::from(height - 1) + u64::from(width)
    };
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || stride < width
        || needed > pixels.len() as u64
    {
        return;
    }
    let bar = taskbar::HEIGHT * scale.clamp(1, 4) as i32;
    let mut wall = Wall { pixels, stride: stride as usize, w: width as i32, h: height as i32 };
    wall.paint(bar);
}

/// The pixels being painted, with the bounds-checked drawing 1.3's `wset` and `wblend` did.
struct Wall<'a> {
    pixels: &'a mut [u32],
    stride: usize,
    w: i32,
    h: i32,
}

/// 1.3's random number generator (`krand`): xorshift32. Each step mixes the number with copies
/// of itself shifted left and right, which is quick, needs no division, and is the same on any
/// processor.
struct Random(u32);

impl Random {
    fn next(&mut self) -> u32 {
        // `<<` and `>>` shift the bits; bits shifted out are dropped (no overflow panic).
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// `krand() % n`, as 1.3 wrote it (a number from 0 to n - 1). A bad `n` (zero or less)
    /// is taken as 1, where 1.3 would have divided by zero.
    fn below(&mut self, n: i32) -> i32 {
        (self.next() % n.max(1) as u32) as i32
    }
}

/// 1.3's `isqrt`: the square root of `v`, rounded down.
fn isqrt(mut v: i32) -> i32 {
    if v <= 0 {
        return 0;
    }
    let mut root = 0;
    let mut bit = 1 << 30;
    while bit > v {
        bit >>= 2;
    }
    while bit != 0 {
        if v >= root + bit {
            v -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// 1.3's `color_lerp`: the colour `t` parts of `tmax` of the way from `a` to `b`, each channel
/// worked out separately and rounded towards `a` (integer division). `t` outside 0 to `tmax`
/// is cut to it.
fn lerp(a: u32, b: u32, t: i32, tmax: i32) -> u32 {
    if tmax <= 0 {
        return a;
    }
    let t = t.clamp(0, tmax);
    let channel = |shift: u32| -> u32 {
        let from = (a >> shift & 255) as i32;
        let to = (b >> shift & 255) as i32;
        (from + (to - from) * t / tmax) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

/// How high a ridge stands above the horizon at column `x`, in pixels: the outline's height
/// (thousandths of the screen's height) found by walking along its points and drawing a
/// straight line between the two either side of `x`.
fn profile(points: &[(i32, i32)], x: i32, w: i32, h: i32) -> i32 {
    let fx = x * 1000 / w;
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if fx >= x0 && fx <= x1 {
            let span = x1 - x0;
            let t = if span != 0 { (fx - x0) * 1000 / span } else { 0 };
            return (y0 + (y1 - y0) * t / 1000) * h / 1000;
        }
    }
    0
}

impl Wall<'_> {
    /// 1.3's `wset`: sets one pixel, if it is on the screen.
    fn set(&mut self, x: i32, y: i32, colour: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.pixels[y as usize * self.stride + x as usize] = colour;
        }
    }

    /// 1.3's `wblend`: mixes `colour` over one pixel, `a` parts of 255.
    fn blend(&mut self, x: i32, y: i32, colour: u32, a: i32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let at = y as usize * self.stride + x as usize;
            self.pixels[at] = lerp(self.pixels[at], colour, a, 255);
        }
    }

    /// Blends `colour` along row `y` from `x0` to `x1` (both included). The same as calling
    /// [`Wall::blend`] for each, but cut to the screen first, so a huge span costs no more
    /// than the screen's width.
    fn blend_span(&mut self, y: i32, x0: i32, x1: i32, colour: u32, a: i32) {
        for x in x0.max(0)..=x1.min(self.w - 1) {
            self.blend(x, y, colour, a);
        }
    }

    /// Fills a whole row.
    fn row(&mut self, y: i32, colour: u32) {
        let at = y as usize * self.stride;
        self.pixels[at..at + self.w as usize].fill(colour);
    }

    /// `bar` is the taskbar's height in device pixels.
    fn paint(&mut self, bar: i32) {
        let (w, h) = (self.w, self.h);
        let horizon = h * 76 / 100;
        let mut random = Random(SEED);

        // The sky: three colours, dark violet at the top through mauve to orange at the horizon.
        let middle = horizon * 55 / 100;
        for y in 0..horizon {
            let colour = if y < middle {
                lerp(0x161a36, 0x6a4a7c, y, middle)
            } else {
                lerp(0x6a4a7c, 0xf2a35e, y - middle, horizon - middle)
            };
            self.row(y, colour);
        }

        // Stars: `w / 6` of them, in the top 40 per cent of the sky, fainter lower down.
        let star_sky = horizon * 40 / 100;
        for _ in 0..w / 6 {
            let x = random.below(w);
            let y = random.below(star_sky);
            let a = 80 + random.below(160);
            self.blend(x, y, 0xffffff, a * (star_sky - y) / star_sky.max(1));
        }

        // The setting sun with a soft glow: many faint discs of growing size laid on top of
        // each other, then the sun itself with a gradient, orange at the bottom.
        let sun_x = w * 91 / 100;
        let sun_y = horizon - h * 15 / 100;
        let sun_r = h * 45 / 1000;
        let mut ring = sun_r * 4;
        while ring > sun_r {
            for dy in -ring..=ring {
                if sun_y + dy < horizon && sun_y + dy >= 0 {
                    let dx = isqrt(ring * ring - dy * dy);
                    self.blend_span(sun_y + dy, sun_x - dx, sun_x + dx, 0xffc27a, 6);
                }
            }
            ring -= 2;
        }
        for dy in -sun_r..=sun_r {
            if sun_y + dy < horizon && sun_y + dy >= 0 {
                let dx = isqrt(sun_r * sun_r - dy * dy);
                let colour = lerp(0xffe6b0, 0xffb066, dy + sun_r, sun_r * 2);
                for x in (sun_x - dx).max(0)..=(sun_x + dx).min(w - 1) {
                    self.set(x, sun_y + dy, colour);
                }
            }
        }

        // The distant range.
        for x in 0..w {
            let top = horizon - profile(&FAR_RIDGE, x, w, h) * 55 / 100;
            for y in top..horizon {
                self.set(x, y, lerp(0x8a5a7e, 0x9c6a80, y - top, horizon - top));
            }
        }

        // The mountain, with a warm rim of light along the ridge.
        for x in 0..w {
            let top = horizon - profile(&RIDGE, x, w, h);
            for y in top..horizon {
                self.set(x, y, lerp(0x342a4c, 0x1d1a30, y - top, horizon - top));
            }
            self.set(x, top, 0xc98a8a);
            self.blend(x, top + 1, 0xc98a8a, 120);
            self.blend(x, top + 2, 0xc98a8a, 50);
        }

        // The "tablecloth" of cloud resting on the flat top, with soft wisps spilling over.
        let cloud_left = w * 325 / 1000;
        let cloud_right = w * 605 / 1000;
        let cloud_top = horizon - h * 401 / 1000;
        let mut bump = [0i32; 64];
        for b in &mut bump {
            *b = 12 + random.below(14);
        }
        for x in cloud_left..cloud_right {
            let segment = ((x - cloud_left) / 22) as usize;
            let t = (x - cloud_left) % 22;
            // The puffy outline: blend between neighbouring bumps.
            let (b0, b1) = (bump[segment % 64], bump[(segment + 1) % 64]);
            let b = b0 + (b1 - b0) * t / 22;
            let edge = (x - cloud_left).min(cloud_right - x);
            let taper = (edge * 256 / 40).min(256);
            let height = b * taper / 256;
            let spill = 8 * taper / 256;
            for k in -height..=spill {
                let a = if k < 0 {
                    // Dense body, soft top.
                    i32::min(230, 230 * (height + k) * 3 / (height * 2 + 1))
                } else {
                    // Wisps over the edge.
                    230 - k * 210 / (spill + 1)
                };
                let a = a * taper / 256;
                if a > 0 {
                    self.blend(x, cloud_top + k, 0xf4ecf0, a);
                }
            }
        }

        // The sea, with the sun's reflections stopping above the taskbar.
        for y in horizon..h {
            self.row(y, lerp(0x37406a, 0x0e1226, y - horizon, h - horizon));
        }
        for _ in 0..w / 2 {
            let y = horizon + 2 + random.below(h - horizon - bar);
            let depth = y - horizon;
            let length = 6 + random.below(30);
            let mut x = sun_x + random.below(60 + depth * 3) - (30 + depth * 3 / 2);
            if random.below(3) == 0 {
                x = random.below(w);
            }
            let a = 170 - depth * 170 / (h - horizon).max(1);
            self.blend_span(y, x, x.saturating_add(length - 1), 0xf7b56c, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_1_3s() {
        // xorshift32 from the seed: worked out by hand from the three shifts.
        let mut random = Random(SEED);
        let mut v = SEED;
        v ^= v << 13;
        v ^= v >> 17;
        v ^= v << 5;
        assert_eq!(random.next(), v);
        assert_eq!(random.below(0), 0);
    }

    #[test]
    fn square_roots() {
        for v in 0..5000 {
            let r = isqrt(v);
            assert!(r * r <= v && (r + 1) * (r + 1) > v, "{v}");
        }
        assert_eq!(isqrt(-5), 0);
        assert_eq!(isqrt(i32::MAX), 46340);
    }

    #[test]
    fn lerp_is_1_3s() {
        assert_eq!(lerp(0x000000, 0xff8040, 1, 2), 0x7f4020);
        assert_eq!(lerp(0x123456, 0xffffff, 0, 5), 0x123456);
        assert_eq!(lerp(0x123456, 0xffffff, 5, 5), 0xffffff);
        assert_eq!(lerp(0x123456, 0xffffff, 3, 0), 0x123456);
    }
}
