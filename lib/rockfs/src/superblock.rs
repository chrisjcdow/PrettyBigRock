// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The superblock (ROCKFS.md, "The superblock"): block 0 of the volume, with a backup copy in
//! the last block.
//!
//! A *superblock* is the record at a fixed place that says what the rest of the volume looks
//! like: how big it is, where the bitmap and data start, where the inode table is, and
//! whether it was shut down cleanly. Everything else is found by starting here, so it is the
//! one thing a reader must be able to trust, and the reason there are two copies.

use crate::inode::Inode;
use crate::layout::*;
use alloc::string::String;
use alloc::vec::Vec;

/// The eight bytes every superblock starts with.
pub const MAGIC: [u8; 8] = *b"ROCKFS!\0";

/// The superblock's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    /// 1.
    pub version: u16,
    /// 12 (blocks are 2^12 bytes).
    pub block_shift: u16,
    /// How names are compared; 1 is "A-Z match a-z".
    pub name_rule: u16,
    /// Blocks in the volume, including the backup superblock.
    pub block_count: u64,
    /// First block of the bitmap (1).
    pub bitmap_start: u64,
    /// Blocks in the bitmap.
    pub bitmap_blocks: u64,
    /// First data block.
    pub data_start: u64,
    /// Free blocks: a hint, since the bitmap is authoritative.
    pub free_blocks: u64,
    /// Blocks held back for deleting and housekeeping.
    pub reserve_blocks: u64,
    /// Inode slots, including inode 0.
    pub inode_count: u32,
    /// Free inode slots: a hint.
    pub free_inodes: u32,
    /// The root folder's inode (1).
    pub root_inode: u32,
    /// 0 = shut down cleanly, 1 = in use.
    pub state: u32,
    /// Feature bits an older reader may ignore.
    pub features_compat: u32,
    /// Feature bits that make an older reader read-only.
    pub features_ro_compat: u32,
    /// Feature bits that make an older reader refuse the volume.
    pub features_incompat: u32,
    /// How many times the volume has been mounted.
    pub mount_count: u32,
    /// When it was formatted.
    pub created: u64,
    /// When it was last mounted.
    pub mounted: u64,
    /// A random ID chosen at format time.
    pub uuid: [u8; 16],
    /// The volume name, 64 bytes of UTF-8 padded with zeros.
    pub label: [u8; 64],
    /// The inode describing the inode table file.
    pub table: Inode,
}

impl Superblock {
    /// Reads a superblock from the first 1024 bytes of a block. Returns `None` unless the magic
    /// and checksum are right; nothing else is looked at until they are (ROCKFS.md: "A reader
    /// must check `magic`, `version` and `checksum` before trusting anything else"). The
    /// version is checked by the caller, which needs to tell "damaged" from "too new".
    pub fn parse(raw: &[u8]) -> Option<Superblock> {
        if raw.len() < SUPERBLOCK_SIZE || raw[0..8] != MAGIC {
            return None;
        }
        if pbr_crc::crc32(&raw[..1020]) != u32_at(raw, 1020) {
            return None;
        }
        // The embedded table inode is not verified on its own: the superblock's checksum
        // already covers its bytes. (Neither rockfs.c nor rockfs.py checks it either.)
        let table = Inode::parse(&raw[512..768], false).ok()?;
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&raw[112..128]);
        let mut label = [0u8; 64];
        label.copy_from_slice(&raw[128..192]);
        Some(Superblock {
            version: u16_at(raw, 8),
            block_shift: u16_at(raw, 10),
            name_rule: u16_at(raw, 12),
            block_count: u64_at(raw, 16),
            bitmap_start: u64_at(raw, 24),
            bitmap_blocks: u64_at(raw, 32),
            data_start: u64_at(raw, 40),
            free_blocks: u64_at(raw, 48),
            reserve_blocks: u64_at(raw, 56),
            inode_count: u32_at(raw, 64),
            free_inodes: u32_at(raw, 68),
            root_inode: u32_at(raw, 72),
            state: u32_at(raw, 76),
            features_compat: u32_at(raw, 80),
            features_ro_compat: u32_at(raw, 84),
            features_incompat: u32_at(raw, 88),
            mount_count: u32_at(raw, 92),
            created: u64_at(raw, 96),
            mounted: u64_at(raw, 104),
            uuid,
            label,
            table,
        })
    }

    /// The 1024 bytes to store, with a fresh checksum. Reserved bytes are zero.
    pub fn to_bytes(&self) -> [u8; SUPERBLOCK_SIZE] {
        let mut raw = [0u8; SUPERBLOCK_SIZE];
        raw[0..8].copy_from_slice(&MAGIC);
        put_u16(&mut raw, 8, self.version);
        put_u16(&mut raw, 10, self.block_shift);
        put_u16(&mut raw, 12, self.name_rule);
        put_u64(&mut raw, 16, self.block_count);
        put_u64(&mut raw, 24, self.bitmap_start);
        put_u64(&mut raw, 32, self.bitmap_blocks);
        put_u64(&mut raw, 40, self.data_start);
        put_u64(&mut raw, 48, self.free_blocks);
        put_u64(&mut raw, 56, self.reserve_blocks);
        put_u32(&mut raw, 64, self.inode_count);
        put_u32(&mut raw, 68, self.free_inodes);
        put_u32(&mut raw, 72, self.root_inode);
        put_u32(&mut raw, 76, self.state);
        put_u32(&mut raw, 80, self.features_compat);
        put_u32(&mut raw, 84, self.features_ro_compat);
        put_u32(&mut raw, 88, self.features_incompat);
        put_u32(&mut raw, 92, self.mount_count);
        put_u64(&mut raw, 96, self.created);
        put_u64(&mut raw, 104, self.mounted);
        raw[112..128].copy_from_slice(&self.uuid);
        raw[128..192].copy_from_slice(&self.label);
        raw[512..768].copy_from_slice(&self.table.to_bytes());
        let sum = pbr_crc::crc32(&raw[..1020]);
        put_u32(&mut raw, 1020, sum);
        raw
    }

    /// The label as text: up to the first zero byte. Bytes that are not valid UTF-8 are shown
    /// as the replacement character rather than hidden.
    pub fn label_text(&self) -> String {
        let n = self.label.iter().position(|&b| b == 0).unwrap_or(self.label.len());
        String::from_utf8_lossy(&self.label[..n]).into_owned()
    }

    /// Packs `text` into the 64-byte label field, cutting it to 63 bytes (so a zero byte always
    /// ends it, as rockfs.c does) at a character boundary.
    pub fn pack_label(text: &str) -> [u8; 64] {
        let mut end = text.len().min(63);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut out = [0u8; 64];
        out[..end].copy_from_slice(&text.as_bytes()[..end]);
        out
    }

    /// The bytes of the whole superblock block: the 1024 bytes, then zeros.
    pub fn block_bytes(&self) -> Vec<u8> {
        let mut block = alloc::vec![0u8; BLOCK];
        block[..SUPERBLOCK_SIZE].copy_from_slice(&self.to_bytes());
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extents::Extent;
    use crate::inode::Top;

    pub fn sample() -> Superblock {
        Superblock {
            version: 1,
            block_shift: 12,
            name_rule: 1,
            block_count: 4096,
            bitmap_start: 1,
            bitmap_blocks: 1,
            data_start: 2,
            free_blocks: 4000,
            reserve_blocks: 40,
            inode_count: 16,
            free_inodes: 14,
            root_inode: 1,
            state: 0,
            features_compat: 0,
            features_ro_compat: 0,
            features_incompat: 0,
            mount_count: 3,
            created: 1000,
            mounted: 2000,
            uuid: [7; 16],
            label: Superblock::pack_label("Data"),
            table: Inode {
                kind: TYPE_FILE,
                flags: 0,
                parent: 0,
                size: 4096,
                created: 1,
                modified: 1,
                generation: 0,
                depth: 0,
                top: Top::Extents(alloc::vec![Extent { file_block: 0, disk_block: 2, length: 1 }]),
            },
        }
    }

    #[test]
    fn round_trip() {
        let sb = sample();
        assert_eq!(Superblock::parse(&sb.to_bytes()), Some(sb.clone()));
        assert_eq!(sb.label_text(), "Data");
        assert_eq!(sb.block_bytes().len(), BLOCK);
    }

    #[test]
    fn damage_is_noticed() {
        let raw = sample().to_bytes();
        for at in [0, 9, 16, 100, 130, 600, 1019, 1020] {
            let mut bad = raw;
            bad[at] ^= 0x10;
            assert_eq!(Superblock::parse(&bad), None, "byte {at}");
        }
        assert_eq!(Superblock::parse(&raw[..1000]), None);
    }

    #[test]
    fn labels_are_cut_on_a_character_boundary() {
        let long = "é".repeat(40);
        let packed = Superblock::pack_label(&long);
        assert_eq!(packed[63], 0);
        let sb = Superblock { label: packed, ..sample() };
        assert_eq!(sb.label_text(), "é".repeat(31));
    }
}
