// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! PrettyBigRock's 5x11 bitmap font: the hand-drawn ASCII font 1.x uses (a test checks every
//! ASCII glyph against `classic/tools/mkassets.py`, where it was drawn), plus the Latin-1 and
//! Latin Extended-A letters and a few symbols, drawn in the same style (note 0023, section 3).
//!
//! A *bitmap font* stores each character as a small grid of dots instead of an outline, so
//! drawing text is just copying dots to the screen. A glyph here is [`HEIGHT`] rows of one byte
//! each; in a byte, bit 4 is the leftmost of the [`WIDTH`] dots and bit 0 the rightmost (bits 5
//! to 7 are always 0). A set bit is a dot of the text colour. When text is drawn there is one
//! empty column between characters, so a character takes 6 columns of the screen, and a glyph
//! may use all 5 of its own.
//!
//! # The cell, row by row
//!
//! Rows are numbered from 0 at the top:
//! - **rows 0 and 1** are the [`ABOVE`] band: empty in every ASCII glyph, and used for the
//!   accents of capital letters and of letters with ascenders (b d h k l t);
//! - **rows 2 to 8** hold capitals, digits and the tall part of letters like `b` and `l`.
//!   Lowercase letters without ascenders (`a c e g m n o r s u w y z`) occupy rows 4 to 8, and
//!   their accents go in rows 1 and 2;
//! - **rows 9 and 10** are for descenders (the tails of `g j p q y , ;`) and for marks that
//!   hang below a letter (cedilla, ogonek).
//!
//! An accented letter is its base letter with a mark added: the base letter's dots are left
//! exactly as they are, so that `é` reads as `e` (a test checks this).
//!
//! # Where the drawings are
//!
//! The source of the font is `glyphs.txt`, next to `Cargo.toml`: every character drawn as
//! lines of `#` and `.`, which can be read and reviewed by eye. `src/table.rs` is the same
//! drawings as numbers, generated from it. After changing `glyphs.txt`, run
//! `PBR_UPDATE_FONT=1 cargo test -p pbr-font` to regenerate the table; a plain test run fails
//! if the two disagree.
//!
//! The kernel's early text console and the console service both draw with this library, so
//! they show the same letters. `no_std` means this library uses only Rust's `core` library, not
//! the standard library (which needs an operating system underneath), so it builds for the
//! kernel, for programs and for the host.

#![cfg_attr(not(test), no_std)]

mod table;
#[cfg(test)]
mod tests;

use table::GLYPHS;

/// Glyph width in dots.
pub const WIDTH: usize = 5;
/// Glyph height in dots (rows): the [`ABOVE`] rows for accents, the capitals' 7 rows, and two
/// rows below for descenders.
pub const HEIGHT: usize = 11;
/// The number of rows above the capitals (rows 0 and 1), where the accents of capital letters
/// go. Capitals start at row `ABOVE`.
pub const ABOVE: usize = 2;

/// One character's dots: [`HEIGHT`] rows, bit 4 of each the leftmost dot.
pub type Glyph = [u8; HEIGHT];

/// The code point of the first character in the table (space); the table starts with the
/// printable ASCII characters in order, which lets [`lookup`] index it directly.
const FIRST_ASCII: u32 = 0x20;
/// The code point of the last ASCII character in the table (`~`).
const LAST_ASCII: u32 = 0x7E;

/// The glyph for a character, or `None` if the font doesn't have it.
///
/// `Option` is Rust's way of saying "a value or nothing": the caller has to look at which it
/// got before using the glyph.
pub fn lookup(c: char) -> Option<&'static Glyph> {
    let code = u32::from(c);
    if (FIRST_ASCII..=LAST_ASCII).contains(&code) {
        // ASCII is the common case, so it skips the search: entry n is character 0x20 + n.
        return Some(&GLYPHS[(code - FIRST_ASCII) as usize].1);
    }
    // The table is sorted by code point, so a binary search (halve the range, repeat) finds it.
    GLYPHS.binary_search_by_key(&code, |&(code, _)| code).ok().map(|i| &GLYPHS[i].1)
}

/// The glyph for a character; `?`'s glyph for one the font doesn't have. (The kernel's console
/// and the console service use this, since they must draw something for every byte of text.)
pub fn glyph(c: char) -> &'static Glyph {
    match lookup(c) {
        Some(g) => g,
        None => &GLYPHS[(u32::from('?') - FIRST_ASCII) as usize].1,
    }
}

/// Every character the font has, in code point order.
///
/// `impl Iterator` means "something you can loop over, giving `char`s"; the exact type is the
/// library's business.
pub fn characters() -> impl Iterator<Item = char> {
    // `filter_map` skips entries whose code point isn't a valid `char`; the table has none.
    GLYPHS.iter().filter_map(|&(code, _)| char::from_u32(code))
}
