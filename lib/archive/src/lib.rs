// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The start-up archive (docs/design/0009-programs-and-granite.md, section 1): the one file
//! format the kernel unpacks, holding the root program and what it needs to start the rest.
//!
//! [`Archive::parse`] checks every rule in the specification before anything is used, so a
//! damaged or hostile archive is reported, never trusted. [`build`] writes one.
//! `tools/startarc.py` is an independent reader, for cross-checking.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

/// The magic at the start of every archive.
pub const MAGIC: &[u8; 8] = b"PBRARCHV";
/// The version this code writes, and the major version it reads.
pub const VERSION: (u16, u16) = (1, 0);
/// Files and the archive's total size are multiples of this.
pub const ALIGN: u64 = 4096;
/// The size of the header and of each entry.
pub const HEADER: usize = 64;
/// The longest name, in bytes.
pub const MAX_NAME: usize = 47;

/// Why an archive was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than its header says, or than a header at all.
    Truncated,
    /// The magic is wrong: this isn't a start-up archive.
    NotAnArchive,
    /// A major version this code doesn't understand.
    UnsupportedVersion(u16),
    /// The total size isn't a multiple of 4096.
    BadSize,
    /// The CRC-32 doesn't match: the archive is damaged.
    BadChecksum,
    /// A reserved field isn't zero.
    ReservedNotZero,
    /// Entry `n` has a bad name (empty, too long, not UTF-8, a zero byte inside, or bad padding).
    BadName(u32),
    /// Two entries have the same name (the second is entry `n`).
    DuplicateName(u32),
    /// Entry `n`'s file isn't page-aligned, overlaps another, the entry table, or runs past the end.
    BadPlacement(u32),
    /// The space between files isn't zero.
    GapNotZero,
    /// Too many files for the format.
    TooManyFiles,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => write!(f, "the archive is cut short"),
            Error::NotAnArchive => write!(f, "this isn't a start-up archive"),
            Error::UnsupportedVersion(v) => write!(f, "archive version {v} isn't supported"),
            Error::BadSize => write!(f, "the archive's size isn't a whole number of pages"),
            Error::BadChecksum => write!(f, "the archive is damaged (its checksum doesn't match)"),
            Error::ReservedNotZero => write!(f, "a reserved field isn't zero"),
            Error::BadName(n) => write!(f, "file {n} has an invalid name"),
            Error::DuplicateName(n) => write!(f, "file {n}'s name is used twice"),
            Error::BadPlacement(n) => write!(f, "file {n} is in the wrong place"),
            Error::GapNotZero => write!(f, "the space between files isn't empty"),
            Error::TooManyFiles => write!(f, "too many files"),
        }
    }
}

/// The CRC-32 used by zip and PNG (polynomial 0xEDB88320, reflected).
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

/// A checked archive.
#[derive(Clone, Copy)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    count: u32,
}

/// One file in an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// Its name.
    pub name: &'a str,
    /// Where it starts, from the start of the archive (a multiple of 4096).
    pub offset: u64,
    /// Its contents.
    pub data: &'a [u8],
}

impl<'a> Archive<'a> {
    /// Checks `bytes` against every rule of the format. `bytes` may be longer than the archive
    /// (a loader may round it up); only the archive's own length is used.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER {
            return Err(Error::Truncated);
        }
        if &bytes[..8] != MAGIC {
            return Err(Error::NotAnArchive);
        }
        let (major, minor) = (u16_at(bytes, 8), u16_at(bytes, 10));
        if major != VERSION.0 {
            return Err(Error::UnsupportedVersion(major));
        }
        let count = u32_at(bytes, 12);
        let total = u64_at(bytes, 16);
        if total == 0 || !total.is_multiple_of(ALIGN) {
            return Err(Error::BadSize);
        }
        if total > bytes.len() as u64 {
            return Err(Error::Truncated);
        }
        let bytes = &bytes[..total as usize];
        // The checksum covers everything, with its own field taken as zero.
        let mut crc = crc32_update(0xFFFF_FFFF, &bytes[..24]);
        crc = crc32_update(crc, &[0; 4]);
        crc = crc32_update(crc, &bytes[28..]) ^ 0xFFFF_FFFF;
        if crc != u32_at(bytes, 24) {
            return Err(Error::BadChecksum);
        }
        if minor == 0 && bytes[28..HEADER].iter().any(|&b| b != 0) {
            return Err(Error::ReservedNotZero);
        }
        let table_end = (count as u64)
            .checked_mul(HEADER as u64)
            .and_then(|t| t.checked_add(HEADER as u64))
            .ok_or(Error::TooManyFiles)?;
        if table_end > total {
            return Err(Error::Truncated);
        }
        let a = Archive { bytes, count };
        // Names, order, placement; and the gaps between files must be zero.
        let mut gap_from = table_end;
        let mut names: Vec<&str> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let e = HEADER + i as usize * HEADER;
            let raw = &bytes[e..e + 48];
            let len = raw.iter().position(|&b| b == 0).unwrap_or(48);
            if len == 0 || len > MAX_NAME || raw[len..].iter().any(|&b| b != 0) {
                return Err(Error::BadName(i));
            }
            let name = core::str::from_utf8(&raw[..len]).map_err(|_| Error::BadName(i))?;
            if names.contains(&name) {
                return Err(Error::DuplicateName(i));
            }
            names.push(name);
            let offset = u64_at(bytes, e + 48);
            let size = u64_at(bytes, e + 56);
            let end = offset.checked_add(size).ok_or(Error::BadPlacement(i))?;
            if !offset.is_multiple_of(ALIGN) || offset < gap_from || end > total {
                return Err(Error::BadPlacement(i));
            }
            if bytes[gap_from as usize..offset as usize].iter().any(|&b| b != 0) {
                return Err(Error::GapNotZero);
            }
            gap_from = end;
        }
        if bytes[gap_from as usize..].iter().any(|&b| b != 0) {
            return Err(Error::GapNotZero);
        }
        Ok(a)
    }

    /// The archive's own length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True if the archive holds no files.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The number of files.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// File number `i`.
    pub fn entry(&self, i: u32) -> Option<Entry<'a>> {
        if i >= self.count {
            return None;
        }
        let e = HEADER + i as usize * HEADER;
        let raw = &self.bytes[e..e + 48];
        let len = raw.iter().position(|&b| b == 0).unwrap_or(48);
        let name = core::str::from_utf8(&raw[..len]).ok()?;
        let offset = u64_at(self.bytes, e + 48);
        let size = u64_at(self.bytes, e + 56);
        Some(Entry { name, offset, data: &self.bytes[offset as usize..(offset + size) as usize] })
    }

    /// Every file, in order.
    pub fn entries(&self) -> impl Iterator<Item = Entry<'a>> + '_ {
        (0..self.count).filter_map(|i| self.entry(i))
    }

    /// The file called `name`.
    pub fn find(&self, name: &str) -> Option<Entry<'a>> {
        self.entries().find(|e| e.name == name)
    }
}

/// Writes an archive holding `files` (name, contents), in that order.
pub fn build(files: &[(&str, &[u8])]) -> Result<Vec<u8>, Error> {
    let count = u32::try_from(files.len()).map_err(|_| Error::TooManyFiles)?;
    let mut out = alloc::vec![0u8; HEADER * (files.len() + 1)];
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.0.to_le_bytes());
    out[10..12].copy_from_slice(&VERSION.1.to_le_bytes());
    out[12..16].copy_from_slice(&count.to_le_bytes());
    for (i, (name, data)) in files.iter().enumerate() {
        if name.is_empty() || name.len() > MAX_NAME || name.contains('\0') {
            return Err(Error::BadName(i as u32));
        }
        if files[..i].iter().any(|(n, _)| n == name) {
            return Err(Error::DuplicateName(i as u32));
        }
        out.resize(out.len().next_multiple_of(ALIGN as usize), 0);
        let offset = out.len() as u64;
        out.extend_from_slice(data);
        let e = HEADER * (i + 1);
        out[e..e + name.len()].copy_from_slice(name.as_bytes());
        out[e + 48..e + 56].copy_from_slice(&offset.to_le_bytes());
        out[e + 56..e + 64].copy_from_slice(&(data.len() as u64).to_le_bytes());
    }
    out.resize(out.len().next_multiple_of(ALIGN as usize).max(ALIGN as usize), 0);
    let total = out.len() as u64;
    out[16..24].copy_from_slice(&total.to_le_bytes());
    let crc = crc32(&out);
    out[24..28].copy_from_slice(&crc.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        build(&[("root", b"\x7fELF root"), ("drivers/serial", &[7u8; 5000]), ("empty", b"")])
            .unwrap()
    }

    /// Recomputes the checksum after a deliberate change, so the test reaches the later checks.
    fn reseal(a: &mut [u8]) {
        a[24..28].fill(0);
        let c = crc32(a);
        a[24..28].copy_from_slice(&c.to_le_bytes());
    }

    #[test]
    fn known_crc() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trip() {
        let bytes = sample();
        assert_eq!(bytes.len() % 4096, 0);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.count(), 3);
        let names: Vec<&str> = a.entries().map(|e| e.name).collect();
        assert_eq!(names, ["root", "drivers/serial", "empty"]);
        assert_eq!(a.find("root").unwrap().data, b"\x7fELF root");
        assert_eq!(a.find("drivers/serial").unwrap().data.len(), 5000);
        assert_eq!(a.find("empty").unwrap().data, b"");
        assert!(a.entries().all(|e| e.offset % 4096 == 0));
        assert!(a.find("missing").is_none());
        // A loader may hand over more than the archive.
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0xAA; 4096]);
        assert_eq!(Archive::parse(&longer).unwrap().len(), bytes.len());
        // An archive with nothing in it is still an archive.
        let empty = build(&[]).unwrap();
        assert!(Archive::parse(&empty).unwrap().is_empty());
    }

    #[test]
    fn refuses_bad_archives() {
        let good = sample();
        let mut a = good.clone();
        a[0] = b'X';
        assert_eq!(Archive::parse(&a).err(), Some(Error::NotAnArchive));
        let mut a = good.clone();
        a[8] = 2;
        assert_eq!(Archive::parse(&a).err(), Some(Error::UnsupportedVersion(2)));
        let mut a = good.clone();
        a[5000] ^= 1;
        assert_eq!(Archive::parse(&a).err(), Some(Error::BadChecksum));
        assert_eq!(Archive::parse(&good[..good.len() - 4096]).err(), Some(Error::Truncated));
        assert_eq!(Archive::parse(&good[..10]).err(), Some(Error::Truncated));
        // A file pointing outside the archive.
        let mut a = good.clone();
        a[64 + 56..64 + 64].copy_from_slice(&u64::MAX.to_le_bytes());
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::BadPlacement(0)));
        // A file overlapping the entry table.
        let mut a = good.clone();
        a[64 + 48..64 + 56].copy_from_slice(&0u64.to_le_bytes());
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::BadPlacement(0)));
        // Two names the same.
        let mut a = good.clone();
        a[128..128 + 48].fill(0);
        a[128..132].copy_from_slice(b"root");
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::DuplicateName(1)));
        // A name with junk after its end, and an invalid UTF-8 name.
        let mut a = good.clone();
        a[64 + 10] = b'x';
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::BadName(0)));
        let mut a = good.clone();
        a[64] = 0xFF;
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::BadName(0)));
        // Something hidden between files.
        let mut a = good.clone();
        a[4096 + 100] = 1;
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::GapNotZero));
        // A reserved field set.
        let mut a = good.clone();
        a[40] = 1;
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::ReservedNotZero));
        // A count so large the table can't fit.
        let mut a = good.clone();
        a[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        reseal(&mut a);
        assert_eq!(Archive::parse(&a).err(), Some(Error::Truncated));
        // Names the writer refuses.
        assert_eq!(build(&[("", b"")]).err(), Some(Error::BadName(0)));
        assert_eq!(build(&[(&"x".repeat(48), b"")]).err(), Some(Error::BadName(0)));
        assert_eq!(build(&[("a", b""), ("a", b"")]).err(), Some(Error::DuplicateName(1)));
    }

    /// Flipping any single byte of the header or entry table is always caught.
    #[test]
    fn every_header_byte_matters() {
        let good = sample();
        for i in 0..256 {
            let mut a = good.clone();
            a[i] ^= 0x41;
            assert!(Archive::parse(&a).is_err(), "byte {i}");
        }
    }
}
