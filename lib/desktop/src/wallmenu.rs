// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The small menu on a right click on the wallpaper, with one item, Screens…
//! (docs/design/0024-the-session-desktop.md, section 6).
//!
//! It is drawn in the same way as the start menu (`menu.rs`): a popup window that is exactly
//! the menu, in the start menu's paper, text and highlight, so the two look alike. A *popup* is
//! a window the display server closes by itself when the user clicks elsewhere; the desktop
//! opens one over the wallpaper with the serial of the click (`apps/desktop`).

use crate::look;
use pbr_draw::{Canvas, Rect, TextStyle};

/// The only item's words.
pub const LABEL: &str = "Screens\u{2026}";
/// The menu's size, in logical pixels: the item's row and the room around it.
const WIDTH: i32 = 132;
const PAD: i32 = 4;
const ROW: i32 = 26;
/// Where the label starts, from the left edge.
const TEXT_X: i32 = 14;

/// The menu: whether its item is under the pointer decides how it is drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct WallMenu {
    /// The item is under the pointer, so it is drawn highlighted.
    pub hovered: bool,
}

impl WallMenu {
    /// Its size in logical pixels.
    pub fn size() -> (i32, i32) {
        (WIDTH, ROW + 2 * PAD)
    }

    /// Whether (`x`, `y`), in the menu's own logical pixels, is on its item.
    pub fn hits(x: i32, y: i32) -> bool {
        let (w, h) = Self::size();
        x >= 0 && x < w && y >= PAD && y < h - PAD
    }

    /// Draws it into `canvas` (sized [`WallMenu::size`], any scale) at 0, 0. The four corners
    /// outside the rounded shape are left as they were.
    pub fn draw(&self, canvas: &mut Canvas<'_>) {
        let (w, h) = Self::size();
        canvas.fill_round_rect(Rect::new(0, 0, w, h), 6, look::C_MENU);
        if self.hovered {
            canvas.fill_round_rect(Rect::new(PAD, PAD, w - 2 * PAD, ROW), 4, look::C_MENU_HOVER);
        }
        crate::text(canvas, TEXT_X, PAD + 9, LABEL, look::C_MENU_TEXT, TextStyle::PLAIN);
    }
}
