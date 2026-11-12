// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests for the reference-picture library. Several build PNG files by hand from pieces, so that
//! what the reader accepts and refuses is tested against bytes the writer would never make.

use super::*;
use std::process::Command;

/// A picture whose pixels are made from where they are, with the given alpha.
fn gradient(width: u32, height: u32, alpha: u32) -> Picture {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let r = x.wrapping_mul(7).wrapping_add(y) & 0xFF;
            let g = y.wrapping_mul(13).wrapping_add(x * 3) & 0xFF;
            let b = (x ^ y).wrapping_mul(5) & 0xFF;
            pixels.push(alpha << 24 | r << 16 | g << 8 | b);
        }
    }
    Picture { width, height, pixels }
}

/// A chunk with a correct CRC, built without the library's own writer.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut joined = kind.to_vec();
    joined.extend_from_slice(data);
    out.extend_from_slice(&pbr_crc::crc32(&joined).to_be_bytes());
    out
}

/// The data of an IHDR chunk.
fn ihdr(width: u32, height: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&[depth, colour, 0, 0, interlace]);
    data
}

/// A zlib stream holding `raw` in stored blocks of at most `size` bytes.
fn zlib_stored(raw: &[u8], size: usize) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(size).collect();
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// A whole PNG file from chunks.
fn png(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    for c in chunks {
        out.extend_from_slice(c);
    }
    out
}

/// A valid 2x1 RGB picture's raw data (one filter byte, two pixels).
const RAW_2X1: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

/// A valid file for the 2x1 picture, with the pixel data given.
fn file_2x1(zlib: &[u8]) -> Vec<u8> {
    png(&[chunk(b"IHDR", &ihdr(2, 1, 8, 2, 0)), chunk(b"IDAT", zlib), chunk(b"IEND", &[])])
}

fn round_trip(picture: &Picture) -> Vec<u8> {
    let file = encode(picture).unwrap();
    assert_eq!(&decode(&file).unwrap(), picture);
    assert_eq!(encode(picture).unwrap(), file, "the same picture gives the same bytes");
    file
}

#[test]
fn adler32_known_values() {
    // Values from Python's zlib.adler32.
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    // Long enough to cross the 5552-byte pieces, and all 255s to push the sums as far as they go.
    assert_eq!(adler32(&[0xFF; 20_000]), {
        let (mut a, mut b) = (1u64, 0u64);
        for _ in 0..20_000 {
            a = (a + 255) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16 | a) as u32
    });
}

#[test]
fn round_trips() {
    round_trip(&Picture { width: 1, height: 1, pixels: vec![0xFF12_3456] });
    round_trip(&gradient(37, 1, 255));
    round_trip(&gradient(1, 37, 255));
    round_trip(&gradient(37, 29, 128));
    round_trip(&gradient(37, 29, 0));
    // Two blocks or more.
    round_trip(&gradient(300, 100, 77));
}

#[test]
fn opaque_pictures_use_colour_type_2_and_others_6() {
    let opaque = round_trip(&gradient(5, 4, 255));
    assert_eq!(opaque[25], 2);
    let mut see_through = gradient(5, 4, 255);
    see_through.pixels[7] = 0xFE00_0000;
    let file = round_trip(&see_through);
    assert_eq!(file[25], 6);
}

#[test]
fn stored_block_boundaries() {
    // The raw data is height * (1 + width * bytes_per_pixel) bytes; these sizes make it exactly
    // 65535 (one full block), 65536 (a full block and a block of 1) and neighbours.
    // 28x771 RGB: 771 rows of 85 bytes = 65535. 1x16384 RGB: 16384 rows of 4 bytes = 65536.
    // 1x13107 RGBA: 13107 rows of 5 bytes = 65535.
    let cases = [
        (28, 771, 255, 65_535),
        (1, 16_384, 255, 65_536),
        (1, 13_107, 7, 65_535),
        (28, 770, 255, 65_450),
        (28, 772, 255, 65_620),
    ];
    for (width, height, alpha, raw_len) in cases {
        let file = round_trip(&gradient(width, height, alpha));
        // 8 signature + 25 IHDR chunk + 8 IDAT header + 2 zlib header = 43: the first block.
        let flag = file[43];
        let length = u16::from_le_bytes([file[44], file[45]]);
        if raw_len <= 65_535 {
            assert_eq!((flag, usize::from(length)), (1, raw_len), "one block, the last");
        } else {
            assert_eq!((flag, length), (0, 65_535), "a full block, then another");
            // The second block header follows the first block's 65535 bytes.
            let second = 43 + 5 + 65_535;
            assert_eq!(file[second], 1);
            assert_eq!(
                usize::from(u16::from_le_bytes([file[second + 1], file[second + 2]])),
                raw_len - 65_535
            );
        }
    }
}

#[test]
fn a_known_file() {
    // A 1x1 solid red picture, written out by hand. The CRCs and the Adler-32 (0x03010100 for
    // the raw bytes 00 FF 00 00) were worked out with Python's zlib, not with this library.
    #[rustfmt::skip]
    let known: [u8; 72] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00,
        0x90, 0x77, 0x53, 0xde,
        0x00, 0x00, 0x00, 0x0f, 0x49, 0x44, 0x41, 0x54,
        0x78, 0x01, 0x01, 0x04, 0x00, 0xfb, 0xff, 0x00, 0xff, 0x00, 0x00,
        0x03, 0x01, 0x01, 0x00,
        0x8d, 0x1d, 0xe5, 0x82,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let red = Picture { width: 1, height: 1, pixels: vec![0xFFFF_0000] };
    assert_eq!(encode(&red).unwrap(), known);
    assert_eq!(decode(&known).unwrap(), red);
}

/// Python (standard library only) reads a file and prints its size, then its pixels as
/// AARRGGBB in hex. It checks every chunk's CRC itself and uses `zlib.decompress`.
const PYTHON_READER: &str = r#"
import sys, struct, zlib
d = open(sys.argv[1], 'rb').read()
assert d[:8] == b'\x89PNG\r\n\x1a\n'
pos, idat, header, names = 8, b'', None, []
while pos < len(d):
    n, = struct.unpack('>I', d[pos:pos + 4])
    kind = d[pos + 4:pos + 8]
    body = d[pos + 8:pos + 8 + n]
    crc, = struct.unpack('>I', d[pos + 8 + n:pos + 12 + n])
    assert zlib.crc32(kind + body) == crc, 'bad CRC in ' + repr(kind)
    names.append(kind)
    if kind == b'IHDR':
        header = body
    elif kind == b'IDAT':
        idat += body
    pos += 12 + n
assert pos == len(d) and names[0] == b'IHDR' and names[-1] == b'IEND'
w, h, depth, colour, comp, filt, inter = struct.unpack('>IIBBBBB', header)
assert (depth, comp, filt, inter) == (8, 0, 0, 0)
bpp = {2: 3, 6: 4}[colour]
raw = zlib.decompress(idat)
assert len(raw) == h * (1 + w * bpp)
out = []
for y in range(h):
    row = raw[y * (1 + w * bpp):(y + 1) * (1 + w * bpp)]
    assert row[0] == 0
    for x in range(w):
        p = row[1 + x * bpp:1 + (x + 1) * bpp]
        a = p[3] if bpp == 4 else 255
        out.append('%02x%02x%02x%02x' % (a, p[0], p[1], p[2]))
print(w, h)
print(' '.join(out))
"#;

#[test]
fn python_reads_what_we_write() {
    let dir = std::env::temp_dir().join(format!("pbr-testpng-python-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut skipped = false; // Only so the loop can stop at the first picture.
    // Includes a picture with three stored blocks, in both colour types.
    for (i, picture) in [
        gradient(1, 1, 255),
        gradient(19, 11, 255),
        gradient(19, 11, 200),
        gradient(300, 250, 255),
        gradient(300, 250, 99),
    ]
    .iter()
    .enumerate()
    {
        let path = dir.join(format!("p{i}.png"));
        std::fs::write(&path, encode(picture).unwrap()).unwrap();
        // `-I` stops Python reading modules from the current folder.
        let run = match Command::new("python3")
            .arg("-I")
            .arg("-c")
            .arg(PYTHON_READER)
            .arg(&path)
            .output()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipped: python3 is not available ({e})");
                skipped = true;
                break;
            }
        };
        assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
        let text = String::from_utf8(run.stdout).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), format!("{} {}", picture.width, picture.height));
        let want: Vec<String> = picture.pixels.iter().map(|p| format!("{p:08x}")).collect();
        assert_eq!(lines.next().unwrap(), want.join(" "), "picture {i}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = skipped;
}

#[test]
fn encode_refuses_bad_pictures() {
    let bad = |width, height, count: usize| {
        encode(&Picture { width, height, pixels: vec![0xFF00_0000; count] })
    };
    assert!(bad(0, 1, 0).unwrap_err().contains("side of 0"));
    assert!(bad(1, 0, 0).unwrap_err().contains("side of 0"));
    assert!(bad(MAX_SIDE + 1, 1, 1).unwrap_err().contains("too big"));
    assert!(bad(1, MAX_SIDE + 1, 1).unwrap_err().contains("too big"));
    assert!(bad(2, 2, 3).unwrap_err().contains("needs 4 pixels, but it has 3"));
    assert!(bad(2, 2, 5).is_err());
    assert!(bad(u32::MAX, u32::MAX, 0).is_err());
    assert!(bad(1, 1, 1).is_ok());
}

#[test]
fn every_prefix_of_a_file_is_refused() {
    for picture in [gradient(3, 2, 255), gradient(3, 2, 10)] {
        let file = encode(&picture).unwrap();
        for length in 0..file.len() {
            assert!(decode(&file[..length]).is_err(), "a prefix of {length} bytes");
        }
    }
}

#[test]
fn every_single_byte_change_is_refused() {
    for picture in [gradient(3, 2, 255), gradient(3, 2, 10)] {
        let file = encode(&picture).unwrap();
        for at in 0..file.len() {
            for change in [0x01u8, 0x80, 0xFF] {
                let mut damaged = file.clone();
                damaged[at] ^= change;
                // The chunk CRCs cover every byte after the signature, and the signature is
                // checked, so no change can get through.
                assert!(decode(&damaged).is_err(), "byte {at} changed by {change:#x}");
            }
        }
    }
}

#[test]
fn damage_inside_the_pixel_data_is_caught_even_with_a_good_crc() {
    // Change each byte of the zlib stream and give the chunk a correct CRC again, so that
    // only the reader's own checks (block lengths, Adler-32, sizes) can notice. Must never
    // panic; and since every byte is covered by one of those checks, always an error.
    let zlib = zlib_stored(&RAW_2X1, 4);
    for at in 0..zlib.len() {
        for change in [0x01u8, 0x80, 0xFF] {
            let mut damaged = zlib.clone();
            damaged[at] ^= change;
            let file = file_2x1(&damaged);
            let result = decode(&file);
            // The top bit of a block header byte is padding up to the byte boundary, which
            // deflate says to ignore, so this reader ignores it too.
            let ignored_padding = change == 0x80 && matches!(at, 2 | 11);
            if !ignored_padding {
                assert!(result.is_err(), "zlib byte {at} changed by {change:#x}");
            }
        }
    }
}

#[test]
fn huge_claims_are_refused_without_allocating() {
    let file = png(&[
        chunk(b"IHDR", &ihdr(MAX_SIDE, MAX_SIDE, 8, 6, 0)),
        chunk(b"IDAT", &zlib_stored(&[0; 100], 100)),
        chunk(b"IEND", &[]),
    ]);
    let started = std::time::Instant::now();
    let error = decode(&file).unwrap_err();
    assert!(error.contains("needs"), "{error}");
    assert!(started.elapsed().as_secs() < 5);
    // A stream whose first block claims far more than the picture holds.
    let mut zlib = vec![0x78, 0x01, 0x01, 0xFF, 0xFF, 0x00, 0x00];
    zlib.extend_from_slice(&[0; 20]);
    assert!(decode(&file_2x1(&zlib)).is_err());
}

#[test]
fn sizes_and_formats_that_are_not_ours_are_refused() {
    let zlib = zlib_stored(&RAW_2X1, 7);
    let with_header = |header: Vec<u8>| {
        png(&[chunk(b"IHDR", &header), chunk(b"IDAT", &zlib), chunk(b"IEND", &[])])
    };
    let error = |file: Vec<u8>| decode(&file).unwrap_err();
    assert!(error(with_header(ihdr(0, 1, 8, 2, 0))).contains("side of 0"));
    assert!(error(with_header(ihdr(2, MAX_SIDE + 1, 8, 2, 0))).contains("too big"));
    assert!(error(with_header(ihdr(2, 1, 16, 2, 0))).contains("16 bits"));
    assert!(error(with_header(ihdr(2, 1, 8, 0, 0))).contains("colour type 0"));
    assert!(error(with_header(ihdr(2, 1, 8, 3, 0))).contains("colour type 3"));
    assert!(error(with_header(ihdr(2, 1, 8, 4, 0))).contains("colour type 4"));
    assert!(error(with_header(ihdr(2, 1, 8, 2, 1))).contains("interlaced"));
    assert!(error(with_header(vec![0; 12])).contains("IHDR"));
    // And the control: the same file with a good header is read.
    assert!(decode(&with_header(ihdr(2, 1, 8, 2, 0))).is_ok());
}

#[test]
fn bad_pixel_data_is_refused() {
    let error = |zlib: &[u8]| decode(&file_2x1(zlib)).unwrap_err();

    // A compressed block, made by hand: a block header with BFINAL = 1 and BTYPE = 01
    // (fixed Huffman codes), as `zlib.compress` would start its output.
    let mut compressed = vec![0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    assert!(error(&compressed).contains("only reads uncompressed"), "{}", error(&compressed));
    compressed[2] = 0x05; // BTYPE = 10, dynamic codes.
    assert!(error(&compressed).contains("only reads uncompressed"));
    compressed[2] = 0x07; // BTYPE = 11, which no stream may use.
    assert!(error(&compressed).contains("only reads uncompressed"));

    let good = zlib_stored(&RAW_2X1, 7);
    assert!(decode(&file_2x1(&good)).is_ok());

    let mut wrong_nlen = good.clone();
    wrong_nlen[5] ^= 1;
    assert!(error(&wrong_nlen).contains("don't agree"));

    let mut wrong_adler = good.clone();
    *wrong_adler.last_mut().unwrap() ^= 1;
    assert!(error(&wrong_adler).contains("Adler-32"));

    let mut wrong_header = good.clone();
    wrong_header[0] = 0x79;
    assert!(error(&wrong_header).contains("zlib header"));
    let mut dictionary = good.clone();
    // 0x7820 is a multiple of 31, so this header is well formed except for asking for a
    // preset dictionary.
    dictionary[1] = 0x20;
    assert!(error(&dictionary).contains("preset dictionary"));

    let mut filtered = RAW_2X1;
    filtered[0] = 1;
    assert!(error(&zlib_stored(&filtered, 7)).contains("filter type 1"));

    assert!(error(&zlib_stored(&RAW_2X1[..6], 6)).contains("needs"));
    let mut long = RAW_2X1.to_vec();
    long.push(0);
    assert!(error(&zlib_stored(&long, 8)).contains("more pixel data"));

    let mut extra = good.clone();
    extra.push(0);
    assert!(error(&extra).contains("extra bytes"));

    // An empty stream is already refused for being smaller than the picture needs; the
    // zlib reader's own message is tested directly.
    assert!(error(&[]).contains("needs"));
    assert!(unwrap_zlib(&[], 7).unwrap_err().contains("too short"));
    assert!(error(&good[..good.len() - 1]).contains("Adler"));
    // A first block that isn't the last, and then nothing.
    let mut unfinished = good.clone();
    unfinished.truncate(2 + 5 + 7);
    unfinished[2] = 0;
    assert!(error(&unfinished).contains("before its last block"));
}

#[test]
fn bad_files_are_refused() {
    let zlib = zlib_stored(&RAW_2X1, 7);
    let head = chunk(b"IHDR", &ihdr(2, 1, 8, 2, 0));
    let data = chunk(b"IDAT", &zlib);
    let end = chunk(b"IEND", &[]);
    let error = |chunks: &[Vec<u8>]| decode(&png(chunks)).unwrap_err();

    assert!(decode(&[]).unwrap_err().contains("signature"));
    assert!(decode(b"P6\n1 1\n255\nabc").unwrap_err().contains("signature"));
    assert!(decode(&png(&[head.clone(), data.clone(), end.clone()])).is_ok());

    assert!(error(&[data.clone(), head.clone(), end.clone()]).contains("not IHDR"));
    assert!(error(&[head.clone(), end.clone()]).contains("no IDAT"));
    assert!(error(&[head.clone(), data.clone()]).contains("middle of a chunk"));
    assert!(error(&[head.clone(), head.clone(), data.clone(), end.clone()]).contains("two IHDR"));
    assert!(error(&[head.clone(), chunk(b"IEND", &[1]), data.clone()]).contains("IEND"));
    assert!(
        error(&[head.clone(), chunk(b"XYZW", &[1]), data.clone(), end.clone()])
            .contains("XYZW chunk that this reader doesn't know")
    );

    // Data after IEND, whether a whole chunk or a stray byte.
    assert!(
        error(&[head.clone(), data.clone(), end.clone(), data.clone()]).contains("after the IEND")
    );
    let mut trailing = png(&[head.clone(), data.clone(), end.clone()]);
    trailing.push(0);
    assert!(decode(&trailing).unwrap_err().contains("1 bytes of data after"));

    // A chunk CRC that doesn't match.
    let mut bad_crc = png(&[head.clone(), data.clone(), end.clone()]);
    bad_crc[8 + 8 + 13] ^= 0x40;
    assert!(decode(&bad_crc).unwrap_err().contains("damaged"));

    // A length that runs past the end of the file, including the largest.
    let mut long = png(&[head.clone(), data.clone(), end.clone()]);
    long[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode(&long).unwrap_err().contains("middle of a"));
}

#[test]
fn several_idat_chunks_and_ancillary_chunks_are_accepted() {
    let picture = gradient(9, 7, 255);
    let mut raw = Vec::new();
    for row in picture.pixels.chunks(9) {
        raw.push(0);
        for p in row {
            raw.extend_from_slice(&p.to_be_bytes()[1..]);
        }
    }
    // Stored blocks of 10 bytes, which the zlib stream then gets cut across three IDATs at
    // places that are not block boundaries.
    let zlib = zlib_stored(&raw, 10);
    let (a, rest) = zlib.split_at(3);
    let (b, c) = rest.split_at(40);
    let file = png(&[
        chunk(b"IHDR", &ihdr(9, 7, 8, 2, 0)),
        chunk(b"tEXt", b"Comment\0made by hand"),
        chunk(b"IDAT", a),
        chunk(b"IDAT", b),
        chunk(b"pHYs", &[0; 9]),
        chunk(b"IDAT", c),
        chunk(b"IEND", &[]),
    ]);
    assert_eq!(decode(&file).unwrap(), picture);
}

#[test]
fn compare_says_how_pictures_differ() {
    let a = gradient(4, 3, 255);
    assert_eq!(compare(&a, &a.clone()), Ok(()));

    let wide = gradient(5, 3, 255);
    assert_eq!(compare(&a, &wide).unwrap_err(), "the picture is 4x3 but the reference is 5x3");

    let mut b = a.clone();
    b.pixels[6] = 0x1234_5678; // x = 2, y = 1.
    let message = compare(&a, &b).unwrap_err();
    assert!(message.starts_with("1 of 12 pixels differ"), "{message}");
    assert!(message.contains("(2, 1)"), "{message}");
    assert!(message.contains(&format!("{:#010x}", a.pixels[6])), "{message}");
    assert!(message.contains("0x12345678"), "{message}");

    let mut c = a.clone();
    for i in [3, 5, 9, 11] {
        c.pixels[i] ^= 1;
    }
    let message = compare(&a, &c).unwrap_err();
    assert!(message.starts_with("4 of 12 pixels differ"), "{message}");
    assert!(message.contains("first is at (3, 0)"), "{message}");

    // A hand-built picture with the wrong number of pixels doesn't panic.
    let broken = Picture { width: 4, height: 3, pixels: vec![0; 2] };
    assert!(compare(&a, &broken).is_err());
    let zero = Picture { width: 0, height: 0, pixels: vec![] };
    assert!(compare(&zero, &zero).is_ok());
}

/// A folder of its own for one test, removed when the test is done (even by a failure).
struct TempDir(PathBuf);

impl TempDir {
    fn new(test: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("pbr-testpng-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn check_writes_then_passes_then_fails() {
    let dir = TempDir::new("check");
    // The folder doesn't exist yet: updating makes it.
    let file = dir.0.join("deeper").join("check-test-picture.png");
    let picture = gradient(6, 5, 255);

    // Missing file.
    let message = check_with(&file, &picture, false).unwrap_err();
    assert!(message.contains("check-test-picture.png"), "{message}");
    assert!(message.contains("PBR_UPDATE_PICTURES=1 cargo xtask test"), "{message}");
    assert!(message.contains("look at the new picture"), "{message}");

    // Writing.
    check_with(&file, &picture, true).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), encode(&picture).unwrap());
    check_with(&file, &picture, false).unwrap();

    // Updating with the same picture leaves the file alone.
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    check_with(&file, &picture, true).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);

    // A changed picture fails, and what was drawn is saved for a person to open.
    let mut changed = picture.clone();
    changed.pixels[8] = 0xFF00_FF00;
    let saved = got_path(&file);
    let _ = std::fs::remove_file(&saved);
    let message = check_with(&file, &changed, false).unwrap_err();
    assert!(message.contains("1 of 30 pixels differ"), "{message}");
    assert!(message.contains(&saved.display().to_string()), "{message}");
    assert_eq!(decode(&std::fs::read(&saved).unwrap()).unwrap(), changed);
    let _ = std::fs::remove_file(&saved);
    // The reference itself was not touched.
    assert_eq!(std::fs::read(&file).unwrap(), encode(&picture).unwrap());

    // Updating changes it.
    check_with(&file, &changed, true).unwrap();
    check_with(&file, &changed, false).unwrap();

    // A bad picture can't be written; a file that isn't a PNG is named in the error.
    let bad = Picture { width: 0, height: 0, pixels: vec![] };
    assert!(check_with(&file, &bad, true).unwrap_err().contains("check-test-picture.png"));
    std::fs::write(&file, b"not a picture").unwrap();
    let message = check_with(&file, &picture, false).unwrap_err();
    assert!(
        message.contains("check-test-picture.png") && message.contains("signature"),
        "{message}"
    );
}

#[test]
fn check_with_a_file_name_that_has_no_folder() {
    // The update path must cope with a parent of "" (a bare file name); run it in a folder of
    // its own by using a path that has one, and check the name-only case just fails cleanly.
    let message =
        check_with(Path::new("pbr-testpng-no-such-file.png"), &gradient(1, 1, 255), false)
            .unwrap_err();
    assert!(message.contains("doesn't exist"), "{message}");
}
