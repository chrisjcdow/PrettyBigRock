// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The session's desktop (docs/design/0024-the-session-desktop.md, section 6), as plain code
//! built and tested on the host; `apps/desktop` is the program around it.
//!
//! This is the drawing half, carried over from PrettyBigRock 1.3's `classic/kernel/wm.c`:
//!
//! - [`look`]: 1.3's colours;
//! - [`wallpaper`]: Table Mountain at dusk;
//! - [`icons`]: the 16 x 16 pictures;
//! - [`shortcuts`]: the app icons down the desktop's left edge;
//! - [`taskbar`]: the start button, a button for each window, and the clock;
//! - [`menu`]: the start menu.
//!
//! and the rest of what the desktop works out:
//!
//! - [`screens`]: its screen arrangement, `screens.txt`;
//! - [`clock`]: the taskbar's time and date.
//!
//! Each part says where things are (so the app can tell what a click hit) as well as how to
//! draw them, and the two come from the same numbers, so a click always lands on what was drawn.
//! Every position is in *logical pixels* (see `pbr_draw::Canvas`: the canvas multiplies by its
//! scale, so the same code draws at 1x and 2x).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod clock;
pub mod icons;
pub mod look;
pub mod menu;
pub mod screens;
pub mod shortcuts;
pub mod taskbar;
pub mod wallpaper;

use pbr_draw::{Canvas, Colour, TextStyle};
use pbr_font::ABOVE;

/// Draws text where 1.3 would have: 1.3's font started its capitals on the glyph's first row,
/// and ours has `ABOVE` empty rows over them (room for accents), so the text is drawn that many
/// dots higher. `y` is where 1.3 passed to `gfx_text`; this keeps every label where it was.
pub(crate) fn text(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    text: &str,
    colour: Colour,
    style: TextStyle,
) {
    let up = (ABOVE as i32).saturating_mul(style.size.clamp(1, 16) as i32);
    canvas.text(x, y.saturating_sub(up), text, colour, style);
}
