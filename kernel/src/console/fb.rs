// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A text console drawn on a framebuffer, with the 1.x font.

use super::font;
use pbr_bootinfo::Framebuffer;

/// Background: the dusk blue of the 1.x desktop.
const BACKGROUND: (u8, u8, u8) = (0x1B, 0x26, 0x3B);
/// Ordinary text.
const TEXT: (u8, u8, u8) = (0xE8, 0xEC, 0xF1);
/// The first line.
const TITLE: (u8, u8, u8) = (0xF2, 0xB8, 0x4B);

/// A screen of text.
pub struct FbConsole {
    base: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    fb: Framebuffer,
    scale: usize,
    col: usize,
    row: usize,
    lines: usize,
}

impl FbConsole {
    /// # Safety
    /// `base` must be the framebuffer's memory, mapped writable, used by nothing else.
    pub unsafe fn new(fb: Framebuffer, base: u64) -> Option<Self> {
        let bpp = usize::from(fb.format.bits_per_pixel).div_ceil(8);
        if !(2..=4).contains(&bpp) || fb.width < 64 || fb.height < 64 {
            return None;
        }
        let scale = if fb.width >= 2560 {
            3
        } else if fb.width >= 1024 {
            2
        } else {
            1
        };
        let mut c = Self {
            base: base as *mut u8,
            width: fb.width as usize,
            height: fb.height as usize,
            stride: fb.stride as usize,
            bytes_per_pixel: bpp,
            fb,
            scale,
            col: 0,
            row: 0,
            lines: 0,
        };
        c.fill(0, 0, c.width, c.height, BACKGROUND);
        Some(c)
    }

    fn pixel(&self, (r, g, b): (u8, u8, u8)) -> u32 {
        let f = &self.fb.format;
        let chan = |v: u8, shift: u8, size: u8| {
            if size == 0 { 0 } else { (u32::from(v) >> (8 - size.min(8))) << shift }
        };
        chan(r, f.red_shift, f.red_size)
            | chan(g, f.green_shift, f.green_size)
            | chan(b, f.blue_shift, f.blue_size)
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, colour: (u8, u8, u8)) {
        let p = self.pixel(colour).to_le_bytes();
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                let o = yy * self.stride + xx * self.bytes_per_pixel;
                for (i, byte) in p.iter().take(self.bytes_per_pixel).enumerate() {
                    // SAFETY: (xx, yy) is inside the screen, so the byte is inside the
                    // framebuffer `new`'s caller gave us.
                    unsafe { self.base.add(o + i).write_volatile(*byte) };
                }
            }
        }
    }

    fn cell_w(&self) -> usize {
        (font::WIDTH + 1) * self.scale
    }
    fn cell_h(&self) -> usize {
        (font::HEIGHT + 2) * self.scale
    }
    fn margin(&self) -> usize {
        8 * self.scale
    }
    fn cols(&self) -> usize {
        (self.width - 2 * self.margin()) / self.cell_w()
    }
    fn rows(&self) -> usize {
        (self.height - 2 * self.margin()) / self.cell_h()
    }

    fn draw(&mut self, c: char) {
        let colour = if self.lines == 0 { TITLE } else { TEXT };
        let (x0, y0) =
            (self.margin() + self.col * self.cell_w(), self.margin() + self.row * self.cell_h());
        let s = self.scale;
        for (dy, bits) in font::glyph(c).iter().enumerate() {
            for dx in 0..font::WIDTH {
                if bits & (0x10 >> dx) != 0 {
                    self.fill(x0 + dx * s, y0 + dy * s, s, s, colour);
                }
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        self.lines += 1;
        if self.row + 1 < self.rows() {
            self.row += 1;
            return;
        }
        // Scroll up one line.
        let (top, ch) = (self.margin(), self.cell_h());
        let span = self.width * self.bytes_per_pixel;
        for y in top..top + (self.rows() - 1) * ch {
            // SAFETY: both rows are inside the framebuffer, and they don't overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.base.add((y + ch) * self.stride),
                    self.base.add(y * self.stride),
                    span,
                );
            }
        }
        let y = top + (self.rows() - 1) * ch;
        self.fill(0, y, self.width, ch, BACKGROUND);
    }

    /// Writes text, wrapping at the right edge.
    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => self.col = 0,
                c => {
                    if self.col >= self.cols() {
                        self.newline();
                    }
                    self.draw(c);
                    self.col += 1;
                }
            }
        }
    }
}
