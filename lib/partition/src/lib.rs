// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding the volumes on a disk (docs/design/0015-storage-and-the-block-cache.md, section 2).
//!
//! A disk is a long row of 512-byte **sectors**. Usually its first sectors hold a **partition
//! table**, which divides the rest into **partitions**: one might hold Windows, another a
//! RockFS volume. There are two kinds of table:
//! - the **MBR** (master boot record, 1983): sector 0, with room for four partitions;
//! - the **GPT** (GUID partition table, part of UEFI): a header in sector 1 and a list of up to
//!   128 partitions after it, each with checksums, and a backup copy at the end of the disk. A
//!   GPT disk still has an MBR, with one "protective" entry of type `0xEE` covering the disk, so
//!   old tools see it as in use.
//!
//! Some disks have no table at all: a USB stick formatted as one big FAT volume, or a RockFS
//! "whole disk" volume (ROCKFS.md, "Where a volume lives").
//!
//! Then each volume's **filesystem** is identified by what's in it, not by the partition's
//! type, which can be wrong: RockFS by its superblock's magic and checksum, FAT32 by its boot
//! sector, empty by zeros.
//!
//! **Everything on the disk is hostile input.** A disk could come from anywhere, and a
//! damaged or deliberately malicious table must never crash the storage service or make it
//! read outside the disk. So no number read from the disk is used for indexing, multiplying or
//! adding without a check, and a table whose partitions run off the disk or overlap is
//! reported as damaged and none of it is used. Tests on the host feed it synthetic tables and
//! random corruption of real ones.

#![no_std]

// `alloc` is the part of Rust's standard library that needs a memory allocator but no
// operating system: `Vec` (a growable array) and `String`.
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(test)]
mod tests;

/// Bytes in a sector. Disks with other sector sizes aren't supported yet (note 0015).
pub const SECTOR: usize = 512;

/// The most volumes reported for one disk (a GPT may list 128 partitions).
pub const MAX_VOLUMES: usize = 128;

/// The GPT partition type of a RockFS volume (ROCKFS.md), as it's stored: GUIDs are stored with
/// their first three fields little-endian ("mixed-endian").
pub const ROCKFS_GPT_TYPE: [u8; 16] =
    guid(0x0397_1446, 0x6A02, 0x47DC, [0xBB, 0x5F, 0x6A, 0x75, 0x72, 0xE5, 0x24, 0x7C]);
/// The EFI system partition's GPT type.
pub const ESP_GPT_TYPE: [u8; 16] =
    guid(0xC12A_7328, 0xF81F, 0x11D2, [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
/// The MBR partition type of a RockFS volume.
pub const ROCKFS_MBR_TYPE: u8 = 0x7F;

/// A GUID's 16 bytes as GPT stores them.
const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// A disk to read from. A *trait* describes what a type can do (like an interface in other
/// languages): the storage service implements it over a disk driver, and the tests over a
/// `Vec` of bytes.
pub trait Disk {
    /// How many sectors the disk has.
    fn sectors(&self) -> u64;
    /// Reads `buf.len() / SECTOR` sectors from `sector`.
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), ReadError>;
}

/// The disk couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadError;

/// What a volume's contents look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filesystem {
    /// A RockFS superblock (or its backup) with the right magic, version and checksum.
    RockFs,
    /// A FAT32 boot sector.
    Fat32,
    /// Nothing but zeros in its first 4 KiB: never used.
    Empty,
    /// Anything else.
    Unknown,
}

/// Which partition table the disk has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    /// None: the disk is one volume.
    None,
    /// An MBR.
    Mbr,
    /// A GPT.
    Gpt,
}

/// One volume found on a disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Its partition's number from 1 (its position in the table), or 0 for a whole disk.
    pub partition: u32,
    /// Its first sector.
    pub start: u64,
    /// Its length in sectors.
    pub sectors: u64,
    /// What it contains.
    pub filesystem: Filesystem,
    /// Its name, from the filesystem (RockFS's label, FAT's volume label), or empty.
    pub label: String,
}

/// Something about the disk worth saying in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The partition table is damaged (the reason), so none of it was used.
    Damaged(&'static str),
    /// The GPT's main copy was damaged; its backup at the end of the disk was used.
    UsedBackupGpt,
    /// An MBR extended partition (number), which isn't supported yet.
    Extended(u32),
    /// More partitions than [`MAX_VOLUMES`]; the rest were ignored.
    TooMany,
}

/// What [`scan`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    /// The partition table.
    pub table: Table,
    /// The volumes, in table order.
    pub volumes: Vec<Volume>,
    /// Anything worth reporting.
    pub problems: Vec<Problem>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32
}

/// Reads `count` sectors from `at`, refusing anything past the end of the disk.
fn read(disk: &mut impl Disk, at: u64, count: usize) -> Result<Vec<u8>, ReadError> {
    let end = at.checked_add(count as u64).ok_or(ReadError)?;
    if end > disk.sectors() {
        return Err(ReadError);
    }
    let mut buf = vec![0u8; count * SECTOR];
    disk.read(at, &mut buf)?;
    Ok(buf)
}

/// Finds the volumes on `disk`. Fails only if the disk can't be read; a damaged table is
/// reported in [`Scan::problems`], with no volumes.
///
/// (`impl Disk` means "any type that implements the `Disk` trait"; Rust makes a copy of the
/// function for each type it's used with.)
pub fn scan(disk: &mut impl Disk) -> Result<Scan, ReadError> {
    let total = disk.sectors();
    let mut out = Scan { table: Table::None, volumes: Vec::new(), problems: Vec::new() };
    if total == 0 {
        return Ok(out);
    }
    let first = read(disk, 0, 1)?;

    // A RockFS volume on the whole disk has its superblock in sector 0, so look for it first
    // (ROCKFS.md, "Where a volume lives").
    if total >= 2 {
        let two = read(disk, 0, 2)?;
        if rockfs_superblock(&two).is_some() {
            out.volumes.push(whole(disk, total)?);
            return Ok(out);
        }
    }
    let signed = first[510] == 0x55 && first[511] == 0xAA;
    // A FAT volume on the whole disk (a "superfloppy") also ends its first sector with 0x55AA,
    // so it's told apart from an MBR by parsing as a FAT boot sector.
    if signed && fat32_boot_sector(&first, total).is_some() {
        out.volumes.push(whole(disk, total)?);
        return Ok(out);
    }
    if !signed {
        out.volumes.push(whole(disk, total)?);
        return Ok(out);
    }
    let entries: Vec<(u8, u64, u64)> = (0..4)
        .map(|i| {
            let e = 446 + 16 * i;
            (first[e + 4], u64::from(u32_at(&first, e + 8)), u64::from(u32_at(&first, e + 12)))
        })
        .collect();
    if entries.iter().any(|&(kind, _, _)| kind == 0xEE) {
        out.table = Table::Gpt;
        gpt(disk, total, &mut out)?;
    } else {
        out.table = Table::Mbr;
        mbr(disk, total, &entries, &mut out)?;
    }
    Ok(out)
}

/// The whole disk as one volume.
fn whole(disk: &mut impl Disk, total: u64) -> Result<Volume, ReadError> {
    identify(disk, 0, total, 0)
}

/// Checks a list of (partition number, start, length) against the disk and each other: every
/// partition inside the usable area, none overlapping. Returns why not, if not.
fn check_layout(
    parts: &mut [(u32, u64, u64)],
    first_usable: u64,
    end: u64,
) -> Result<(), &'static str> {
    for &(_, start, len) in parts.iter() {
        if len == 0 {
            return Err("a partition has no sectors");
        }
        let Some(stop) = start.checked_add(len) else { return Err("a partition's size wraps") };
        if start < first_usable || stop > end {
            return Err("a partition lies outside the disk");
        }
    }
    parts.sort_by_key(|p| p.1);
    for w in parts.windows(2) {
        // Both were checked above, so this addition can't wrap.
        if w[0].1 + w[0].2 > w[1].1 {
            return Err("two partitions overlap");
        }
    }
    Ok(())
}

fn mbr(
    disk: &mut impl Disk,
    total: u64,
    entries: &[(u8, u64, u64)],
    out: &mut Scan,
) -> Result<(), ReadError> {
    let mut parts = Vec::new();
    for (i, &(kind, start, len)) in entries.iter().enumerate() {
        let number = i as u32 + 1;
        match kind {
            0 => {}
            // Extended partitions hold a chain of further tables. Not in M5 (note 0015).
            0x05 | 0x0F | 0x85 => out.problems.push(Problem::Extended(number)),
            _ => parts.push((number, start, len)),
        }
    }
    if let Err(why) = check_layout(&mut parts, 1, total) {
        out.problems.push(Problem::Damaged(why));
        return Ok(());
    }
    parts.sort_by_key(|p| p.0);
    for (number, start, len) in parts {
        out.volumes.push(identify(disk, start, len, number)?);
    }
    Ok(())
}

/// A GPT header, checked.
struct GptHeader {
    entries_at: u64,
    count: u32,
    size: u32,
    entries_crc: u32,
    first_usable: u64,
    last_usable: u64,
}

/// Reads and checks the GPT header at `at`, which should say it is at `at`.
fn gpt_header(disk: &mut impl Disk, at: u64, total: u64) -> Result<Option<GptHeader>, ReadError> {
    let s = read(disk, at, 1)?;
    if &s[0..8] != b"EFI PART" {
        return Ok(None);
    }
    let size = u32_at(&s, 12) as usize;
    if !(92..=SECTOR).contains(&size) {
        return Ok(None);
    }
    let mut h = s[..size].to_vec();
    h[16..20].fill(0); // the header's checksum is worked out with its own field as 0
    if pbr_crc::crc32(&h) != u32_at(&s, 16) || u64_at(&s, 24) != at {
        return Ok(None);
    }
    let header = GptHeader {
        first_usable: u64_at(&s, 40),
        last_usable: u64_at(&s, 48),
        entries_at: u64_at(&s, 72),
        count: u32_at(&s, 80),
        size: u32_at(&s, 84),
        entries_crc: u32_at(&s, 88),
    };
    // The entry list must be a sane size, and lie on the disk outside the usable area.
    let size_ok = header.size >= 128 && header.size <= 1024 && header.size % 8 == 0;
    let bytes = u64::from(header.count) * u64::from(header.size);
    let usable_ok = header.first_usable <= header.last_usable && header.last_usable < total;
    if !size_ok || header.count == 0 || bytes > 128 * 1024 || !usable_ok {
        return Ok(None);
    }
    let sectors = bytes.div_ceil(SECTOR as u64);
    match header.entries_at.checked_add(sectors) {
        Some(end) if end <= total && header.entries_at >= 1 => Ok(Some(header)),
        _ => Ok(None),
    }
}

fn gpt(disk: &mut impl Disk, total: u64, out: &mut Scan) -> Result<(), ReadError> {
    // The main copy, else the backup in the last sector.
    let mut chosen = None;
    for (at, backup) in [(1, false), (total - 1, true)] {
        if at >= total {
            continue;
        }
        let Some(h) = gpt_header(disk, at, total)? else { continue };
        let bytes = h.count as usize * h.size as usize;
        let list = read(disk, h.entries_at, bytes.div_ceil(SECTOR))?;
        if pbr_crc::crc32(&list[..bytes]) == h.entries_crc {
            chosen = Some((h, list, backup));
            break;
        }
    }
    let Some((h, list, backup)) = chosen else {
        out.problems.push(Problem::Damaged("neither copy of the GPT is intact"));
        return Ok(());
    };
    if backup {
        out.problems.push(Problem::UsedBackupGpt);
    }
    let mut parts = Vec::new();
    for i in 0..h.count as usize {
        let e = &list[i * h.size as usize..][..128];
        if e[..16].iter().all(|&b| b == 0) {
            continue; // an unused entry
        }
        let (first, last) = (u64_at(e, 32), u64_at(e, 40));
        if last < first {
            out.problems.push(Problem::Damaged("a partition ends before it starts"));
            return Ok(());
        }
        if parts.len() == MAX_VOLUMES {
            out.problems.push(Problem::TooMany);
            break;
        }
        // `last` is inclusive; its sector count can't wrap because it's below the disk's size
        // (checked next), but `checked_add` keeps that from depending on order.
        let Some(len) = (last - first).checked_add(1) else {
            out.problems.push(Problem::Damaged("a partition's size wraps"));
            return Ok(());
        };
        parts.push((i as u32 + 1, first, len));
    }
    let end = h.last_usable.saturating_add(1);
    if let Err(why) = check_layout(&mut parts, h.first_usable, end) {
        out.problems.push(Problem::Damaged(why));
        return Ok(());
    }
    parts.sort_by_key(|p| p.0);
    for (number, start, len) in parts {
        out.volumes.push(identify(disk, start, len, number)?);
    }
    Ok(())
}

/// Works out what the volume of `len` sectors at `start` contains.
fn identify(
    disk: &mut impl Disk,
    start: u64,
    len: u64,
    partition: u32,
) -> Result<Volume, ReadError> {
    let mut v = Volume {
        partition,
        start,
        sectors: len,
        filesystem: Filesystem::Unknown,
        label: String::new(),
    };
    // The first 4 KiB (8 sectors), or as much of the volume as there is.
    let head = read(disk, start, len.min(8) as usize)?;
    if let Some(label) = rockfs_superblock(&head) {
        v.filesystem = Filesystem::RockFs;
        v.label = label;
        return Ok(v);
    }
    // A damaged main superblock: try the backup in the volume's last whole 4 KiB block.
    if len >= 16 {
        let last_block = len / 8 - 1;
        let backup = read(disk, start + last_block * 8, 2)?;
        if let Some(label) = rockfs_superblock(&backup) {
            v.filesystem = Filesystem::RockFs;
            v.label = label;
            return Ok(v);
        }
    }
    if let Some(label) = fat32_boot_sector(&head[..SECTOR], len) {
        v.filesystem = Filesystem::Fat32;
        v.label = label;
        return Ok(v);
    }
    if head.iter().all(|&b| b == 0) {
        v.filesystem = Filesystem::Empty;
    }
    Ok(v)
}

/// If `b` (at least 1024 bytes) starts with a valid RockFS superblock, its label.
pub fn rockfs_superblock(b: &[u8]) -> Option<String> {
    if b.len() < 1024 || &b[0..8] != b"ROCKFS!\0" || u16_at(b, 8) != 1 {
        return None;
    }
    if pbr_crc::crc32(&b[..1020]) != u32_at(b, 1020) {
        return None;
    }
    Some(text(&b[128..192]))
}

/// If `s` (one sector) is a FAT32 boot sector for a volume of `len` sectors, its label.
///
/// FAT32 is recognised as Linux recognises it: the FAT12/16 fields (the root folder's entry
/// count and the 16-bit FAT size) are zero and the 32-bit FAT size isn't. (Microsoft's rule,
/// by cluster count, would call our build tool's small EFI partition FAT16, though every
/// firmware reads it as FAT32.)
pub fn fat32_boot_sector(s: &[u8], len: u64) -> Option<String> {
    if s.len() < SECTOR || s[510] != 0x55 || s[511] != 0xAA || !matches!(s[0], 0xEB | 0xE9) {
        return None;
    }
    let bytes_per_sector = u16_at(s, 11);
    let per_cluster = s[13];
    let reserved = u16_at(s, 14);
    let fats = s[16];
    let root_entries = u16_at(s, 17);
    let fat16_size = u16_at(s, 22);
    let total =
        u64::from(if u16_at(s, 19) != 0 { u32::from(u16_at(s, 19)) } else { u32_at(s, 32) });
    let fat_size = u64::from(u32_at(s, 36));
    let root_cluster = u32_at(s, 44);
    let ok = bytes_per_sector == 512
        && per_cluster != 0
        && per_cluster.is_power_of_two()
        && reserved != 0
        && fats != 0
        && root_entries == 0
        && fat16_size == 0
        && fat_size != 0
        && root_cluster >= 2
        && total != 0
        && total <= len
        && u64::from(reserved) + u64::from(fats) * fat_size < total;
    if !ok {
        return None;
    }
    // The volume label, if the extended boot signature says it's there.
    let label = if s[66] == 0x29 { text(&s[71..82]) } else { String::new() };
    Some(if label == "NO NAME" { String::new() } else { label })
}

/// Text from a fixed-size field: up to the first zero byte, without trailing spaces. Bytes that
/// aren't valid UTF-8 make it empty rather than wrong.
fn text(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    match core::str::from_utf8(&b[..n]) {
        Ok(s) => String::from(s.trim_end_matches(' ')),
        Err(_) => String::new(),
    }
}
