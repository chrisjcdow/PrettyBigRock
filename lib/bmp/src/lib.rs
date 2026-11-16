// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading and writing Windows bitmaps (`.bmp` files), as PrettyBigRock 1.3's Paint did
//! (docs/design/0026-apps-m6-tests-and-stages.md, section 1; 1.3's `save_answer` and
//! `paint_open_file` in `classic/kernel/apps.c`).
//!
//! A bitmap file is a small header followed by the picture's pixels. [`encode`] writes the
//! one kind 1.3 wrote, byte for byte: 24 bits a pixel, no compression, the bottom row first
//! (that is how Windows stores them), each row padded with zeros to a multiple of four bytes.
//! [`decode`] reads what 1.3 read: that kind, and 32 bits a pixel, with the rows in either
//! order.
//!
//! **Files are hostile input** (note 0002). A file may say it is four billion pixels wide, or
//! that its pixels start past its end, or just be cut short. [`decode`] does every sum on
//! numbers wide enough that they cannot overflow, checks the claimed size against the file's
//! real length and against [`MAX_PIXELS`] before it allocates anything, and never panics, whatever
//! the bytes. (In Rust, reading past the end of a slice stops the program with a *panic*, so
//! every length is checked before the slice is indexed, and the tests feed the code thousands
//! of damaged files to be sure.)
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries (not the
//! operating system's), so a PrettyBigRock program can use it; the tests use the standard
//! library.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

/// The most pixels a picture may have (64 million: 8000 by 8000). A bigger file is refused
/// before any memory is set aside for it.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// The size of the file header and the info header that 1.3 wrote: where its pixels start.
const HEADER: usize = 54;
/// How many pixels a metre 1.3 wrote (72 to the inch).
const PIXELS_PER_METRE: u32 = 2835;

/// A picture: `width` by `height` pixels of `0x00RRGGBB`, row after row, the top row first. This
/// is the layout sandstone's clipboard uses for pictures too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Picture {
    /// The picture, or `None` if it has no pixels at all, is more than [`MAX_PIXELS`], or
    /// `pixels` isn't exactly `width * height` long. (That check is what lets everything else
    /// in this library trust a `Picture`.) Whatever is in each pixel's top byte is dropped.
    pub fn new(width: u32, height: u32, mut pixels: Vec<u32>) -> Option<Picture> {
        let count = u64::from(width) * u64::from(height);
        if count == 0 || count > MAX_PIXELS || pixels.len() as u64 != count {
            return None;
        }
        for p in &mut pixels {
            *p &= 0x00FF_FFFF;
        }
        Some(Picture { width, height, pixels })
    }

    /// How many pixels wide.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// How many pixels high.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, `0x00RRGGBB`, row after row, the top row first.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Gives the pixels up, to keep them without copying.
    pub fn into_pixels(self) -> Vec<u32> {
        self.pixels
    }
}

/// Why a file can't be opened as a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BmpError {
    /// It doesn't start with `BM`: it isn't a bitmap at all.
    NotABitmap,
    /// It is a bitmap, but not a kind this library opens: the text says what.
    Unsupported(&'static str),
    /// The file ends before the header or the pixels it describes do.
    CutShort,
    /// More than [`MAX_PIXELS`] pixels.
    TooBig,
}

impl fmt::Display for BmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BmpError::NotABitmap => write!(f, "it is not a bitmap"),
            BmpError::Unsupported(what) => write!(f, "{what}"),
            BmpError::CutShort => write!(f, "the file ends too soon"),
            BmpError::TooBig => write!(f, "the picture is too big"),
        }
    }
}

/// Bytes in one row of a 24-bit picture `width` pixels wide: three a pixel, rounded up to a
/// multiple of four. `u64`, so that no `u32` width can overflow it.
fn row_bytes(width: u32, bytes_per_pixel: u32) -> u64 {
    (u64::from(width) * u64::from(bytes_per_pixel)).div_ceil(4) * 4
}

/// Writes `picture` as 1.3's Paint did: 24 bits a pixel, uncompressed, bottom row first.
///
/// The header is 1.3's, field for field: the sizes, `54` where the pixels start, a 40-byte info
/// header, one plane, 2835 pixels a metre each way (72 to the inch), and zeros everywhere
/// else. Padding bytes are zero.
pub fn encode(picture: &Picture) -> Vec<u8> {
    let row = row_bytes(picture.width, 3) as usize;
    // `Picture::new` kept the pixels to 64 million, so this is at most about 256 MB: it
    // fits a `u32` (the header's size fields) as well as a `usize`.
    let data = row * picture.height as usize;
    let mut out = alloc::vec![0u8; HEADER + data];
    out[0] = b'B';
    out[1] = b'M';
    put32(&mut out, 2, (HEADER + data) as u32);
    put32(&mut out, 10, HEADER as u32);
    put32(&mut out, 14, 40);
    put32(&mut out, 18, picture.width);
    put32(&mut out, 22, picture.height);
    put16(&mut out, 26, 1);
    put16(&mut out, 28, 24);
    put32(&mut out, 34, data as u32);
    put32(&mut out, 38, PIXELS_PER_METRE);
    put32(&mut out, 42, PIXELS_PER_METRE);
    let width = picture.width as usize;
    for (y, line) in picture.pixels.chunks_exact(width).enumerate() {
        // Bitmaps are stored bottom row first.
        let start = HEADER + (picture.height as usize - 1 - y) * row;
        for (x, &p) in line.iter().enumerate() {
            let at = start + x * 3;
            out[at] = p as u8;
            out[at + 1] = (p >> 8) as u8;
            out[at + 2] = (p >> 16) as u8;
        }
    }
    out
}

/// Reads a bitmap, by 1.3's rules: it starts with `BM`; it has an info header of 40 bytes or
/// more; 24 or 32 bits a pixel; no compression; a width above zero; a height of either sign
/// (below zero means the top row comes first); and all its pixels inside the file.
///
/// The picture may not have more than [`MAX_PIXELS`] pixels. A 32-bit file's fourth byte
/// (alpha, if it means anything) is ignored. The file's size limit (1.3's was 8 MiB) is for
/// the caller, which knows the size before it reads.
pub fn decode(file: &[u8]) -> Result<Picture, BmpError> {
    if !file.starts_with(b"BM") {
        return Err(BmpError::NotABitmap);
    }
    if file.len() < HEADER {
        return Err(BmpError::CutShort);
    }
    let (offset, header) = (u64::from(get32(file, 10)), u64::from(get32(file, 14)));
    // The info header: its size must at least be the 40 bytes this reads fields from, and
    // the pixels can't start inside the headers.
    if header < 40 {
        return Err(BmpError::Unsupported("its header is of a kind this can't read"));
    }
    if offset < 14 + header {
        return Err(BmpError::Unsupported("its pixels start inside its header"));
    }
    // Width and height are signed 32-bit numbers in the file.
    let width = get32(file, 18) as i32;
    let height = get32(file, 22) as i32;
    let bits = get16(file, 28);
    let compression = get32(file, 30);
    if width <= 0 {
        return Err(BmpError::Unsupported("its width isn't above zero"));
    }
    if height == 0 {
        return Err(BmpError::Unsupported("its height is zero"));
    }
    if bits != 24 && bits != 32 {
        return Err(BmpError::Unsupported("it isn't 24 or 32 bits a pixel"));
    }
    if compression != 0 {
        return Err(BmpError::Unsupported("it is compressed"));
    }
    let top_down = height < 0;
    // `unsigned_abs` is safe for the most negative number, which negating would overflow.
    let (width, height) = (width as u32, height.unsigned_abs());
    // Both are under 2^32, so their product fits a `u64`: no overflow, so the limit is real.
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(BmpError::TooBig);
    }
    let bytes = u32::from(bits / 8);
    let row = row_bytes(width, bytes);
    // `row` is under 2^35 and `height` under 2^32 (and the product under 2^28 * 4 by the
    // limit above), so none of this overflows.
    let end = offset + row * u64::from(height);
    if end > file.len() as u64 {
        return Err(BmpError::CutShort);
    }
    // Everything is inside the file now, and the picture is at most 64 million pixels.
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        let from_top = if top_down { y } else { height - 1 - y };
        let start = (offset + u64::from(from_top) * row) as usize;
        let line = &file[start..start + width as usize * bytes as usize];
        for px in line.chunks_exact(bytes as usize) {
            pixels.push(u32::from(px[0]) | u32::from(px[1]) << 8 | u32::from(px[2]) << 16);
        }
    }
    Picture::new(width, height, pixels).ok_or(BmpError::TooBig)
}

/// Writes a 16-bit little-endian number at `at` (the header's fields are all little-endian).
fn put16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Writes a 32-bit little-endian number at `at`.
fn put32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Reads a 16-bit number at `at`; only called with `at` inside the 54-byte header, which
/// `decode` has checked is there.
fn get16(file: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([file[at], file[at + 1]])
}

/// Reads a 32-bit number at `at` (inside the header, as above).
fn get32(file: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]])
}

#[cfg(test)]
mod tests;
