// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the drawing tests share: a [`Sheet`] of pixels to compare with the reference pictures
//! kept in `tests/pictures/desktop/` (docs/design/0023-draw-window-and-sandstone.md, section 6).
//!
//! After a change to the look that is meant: `PBR_UPDATE_PICTURES=1 cargo test -p
//! pbr-desktop`, then look at each picture that changed before committing it.

// Each test file uses its own part of this.
#![allow(dead_code)]

use pbr_draw::Canvas;
use std::path::PathBuf;

/// Pixels of a screen or a window, owned by the test.
#[derive(Clone)]
pub struct Sheet {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

impl Sheet {
    /// A sheet of one colour (`0x00RRGGBB`).
    pub fn new(width: u32, height: u32, rgb: u32) -> Sheet {
        Sheet { pixels: vec![rgb; (width * height) as usize], width, height }
    }

    /// A canvas on the sheet, at `scale`.
    pub fn canvas(&mut self, scale: u32) -> Canvas<'_> {
        Canvas::new(&mut self.pixels, self.width, self.height, self.width)
            .expect("a canvas")
            .with_scale(scale)
    }

    /// Compares the sheet with the reference picture `name`, and fails the test with what
    /// differs if it isn't the same, pixel for pixel. The wallpaper's pixels are `0x00RRGGBB`
    /// and drawing sets the spare byte, so the byte is made 255 first: only the colours count.
    pub fn check(&self, name: &str) {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/pictures/desktop")
            .join(format!("{name}.png"));
        let got = pbr_testpng::Picture {
            width: self.width,
            height: self.height,
            pixels: self.pixels.iter().map(|p| p | 0xFF00_0000).collect(),
        };
        if let Err(why) = pbr_testpng::check(&file, &got) {
            panic!("{name}: {why}");
        }
    }
}
