// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Composing: drawing a screen of the desktop from a [`Scene`]
//! (docs/design/0022-display-server.md, section 7).
//!
//! *Composing* means putting the desktop together from its parts, back to front: the plain
//! desktop colour, each window (shadow, frame, title bar and the app's own picture), the
//! window menu, and the pointer on top. The result goes into a screen's buffer, which the
//! display driver then shows (note 0020).
//!
//! **Logical and device pixels.** The scene is in *logical* pixels of the one space all the
//! screens share (note 0022, section 2). A screen at 2x has two device pixels, the real dots
//! of its buffer, to each logical one. `lib/draw`'s [`Canvas`] does that multiplying when it
//! is made `with_scale`; this file only has to move the space's coordinates so that the
//! screen's top left corner is the canvas's (0, 0) ([`Origin`]).
//!
//! **Three kinds of screen.** An ordinary one is drawn straight into its buffer, and, when only
//! some rectangles changed, only inside those. A *turned* one (rotation 90, 180 or 270) is
//! drawn the right way up into memory the caller lends ([`compose`]'s `scratch`) and then
//! copied, turned, into the buffer. A *mirroring* one shows another screen's part of the
//! space, shrunk or stretched to fit and centred. Both of those are drawn whole every time:
//! they are rare, and a turned copy can't be patched in a corner.
//!
//! The submodules draw [`window`]s and the pointer.

mod pointer;
mod window;

use crate::desk::{Farewell, Shown, WindowId};
use crate::scene::Scene;
use crate::space::{Screen, Space};
use alloc::vec::Vec;
use pbr_draw::{Canvas, Colour, Damage, MAX_RECTS, Picture, Rect, TextStyle, Turn};

/// The desktop where no window is.
const DESKTOP: Colour = Colour::solid(0x1b263b);
/// The farewell screen.
const FAREWELL: Colour = Colour::solid(0x101418);
/// The most pixels of scratch memory composing will ask for (256 megabytes): more than any
/// screen the drawing library can make a canvas of is worth, but it keeps a lying driver
/// from making the display server ask for gigabytes.
const MAX_SCRATCH: u64 = 1 << 26;

/// How composing gets at windows' pixels: the display server maps each window's buffers.
pub trait Frames {
    /// The picture of window `id`'s frame `shown.frame` of the buffers `shown.buffers`
    /// (`shown.buffers.width` x `height` pixels, rows `stride` bytes apart), or `None` if it
    /// can't be had.
    ///
    /// (A *trait* is Rust's way to say "anything that can do this": the display server
    /// implements it over real shared memory, the tests over plain arrays.)
    fn picture(&self, id: WindowId, shown: &Shown) -> Option<Picture<'_>>;
}

/// A screen's buffer, as the display driver lends it: `height` rows of `width` 32-bit pixels
/// (blue, green, red, unused byte), each row `stride` PIXELS after the one before.
pub struct Target<'a> {
    /// The pixels. (`&mut` is a unique borrow: nothing else touches them meanwhile.)
    pub pixels: &'a mut [u32],
    /// Pixels in a row that are shown.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// Pixels from the start of one row to the start of the next.
    pub stride: u32,
}

/// Where the screen's top left corner is in the space: subtracted from every place in the
/// space to give a place on the screen's canvas.
#[derive(Clone, Copy)]
pub(crate) struct Origin {
    x: i32,
    y: i32,
}

impl Origin {
    /// A rectangle of the space, on the canvas.
    pub(crate) fn rect(self, r: Rect) -> Rect {
        r.translate(self.x.saturating_neg(), self.y.saturating_neg())
    }

    /// A point of the space, on the canvas.
    pub(crate) fn point(self, x: i32, y: i32) -> (i32, i32) {
        (x.saturating_sub(self.x), y.saturating_sub(self.y))
    }
}

/// Draws screen `screen` of `space` from `scene` into `target`, only where `areas` (rectangles
/// in the space, in logical pixels) touch it, or all of it if `areas` is `None`. Returns the
/// rectangles of `target` that changed, in device pixels (at most 64; merged with
/// `pbr_draw::Damage` if there are more). `scratch` is memory the caller keeps between calls,
/// for screens that are turned or mirror another.
///
/// Nothing the scene holds can make this panic or draw outside `target`: places and sizes may
/// be nonsense (they come from apps), and `lib/draw` cuts everything to the buffer. A screen
/// that doesn't exist, or a buffer too small for its own `width`, `height` and `stride`,
/// draws nothing.
pub fn compose(
    scene: &Scene,
    space: &Space,
    screen: u32,
    target: Target<'_>,
    areas: Option<&[Rect]>,
    frames: &dyn Frames,
    scratch: &mut Vec<u32>,
) -> Vec<Rect> {
    let Some(screen) = space.screen(screen) else { return Vec::new() };
    let Target { pixels, width, height, stride } = target;
    let Some(canvas) = Canvas::new(pixels, width, height, stride) else { return Vec::new() };
    let whole = Rect::new(0, 0, width as i32, height as i32);
    // `width` and `height` are at most 16384 here, so they fit an `i32`.
    if screen.place.mirrors != 0 {
        mirror(canvas, space, screen, scene, frames, scratch);
        return alloc::vec![whole];
    }
    let turn = match screen.place.rotation {
        90 => Some(Turn::Right),
        180 => Some(Turn::Half),
        270 => Some(Turn::Left),
        _ => None,
    };
    if let Some(turn) = turn {
        turned(canvas, screen, turn, scene, frames, scratch);
        return alloc::vec![whole];
    }
    straight(canvas, screen, scene, areas, frames, whole)
}

/// An ordinary screen: straight into the buffer, inside `areas`.
fn straight(
    canvas: Canvas<'_>,
    screen: &Screen,
    scene: &Scene,
    areas: Option<&[Rect]>,
    frames: &dyn Frames,
    whole: Rect,
) -> Vec<Rect> {
    let factor = screen.factor();
    let rect = screen.rect();
    let origin = Origin { x: rect.x, y: rect.y };
    let mut canvas = canvas.with_scale(factor as u32);
    let Some(areas) = areas else {
        draw_scene(&mut canvas, origin, scene, rect, rect, frames);
        return alloc::vec![whole];
    };
    let mut drawn = Vec::new();
    for area in inside(areas, rect) {
        // `clipped` is a canvas that can draw only inside the area: whatever the scene
        // draws elsewhere is cut off, which is what keeps a small change cheap.
        let local = origin.rect(area);
        let mut part = canvas.clipped(local);
        draw_scene(&mut part, origin, scene, rect, area, frames);
        let device = Rect::new(
            local.x.saturating_mul(factor),
            local.y.saturating_mul(factor),
            local.width.saturating_mul(factor),
            local.height.saturating_mul(factor),
        );
        let device = device.intersect(&whole);
        if !device.is_empty() {
            drawn.push(device);
        }
    }
    drawn
}

/// The parts of `areas` that are on the screen (`rect`), at most [`MAX_RECTS`] of them: more
/// are joined, which draws a little more than was asked.
fn inside(areas: &[Rect], rect: Rect) -> Vec<Rect> {
    let mut list: Vec<Rect> =
        areas.iter().map(|a| a.intersect(&rect)).filter(|a| !a.is_empty()).collect();
    if list.len() > MAX_RECTS {
        let mut damage = Damage::new();
        list.iter().for_each(|a| damage.add(*a));
        list = damage.rects().to_vec();
    }
    list
}

/// Lends `scratch` out as `width` by `height` pixels, grown if need be; `None` if that is
/// empty or more than [`MAX_SCRATCH`].
fn lend(scratch: &mut Vec<u32>, width: u32, height: u32) -> Option<&mut [u32]> {
    let n = u64::from(width) * u64::from(height);
    if n == 0 || n > MAX_SCRATCH {
        return None;
    }
    let n = n as usize;
    if scratch.len() < n {
        scratch.resize(n, 0);
    }
    Some(&mut scratch[..n])
}

/// Draws the part of the space `screen` shows, the right way up, into `scratch`, at the
/// screen's scale: the logical size times the factor, each way. Returns the size in pixels.
fn draw_upright(
    screen: &Screen,
    scene: &Scene,
    frames: &dyn Frames,
    scratch: &mut Vec<u32>,
) -> Option<(u32, u32)> {
    let factor = screen.factor() as u32;
    let rect = screen.rect();
    let (width, height) = (rect.width.max(0) as u32 * factor, rect.height.max(0) as u32 * factor);
    let pixels = lend(scratch, width, height)?;
    let mut canvas = Canvas::new(pixels, width, height, width)?.with_scale(factor);
    draw_scene(&mut canvas, Origin { x: rect.x, y: rect.y }, scene, rect, rect, frames);
    Some((width, height))
}

/// A turned screen: drawn upright into `scratch`, then copied turned.
fn turned(
    mut canvas: Canvas<'_>,
    screen: &Screen,
    turn: Turn,
    scene: &Scene,
    frames: &dyn Frames,
    scratch: &mut Vec<u32>,
) {
    // Black first: the turned picture may be a few pixels short of the buffer (a mode that
    // isn't a whole number of logical pixels).
    canvas.fill(Colour::BLACK);
    let Some((width, height)) = draw_upright(screen, scene, frames, scratch) else { return };
    let Some(picture) = Picture::new(&scratch[..(width * height) as usize], width, height, width)
    else {
        return;
    };
    canvas.picture_turned(&picture, 0, 0, turn);
}

/// A mirroring screen: what the screen it mirrors shows, as big as fits, in the middle, with
/// black round it. The screen's own scale and rotation mean nothing, and the screen mirrored
/// is shown upright whatever its rotation (the space is what is shown).
fn mirror(
    mut canvas: Canvas<'_>,
    space: &Space,
    screen: &Screen,
    scene: &Scene,
    frames: &dyn Frames,
    scratch: &mut Vec<u32>,
) {
    canvas.fill(Colour::BLACK);
    let Some(source) = space.shows(screen.number) else { return };
    let Some((sw, sh)) = draw_upright(source, scene, frames, scratch) else { return };
    let Some(picture) = Picture::new(&scratch[..(sw * sh) as usize], sw, sh, sw) else { return };
    let (tw, th) = canvas.device_size();
    let (tw, th, sw, sh) = (i64::from(tw), i64::from(th), i64::from(sw), i64::from(sh));
    // Fit by the side that runs out first (cross-multiplying compares the ratios exactly).
    let (dw, dh) = if tw * sh <= th * sw { (tw, sh * tw / sw) } else { (sw * th / sh, th) };
    let dest = Rect::new(((tw - dw) / 2) as i32, ((th - dh) / 2) as i32, dw as i32, dh as i32);
    canvas.picture_fit(&picture, dest);
}

/// The whole scene on `c`, back to front. `screen` is the screen's part of the space, `seen`
/// the part of it that is being drawn (what is outside it is not worked out at all), and `c`
/// can be clipped to the same.
fn draw_scene(
    c: &mut Canvas,
    o: Origin,
    scene: &Scene,
    screen: Rect,
    seen: Rect,
    frames: &dyn Frames,
) {
    if let Some(farewell) = scene.farewell {
        c.fill(FAREWELL);
        let lines: &[&str] = match farewell {
            Farewell::Restarting => &["Restarting\u{2026}"],
            Farewell::ShuttingDown => &["Shutting down\u{2026}"],
            Farewell::Halted => {
                &["PrettyBigRock has shut down.", "You can turn the computer off now."]
            }
        };
        let style = TextStyle::PLAIN.sized(2);
        let area = o.rect(screen);
        // Size 2 doubles the 7-row capitals, so the centring is done on twice the height;
        // lines are a line height apart.
        let n = lines.len() as i32;
        let block = 14 + (n - 1) * style.line_height();
        let top = area.y.saturating_add((area.height - block) / 2 - 2 * pbr_font::ABOVE as i32);
        for (i, text) in lines.iter().enumerate() {
            let x = area.x.saturating_add((area.width - style.width(text)) / 2);
            c.text(x, top + i as i32 * style.line_height(), text, Colour::WHITE, style);
        }
        return;
    }
    c.fill(DESKTOP);
    for w in &scene.windows {
        window::draw(c, o, w, seen, frames);
    }
    if let Some(menu) = &scene.menu {
        window::draw_menu(c, o, menu, seen);
    }
    if let Some((x, y, cursor)) = scene.pointer {
        pointer::draw(c, o, x, y, cursor, seen);
    }
}
