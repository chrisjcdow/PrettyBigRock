// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Everything together: a window as 1.3 drew it (`draw_window` and `ui_button` in
//! `classic/kernel/wm.c`), made with this library's calls, at 1× and 2×.
//!
//! The other picture tests each show one kind of drawing. This one shows that they add up to
//! the look the desktop needs (docs/design/0026-apps-m6-tests-and-stages.md, section 7:
//! "double-buffered drawing, alpha, gradients, rounded shapes, the font"), and gives a person
//! one picture to judge that look by. The display server and sandstone will draw the real
//! thing (notes 0022 and 0023); the colours and sizes here are 1.3's.

mod common;

use common::Sheet;
use pbr_draw::{Canvas, Colour, Rect, TextStyle};

/// 1.3's title bar height.
const TITLE: i32 = 26;

/// A push button, as 1.3's `ui_button`: a grey rim, a white face, and a slightly darker lower
/// half, which is drawn by clipping the same shape to the bottom half.
fn button(c: &mut Canvas, area: Rect, label: &str, pressed: bool) {
    let rim = if pressed { 0x8d929c } else { 0xb4b8c0 };
    c.fill_round_rect(area, 4, Colour::solid(rim));
    let face = area.inset(1);
    if pressed {
        c.fill_round_rect(face, 3, Colour::solid(0xd9dbe0));
    } else {
        c.fill_round_rect(face, 3, Colour::solid(0xfdfdfd));
        let lower = Rect::new(area.x, area.y + area.height / 2, area.width, area.height);
        c.clipped(lower).fill_round_rect(face, 3, Colour::solid(0xeceef1));
    }
    let nudge = i32::from(pressed);
    let x = area.x + (area.width - TextStyle::PLAIN.width(label)) / 2 + nudge;
    // The letters' own rows start two rows down their cell (the two above are for accents).
    let y = area.y + (area.height - 9) / 2 - 2 + nudge;
    c.text(x, y, label, Colour::solid(0x22252c), TextStyle::PLAIN);
}

/// A window with its shadow, frame, title bar and buttons. `c`'s (0, 0) is where the window's
/// corner goes.
fn window(c: &mut Canvas, width: i32, height: i32, title: &str, focused: bool) {
    let frame = Rect::new(0, 0, width, height);
    // 1.3 drew its shadow as five see-through black rectangles, each a pixel further down and
    // right; a soft shadow under the window is the same idea with one call.
    c.shadow(frame.translate(2, 4), 0, 6, Colour::BLACK.with_alpha(70));
    c.fill_rect(frame, Colour::solid(0xf5f2ec));
    let (top, bottom, shine, edge, ink) = if focused {
        (0x2f6b55, 0x22503e, 0x4b8a72, 0x1d3b30, 0xffffff)
    } else {
        (0xaeb2ba, 0x989da7, 0xc4c7cd, 0x80858f, 0x3d414a)
    };
    c.gradient_down(Rect::new(0, 0, width, TITLE), Colour::solid(top), Colour::solid(bottom));
    c.fill_rect(Rect::new(1, 1, width - 2, 1), Colour::solid(shine));
    c.stroke_rect(frame, 1, Colour::solid(edge));
    // The title can't run under the buttons.
    c.clipped(Rect::new(0, 0, width - 50, TITLE)).text(
        10,
        7,
        title,
        Colour::solid(ink),
        TextStyle::BOLD,
    );
    // Close: a red disc with a white cross of two thin lines each way, as 1.3's.
    let (x, y) = (width - 22, 5);
    let close = if focused { 0xe25c5c } else { 0xc5c8ce };
    c.fill_round_rect(Rect::new(x, y, 16, 16), 8, Colour::solid(close));
    for (from, to) in [
        ((x + 5, y + 5), (x + 10, y + 10)),
        ((x + 10, y + 5), (x + 5, y + 10)),
        ((x + 6, y + 5), (x + 10, y + 9)),
        ((x + 9, y + 5), (x + 5, y + 9)),
    ] {
        c.line(from, to, 1, Colour::WHITE);
    }
    // Minimise: a yellow disc with a bar.
    let x = x - 22;
    let minimise = if focused { 0xe7b44a } else { 0xc5c8ce };
    c.fill_round_rect(Rect::new(x, y, 16, 16), 8, Colour::solid(minimise));
    c.fill_rect(Rect::new(x + 4, y + 7, 8, 2), Colour::WHITE);
}

/// The whole picture: 300 by 190 logical pixels of dusk-blue desktop with two windows.
fn draw(c: &mut Canvas) {
    c.gradient_down(Rect::new(0, 0, 300, 190), Colour::solid(0x1b263b), Colour::solid(0x40304e));
    {
        let mut back = c.sub(Rect::new(14, 12, 190, 110));
        window(&mut back, 190, 110, "Notepad", false);
        let ink = Colour::solid(0x22252c);
        back.text(8, TITLE + 6, "Named for Table Mountain,", ink, TextStyle::PLAIN);
        back.text(8, TITLE + 17, "a pretty big rock.", ink, TextStyle::PLAIN);
    }
    let mut front = c.sub(Rect::new(84, 58, 200, 118));
    window(&mut front, 200, 118, "Save changes? \u{2014} caf\u{e9}.txt", true);
    let ink = Colour::solid(0x22252c);
    front.text(12, TITLE + 10, "\u{201c}caf\u{e9}.txt\u{201d} has changes", ink, TextStyle::PLAIN);
    front.text(12, TITLE + 21, "that aren\u{2019}t saved yet.", ink, TextStyle::PLAIN);
    front.text(12, TITLE + 38, "Save", ink, TextStyle::PLAIN.sized(2));
    button(&mut front, Rect::new(12, 86, 56, 22), "Save", false);
    button(&mut front, Rect::new(74, 86, 56, 22), "Discard", true);
    button(&mut front, Rect::new(136, 86, 52, 22), "Cancel", false);
}

#[test]
fn a_window_as_1_3_drew_it_at_scale_1() {
    let mut sheet = Sheet::new(300, 190, Colour::BLACK);
    draw(&mut sheet.canvas());
    sheet.check("look-1x");
}

#[test]
fn a_window_as_1_3_drew_it_at_scale_2() {
    let mut sheet = Sheet::new(600, 380, Colour::BLACK);
    draw(&mut sheet.canvas().with_scale(2));
    sheet.check("look-2x");
}
