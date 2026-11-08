// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drawing text as pixels: how big the characters are on a screen of a given size, and what
//! each row of pixels in a row of text looks like.
//!
//! It copies the kernel's own text console (`kernel/src/console/fb.rs`): the same font, the
//! same colours, the same scale and margins, so the screen looks the same when the console
//! service takes over from the kernel. The console service only has to copy the pixels this
//! makes into the screen's memory, which is the one part that needs `unsafe`; everything about
//! *which* pixels is here, where the host can test it.
//!
//! A *framebuffer* is the screen's memory: one number per pixel, row after row. Here a pixel is
//! always 32 bits, with the colour in the lowest three bytes in one of two orders
//! ([`PixelOrder`]), as `pbr.display` says.

use crate::grid::MAX_CELLS;
use pbr_font::{HEIGHT, WIDTH, glyph};

/// The background: the dusk blue of the 1.x desktop.
pub const BACKGROUND: (u8, u8, u8) = (0x1B, 0x26, 0x3B);
/// Ordinary text (and the cursor).
pub const TEXT: (u8, u8, u8) = (0xE8, 0xEC, 0xF1);

/// The biggest screen accepted, in pixels each way. Far bigger than any real one; it keeps
/// the arithmetic here from ever overflowing, whatever a driver says.
pub const MAX_SIDE: u32 = 16_384;

/// How the three colours of a 32-bit pixel are laid out (`PixelFormat` in `pbr.display`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    /// Blue in the lowest byte, then green, then red.
    Bgrx,
    /// Red in the lowest byte, then green, then blue.
    Rgbx,
}

/// How text is laid out on a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Pixels across.
    pub width: usize,
    /// Pixels down.
    pub height: usize,
    /// Bytes from the start of one row of pixels to the next.
    pub stride: usize,
    /// How a pixel's colours are laid out.
    pub order: PixelOrder,
    /// How many pixels wide and tall each dot of the font is.
    pub scale: usize,
    /// Pixels across one character cell: the glyph and a column of space.
    pub cell_width: usize,
    /// Pixels down one character cell: the glyph and two rows of space.
    pub cell_height: usize,
    /// Pixels of space around the text on every side.
    pub margin: usize,
    /// Characters across.
    pub columns: usize,
    /// Characters down.
    pub rows: usize,
}

impl Layout {
    /// The layout for a screen of `width` by `height` pixels, with rows `stride` bytes apart.
    /// `None` if the screen cannot be used: no room for a single character, not 32-bit pixels
    /// that rows can hold, or sizes that are beyond [`MAX_SIDE`] (a driver that lies must not
    /// make the console draw outside the screen's memory).
    pub fn new(width: u32, height: u32, stride: u32, order: PixelOrder) -> Option<Layout> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        // Whole 32-bit pixels, so that they can be written as numbers; and a row of pixels
        // fits in a row of the screen's memory.
        if !stride.is_multiple_of(4) || u64::from(stride) < u64::from(width) * 4 {
            return None;
        }
        // The kernel's rule: bigger screens get bigger letters.
        let scale = if width >= 2560 {
            3
        } else if width >= 1024 {
            2
        } else {
            1
        };
        let (cell_width, cell_height, margin) =
            ((WIDTH + 1) * scale, (HEIGHT + 2) * scale, 8 * scale);
        let columns = (width as usize).checked_sub(2 * margin)? / cell_width;
        let rows = (height as usize).checked_sub(2 * margin)? / cell_height;
        if columns == 0 || rows == 0 || columns * rows > MAX_CELLS {
            return None;
        }
        Some(Layout {
            width: width as usize,
            height: height as usize,
            stride: stride as usize,
            order,
            scale,
            cell_width,
            cell_height,
            margin,
            columns,
            rows,
        })
    }

    /// How many bytes of the screen's memory the layout uses: all its rows.
    pub fn bytes(&self) -> u64 {
        self.stride as u64 * self.height as u64
    }

    /// The pixel number for a colour, as it is stored in memory (a 32-bit number written
    /// little-endian, which is how both screens and processors here do it).
    pub fn pixel(&self, (r, g, b): (u8, u8, u8)) -> u32 {
        let (r, g, b) = (u32::from(r), u32::from(g), u32::from(b));
        match self.order {
            PixelOrder::Bgrx => b | g << 8 | r << 16,
            PixelOrder::Rgbx => r | g << 8 | b << 16,
        }
    }

    /// Pixels across the text: what [`Layout::render_line`] fills in.
    pub fn text_width(&self) -> usize {
        self.columns * self.cell_width
    }

    /// Where a row of text starts on the screen: (x, y) in pixels of its top left corner.
    pub fn origin(&self, row: usize) -> (usize, usize) {
        (self.margin, self.margin + row * self.cell_height)
    }

    /// Makes one line of pixels, line `line` (from 0 to `cell_height`) of a row of text, into
    /// `out`, which must be [`Layout::text_width`] long (it is cut or left alone otherwise).
    /// `chars` is the row of the grid: characters missing at the end are spaces, and ones the
    /// font cannot draw show as `?`. With `caret`, that column has the cursor, an underline
    /// along the bottom of the cell.
    pub fn render_line(&self, chars: &[char], caret: Option<usize>, line: usize, out: &mut [u32]) {
        let (background, text) = (self.pixel(BACKGROUND), self.pixel(TEXT));
        let glyph_row = line / self.scale;
        for (column, cell) in out.chunks_mut(self.cell_width).take(self.columns).enumerate() {
            let c = chars.get(column).copied().unwrap_or(' ');
            // The glyph is `HEIGHT` rows; the rows after it are the space between lines.
            let bits = if glyph_row < HEIGHT { glyph(c)[glyph_row] } else { 0 };
            let underline = caret == Some(column) && glyph_row == HEIGHT + 1;
            for (x, pixel) in cell.iter_mut().enumerate() {
                let dot = x / self.scale;
                let lit = underline || (dot < WIDTH && bits & (0x10 >> dot) != 0);
                *pixel = if lit { text } else { background };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    fn layout(width: u32, height: u32) -> Layout {
        Layout::new(width, height, width * 4, PixelOrder::Bgrx).unwrap()
    }

    #[test]
    fn the_letters_are_as_big_as_the_kernel_makes_them() {
        // The kernel's rule: scale 1 below 1024 wide, 2 from 1024, 3 from 2560.
        for (width, scale) in
            [(64, 1), (800, 1), (1023, 1), (1024, 2), (1280, 2), (2559, 2), (2560, 3), (3840, 3)]
        {
            assert_eq!(layout(width, 1000).scale, scale, "{width}");
        }
        let l = layout(1280, 800);
        assert_eq!((l.cell_width, l.cell_height, l.margin), (12, 22, 16));
        assert_eq!((l.columns, l.rows), ((1280 - 32) / 12, (800 - 32) / 22));
        let l = layout(800, 600);
        assert_eq!((l.cell_width, l.cell_height, l.margin), (6, 11, 8));
        assert_eq!((l.columns, l.rows), (130, 53));
        assert_eq!(l.text_width(), 780);
        assert_eq!(l.origin(2), (8, 8 + 22));
    }

    #[test]
    fn a_screen_that_cannot_be_used_is_refused() {
        let ok = |w, h, s| Layout::new(w, h, s, PixelOrder::Rgbx);
        assert!(ok(800, 600, 3200).is_some());
        assert!(ok(800, 600, 4096).is_some(), "rows may be padded");
        assert!(ok(0, 600, 3200).is_none());
        assert!(ok(800, 0, 3200).is_none());
        assert!(ok(800, 600, 3199).is_none(), "a row is shorter than its pixels");
        assert!(ok(800, 600, 3202).is_none(), "not whole pixels");
        assert!(ok(800, 600, 0).is_none());
        assert!(ok(16, 600, 64).is_none(), "no room inside the margins");
        assert!(ok(21, 600, 84).is_none(), "room for margins and no letter");
        assert!(ok(22, 600, 88).is_some(), "room for exactly one");
        assert!(ok(800, 16, 3200).is_none());
        assert!(ok(MAX_SIDE + 1, 600, u32::MAX - 3).is_none());
        assert!(ok(800, MAX_SIDE + 1, 3200).is_none());
        assert!(ok(u32::MAX, u32::MAX, u32::MAX).is_none());
        assert!(ok(800, 600, u32::MAX).is_none());
        assert!(ok(800, 600, u32::MAX - 3).is_some(), "the service checks the memory is there");
        assert_eq!(ok(800, 600, 3200).unwrap().bytes(), 3200 * 600);
        // The biggest screen allowed is still a sane grid.
        assert!(ok(MAX_SIDE, MAX_SIDE, MAX_SIDE * 4).is_some());
    }

    #[test]
    fn colours_are_packed_in_the_screens_order() {
        let (r, g, b) = (0x11, 0x22, 0x33);
        let bgrx = Layout::new(800, 600, 3200, PixelOrder::Bgrx).unwrap();
        let rgbx = Layout::new(800, 600, 3200, PixelOrder::Rgbx).unwrap();
        // As bytes in memory: blue first, or red first; the fourth byte is 0.
        assert_eq!(bgrx.pixel((r, g, b)).to_le_bytes(), [b, g, r, 0]);
        assert_eq!(rgbx.pixel((r, g, b)).to_le_bytes(), [r, g, b, 0]);
    }

    /// The pixels of one text row, for a layout, as lines of '#' (text) and '.' (background).
    fn picture(l: &Layout, chars: &[char], caret: Option<usize>) -> Vec<String> {
        let (bg, fg) = (l.pixel(BACKGROUND), l.pixel(TEXT));
        (0..l.cell_height)
            .map(|line| {
                let mut out = vec![0xDEAD_BEEF; l.text_width()];
                l.render_line(chars, caret, line, &mut out);
                out.iter()
                    .map(|&p| match p {
                        p if p == fg => '#',
                        p if p == bg => '.',
                        p => panic!("a pixel that is neither colour: {p:#x}"),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_letter_is_its_glyph() {
        let l = layout(60, 60); // scale 1: 7 columns of 6 pixels
        assert_eq!(l.scale, 1);
        let rows = picture(&l, &['A'], None);
        assert_eq!(rows.len(), HEIGHT + 2);
        // 'A' is 0e 11 11 1f 11 11 11 in the font; its cell has a column of space after it.
        assert_eq!(&rows[0][..6], ".###..");
        assert_eq!(&rows[1][..6], "#...#.");
        assert_eq!(&rows[3][..6], "#####.");
        assert_eq!(&rows[6][..6], "#...#.");
        assert_eq!(&rows[7][..6], "......", "the rows below the letters are blank");
        assert_eq!(&rows[10][..6], "......");
        // The cells after it are spaces, because the row had nothing more.
        assert!(rows.iter().all(|r| r[6..].chars().all(|c| c == '.')));
        assert!(rows.iter().all(|r| r.len() == 6 * l.columns));
    }

    #[test]
    fn bigger_screens_draw_each_dot_bigger() {
        let big = layout(1100, 600);
        assert_eq!(big.scale, 2);
        let small = layout(60, 60);
        let (b, s) = (picture(&big, &['g', 'Q'], None), picture(&small, &['g', 'Q'], None));
        for line in 0..big.cell_height {
            for cell in 0..2 {
                for x in 0..big.cell_width {
                    assert_eq!(
                        b[line].as_bytes()[cell * big.cell_width + x],
                        s[line / 2].as_bytes()[cell * small.cell_width + x / 2],
                        "line {line}, cell {cell}, x {x}"
                    );
                }
            }
        }
        // And scale 3.
        let huge = layout(2600, 600);
        assert_eq!(huge.scale, 3);
        let h = picture(&huge, &['W'], None);
        let w = picture(&small, &['W'], None);
        assert_eq!(h[3 * 3].as_bytes()[..3], w[3].as_bytes()[..1].repeat(3)[..]);
    }

    #[test]
    fn every_printable_character_is_drawn_and_the_rest_is_a_question_mark() {
        let l = layout(60, 60);
        for c in ' '..='~' {
            let got = picture(&l, &[c], None);
            let rows: Vec<u8> = got
                .iter()
                .map(|r| r[..5].bytes().fold(0, |a, b| a << 1 | u8::from(b == b'#')))
                .collect();
            assert_eq!(rows[..HEIGHT], glyph(c)[..], "{c:?}");
        }
        assert_eq!(picture(&l, &['\u{e9}'], None), picture(&l, &['?'], None));
        assert_eq!(picture(&l, &['\n'], None), picture(&l, &['?'], None));
    }

    #[test]
    fn the_cursor_is_an_underline_under_its_column_only() {
        let l = layout(60, 60);
        let plain = picture(&l, &['a', 'b', 'c'], None);
        let with = picture(&l, &['a', 'b', 'c'], Some(1));
        for line in 0..l.cell_height {
            if line == HEIGHT + 1 {
                assert_eq!(&with[line][6..12], "######");
                assert_eq!(&with[line][..6], &plain[line][..6]);
                assert_eq!(&with[line][12..], &plain[line][12..]);
            } else {
                assert_eq!(with[line], plain[line], "line {line}");
            }
        }
        // On a bigger screen the underline is as thick as a dot.
        let big = layout(1100, 600);
        let with = picture(&big, &[], Some(0));
        let thick: Vec<usize> =
            (0..big.cell_height).filter(|&i| with[i].starts_with('#')).collect();
        assert_eq!(thick, [20, 21]);
        // A cursor at a column past the end of the row's text is drawn too, on a space.
        assert!(picture(&l, &['a'], Some(3))[HEIGHT + 1].contains("######"));
        // And one past the last column of the screen is not drawn at all.
        assert_eq!(picture(&l, &[], Some(l.columns + 5)), picture(&l, &[], None));
    }

    #[test]
    fn rendering_never_goes_outside_its_buffer() {
        let l = layout(60, 60);
        // Too long, too short, empty, and a line number beyond the cell: nothing panics.
        for len in [0, 1, 5, 6, 7, l.text_width() - 1, l.text_width(), l.text_width() + 100] {
            let mut out = vec![7u32; len];
            for line in [0, 5, l.cell_height - 1, l.cell_height, 1000, usize::MAX] {
                l.render_line(&['x'; 300], Some(2), line, &mut out);
            }
        }
        let mut out = vec![7u32; l.text_width() + 100];
        l.render_line(&['x'], None, 0, &mut out);
        assert!(
            out[l.text_width()..].iter().all(|&p| p == 7),
            "the buffer past the text is left alone"
        );
    }
}
