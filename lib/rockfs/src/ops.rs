// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The [`Filesystem`] implementation: what `lib/fsserve` calls (docs/design/0016-filesystems.md,
//! section 1).
//!
//! Two kinds of change, with different promises (note 0014, section 6):
//! - a **file's contents** (`write`, `set_size`) are written to new blocks at once and stay
//!   *pending* until `commit` or `unmount`, which makes them all take effect together;
//! - **folder changes** (`create`, `remove`, `rename`) are durable when the call returns, with
//!   a flush between the steps whose order matters (ROCKFS.md, "Saving safely").

use crate::dirs::Found;
use crate::folder::Entries;
use crate::inode::Inode;
use crate::layout::*;
use crate::names::valid_name;
use crate::volume::RockFs;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, Entry, Filesystem, FsError, Kind, NodeId, Space, Stat};

/// The kind a folder entry's type byte and an inode's type stand for.
fn kind_of(inode: &Inode) -> Kind {
    if inode.kind == TYPE_FOLDER { Kind::Folder } else { Kind::File }
}

/// The folder cursor for "the entry at byte `pos` of block `k`, and everything after it". It is
/// never 0, since 0 means "start" or "the end".
fn cursor_for(k: u64, pos: usize) -> u64 {
    (k << 12 | pos as u64) + 1
}

impl<D: BlockDevice> RockFs<D> {
    /// The inode a folder entry names, checked: it must be in use and of the type the entry
    /// says. (A mismatch means the folder or the inode is damaged.)
    pub(crate) fn read_child(&mut self, found: &Found) -> Result<(u32, Inode), FsError> {
        let n = found.entry.inode;
        if n == 0 || n >= self.sb.inode_count {
            return Err(FsError::Damaged);
        }
        let inode = self.read_inode(n, true)?;
        let type_byte = u8::try_from(inode.kind).map_err(|_| FsError::Damaged)?;
        if !matches!(inode.kind, TYPE_FILE | TYPE_FOLDER) || type_byte != found.entry.kind {
            return Err(FsError::Damaged);
        }
        Ok((n, inode))
    }

    /// A file's size and modification time including pending changes.
    fn current(&self, n: u32, inode: &Inode) -> (u64, u64) {
        match self.pending.get(&n) {
            Some(p) if p.generation == inode.generation => (p.size, p.modified),
            _ => (inode.size, inode.modified),
        }
    }

    /// Makes every pending file durable. Stops at the first failure.
    pub(crate) fn commit_all(&mut self) -> Result<(), FsError> {
        let ids: Vec<u32> = self.pending.keys().copied().collect();
        let mut first = Ok(());
        for n in ids {
            if let Err(e) = self.commit_pending(n)
                && first.is_ok()
            {
                first = Err(e);
            }
        }
        first
    }
}

impl<D: BlockDevice> Filesystem for RockFs<D> {
    fn root(&self) -> NodeId {
        self.root
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError> {
        let (n, inode) = self.resolve_folder(folder)?;
        let found = self.dir_find(n, &inode, name.as_bytes())?.ok_or(FsError::NotFound)?;
        let (child, child_inode) = self.read_child(&found)?;
        Ok(self.node_of(child, &child_inode))
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError> {
        let (n, inode) = self.resolve(node)?;
        let (size, modified) = self.current(n, &inode);
        Ok(Stat {
            kind: kind_of(&inode),
            size: if inode.kind == TYPE_FILE { size } else { 0 },
            created: inode.created,
            modified,
            hidden: inode.flags & 1 != 0,
        })
    }

    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let (n, inode) = self.resolve_file(node)?;
        if buf.len() > MAX_TRANSFER {
            return Err(FsError::BadArgument);
        }
        let (size, extents) = match self.pending.get(&n) {
            Some(p) if p.generation == inode.generation => (p.size, p.extents.clone()),
            _ => (inode.size, self.extents_loaded(n, &inode)?.extents),
        };
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        let len = (size - offset).min(buf.len() as u64) as usize;
        let mut done = 0;
        while done < len {
            let pos = offset + done as u64;
            let k = pos / BLOCK_U64;
            let in_block = (pos % BLOCK_U64) as usize;
            // The extent holding block `k`, if any: otherwise `k` is in a hole that lasts until
            // the next extent starts, and reads as zeros.
            let after = extents.partition_point(|e| e.file_block <= k);
            let mapped = after.checked_sub(1).map(|i| extents[i]).filter(|e| k < e.file_end());
            match mapped {
                Some(e) => {
                    let room = (e.file_end() - k) * BLOCK_U64 - in_block as u64;
                    let take = ((len - done) as u64).min(room) as usize;
                    let first = e.disk_block + (k - e.file_block);
                    let count = (in_block + take).div_ceil(BLOCK);
                    let mut raw = alloc::vec![0u8; count * BLOCK];
                    self.read_blocks(first, &mut raw)?;
                    buf[done..done + take].copy_from_slice(&raw[in_block..in_block + take]);
                    done += take;
                }
                None => {
                    let hole_end = extents.get(after).map_or(u64::MAX, |e| e.file_block);
                    let room = hole_end.saturating_mul(BLOCK_U64) - pos;
                    let take = (len - done).min(room.min(usize::MAX as u64) as usize);
                    buf[done..done + take].fill(0);
                    done += take;
                }
            }
        }
        Ok(len)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        self.writable()?;
        let (n, inode) = self.resolve_file(node)?;
        if data.len() > MAX_TRANSFER {
            return Err(FsError::BadArgument);
        }
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(data.len() as u64).ok_or(FsError::TooBig)?;
        let (size, _) = self.current(n, &inode);
        self.modify_file(n, &inode, Some((offset, data)), size.max(end))?;
        Ok(data.len())
    }

    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError> {
        self.writable()?;
        let (n, inode) = self.resolve_file(node)?;
        if self.current(n, &inode).0 == size {
            return Ok(());
        }
        self.modify_file(n, &inode, None, size)
    }

    fn commit(&mut self, node: NodeId) -> Result<(), FsError> {
        let (n, _) = self.resolve(node)?;
        self.commit_pending(n)
    }

    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        out: &mut dyn FnMut(Entry) -> bool,
    ) -> Result<u64, FsError> {
        let (n, inode) = self.resolve_folder(folder)?;
        let view = self.folder_view(n, &inode)?;
        let blocks = inode.size / BLOCK_U64;
        let (mut k, mut from) = match cursor.checked_sub(1) {
            Some(v) => (v >> 12, (v & 0xFFF) as usize),
            None => (0, 0),
        };
        while k < blocks {
            let block = self.read_folder_block(&view, k)?;
            for entry in Entries::new(&block[..]) {
                let entry = entry?;
                if entry.inode == 0 || entry.pos < from {
                    continue;
                }
                let name = entry.name(&block[..]);
                if !crate::names::valid_stored_name(name) {
                    return Err(FsError::Damaged);
                }
                let found = Found { entry, name: name.to_vec() };
                let (child, child_inode) = self.read_child(&found)?;
                let (size, modified) = self.current(child, &child_inode);
                let item = Entry {
                    // `valid_stored_name` has just shown these bytes are UTF-8.
                    name: alloc::string::String::from_utf8_lossy(name).into_owned(),
                    kind: kind_of(&child_inode),
                    size: if child_inode.kind == TYPE_FILE { size } else { 0 },
                    modified,
                    hidden: child_inode.flags & 1 != 0,
                };
                if !out(item) {
                    return Ok(cursor_for(k, entry.pos));
                }
            }
            k += 1;
            from = 0;
        }
        Ok(0)
    }

    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError> {
        self.writable()?;
        let (n, inode) = self.resolve_folder(folder)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        if self.dir_find(n, &inode, name.as_bytes())?.is_some() {
            return Err(FsError::AlreadyExists);
        }
        let type_code = if kind == Kind::Folder { TYPE_FOLDER } else { TYPE_FILE };
        // The new inode is written first; the name that makes it reachable second. The folder
        // change begins by flushing, which puts the inode on the disk before the folder's own
        // inode is rewritten to include the name.
        let (child, child_inode) = self.alloc_inode(type_code, n)?;
        match self.dir_insert(n, name.as_bytes(), child, type_code as u8, false) {
            Ok(()) => Ok(self.node_of(child, &child_inode)),
            Err(e) => {
                // No name was added, so nothing refers to the inode: free it again.
                let _ = self.free_inode(child, &child_inode);
                Err(e)
            }
        }
    }

    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError> {
        self.writable()?;
        let (n, inode) = self.resolve_folder(folder)?;
        let found = self.dir_find(n, &inode, name.as_bytes())?.ok_or(FsError::NotFound)?;
        let (child, child_inode) = self.read_child(&found)?;
        if child_inode.kind == TYPE_FOLDER && !self.dir_is_empty(child, &child_inode)? {
            return Err(FsError::FolderNotEmpty);
        }
        // First the name, then the inode and its blocks (ROCKFS.md, "Deleting a file or folder").
        self.dir_delete(n, child, &found.name)?;
        self.free_inode(child, &child_inode)
    }

    fn rename(
        &mut self,
        from: NodeId,
        from_name: &str,
        to: NodeId,
        to_name: &str,
        replace: bool,
    ) -> Result<(), FsError> {
        self.writable()?;
        let (from_n, from_inode) = self.resolve_folder(from)?;
        let (to_n, to_inode) = self.resolve_folder(to)?;
        if !valid_name(to_name) {
            return Err(FsError::BadName);
        }
        let src = self.dir_find(from_n, &from_inode, from_name.as_bytes())?;
        let src = src.ok_or(FsError::NotFound)?;
        let (moving, moving_inode) = self.read_child(&src)?;
        if moving_inode.kind == TYPE_FOLDER {
            // A folder can't go inside itself: walk up from the destination to the root.
            let mut up = to_n;
            for _ in 0..=self.sb.inode_count {
                if up == moving {
                    return Err(FsError::BadArgument);
                }
                if up == self.sb.root_inode {
                    break;
                }
                up = self.read_inode(up, true)?.parent;
            }
            if up != self.sb.root_inode {
                return Err(FsError::Damaged); // the parent links loop
            }
        }
        let type_byte = moving_inode.kind as u8;
        let dst = self.dir_find(to_n, &to_inode, to_name.as_bytes())?;
        // What is being replaced, if anything: only an existing file, and only when asked.
        let replaced = match &dst {
            Some(d) if d.entry.inode == moving => {
                // The same item under a name that counts as the same: only the capitals change.
                if from_n != to_n {
                    return Err(FsError::Damaged); // one item named in two folders
                }
                if d.name == to_name.as_bytes() {
                    return Ok(());
                }
                None
            }
            Some(d) => {
                let (old, old_inode) = self.read_child(d)?;
                if !replace || old_inode.kind != TYPE_FILE {
                    return Err(FsError::AlreadyExists);
                }
                Some((old, old_inode, d.name.clone()))
            }
            None => None,
        };
        let to = to_name.as_bytes();
        if from_n == to_n {
            // Within one folder the whole rename is one change of the folder: atomic.
            let over = replaced.as_ref().map(|(_, _, name)| name.as_slice());
            self.dir_rename_within(from_n, &src.name, moving, to, type_byte, over)?;
        } else {
            // Across folders it takes several writes. The item's `parent` says which folder
            // it is going to, and is written first when replacing, so that after a crash with
            // a name in both folders, the check keeps the name in the folder `parent` names
            // (ROCKFS.md, "Checking a volume"):
            // - a plain move adds the new name, then changes `parent`, then removes the old
            //   name: a crash before `parent` changes keeps the old name, after it the new;
            // - a move over a file changes `parent`, then points the destination's name at
            //   the item in one folder write (so that name is always the old file or the new),
            //   then removes the old name.
            match &replaced {
                None => {
                    self.dir_insert(to_n, to, moving, type_byte, false)?;
                    self.set_parent(moving, to_n)?;
                }
                Some((_, _, name)) => {
                    self.set_parent(moving, to_n)?;
                    if let Err(e) = self.dir_replace(to_n, name, to, moving, type_byte) {
                        // Nothing was replaced: put `parent` back.
                        self.set_parent(moving, from_n)?;
                        return Err(e);
                    }
                }
            }
            self.dir_delete(from_n, moving, &src.name)?;
        }
        if let Some((old, old_inode, _)) = replaced {
            self.free_inode(old, &old_inode)?;
        }
        Ok(())
    }

    fn space(&mut self) -> Result<Space, FsError> {
        Ok(Space {
            total: self.sb.block_count.saturating_mul(BLOCK_U64),
            free: self.available(false).saturating_mul(BLOCK_U64),
            label: self.sb.label_text(),
            filesystem: "RockFS",
        })
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        if self.read_only {
            return Ok(());
        }
        if self.poisoned {
            return Err(FsError::Disk);
        }
        // If a commit fails (for example no room for its extent tree) the volume is left
        // marked "in use": the next mount checks it, and the file is as it was before.
        self.commit_all()?;
        // The bitmap must be on the disk before the volume is marked clean, or a clean volume
        // could carry a stale bitmap.
        self.write_bitmap()?;
        let mut sb = self.sb.clone();
        sb.state = 0;
        sb.free_blocks = self.bitmap.free();
        self.write_superblocks(&sb)?;
        self.read_only = true;
        Ok(())
    }
}

impl<D: BlockDevice> RockFs<D> {
    /// Records in inode `n` that it is now in folder `parent`.
    pub(crate) fn set_parent(&mut self, n: u32, parent: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(n, true)?;
        if inode.parent != parent {
            inode.parent = parent;
            self.write_inode(n, &inode)?;
        }
        Ok(())
    }
}
