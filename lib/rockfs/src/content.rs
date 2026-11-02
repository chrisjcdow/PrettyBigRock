// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Changing what a file or folder contains, safely (ROCKFS.md, "Saving safely"; note 0016,
//! section 2).
//!
//! RockFS never overwrites a block that a file is using. A change is **copy-on-write**: the
//! changed parts are written to *new* blocks, and the old blocks stay exactly as they were
//! until nothing on the disk points at them. The change only takes effect when the file's
//! inode is rewritten to point at the new blocks, and that is a single sector write, which a
//! power cut either completes or does not start. So after a crash every file is wholly old or
//! wholly new, never a mixture.
//!
//! The difficulty is that the disk (and the block cache in front of it) is free to carry out
//! writes in any order between two flushes. So the order matters, and this module puts a flush
//! exactly where one write must be safely on the disk before another:
//!
//! 1. the new data blocks are written when `write` is called, to blocks nothing points at;
//! 2. at commit, the new extent-tree blocks are written, then **flush**: everything the new
//!    version points at is now on the disk;
//! 3. the inode is written. This is the moment the change takes effect;
//! 4. **flush**: the change is now on the disk;
//! 5. only now are the replaced blocks freed (in the in-memory bitmap), because nothing on the
//!    disk points at them any more and they may be reused.
//!
//! A file's changes that are written but not yet committed are *pending*: the file's new
//! extent list and size are kept in memory (and seen by `read` and `stat`), and the inode on the
//! disk still describes the old version.

use crate::extents::{Extent, build_tree, lookup, merge, subtract, tree_blocks_needed};
use crate::inode::Inode;
use crate::layout::*;
use crate::runs::{Run, RunSet};
use crate::tree::Loaded;
use crate::volume::RockFs;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// A file's changes that are not yet committed.
pub(crate) struct Pending {
    /// The generation of the file's inode, to be sure it is still the same file.
    pub generation: u32,
    /// The file's extents as they will be.
    pub extents: Rc<Vec<Extent>>,
    /// The disk blocks of the extent tree the inode on the disk uses; freed at commit.
    pub old_tree: Rc<Vec<u64>>,
    /// The file's size as it will be.
    pub size: u64,
    /// The modification time as it will be.
    pub modified: u64,
    /// Blocks written by these changes and still part of the file. Nothing on the disk points
    /// at them, so if they are overwritten again they are freed at once.
    pub fresh: RunSet,
    /// Blocks of the committed version that these changes dropped. Freed at commit, after the
    /// inode is on the disk.
    pub stale: RunSet,
    /// Whether anything has changed (a `Pending` is created before the first change is known to
    /// work).
    pub dirty: bool,
}

/// The result of changing a range of a file in copy-on-write fashion.
pub(crate) struct Cow {
    /// The new extent list.
    pub extents: Vec<Extent>,
    /// The new blocks written. Already marked used in the bitmap.
    pub new_runs: Vec<Run>,
    /// The old blocks the new list no longer uses.
    pub dropped: Vec<Run>,
}

/// What an update writes the result to.
#[derive(Clone, Copy)]
pub(crate) enum Target {
    /// An inode in the inode table.
    Inode(u32),
    /// The inode table itself, which is described by the superblock; with the new number of
    /// inode slots when the table is growing.
    Table { inode_count: u32 },
}

/// Everything needed to make a new version of a file or folder take effect.
pub(crate) struct Update<'a> {
    pub target: Target,
    pub size: u64,
    pub modified: u64,
    pub extents: &'a [Extent],
    /// The extent-tree blocks of the old version, freed once the new one is on the disk.
    pub old_tree: &'a [u64],
    /// Data blocks of the old version no longer used, freed likewise.
    pub release: &'a [Run],
    /// Whether the extent tree may use the reserve (only a deletion's folder change does).
    pub privileged: bool,
}

impl<D: BlockDevice> RockFs<D> {
    /// Free blocks an allocation may use: all of them for a privileged one, otherwise all but
    /// the reserve (ROCKFS.md, "Running out of space").
    pub(crate) fn available(&self, privileged: bool) -> u64 {
        let reserve = if privileged { 0 } else { self.sb.reserve_blocks };
        self.bitmap.free().saturating_sub(reserve)
    }

    /// Allocates `count` blocks, as few runs as possible. `NoSpace`, changing nothing, if there
    /// are not that many available.
    pub(crate) fn allocate(&mut self, count: u64, privileged: bool) -> Result<Vec<Run>, FsError> {
        if count > self.available(privileged) {
            return Err(FsError::NoSpace);
        }
        let (lo, hi) = (self.sb.data_start, self.sb.block_count - 1);
        self.bitmap.allocate(count, lo, hi).ok_or(FsError::NoSpace)
    }

    /// The extents (and tree blocks) of file or folder `n`, from the cache or the disk.
    pub(crate) fn extents_loaded(&mut self, n: u32, inode: &Inode) -> Result<Loaded, FsError> {
        if let Some((cn, gen_, loaded)) = &self.cache
            && *cn == n
            && *gen_ == inode.generation
        {
            return Ok(loaded.clone());
        }
        let loaded = self.load_extents(inode)?;
        self.cache = Some((n, inode.generation, loaded.clone()));
        Ok(loaded)
    }

    /// Starts tracking changes to file `n`, whose committed inode is `inode`.
    pub(crate) fn new_pending(&mut self, n: u32, inode: &Inode) -> Result<Pending, FsError> {
        let loaded = self.extents_loaded(n, inode)?;
        Ok(Pending {
            generation: inode.generation,
            extents: loaded.extents,
            old_tree: loaded.tree,
            size: inode.size,
            modified: inode.modified,
            fresh: RunSet::default(),
            stale: RunSet::default(),
            dirty: false,
        })
    }

    /// Changes a file in copy-on-write fashion: writes `patch` (an offset and bytes) and makes
    /// the file `new_size` bytes long, writing every changed block to a *new* place.
    ///
    /// `extents` and `old_size` describe the file now. The data blocks are written; nothing
    /// else is. The caller makes the result take effect (see [`RockFs::commit_update`]), or
    /// returns the new blocks with [`RockFs::release`] if it can't.
    ///
    /// Besides the blocks the patch touches, one more block may be rewritten: if the file
    /// grows or shrinks inside its last block, the bytes after the end of the data in that
    /// block must be zeros, because a later growth shows them as part of the file. Our own
    /// writers always leave them zero; a disk written by anything else might not, so this
    /// makes it so rather than assuming.
    pub(crate) fn cow_apply(
        &mut self,
        extents: &[Extent],
        old_size: u64,
        new_size: u64,
        patch: Option<(u64, &[u8])>,
        privileged: bool,
    ) -> Result<Cow, FsError> {
        let end_block = new_size.div_ceil(BLOCK_U64);
        let (off, data) = patch.unwrap_or((0, &[]));
        let end = off.checked_add(data.len() as u64).ok_or(FsError::TooBig)?;
        // Which file blocks get new contents.
        let mut rewrite: Vec<u64> = Vec::new();
        if !data.is_empty() {
            rewrite.extend(off / BLOCK_U64..=(end - 1) / BLOCK_U64);
        }
        // Bytes at and after `cut` are zero in the result (unless the patch covers them).
        let cut = old_size.min(new_size);
        let mut preloaded: Option<(u64, Box<[u8; BLOCK]>)> = None;
        if old_size != new_size
            && !cut.is_multiple_of(BLOCK_U64)
            && !rewrite.contains(&(cut / BLOCK_U64))
        {
            let k = cut / BLOCK_U64;
            if let Some(disk) = lookup(extents, k) {
                let block = self.read_block(disk)?;
                if block[(cut % BLOCK_U64) as usize..].iter().any(|&b| b != 0) {
                    rewrite.push(k);
                    preloaded = Some((k, block));
                }
            }
        }
        rewrite.sort_unstable();
        rewrite.dedup();

        // The old contents of each block to rewrite, where the patch doesn't cover all of it.
        let mut old: Vec<Option<Box<[u8; BLOCK]>>> = Vec::with_capacity(rewrite.len());
        for &k in &rewrite {
            if preloaded.as_ref().is_some_and(|(tk, _)| *tk == k) {
                old.push(preloaded.take().map(|(_, block)| block));
                continue;
            }
            let lo = k * BLOCK_U64;
            let covered = off <= lo && end >= lo + BLOCK_U64;
            old.push(if covered || lo >= cut {
                None
            } else {
                lookup(extents, k).map(|disk| self.read_block(disk)).transpose()?
            });
        }

        // The ranges of file blocks to cut out of the old list: the rewritten blocks (joined
        // where they touch) and everything past the new end.
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for &k in &rewrite {
            match ranges.last_mut() {
                Some(r) if r.1 == k => r.1 = k + 1,
                _ => ranges.push((k, k + 1)),
            }
        }
        ranges.push((end_block, u64::MAX));
        let (mut kept, dropped) = subtract(extents, &ranges);
        if rewrite.is_empty() {
            return Ok(Cow { extents: kept, new_runs: Vec::new(), dropped });
        }

        let runs = self.allocate(rewrite.len() as u64, privileged)?;
        let mut disk_blocks = runs.iter().flat_map(|r| r.start..r.end());
        for &k in &rewrite {
            let disk = disk_blocks.next().ok_or(FsError::NoSpace)?;
            kept.push(Extent { file_block: k, disk_block: disk, length: 1 });
        }
        let new_list = merge(kept);
        // The extent tree for the new list must fit too (ROCKFS.md: a writer must be able to
        // finish what it starts). It is checked now, before anything is written, so a write
        // that cannot be saved fails cleanly.
        if tree_blocks_needed(new_list.len()) > self.available(privileged) {
            self.release(&runs);
            return Err(FsError::NoSpace);
        }

        // Build and write the new blocks, one run at a time.
        let mut index = 0;
        for run in &runs {
            let mut bytes = vec![0u8; run.len as usize * BLOCK];
            for chunk in bytes.chunks_mut(BLOCK) {
                let k = rewrite[index];
                let block_start = k * BLOCK_U64;
                if let Some(content) = &old[index] {
                    chunk.copy_from_slice(&content[..]);
                }
                // Zero everything at and after `cut`.
                let zero_from = cut.saturating_sub(block_start).min(BLOCK_U64) as usize;
                chunk[zero_from..].fill(0);
                // Lay the patch over it.
                let (from, to) = (off.max(block_start), end.min(block_start + BLOCK_U64));
                if from < to {
                    let src = &data[(from - off) as usize..(to - off) as usize];
                    chunk[(from - block_start) as usize..(to - block_start) as usize]
                        .copy_from_slice(src);
                }
                index += 1;
            }
            if let Err(e) = self.write_blocks(run.start, &bytes) {
                self.release(&runs);
                return Err(e);
            }
        }
        Ok(Cow { extents: new_list, new_runs: runs, dropped })
    }

    /// Writes `patch` into file `n` and sets its size, as a pending change (not yet durable).
    pub(crate) fn modify_file(
        &mut self,
        n: u32,
        inode: &Inode,
        patch: Option<(u64, &[u8])>,
        new_size: u64,
    ) -> Result<(), FsError> {
        let mut p = match self.pending.remove(&n) {
            Some(p) => p,
            None => self.new_pending(n, inode)?,
        };
        let cow = match self.cow_apply(&p.extents, p.size, new_size, patch, false) {
            Ok(c) => c,
            Err(e) => {
                if p.dirty {
                    self.pending.insert(n, p);
                }
                return Err(e);
            }
        };
        // Blocks the change dropped: ones an earlier uncommitted change wrote can be freed
        // right now; ones from the committed version must wait for the commit.
        for run in cow.dropped {
            let (fresh, committed) = p.fresh.take(run);
            self.release(&fresh);
            for r in committed {
                p.stale.insert(r);
            }
        }
        for r in cow.new_runs {
            p.fresh.insert(r);
        }
        p.extents = Rc::new(cow.extents);
        p.size = new_size;
        p.modified = self.now();
        p.dirty = true;
        self.pending.insert(n, p);
        Ok(())
    }

    /// Makes file `n`'s pending changes durable (note 0014, section 6). Nothing if it has none.
    /// If this fails the changes stay pending and the file on the disk is as it was.
    pub(crate) fn commit_pending(&mut self, n: u32) -> Result<(), FsError> {
        let Some(p) = self.pending.remove(&n) else { return Ok(()) };
        if !p.dirty {
            return Ok(());
        }
        let update = Update {
            target: Target::Inode(n),
            size: p.size,
            modified: p.modified,
            extents: &p.extents,
            old_tree: &p.old_tree,
            release: p.stale.runs(),
            privileged: false,
        };
        match self.commit_update(&update) {
            // The fresh blocks are part of the committed file now, so there is nothing to free.
            Ok(_) => Ok(()),
            Err(e) => {
                self.pending.insert(n, p);
                Err(e)
            }
        }
    }

    /// Makes a new version of a file, folder or the inode table take effect, in the order the
    /// module comment gives. On success returns the blocks of the new extent tree.
    ///
    /// On `NoSpace` for the tree nothing has been written. A failure to write the disk marks
    /// the volume poisoned (nothing more may be changed).
    pub(crate) fn commit_update(&mut self, u: &Update<'_>) -> Result<Rc<Vec<u64>>, FsError> {
        self.writable()?;
        // Read the inode as it is now, so that changes made to it meanwhile (a rename changes
        // `parent`) are kept and only the contents fields are replaced.
        let mut inode = match u.target {
            Target::Inode(n) => self.read_inode(n, true)?,
            Target::Table { .. } => self.sb.table.clone(),
        };
        let need = tree_blocks_needed(u.extents.len());
        let runs = if need > 0 { self.allocate(need, u.privileged)? } else { Vec::new() };
        let blocks: Vec<u64> = runs.iter().flat_map(|r| r.start..r.end()).collect();
        let image = match build_tree(u.extents, &blocks) {
            Ok(image) => image,
            Err(e) => {
                self.release(&runs);
                return Err(e);
            }
        };
        // Step 2: the extent-tree blocks, then a flush. This flush also makes the data blocks
        // written earlier durable: everything the new inode will point at is on the disk
        // before the inode is.
        for (disk, page) in &image.blocks {
            self.write_blocks(*disk, &page[..])?;
        }
        self.dev_flush()?;
        // Step 3: the inode. This is the moment the change takes effect.
        inode.size = u.size;
        inode.modified = u.modified;
        inode.top = image.top;
        inode.depth = image.depth;
        match u.target {
            Target::Inode(n) => self.write_inode(n, &inode)?,
            Target::Table { inode_count } => {
                let mut sb = self.sb.clone();
                sb.free_inodes =
                    sb.free_inodes.saturating_add(inode_count.saturating_sub(sb.inode_count));
                sb.inode_count = inode_count;
                sb.table = inode;
                // The inode table is described by the superblock, so changing it is a change
                // to the volume's structure: backup, flush, main (which the next flush covers).
                let bytes = sb.to_bytes();
                self.dev_write((sb.block_count - 1) * SECTORS_PER_BLOCK, &bytes)?;
                self.dev_flush()?;
                self.dev_write(0, &bytes)?;
                self.sb = sb;
            }
        }
        // Step 4: the change is on the disk.
        self.dev_flush()?;
        // Step 5: nothing on the disk points at the replaced blocks any more.
        self.release(u.release);
        self.release_blocks(u.old_tree);
        self.cache = None;
        Ok(Rc::new(blocks))
    }
}
