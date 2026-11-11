// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A screen's description of itself: the EDID (Extended Display Identification Data) that a
//! monitor, or a virtual monitor, hands to the computer it is plugged in to
//! (docs/design/0020-screens-and-virtio-gpu.md, section 4).
//!
//! An EDID says who made the screen, which model and serial number it is, how big its picture
//! is in millimetres and which size of picture it prefers. The display server uses the size to
//! pick a scale factor for text, and the maker, model and serial number to recognise the same
//! screen again when it is plugged back in.
//!
//! The data comes from a device (and, for a real monitor, from a cable that anybody can plug a
//! gadget in to), so [`parse`] trusts none of it: it checks the fixed header and the checksum,
//! never reads outside the 128 bytes it was given, and answers `None` for anything wrong.

use alloc::format;
use alloc::string::String;

/// The size of the base block, the only part of an EDID read here. (A screen can add 128-byte
/// extension blocks after it, for more modes and for audio; none of that is needed.)
pub const BLOCK: usize = 128;

/// The eight bytes every base block starts with.
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
/// Where the first detailed timing descriptor starts. It describes the screen's preferred mode,
/// and is 18 bytes long.
const FIRST_TIMING: usize = 54;

/// What [`parse`] makes of an EDID. (`derive` asks the compiler to write the obvious code to
/// copy, print and compare a type.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edid {
    /// The picture's width in millimetres, 0 if the screen doesn't say.
    pub width_mm: u32,
    /// The picture's height in millimetres, 0 if the screen doesn't say.
    pub height_mm: u32,
    /// The maker's three-letter code, the product code and the serial number, such as
    /// `"RHT-1234-00000001"`: the maker in letters, the other two in hexadecimal. At most 64
    /// characters (it is 17).
    pub identity: String,
    /// The size in pixels of the mode the screen prefers, if it names one.
    pub preferred: Option<(u32, u32)>,
}

/// Reads the base block of an EDID. (`Option` is Rust's "maybe": `Some(value)` or `None`. `?`
/// on an `Option` returns `None` from the function at once if it was `None`, which is how this
/// gives up at the first thing that is wrong.) `None` if there are fewer than [`BLOCK`] bytes, the header
/// isn't right, or the bytes don't add up to a multiple of 256 (the last byte is there to make
/// them do so, which catches most damage).
pub fn parse(data: &[u8]) -> Option<Edid> {
    // Turning the slice into an array of exactly this length means every index below is
    // checked against the length when the code is compiled, so none of it can go wrong while
    // it runs.
    let block: &[u8; BLOCK] = data.get(..BLOCK)?.try_into().ok()?;
    if block[..8] != HEADER {
        return None;
    }
    if block.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) != 0 {
        return None;
    }

    // The maker is three letters packed into 15 bits, five bits each, with 1 meaning A.
    let packed = u16::from_be_bytes([block[8], block[9]]);
    let letter = |shift: u32| match (packed >> shift) & 0x1F {
        n @ 1..=26 => char::from(b'A' + n as u8 - 1),
        _ => '?',
    };
    let product = u16::from_le_bytes([block[10], block[11]]);
    let serial = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
    let identity = format!("{}{}{}-{product:04X}-{serial:08X}", letter(10), letter(5), letter(0));

    // The first 18-byte descriptor is a detailed timing if its pixel clock isn't 0 (a clock of
    // 0 marks the other kinds, such as a name or a serial number in text).
    let timing = &block[FIRST_TIMING..FIRST_TIMING + 18];
    let has_timing = u16::from_le_bytes([timing[0], timing[1]]) != 0;
    let high = |byte: u8| u32::from(byte >> 4);
    let low = |byte: u8| u32::from(byte & 0x0F);
    let (mut preferred, mut width_mm, mut height_mm) = (None, 0, 0);
    if has_timing {
        // Sizes are 12 bits: eight in one byte and the top four in the high half of another.
        let width = u32::from(timing[2]) | (high(timing[4]) << 8);
        let height = u32::from(timing[5]) | (high(timing[7]) << 8);
        if width != 0 && height != 0 {
            preferred = Some((width, height));
        }
        width_mm = u32::from(timing[12]) | (high(timing[14]) << 8);
        height_mm = u32::from(timing[13]) | (low(timing[14]) << 8);
    }
    // Without a picture size in the timing, the older size in centimetres will do. (Version 1.4
    // uses these two bytes for the shape of the picture when one is 0, which counts as unknown.)
    if width_mm == 0 || height_mm == 0 {
        (width_mm, height_mm) = (0, 0);
        if block[21] != 0 && block[22] != 0 {
            width_mm = u32::from(block[21]) * 10;
            height_mm = u32::from(block[22]) * 10;
        }
    }
    Some(Edid { width_mm, height_mm, identity, preferred })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;
    use alloc::vec::Vec;

    /// A timing descriptor: the pixel clock (in 10 kHz units), the size in pixels and the size
    /// in millimetres.
    type Timing = (u16, (u32, u32), (u32, u32));

    /// What goes in a test block.
    struct Spec {
        maker: [u8; 3],
        product: u16,
        serial: u32,
        cm: (u8, u8),
        timing: Option<Timing>,
    }

    /// A block like the one QEMU's virtual monitor sends, with the checksum filled in.
    fn block(s: &Spec) -> [u8; BLOCK] {
        let mut b = [0u8; BLOCK];
        b[..8].copy_from_slice(&HEADER);
        let [a, c, d] = s.maker.map(|l| u16::from(l - b'A' + 1));
        b[8..10].copy_from_slice(&((a << 10) | (c << 5) | d).to_be_bytes());
        b[10..12].copy_from_slice(&s.product.to_le_bytes());
        b[12..16].copy_from_slice(&s.serial.to_le_bytes());
        b[18] = 1; // version 1.4
        b[19] = 4;
        (b[21], b[22]) = s.cm;
        if let Some((clock, (w, h), (wmm, hmm))) = s.timing {
            let t = &mut b[FIRST_TIMING..FIRST_TIMING + 18];
            t[..2].copy_from_slice(&clock.to_le_bytes());
            t[2] = w as u8;
            t[4] = ((w >> 8) as u8) << 4;
            t[5] = h as u8;
            t[7] = ((h >> 8) as u8) << 4;
            t[12] = wmm as u8;
            t[13] = hmm as u8;
            t[14] = (((wmm >> 8) as u8) << 4) | ((hmm >> 8) as u8);
        }
        fix(&mut b);
        b
    }

    fn fix(b: &mut [u8; BLOCK]) {
        b[BLOCK - 1] = 0;
        b[BLOCK - 1] = 0u8.wrapping_sub(b.iter().fold(0u8, |s, x| s.wrapping_add(*x)));
    }

    fn qemu_like() -> Spec {
        Spec {
            maker: *b"RHT",
            product: 0x1234,
            serial: 1,
            cm: (53, 30),
            timing: Some((14850, (1920, 1080), (531, 299))),
        }
    }

    #[test]
    fn every_field_is_read() {
        let e = parse(&block(&qemu_like())).unwrap();
        assert_eq!(e.identity, "RHT-1234-00000001");
        assert_eq!((e.width_mm, e.height_mm), (531, 299), "the timing's size wins over the cm");
        assert_eq!(e.preferred, Some((1920, 1080)));
    }

    #[test]
    fn sizes_use_all_twelve_bits() {
        let s = Spec {
            maker: *b"ZZA",
            product: 0xFFFF,
            serial: 0xDEAD_BEEF,
            cm: (0, 0),
            timing: Some((1, (3840, 2160), (1600, 900))),
        };
        let e = parse(&block(&s)).unwrap();
        assert_eq!(e.identity, "ZZA-FFFF-DEADBEEF");
        assert_eq!((e.width_mm, e.height_mm), (1600, 900));
        assert_eq!(e.preferred, Some((3840, 2160)));
    }

    #[test]
    fn the_size_falls_back_to_centimetres_and_then_to_nothing() {
        // No timing at all (its pixel clock is 0).
        let mut s = qemu_like();
        s.timing = None;
        let e = parse(&block(&s)).unwrap();
        assert_eq!((e.width_mm, e.height_mm, e.preferred), (530, 300, None));
        // A timing that gives no size.
        let mut s = qemu_like();
        s.timing = Some((14850, (1280, 720), (0, 0)));
        let e = parse(&block(&s)).unwrap();
        assert_eq!((e.width_mm, e.height_mm, e.preferred), (530, 300, Some((1280, 720))));
        // Only one of the two sizes: version 1.4's shape of picture, which is not a size.
        let mut s = qemu_like();
        s.timing = None;
        s.cm = (16, 0);
        assert_eq!(parse(&block(&s)).map(|e| (e.width_mm, e.height_mm)), Some((0, 0)));
        // A timing with one size missing falls back too.
        let mut s = qemu_like();
        s.timing = Some((14850, (1280, 720), (500, 0)));
        s.cm = (0, 0);
        assert_eq!(parse(&block(&s)).map(|e| (e.width_mm, e.height_mm)), Some((0, 0)));
        // No sizes anywhere.
        let mut s = qemu_like();
        s.timing = None;
        s.cm = (0, 0);
        assert_eq!(parse(&block(&s)).map(|e| (e.width_mm, e.height_mm)), Some((0, 0)));
    }

    #[test]
    fn a_timing_with_no_pixels_names_no_mode() {
        let mut s = qemu_like();
        s.timing = Some((14850, (0, 1080), (531, 299)));
        assert_eq!(parse(&block(&s)).unwrap().preferred, None);
    }

    #[test]
    fn a_maker_code_that_is_not_letters_still_gives_an_identity() {
        let mut b = block(&qemu_like());
        b[8] = 0;
        b[9] = 0;
        fix(&mut b);
        assert_eq!(parse(&b).unwrap().identity, "???-1234-00000001");
        b[8] = 0xFF;
        b[9] = 0xFF;
        fix(&mut b);
        assert_eq!(parse(&b).unwrap().identity, "???-1234-00000001");
    }

    #[test]
    fn extension_blocks_after_the_base_are_ignored() {
        let mut data = block(&qemu_like()).to_vec();
        data.extend_from_slice(&[0x55; 896]);
        assert_eq!(parse(&data), parse(&block(&qemu_like())));
    }

    #[test]
    fn a_wrong_header_or_checksum_gives_nothing() {
        let good = block(&qemu_like());
        for i in 0..8 {
            let mut b = good;
            b[i] ^= 1;
            fix(&mut b);
            assert_eq!(parse(&b), None, "header byte {i}");
        }
        for i in 0..BLOCK {
            let mut b = good;
            b[i] = b[i].wrapping_add(1);
            assert_eq!(parse(&b), None, "byte {i} changed without fixing the checksum");
        }
    }

    #[test]
    fn nonsense_gives_nothing_and_never_panics() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0; BLOCK]), None);
        assert_eq!(parse(&[0xFF; BLOCK]), None);
        assert_eq!(parse(&[0xFF; 4096]), None);
        // The header and a checksum, and nothing else: a valid block of no information.
        let mut empty = [0u8; BLOCK];
        empty[..8].copy_from_slice(&HEADER);
        fix(&mut empty);
        let e = parse(&empty).unwrap();
        assert_eq!((e.width_mm, e.height_mm, e.preferred), (0, 0, None));
        assert_eq!(e.identity, "???-0000-00000000");
        // Every truncation.
        let good = block(&qemu_like());
        for n in 0..BLOCK {
            assert_eq!(parse(&good[..n]), None, "{n} bytes");
        }
        assert!(parse(&good).is_some());
    }

    #[test]
    fn random_damage_never_panics_and_what_passes_is_well_formed() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            let mut b = [0u8; BLOCK];
            for x in b.iter_mut() {
                *x = rng.next() as u8;
            }
            // Half the time, give it a valid header and checksum so the rest is exercised.
            if rng.below(2) == 0 {
                b[..8].copy_from_slice(&HEADER);
                fix(&mut b);
            }
            if let Some(e) = parse(&b) {
                assert_eq!(e.identity.len(), 17);
                assert!(e.width_mm <= 4095 && e.height_mm <= 4095);
                if let Some((w, h)) = e.preferred {
                    assert!((1..=4095).contains(&w) && (1..=4095).contains(&h));
                }
            }
        }
        let junk: Vec<u8> = (0..300u32).map(|i| (i * 31) as u8).collect();
        for n in 0..junk.len() {
            let _ = parse(&junk[..n]);
        }
    }
}
