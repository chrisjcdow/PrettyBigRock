// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The bitmap reader and writer on the host: a picture 1.3's Paint saved (and byte for byte
//! what writing it again gives), 32-bit files in both row orders, odd widths and their padding,
//! every cut-short file, headers that lie, and thousands of damaged files.

use super::*;

/// A picture 1.3's Paint saved: 600 x 362, on the 1.3 test disk (`tests/data/classic-1.3.img.gz`
/// holds the same file as `/Pictures/rock.bmp`).
const ROCK: &[u8] = include_bytes!("../../../tests/data/classic-1.3-rock.bmp");

/// A picture whose pixels all differ a little, so a mix-up of rows or colours shows.
fn sample(width: u32, height: u32) -> Picture {
    let pixels = (0..width * height).map(|i| i.wrapping_mul(0x0101_0305) & 0x00FF_FFFF).collect();
    Picture::new(width, height, pixels).expect("a valid picture")
}

/// A 32-bit file of `p`, with alpha `alpha` in every pixel, rows bottom first or top first.
fn file32(p: &Picture, top_down: bool, alpha: u8) -> Vec<u8> {
    let mut f = encode(p);
    f.truncate(HEADER);
    put16(&mut f, 28, 32);
    let h = p.height() as i32;
    put32(&mut f, 22, (if top_down { -h } else { h }) as u32);
    let rows: Vec<&[u32]> = p.pixels().chunks(p.width() as usize).collect();
    let order: Vec<&&[u32]> =
        if top_down { rows.iter().collect() } else { rows.iter().rev().collect() };
    for row in order {
        for &px in row.iter() {
            f.extend_from_slice(&[px as u8, (px >> 8) as u8, (px >> 16) as u8, alpha]);
        }
    }
    f
}

#[test]
fn a_picture_1_3_saved_decodes_and_encodes_to_the_same_bytes() {
    let p = decode(ROCK).expect("1.3's own file");
    assert_eq!((p.width(), p.height()), (600, 362));
    // Not blank: the picture has something drawn in it.
    assert!(p.pixels().iter().any(|&px| px != 0x00FF_FFFF));
    assert!(p.pixels().iter().all(|&px| px <= 0x00FF_FFFF));
    assert_eq!(encode(&p), ROCK);
}

#[test]
fn rows_and_colours_are_where_they_should_be() {
    // Blue in the top left, red at the bottom right, in a 2 x 2 picture.
    let p = Picture::new(2, 2, vec![0x0000FF, 0x00FF00, 0x808080, 0xFF0000]).unwrap();
    let f = encode(&p);
    // 54 bytes of header; rows of 6 bytes padded to 8; the bottom row first; blue, green, red.
    assert_eq!(f.len(), 54 + 16);
    assert_eq!(&f[54..62], [0x80, 0x80, 0x80, 0x00, 0x00, 0xFF, 0, 0]);
    assert_eq!(&f[62..70], [0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0, 0]);
    assert_eq!(decode(&f).unwrap(), p);
}

#[test]
fn the_header_is_1_3s() {
    let f = encode(&sample(5, 3));
    let row = 16; // 5 * 3 = 15, padded to 16
    let mut want = [0u8; 54];
    want[..2].copy_from_slice(b"BM");
    want[2..6].copy_from_slice(&(54 + row * 3u32).to_le_bytes());
    want[10] = 54;
    want[14] = 40;
    want[18] = 5;
    want[22] = 3;
    want[26] = 1;
    want[28] = 24;
    want[34..38].copy_from_slice(&(row * 3).to_le_bytes());
    want[38..42].copy_from_slice(&2835u32.to_le_bytes());
    want[42..46].copy_from_slice(&2835u32.to_le_bytes());
    assert_eq!(&f[..54], &want[..]);
}

#[test]
fn odd_widths_are_padded_and_come_back() {
    for width in [1, 2, 3, 4, 5, 6, 7, 13] {
        for height in [1, 2, 3] {
            let p = sample(width, height);
            let f = encode(&p);
            let row = (width as usize * 3).div_ceil(4) * 4;
            assert_eq!(f.len(), 54 + row * height as usize, "{width}x{height}");
            // The padding is zeros.
            for y in 0..height as usize {
                let used = 54 + y * row + width as usize * 3;
                assert!(f[used..54 + (y + 1) * row].iter().all(|&b| b == 0));
            }
            assert_eq!(decode(&f).unwrap(), p, "{width}x{height}");
        }
    }
}

#[test]
fn thirty_two_bit_files_in_either_row_order() {
    for (width, height) in [(1, 1), (3, 2), (5, 4), (8, 8)] {
        let p = sample(width, height);
        for top_down in [false, true] {
            // Whatever the alpha byte says is ignored.
            for alpha in [0, 255, 77] {
                let f = file32(&p, top_down, alpha);
                assert_eq!(decode(&f).unwrap(), p, "{width}x{height} top_down={top_down}");
            }
        }
    }
}

#[test]
fn a_24_bit_file_with_the_top_row_first() {
    let p = sample(5, 3);
    let mut f = encode(&p);
    // Turn it upside down: reverse the order of the rows and say the height is negative.
    let row = 16;
    let rows: Vec<Vec<u8>> = f[54..].chunks(row).map(<[u8]>::to_vec).collect();
    f.truncate(54);
    for r in rows.iter().rev() {
        f.extend_from_slice(r);
    }
    put32(&mut f, 22, (-3i32) as u32);
    assert_eq!(decode(&f).unwrap(), p);
}

#[test]
fn a_longer_info_header_and_an_offset_past_it_are_fine() {
    // A 124-byte (version 5) info header, as many programs write: fields as before.
    let p = sample(4, 2);
    let plain = encode(&p);
    let mut f = plain[..54].to_vec();
    f.extend_from_slice(&[0u8; 84]);
    f.extend_from_slice(&plain[54..]);
    put32(&mut f, 14, 124);
    put32(&mut f, 10, 14 + 124);
    assert_eq!(decode(&f).unwrap(), p);
    // And a gap between the headers and the pixels.
    let mut g = plain[..54].to_vec();
    g.extend_from_slice(&[0xEE; 10]);
    g.extend_from_slice(&plain[54..]);
    put32(&mut g, 10, 64);
    assert_eq!(decode(&g).unwrap(), p);
}

#[test]
fn every_cut_short_file_is_an_error() {
    // A small picture with padding, as a file of each kind.
    for file in
        [encode(&sample(3, 2)), file32(&sample(3, 2), false, 0), file32(&sample(3, 2), true, 0)]
    {
        for len in 0..file.len() {
            assert!(decode(&file[..len]).is_err(), "cut to {len} of {}", file.len());
        }
        assert!(decode(&file).is_ok());
    }
    // The last row's padding isn't needed: but its pixels are.
    assert_eq!(decode(&encode(&sample(3, 2))[..54 + 9 + 8]), Err(BmpError::CutShort));
}

#[test]
fn the_errors_say_what_is_wrong() {
    assert_eq!(decode(b""), Err(BmpError::NotABitmap));
    assert_eq!(decode(b"B"), Err(BmpError::NotABitmap));
    assert_eq!(
        decode(b"PNG not a bitmap at all, but long enough to have a header......."),
        Err(BmpError::NotABitmap)
    );
    assert_eq!(decode(b"BM"), Err(BmpError::CutShort));
    assert_eq!(decode(&encode(&sample(2, 2))[..53]), Err(BmpError::CutShort));
    assert_eq!(BmpError::NotABitmap.to_string(), "it is not a bitmap",);
    assert_eq!(BmpError::TooBig.to_string(), "the picture is too big");
}

/// A valid file with one field changed.
fn with(width_height: Option<(u32, u32)>, change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let (w, h) = width_height.unwrap_or((4, 4));
    let mut f = encode(&sample(w, h));
    change(&mut f);
    f
}

#[test]
fn headers_that_lie_are_refused() {
    let unsupported = |f: Vec<u8>| matches!(decode(&f), Err(BmpError::Unsupported(_)));
    // 16 bits, 8 bits, 1 bit, 48 bits.
    for bits in [16u16, 8, 1, 48, 0] {
        assert!(unsupported(with(None, |f| put16(f, 28, bits))), "{bits} bits");
    }
    // Compression: RLE8, RLE4, bitfields, JPEG, PNG.
    for c in [1u32, 2, 3, 4, 5, 0x8000_0000] {
        assert!(unsupported(with(None, |f| put32(f, 30, c))), "compression {c}");
    }
    // Widths of zero and below; a height of zero.
    for w in [0u32, (-1i32) as u32, i32::MIN as u32, 0x8000_0001] {
        assert!(unsupported(with(None, |f| put32(f, 18, w))), "width {w:#x}");
    }
    assert!(unsupported(with(None, |f| put32(f, 22, 0))));
    // Info headers too small to hold the fields (an old OS/2 bitmap's is 12).
    for h in [0u32, 12, 39] {
        assert!(unsupported(with(None, |f| put32(f, 14, h))), "header {h}");
    }
    // Pixels said to start inside the headers.
    for off in [0u32, 14, 53] {
        assert!(unsupported(with(None, |f| put32(f, 10, off))), "offset {off}");
    }
}

#[test]
fn sizes_that_would_overflow_are_refused_before_anything_is_set_aside() {
    // Huge widths and heights, of either sign: too big, or cut short, never a panic or a big
    // allocation.
    let cases = [
        (0x7FFF_FFFFu32, 0x7FFF_FFFFu32),
        (0x7FFF_FFFF, 1),
        (1, 0x7FFF_FFFF),
        (1, 0x8000_0000), // the most negative height
        (0x7FFF_FFFF, 0x8000_0000),
        (0x4000_0000, 0x4000_0000),
        (8193, 8193),
        (65536, 65536),
        (1 << 20, 1 << 20),
    ];
    for (w, h) in cases {
        for bits in [24u16, 32] {
            let f = with(None, |f| {
                put32(f, 18, w);
                put32(f, 22, h);
                put16(f, 28, bits);
            });
            assert_eq!(decode(&f), Err(BmpError::TooBig), "{w}x{h:#x} at {bits}");
        }
    }
    // Just inside the limit, but the file has no such pixels: cut short, not allocated.
    let f = with(None, |f| {
        put32(f, 18, 8000);
        put32(f, 22, 8000);
    });
    assert_eq!(decode(&f), Err(BmpError::CutShort));
    // Offsets that overflow when the pixels' size is added.
    for off in [0xFFFF_FFFFu32, 0xFFFF_FFC0, 0x8000_0000] {
        let f = with(None, |f| put32(f, 10, off));
        assert_eq!(decode(&f), Err(BmpError::CutShort), "offset {off:#x}");
    }
    // A 32-bit file whose rows would be wider than the file.
    let f = with(None, |f| {
        put16(f, 28, 32);
    });
    assert_eq!(decode(&f), Err(BmpError::CutShort));
}

#[test]
fn the_pixel_limit_is_where_it_says() {
    assert_eq!(MAX_PIXELS, 64 * 1024 * 1024);
    // A picture's own limit: Picture::new refuses what decode would.
    assert!(Picture::new(0, 5, vec![]).is_none());
    assert!(Picture::new(2, 2, vec![0; 3]).is_none());
    assert!(Picture::new(2, 2, vec![0; 5]).is_none());
    assert!(Picture::new(u32::MAX, u32::MAX, vec![0; 4]).is_none());
    assert!(Picture::new(8193, 8193, vec![0; 8193 * 8193]).is_none());
    // The top byte of a pixel is dropped.
    let p = Picture::new(1, 1, vec![0xFF12_3456]).unwrap();
    assert_eq!(p.pixels(), [0x0012_3456]);
    assert_eq!(p.into_pixels(), [0x0012_3456]);
}

/// A tiny, repeatable random number generator (xorshift), so a failure can be run again.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn thousands_of_damaged_files_never_panic() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let kinds =
        [encode(&sample(5, 3)), file32(&sample(5, 3), false, 9), file32(&sample(3, 4), true, 0)];
    let mut accepted = 0;
    for round in 0..6000 {
        let mut f = kinds[round % kinds.len()].clone();
        // Damage a few bytes, mostly in the header, where it matters; sometimes cut the file or
        // add junk to its end.
        for _ in 0..1 + random.next() % 4 {
            let at = if random.next().is_multiple_of(4) {
                random.next() as usize % f.len()
            } else {
                random.next() as usize % HEADER
            };
            f[at] = match random.next() % 3 {
                0 => random.next() as u8,
                1 => 0xFF,
                _ => 0,
            };
        }
        match random.next() % 5 {
            0 => f.truncate(random.next() as usize % (f.len() + 1)),
            1 => f.extend_from_slice(&[0xAA; 7]),
            _ => {}
        }
        if let Ok(p) = decode(&f) {
            accepted += 1;
            // Whatever it accepted is a whole, sound picture that encodes again.
            assert_eq!(p.pixels().len() as u64, u64::from(p.width()) * u64::from(p.height()));
            assert_eq!(decode(&encode(&p)).unwrap(), p);
        }
    }
    // Some damage is harmless (a changed pixel), so the loop did reach the good paths too.
    assert!(accepted > 100, "only {accepted} accepted");
}

#[test]
fn pure_noise_never_panics() {
    let mut random = Random(12345);
    for round in 0..2000 {
        let len = (random.next() % 200) as usize;
        let mut f: Vec<u8> = (0..len).map(|_| random.next() as u8).collect();
        // Half of them start like a bitmap, so that the header fields are looked at.
        if round % 2 == 0 && f.len() >= 2 {
            f[0] = b'B';
            f[1] = b'M';
        }
        let _ = decode(&f);
    }
}
