// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding, creating and freeing inodes, and turning a `NodeId` back into one.
//!
//! The inode table is an ordinary file whose blocks come from the data area (ROCKFS.md, "The
//! inode table"), so a volume never runs out of inodes while it has free space: when every slot
//! is in use the table is made one block (16 inodes) longer.

use crate::content::{Target, Update};
use crate::extents::lookup;
use crate::inode::{Inode, Top};
use crate::layout::*;
use crate::volume::{RockFs, node_id};
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError, NodeId};

impl<D: BlockDevice> RockFs<D> {
    /// The inode a `NodeId` names. `Gone` if that inode is free or has been reused since (its
    /// generation is different): an old `NodeId` must never reach a newer file.
    pub(crate) fn resolve(&mut self, node: NodeId) -> Result<(u32, Inode), FsError> {
        let n = (node & 0xFFFF_FFFF) as u32;
        let generation = (node >> 32) as u32;
        if n == 0 || n >= self.sb.inode_count {
            return Err(FsError::Gone);
        }
        let inode = self.read_inode(n, true)?;
        if !inode.in_use() || inode.generation != generation {
            return Err(FsError::Gone);
        }
        if inode.kind != TYPE_FILE && inode.kind != TYPE_FOLDER {
            return Err(FsError::Damaged);
        }
        Ok((n, inode))
    }

    /// Like [`resolve`](Self::resolve), for a node that must be a folder.
    pub(crate) fn resolve_folder(&mut self, node: NodeId) -> Result<(u32, Inode), FsError> {
        let (n, inode) = self.resolve(node)?;
        if inode.kind != TYPE_FOLDER {
            return Err(FsError::NotAFolder);
        }
        Ok((n, inode))
    }

    /// Like [`resolve`](Self::resolve), for a node that must be a file.
    pub(crate) fn resolve_file(&mut self, node: NodeId) -> Result<(u32, Inode), FsError> {
        let (n, inode) = self.resolve(node)?;
        if inode.kind != TYPE_FILE {
            return Err(FsError::NotAFile);
        }
        Ok((n, inode))
    }

    /// The `NodeId` of inode `n`.
    pub(crate) fn node_of(&self, n: u32, inode: &Inode) -> NodeId {
        node_id(n, inode.generation)
    }

    /// The first free inode at or after `from`, and its generation.
    fn find_free_inode(&mut self, from: u32) -> Result<Option<(u32, u32)>, FsError> {
        let count = self.sb.inode_count;
        let mut n = from.max(2);
        while n < count {
            // Read a whole table block of 16 inodes at a time.
            let first = n - n % INODES_PER_BLOCK;
            let disk = lookup(&self.table.extents, u64::from(first / INODES_PER_BLOCK))
                .ok_or(FsError::Damaged)?;
            let block = self.read_block(disk)?;
            for i in n % INODES_PER_BLOCK..INODES_PER_BLOCK {
                let slot = first + i;
                if slot >= count {
                    break;
                }
                let raw = &block[i as usize * INODE_SIZE..][..INODE_SIZE];
                if u16_at(raw, 0) == TYPE_FREE {
                    return Ok(Some((slot, u32_at(raw, 32))));
                }
            }
            n = first + INODES_PER_BLOCK;
        }
        Ok(None)
    }

    /// Makes the inode table one block (16 inodes) longer.
    ///
    /// The new block is zeros, so its inodes are free. It is added like any copy-on-write
    /// change; the table's own inode lives in the superblock, so the change takes effect when
    /// the superblock is rewritten (backup, flush, main), with the larger `inode_count` in the
    /// same write.
    fn grow_table(&mut self) -> Result<(), FsError> {
        let count = self.sb.inode_count;
        let new_count = count.checked_add(INODES_PER_BLOCK).ok_or(FsError::NoSpace)?;
        let size = self.sb.table.size;
        let zeros = [0u8; BLOCK];
        let old = self.table.clone();
        let cow =
            self.cow_apply(&old.extents, size, size + BLOCK_U64, Some((size, &zeros)), false)?;
        let update = Update {
            target: Target::Table { inode_count: new_count },
            size: size + BLOCK_U64,
            modified: self.now(),
            extents: &cow.extents,
            old_tree: &old.tree,
            release: &cow.dropped,
            privileged: false,
        };
        match self.commit_update(&update) {
            Ok(tree) => {
                self.table.extents = alloc::rc::Rc::new(cow.extents);
                self.table.tree = tree;
                Ok(())
            }
            Err(e) => {
                self.release(&cow.new_runs);
                Err(e)
            }
        }
    }

    /// Takes a free inode for a new file or folder, growing the table if there is none, and
    /// writes it. Nothing refers to it until the caller adds a name for it to a folder.
    pub(crate) fn alloc_inode(&mut self, kind: u16, parent: u32) -> Result<(u32, Inode), FsError> {
        let found = match self.find_free_inode(self.inode_hint)? {
            Some(f) => Some(f),
            None if self.inode_hint > 2 => self.find_free_inode(2)?,
            None => None,
        };
        let (n, generation) = match found {
            Some(f) => f,
            None => {
                let first_new = self.sb.inode_count;
                self.grow_table()?;
                (first_new, 0)
            }
        };
        let now = self.now();
        let inode = Inode {
            kind,
            flags: 0,
            parent,
            size: 0,
            created: now,
            modified: now,
            generation,
            depth: 0,
            top: Top::Extents(Vec::new()),
        };
        self.write_inode(n, &inode)?;
        self.sb.free_inodes = self.sb.free_inodes.saturating_sub(1);
        self.inode_hint = n + 1;
        Ok((n, inode))
    }

    /// Frees inode `n` (its committed state is `inode`) and the blocks it owns: it is marked free
    /// with its generation increased, so that a `NodeId` for the old file is `Gone` for ever.
    ///
    /// The blocks are only freed once the freed inode is on the disk: otherwise a block could be
    /// reused and rewritten while a lost write left the old inode still pointing at it.
    /// If the inode's extent tree cannot be read (it is damaged) the inode is freed anyway and the
    /// blocks are left for the next check to reclaim: a damaged file can always be deleted.
    pub(crate) fn free_inode(&mut self, n: u32, inode: &Inode) -> Result<(), FsError> {
        self.writable()?;
        let owned = self.load_extents(inode).ok();
        self.write_inode(n, &Inode::free(inode.generation.wrapping_add(1)))?;
        self.dev_flush()?;
        if let Some(p) = self.pending.remove(&n) {
            // Uncommitted data nothing on the disk pointed at.
            self.release(p.fresh.runs());
        }
        if let Some(loaded) = owned {
            let runs: Vec<_> = loaded.extents.iter().map(|e| e.run()).collect();
            self.release(&runs);
            self.release_blocks(&loaded.tree);
        }
        self.sb.free_inodes = self.sb.free_inodes.saturating_add(1);
        self.inode_hint = self.inode_hint.min(n);
        self.cache = None;
        Ok(())
    }
}
