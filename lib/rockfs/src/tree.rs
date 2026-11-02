// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading a file's extent tree from the disk, with every rule checked.
//!
//! Anything on the disk may be wrong: a bit flipped by failing hardware, or a deliberately
//! crafted image. Reading a tree must therefore never loop, never index outside a buffer, never
//! read outside the volume and never allocate more than the disk really holds. The rules,
//! each of which makes a reader safe against one kind of lie (note 0016, section 2):
//!
//! - each extent block has the right magic and checksum, and 1 to 170 entries;
//! - the depth goes down by exactly one at every level, and the top is at most
//!   [`MAX_DEPTH`], so a tree cannot be deeper than that and cannot contain a loop;
//! - extents are sorted and do not overlap, and every one lies inside the volume's data area;
//! - each index entry's range of file blocks contains exactly what its child holds, so looking
//!   a block up in the flattened list gives the same answer as walking the tree;
//! - no extent block is used twice, and no extent lies beyond the file's last block.

use crate::extents::{Extent, xblock_checksum};
use crate::inode::{IndexEntry, Inode, Top};
use crate::layout::*;
use crate::volume::RockFs;
use alloc::collections::BTreeSet;
use alloc::rc::Rc;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// A file's extents, flattened into one sorted list, and the disk blocks of its extent tree.
///
/// (`Rc` is Rust's reference-counted pointer: cloning it copies a pointer, not the list, and
/// the list is freed when the last copy goes. It lets the cache, a pending change and a reader
/// share one list.)
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// Every extent of the file, sorted by file block.
    pub extents: Rc<Vec<Extent>>,
    /// The disk blocks holding the tree, in the order they were read.
    pub tree: Rc<Vec<u64>>,
}

/// State carried down the walk of a tree.
struct Walk {
    extents: Vec<Extent>,
    tree: Vec<u64>,
    seen: BTreeSet<u64>,
    /// One past the last file block the extents so far cover.
    prev_end: u64,
}

impl<D: BlockDevice> RockFs<D> {
    /// Reads the whole extent tree of `inode`.
    ///
    /// Returns `Damaged` if any rule above is broken. Reads at most one block per tree block, and
    /// each tree block can only be read once, so the work is bounded by the size of the volume.
    pub(crate) fn load_extents(&mut self, inode: &Inode) -> Result<Loaded, FsError> {
        let mut walk =
            Walk { extents: Vec::new(), tree: Vec::new(), seen: BTreeSet::new(), prev_end: 0 };
        match &inode.top {
            Top::Extents(list) => {
                if inode.depth != 0 {
                    return Err(FsError::Damaged);
                }
                self.take_extents(list, 0, u64::MAX, &mut walk)?;
            }
            Top::Index(list) => {
                if inode.depth == 0 || inode.depth > MAX_DEPTH {
                    return Err(FsError::Damaged);
                }
                self.walk_index(list, inode.depth, 0, u64::MAX, &mut walk)?;
            }
        }
        // No extent may reach past the file's last block: otherwise growing the file later
        // would show whatever those blocks hold instead of zeros.
        if let Some(last) = walk.extents.last()
            && last.file_end() > inode.size.div_ceil(BLOCK_U64)
        {
            return Err(FsError::Damaged);
        }
        Ok(Loaded { extents: Rc::new(walk.extents), tree: Rc::new(walk.tree) })
    }

    /// Checks `list` (extents from the inode or a leaf extent block) and appends it.
    /// `lower..upper` is the range of file blocks the parent's index entries allow.
    fn take_extents(
        &self,
        list: &[Extent],
        lower: u64,
        upper: u64,
        walk: &mut Walk,
    ) -> Result<(), FsError> {
        // The last block of the data area is the backup superblock; the first data block is
        // after the bitmap. Everything a file uses lies between.
        let (first, limit) = (self.sb.data_start, self.sb.block_count - 1);
        for e in list {
            let file_end = e.file_block.checked_add(u64::from(e.length)).ok_or(FsError::Damaged)?;
            let disk_end = e.disk_block.checked_add(u64::from(e.length)).ok_or(FsError::Damaged)?;
            if e.length == 0
                || e.file_block < lower.max(walk.prev_end)
                || file_end > upper
                || e.disk_block < first
                || disk_end > limit
            {
                return Err(FsError::Damaged);
            }
            walk.extents.push(*e);
            walk.prev_end = file_end;
        }
        Ok(())
    }

    /// Follows `list` (index entries at `depth`, 1 or more) down into extent blocks.
    fn walk_index(
        &mut self,
        list: &[IndexEntry],
        depth: u16,
        lower: u64,
        upper: u64,
        walk: &mut Walk,
    ) -> Result<(), FsError> {
        if list.is_empty() {
            return Err(FsError::Damaged);
        }
        for (i, entry) in list.iter().enumerate() {
            // Entry `i` covers file blocks from its own start to the next entry's start.
            let from = entry.file_block;
            let to = list.get(i + 1).map_or(upper, |next| next.file_block);
            let child = entry.child;
            if from < lower
                || from >= to
                || child < self.sb.data_start
                || child >= self.sb.block_count - 1
            {
                return Err(FsError::Damaged);
            }
            if !walk.seen.insert(child) {
                return Err(FsError::Damaged);
            }
            walk.tree.push(child);
            let page = self.read_block(child)?;
            if page[0..4] != *b"RXTB"
                || u32_at(&page[..], 12) != xblock_checksum(&page[..])
                || u16_at(&page[..], 6).checked_add(1) != Some(depth)
            {
                return Err(FsError::Damaged);
            }
            let count = usize::from(u16_at(&page[..], 4));
            if count == 0 || count > PER_XBLOCK {
                return Err(FsError::Damaged);
            }
            if depth == 1 {
                let extents: Vec<Extent> = (0..count)
                    .map(|j| Extent {
                        file_block: u64_at(&page[..], 16 + j * 24),
                        disk_block: u64_at(&page[..], 24 + j * 24),
                        length: u32_at(&page[..], 32 + j * 24),
                    })
                    .collect();
                self.take_extents(&extents, from, to, walk)?;
            } else {
                let entries: Vec<IndexEntry> = (0..count)
                    .map(|j| IndexEntry {
                        file_block: u64_at(&page[..], 16 + j * 24),
                        child: u64_at(&page[..], 24 + j * 24),
                    })
                    .collect();
                self.walk_index(&entries, depth - 1, from, to, walk)?;
            }
        }
        Ok(())
    }
}
