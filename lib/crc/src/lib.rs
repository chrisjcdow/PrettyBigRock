// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! CRC-32: the checksum used by ZIP, PNG, GPT partition tables and RockFS (the IEEE polynomial,
//! bit-reversed, as Python's `zlib.crc32` computes it).
//!
//! A checksum is a short number worked out from a block of data, stored beside it. If the data
//! is later damaged, the number worked out again no longer matches, so damage is noticed instead
//! of trusted. CRC-32 catches every burst of errors up to 32 bits long, and almost all others.
//!
//! `#![no_std]` means this library uses only Rust's `core` library, not the standard library
//! (which needs an operating system underneath), so the kernel, programs and the host can all
//! use it.

#![no_std]

/// The table of each byte's CRC, worked out when the program is compiled: a `const fn` is a
/// function Rust can run at compile time, so the table costs nothing at run time.
const TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    update(0, data)
}

/// Continues a CRC-32 over more data: `update(crc32(a), b)` is the CRC-32 of `a` followed by
/// `b`, so data in several pieces needn't be joined first.
pub fn update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        // The standard check value, and values zlib gives.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn in_pieces() {
        let data = b"PrettyBigRock RockFS superblock";
        for split in 0..data.len() {
            assert_eq!(update(crc32(&data[..split]), &data[split..]), crc32(data));
        }
    }
}
