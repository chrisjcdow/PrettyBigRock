// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Image`]: a small picture the app gives a widget (an icon on a button, in a list row, in a
//! menu), borrowed from the app's own pixels.

use crate::element::Look;
use crate::geometry::Rect;
use pbr_draw::{Canvas, Picture};

/// A picture of `width` by `height` pixels, `0xAARRGGBB`, row after row: alpha 0 is clear, 255
/// solid. Usually an icon kept in the app as a `const` array of 16 x 16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image<'a> {
    pixels: &'a [u32],
    width: u32,
    height: u32,
}

impl<'a> Image<'a> {
    /// The picture, or `None` if `pixels` is shorter than `width` times `height` (or either is
    /// 0).
    pub fn new(pixels: &'a [u32], width: u32, height: u32) -> Option<Image<'a>> {
        let need = (width as usize).checked_mul(height as usize)?;
        (need > 0 && pixels.len() >= need).then_some(Image { pixels, width, height })
    }

    /// Its width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Its height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Draws it scaled to fill `dest` (logical pixels): each of its pixels a logical pixel when
    /// `dest` is its size, so at 2x each is a block of four, as crisp as the font.
    pub fn draw(&self, canvas: &mut Canvas<'_>, dest: Rect) {
        if let Some(p) = Picture::new(self.pixels, self.width, self.height, self.width) {
            canvas.picture_fit(&p.with_alpha(), dest);
        }
    }

    /// Adds what it looks like to a widget's look.
    pub fn look(&self, look: Look) -> Look {
        let n = (self.width * self.height) as usize;
        let mut look = look.u64(u64::from(self.width)).u64(u64::from(self.height));
        for p in &self.pixels[..n] {
            look = look.bytes(&p.to_le_bytes());
        }
        look
    }
}
