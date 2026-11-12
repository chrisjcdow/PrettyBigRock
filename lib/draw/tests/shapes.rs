// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Rounded rectangles, ellipses, gradients, shadows, lines and flood fill, tested four ways:
//!
//! 1. **Against a model.** Each shape's rule is written out again below as the slowest, most
//!    obvious function: for every pixel of a small canvas, decide straight from the formula in
//!    the design note whether it is in the shape (`Op::colour_at`). The real drawing must
//!    give the same canvas, pixel for pixel, for a great many cases. The models use no
//!    square-root helper of the library's (they search for roots), and no floating point.
//! 2. **Properties** that hold whatever the rule is: a clip never changes a pixel, a part of a
//!    canvas draws what the whole would, every pixel is mixed once, shapes are symmetric,
//!    damage covers every change.
//! 3. **Hostile arguments**: no panic, nothing outside the canvas touched, and bounded time.
//! 4. **Reference pictures** (`tests/pictures/draw/`), at 1× and 2×.

mod common;

use common::{PAPER, Sheet};
use pbr_draw::{Canvas, Colour, Damage, Rect};
use std::collections::VecDeque;
use std::time::Instant;

/// The same cut-off `Canvas` puts on device positions.
const LIMIT: i64 = 1 << 24;

// ---------------------------------------------------------------------------------------
// A small random number generator (xorshift), so every run is the same.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n
    }
    /// A number from `low` up to (not including) `high`.
    fn range(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((i64::from(high) - i64::from(low)) as u64) as i32
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
    fn colour(&mut self) -> Colour {
        let alpha = self.pick(&[0u8, 1, 64, 128, 191, 254, 255, 255, 128]);
        Colour((u32::from(alpha) << 24) | (self.next() as u32 & 0x00FF_FFFF))
    }
}

// ---------------------------------------------------------------------------------------
// The operations under test, as data, so that one set of checks can run on all of them.

#[derive(Clone, Copy, Debug)]
enum Op {
    Round { rect: Rect, radius: i32, colour: Colour },
    StrokeRound { rect: Rect, radius: i32, thickness: i32, colour: Colour },
    Ellipse { rect: Rect, colour: Colour },
    StrokeEllipse { rect: Rect, thickness: i32, colour: Colour },
    Down { rect: Rect, top: Colour, bottom: Colour },
    Across { rect: Rect, left: Colour, right: Colour },
    Shadow { rect: Rect, radius: i32, blur: i32, colour: Colour },
    Line { from: (i32, i32), to: (i32, i32), width: i32, colour: Colour },
    Flood { x: i32, y: i32, colour: Colour },
}

impl Op {
    fn draw(&self, c: &mut Canvas) {
        match *self {
            Op::Round { rect, radius, colour } => c.fill_round_rect(rect, radius, colour),
            Op::StrokeRound { rect, radius, thickness, colour } => {
                c.stroke_round_rect(rect, radius, thickness, colour)
            }
            Op::Ellipse { rect, colour } => c.fill_ellipse(rect, colour),
            Op::StrokeEllipse { rect, thickness, colour } => {
                c.stroke_ellipse(rect, thickness, colour)
            }
            Op::Down { rect, top, bottom } => c.gradient_down(rect, top, bottom),
            Op::Across { rect, left, right } => c.gradient_across(rect, left, right),
            Op::Shadow { rect, radius, blur, colour } => c.shadow(rect, radius, blur, colour),
            Op::Line { from, to, width, colour } => c.line(from, to, width, colour),
            Op::Flood { x, y, colour } => c.flood_fill(x, y, colour),
        }
    }

    /// The same operation drawn (`dx`, `dy`) further right and down, in logical pixels.
    fn moved(&self, dx: i32, dy: i32) -> Op {
        let mut op = *self;
        let shift = |r: &mut Rect| *r = r.translate(dx, dy);
        let shift_point =
            |p: &mut (i32, i32)| *p = (p.0.saturating_add(dx), p.1.saturating_add(dy));
        match &mut op {
            Op::Round { rect, .. }
            | Op::StrokeRound { rect, .. }
            | Op::Ellipse { rect, .. }
            | Op::StrokeEllipse { rect, .. }
            | Op::Down { rect, .. }
            | Op::Across { rect, .. }
            | Op::Shadow { rect, .. } => shift(rect),
            Op::Line { from, to, .. } => {
                shift_point(from);
                shift_point(to);
            }
            Op::Flood { x, y, .. } => {
                *x = x.saturating_add(dx);
                *y = y.saturating_add(dy);
            }
        }
        op
    }

    /// Whether this call is plainly one that draws nothing (a clear colour, an empty
    /// rectangle, no thickness), so that it must record no damage.
    fn draws_nothing(&self) -> bool {
        let empty = |r: &Rect| r.width <= 0 || r.height <= 0;
        match self {
            Op::Round { rect, colour, .. }
            | Op::Ellipse { rect, colour }
            | Op::Shadow { rect, colour, .. } => empty(rect) || colour.alpha() == 0,
            Op::StrokeRound { rect, thickness, colour, .. }
            | Op::StrokeEllipse { rect, thickness, colour } => {
                empty(rect) || *thickness <= 0 || colour.alpha() == 0
            }
            Op::Down { rect, top, bottom } => empty(rect) || (top.alpha() | bottom.alpha()) == 0,
            Op::Across { rect, left, right } => empty(rect) || (left.alpha() | right.alpha()) == 0,
            Op::Line { width, colour, .. } => *width <= 0 || colour.alpha() == 0,
            Op::Flood { colour, .. } => colour.alpha() == 0,
        }
    }

    fn is_flood(&self) -> bool {
        matches!(self, Op::Flood { .. })
    }
}

// ---------------------------------------------------------------------------------------
// The models. Everything is in device pixels, with the canvas's corner at (0, 0).

/// A logical position or length in device pixels, cut the way the library cuts it.
fn dev(v: i64, scale: i64) -> i64 {
    (v * scale).clamp(-LIMIT, LIMIT)
}

/// A logical rectangle's device edges (left, top, right, bottom), or `None` if it is empty.
fn dev_rect(r: &Rect, scale: i64) -> Option<(i64, i64, i64, i64)> {
    if r.width <= 0 || r.height <= 0 {
        return None;
    }
    let (x, y) = (i64::from(r.x), i64::from(r.y));
    Some((
        dev(x, scale),
        dev(y, scale),
        dev(x + i64::from(r.width), scale),
        dev(y + i64::from(r.height), scale),
    ))
}

/// The corner radius in device pixels: the logical one scaled, brought into 0 to half the
/// shorter side.
fn dev_radius(radius: i32, scale: i64, x0: i64, y0: i64, x1: i64, y1: i64) -> i64 {
    dev(i64::from(radius), scale).clamp(0, ((x1 - x0).min(y1 - y0) / 2).max(0))
}

/// The rule for rounded rectangles, straight from the note: a pixel of the rectangle is out
/// only if it is in a corner square and its centre is farther than `r` from that corner's
/// circle's centre. The top left corner is worked out; the others are found by mirroring the
/// pixel into it.
fn in_round(px: i64, py: i64, (x0, y0, x1, y1): (i64, i64, i64, i64), r: i64) -> bool {
    if px < x0 || px >= x1 || py < y0 || py >= y1 {
        return false;
    }
    let fx = if 2 * px + 1 > x0 + x1 { x0 + x1 - 1 - px } else { px };
    let fy = if 2 * py + 1 > y0 + y1 { y0 + y1 - 1 - py } else { py };
    if fx < x0 + r && fy < y0 + r {
        let (a, b) = (2 * (x0 + r) - (2 * fx + 1), 2 * (y0 + r) - (2 * fy + 1));
        a * a + b * b <= (2 * r) * (2 * r)
    } else {
        true
    }
}

/// The rule for ellipses, straight from the note.
fn in_ellipse(px: i64, py: i64, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> bool {
    let (w, h) = (i128::from(x1 - x0), i128::from(y1 - y0));
    if w <= 0 || h <= 0 {
        return false;
    }
    let dx = i128::from(2 * px + 1 - (2 * x0 + (x1 - x0)));
    let dy = i128::from(2 * py + 1 - (2 * y0 + (y1 - y0)));
    dx * dx * h * h + dy * dy * w * w <= w * w * h * h
}

/// The smallest whole number whose square is at least `n`, by searching.
fn root_up(n: i64) -> i64 {
    let mut d = 0;
    while d * d < n {
        d += 1;
    }
    d
}

/// The plain Bresenham walk, copied from `gfx_line` in `classic/kernel/gfx.c`.
fn bresenham(from: (i64, i64), to: (i64, i64), mut plot: impl FnMut(i64, i64)) {
    let (mut x0, mut y0) = from;
    let (x1, y1) = to;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        plot(x0, y0);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

impl Op {
    /// The colour this operation mixes into the pixel (`px`, `py`), or `None` if the pixel
    /// is not in the shape. For every operation except flood fill and thin lines, which are
    /// not decided one pixel at a time and have their own models below.
    fn colour_at(&self, scale: i64, px: i64, py: i64) -> Option<Colour> {
        match *self {
            Op::Round { rect, radius, colour } => {
                let e = dev_rect(&rect, scale)?;
                let r = dev_radius(radius, scale, e.0, e.1, e.2, e.3);
                in_round(px, py, e, r).then_some(colour)
            }
            Op::StrokeRound { rect, radius, thickness, colour } => {
                let e = dev_rect(&rect, scale)?;
                let r = dev_radius(radius, scale, e.0, e.1, e.2, e.3);
                let t = dev(i64::from(thickness), scale);
                if t <= 0 || !in_round(px, py, e, r) {
                    return None;
                }
                let inner = (e.0 + t, e.1 + t, e.2 - t, e.3 - t);
                let hole = inner.2 > inner.0 && inner.3 > inner.1;
                (!(hole && in_round(px, py, inner, (r - t).max(0)))).then_some(colour)
            }
            Op::Ellipse { rect, colour } => {
                in_ellipse(px, py, dev_rect(&rect, scale)?).then_some(colour)
            }
            Op::StrokeEllipse { rect, thickness, colour } => {
                let e = dev_rect(&rect, scale)?;
                let t = dev(i64::from(thickness), scale);
                if t <= 0 || !in_ellipse(px, py, e) {
                    return None;
                }
                let inner = (e.0 + t, e.1 + t, e.2 - t, e.3 - t);
                let hole = inner.2 > inner.0 && inner.3 > inner.1;
                (!(hole && in_ellipse(px, py, inner))).then_some(colour)
            }
            Op::Down { rect, top, bottom } => {
                let e = dev_rect(&rect, scale)?;
                let inside = px >= e.0 && px < e.2 && py >= e.1 && py < e.3;
                inside.then(|| {
                    Colour::between(top, bottom, (py - e.1) as i32, (e.3 - e.1 - 1) as i32)
                })
            }
            Op::Across { rect, left, right } => {
                let e = dev_rect(&rect, scale)?;
                let inside = px >= e.0 && px < e.2 && py >= e.1 && py < e.3;
                inside.then(|| {
                    Colour::between(left, right, (px - e.0) as i32, (e.2 - e.0 - 1) as i32)
                })
            }
            Op::Shadow { rect, radius, blur, colour } => {
                let e = dev_rect(&rect, scale)?;
                let r = dev_radius(radius, scale, e.0, e.1, e.2, e.3);
                let b = dev(i64::from(blur), scale).min(1024);
                if b <= 0 {
                    return in_round(px, py, e, r).then_some(colour);
                }
                let qx = (2 * (e.0 + r) - (2 * px + 1)).max((2 * px + 1) - 2 * (e.2 - r)).max(0);
                let qy = (2 * (e.1 + r) - (2 * py + 1)).max((2 * py + 1) - 2 * (e.3 - r)).max(0);
                // The distance is at least the larger of the two, so this is far enough out
                // without searching for a root.
                if qx.max(qy) >= 2 * r + 2 * b {
                    return None;
                }
                let d = root_up(qx * qx + qy * qy);
                let dist = (d - 2 * r).max(0);
                if dist >= 2 * b {
                    return None;
                }
                let a = i64::from(colour.alpha());
                let alpha = (a * (2 * b - dist) * (2 * b - dist) + (2 * b) * (2 * b) / 2)
                    / ((2 * b) * (2 * b));
                Some(colour.with_alpha(alpha as u8))
            }
            Op::Line { from, to, width, colour } => {
                let s = scale;
                let w = dev(i64::from(width), s);
                if w < 2 {
                    return None;
                }
                let k = i64::from(w % 2 == 0 && s % 2 == 1);
                let a =
                    (2 * dev(i64::from(from.0), s) + s - k, 2 * dev(i64::from(from.1), s) + s - k);
                let b = (2 * dev(i64::from(to.0), s) + s - k, 2 * dev(i64::from(to.1), s) + s - k);
                let (p, d) = ((2 * px + 1, 2 * py + 1), (b.0 - a.0, b.1 - a.1));
                let v = (p.0 - a.0, p.1 - a.1);
                let (t, l) = (v.0 * d.0 + v.1 * d.1, d.0 * d.0 + d.1 * d.1);
                let w2 = i128::from(w) * i128::from(w);
                let sq =
                    |x: i64, y: i64| i128::from(x) * i128::from(x) + i128::from(y) * i128::from(y);
                let inside = if l == 0 || t <= 0 {
                    sq(v.0, v.1) <= w2
                } else if t >= l {
                    sq(p.0 - b.0, p.1 - b.1) <= w2
                } else {
                    let cross =
                        i128::from(v.0) * i128::from(d.1) - i128::from(v.1) * i128::from(d.0);
                    cross * cross <= w2 * i128::from(l)
                };
                inside.then_some(colour)
            }
            Op::Flood { .. } => None,
        }
    }
}

/// A canvas's pixels, for the models to draw on.
struct Buf {
    w: i32,
    h: i32,
    px: Vec<u32>,
}

impl Buf {
    /// Mixes `colour` into the pixel at (`x`, `y`) if it is in the canvas and the clip. A clear
    /// colour changes nothing, not even the spare byte.
    fn put(&mut self, clip: &Rect, x: i64, y: i64, colour: Colour) {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return;
        }
        if !clip.contains(x as i32, y as i32) || colour.alpha() == 0 {
            return;
        }
        let at = (y * i64::from(self.w) + x) as usize;
        self.px[at] = colour.over(self.px[at]);
    }
}

/// Draws `op` on `buf` the slow way, inside `clip` (device pixels).
fn model(op: &Op, scale: i64, clip: &Rect, buf: &mut Buf) {
    match *op {
        Op::Flood { x, y, colour } => {
            model_flood(buf, clip, dev(i64::from(x), scale), dev(i64::from(y), scale), colour)
        }
        Op::Line { from, to, width, colour } if dev(i64::from(width), scale) == 1 => {
            let a = (dev(i64::from(from.0), scale), dev(i64::from(from.1), scale));
            let b = (dev(i64::from(to.0), scale), dev(i64::from(to.1), scale));
            if colour.alpha() != 0 {
                bresenham(a, b, |x, y| buf.put(clip, x, y, colour));
            }
        }
        _ => {
            for py in 0..i64::from(buf.h) {
                for px in 0..i64::from(buf.w) {
                    if let Some(colour) = op.colour_at(scale, px, py) {
                        buf.put(clip, px, py, colour);
                    }
                }
            }
        }
    }
}

/// Flood fill as a breadth-first search over single pixels.
fn model_flood(buf: &mut Buf, clip: &Rect, x: i64, y: i64, colour: Colour) {
    if x < 0
        || y < 0
        || x > i64::from(i32::MAX)
        || y > i64::from(i32::MAX)
        || !clip.contains(x as i32, y as i32)
    {
        return;
    }
    let at = |x: i64, y: i64| (y * i64::from(buf.w) + x) as usize;
    let old = buf.px[at(x, y)] & 0x00FF_FFFF;
    let new = colour.over(old);
    if new & 0x00FF_FFFF == old {
        return;
    }
    let mut queue = VecDeque::from([(x, y)]);
    let mut seen = vec![false; buf.px.len()];
    seen[at(x, y)] = true;
    while let Some((x, y)) = queue.pop_front() {
        let i = at(x, y);
        buf.px[i] = new;
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if nx < 0 || ny < 0 || !clip.contains(nx as i32, ny as i32) {
                continue;
            }
            let j = at(nx, ny);
            if !seen[j] && buf.px[j] & 0x00FF_FFFF == old {
                seen[j] = true;
                queue.push_back((nx, ny));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Running an operation for real, and the checks that go with every run.

/// Draws on a copy of `bg` (`w` by `h` device pixels) at `scale` through `f`, recording
/// damage.
fn render(
    bg: &[u32],
    w: i32,
    h: i32,
    scale: u32,
    f: impl FnOnce(&mut Canvas),
) -> (Vec<u32>, Damage) {
    let mut px = bg.to_vec();
    let mut damage = Damage::new();
    {
        let mut c = Canvas::new(&mut px, w as u32, h as u32, w as u32)
            .unwrap()
            .with_scale(scale)
            .with_damage(&mut damage);
        f(&mut c);
    }
    (px, damage)
}

/// The first place two pictures differ, as a message.
fn first_difference(got: &[u32], want: &[u32], w: i32) -> Option<String> {
    let at = got.iter().zip(want).position(|(a, b)| a != b)?;
    let (x, y) = (at as i32 % w, at as i32 / w);
    Some(format!("first difference at {x},{y}: got {:#010x}, want {:#010x}", got[at], want[at]))
}

/// A background that is different at every pixel, with a spare byte that no drawing should
/// ever leave alone-and-changed (`0x5A`: drawing sets it to `0xFF`, not drawing keeps it).
fn busy(w: i32, h: i32, salt: u32) -> Vec<u32> {
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as u32, (i / w) as u32);
            0x5A00_0000
                | (x * 37 + y * 91 + salt.wrapping_mul(1_234_567)).wrapping_mul(0x0001_0305)
                    & 0x00FF_FFFF
        })
        .collect()
}

/// A background of blobs of two or three colours (each pixel's spare byte at random), for flood
/// fill.
fn blobs(w: i32, h: i32, rng: &mut Rng) -> Vec<u32> {
    let colours: Vec<u32> = (0..rng.range(2, 4)).map(|_| rng.next() as u32 & 0x00FF_FFFF).collect();
    let cell = rng.range(1, 7);
    let cells_wide = (w + cell - 1) / cell;
    let cells: Vec<u32> = (0..cells_wide * ((h + cell - 1) / cell))
        .map(|_| colours[rng.below(colours.len() as u64) as usize])
        .collect();
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            cells[((y / cell) * cells_wide + x / cell) as usize]
                | rng.pick(&[0u32, 0xFF00_0000, 0x5A00_0000])
        })
        .collect()
}

/// `clip` (logical pixels) as device pixels inside a `w` by `h` canvas.
fn device_clip(clip: &Rect, scale: i32, w: i32, h: i32) -> Rect {
    let s = i64::from(scale);
    Rect::from_edges(
        i64::from(clip.x) * s,
        i64::from(clip.y) * s,
        clip.right() * s,
        clip.bottom() * s,
    )
    .intersect(&Rect::new(0, 0, w, h))
}

/// The checks that go with every drawing call: damage lies inside the clip, covers every
/// change, and is empty if the call plainly draws nothing.
fn check_damage(op: &Op, bg: &[u32], got: &[u32], damage: &Damage, clip: &Rect, w: i32) {
    for r in damage.rects() {
        assert!(clip.contains_rect(r), "{op:?}: damage {r:?} is outside the clip {clip:?}");
    }
    for (i, (a, b)) in got.iter().zip(bg).enumerate() {
        let (x, y) = (i as i32 % w, i as i32 / w);
        if a != b {
            assert!(
                damage.rects().iter().any(|r| r.contains(x, y)),
                "{op:?}: the pixel at {x},{y} changed but is not in the damage {:?}",
                damage.rects()
            );
        }
    }
    if op.draws_nothing() {
        assert!(
            damage.is_empty() && got == bg,
            "{op:?} draws nothing but recorded {:?}",
            damage.rects()
        );
    }
}

/// Real against model, for one canvas, with the damage checks. Returns the real pixels.
fn check_model(op: Op, scale: u32, w: i32, h: i32, bg: &[u32]) -> Vec<u32> {
    let (got, damage) = render(bg, w, h, scale, |c| op.draw(c));
    let mut want = Buf { w, h, px: bg.to_vec() };
    let clip = Rect::new(0, 0, w, h);
    model(&op, i64::from(scale), &clip, &mut want);
    if let Some(why) = first_difference(&got, &want.px, w) {
        panic!("{op:?} at scale {scale} on {w}x{h}: {why}");
    }
    check_damage(&op, bg, &got, &damage, &clip, w);
    got
}

/// Everything that can be checked about one operation: the model, three clips, and a part of
/// the canvas.
fn check_all(op: Op, scale: u32, w: i32, h: i32, bg: &[u32], rng: &mut Rng) {
    let whole = check_model(op, scale, w, h, bg);
    let s = scale as i32;
    for _ in 0..3 {
        // A clip in logical pixels that sometimes sticks out, sometimes is empty.
        let c = Rect::new(
            rng.range(-3, w / s + 3),
            rng.range(-3, h / s + 3),
            rng.range(-1, w / s + 3),
            rng.range(-1, h / s + 3),
        );
        let clip = device_clip(&c, s, w, h);
        let (cut, damage) = render(bg, w, h, scale, |canvas| op.draw(&mut canvas.clipped(c)));
        let mut want = Buf { w, h, px: bg.to_vec() };
        model(&op, i64::from(scale), &clip, &mut want);
        if let Some(why) = first_difference(&cut, &want.px, w) {
            panic!("{op:?} at scale {scale} clipped to {c:?}: {why}");
        }
        check_damage(&op, bg, &cut, &damage, &clip, w);
        if !op.is_flood() {
            // A clip never changes a pixel: inside it, the same as unclipped; outside, as before.
            for (i, ((a, b), z)) in cut.iter().zip(&whole).zip(bg).enumerate() {
                let inside = clip.contains(i as i32 % w, i as i32 / w);
                assert_eq!(*a, if inside { *b } else { *z }, "{op:?} clipped to {c:?}: pixel {i}");
            }
        }
    }
    // Drawing on a part of the canvas, from its own corner, is drawing at its corner, cut to it.
    for _ in 0..2 {
        let area = Rect::new(
            rng.range(-3, w / s),
            rng.range(-3, h / s),
            rng.range(-1, w / s + 3),
            rng.range(-1, h / s + 3),
        );
        let (part, _) = render(bg, w, h, scale, |canvas| op.draw(&mut canvas.sub(area)));
        let (moved, _) = render(bg, w, h, scale, |canvas| {
            op.moved(area.x, area.y).draw(&mut canvas.clipped(area))
        });
        if let Some(why) = first_difference(&part, &moved, w) {
            panic!("{op:?} at scale {scale} on a part {area:?}: {why}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Random cases.

fn random_rect(rng: &mut Rng) -> Rect {
    Rect::new(rng.range(-15, 50), rng.range(-15, 50), rng.range(-2, 46), rng.range(-2, 46))
}

fn random_op(rng: &mut Rng, kind: u64, w: i32, h: i32) -> Op {
    let rect = random_rect(rng);
    let colour = rng.colour();
    match kind {
        0 => Op::Round { rect, radius: rng.range(-3, 30), colour },
        1 => Op::StrokeRound {
            rect,
            radius: rng.range(-3, 30),
            thickness: rng.range(-1, 14),
            colour,
        },
        2 => Op::Ellipse { rect, colour },
        3 => Op::StrokeEllipse { rect, thickness: rng.range(-1, 14), colour },
        4 => Op::Down { rect, top: colour, bottom: rng.colour() },
        5 => Op::Across { rect, left: colour, right: rng.colour() },
        6 => Op::Shadow { rect, radius: rng.range(-3, 20), blur: rng.range(-2, 14), colour },
        7 => {
            let point = |rng: &mut Rng| (rng.range(-20, 80), rng.range(-20, 80));
            Op::Line { from: point(rng), to: point(rng), width: rng.range(-1, 12), colour }
        }
        _ => Op::Flood { x: rng.range(-2, w + 2), y: rng.range(-2, h + 2), colour },
    }
}

#[test]
fn random_cases_match_the_models_and_obey_the_rules() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for kind in 0..9 {
        for case in 0..300u32 {
            let scale = rng.range(1, 4) as u32;
            let (w, h) = (rng.range(6, 60), rng.range(6, 60));
            let op = random_op(&mut rng, kind, w, h);
            let bg = if kind == 8 { blobs(w, h, &mut rng) } else { busy(w, h, case) };
            check_all(op, scale, w, h, &bg, &mut rng);
        }
    }
}

/// Lines that are longer than the canvas by far, thin and thick, at a clip and without: the
/// real line (which jumps to the clip) against the plain walk from end to end.
#[test]
fn long_lines_match_the_plain_walk() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for case in 0..400u32 {
        let reach = *[40, 300, 5_000, 200_000].get((case % 4) as usize).unwrap();
        let point = |rng: &mut Rng| (rng.range(-reach, reach), rng.range(-reach, reach));
        let (from, to) = (point(&mut rng), point(&mut rng));
        let width = if case % 2 == 0 { 1 } else { rng.range(2, 6) };
        let scale = if case % 8 == 0 { 2 } else { 1 };
        let op = Op::Line { from, to, width, colour: Colour(0xA0_20_80_F0) };
        let bg = busy(50, 40, case);
        check_all(op, scale, 50, 40, &bg, &mut rng);
    }
    // Lines that cross a small clip on the diagonal, and end at the same place as a clip edge.
    for case in 0..300u32 {
        let from = (rng.range(-3000, 3000), rng.range(-3000, 3000));
        let to = (rng.range(-3000, 3000), rng.range(-3000, 3000));
        let op = Op::Line { from, to, width: 1, colour: Colour::solid(0x123456) };
        let bg = busy(30, 30, case);
        let clip =
            Rect::new(rng.range(0, 20), rng.range(0, 20), rng.range(1, 12), rng.range(1, 12));
        let (got, _) = render(&bg, 30, 30, 1, |c| op.draw(&mut c.clipped(clip)));
        let mut want = Buf { w: 30, h: 30, px: bg.clone() };
        model(&op, 1, &clip, &mut want);
        assert_eq!(first_difference(&got, &want.px, 30), None, "{op:?} {clip:?}");
    }
}

// ---------------------------------------------------------------------------------------
// Exhaustive cases: every size, every radius, every thickness, three scales.

const SIZES: [i32; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 11, 16, 23, 31, 40];
const RADII: [i32; 17] = [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 20, 25, 41, 100];
const THICKNESSES: [i32; 10] = [-1, 0, 1, 2, 3, 4, 5, 8, 13, 30];

#[test]
fn rounded_rectangles_of_every_size_and_radius_match_the_model() {
    for scale in 1..=3u32 {
        for &w in &SIZES {
            for &h in &SIZES {
                // Room all round, so the whole shape is seen.
                let (cw, ch) = ((w + 6) * scale as i32, (h + 6) * scale as i32);
                let bg = busy(cw, ch, w as u32 * 100 + h as u32);
                let rect = Rect::new(3, 3, w, h);
                for &radius in &RADII {
                    let colour = Colour(0x90_30_70_C0);
                    check_model(Op::Round { rect, radius, colour }, scale, cw, ch, &bg);
                    // The same, as a shadow too small to show: the pixels that come out solid.
                    for &thickness in &THICKNESSES {
                        let op = Op::StrokeRound { rect, radius, thickness, colour };
                        check_model(op, scale, cw, ch, &bg);
                    }
                }
            }
        }
    }
}

#[test]
fn ellipses_of_every_size_match_the_model() {
    for scale in 1..=3u32 {
        for &w in &SIZES {
            for &h in &SIZES {
                let (cw, ch) = ((w + 6) * scale as i32, (h + 6) * scale as i32);
                let bg = busy(cw, ch, w as u32 * 100 + h as u32);
                let rect = Rect::new(3, 3, w, h);
                let colour = Colour(0x90_30_70_C0);
                check_model(Op::Ellipse { rect, colour }, scale, cw, ch, &bg);
                for &thickness in &THICKNESSES {
                    check_model(Op::StrokeEllipse { rect, thickness, colour }, scale, cw, ch, &bg);
                }
            }
        }
    }
}

#[test]
fn shadows_of_every_size_radius_and_blur_match_the_model() {
    for scale in 1..=2u32 {
        for &w in &[0, 1, 2, 5, 8, 15, 24] {
            for &h in &[1, 2, 6, 9, 20] {
                for &radius in &[-1, 0, 1, 3, 6, 12, 50] {
                    for &blur in &[-3, 0, 1, 2, 3, 5, 9, 17] {
                        let pad = 20;
                        let (cw, ch) = ((w + 2 * pad) * scale as i32, (h + 2 * pad) * scale as i32);
                        let bg = busy(cw, ch, (w * 7 + h) as u32);
                        let rect = Rect::new(pad, pad, w, h);
                        let colour = Colour(0x80_00_00_00 | (w * 9000 + h * 77) as u32);
                        check_model(Op::Shadow { rect, radius, blur, colour }, scale, cw, ch, &bg);
                    }
                }
            }
        }
    }
}

/// The note says `e` is zero for exactly the pixels `fill_round_rect` fills. With a blur of one
/// pixel, `e` is 0 or 1 inside the blur, and the pixels drawn at full strength are those with
/// `e = 0`.
#[test]
fn a_shadow_is_at_full_strength_exactly_where_the_shape_is() {
    let black = Colour::BLACK;
    for scale in 1..=3u32 {
        for w in 1..=30 {
            for h in [1, 2, 7, 13, 30] {
                for radius in 0..=16 {
                    let (cw, ch) = ((w + 6) * scale as i32, (h + 6) * scale as i32);
                    let bg = vec![Colour::WHITE.0; (cw * ch) as usize];
                    let rect = Rect::new(3, 3, w, h);
                    let (shadow, _) =
                        render(&bg, cw, ch, scale, |c| c.shadow(rect, radius, 1, black));
                    let (round, _) =
                        render(&bg, cw, ch, scale, |c| c.fill_round_rect(rect, radius, black));
                    for (i, (s, r)) in shadow.iter().zip(&round).enumerate() {
                        assert_eq!(
                            *s == black.0,
                            *r == black.0,
                            "{w}x{h} r{radius} scale {scale}: pixel {i}"
                        );
                    }
                    // And the fade is outside only: nothing inside is lighter than black.
                    assert!(shadow.iter().zip(&round).all(|(s, r)| *r != black.0 || *s == black.0));
                }
            }
        }
    }
}

#[test]
fn gradients_match_the_model_for_every_length() {
    let (a, b) = (Colour(0xFF2F_6B55), Colour(0x4022_503E));
    for scale in 1..=3u32 {
        for len in 0..=30 {
            for thick in [1, 2, 5] {
                let (cw, ch) = ((len + 4) * scale as i32, (thick + 4) * scale as i32);
                let bg = busy(cw, ch, len as u32);
                check_model(
                    Op::Down { rect: Rect::new(2, 2, thick, len), top: a, bottom: b },
                    scale,
                    ch,
                    cw,
                    &busy(ch, cw, 3),
                );
                check_model(
                    Op::Across { rect: Rect::new(2, 2, len, thick), left: a, right: b },
                    scale,
                    cw,
                    ch,
                    &bg,
                );
            }
        }
    }
}

#[test]
fn lines_of_every_width_and_direction_match_the_model() {
    for scale in 1..=3u32 {
        for width in -1..=9 {
            let (cw, ch) = (50 * scale as i32, 40 * scale as i32);
            let bg = busy(cw, ch, width as u32);
            let colour = Colour(0xA0_FF_20_20);
            // Every direction from a middle point, to the corners and edges and beyond.
            let centre = (25, 20);
            for tx in (-10..=60).step_by(5) {
                for ty in (-10..=50).step_by(5) {
                    check_model(
                        Op::Line { from: centre, to: (tx, ty), width, colour },
                        scale,
                        cw,
                        ch,
                        &bg,
                    );
                }
            }
            // Dots, and lines between neighbours.
            for from in [(10, 10), (-2, 7), (49, 39), (60, 60)] {
                check_model(Op::Line { from, to: from, width, colour }, scale, cw, ch, &bg);
                check_model(
                    Op::Line { from, to: (from.0 + 1, from.1), width, colour },
                    scale,
                    cw,
                    ch,
                    &bg,
                );
                check_model(
                    Op::Line { from, to: (from.0, from.1 + 1), width, colour },
                    scale,
                    cw,
                    ch,
                    &bg,
                );
            }
        }
    }
}

/// Where the shape is: wholly off each edge, partly off, and every position between.
#[test]
fn positions_in_and_off_the_canvas_match_the_model() {
    let colour = Colour(0xA0_20_90_30);
    for scale in 1..=3u32 {
        let (cw, ch) = (24, 24);
        let bg = busy(cw, ch, scale);
        for x in -20..=26 {
            for y in (-20..=26).step_by(3) {
                let rect = Rect::new(x, y, 13, 9);
                for op in [
                    Op::Round { rect, radius: 4, colour },
                    Op::StrokeRound { rect, radius: 6, thickness: 2, colour },
                    Op::Ellipse { rect, colour },
                    Op::StrokeEllipse { rect, thickness: 2, colour },
                    Op::Shadow { rect, radius: 3, blur: 4, colour },
                    Op::Down { rect, top: colour, bottom: Colour::WHITE },
                    Op::Across { rect, left: colour, right: Colour::WHITE },
                    Op::Line { from: (x, y), to: (x + 7, y - 5), width: 3, colour },
                    Op::Line { from: (x, y), to: (x + 7, y + 11), width: 1, colour },
                ] {
                    check_model(op, scale, cw, ch, &bg);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Flood fill.

#[test]
fn flood_fill_matches_a_search_over_single_pixels() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for case in 0..1500 {
        let (w, h) = (rng.range(1, 40), rng.range(1, 40));
        let bg = blobs(w, h, &mut rng);
        let scale = rng.range(1, 4);
        // Mostly a colour that is already on the canvas, or one that is not; sometimes see-through.
        let colour = match case % 4 {
            0 => Colour::solid(bg[rng.below(bg.len() as u64) as usize]),
            1 => Colour(0x80_FF_00_00),
            2 => rng.colour(),
            _ => Colour::solid(0x010203),
        };
        let op =
            Op::Flood { x: rng.range(-1, w / scale + 1), y: rng.range(-1, h / scale + 1), colour };
        check_all(op, scale as u32, w, h, &bg, &mut rng);
    }
}

#[test]
fn flood_fill_stops_at_the_edges_of_the_area_and_goes_round_corners() {
    let ink = 0xFF00_0000u32;
    // A winding way from the seed, and open pixels that touch it only at a corner (3,3), (5,3).
    let maze =
        ["#########", "#..#....#", "#..#.##.#", "#.#.#.#.#", "#.####.##", "#......##", "#########"];
    let (w, h) = (maze[0].len() as i32, maze.len() as i32);
    let mut px: Vec<u32> = maze
        .iter()
        .flat_map(|r| r.bytes().map(|b| if b == b'#' { ink } else { 0xFFFF_FFFF }))
        .collect();
    let mut canvas = Canvas::new(&mut px, w as u32, h as u32, w as u32).unwrap();
    canvas.flood_fill(1, 1, Colour::solid(0x00FF00));
    let green = 0xFF00_FF00u32;
    let shown: Vec<String> = px
        .chunks(w as usize)
        .map(|r| {
            r.iter()
                .map(|&p| {
                    if p == green {
                        'g'
                    } else if p == ink {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect();
    assert_eq!(shown[1], "#gg#....#");
    assert_eq!(shown[2], "#gg#.##.#");
    assert_eq!(shown[3], "#g#.#.#.#", "(3,3) touches (2,2) only at a corner");
    assert_eq!(shown[4], "#g####g##");
    assert_eq!(shown[5], "#gggggg##", "the way round along the bottom, then up one");
    // A diagonal gap does not leak: two pixels touching only at a corner are two areas.
    let mut px = vec![0xFFFF_FFFFu32, ink, ink, 0xFFFF_FFFF];
    Canvas::new(&mut px, 2, 2, 2).unwrap().flood_fill(0, 0, Colour::solid(0x00FF00));
    assert_eq!(px, [green, ink, ink, 0xFFFF_FFFF]);
}

#[test]
fn flood_fill_does_nothing_when_nothing_would_change() {
    let mut px = vec![0x1234_5678u32; 16];
    let mut damage = Damage::new();
    {
        let mut c = Canvas::new(&mut px, 4, 4, 4).unwrap().with_damage(&mut damage);
        c.flood_fill(1, 1, Colour::CLEAR);
        c.flood_fill(1, 1, Colour::solid(0x345678)); // the same colour, whatever the spare byte
        c.flood_fill(1, 1, Colour(0x8034_5678)); // mixed with itself: the same colour
        c.flood_fill(-1, 1, Colour::WHITE); // outside
        c.flood_fill(1, 4, Colour::WHITE);
        c.flood_fill(i32::MIN, i32::MAX, Colour::WHITE);
        c.clipped(Rect::new(2, 2, 2, 2)).flood_fill(1, 1, Colour::WHITE); // outside the clip
    }
    assert!(px.iter().all(|&p| p == 0x1234_5678));
    assert!(damage.is_empty());
    // And the clip is a wall: the fill stays in it.
    let mut c = Canvas::new(&mut px, 4, 4, 4).unwrap();
    c.clipped(Rect::new(1, 1, 2, 2)).flood_fill(1, 1, Colour::WHITE);
    assert_eq!(px.iter().filter(|&&p| p == 0xFFFF_FFFF).count(), 4);
}

#[test]
fn flood_fill_fills_a_big_area_with_a_stack_that_stays_small() {
    // A comb: long rows joined at one end, which asks the most of the stack.
    let (w, h) = (2000usize, 1500usize);
    let mut px = vec![0xFFFF_FFFFu32; w * h];
    for y in (1..h).step_by(2) {
        for x in 0..w - 1 {
            px[y * w + x] = 0xFF00_0000;
        }
    }
    let mut c = Canvas::new(&mut px, w as u32, h as u32, w as u32).unwrap();
    c.flood_fill(0, 0, Colour::solid(0x336699));
    assert!(px.iter().all(|&p| p == 0xFF33_6699 || p == 0xFF00_0000));
    assert_eq!(px.iter().filter(|&&p| p == 0xFF33_6699).count(), w * h - (h / 2) * (w - 1));
}

// ---------------------------------------------------------------------------------------
// Every pixel mixed once; symmetry.

#[test]
fn every_pixel_is_mixed_exactly_once() {
    let mut rng = Rng(0x0BAD_C0DE_1234_5678);
    let half = Colour(0x80_00_00_00);
    let white = vec![Colour::WHITE.0; 70 * 70];
    for kind in [0, 1, 2, 3, 7] {
        for _ in 0..400 {
            let scale = rng.range(1, 4) as u32;
            let mut op = random_op(&mut rng, kind, 70, 70);
            match &mut op {
                Op::Round { colour, .. }
                | Op::StrokeRound { colour, .. }
                | Op::Ellipse { colour, .. }
                | Op::StrokeEllipse { colour, .. }
                | Op::Line { colour, .. } => *colour = half,
                _ => unreachable!(),
            }
            let (got, _) = render(&white, 70, 70, scale, |c| op.draw(c));
            assert!(got.iter().all(|&p| p == 0xFFFF_FFFF || p == 0xFF7F_7F7F), "{op:?} at {scale}");
        }
    }
}

/// Symmetric shapes: the picture is its own mirror image, left to right and top to bottom.
#[test]
fn rounded_rectangles_ellipses_and_shadows_are_symmetric() {
    for scale in 1..=3u32 {
        for w in 1..=34 {
            for h in [1, 2, 3, 8, 15, 22, 33] {
                let m = 14; // a margin each side, wider than any blur used
                let (cw, ch) = ((w + 2 * m) * scale as i32, (h + 2 * m) * scale as i32);
                let rect = Rect::new(m, m, w, h);
                let colour = Colour(0xC0_10_20_30);
                let mut ops = vec![Op::Ellipse { rect, colour }];
                for radius in [0, 1, 2, 5, 9, 40] {
                    ops.push(Op::Round { rect, radius, colour });
                    ops.push(Op::StrokeRound { rect, radius, thickness: 2, colour });
                    ops.push(Op::Shadow { rect, radius, blur: 6, colour });
                }
                for thickness in [1, 2, 4, 9] {
                    ops.push(Op::StrokeEllipse { rect, thickness, colour });
                }
                for op in ops {
                    let bg = vec![Colour::WHITE.0; (cw * ch) as usize];
                    let (got, _) = render(&bg, cw, ch, scale, |c| op.draw(c));
                    for y in 0..ch {
                        for x in 0..cw {
                            let p = got[(y * cw + x) as usize];
                            assert_eq!(
                                p,
                                got[(y * cw + (cw - 1 - x)) as usize],
                                "{op:?}: left-right at {x},{y}"
                            );
                            assert_eq!(
                                p,
                                got[((ch - 1 - y) * cw + x) as usize],
                                "{op:?}: top-bottom at {x},{y}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_circle_is_the_same_as_a_rounded_rectangle_of_half_the_side() {
    for scale in 1..=3u32 {
        for side in (2..=40).step_by(2) {
            let (cw, ch) = ((side + 4) * scale as i32, (side + 4) * scale as i32);
            let bg = busy(cw, ch, side as u32);
            let rect = Rect::new(2, 2, side, side);
            let (circle, _) = render(&bg, cw, ch, scale, |c| c.fill_ellipse(rect, Colour::BLACK));
            let (round, _) =
                render(&bg, cw, ch, scale, |c| c.fill_round_rect(rect, side / 2, Colour::BLACK));
            assert_eq!(first_difference(&circle, &round, cw), None, "side {side} at {scale}");
            let (ring, _) =
                render(&bg, cw, ch, scale, |c| c.stroke_ellipse(rect, 3, Colour::BLACK));
            let (round_ring, _) = render(&bg, cw, ch, scale, |c| {
                c.stroke_round_rect(rect, side / 2, 3, Colour::BLACK)
            });
            assert_eq!(
                first_difference(&ring, &round_ring, cw),
                None,
                "ring, side {side} at {scale}"
            );
        }
    }
}

#[test]
fn a_line_with_the_ends_swapped_is_drawn_the_same_when_it_is_thick() {
    // Thin lines are not promised to be the same both ways (the classic walk breaks ties by
    // direction); thick ones are, because the capsule does not have a direction.
    let mut rng = Rng(77);
    for case in 0..300 {
        let (a, b) =
            ((rng.range(-5, 45), rng.range(-5, 45)), (rng.range(-5, 45), rng.range(-5, 45)));
        let width = rng.range(2, 9);
        let bg = busy(40, 40, case);
        let (one, _) = render(&bg, 40, 40, 1, |c| c.line(a, b, width, Colour(0x80_00_00_FF)));
        let (two, _) = render(&bg, 40, 40, 1, |c| c.line(b, a, width, Colour(0x80_00_00_FF)));
        assert_eq!(first_difference(&one, &two, 40), None, "{a:?} {b:?} {width}");
    }
}

#[test]
fn a_thick_horizontal_line_is_exactly_as_thick_as_asked() {
    // The half-pixel shift for even widths: 4 rows for 4, not 3 or 5, at every scale.
    for scale in 1..=4u32 {
        for width in 1..=6 {
            let (cw, ch) = (30 * scale as i32, 30 * scale as i32);
            let bg = vec![Colour::WHITE.0; (cw * ch) as usize];
            let (got, _) =
                render(&bg, cw, ch, scale, |c| c.line((5, 10), (20, 10), width, Colour::BLACK));
            let rows = (0..ch)
                .filter(|&y| got[(y * cw + 12 * scale as i32) as usize] != Colour::WHITE.0)
                .count();
            assert_eq!(rows, (width as u32 * scale) as usize, "width {width} at scale {scale}");
            let columns_in_middle_row = (0..cw)
                .filter(|&x| got[(10 * scale as i32 * cw + x) as usize] != Colour::WHITE.0)
                .count();
            // A horizontal line is as long as its centres' distance plus the round ends.
            assert!(columns_in_middle_row >= (15 * scale) as usize, "{width} at {scale}");
            // And the vertical one is just as thick.
            let (got, _) =
                render(&bg, cw, ch, scale, |c| c.line((10, 5), (10, 20), width, Colour::BLACK));
            let columns = (0..cw)
                .filter(|&x| got[(12 * scale as i32 * cw + x) as usize] != Colour::WHITE.0)
                .count();
            assert_eq!(
                columns,
                (width as u32 * scale) as usize,
                "vertical, width {width} at scale {scale}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Hostile arguments.

const GUARD: u32 = 0xDEAD_BEEF;

/// A canvas in the middle of a bigger buffer, with rows wider than the canvas, and a guard
/// value everywhere else. Anything a drawing call writes outside the canvas shows.
struct Frame {
    buf: Vec<u32>,
    lead: usize,
    w: u32,
    h: u32,
    stride: u32,
}

impl Frame {
    fn new(w: u32, h: u32) -> Frame {
        let (stride, lead, trail) = (w + 11, 37, 53);
        let mut buf = vec![GUARD; lead + (stride * h) as usize + trail];
        for y in 0..h {
            for x in 0..w {
                buf[lead + (y * stride + x) as usize] = 0xFF11_2233;
            }
        }
        Frame { buf, lead, w, h, stride }
    }

    fn canvas(&mut self, scale: u32) -> Canvas<'_> {
        Canvas::new(&mut self.buf[self.lead..], self.w, self.h, self.stride)
            .unwrap()
            .with_scale(scale)
    }

    /// Everything that is not a pixel of the canvas must still be the guard.
    fn check_guard(&self, what: &dyn std::fmt::Debug) {
        for (i, &p) in self.buf.iter().enumerate() {
            let inside = i >= self.lead
                && (i - self.lead) / (self.stride as usize) < self.h as usize
                && (i - self.lead) % (self.stride as usize) < self.w as usize;
            if !inside {
                assert_eq!(p, GUARD, "{what:?} wrote outside the canvas, at {i}");
            }
        }
    }
}

const HOSTILE: [i32; 15] = [
    i32::MIN,
    i32::MIN + 1,
    -10_000_000,
    -1000,
    -3,
    -1,
    0,
    1,
    3,
    20,
    1000,
    10_000_000,
    i32::MAX - 1,
    i32::MAX,
    7,
];

fn hostile(rng: &mut Rng) -> i32 {
    if rng.below(3) == 0 { rng.range(-5, 40) } else { rng.pick(&HOSTILE) }
}

#[test]
fn hostile_arguments_never_panic_and_never_leave_the_canvas() {
    let mut rng = Rng(0xFEED_FACE_0BAD_F00D);
    for scale in [1, 8] {
        let mut frame = Frame::new(20, 13);
        for case in 0..6000 {
            let rect = Rect::new(
                hostile(&mut rng),
                hostile(&mut rng),
                hostile(&mut rng),
                hostile(&mut rng),
            );
            let (colour, other) = (rng.colour(), rng.colour());
            let op = match case % 9 {
                0 => Op::Round { rect, radius: hostile(&mut rng), colour },
                1 => Op::StrokeRound {
                    rect,
                    radius: hostile(&mut rng),
                    thickness: hostile(&mut rng),
                    colour,
                },
                2 => Op::Ellipse { rect, colour },
                3 => Op::StrokeEllipse { rect, thickness: hostile(&mut rng), colour },
                4 => Op::Down { rect, top: colour, bottom: other },
                5 => Op::Across { rect, left: colour, right: other },
                6 => {
                    Op::Shadow { rect, radius: hostile(&mut rng), blur: hostile(&mut rng), colour }
                }
                7 => Op::Line {
                    from: (hostile(&mut rng), hostile(&mut rng)),
                    to: (hostile(&mut rng), hostile(&mut rng)),
                    width: hostile(&mut rng),
                    colour,
                },
                _ => Op::Flood { x: hostile(&mut rng), y: hostile(&mut rng), colour },
            };
            let mut damage = Damage::new();
            {
                let mut c = frame.canvas(scale).with_damage(&mut damage);
                op.draw(&mut c);
                // On a part of the canvas and clipped, too.
                op.draw(&mut c.sub(rect));
                op.draw(&mut c.clipped(rect));
            }
            frame.check_guard(&op);
            for r in damage.rects() {
                assert!(Rect::new(0, 0, 20, 13).contains_rect(r), "{op:?}: damage {r:?}");
            }
        }
    }
}

/// Shapes a hundred times over, at sizes in the millions, on a small canvas: the work must be
/// what the canvas needs, not what the shape is.
#[test]
fn enormous_shapes_cost_what_a_small_canvas_costs() {
    let rects = [
        Rect::new(-10_000_000, -10_000_000, 20_000_000, 20_000_000),
        Rect::new(-100, -100, i32::MAX, i32::MAX),
        Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        Rect::new(-5_000_000, 20, 10_000_000, 3),
        Rect::new(10, -5_000_000, 3, 10_000_000),
        Rect::new(-1_000_000, -1_000_000, 1_000_030, 1_000_030),
    ];
    let colour = Colour(0x80_20_40_60);
    let start = Instant::now();
    for scale in [1, 8] {
        let mut frame = Frame::new(64, 64);
        for rep in 0..100 {
            let mut c = frame.canvas(scale);
            for rect in rects {
                for radius in [0, 5, 1_000_000, i32::MAX] {
                    c.fill_round_rect(rect, radius, colour);
                    for thickness in [1, 3, 1_000_000, i32::MAX] {
                        c.stroke_round_rect(rect, radius, thickness, colour);
                    }
                    for blur in [8, 1_000_000, i32::MAX] {
                        c.shadow(rect, radius, blur, colour);
                    }
                }
                c.fill_ellipse(rect, colour);
                for thickness in [1, 3, 1_000_000, i32::MAX] {
                    c.stroke_ellipse(rect, thickness, colour);
                }
                c.gradient_down(rect, colour, Colour::WHITE);
                c.gradient_across(rect, colour, Colour::WHITE);
            }
            for (from, to) in [
                ((-10_000_000, -10_000_000), (10_000_000, 10_000_000)),
                ((i32::MIN, i32::MAX), (i32::MAX, i32::MIN)),
                ((-16_000_000, 30), (16_000_000, 33)),
                ((20, -16_000_000), (23, 16_000_000)),
                ((-1_000_000, 5), (1_000_000, 5)),
                ((5, 5), (5, 5)),
            ] {
                for width in [1, 2, 3, 1000, 1_000_000, i32::MAX] {
                    c.line(from, to, width, colour);
                }
            }
            // Fill and refill: each pass changes every pixel, so it works through all 4096.
            c.flood_fill(3, 3, Colour::solid(if rep % 2 == 0 { 0xFF0000 } else { 0x00FF00 }));
            frame.check_guard(&"the enormous shapes");
        }
    }
    let took = start.elapsed();
    println!("enormous shapes took {took:?}");
    assert!(took < std::time::Duration::from_secs(5), "took {took:?}");
}

/// Offscreen shapes draw nothing and record nothing.
#[test]
fn shapes_wholly_off_the_canvas_record_no_damage() {
    let colour = Colour::solid(0x336699);
    for scale in [1, 2] {
        for (x, y) in [(-100, 5), (100, 5), (5, -100), (5, 100), (-100, -100), (100, 100)] {
            let rect = Rect::new(x, y, 10, 10);
            for op in [
                Op::Round { rect, radius: 3, colour },
                Op::StrokeRound { rect, radius: 3, thickness: 2, colour },
                Op::Ellipse { rect, colour },
                Op::StrokeEllipse { rect, thickness: 2, colour },
                Op::Down { rect, top: colour, bottom: colour },
                Op::Across { rect, left: colour, right: colour },
                Op::Line { from: (x, y), to: (x + 5, y + 5), width: 1, colour },
                Op::Line { from: (x, y), to: (x + 5, y + 5), width: 4, colour },
                Op::Shadow { rect, radius: 3, blur: 4, colour },
            ] {
                let bg = busy(40, 40, 1);
                let (got, damage) = render(&bg, 40, 40, scale, |c| op.draw(c));
                assert_eq!(got, bg, "{op:?}");
                assert!(damage.is_empty(), "{op:?} recorded {:?}", damage.rects());
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Reference pictures. See `tests/pictures/draw/`.

/// 1.x's colours (`classic/kernel/wm.c`).
const GREEN: Colour = Colour::solid(0x2f6b55);
const DEEP_GREEN: Colour = Colour::solid(0x22503e);
const PINK: Colour = Colour::solid(0xd2577a);
const DARK: Colour = Colour::solid(0x2a2e3d);
const BLUE: Colour = Colour::solid(0x3d6fb5);
const GOLD: Colour = Colour::solid(0xe7b44a);

/// Every picture is 240 by 160 logical pixels; at 2× the same calls give 480 by 320.
fn check_picture(name: &str, draw: fn(&mut Canvas)) {
    for scale in [1u32, 2] {
        let mut sheet = Sheet::new(240 * scale, 160 * scale, PAPER);
        draw(&mut sheet.canvas().with_scale(scale));
        sheet.check(&format!("{name}-{scale}x"));
    }
}

/// 1.x's `ui_button`: three rounded rectangles, the last only on the lower half.
fn button(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, pressed: bool) {
    let colour = |hex| Colour::solid(hex);
    c.fill_round_rect(Rect::new(x, y, w, h), 4, colour(if pressed { 0x8d929c } else { 0xb4b8c0 }));
    let inner = Rect::new(x + 1, y + 1, w - 2, h - 2);
    if pressed {
        c.fill_round_rect(inner, 3, colour(0xd9dbe0));
    } else {
        c.fill_round_rect(inner, 3, colour(0xfdfdfd));
        c.clipped(Rect::new(x, y + h / 2, w, h)).fill_round_rect(inner, 3, colour(0xeceef1));
    }
}

fn draw_shapes(c: &mut Canvas) {
    // Corner radii 0, 3, 6, 10 and a full circle, then little ones.
    for (i, radius) in [0, 3, 6, 10].into_iter().enumerate() {
        c.fill_round_rect(Rect::new(6 + 36 * i as i32, 6, 30, 20), radius, GREEN);
    }
    c.fill_round_rect(Rect::new(152, 2, 28, 28), 14, PINK);
    for (i, size) in [3, 5, 8, 11].into_iter().enumerate() {
        c.fill_round_rect(Rect::new(188 + 13 * i as i32, 6, size, size), size / 2, DARK);
        c.fill_round_rect(Rect::new(188 + 13 * i as i32, 20, size, size), 1, BLUE);
    }
    // 1.x's buttons, and its round close buttons with the white cross.
    button(c, 6, 36, 60, 20, false);
    button(c, 72, 36, 60, 20, true);
    for (i, colour) in [0xe25c5c, 0xc5c8ce, 0xe7b44a].into_iter().enumerate() {
        let x = 142 + 24 * i as i32;
        c.fill_round_rect(Rect::new(x, 38, 16, 16), 8, Colour::solid(colour));
    }
    c.line((147, 43), (152, 48), 1, Colour::WHITE);
    c.line((152, 43), (147, 48), 1, Colour::WHITE);
    c.fill_rect(Rect::new(142 + 48 + 4, 45, 8, 2), Colour::WHITE);
    // Outlines: thickness 1, 2, 3 and 5.
    for (i, (thickness, radius)) in [(1, 6), (2, 6), (3, 6), (5, 12)].into_iter().enumerate() {
        let rect = Rect::new(6 + 58 * i as i32, 64, 52, 30);
        c.stroke_round_rect(rect, radius, thickness, DARK);
    }
    // Ellipses and circles, filled and outlined.
    c.fill_ellipse(Rect::new(6, 102, 44, 26), GREEN);
    c.fill_ellipse(Rect::new(56, 100, 28, 28), PINK);
    for (i, size) in [3, 4, 5, 7, 10].into_iter().enumerate() {
        c.fill_ellipse(Rect::new(92 + 11 * i as i32, 100, size, size), DARK);
    }
    c.stroke_ellipse(Rect::new(6, 132, 44, 24), 1, DARK);
    c.stroke_ellipse(Rect::new(56, 130, 28, 28), 3, BLUE);
    c.stroke_ellipse(Rect::new(92, 132, 44, 24), 6, PINK);
    // See-through shapes overlapping: each is one even tone where it lies over another.
    c.fill_round_rect(Rect::new(150, 106, 40, 30), 10, PINK.with_alpha(128));
    c.fill_round_rect(Rect::new(168, 120, 40, 30), 10, GREEN.with_alpha(128));
    c.fill_ellipse(Rect::new(190, 104, 40, 40), BLUE.with_alpha(128));
    c.stroke_round_rect(Rect::new(146, 100, 90, 56), 12, 4, GOLD.with_alpha(110));
}

#[test]
fn shapes_pictures() {
    check_picture("shapes", draw_shapes);
}

fn draw_gradients(c: &mut Canvas) {
    // A 1.x window's top: the title bar's gradient 26 high, the highlight under its top edge and
    // the frame, focused and not.
    for (i, (top, bottom, frame, light)) in
        [(0x2f6b55, 0x22503e, 0x1d3b30, 0x4b8a72), (0xaeb2ba, 0x989da7, 0x80858f, 0xc4c7cd)]
            .into_iter()
            .enumerate()
    {
        let x = 6 + 116 * i as i32;
        c.fill_rect(Rect::new(x, 6, 108, 40), PAPER);
        c.gradient_down(Rect::new(x, 6, 108, 26), Colour::solid(top), Colour::solid(bottom));
        c.fill_rect(Rect::new(x + 1, 7, 106, 1), Colour::solid(light));
        c.stroke_rect(Rect::new(x, 6, 108, 40), 1, Colour::solid(frame));
        c.fill_round_rect(
            Rect::new(x + 86, 11, 16, 16),
            8,
            Colour::solid(if i == 0 { 0xe25c5c } else { 0xc5c8ce }),
        );
    }
    // The taskbar, with the accent start button, and the start menu's banner.
    c.gradient_down(Rect::new(0, 122, 240, 38), DARK, Colour::solid(0x171a24));
    c.fill_round_rect(Rect::new(6, 127, 56, 28), 6, PINK);
    c.fill_round_rect(Rect::new(70, 127, 70, 28), 5, Colour::solid(0x3d4256));
    c.gradient_down(Rect::new(8, 54, 20, 60), PINK, Colour::solid(0x6a2a52));
    // Across: green to pink, black to white, and the smallest ones.
    c.gradient_across(Rect::new(36, 54, 90, 16), GREEN, PINK);
    c.gradient_across(Rect::new(36, 76, 90, 16), Colour::BLACK, Colour::WHITE);
    c.gradient_across(Rect::new(36, 98, 90, 1), GOLD, BLUE);
    c.gradient_across(Rect::new(36, 102, 2, 12), GOLD, BLUE);
    c.gradient_down(Rect::new(42, 102, 1, 12), GOLD, BLUE);
    c.gradient_down(Rect::new(48, 102, 12, 2), GOLD, BLUE);
    c.gradient_down(Rect::new(64, 102, 12, 12), GREEN, DEEP_GREEN);
    // See-through ones over shapes: clear on one side, nearly solid on the other.
    c.fill_ellipse(Rect::new(138, 52, 50, 50), PINK);
    c.fill_round_rect(Rect::new(176, 66, 54, 44), 8, GREEN);
    c.stroke_rect(Rect::new(134, 50, 100, 64), 2, DARK);
    c.gradient_across(Rect::new(134, 50, 100, 64), Colour(0x00FF_FFFF), Colour(0xE02A_2E3D));
    c.gradient_down(Rect::new(140, 100, 30, 12), Colour(0x00FF_FFFF), Colour(0xFF00_0000));
}

#[test]
fn gradient_pictures() {
    check_picture("gradient", draw_gradients);
}

fn draw_shadows(c: &mut Canvas) {
    // Black at alpha 96, offset down by 2, then the window on top, as 1.3's windows have.
    for (row, radius) in [0, 6].into_iter().enumerate() {
        for (column, blur) in [4, 8, 16].into_iter().enumerate() {
            let window = Rect::new(20 + 80 * column as i32, 22 + 80 * row as i32, 40, 32);
            c.shadow(window.translate(0, 2), radius, blur, Colour::BLACK.with_alpha(96));
            c.fill_round_rect(window, radius, Colour::WHITE);
            c.stroke_round_rect(window, radius, 1, Colour::solid(0xa9adb6));
        }
    }
}

#[test]
fn shadow_pictures() {
    check_picture("shadow", draw_shadows);
}

fn draw_lines(c: &mut Canvas) {
    // A fan of thin lines in every direction, 32 of them.
    let quarter = [(30, 0), (29, 8), (27, 13), (23, 19), (18, 24), (13, 27), (8, 29), (0, 30)];
    for (dx, dy) in quarter {
        for (sx, sy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            c.line((40, 40), (40 + sx * dx, 40 + sy * dy), 1, DARK);
        }
    }
    // Thick lines of width 2, 3, 5 and 9, horizontal, diagonal and steep.
    for (i, width) in [2, 3, 5, 9].into_iter().enumerate() {
        let x = 86 + 38 * i as i32;
        c.line((x, 12), (x + 26, 12), width, GREEN);
        c.line((x, 22), (x + 22, 44), width, PINK);
        c.line((x + 2, 24), (x + 8, 50), width, BLUE);
    }
    // Dots, from width 1 to 9.
    for width in 1..=9 {
        c.line((10 + 18 * (width - 1), 84), (10 + 18 * (width - 1), 84), width, DARK);
    }
    // The close button's cross, as 1.x draws it (thin, two lines doubled to be thicker).
    c.fill_round_rect(Rect::new(176, 76, 16, 16), 8, Colour::solid(0xe25c5c));
    for (a, b) in [
        ((181, 81), (186, 86)),
        ((186, 81), (181, 86)),
        ((182, 81), (186, 85)),
        ((185, 81), (181, 85)),
    ] {
        c.line(a, b, 1, Colour::WHITE);
    }
    // A see-through thick line over solid ones of several widths.
    c.line((10, 112), (110, 148), 5, PINK);
    c.line((10, 148), (110, 108), 3, GREEN);
    c.line((124, 130), (230, 130), 9, DARK);
    c.line((230, 106), (124, 150), 2, GOLD);
    c.line((60, 106), (210, 152), 11, BLUE.with_alpha(128));
    c.line((20, 100), (20, 100), 7, BLUE.with_alpha(128));
}

#[test]
fn line_pictures() {
    check_picture("line", draw_lines);
}

fn draw_flood(c: &mut Canvas) {
    // Outlined shapes, then each filled: the bucket stops at the outline.
    c.stroke_ellipse(Rect::new(6, 6, 70, 46), 2, DARK);
    c.flood_fill(40, 30, PINK);
    c.stroke_round_rect(Rect::new(84, 6, 70, 46), 10, 2, DARK);
    c.flood_fill(110, 30, GREEN);
    // A triangle of thick lines, and a house of thin ones.
    for (a, b) in [((172, 50), (200, 8)), ((200, 8), (232, 50)), ((232, 50), (172, 50))] {
        c.line(a, b, 2, DARK);
    }
    c.flood_fill(200, 35, GOLD);
    // A box with two overlapping circles: fill the box round them, one circle see-through, the
    // other solid, and where they overlap.
    c.stroke_rect(Rect::new(6, 60, 112, 92), 1, DARK);
    c.stroke_ellipse(Rect::new(14, 68, 52, 52), 2, DARK);
    c.stroke_ellipse(Rect::new(50, 94, 60, 50), 2, DARK);
    c.flood_fill(10, 64, Colour::solid(0xbfd9e6));
    c.flood_fill(30, 80, PINK.with_alpha(128));
    c.flood_fill(100, 130, GREEN);
    c.flood_fill(60, 104, GOLD);
    // Rings inside a box, each band its own colour; the middle see-through over the fill.
    c.stroke_rect(Rect::new(126, 60, 112, 96), 1, DARK);
    for (x, y, w, h) in [(130, 64, 104, 88), (146, 78, 72, 60), (162, 92, 40, 32)] {
        c.stroke_ellipse(Rect::new(x, y, w, h), 3, DARK);
    }
    c.flood_fill(128, 62, Colour::solid(0xfff2c6));
    c.flood_fill(138, 108, GREEN);
    c.flood_fill(152, 108, PINK);
    c.flood_fill(182, 108, DARK.with_alpha(128));
    c.flood_fill(182, 108, Colour::WHITE.with_alpha(100));
}

#[test]
fn flood_pictures() {
    check_picture("flood", draw_flood);
}
