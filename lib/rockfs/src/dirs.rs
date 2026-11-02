// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Folder operations on the disk: finding a name, adding and removing one.
//!
//! A folder is stored as a file whose bytes are folder blocks (see `folder.rs`), so changing one
//! block of a folder is a copy-on-write change of that file, and it is made durable before the
//! operation returns (note 0014, section 6). Folders never hold pending changes.

use crate::content::{Target, Update};
use crate::extents::{Extent, lookup};
use crate::folder::{self, Entries, Entry};
use crate::inode::Inode;
use crate::layout::*;
use crate::names::same_name;
use crate::tree::Loaded;
use crate::volume::RockFs;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// Checks that a folder's extents are a plain run of whole blocks from block 0, with no gaps,
/// as long as its size says.
pub(crate) fn folder_shape_ok(loaded: &Loaded, size: u64) -> Result<(), FsError> {
    if !size.is_multiple_of(BLOCK_U64) {
        return Err(FsError::Damaged);
    }
    let mut next = 0;
    for e in loaded.extents.iter() {
        if e.file_block != next {
            return Err(FsError::Damaged);
        }
        next = e.file_end();
    }
    if next != size / BLOCK_U64 {
        return Err(FsError::Damaged);
    }
    Ok(())
}

/// A name found in a folder.
pub(crate) struct Found {
    /// Its entry in that block.
    pub entry: Entry,
    /// The name exactly as stored.
    pub name: Vec<u8>,
}

impl<D: BlockDevice> RockFs<D> {
    /// The extents of folder `n`, checked with [`folder_shape_ok`]. (A file may have holes; a
    /// folder never does, so a folder with a huge size and few blocks is damaged, not something
    /// to loop over.)
    pub(crate) fn folder_view(&mut self, n: u32, inode: &Inode) -> Result<Loaded, FsError> {
        let loaded = self.extents_loaded(n, inode)?;
        folder_shape_ok(&loaded, inode.size)?;
        Ok(loaded)
    }

    /// Reads block `k` of a folder.
    pub(crate) fn read_folder_block(
        &mut self,
        view: &Loaded,
        k: u64,
    ) -> Result<Box<[u8; BLOCK]>, FsError> {
        let disk = lookup(&view.extents, k).ok_or(FsError::Damaged)?;
        self.read_block(disk)
    }

    /// Finds the entry in folder `n` that counts as the same name as `name`.
    pub(crate) fn dir_find(
        &mut self,
        n: u32,
        inode: &Inode,
        name: &[u8],
    ) -> Result<Option<Found>, FsError> {
        let view = self.folder_view(n, inode)?;
        for k in 0..inode.size / BLOCK_U64 {
            let block = self.read_folder_block(&view, k)?;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode != 0 && same_name(entry.name(&block[..]), name) {
                    let stored = entry.name(&block[..]).to_vec();
                    return Ok(Some(Found { entry, name: stored }));
                }
            }
        }
        Ok(None)
    }

    /// Whether folder `n` has no names in it.
    pub(crate) fn dir_is_empty(&mut self, n: u32, inode: &Inode) -> Result<bool, FsError> {
        let view = self.folder_view(n, inode)?;
        for k in 0..inode.size / BLOCK_U64 {
            let block = self.read_folder_block(&view, k)?;
            for entry in Entries::new(&block[..]) {
                if entry?.inode != 0 {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Writes block `k` of folder `n` (`k` equal to the folder's block count adds a block) and
    /// makes the result take effect before returning.
    fn dir_rewrite(
        &mut self,
        n: u32,
        inode: &Inode,
        view: &Loaded,
        k: u64,
        block: &[u8; BLOCK],
        privileged: bool,
    ) -> Result<(), FsError> {
        let mut changes = BTreeMap::new();
        changes.insert(k, Box::new(*block));
        self.dir_commit(n, inode, view, changes, privileged)
    }

    /// Writes new versions of several blocks of folder `n` (keyed by block number; the folder's
    /// block count adds a block) and makes them all take effect together, with one write of the
    /// folder's inode. A change that touches two places in a folder is atomic this way: after
    /// a crash the folder is wholly old or wholly new.
    fn dir_commit(
        &mut self,
        n: u32,
        inode: &Inode,
        view: &Loaded,
        changes: BTreeMap<u64, Box<[u8; BLOCK]>>,
        privileged: bool,
    ) -> Result<(), FsError> {
        let mut extents: Vec<Extent> = view.extents.to_vec();
        let mut size = inode.size;
        // `new_runs` are the blocks written here (returned if the change can't be made);
        // `dropped` the folder's old blocks the change replaces (freed once it is on the disk).
        // Each block number appears once, so a later step never drops an earlier step's block.
        let mut new_runs = Vec::new();
        let mut dropped = Vec::new();
        for (k, block) in &changes {
            let at = k * BLOCK_U64;
            if at > size {
                self.release(&new_runs);
                return Err(FsError::BadArgument);
            }
            let new_size = if at == size { size + BLOCK_U64 } else { size };
            let cow = match self.cow_apply(
                &extents,
                size,
                new_size,
                Some((at, &block[..])),
                privileged,
            ) {
                Ok(cow) => cow,
                Err(e) => {
                    self.release(&new_runs);
                    return Err(e);
                }
            };
            new_runs.extend(cow.new_runs);
            dropped.extend(cow.dropped);
            extents = cow.extents;
            size = new_size;
        }
        let update = Update {
            target: Target::Inode(n),
            size,
            modified: self.now(),
            extents: &extents,
            old_tree: &view.tree,
            release: &dropped,
            privileged,
        };
        match self.commit_update(&update) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.release(&new_runs);
                Err(e)
            }
        }
    }

    /// Block `k` of a folder, as changed so far in `edits` or as on the disk.
    fn block_for_edit(
        &mut self,
        view: &Loaded,
        edits: &BTreeMap<u64, Box<[u8; BLOCK]>>,
        k: u64,
    ) -> Result<Box<[u8; BLOCK]>, FsError> {
        match edits.get(&k) {
            Some(b) => Ok(b.clone()),
            None => self.read_folder_block(view, k),
        }
    }

    /// Where the entry stored under exactly `name` is (for `inode`, if given), in a folder of
    /// `blocks` blocks being changed in `edits`: (block, position).
    fn find_for_edit(
        &mut self,
        view: &Loaded,
        blocks: u64,
        edits: &BTreeMap<u64, Box<[u8; BLOCK]>>,
        name: &[u8],
        inode: Option<u32>,
    ) -> Result<Option<(u64, usize)>, FsError> {
        for k in 0..blocks {
            let block = self.block_for_edit(view, edits, k)?;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode != 0
                    && inode.is_none_or(|i| i == entry.inode)
                    && entry.name(&block[..]) == name
                {
                    return Ok(Some((k, entry.pos)));
                }
            }
        }
        Ok(None)
    }

    /// Renames inode `moving` (type `kind`), stored in folder `n` under exactly `from_name`, to
    /// `to_name` in the same folder, as **one** change of the folder, so a crash leaves it under
    /// one name or the other, never both or neither. With `over` (the stored name of another
    /// entry in the folder, a file the caller has checked may be replaced), that entry names
    /// `moving` instead, in the same change.
    pub(crate) fn dir_rename_within(
        &mut self,
        n: u32,
        from_name: &[u8],
        moving: u32,
        to_name: &[u8],
        kind: u8,
        over: Option<&[u8]>,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(n, true)?;
        let view = self.folder_view(n, &inode)?;
        let blocks = inode.size / BLOCK_U64;
        let mut edits: BTreeMap<u64, Box<[u8; BLOCK]>> = BTreeMap::new();
        // The old name goes.
        let (k, pos) = self
            .find_for_edit(&view, blocks, &edits, from_name, Some(moving))?
            .ok_or(FsError::NotFound)?;
        let mut block = self.block_for_edit(&view, &edits, k)?;
        folder::remove_at(&mut block[..], pos)?;
        edits.insert(k, block);
        match over {
            // The replaced file's entry names the moving one, under the new spelling if it
            // fits there (it nearly always does: it counts as the same name).
            Some(old) => {
                let (k, pos) = self
                    .find_for_edit(&view, blocks, &edits, old, None)?
                    .ok_or(FsError::NotFound)?;
                let mut block = self.block_for_edit(&view, &edits, k)?;
                if !folder::replace_at(&mut block[..], pos, to_name, moving, kind)? {
                    folder::repoint(&mut block[..], pos, moving, kind);
                }
                edits.insert(k, block);
            }
            // The new name goes wherever there is room, or in a new block.
            None => {
                let mut placed = false;
                for k in 0..blocks {
                    let mut block = self.block_for_edit(&view, &edits, k)?;
                    if folder::try_insert(&mut block[..], to_name, moving, kind)? {
                        edits.insert(k, block);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    edits.insert(blocks, folder::new_block(to_name, moving, kind));
                }
            }
        }
        self.dir_commit(n, &inode, &view, edits, false)
    }

    /// Whether folder `n` (whose extents are `view`, `size` bytes long) has an entry for inode
    /// `child`. The check uses it to decide which of two names to keep.
    pub(crate) fn dir_names(
        &mut self,
        view: &Loaded,
        size: u64,
        child: u32,
    ) -> Result<bool, FsError> {
        if folder_shape_ok(view, size).is_err() {
            return Ok(false);
        }
        for k in 0..size / BLOCK_U64 {
            let block = self.read_folder_block(view, k)?;
            for entry in Entries::new(&block[..]) {
                let Ok(entry) = entry else { return Ok(false) };
                if entry.inode == child && crate::names::valid_stored_name(entry.name(&block[..])) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Adds the name `name` for inode `child` to folder `n`: into the first block with room, or
    /// in a new block. The caller has checked that the name is free and valid. A repair passes
    /// `privileged` to be allowed the reserve.
    pub(crate) fn dir_insert(
        &mut self,
        n: u32,
        name: &[u8],
        child: u32,
        kind: u8,
        privileged: bool,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(n, true)?;
        let view = self.folder_view(n, &inode)?;
        let blocks = inode.size / BLOCK_U64;
        for k in 0..blocks {
            let mut block = self.read_folder_block(&view, k)?;
            if folder::try_insert(&mut block[..], name, child, kind)? {
                return self.dir_rewrite(n, &inode, &view, k, &block, privileged);
            }
        }
        let block = folder::new_block(name, child, kind);
        self.dir_rewrite(n, &inode, &view, blocks, &block, privileged)
    }

    /// Removes the entry for inode `child` stored under exactly `name` from folder `n`.
    /// May use the reserve: deleting must work on a full disk.
    pub(crate) fn dir_delete(&mut self, n: u32, child: u32, name: &[u8]) -> Result<(), FsError> {
        let inode = self.read_inode(n, true)?;
        let view = self.folder_view(n, &inode)?;
        for k in 0..inode.size / BLOCK_U64 {
            let mut block = self.read_folder_block(&view, k)?;
            let mut found = None;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode == child && entry.name(&block[..]) == name {
                    found = Some(entry.pos);
                    break;
                }
            }
            if let Some(pos) = found {
                folder::remove_at(&mut block[..], pos)?;
                return self.dir_rewrite(n, &inode, &view, k, &block, true);
            }
        }
        Err(FsError::NotFound)
    }

    /// Makes the entry in folder `n` stored under exactly `old_name` name inode `child` instead,
    /// under the spelling `new_name`, in a single block write. This is how a rename replaces an
    /// existing file: the name is always either the old file or the new one.
    ///
    /// If the new spelling does not fit in that block the entry keeps its old spelling (still
    /// one write, still atomic).
    pub(crate) fn dir_replace(
        &mut self,
        n: u32,
        old_name: &[u8],
        new_name: &[u8],
        child: u32,
        kind: u8,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(n, true)?;
        let view = self.folder_view(n, &inode)?;
        for k in 0..inode.size / BLOCK_U64 {
            let mut block = self.read_folder_block(&view, k)?;
            let mut found = None;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode != 0 && entry.name(&block[..]) == old_name {
                    found = Some(entry.pos);
                    break;
                }
            }
            if let Some(pos) = found {
                if !folder::replace_at(&mut block[..], pos, new_name, child, kind)? {
                    folder::repoint(&mut block[..], pos, child, kind);
                }
                return self.dir_rewrite(n, &inode, &view, k, &block, false);
            }
        }
        Err(FsError::NotFound)
    }

    /// Makes the entry in folder `n` stored under exactly `name` name inode `child` instead.
    pub(crate) fn dir_repoint(
        &mut self,
        n: u32,
        name: &[u8],
        child: u32,
        kind: u8,
        privileged: bool,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(n, true)?;
        let view = self.folder_view(n, &inode)?;
        for k in 0..inode.size / BLOCK_U64 {
            let mut block = self.read_folder_block(&view, k)?;
            let mut found = None;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode != 0 && entry.name(&block[..]) == name {
                    found = Some(entry.pos);
                    break;
                }
            }
            if let Some(pos) = found {
                folder::repoint(&mut block[..], pos, child, kind);
                return self.dir_rewrite(n, &inode, &view, k, &block, privileged);
            }
        }
        Err(FsError::NotFound)
    }
}
