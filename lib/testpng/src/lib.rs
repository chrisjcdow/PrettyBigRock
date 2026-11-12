// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reference pictures for tests (docs/design/0023-draw-window-and-sandstone.md, section 6).
//!
//! A test that draws something can compare it, pixel for pixel, with a picture kept in the
//! repository. The pictures are PNG files, so GitHub shows them and a person can open them in
//! any viewer; this library writes and reads them with no outside code (note 0002: no
//! third-party packages). It runs on the host only and uses `std`.
//!
//! **What is in a PNG file.** A PNG file is an 8-byte signature followed by a list of
//! *chunks*. Each chunk is: its data length (4 bytes), a 4-letter type, the data, and a
//! *CRC-32* of the type and the data (a checksum, see `pbr_crc`: if a byte is damaged, the
//! checksum no longer matches, so damage is noticed instead of trusted). A picture needs three
//! kinds: `IHDR` (size and pixel format), `IDAT` (the pixels) and `IEND` (the end). A type
//! whose first letter is lower case is *ancillary*: a reader may skip it if it doesn't know
//! it. An unknown type starting with a capital is *critical*: the file can't be read right
//! without it.
//!
//! **Why the pixels are not compressed.** PNG squeezes the pixels with *deflate* (inside a
//! small wrapper called *zlib*). Deflate has a "stored" block type that squeezes nothing: it
//! is just a length and the bytes. A file made only of stored blocks is still a valid PNG, and
//! needs none of deflate's difficult code: no matching, no Huffman codes. Git compresses its
//! repository itself, so the file is no larger there. The price is that this reader can't
//! read a PNG that another tool compressed; it says so, and the fix is to write the picture
//! again with `PBR_UPDATE_PICTURES=1`.
//!
//! [`encode`], [`decode`] and [`check`] move between three things: a [`Picture`] in memory
//! (0xAARRGGBB pixels, as the drawing code makes them), the bytes of a PNG file, and a file on
//! disk.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// A picture: `pixels` holds `width * height` pixels, row by row from the top, each 0xAARRGGBB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// Pixels in each row.
    pub width: u32,
    /// Number of rows.
    pub height: u32,
    /// The pixels, row by row from the top, each 0xAARRGGBB (alpha, red, green, blue).
    pub pixels: Vec<u32>,
}

/// The largest side accepted, each way.
pub const MAX_SIDE: u32 = 16_384;

/// The 8 bytes every PNG file starts with. The odd first byte (above 127) and the line-end
/// bytes are there so that a file damaged by a text-mode copy is noticed at once.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The most data one deflate stored block can hold: its length is a 16-bit number.
const MAX_STORED: usize = 65_535;

/// PNG colour type 2: red, green, blue, 3 bytes a pixel.
const COLOUR_RGB: u8 = 2;

/// PNG colour type 6: red, green, blue, alpha, 4 bytes a pixel.
const COLOUR_RGBA: u8 = 6;

/// The Adler-32 checksum of `data`, which ends every zlib stream.
///
/// Like a CRC it is a short number worked out from the data, so that damage is noticed. It is
/// two 16-bit sums side by side: `a` is 1 plus every byte, and `b` is the sum of every value
/// `a` has had. Both are kept below 65521, the largest prime under 65536. It is simpler and
/// faster than a CRC, which is why zlib uses it, and it is the whole of what this library
/// needs to check the pixel data.
fn adler32(data: &[u8]) -> u32 {
    const PRIME: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // The `%` is only needed now and then: 5552 bytes is the most that can be added before `b`
    // could pass what a `u32` holds, so working in pieces of 5552 keeps the sums exact while
    // doing the slow division rarely. `chunks` hands out the slice in pieces of that size.
    for piece in data.chunks(5552) {
        for &byte in piece {
            a += u32::from(byte);
            b += a;
        }
        a %= PRIME;
        b %= PRIME;
    }
    (b << 16) | a
}

/// Checks a picture's size against the limits, for both [`encode`] and [`decode`]. Gives the
/// number of pixels. In Rust, `Result<T, String>` is "either a value (`Ok`) or an error message
/// (`Err`)"; a function ending in `?` after a call to this passes the `Err` straight on.
fn check_sides(width: u32, height: u32) -> Result<u64, String> {
    if width == 0 || height == 0 {
        return Err(format!("a picture can't have a side of 0 ({width}x{height})"));
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "a picture of {width}x{height} is too big: a side can be at most {MAX_SIDE}"
        ));
    }
    Ok(u64::from(width) * u64::from(height))
}

/// Adds one chunk (length, type, data, CRC) to `out`.
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), String> {
    let length =
        u32::try_from(data.len()).map_err(|_| "a picture's data is too big for a PNG chunk")?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    // `update` carries on a checksum over more bytes, so the type and the data needn't be
    // joined first.
    let crc = pbr_crc::update(pbr_crc::update(0, kind), data);
    out.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

/// The PNG file for a picture. Err (with a plain-English reason) if a side is 0 or above
/// MAX_SIDE, or pixels.len() != width * height.
///
/// The file has no choices left in it, so the same picture always gives the same bytes: a
/// picture that didn't change doesn't show as changed in a commit.
pub fn encode(picture: &Picture) -> Result<Vec<u8>, String> {
    let count = check_sides(picture.width, picture.height)?;
    if picture.pixels.len() as u64 != count {
        return Err(format!(
            "a picture of {}x{} needs {count} pixels, but it has {}",
            picture.width,
            picture.height,
            picture.pixels.len()
        ));
    }
    // Colour type 2 (no alpha) when every pixel is fully opaque, as the files of most pictures
    // can be; this keeps them a quarter smaller and readable by every viewer.
    let opaque = picture.pixels.iter().all(|&p| p >> 24 == 0xFF);
    let (colour_type, bytes_per_pixel) = if opaque { (COLOUR_RGB, 3) } else { (COLOUR_RGBA, 4) };

    // The raw data PNG wants: each row is a "filter type" byte (0 means the row is stored as
    // it is) followed by its pixels.
    let width = picture.width as usize;
    let mut raw = Vec::with_capacity((1 + width * bytes_per_pixel) * picture.height as usize);
    // `chunks_exact(n)` goes through a slice `n` items at a time (here, one row of pixels).
    for row in picture.pixels.chunks_exact(width) {
        raw.push(0);
        for &pixel in row {
            let [alpha, red, green, blue] = pixel.to_be_bytes();
            raw.extend_from_slice(&[red, green, blue]);
            if !opaque {
                raw.push(alpha);
            }
        }
    }

    // The zlib wrapper: two header bytes (0x78: deflate with the biggest window; 0x01: no
    // preset dictionary, fastest level, and the pair is a multiple of 31 as the format wants),
    // then deflate blocks, then the Adler-32 of the raw data.
    let blocks = raw.len().div_ceil(MAX_STORED);
    let mut zlib = Vec::with_capacity(raw.len() + blocks * 5 + 6);
    zlib.extend_from_slice(&[0x78, 0x01]);
    for (i, block) in raw.chunks(MAX_STORED).enumerate() {
        // A stored block starts with one byte whose lowest bit says "this is the last block"
        // and whose next two bits (00) say "stored"; then its length and the length's
        // complement (all bits flipped, so a reader can notice a damaged length), both
        // little-endian.
        let last = i + 1 == blocks;
        let length = u16::try_from(block.len()).map_err(|_| "a stored block is too long")?;
        zlib.push(u8::from(last));
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&picture.width.to_be_bytes());
    header.extend_from_slice(&picture.height.to_be_bytes());
    // Bit depth 8, the colour type, then compression method 0 (deflate), filter method 0 and no
    // interlacing: the only choices PNG offers for these three.
    header.extend_from_slice(&[8, colour_type, 0, 0, 0]);

    let mut file = Vec::with_capacity(zlib.len() + 57);
    file.extend_from_slice(&SIGNATURE);
    push_chunk(&mut file, b"IHDR", &header)?;
    push_chunk(&mut file, b"IDAT", &zlib)?;
    push_chunk(&mut file, b"IEND", &[])?;
    Ok(file)
}

/// How the pictures in a file are laid out, from its `IHDR` chunk.
struct Header {
    width: u32,
    height: u32,
    /// 3 for colour type 2, 4 for colour type 6.
    bytes_per_pixel: usize,
}

/// Reads the data of an `IHDR` chunk, refusing every format `encode` doesn't write.
fn parse_header(data: &[u8]) -> Result<Header, String> {
    // `<&[u8; 13]>::try_from` turns a slice into an array of a fixed size, if it has that size.
    let Ok(&[w0, w1, w2, w3, h0, h1, h2, h3, depth, colour, compression, filter, interlace]) =
        <&[u8; 13]>::try_from(data)
    else {
        return Err(format!("the IHDR chunk is {} bytes long; it should be 13", data.len()));
    };
    let width = u32::from_be_bytes([w0, w1, w2, w3]);
    let height = u32::from_be_bytes([h0, h1, h2, h3]);
    check_sides(width, height)?;
    if depth != 8 {
        return Err(format!("the picture has {depth} bits a colour; this reader only reads 8"));
    }
    let bytes_per_pixel = match colour {
        COLOUR_RGB => 3,
        COLOUR_RGBA => 4,
        other => {
            return Err(format!(
                "the picture has colour type {other}; this reader only reads 2 (red, green, \
                 blue) and 6 (red, green, blue, alpha)"
            ));
        }
    };
    if compression != 0 || filter != 0 {
        return Err("the picture uses a compression or filter method PNG doesn't define".into());
    }
    if interlace != 0 {
        return Err(
            "the picture is interlaced; this reader only reads pictures in row order".into()
        );
    }
    Ok(Header { width, height, bytes_per_pixel })
}

/// A chunk's type as text for an error message: its letters, or `?` for any other byte, so
/// that damaged bytes can't put odd characters in a message.
fn chunk_name(kind: &[u8; 4]) -> String {
    kind.iter().map(|&b| if b.is_ascii_alphabetic() { char::from(b) } else { '?' }).collect()
}

/// One chunk read from a file.
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    /// The rest of the file after this chunk. (`'a` is a *lifetime*: it says that `data` and
    /// `after` point into the same bytes the chunk was read from, and can't outlive them.)
    after: &'a [u8],
}

/// Takes the first chunk off `rest`. Every length is checked against what is really there
/// before it is used, because the bytes may be anything.
fn next_chunk(rest: &[u8]) -> Result<Chunk<'_>, String> {
    // A slice pattern like this matches a slice that starts with those 8 bytes, and names the
    // bytes and the remainder; it doesn't match (so `else` runs) if the slice is too short.
    let Some((&[l0, l1, l2, l3, t0, t1, t2, t3], after_header)) = rest.split_first_chunk::<8>()
    else {
        return Err("the file ends in the middle of a chunk's header".into());
    };
    let kind = [t0, t1, t2, t3];
    let name = chunk_name(&kind);
    let length = u32::from_be_bytes([l0, l1, l2, l3]);
    // A `usize` can be smaller than a `u32` on some computers, so the conversion is checked.
    let length = usize::try_from(length).map_err(|_| format!("the {name} chunk is too long"))?;
    // The data and the 4-byte CRC after it must both be there.
    let Some(wanted) = length.checked_add(4).filter(|&n| n <= after_header.len()) else {
        return Err(format!("the file ends in the middle of a {name} chunk"));
    };
    let (data, after_data) = after_header.split_at(length);
    let (crc, after_chunk) = after_data.split_at(wanted - length);
    let stored = u32::from_be_bytes([crc[0], crc[1], crc[2], crc[3]]);
    let worked_out = pbr_crc::update(pbr_crc::update(0, &kind), data);
    if stored != worked_out {
        return Err(format!(
            "the {name} chunk is damaged: its CRC is {stored:#010x} but its data gives \
             {worked_out:#010x}"
        ));
    }
    Ok(Chunk { kind, data, after: after_chunk })
}

/// Reads the deflate stored blocks of a zlib stream and gives the raw data in them, which must
/// be exactly `expected` bytes. `expected` is already known to be no more than the stream holds
/// plus its wrapping, so the memory this asks for is never more than the file already used.
fn unwrap_zlib(zlib: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    // The two header bytes: the low 4 bits of the first say "deflate", the high 4 the window
    // size (at most 7); the pair read as a number must be a multiple of 31 (a check on
    // itself); bit 5 of the second says a preset dictionary follows, which is never used.
    let Some((&[cmf, flg], mut rest)) = zlib.split_first_chunk::<2>() else {
        return Err("the pixel data is too short to be a zlib stream".into());
    };
    if cmf & 0x0F != 8 || cmf >> 4 > 7 || (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return Err("the pixel data doesn't start with a zlib header".into());
    }
    if flg & 0x20 != 0 {
        return Err("the pixel data wants a preset dictionary, which PNG doesn't allow".into());
    }
    let mut raw = Vec::with_capacity(expected);
    loop {
        let Some((&header, after)) = rest.split_first() else {
            return Err("the pixel data ends before its last block".into());
        };
        // Bit 0 of a block header says "last block"; bits 1 and 2 say how it is stored.
        // 00 is stored, 01 and 10 are the two kinds of Huffman-coded block and 11 is invalid.
        if (header >> 1) & 3 != 0 {
            return Err("the pixel data is compressed; this reader only reads uncompressed PNG \
                        files, the kind it writes. Write the picture again with \
                        PBR_UPDATE_PICTURES=1"
                .into());
        }
        // The rest of the header byte is padding: a stored block starts on a byte boundary.
        let Some((&[l0, l1, n0, n1], after)) = after.split_first_chunk::<4>() else {
            return Err("the pixel data ends in the middle of a block header".into());
        };
        let length = u16::from_le_bytes([l0, l1]);
        if u16::from_le_bytes([n0, n1]) != !length {
            return Err("a block's length and its check copy don't agree".into());
        }
        let length = usize::from(length);
        if length > after.len() {
            return Err("the pixel data ends in the middle of a block".into());
        }
        let (block, after) = after.split_at(length);
        if block.len() > expected - raw.len() {
            return Err(format!(
                "the picture has more pixel data than its size needs ({expected} bytes)"
            ));
        }
        raw.extend_from_slice(block);
        rest = after;
        if header & 1 != 0 {
            break;
        }
    }
    let Some((&checksum, after)) = rest.split_first_chunk::<4>() else {
        return Err("the pixel data ends before its Adler-32 checksum".into());
    };
    if !after.is_empty() {
        return Err("there are extra bytes after the pixel data's checksum".into());
    }
    if raw.len() != expected {
        return Err(format!(
            "the picture has {} bytes of pixel data but its size needs {expected}",
            raw.len()
        ));
    }
    let stored = u32::from_be_bytes(checksum);
    if stored != adler32(&raw) {
        return Err("the pixel data is damaged: its Adler-32 checksum doesn't match".into());
    }
    Ok(raw)
}

/// Reads a PNG file that `encode` could have written. Err with a plain-English reason for
/// anything else; never panics, whatever the bytes.
///
/// A few harmless differences are accepted, since another tool may have rewritten the file:
/// the pixel data split over several `IDAT` chunks, chunks it doesn't need, and stored blocks
/// of any size.
pub fn decode(file: &[u8]) -> Result<Picture, String> {
    let Some(mut rest) = file.strip_prefix(&SIGNATURE[..]) else {
        return Err("this isn't a PNG file (it doesn't start with the PNG signature)".into());
    };
    let mut header: Option<Header> = None;
    let mut zlib: Vec<u8> = Vec::new();
    let mut seen_data = false;
    let mut first = true;
    loop {
        let Chunk { kind, data, after } = next_chunk(rest)?;
        rest = after;
        if first && &kind != b"IHDR" {
            return Err(format!("the first chunk is {}, not IHDR", chunk_name(&kind)));
        }
        first = false;
        match &kind {
            b"IHDR" => {
                if header.is_some() {
                    return Err("the file has two IHDR chunks".into());
                }
                header = Some(parse_header(data)?);
            }
            b"IDAT" => {
                seen_data = true;
                zlib.extend_from_slice(data);
            }
            b"IEND" => {
                if !data.is_empty() {
                    return Err("the IEND chunk has data in it".into());
                }
                break;
            }
            // A palette may appear in a true-colour picture as a hint to viewers on limited
            // screens; it changes nothing here.
            b"PLTE" => {}
            // A capital first letter marks a chunk the picture can't be read without.
            _ if kind[0] & 0x20 == 0 => {
                return Err(format!(
                    "the file has a {} chunk that this reader doesn't know and can't skip",
                    chunk_name(&kind)
                ));
            }
            _ => {}
        }
    }
    if !rest.is_empty() {
        return Err(format!("there are {} bytes of data after the IEND chunk", rest.len()));
    }
    if !seen_data {
        return Err("the file has no IDAT chunk (no pixels)".into());
    }
    // `first` is false and the loop only ends at IEND, so a header was read; this is for the
    // compiler and for the reader of this code.
    let Some(header) = header else {
        return Err("the file has no IHDR chunk".into());
    };

    // Now the size is known, work out how much raw data it needs, and refuse before
    // allocating anything if the file doesn't hold that much. The numbers are `u64` and at most
    // 16384 * (1 + 16384 * 4), so nothing overflows. (A file can't hold less than its raw
    // data, because stored blocks only add to it.)
    let width = u64::from(header.width);
    let stride = 1 + width * header.bytes_per_pixel as u64;
    let expected = stride * u64::from(header.height);
    if (zlib.len() as u64) < expected {
        return Err(format!(
            "the file holds {} bytes of pixel data but a {}x{} picture needs {expected}",
            zlib.len(),
            header.width,
            header.height
        ));
    }
    let expected = usize::try_from(expected).map_err(|_| "the picture is too big")?;
    let raw = unwrap_zlib(&zlib, expected)?;
    drop(zlib);

    let mut pixels = Vec::with_capacity(header.width as usize * header.height as usize);
    for (y, row) in raw.chunks_exact(stride as usize).enumerate() {
        let (&filter, row) = row.split_first().ok_or("an empty row")?;
        if filter != 0 {
            return Err(format!(
                "row {y} uses PNG filter type {filter}; this reader only reads filter type 0 \
                 (rows stored as they are)"
            ));
        }
        for p in row.chunks_exact(header.bytes_per_pixel) {
            let alpha = p.get(3).copied().unwrap_or(0xFF);
            pixels.push(u32::from_be_bytes([alpha, p[0], p[1], p[2]]));
        }
    }
    Ok(Picture { width: header.width, height: header.height, pixels })
}

/// Ok if the two are the same size and every pixel is equal. Otherwise Err saying how they
/// differ: the two sizes, or how many pixels differ, where the first differing one is (x, y),
/// and both its values in hex.
pub fn compare(got: &Picture, want: &Picture) -> Result<(), String> {
    if (got.width, got.height) != (want.width, want.height) {
        return Err(format!(
            "the picture is {}x{} but the reference is {}x{}",
            got.width, got.height, want.width, want.height
        ));
    }
    if got.pixels.len() != want.pixels.len() {
        return Err(format!(
            "the picture has {} pixels but the reference has {}",
            got.pixels.len(),
            want.pixels.len()
        ));
    }
    let mut different = 0u64;
    let mut first = None;
    for (i, (&g, &w)) in got.pixels.iter().zip(&want.pixels).enumerate() {
        if g != w {
            different += 1;
            first.get_or_insert((i, g, w));
        }
    }
    let Some((i, g, w)) = first else {
        return Ok(());
    };
    // `width` can't be 0 here when a pixel exists in a well-formed picture; `max(1)` keeps a
    // badly built one from dividing by zero.
    let width = got.width.max(1) as usize;
    Err(format!(
        "{different} of {} pixels differ; the first is at ({}, {}): got {g:#010x}, wanted \
         {w:#010x}",
        got.pixels.len(),
        i % width,
        i / width
    ))
}

/// Compares `got` with the reference picture in `file`.
///
/// If the environment variable `PBR_UPDATE_PICTURES` is `1`, the reference is written instead
/// (made new, or changed to match `got`) and the answer is Ok: that is how a change to the way
/// things look is meant to be recorded. `PBR_UPDATE_PICTURES=1 cargo xtask test` does it for
/// all of them; the pictures should then be looked at before they are committed.
pub fn check(file: &Path, got: &Picture) -> Result<(), String> {
    check_with(file, got, std::env::var("PBR_UPDATE_PICTURES").is_ok_and(|v| v == "1"))
}

/// Where a picture that didn't match is saved for a person to open: the same file name, in a
/// `pbr-pictures` folder in the computer's temporary folder.
fn got_path(file: &Path) -> PathBuf {
    let name = file.file_name().unwrap_or(std::ffi::OsStr::new("picture.png"));
    std::env::temp_dir().join("pbr-pictures").join(name)
}

/// [`check`] with the environment variable already read. This is separate because tests run
/// side by side in one process, and the environment is shared by all of them.
fn check_with(file: &Path, got: &Picture, update: bool) -> Result<(), String> {
    let shown = file.display();
    if update {
        let bytes = encode(got).map_err(|e| format!("{shown}: {e}"))?;
        // Leave a file that is already right alone, so its modified time and the build tools
        // watching it don't change for nothing.
        if std::fs::read(file).is_ok_and(|old| old == bytes) {
            return Ok(());
        }
        if let Some(folder) = file.parent().filter(|f| !f.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)
                .map_err(|e| format!("can't make the folder for {shown}: {e}"))?;
        }
        return std::fs::write(file, bytes).map_err(|e| format!("can't write {shown}: {e}"));
    }

    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "the reference picture {shown} doesn't exist. PBR_UPDATE_PICTURES=1 cargo xtask \
                 test writes reference pictures afresh; look at the new picture before it is \
                 committed"
            ));
        }
        Err(e) => return Err(format!("can't read the reference picture {shown}: {e}")),
    };
    let want = decode(&bytes).map_err(|e| format!("the reference picture {shown}: {e}"))?;
    compare(got, &want).map_err(|difference| {
        // Save what was drawn so a person can open it beside the reference. If this fails the
        // difference is still reported, so the error is ignored.
        let saved = got_path(file);
        let written = encode(got).ok().is_some_and(|png| {
            saved.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
                && std::fs::write(&saved, png).is_ok()
        });
        if written {
            format!(
                "{shown} differs from what was drawn: {difference}. What was drawn is saved as \
                 {}; open both to see. If the change is meant, run PBR_UPDATE_PICTURES=1 cargo \
                 xtask test and look at the new picture before it is committed",
                saved.display()
            )
        } else {
            format!("{shown} differs from what was drawn: {difference}")
        }
    })
}

#[cfg(test)]
mod tests;
