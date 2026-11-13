// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drawing a window: its shadow and frame, title bar, icon and buttons, the app's picture
//! inside, and the window menu (docs/design/0022-display-server.md, section 6).
//!
//! The look is 1.3's, as `lib/draw/tests/look.rs` draws it (`draw_window` and `ui_button` in
//! `classic/kernel/wm.c`): a green title bar when the window has the focus, grey when it
//! hasn't, round buttons at the right. The system windows and trusted apps are told apart
//! from the rest by their title bars, which only this file draws: no app's picture can reach
//! outside its content rectangle, so none can forge either.
//!
//! **Where things are** comes only from `decor.rs` (`decor::frame`, `decor::button` and the
//! rest), the same functions `input.rs` uses to work out what a click hit, so that what is
//! drawn and what is clickable can't drift apart. Those give places in the *space*; every
//! drawing call here goes through [`Origin::rect`] to move them to the screen's canvas.

use super::{Frames, Origin};
use crate::decor::{self, Button};
use crate::desk::Menu;
use crate::scene::SceneWindow;
use alloc::string::String;
use pbr_draw::{Canvas, Colour, Rect, TextStyle};
use pbr_font::ABOVE;

/// The inside of a window the app hasn't (yet) drawn over.
pub(super) const BODY: Colour = Colour::solid(0xf5f2ec);
/// The ink of ordinary text on a light background.
const INK: Colour = Colour::solid(0x22252c);
/// How the shadow of a window or the window menu is drawn: `decor::SHADOW` allows for it.
const SHADOW: Colour = Colour::BLACK.with_alpha(70);

/// The colours of a title bar.
struct Look {
    top: Colour,
    bottom: Colour,
    /// The light line just under the top edge.
    shine: Colour,
    /// The line round the frame.
    edge: Colour,
    /// The title's colour.
    ink: Colour,
}

impl Look {
    /// A focused window's green title bar.
    const FOCUSED: Look = Look {
        top: Colour::solid(0x2f6b55),
        bottom: Colour::solid(0x22503e),
        shine: Colour::solid(0x4b8a72),
        edge: Colour::solid(0x1d3b30),
        ink: Colour::WHITE,
    };
    /// A window without the focus: grey.
    const UNFOCUSED: Look = Look {
        top: Colour::solid(0xaeb2ba),
        bottom: Colour::solid(0x989da7),
        shine: Colour::solid(0xc4c7cd),
        edge: Colour::solid(0x80858f),
        ink: Colour::solid(0x3d414a),
    };
    /// A system window with the focus: brown, so it can't be mistaken for an app's window.
    const SYSTEM_FOCUSED: Look = Look {
        top: Colour::solid(0x8a4b1f),
        bottom: Colour::solid(0x6d3a17),
        shine: Colour::solid(0xa86536),
        edge: Colour::solid(0x4f2a10),
        ink: Colour::WHITE,
    };
    /// A system window without the focus: grey, but its edge is still brown.
    const SYSTEM_UNFOCUSED: Look = Look { edge: Colour::solid(0x8a4b1f), ..Look::UNFOCUSED };

    fn of(w: &SceneWindow) -> &'static Look {
        match (w.system, w.focused) {
            (true, true) => &Look::SYSTEM_FOCUSED,
            (true, false) => &Look::SYSTEM_UNFOCUSED,
            (false, true) => &Look::FOCUSED,
            (false, false) => &Look::UNFOCUSED,
        }
    }
}

/// Draws window `w`, if any of it is in `seen` (the part of the space being drawn).
pub(super) fn draw(c: &mut Canvas, o: Origin, w: &SceneWindow, seen: Rect, frames: &dyn Frames) {
    // `decor`'s arithmetic is plain, so a window put absurdly far away (no real one is) is
    // left out before it is asked about, rather than overflowing there.
    const FAR: i32 = 1 << 30;
    let c0 = w.content;
    if c0.x.unsigned_abs() > FAR as u32 || c0.y.unsigned_abs() > FAR as u32 {
        return;
    }
    // A window off to the side costs nothing: not even its shadow is worked out.
    if decor::bounds(w.content, w.decorated).intersect(&seen).is_empty() {
        return;
    }
    if w.decorated {
        // The shadow goes first, under the frame, and is the frame moved a little down and
        // right and blurred.
        let frame = o.rect(decor::frame(w.content));
        c.shadow(frame.translate(2, 4), 0, 6, SHADOW);
    }
    content(c, o, w, frames);
    if w.decorated {
        decorations(c, o, w);
    }
}

/// The app's picture, and the window's body colour wherever the picture doesn't reach.
fn content(c: &mut Canvas, o: Origin, w: &SceneWindow, frames: &dyn Frames) {
    let area = o.rect(w.content);
    // `if let ... && let ...` runs the block only if both patterns match.
    let picture = match &w.shown {
        Some(s) => frames.picture(w.id, s).map(|p| {
            // While the app catches up with a resize its picture is a different size from the
            // content; it is stretched to the size it was drawn for, and cut to the content.
            let (width, height) = s.buffers.logical;
            (p, Rect::new(area.x, area.y, width, height))
        }),
        None => None,
    };
    let covered = picture.as_ref().map_or(Rect::EMPTY, |(_, dest)| dest.intersect(&area));
    // Only the part the picture won't cover is filled, so no pixel is written twice.
    if covered.is_empty() {
        c.fill_rect(area, BODY);
    } else {
        let (a, h) = (area, covered);
        for strip in [
            Rect::from_edges(a.x.into(), a.y.into(), a.right(), h.y.into()),
            Rect::from_edges(a.x.into(), h.bottom(), a.right(), a.bottom()),
            Rect::from_edges(a.x.into(), h.y.into(), h.x.into(), h.bottom()),
            Rect::from_edges(h.right(), h.y.into(), a.right(), h.bottom()),
        ] {
            c.fill_rect(strip, BODY);
        }
    }
    if let Some((picture, dest)) = picture {
        // `clipped` keeps the picture inside the content, whatever size it is.
        c.clipped(area).picture_fit(&picture, dest);
    }
}

/// The frame's edge, title bar, buttons, icon and title.
fn decorations(c: &mut Canvas, o: Origin, w: &SceneWindow) {
    let look = Look::of(w);
    let frame = decor::frame(w.content);
    let bar = o.rect(decor::title_bar(w.content));
    let f = o.rect(frame);
    c.gradient_down(bar, look.top, look.bottom);
    c.fill_rect(
        Rect::new(f.x.saturating_add(1), f.y.saturating_add(1), f.width.saturating_sub(2), 1),
        look.shine,
    );
    c.stroke_rect(f, 1, look.edge);

    for button in [Button::Close, Button::Maximise, Button::Minimise] {
        draw_button(c, o.rect(decor::button(w.content, button)), button, w);
    }
    if w.system {
        emblem(c, o.rect(decor::icon(w.content)));
    } else {
        letter(c, o.rect(decor::icon(w.content)), &w.title);
    }

    // The words stop short of the buttons, and of the "trusted" mark if there is one.
    let words = decor::text_area(w.content);
    let mut right = words.right();
    if w.trusted {
        let pill = trusted_pill(w.content);
        right = right.min(i64::from(pill.x) - 6);
        // Drawn even where the window is too narrow for it, over the icon: the mark must
        // always show. It is kept inside the frame.
        draw_trusted(&mut c.clipped(f), o.rect(pill), w.focused);
    }
    let room = o.rect(Rect::from_edges(words.x.into(), words.y.into(), right, words.bottom()));
    // Text sits 7 rows down the title bar's top, as in look.rs.
    c.clipped(room).text(
        o.rect(words).x,
        f.y.saturating_add(7),
        &w.title,
        look.ink,
        TextStyle::BOLD,
    );
}

/// The rounded mark "trusted" sits in, just left of the minimise button.
fn trusted_pill(content: Rect) -> Rect {
    let minimise = decor::button(content, Button::Minimise);
    let width = TextStyle::PLAIN.width("trusted") + 12;
    Rect::new(minimise.x.saturating_sub(6 + width), minimise.y + 1, width, decor::BUTTON - 2)
}

/// Draws the "trusted" mark in `pill`.
fn draw_trusted(c: &mut Canvas, pill: Rect, focused: bool) {
    let colour = Colour::solid(if focused { 0xe9f2ec } else { 0xd5d8de });
    c.fill_round_rect(pill, pill.height / 2, colour);
    let y = pill.y.saturating_add(centred(pill.height));
    c.text(pill.x.saturating_add(6), y, "trusted", INK, TextStyle::PLAIN);
}

/// Where the top of a text cell goes so that capital letters (7 rows tall, starting `ABOVE`
/// rows into their cell) are centred in a space `height` high.
pub(super) fn centred(height: i32) -> i32 {
    (height - 7) / 2 - ABOVE as i32
}

/// A title bar button: a round disc with a white mark on it.
fn draw_button(c: &mut Canvas, r: Rect, button: Button, w: &SceneWindow) {
    let mut disc = match (w.focused, button) {
        (false, _) => Colour::solid(0xc5c8ce),
        (true, Button::Close) => Colour::solid(0xe25c5c),
        (true, Button::Maximise) => Colour::solid(0x5cb85c),
        (true, Button::Minimise) => Colour::solid(0xe7b44a),
    };
    if w.pressed == Some(button) {
        // A quarter of the way to black.
        disc = Colour::between(disc, Colour::BLACK, 1, 4);
    }
    c.fill_round_rect(r, decor::BUTTON / 2, disc);
    let (x, y) = (r.x, r.y);
    match button {
        Button::Close => {
            // Two thin lines each way make a cross a little heavier than one line.
            for (from, to) in [
                ((x + 5, y + 5), (x + 10, y + 10)),
                ((x + 10, y + 5), (x + 5, y + 10)),
                ((x + 6, y + 5), (x + 10, y + 9)),
                ((x + 9, y + 5), (x + 5, y + 9)),
            ] {
                c.line(from, to, 1, Colour::WHITE);
            }
        }
        Button::Maximise if w.maximised => {
            // "Restore": a square in front of another. The front one is first filled with
            // the disc's colour, which hides the corner of the back one behind it.
            c.stroke_rect(Rect::new(x + 6, y + 4, 6, 5), 1, Colour::WHITE);
            c.fill_rect(Rect::new(x + 4, y + 6, 6, 5), disc);
            c.stroke_rect(Rect::new(x + 4, y + 6, 6, 5), 1, Colour::WHITE);
        }
        Button::Maximise => c.stroke_rect(Rect::new(x + 4, y + 5, 8, 6), 1, Colour::WHITE),
        Button::Minimise => c.fill_rect(Rect::new(x + 4, y + 7, 8, 2), Colour::WHITE),
    }
}

/// The icon of an app's window: a blue rounded square with the title's first letter.
fn letter(c: &mut Canvas, r: Rect, title: &str) {
    c.fill_round_rect(r, 3, Colour::solid(0x4a6fa5));
    let Some(first) = title.chars().next().filter(|ch| !ch.is_whitespace()) else { return };
    let mut buffer = [0u8; 4];
    let text: &str = first.encode_utf8(&mut buffer);
    // A bold letter's dots are 6 pixels across (5, and the same again one to the right).
    c.text(
        r.x.saturating_add(5),
        r.y.saturating_add(centred(r.height)),
        text,
        Colour::WHITE,
        TextStyle::BOLD,
    );
}

/// The icon of a system window: a boulder (the rock the system is named for), grey with a
/// lighter patch where the light falls and a flat bottom, so it sits.
fn emblem(c: &mut Canvas, r: Rect) {
    let mut part = c.clipped(Rect::new(r.x, r.y, r.width, r.height - 2));
    part.fill_ellipse(Rect::new(r.x, r.y + 1, r.width, r.height + 1), Colour::solid(0x8d929c));
    part.fill_ellipse(Rect::new(r.x + 3, r.y + 3, 6, 4), Colour::solid(0xc3c7ce));
}

/// Draws the window menu over everything.
pub(super) fn draw_menu(c: &mut Canvas, o: Origin, menu: &Menu, seen: Rect) {
    // The shadow reaches a few pixels further than the panel.
    if decor::menu_bounds(menu.rect).intersect(&seen).is_empty() {
        return;
    }
    let panel = o.rect(menu.rect);
    c.shadow(panel.translate(2, 4), 0, 6, SHADOW);
    c.fill_rect(panel, Colour::WHITE);
    c.stroke_rect(panel, 1, Colour::solid(0xb4b8c0));
    let mut inside = c.clipped(panel);
    for (i, item) in menu.items.iter().enumerate() {
        let top =
            i32::try_from(i).unwrap_or(i32::MAX).saturating_mul(decor::MENU_ROW).saturating_add(2);
        let row = Rect::new(
            panel.x.saturating_add(1),
            panel.y.saturating_add(top),
            panel.width.saturating_sub(2),
            decor::MENU_ROW,
        );
        let hovered = menu.hover == Some(i);
        if hovered {
            inside.fill_rect(row, Colour::solid(0x2f6b55));
        }
        let label: String = item.label();
        let ink = if hovered { Colour::WHITE } else { INK };
        inside.text(
            panel.x.saturating_add(10),
            row.y.saturating_add(centred(row.height)),
            &label,
            ink,
            TextStyle::PLAIN,
        );
    }
}
