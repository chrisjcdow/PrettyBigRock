// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the picture tests share: a sheet to draw on, and comparing it with the reference
//! picture kept in `tests/pictures/draw/` (docs/design/0023-draw-window-and-sandstone.md,
//! section 6).
//!
//! A test draws on a [`Sheet`] and calls [`Sheet::check`] with the picture's name. The
//! reference pictures are PNG files, so a person can look at them, and a change to how
//! something is drawn shows up as a changed picture in the commit. To write them afresh after
//! a change that is meant: `PBR_UPDATE_PICTURES=1 cargo xtask test`, then look at each one
//! that changed before committing it.

// Each test file uses its own part of this.
#![allow(dead_code)]

use pbr_draw::{Canvas, Colour};
use std::path::PathBuf;

/// The colour the sheets start as: 1.x's window body, so shapes are seen as they will be used.
pub const PAPER: Colour = Colour::solid(0xf5f2ec);

/// Pixels to draw on, owned by the test.
pub struct Sheet {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

impl Sheet {
    /// A sheet of one colour.
    pub fn new(width: u32, height: u32, colour: Colour) -> Sheet {
        Sheet { pixels: vec![colour.0; (width * height) as usize], width, height }
    }

    /// A canvas over the whole sheet, at scale 1 (use `with_scale` for another).
    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(&mut self.pixels, self.width, self.height, self.width).unwrap()
    }

    /// Compares the sheet with the reference picture `name`, and fails the test with what
    /// differs if it isn't the same, pixel for pixel.
    pub fn check(&self, name: &str) {
        // A canvas is always solid: anything else here means some drawing call stored a
        // pixel without setting its spare byte.
        if let Some(at) = self.pixels.iter().position(|p| p >> 24 != 0xFF) {
            let (x, y) = (at as u32 % self.width, at as u32 / self.width);
            panic!("{name}: the pixel at {x},{y} isn't solid: {:#010x}", self.pixels[at]);
        }
        // `env!` is filled in by the compiler: the folder of this crate's Cargo.toml.
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/pictures/draw")
            .join(format!("{name}.png"));
        let got = pbr_testpng::Picture {
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
        };
        if let Err(why) = pbr_testpng::check(&file, &got) {
            panic!("{name}: {why}");
        }
    }
}
