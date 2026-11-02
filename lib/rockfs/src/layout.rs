// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The fixed sizes of RockFS version 1 (docs/ROCKFS.md) and the small helpers every other
//! module uses to read and write numbers inside byte buffers.
//!
//! Everything on a RockFS disk is **little-endian**: the least significant byte of a number
//! comes first. Rust spells that `u32::from_le_bytes` and `to_le_bytes`.

/// Bytes in a block, the unit RockFS allocates (ROCKFS.md, "Basics").
pub const BLOCK: usize = 4096;
/// [`BLOCK`] as a `u64`, for arithmetic on block numbers and file offsets. (Rust never converts
/// between number types silently, so both spellings are needed.)
pub const BLOCK_U64: u64 = BLOCK as u64;
/// Sectors (512 bytes, what the disk reads and writes) in a block.
pub const SECTORS_PER_BLOCK: u64 = 8;
/// Bytes in an inode.
pub const INODE_SIZE: usize = 256;
/// Inodes in a block.
pub const INODES_PER_BLOCK: u32 = 16;
/// Extent or index entries held in the inode itself.
pub const IN_INODE: usize = 7;
/// Entries in one extent block.
pub const PER_XBLOCK: usize = 170;
/// The deepest extent tree accepted. Real files never need more than 4 (7 x 170^4 extents);
/// the limit exists so a damaged tree cannot make a reader recurse without end.
pub const MAX_DEPTH: u16 = 8;
/// Bytes the superblock occupies at the start of its block.
pub const SUPERBLOCK_SIZE: usize = 1024;
/// The smallest volume `format` makes (256 KiB) and `mount` accepts.
pub const MIN_BLOCKS: u64 = 64;
/// Blocks one bitmap block describes (one bit each).
pub const BITS_PER_BITMAP_BLOCK: u64 = (BLOCK * 8) as u64;
/// The longest name, in bytes of UTF-8 (255 characters of up to 4 bytes).
pub const MAX_NAME_BYTES: usize = 1020;
/// The longest name, in characters.
pub const MAX_NAME_CHARS: usize = 255;
/// Largest transfer a [`pbr_fs::BlockDevice`] accepts in one call.
pub const MAX_TRANSFER: usize = 65536;

/// Inode type: free.
pub const TYPE_FREE: u16 = 0;
/// Inode type: file.
pub const TYPE_FILE: u16 = 1;
/// Inode type: folder.
pub const TYPE_FOLDER: u16 = 2;

/// Reads a `u16` at byte `at`. The callers all use fixed offsets inside buffers of known size,
/// so `at + 2` is always inside `b`; nothing here depends on a number from the disk.
pub fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// Reads a `u32` at byte `at` (see [`u16_at`]).
pub fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads a `u64` at byte `at` (see [`u16_at`]).
pub fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32
}

/// Writes a `u16` at byte `at`.
pub fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Writes a `u32` at byte `at`.
pub fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Writes a `u64` at byte `at`.
pub fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Rounds up to a multiple of 8, as folder entries need.
pub fn align8(n: usize) -> usize {
    (n + 7) & !7
}
