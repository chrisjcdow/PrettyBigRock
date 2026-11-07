// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The [`Filesystem`] implementation: what `lib/fsserve` calls (docs/design/0016-filesystems.md,
//! section 1).
//!
//! Two kinds of change, with different promises (note 0014, section 6):
//! - a **file's contents** (`write`, `set_size`) are visible at once and become durable together
//!   at `commit` or `unmount` (`file.rs`);
//! - **folder changes** (`create`, `remove`, `rename`) are durable when the call returns: each
//!   ends with a flush, and has one in the middle where the order of writes matters.
//!
//! Every method that changes the volume first checks that it may (`writable`), and passes any
//! failure of the disk through `track`, which stops further changes: after a failed write the
//! disk and our picture of it may disagree.

use crate::chain::Chain;
use crate::dirent::{ATTR_ARCHIVE, ATTR_DIR, ENTRY, Item, dot_entries, set_cluster, short_entry};
use crate::folder::Folder;
use crate::mount::{Fat, Node, ROOT};
use crate::names::{exact_short, lfn_entries, make_short, valid_name};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, Entry, Filesystem, FsError, Kind, NodeId, Space, Stat};

impl<D: BlockDevice> Fat<D> {
    /// The first cluster of folder `id`: `Gone` if there is no such node, `NotAFolder` if it is
    /// a file.
    fn folder_first(&self, id: NodeId) -> Result<u32, FsError> {
        let node = self.nodes.get(&id).ok_or(FsError::Gone)?;
        if node.kind != Kind::Folder {
            return Err(FsError::NotAFolder);
        }
        Ok(node.chain.first)
    }

    /// The node for `item` of folder `parent`, made if this is the first time it has been
    /// looked at.
    fn node_for(&mut self, parent: NodeId, item: &Item) -> Result<NodeId, FsError> {
        if let Some(&id) = self.by_entry.get(&(parent, item.slot)) {
            return Ok(id);
        }
        let geo = &self.vol.geo;
        let kind = if item.is_dir() { Kind::Folder } else { Kind::File };
        // A folder needs a cluster. A file with no cluster is empty (if it says otherwise, it
        // is reported when read). Nothing may point outside the volume, and no file can be
        // bigger than the volume.
        let cluster_ok = match kind {
            Kind::Folder => geo.is_data_cluster(item.cluster),
            Kind::File => {
                (item.cluster == 0 || geo.is_data_cluster(item.cluster))
                    && u64::from(item.size).div_ceil(u64::from(geo.cluster_bytes))
                        <= u64::from(geo.clusters)
            }
        };
        if !cluster_ok {
            return Err(FsError::Damaged);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(
            id,
            Node {
                kind,
                parent,
                slot: item.slot,
                chain: Chain::new(item.cluster),
                size: if kind == Kind::File { item.size } else { 0 },
                created: item.created,
                modified: item.modified,
                hidden: item.hidden(),
                dirty: false,
                // A file with no clusters has nothing to check.
                verified: kind == Kind::Folder || item.cluster == 0,
                trim: false,
            },
        );
        self.by_entry.insert((parent, item.slot), id);
        Ok(id)
    }

    /// Makes the entries for a new name: its short name and, if it needs one, its long-name
    /// entries (in disk order). `taken` holds the short names that may not be used.
    fn new_entries(
        name: &str,
        taken: &BTreeSet<[u8; 11]>,
    ) -> Result<([u8; 11], Vec<[u8; ENTRY]>), FsError> {
        if let Some(short) = exact_short(name)
            && !taken.contains(&short)
        {
            return Ok((short, Vec::new()));
        }
        let short = make_short(name, &|s| taken.contains(s)).ok_or(FsError::NoSpace)?;
        Ok((short, lfn_entries(name, &short)))
    }

    /// Finds room for `count` entries in a row in `folder`, adding clusters to the folder if it
    /// is full. If that fails, the clusters it added are given back.
    fn reserve_run(&mut self, folder: &mut Folder, count: u32) -> Result<u32, FsError> {
        let original_clusters = folder.clusters.len();
        let original_last = folder.clusters.last().copied();
        loop {
            if let Some(start) = folder.find_run(count) {
                return Ok(start);
            }
            if let Err(e) = self.vol.grow_folder(folder) {
                // Undo what this call added.
                if let (Some(last), Some(&first_added)) =
                    (original_last, folder.clusters.get(original_clusters))
                {
                    self.vol.release_tail(Some(last), first_added)?;
                }
                folder.clusters.truncate(original_clusters);
                let bytes = original_clusters * self.vol.geo.cluster_bytes as usize;
                folder.bytes.truncate(bytes);
                return Err(e);
            }
        }
    }

    /// Creates a file or folder (`create`, once the checks are done).
    fn create_inner(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError> {
        let parent_first = self.folder_first(folder)?;
        let mut f = self.vol.load_folder(parent_first)?;
        let items = f.items();
        if items.iter().any(|i| i.matches(name)) {
            return Err(FsError::AlreadyExists);
        }
        let taken: BTreeSet<[u8; 11]> = items.iter().map(|i| i.short).collect();
        let (short, mut entries) = Self::new_entries(name, &taken)?;
        let now = (self.clock)();

        // A new folder gets a cluster of its own, holding `.` and `..`, written before any
        // entry refers to it. The parent is written as 0 if it is the root, as every FAT
        // system does.
        let cluster = if kind == Kind::Folder {
            let c = self.vol.alloc_zeroed(None)?;
            let up = if folder == ROOT { 0 } else { parent_first };
            let dots = dot_entries(c, up, now);
            let written = match self.vol.geo.cluster_offset(c) {
                Some(at) => self.vol.write_bytes(at, &dots.concat()),
                None => Err(FsError::Damaged),
            };
            if let Err(e) = written {
                self.vol.free_chain(c)?;
                return Err(e);
            }
            c
        } else {
            0
        };
        let attr = if kind == Kind::Folder { ATTR_DIR } else { ATTR_ARCHIVE };
        entries.push(short_entry(&short, attr, cluster, 0, now));

        let placed = match self.reserve_run(&mut f, entries.len() as u32) {
            Ok(start) => start,
            Err(e) => {
                if cluster != 0 {
                    self.vol.free_chain(cluster)?;
                }
                return Err(e);
            }
        };
        let changed = f.place(placed, &entries)?;
        if cluster != 0 {
            // The new folder's cluster and the FAT are on the disk before its entry.
            self.vol.flush()?;
        }
        self.vol.write_slots(&f, placed, changed)?;
        self.vol.flush()?;

        let slot = placed + entries.len() as u32 - 1;
        let item = f.items().into_iter().find(|i| i.slot == slot).ok_or(FsError::Damaged)?;
        self.node_for(folder, &item)
    }

    /// Removes a file or empty folder (`remove`, once the checks are done).
    fn remove_inner(&mut self, folder: NodeId, name: &str) -> Result<(), FsError> {
        let first = self.folder_first(folder)?;
        let mut f = self.vol.load_folder(first)?;
        let item = f.items().into_iter().find(|i| i.matches(name)).ok_or(FsError::NotFound)?;
        let node_id = self.by_entry.get(&(folder, item.slot)).copied();
        // The chain to free is the node's, if there is one: it may be ahead of the entry.
        let chain_first =
            node_id.and_then(|id| self.nodes.get(&id)).map_or(item.cluster, |n| n.chain.first);
        if item.is_dir() {
            if !self.vol.geo.is_data_cluster(item.cluster) {
                return Err(FsError::Damaged);
            }
            if !self.vol.load_folder(item.cluster)?.is_empty() {
                return Err(FsError::FolderNotEmpty);
            }
        }
        // Check the chain is a proper one before anything is changed.
        self.vol.check_chain(chain_first)?;

        // The entries go first, and are on the disk before the clusters are freed: a crash
        // between loses space, but never leaves an entry pointing at clusters another file
        // might take.
        f.delete(item.first_slot, item.slot)?;
        self.vol.write_slots(&f, item.first_slot, item.slot - item.first_slot + 1)?;
        self.vol.flush()?;
        if chain_first != 0 {
            self.vol.free_chain(chain_first)?;
            self.vol.flush()?;
        }
        self.by_entry.remove(&(folder, item.slot));
        if let Some(id) = node_id {
            self.nodes.remove(&id);
        }
        Ok(())
    }

    /// Renames or moves (`rename`, once the checks are done).
    fn rename_inner(
        &mut self,
        from: NodeId,
        from_name: &str,
        to: NodeId,
        to_name: &str,
        replace: bool,
    ) -> Result<(), FsError> {
        let from_first = self.folder_first(from)?;
        let to_first = self.folder_first(to)?;
        let mut src = self.vol.load_folder(from_first)?;
        // The same folder is loaded once, so that both halves of the change see each other.
        let mut other = if from == to { None } else { Some(self.vol.load_folder(to_first)?) };
        let src_items = src.items();
        let item = src_items.iter().find(|i| i.matches(from_name)).ok_or(FsError::NotFound)?;
        let dst_items = match &other {
            Some(d) => d.items(),
            None => src_items.clone(),
        };

        // What is at the destination already.
        let existing = dst_items.iter().find(|i| i.matches(to_name));
        let same = existing.is_some_and(|e| from == to && e.slot == item.slot);
        if same && item.name.as_deref() == Some(to_name) {
            return Ok(()); // renaming a file to its own name
        }
        let moving_id = self.by_entry.get(&(from, item.slot)).copied();
        if item.is_dir() && !same {
            if !self.vol.geo.is_data_cluster(item.cluster) {
                return Err(FsError::Damaged);
            }
            // A folder cannot go inside itself: walk up from the destination to the root. (The
            // folders above one we have a node for all have nodes: each was reached by looking
            // it up in the one above.)
            if let Some(moving) = moving_id {
                let mut up = to;
                let mut reached_root = false;
                for _ in 0..=self.nodes.len() {
                    if up == moving {
                        return Err(FsError::BadArgument);
                    }
                    if up == ROOT {
                        reached_root = true;
                        break;
                    }
                    up = self.nodes.get(&up).ok_or(FsError::Damaged)?.parent;
                }
                if !reached_root {
                    return Err(FsError::Damaged); // the parent links loop
                }
            }
        }

        // Only a file is replaced, and only when asked. (Checked after the folder-into-itself
        // test, so the error for that does not depend on what happens to be at the destination.)
        let replaced = match existing {
            Some(e) if !same => {
                if !replace || e.is_dir() {
                    return Err(FsError::AlreadyExists);
                }
                Some(e)
            }
            _ => None,
        };

        // A moved folder's `..` must say where it now is. Check it is there before changing
        // anything.
        let dotdot = if item.is_dir() && from != to {
            let at =
                self.vol.geo.cluster_offset(item.cluster).ok_or(FsError::Damaged)? + ENTRY as u64;
            let mut e = [0u8; ENTRY];
            self.vol.read_bytes(at, &mut e)?;
            if &e[..11] != b"..         " {
                return Err(FsError::Damaged);
            }
            Some((at, e))
        } else {
            None
        };

        // The chain of a file being replaced is checked before anything is changed.
        let replaced_chain = match replaced {
            Some(r) => {
                let first = self
                    .by_entry
                    .get(&(to, r.slot))
                    .and_then(|id| self.nodes.get(id))
                    .map_or(r.cluster, |n| n.chain.first);
                self.vol.check_chain(first)?;
                first
            }
            None => 0,
        };

        // The new name's entries. The names that are about to disappear do not count as taken.
        let mut taken: BTreeSet<[u8; 11]> = dst_items.iter().map(|i| i.short).collect();
        if from == to {
            taken.remove(&item.short);
        }
        if let Some(r) = replaced {
            taken.remove(&r.short);
        }
        let (short, mut entries) = Self::new_entries(to_name, &taken)?;
        // The short entry is the old one with the new name: everything else (times, attributes,
        // first cluster, size) stays. The "NT" byte, which only describes the old short name's
        // case, goes.
        let mut moved = *src.entry(item.slot).ok_or(FsError::Damaged)?;
        moved[..11].copy_from_slice(&short);
        moved[12] = 0;
        entries.push(moved);

        // Copy out what is needed from the items, which borrow the folders.
        let (old_first, old_slot) = (item.first_slot, item.slot);
        let replaced_slots = replaced.map(|r| (r.first_slot, r.slot));

        // 1. The new entries go in, and are on the disk before the old ones go: a crash in
        // between leaves the file under both names, never under neither.
        let dst = other.as_mut().unwrap_or(&mut src);
        let start = self.reserve_run(dst, entries.len() as u32)?;
        let changed = dst.place(start, &entries)?;
        self.vol.write_slots(dst, start, changed)?;
        if let Some((at, mut e)) = dotdot {
            let up = if to == ROOT { 0 } else { to_first };
            set_cluster(&mut e, up);
            self.vol.write_bytes(at, &e)?;
        }
        self.vol.flush()?;

        // 2. The old entries, and the replaced file's, are deleted.
        src.delete(old_first, old_slot)?;
        self.vol.write_slots(&src, old_first, old_slot - old_first + 1)?;
        if let Some((first, slot)) = replaced_slots {
            let dst = other.as_mut().unwrap_or(&mut src);
            dst.delete(first, slot)?;
            self.vol.write_slots(dst, first, slot - first + 1)?;
        }
        self.vol.flush()?;
        // 3. Only now can the replaced file's clusters be freed.
        if replaced_chain != 0 {
            self.vol.free_chain(replaced_chain)?;
            self.vol.flush()?;
        }

        // The nodes follow the entries: the replaced file is gone, the moved one has a new place.
        if let Some((_, slot)) = replaced_slots
            && let Some(id) = self.by_entry.remove(&(to, slot))
        {
            self.nodes.remove(&id);
        }
        if let Some(id) = moving_id {
            let new_slot = start + entries.len() as u32 - 1;
            self.by_entry.remove(&(from, old_slot));
            self.by_entry.insert((to, new_slot), id);
            if let Some(node) = self.nodes.get_mut(&id) {
                node.parent = to;
                node.slot = new_slot;
            }
        }
        Ok(())
    }
}

impl<D: BlockDevice> Filesystem for Fat<D> {
    fn root(&self) -> NodeId {
        ROOT
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError> {
        let first = self.folder_first(folder)?;
        let found = self.vol.load_folder(first)?;
        let item = found.items().into_iter().find(|i| i.matches(name)).ok_or(FsError::NotFound)?;
        self.node_for(folder, &item)
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError> {
        let n = self.nodes.get(&node).ok_or(FsError::Gone)?;
        Ok(Stat {
            kind: n.kind,
            size: u64::from(n.size),
            created: n.created,
            modified: n.modified,
            hidden: n.hidden,
        })
    }

    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        self.file_read(node, offset, buf)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let result = self.file_write(node, offset, data);
        self.track(result)
    }

    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError> {
        let result = self.file_set_size(node, size);
        self.track(result)
    }

    fn commit(&mut self, node: NodeId) -> Result<(), FsError> {
        self.file_commit(node)
    }

    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        out: &mut dyn FnMut(Entry) -> bool,
    ) -> Result<u64, FsError> {
        let first = self.folder_first(folder)?;
        let loaded = self.vol.load_folder(first)?;
        // The cursor is one more than the slot of the first entry of the next item (so that
        // 0 can mean "the start"). Items are found by their slot, not counted, so a folder
        // changed between calls may repeat or miss an item, but cannot invent one.
        for item in loaded.items() {
            let Some(name) = item.name.clone() else { continue };
            if item.dots || u64::from(item.first_slot) + 1 < cursor {
                continue;
            }
            let kind = if item.is_dir() { Kind::Folder } else { Kind::File };
            // A file with changes not yet committed shows them.
            let live = self.by_entry.get(&(folder, item.slot)).and_then(|id| self.nodes.get(id));
            let entry = Entry {
                name,
                kind,
                size: match (kind, live) {
                    (Kind::Folder, _) => 0,
                    (_, Some(n)) => u64::from(n.size),
                    _ => u64::from(item.size),
                },
                modified: live.map_or(item.modified, |n| n.modified),
                hidden: item.hidden(),
            };
            if !out(entry) {
                return Ok(u64::from(item.first_slot) + 1);
            }
        }
        Ok(0)
    }

    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError> {
        self.writable()?;
        self.folder_first(folder)?;
        if !valid_name(name) {
            return Err(FsError::BadName);
        }
        let result = self.create_inner(folder, name, kind);
        self.track(result)
    }

    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError> {
        self.writable()?;
        self.folder_first(folder)?;
        let result = self.remove_inner(folder, name);
        self.track(result)
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
        self.folder_first(from)?;
        self.folder_first(to)?;
        if !valid_name(to_name) {
            return Err(FsError::BadName);
        }
        let result = self.rename_inner(from, from_name, to, to_name, replace);
        self.track(result)
    }

    fn space(&mut self) -> Result<Space, FsError> {
        let cluster_bytes = u64::from(self.vol.geo.cluster_bytes);
        Ok(Space {
            total: u64::from(self.vol.geo.clusters) * cluster_bytes,
            free: u64::from(self.vol.free) * cluster_bytes,
            label: self.info().label,
            filesystem: "FAT32",
        })
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        if self.read_only {
            return Ok(());
        }
        if self.poisoned {
            return Err(FsError::Disk);
        }
        // If a commit fails the volume is left marked "in use", as after a crash.
        let pending: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| n.dirty || n.trim).map(|(&id, _)| id).collect();
        let mut first_error = Ok(());
        for id in pending {
            if let Err(e) = self.file_commit(id)
                && first_error.is_ok()
            {
                first_error = Err(e);
            }
        }
        first_error?;
        // Everything else is on the disk before the volume says it was shut down cleanly.
        let result =
            self.write_fsinfo().and_then(|()| self.vol.flush()).and_then(|()| self.mark_clean());
        self.track(result)?;
        self.read_only = true;
        Ok(())
    }
}
