// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A text console drawn on a framebuffer, with the 1.x font.
//!
//! The screen's memory is never read back (note 0027, section 5). Firmware screens are mapped
//! write-combining: writes are gathered and sent to the graphics card in bursts, which is fast,
//! but every read goes to the card on its own and is very slow, so scrolling by copying the
//! screen upwards would take a tenth of a second a line on a real PC. Instead the text on the
//! screen is kept in ordinary memory ([`Text`]), and scrolling redraws only the cells whose
//! letter changes.

// The font is shared with the console service, so both draw the same letters.
use pbr_bootinfo::Framebuffer;
use pbr_font as font;

/// Background: the dusk blue of the 1.x desktop.
const BACKGROUND: (u8, u8, u8) = (0x1B, 0x26, 0x3B);
/// Ordinary text.
const TEXT: (u8, u8, u8) = (0xE8, 0xEC, 0xF1);
/// The first line.
const TITLE: (u8, u8, u8) = (0xF2, 0xB8, 0x4B);

/// The most columns and rows kept; a larger screen uses only this much of itself.
const MAX_COLS: usize = 256;
const MAX_ROWS: usize = 128;

/// What is on the screen, cell by cell, and which rows are in the first line's colour. It
/// lives in the console's `static`, not on a stack, since it is large (128 KiB).
pub struct Text {
    cells: [[char; MAX_COLS]; MAX_ROWS],
    title: [bool; MAX_ROWS],
}

impl Text {
    /// An empty screen. `const` so it can start a `static`.
    pub const fn new() -> Self {
        Self { cells: [[' '; MAX_COLS]; MAX_ROWS], title: [false; MAX_ROWS] }
    }
}

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

    /// Clears `text` to match the screen [`FbConsole::new`] just cleared. In place: a new
    /// `Text` would be built on the stack first, and the loader's stack, which the kernel is
    /// still on, is too small for it.
    pub fn clear(&self, text: &mut Text) {
        text.cells.iter_mut().for_each(|row| row.fill(' '));
        text.title.fill(false);
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
        let pixel = self.pixel(colour);
        let p = pixel.to_le_bytes();
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                let o = yy * self.stride + xx * self.bytes_per_pixel;
                if self.bytes_per_pixel == 4 && o.is_multiple_of(4) {
                    // SAFETY: as below, and the four bytes are aligned.
                    unsafe { self.base.add(o).cast::<u32>().write_volatile(pixel) };
                    continue;
                }
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
        // The font's 11 rows already hold the space between lines: its top two are empty
        // except for the accents of capital letters (lib/font).
        font::HEIGHT * self.scale
    }
    fn margin(&self) -> usize {
        8 * self.scale
    }
    fn cols(&self) -> usize {
        ((self.width - 2 * self.margin()) / self.cell_w()).min(MAX_COLS)
    }
    fn rows(&self) -> usize {
        ((self.height - 2 * self.margin()) / self.cell_h()).min(MAX_ROWS)
    }

    /// Draws `c` in the cell at (`col`, `row`), over whatever was there.
    fn draw_at(&mut self, col: usize, row: usize, c: char, title: bool) {
        let colour = if title { TITLE } else { TEXT };
        let (x0, y0) = (self.margin() + col * self.cell_w(), self.margin() + row * self.cell_h());
        let (w, h) = (self.cell_w(), self.cell_h());
        self.fill(x0, y0, w, h, BACKGROUND);
        let s = self.scale;
        for (dy, bits) in font::glyph(c).iter().enumerate() {
            for dx in 0..font::WIDTH {
                if bits & (0x10 >> dx) != 0 {
                    self.fill(x0 + dx * s, y0 + dy * s, s, s, colour);
                }
            }
        }
    }

    fn newline(&mut self, text: &mut Text) {
        self.col = 0;
        self.lines += 1;
        if self.row + 1 < self.rows() {
            self.row += 1;
            return;
        }
        // Scroll up one line: each row takes the one below's text, redrawing only the cells
        // that change, then the last row is emptied.
        let (rows, cols) = (self.rows(), self.cols());
        for r in 0..rows {
            let title = r + 1 < rows && text.title[r + 1];
            for c in 0..cols {
                let new = if r + 1 < rows { text.cells[r + 1][c] } else { ' ' };
                if new != text.cells[r][c] || (title != text.title[r] && new != ' ') {
                    self.draw_at(c, r, new, title);
                    text.cells[r][c] = new;
                }
            }
            text.title[r] = title;
        }
    }

    /// Writes text, wrapping at the right edge.
    pub fn write_str(&mut self, s: &str, text: &mut Text) {
        for c in s.chars() {
            match c {
                '\n' => self.newline(text),
                '\r' => self.col = 0,
                c => {
                    if self.col >= self.cols() {
                        self.newline(text);
                    }
                    let (col, row, title) = (self.col, self.row, self.lines == 0);
                    self.draw_at(col, row, c, title);
                    text.cells[row][col] = c;
                    text.title[row] = title;
                    self.col += 1;
                }
            }
        }
    }
}
