// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A folder held in memory.
//!
//! A folder is a cluster chain full of 32-byte entries (`dirent.rs`). FAT limits a folder to
//! 65536 entries (2 MiB), so a whole folder can be read into memory and worked on there: no
//! allocation here is bigger than that, however a hostile disk's chain is built, because a
//! longer chain is refused. Changes are made in the copy in memory and written back entry by
//! entry.

use crate::dirent::{ENTRY, Item, Scan, Scanner};
use crate::volume::Volume;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// The most entries a folder can have: the limit Windows and the FAT specification set.
pub(crate) const MAX_SLOTS: u32 = 65_536;

/// A folder's clusters and their contents.
pub(crate) struct Folder {
    /// The clusters of the chain, in order.
    pub clusters: Vec<u32>,
    /// All the entries, `clusters.len() * cluster_bytes` bytes.
    pub bytes: Vec<u8>,
}

impl Folder {
    /// How many entry slots there are (used or free).
    pub fn slots(&self) -> u32 {
        (self.bytes.len() / ENTRY) as u32
    }

    /// The entry in `slot`.
    pub fn entry(&self, slot: u32) -> Option<&[u8; ENTRY]> {
        let at = slot as usize * ENTRY;
        self.bytes.get(at..at + ENTRY)?.try_into().ok()
    }

    /// The first slot of the free space at the end of the folder: the slot whose first byte is
    /// 0x00, or the number of slots if there is none.
    pub fn end(&self) -> u32 {
        self.bytes.chunks_exact(ENTRY).position(|e| e[0] == 0x00).map_or(self.slots(), |i| i as u32)
    }

    /// Everything in the folder, in order: the files and folders, and the label if there is one.
    pub fn scan(&self) -> (Vec<Item>, Option<[u8; 11]>) {
        let mut scanner = Scanner::new();
        let (mut items, mut label) = (Vec::new(), None);
        for (slot, e) in self.bytes.chunks_exact(ENTRY).enumerate() {
            let Ok(e) = <&[u8; ENTRY]>::try_from(e) else { break };
            match scanner.feed(slot as u32, e) {
                Scan::End => break,
                Scan::Skip => {}
                Scan::Label(l) => {
                    label.get_or_insert(l);
                }
                Scan::Item(item) => items.push(item),
            }
        }
        (items, label)
    }

    /// The files and folders in it (as [`Folder::scan`]).
    pub fn items(&self) -> Vec<Item> {
        self.scan().0
    }

    /// Whether it holds nothing but `.` and `..` (and deleted entries).
    pub fn is_empty(&self) -> bool {
        self.items().iter().all(|i| i.dots)
    }

    /// Finds `count` free slots in a row for new entries: a run of deleted entries, or the free
    /// space at the end, or a run of deleted entries running into it. The first slot of the run
    /// is returned. `None` if the folder has no room (it then needs a new cluster).
    pub fn find_run(&self, count: u32) -> Option<u32> {
        let end = self.end();
        let (mut start, mut length) = (0, 0);
        for slot in 0..end {
            if self.entry(slot).is_some_and(|e| e[0] == 0xE5) {
                if length == 0 {
                    start = slot;
                }
                length += 1;
                if length == count {
                    return Some(start);
                }
            } else {
                length = 0;
            }
        }
        // The deleted entries just before the end, if any, join the free space after it.
        let run_start = if length > 0 { start } else { end };
        (length + (self.slots() - end) >= count).then_some(run_start)
    }

    /// Puts `entries` into the slots from `slot` on (in memory only). If that uses the free space
    /// at the end of the folder, the slot after them is cleared so the end marker stays right
    /// even if the folder had rubbish after its old end (nothing after the first 0x00 counts,
    /// so rubbish there would come alive). Returns how many slots were changed, which is the
    /// number of entries, plus one if a slot was cleared.
    pub fn place(&mut self, slot: u32, entries: &[[u8; ENTRY]]) -> Result<u32, FsError> {
        let old_end = self.end();
        let after = slot + entries.len() as u32;
        if after > self.slots() {
            return Err(FsError::Damaged);
        }
        for (i, e) in entries.iter().enumerate() {
            let at = (slot as usize + i) * ENTRY;
            self.bytes[at..at + ENTRY].copy_from_slice(e);
        }
        let mut changed = entries.len() as u32;
        if after > old_end && after < self.slots() {
            let at = after as usize * ENTRY;
            self.bytes[at..at + ENTRY].fill(0);
            changed += 1;
        }
        Ok(changed)
    }

    /// Marks the entries in slots `first..=last` deleted (in memory only).
    pub fn delete(&mut self, first: u32, last: u32) -> Result<(), FsError> {
        for slot in first..=last {
            let at = slot as usize * ENTRY;
            *self.bytes.get_mut(at).ok_or(FsError::Damaged)? = 0xE5;
        }
        Ok(())
    }
}

impl<D: BlockDevice> Volume<D> {
    /// Reads the folder whose first cluster is `first`. A chain that loops, leaves the volume,
    /// or is longer than a folder can be is [`FsError::Damaged`].
    pub fn load_folder(&mut self, first: u32) -> Result<Folder, FsError> {
        let cluster_bytes = self.geo.cluster_bytes as usize;
        let most = (MAX_SLOTS as usize * ENTRY).div_ceil(cluster_bytes);
        let mut clusters = Vec::new();
        let mut cluster = first;
        loop {
            if clusters.len() >= most {
                return Err(FsError::Damaged);
            }
            clusters.push(cluster);
            match self.follow(cluster)? {
                Some(next) => cluster = next,
                None => break,
            }
        }
        let mut bytes = vec![0u8; clusters.len() * cluster_bytes];
        // Read neighbouring clusters in one request.
        let mut i = 0;
        while i < clusters.len() {
            let mut n = 1;
            while i + n < clusters.len()
                && clusters[i + n] == clusters[i] + n as u32
                && (n + 1) * cluster_bytes <= 65_536
            {
                n += 1;
            }
            let at = self.geo.cluster_offset(clusters[i]).ok_or(FsError::Damaged)?;
            self.read_bytes(at, &mut bytes[i * cluster_bytes..(i + n) * cluster_bytes])?;
            i += n;
        }
        Ok(Folder { clusters, bytes })
    }

    /// Writes the entries in slots `from..from + count` of `folder` to the disk.
    pub fn write_slots(&mut self, folder: &Folder, from: u32, count: u32) -> Result<(), FsError> {
        let per_cluster = self.geo.cluster_bytes / ENTRY as u32;
        let mut slot = from;
        let end = from.checked_add(count).ok_or(FsError::Damaged)?;
        while slot < end {
            let within = slot % per_cluster;
            let n = (per_cluster - within).min(end - slot);
            let cluster =
                *folder.clusters.get((slot / per_cluster) as usize).ok_or(FsError::Damaged)?;
            let at = self.geo.cluster_offset(cluster).ok_or(FsError::Damaged)?
                + u64::from(within) * ENTRY as u64;
            let bytes = folder
                .bytes
                .get(slot as usize * ENTRY..(slot + n) as usize * ENTRY)
                .ok_or(FsError::Damaged)?;
            self.write_bytes(at, bytes)?;
            slot += n;
        }
        Ok(())
    }

    /// Adds a zeroed cluster to the end of `folder`, on the disk and in memory. A folder that is
    /// already as big as FAT allows is [`FsError::NoSpace`].
    pub fn grow_folder(&mut self, folder: &mut Folder) -> Result<(), FsError> {
        let cluster_bytes = self.geo.cluster_bytes as usize;
        if folder.slots() as usize + cluster_bytes / ENTRY > MAX_SLOTS as usize {
            return Err(FsError::NoSpace);
        }
        let last = *folder.clusters.last().ok_or(FsError::Damaged)?;
        let cluster = self.alloc_zeroed(Some(last))?;
        folder.clusters.push(cluster);
        folder.bytes.resize(folder.bytes.len() + cluster_bytes, 0);
        Ok(())
    }

    /// The volume label in the root folder, if it has one.
    pub fn root_label(&mut self) -> Result<Option<[u8; 11]>, FsError> {
        let root = self.geo.root;
        let folder = self.load_folder(root)?;
        Ok(folder.scan().1)
    }
}
