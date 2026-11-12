// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The canvas's own drawing (filled and outlined rectangles, see-through colours, the clip,
//! parts of a canvas) against reference pictures, at 1× and 2×.

mod common;

use common::{PAPER, Sheet};
use pbr_draw::{Canvas, Colour, Rect};

/// 1.x's colours: the title bar's green, the accent pink and the taskbar's dark blue.
const GREEN: Colour = Colour::solid(0x2f6b55);
const PINK: Colour = Colour::solid(0xd2577a);
const DARK: Colour = Colour::solid(0x2a2e3d);

/// The same drawing calls for every scale: 100 by 60 logical pixels.
fn draw(c: &mut Canvas) {
    // Solid rectangles, one hanging off the left edge and one off the bottom.
    c.fill_rect(Rect::new(6, 6, 30, 20), GREEN);
    c.fill_rect(Rect::new(-10, 34, 24, 12), PINK);
    c.fill_rect(Rect::new(40, 50, 12, 40), DARK);
    // See-through ones over them and over each other: 25%, 50% and 75% solid.
    c.fill_rect(Rect::new(20, 14, 30, 30), PINK.with_alpha(64));
    c.fill_rect(Rect::new(28, 20, 30, 30), DARK.with_alpha(128));
    c.fill_rect(Rect::new(36, 26, 30, 30), Colour::WHITE.with_alpha(191));
    // Frames: thin, thick, see-through, and one so thick that it is filled.
    c.stroke_rect(Rect::new(70, 4, 26, 16), 1, DARK);
    c.stroke_rect(Rect::new(72, 6, 22, 12), 3, PINK);
    c.stroke_rect(Rect::new(70, 24, 26, 16), 4, GREEN.with_alpha(128));
    c.stroke_rect(Rect::new(70, 44, 10, 10), 5, DARK);
    // A part of the canvas: it draws from its own corner and can't leave its area.
    let mut part = c.sub(Rect::new(84, 44, 12, 12));
    part.fill(GREEN);
    part.fill_rect(Rect::new(-3, -3, 9, 9), PINK);
    part.stroke_rect(Rect::new(2, 2, 100, 100), 2, DARK);
}

#[test]
fn rectangles_at_scale_1() {
    let mut sheet = Sheet::new(100, 60, PAPER);
    draw(&mut sheet.canvas());
    sheet.check("canvas-1x");
}

#[test]
fn rectangles_at_scale_2() {
    let mut sheet = Sheet::new(200, 120, PAPER);
    draw(&mut sheet.canvas().with_scale(2));
    sheet.check("canvas-2x");
    // With only rectangles, 2× is exactly 1× with every pixel doubled each way.
    let mut small = Sheet::new(100, 60, PAPER);
    draw(&mut small.canvas());
    for y in 0..120 {
        for x in 0..200 {
            assert_eq!(
                sheet.pixels[y * 200 + x],
                small.pixels[y / 2 * 100 + x / 2],
                "the pixel at {x},{y}"
            );
        }
    }
}

/// Drawing with a clip gives exactly the part of the unclipped drawing that is inside it.
#[test]
fn a_clip_cuts_and_never_changes_what_is_drawn() {
    let mut whole = Sheet::new(100, 60, PAPER);
    draw(&mut whole.canvas());
    for clip in [
        Rect::new(0, 0, 100, 60),
        Rect::new(10, 10, 50, 30),
        Rect::new(33, 0, 1, 60),
        Rect::new(0, 29, 100, 1),
        Rect::new(-20, -20, 60, 60),
        Rect::new(80, 40, 500, 500),
        Rect::new(50, 30, 0, 0),
    ] {
        let mut cut = Sheet::new(100, 60, PAPER);
        draw(&mut cut.canvas().clipped(clip));
        for y in 0..60 {
            for x in 0..100 {
                let inside = clip.contains(x, y);
                let want = if inside { whole.pixels[(y * 100 + x) as usize] } else { PAPER.0 };
                assert_eq!(cut.pixels[(y * 100 + x) as usize], want, "{clip:?}: {x},{y}");
            }
        }
    }
}
