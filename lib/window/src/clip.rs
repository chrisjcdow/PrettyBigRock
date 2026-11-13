// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning clipboard items into bytes and back (note 0022, section 5). A picture crosses as
//! `Bgrx` pixels, each a little-endian `u32`, the way a window's frames are.

use alloc::string::String;
use alloc::vec::Vec;

/// A picture's pixels as bytes.
pub fn pixels_to_bytes(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        out.extend_from_slice(&p.to_le_bytes());
    }
    out
}

/// Bytes as pixels; a last partial pixel is dropped.
pub fn bytes_to_pixels(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Text from bytes, or `None` if they are not UTF-8 (the clipboard comes from another app, so
/// it is not trusted to be).
pub fn text_from_bytes(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_round_trip() {
        let p = [0u32, 1, 0x00FF_8040, u32::MAX];
        let b = pixels_to_bytes(&p);
        assert_eq!(b.len(), 16);
        assert_eq!(&b[8..12], [0x40, 0x80, 0xFF, 0x00]);
        assert_eq!(bytes_to_pixels(&b), p);
        assert_eq!(bytes_to_pixels(&b[..14]), p[..3]);
        assert!(bytes_to_pixels(&[]).is_empty());
    }

    #[test]
    fn text_must_be_utf8() {
        assert_eq!(text_from_bytes("héllo".as_bytes().to_vec()).as_deref(), Some("héllo"));
        assert_eq!(text_from_bytes(alloc::vec![0xFF, 0xFE]), None);
        assert_eq!(text_from_bytes(Vec::new()).as_deref(), Some(""));
    }
}
