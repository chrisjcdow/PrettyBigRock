// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Extents and extent trees (ROCKFS.md, "Where a file's contents live: extents").
//!
//! A file's contents are cut into 4 KiB **file blocks**, numbered from 0. An **extent** says
//! "file blocks 0-99 are disk blocks 5000-5099", so a file stored in a few pieces needs only a
//! few extents. The inode has room for 7; a file with more has an **extent tree**: the inode
//! holds *index entries* pointing at **extent blocks**, each holding up to 170 extents or
//! further index entries.
//!
//! This module holds the parts that are plain calculations on lists of extents (looking a block
//! up, cutting pieces out, joining neighbours, laying a list out as a tree). Reading a tree from
//! the disk, which needs the device, is in `tree.rs`.

use crate::inode::{IndexEntry, Top};
use crate::layout::*;
use crate::runs::Run;
use alloc::boxed::Box;
use alloc::vec::Vec;
use pbr_fs::FsError;

/// "File blocks `file_block` .. `file_block + length` are disk blocks `disk_block` ..".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// The first file block covered.
    pub file_block: u64,
    /// Where it is on the disk.
    pub disk_block: u64,
    /// How many consecutive blocks (1 or more).
    pub length: u32,
}

impl Extent {
    /// One past the last file block covered.
    pub fn file_end(&self) -> u64 {
        self.file_block.saturating_add(u64::from(self.length))
    }

    /// The disk blocks, as a run.
    pub fn run(&self) -> Run {
        Run { start: self.disk_block, len: u64::from(self.length) }
    }
}

/// The disk block holding file block `k`, or `None` if no extent covers it (a gap, which reads
/// as zeros). `extents` must be sorted and non-overlapping, which loading a tree guarantees.
///
/// Lookup is a binary search; "the last extent that starts at or before `k`" is exactly the
/// rule ROCKFS.md gives for walking the tree.
pub fn lookup(extents: &[Extent], k: u64) -> Option<u64> {
    let after = extents.partition_point(|e| e.file_block <= k);
    let e = extents.get(after.checked_sub(1)?)?;
    (k < e.file_end()).then(|| e.disk_block + (k - e.file_block))
}

/// Cuts the file blocks in `remove` out of `extents`.
///
/// `remove` is a sorted list of non-overlapping half-open ranges `(start, end)` of file blocks.
/// Returns the extents that remain (pieces of the old ones, still sorted) and the disk blocks
/// that were cut loose.
pub fn subtract(extents: &[Extent], remove: &[(u64, u64)]) -> (Vec<Extent>, Vec<Run>) {
    let mut kept = Vec::with_capacity(extents.len() + remove.len());
    let mut dropped = Vec::new();
    let mut first_range = 0;
    for e in extents {
        let end = e.file_end();
        let mut start = e.file_block;
        // Ranges that end before this extent starts matter to no later extent either, since
        // the extents are sorted.
        while first_range < remove.len() && remove[first_range].1 <= start {
            first_range += 1;
        }
        let mut r = first_range;
        let piece = |from: u64, to: u64| Extent {
            file_block: from,
            disk_block: e.disk_block + (from - e.file_block),
            length: (to - from) as u32,
        };
        while start < end {
            let Some(&(rs, re)) = remove.get(r).filter(|range| range.0 < end) else {
                kept.push(piece(start, end));
                break;
            };
            if rs > start {
                kept.push(piece(start, rs));
            }
            let (cut_from, cut_to) = (rs.max(start), re.min(end));
            dropped.push(Run {
                start: e.disk_block + (cut_from - e.file_block),
                len: cut_to - cut_from,
            });
            start = cut_to;
            r += 1;
        }
    }
    (kept, dropped)
}

/// Sorts extents by file block and joins those that continue each other, in both the file and
/// the disk, so a file written in one go is one extent.
pub fn merge(mut list: Vec<Extent>) -> Vec<Extent> {
    list.sort_by_key(|e| e.file_block);
    let mut out: Vec<Extent> = Vec::with_capacity(list.len());
    for e in list {
        if let Some(last) = out.last_mut() {
            let joined = u64::from(last.length) + u64::from(e.length);
            if last.file_end() == e.file_block
                && last.disk_block + u64::from(last.length) == e.disk_block
                && joined <= u64::from(u32::MAX)
            {
                last.length = joined as u32;
                continue;
            }
        }
        out.push(e);
    }
    out
}

/// How many extent blocks a file of `n` extents needs (none when they fit in the inode).
pub fn tree_blocks_needed(n: usize) -> u64 {
    if n <= IN_INODE {
        return 0;
    }
    let mut level = n.div_ceil(PER_XBLOCK) as u64;
    let mut total = level;
    while level > IN_INODE as u64 {
        level = level.div_ceil(PER_XBLOCK as u64);
        total += level;
    }
    total
}

/// An extent tree laid out for writing: what goes in the inode, and the extent blocks.
pub struct TreeImage {
    /// The entries for the inode.
    pub top: Top,
    /// The inode's `extent_depth`.
    pub depth: u16,
    /// The extent blocks to write, as (disk block, contents).
    pub blocks: Vec<(u64, Box<[u8; BLOCK]>)>,
}

/// The checksum of an extent block: CRC-32 of the whole block with the checksum field (bytes
/// 12 to 15) counted as zero.
pub fn xblock_checksum(block: &[u8]) -> u32 {
    pbr_crc::update(pbr_crc::update(pbr_crc::crc32(&block[..12]), &[0; 4]), &block[16..BLOCK])
}

/// Lays `extents` out as an inode's entries and extent blocks, taking disk blocks from `blocks`
/// (exactly [`tree_blocks_needed`] of them). Levels are built from the bottom up, 170 entries
/// to a block, until what is left fits in the inode.
pub fn build_tree(extents: &[Extent], blocks: &[u64]) -> Result<TreeImage, FsError> {
    if extents.len() <= IN_INODE {
        return Ok(TreeImage { top: Top::Extents(extents.to_vec()), depth: 0, blocks: Vec::new() });
    }
    if blocks.len() as u64 != tree_blocks_needed(extents.len()) {
        return Err(FsError::BadArgument);
    }
    let mut spare = blocks.iter().copied();
    let mut out = Vec::new();
    // The current level as (first file block, disk block of the child; the extents themselves
    // at the lowest level).
    let mut index: Vec<IndexEntry> = Vec::new();
    let mut depth: u16 = 0;
    let mut level_len = extents.len();
    loop {
        let mut next: Vec<IndexEntry> = Vec::new();
        for start in (0..level_len).step_by(PER_XBLOCK) {
            let count = (level_len - start).min(PER_XBLOCK);
            let disk = spare.next().ok_or(FsError::BadArgument)?;
            let mut page = Box::new([0u8; BLOCK]);
            page[0..4].copy_from_slice(b"RXTB");
            put_u16(&mut page[..], 4, count as u16);
            put_u16(&mut page[..], 6, depth);
            for i in 0..count {
                let at = 16 + i * 24;
                if depth == 0 {
                    let e = &extents[start + i];
                    put_u64(&mut page[..], at, e.file_block);
                    put_u64(&mut page[..], at + 8, e.disk_block);
                    put_u32(&mut page[..], at + 16, e.length);
                } else {
                    let e = &index[start + i];
                    put_u64(&mut page[..], at, e.file_block);
                    put_u64(&mut page[..], at + 8, e.child);
                }
            }
            let sum = xblock_checksum(&page[..]);
            put_u32(&mut page[..], 12, sum);
            let first =
                if depth == 0 { extents[start].file_block } else { index[start].file_block };
            next.push(IndexEntry { file_block: first, child: disk });
            out.push((disk, page));
        }
        depth += 1;
        index = next;
        level_len = index.len();
        if level_len <= IN_INODE {
            return Ok(TreeImage { top: Top::Index(index), depth, blocks: out });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    fn ext(f: u64, d: u64, l: u32) -> Extent {
        Extent { file_block: f, disk_block: d, length: l }
    }

    /// Every file block an extent list maps, as a map from file block to disk block.
    fn blocks_of(list: &[Extent]) -> BTreeMap<u64, u64> {
        let mut m = BTreeMap::new();
        for e in list {
            for i in 0..u64::from(e.length) {
                assert!(m.insert(e.file_block + i, e.disk_block + i).is_none(), "overlap");
            }
        }
        m
    }

    #[test]
    fn lookup_finds_blocks_and_gaps() {
        let list = [ext(2, 100, 3), ext(10, 200, 1)];
        assert_eq!(lookup(&list, 0), None);
        assert_eq!(lookup(&list, 2), Some(100));
        assert_eq!(lookup(&list, 4), Some(102));
        assert_eq!(lookup(&list, 5), None);
        assert_eq!(lookup(&list, 10), Some(200));
        assert_eq!(lookup(&list, 11), None);
        assert_eq!(lookup(&[], 0), None);
    }

    #[test]
    fn subtract_matches_a_block_model() {
        let mut seed = 99u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for _ in 0..500 {
            // A random sorted extent list.
            let mut list = Vec::new();
            let mut f = rnd(4);
            let mut d = 1000;
            for _ in 0..rnd(8) {
                let l = 1 + rnd(6) as u32;
                list.push(ext(f, d, l));
                f += u64::from(l) + rnd(3);
                d += u64::from(l) + rnd(5) + 1;
            }
            // A random sorted list of ranges.
            let mut remove = Vec::new();
            let mut at = rnd(6);
            for _ in 0..rnd(5) {
                let len = 1 + rnd(5);
                remove.push((at, at + len));
                at += len + rnd(4) + 1;
            }
            let (kept, dropped) = subtract(&list, &remove);
            let before = blocks_of(&list);
            let after = blocks_of(&kept);
            let cut: Vec<u64> = before
                .iter()
                .filter(|(f, _)| remove.iter().any(|r| (r.0..r.1).contains(f)))
                .map(|(_, d)| *d)
                .collect();
            let want: BTreeMap<u64, u64> = before
                .iter()
                .filter(|(f, _)| !remove.iter().any(|r| (r.0..r.1).contains(f)))
                .map(|(f, d)| (*f, *d))
                .collect();
            assert_eq!(after, want);
            let mut got: Vec<u64> = dropped.iter().flat_map(|r| r.start..r.end()).collect();
            got.sort();
            let mut cut = cut;
            cut.sort();
            assert_eq!(got, cut);
            assert!(kept.windows(2).all(|w| w[0].file_end() <= w[1].file_block));
        }
    }

    #[test]
    fn merge_joins_only_true_neighbours() {
        let m = merge(vec![ext(5, 55, 1), ext(0, 10, 2), ext(2, 12, 3), ext(6, 99, 1)]);
        assert_eq!(m, vec![ext(0, 10, 5), ext(5, 55, 1), ext(6, 99, 1)]);
        // A join that would overflow the 32-bit length is not made.
        let big =
            merge(vec![ext(0, 0, u32::MAX), ext(u64::from(u32::MAX), u64::from(u32::MAX), 1)]);
        assert_eq!(big.len(), 2);
    }

    #[test]
    fn tree_sizes() {
        assert_eq!(tree_blocks_needed(0), 0);
        assert_eq!(tree_blocks_needed(7), 0);
        assert_eq!(tree_blocks_needed(8), 1);
        assert_eq!(tree_blocks_needed(7 * 170), 7);
        assert_eq!(tree_blocks_needed(7 * 170 + 1), 8 + 1);
        // The same numbers tools/rockfs.py computes.
        assert_eq!(tree_blocks_needed(200_000), 1177 + 7);
    }

    #[test]
    fn build_tree_shapes() {
        let list: Vec<Extent> = (0..8).map(|i| ext(i * 2, 1000 + i * 3, 1)).collect();
        let t = build_tree(&list, &[50]).unwrap();
        assert_eq!(t.depth, 1);
        assert_eq!(t.blocks.len(), 1);
        let Top::Index(top) = &t.top else { panic!() };
        assert_eq!(top, &vec![IndexEntry { file_block: 0, child: 50 }]);
        let (disk, page) = &t.blocks[0];
        assert_eq!(*disk, 50);
        assert_eq!(&page[0..4], b"RXTB");
        assert_eq!(u16_at(&page[..], 4), 8);
        assert_eq!(u32_at(&page[..], 12), xblock_checksum(&page[..]));
        // A wrong number of blocks is refused rather than trusted.
        assert!(build_tree(&list, &[]).is_err());
        // Small lists need no blocks at all.
        let t = build_tree(&list[..7], &[]).unwrap();
        assert_eq!(t.depth, 0);
        // Two levels.
        let many: Vec<Extent> = (0..1200).map(|i| ext(i * 2, 5000 + i * 3, 1)).collect();
        let need = tree_blocks_needed(many.len());
        let blocks: Vec<u64> = (0..need).map(|i| 100 + i).collect();
        let t = build_tree(&many, &blocks).unwrap();
        assert_eq!(t.depth, 2);
        assert_eq!(t.blocks.len() as u64, need);
    }
}
