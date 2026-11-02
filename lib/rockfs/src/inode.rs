// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Inodes (ROCKFS.md, "Inodes"): the 256-byte record describing one file or folder.
//!
//! An *inode* holds everything about a file except its name: its type, size, dates, and where
//! its contents are. Names live in the folder that contains the file, so one inode could in
//! principle have several names, and renaming never touches the inode.

use crate::extents::Extent;
use crate::layout::*;
use alloc::vec::Vec;
use pbr_fs::FsError;

/// One entry of an index node: "file blocks from `file_block` on are described by the extent
/// block `child`".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// The first file block the child covers.
    pub file_block: u64,
    /// The disk block holding the child extent block.
    pub child: u64,
}

/// The up to 7 entries stored in the inode itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Top {
    /// Extents: the file's contents are right here (`extent_depth` is 0).
    Extents(Vec<Extent>),
    /// Index entries pointing at extent blocks (`extent_depth` is 1 or more).
    Index(Vec<IndexEntry>),
}

/// A parsed inode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inode {
    /// [`TYPE_FREE`], [`TYPE_FILE`] or [`TYPE_FOLDER`] (or, on a damaged disk, anything).
    pub kind: u16,
    /// Bit 0 is "hidden"; the other bits are reserved.
    pub flags: u16,
    /// The inode of the folder this one is in (the root's parent is itself).
    pub parent: u32,
    /// Size in bytes.
    pub size: u64,
    /// When it was created, seconds since 1970 UTC.
    pub created: u64,
    /// When its contents last changed.
    pub modified: u64,
    /// Increases each time this inode number is reused, so an old `NodeId` never names a newer
    /// file.
    pub generation: u32,
    /// How deep the extent tree is: 0 when `top` holds extents.
    pub depth: u16,
    /// The entries in the inode.
    pub top: Top,
}

impl Inode {
    /// A free inode with the given generation.
    pub fn free(generation: u32) -> Inode {
        Inode {
            kind: TYPE_FREE,
            flags: 0,
            parent: 0,
            size: 0,
            created: 0,
            modified: 0,
            generation,
            depth: 0,
            top: Top::Extents(Vec::new()),
        }
    }

    /// Reads an inode from its 256 bytes.
    ///
    /// With `verify`, an in-use inode must have a good checksum and a sensible entry count
    /// (`Damaged` otherwise). A free inode (type 0) is never verified, since the formatter
    /// leaves them all zero. Without `verify` only the entries are decoded, as far as they make
    /// sense, which is how the free-inode search and the check read inodes that may be damaged.
    pub fn parse(raw: &[u8], verify: bool) -> Result<Inode, FsError> {
        debug_assert_eq!(raw.len(), INODE_SIZE);
        let kind = u16_at(raw, 0);
        let count = usize::from(u16_at(raw, 36));
        let depth = u16_at(raw, 38);
        if verify && kind != TYPE_FREE {
            if pbr_crc::crc32(&raw[..252]) != u32_at(raw, 252) {
                return Err(FsError::Damaged);
            }
            if count > IN_INODE || depth > MAX_DEPTH {
                return Err(FsError::Damaged);
            }
        }
        let count = count.min(IN_INODE);
        let entries = &raw[64..64 + IN_INODE * 24];
        let top = if depth == 0 {
            Top::Extents(
                (0..count)
                    .map(|i| Extent {
                        file_block: u64_at(entries, i * 24),
                        disk_block: u64_at(entries, i * 24 + 8),
                        length: u32_at(entries, i * 24 + 16),
                    })
                    .collect(),
            )
        } else {
            Top::Index(
                (0..count)
                    .map(|i| IndexEntry {
                        file_block: u64_at(entries, i * 24),
                        child: u64_at(entries, i * 24 + 8),
                    })
                    .collect(),
            )
        };
        Ok(Inode {
            kind,
            flags: u16_at(raw, 2),
            parent: u32_at(raw, 4),
            size: u64_at(raw, 8),
            created: u64_at(raw, 16),
            modified: u64_at(raw, 24),
            generation: u32_at(raw, 32),
            depth,
            top,
        })
    }

    /// The 256 bytes to store, with a fresh checksum. Reserved bytes are written as zero.
    pub fn to_bytes(&self) -> [u8; INODE_SIZE] {
        let mut raw = [0u8; INODE_SIZE];
        put_u16(&mut raw, 0, self.kind);
        put_u16(&mut raw, 2, self.flags);
        put_u32(&mut raw, 4, self.parent);
        put_u64(&mut raw, 8, self.size);
        put_u64(&mut raw, 16, self.created);
        put_u64(&mut raw, 24, self.modified);
        put_u32(&mut raw, 32, self.generation);
        put_u16(&mut raw, 38, self.depth);
        match &self.top {
            Top::Extents(list) => {
                put_u16(&mut raw, 36, list.len().min(IN_INODE) as u16);
                for (i, e) in list.iter().take(IN_INODE).enumerate() {
                    let at = 64 + i * 24;
                    put_u64(&mut raw, at, e.file_block);
                    put_u64(&mut raw, at + 8, e.disk_block);
                    put_u32(&mut raw, at + 16, e.length);
                }
            }
            Top::Index(list) => {
                put_u16(&mut raw, 36, list.len().min(IN_INODE) as u16);
                for (i, e) in list.iter().take(IN_INODE).enumerate() {
                    let at = 64 + i * 24;
                    put_u64(&mut raw, at, e.file_block);
                    put_u64(&mut raw, at + 8, e.child);
                }
            }
        }
        let sum = pbr_crc::crc32(&raw[..252]);
        put_u32(&mut raw, 252, sum);
        raw
    }

    /// Whether this is a file or folder.
    pub fn in_use(&self) -> bool {
        self.kind != TYPE_FREE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample() -> Inode {
        Inode {
            kind: TYPE_FILE,
            flags: 1,
            parent: 7,
            size: 123_456,
            created: 1,
            modified: 2,
            generation: 9,
            depth: 0,
            top: Top::Extents(vec![
                Extent { file_block: 0, disk_block: 50, length: 3 },
                Extent { file_block: 10, disk_block: 90, length: 1 },
            ]),
        }
    }

    #[test]
    fn round_trip() {
        let i = sample();
        let raw = i.to_bytes();
        assert_eq!(Inode::parse(&raw, true).unwrap(), i);
        let tree = Inode {
            depth: 2,
            top: Top::Index(vec![IndexEntry { file_block: 0, child: 77 }]),
            ..sample()
        };
        assert_eq!(Inode::parse(&tree.to_bytes(), true).unwrap(), tree);
    }

    #[test]
    fn checksum_and_counts_are_enforced() {
        let mut raw = sample().to_bytes();
        raw[9] ^= 1;
        assert_eq!(Inode::parse(&raw, true), Err(FsError::Damaged));
        assert!(Inode::parse(&raw, false).is_ok());
        let mut raw = sample().to_bytes();
        put_u16(&mut raw, 36, 8);
        let sum = pbr_crc::crc32(&raw[..252]);
        put_u32(&mut raw, 252, sum);
        assert_eq!(Inode::parse(&raw, true), Err(FsError::Damaged));
        // Without verification the count is clamped instead of trusted.
        let Top::Extents(list) = Inode::parse(&raw, false).unwrap().top else { panic!() };
        assert_eq!(list.len(), IN_INODE);
    }

    #[test]
    fn free_inodes_are_not_verified() {
        let raw = [0u8; INODE_SIZE];
        let i = Inode::parse(&raw, true).unwrap();
        assert!(!i.in_use());
        assert_eq!(i.generation, 0);
    }
}
