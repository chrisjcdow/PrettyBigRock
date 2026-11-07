// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What happens to a file's contents: reading, writing, changing its size, and making the
//! changes durable (note 0014, section 6; note 0016, section 3).
//!
//! RockFS writes a changed file to new blocks and switches to them in one write, so a crash
//! leaves the file as it was. FAT cannot: a file is its folder entry (first cluster and size)
//! plus a chain in the FAT, and the data sits in the clusters themselves, so overwriting part
//! of a file overwrites it in place, and a crash then leaves new data in some places and old in
//! others. Nothing cheaper exists. What we do is order the steps that *can* be ordered so that
//! a crash does the least harm:
//!
//! - **growing** takes new clusters and links them into the FAT first, then writes the data
//!   into them. Nothing on the disk refers to the new size yet, so a crash before the commit
//!   leaves at worst some space marked used that belongs to nothing;
//! - **commit** flushes (so the data and the FAT are on the disk), then updates the folder
//!   entry, then flushes (the change has now happened), and only then frees clusters that a
//!   cut released. A crash before the entry update shows the file as it was; after, as it is.
//!   The freeing comes last, and the chain is cut off before the clusters are freed, so a crash
//!   can lose space but never leave a file pointing into free clusters (which another file
//!   could then take);
//! - **cutting** a file only changes it in memory until the commit. The released clusters
//!   stay in its chain, are reused if the file grows again, and are freed by the commit.
//!
//! A *flush* is a request to the disk to finish writing everything given to it so far, before
//! anything given after (the disk and the cache above it may otherwise reorder writes).

use crate::chain::Reach;
use crate::dirent::{ATTR_DIR, ENTRY, set_cluster, touch};
use crate::mount::{Fat, Node};
use crate::table::END;
use crate::volume::Volume;
use pbr_fs::{BlockDevice, FsError, Kind, NodeId};

/// The most bytes one read or write may move (the limit of a request to the filesystem).
pub(crate) const MAX_TRANSFER: usize = 65_536;

/// Checks, once, that `node`'s chain is a proper one (see [`Node::verified`]). A chain longer
/// than the size needs (a disk written by another system, or by a crash) is noted for trimming:
/// the surplus is freed at the next commit, like the clusters a cut leaves behind.
fn verify<D: BlockDevice>(vol: &mut Volume<D>, node: &mut Node) -> Result<(), FsError> {
    if node.verified {
        return Ok(());
    }
    let count = vol.check_chain(node.chain.first)?;
    node.verified = true;
    let cluster_bytes = u64::from(vol.geo.cluster_bytes);
    if u64::from(count) > u64::from(node.size).div_ceil(cluster_bytes) {
        node.trim = true;
    }
    Ok(())
}

impl<D: BlockDevice> Fat<D> {
    /// The byte address, in the volume, of the entry in `slot` of the folder `parent`.
    pub(crate) fn entry_addr(&mut self, parent: NodeId, slot: u32) -> Result<u64, FsError> {
        let cluster_bytes = u64::from(self.vol.geo.cluster_bytes);
        let offset = u64::from(slot) * ENTRY as u64;
        let index = u32::try_from(offset / cluster_bytes).map_err(|_| FsError::Damaged)?;
        let folder = self.nodes.get_mut(&parent).ok_or(FsError::Damaged)?;
        let Reach::At(cluster) = self.vol.cluster_at(&mut folder.chain, index)? else {
            return Err(FsError::Damaged);
        };
        Ok(self.vol.geo.cluster_offset(cluster).ok_or(FsError::Damaged)? + offset % cluster_bytes)
    }

    /// Reads from a file (see [`pbr_fs::Filesystem::read`]).
    pub(crate) fn file_read(
        &mut self,
        id: NodeId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if buf.len() > MAX_TRANSFER {
            return Err(FsError::BadArgument);
        }
        let node = self.nodes.get_mut(&id).ok_or(FsError::Gone)?;
        if node.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        let size = u64::from(node.size);
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        verify(&mut self.vol, node)?;
        let len = (size - offset).min(buf.len() as u64) as usize;
        self.vol.read_chain(&mut node.chain, offset, &mut buf[..len])?;
        Ok(len)
    }

    /// Writes to a file (see [`pbr_fs::Filesystem::write`]).
    pub(crate) fn file_write(
        &mut self,
        id: NodeId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, FsError> {
        self.writable()?;
        if data.len() > MAX_TRANSFER {
            return Err(FsError::BadArgument);
        }
        let now = (self.clock)();
        let node = self.nodes.get_mut(&id).ok_or(FsError::Gone)?;
        if node.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        // A file's size is a 32-bit number in its entry: the most it can be is 4 GiB - 1.
        let end = offset
            .checked_add(data.len() as u64)
            .filter(|&e| e <= u64::from(u32::MAX))
            .ok_or(FsError::TooBig)?;
        if data.is_empty() {
            return Ok(0);
        }
        verify(&mut self.vol, node)?;
        let cluster_bytes = u64::from(self.vol.geo.cluster_bytes);
        let size = u64::from(node.size);
        let needed = u32::try_from(end.div_ceil(cluster_bytes)).map_err(|_| FsError::TooBig)?;
        let has = u32::try_from(size.div_ceil(cluster_bytes)).map_err(|_| FsError::TooBig)?;
        // New clusters hold whatever was in them before, so every byte of them that will be
        // read must be written: the gap before `offset`, if there is one, is zeroed, and the
        // data covers the rest up to `end`. Bytes beyond `end` in the last cluster are beyond
        // the file's end, and are zeroed later if the file grows over them.
        //
        // If something fails after clusters were added, they are not counted in the size, so
        // the file is marked as possibly too long and the commit frees them.
        let was_trim = node.trim;
        if needed > has {
            self.vol.grow(&mut node.chain, needed)?;
            node.trim = true;
        }
        if offset > size {
            self.vol.zero_chain(&mut node.chain, size, offset)?;
        }
        self.vol.write_chain(&mut node.chain, offset, data)?;
        node.trim = was_trim;
        node.size = size.max(end) as u32;
        node.modified = now;
        node.dirty = true;
        Ok(data.len())
    }

    /// Makes a file `new_size` long (see [`pbr_fs::Filesystem::set_size`]).
    pub(crate) fn file_set_size(&mut self, id: NodeId, new_size: u64) -> Result<(), FsError> {
        self.writable()?;
        let now = (self.clock)();
        let node = self.nodes.get_mut(&id).ok_or(FsError::Gone)?;
        if node.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        let new = u32::try_from(new_size).map_err(|_| FsError::TooBig)?;
        let size = node.size;
        if new == size {
            return Ok(());
        }
        verify(&mut self.vol, node)?;
        if new > size {
            let cluster_bytes = u64::from(self.vol.geo.cluster_bytes);
            let needed = u32::try_from(u64::from(new).div_ceil(cluster_bytes))
                .map_err(|_| FsError::TooBig)?;
            let was_trim = node.trim;
            self.vol.grow(&mut node.chain, needed)?;
            node.trim = true;
            // What was past the old end (in the last cluster, or in clusters a cut left in the
            // chain) is stale and must read as zeros.
            self.vol.zero_chain(&mut node.chain, u64::from(size), u64::from(new))?;
            node.trim = was_trim;
        } else {
            // Only in memory: the clusters stay in the chain until the commit.
            node.trim = true;
            node.chain.forget();
        }
        node.size = new;
        node.modified = now;
        node.dirty = true;
        Ok(())
    }

    /// Makes a file's changes durable (see [`pbr_fs::Filesystem::commit`]).
    pub(crate) fn file_commit(&mut self, id: NodeId) -> Result<(), FsError> {
        let node = self.nodes.get(&id).ok_or(FsError::Gone)?;
        if node.kind != Kind::File || !(node.dirty || node.trim) {
            return Ok(());
        }
        // On a read-only mount nothing was changed, but a file read there may have been noted
        // as having a surplus (see `verify`); there is nothing to free it with.
        if self.read_only {
            return Ok(());
        }
        self.writable()?;
        let result = self.commit_steps(id);
        self.track(result)
    }

    /// The steps of a commit (see the top of this file for why they are in this order).
    fn commit_steps(&mut self, id: NodeId) -> Result<(), FsError> {
        let node = self.nodes.get(&id).ok_or(FsError::Gone)?;
        let (parent, slot, size, mut chain, trim, modified) =
            (node.parent, node.slot, node.size, node.chain, node.trim, node.modified);
        // 1. Everything the entry is about to point at is on the disk.
        self.vol.flush()?;

        // 2. Find what a cut released, and check it is a proper chain before anything changes
        // (so that finding it damaged leaves everything as it was).
        let mut release: Option<(Option<u32>, u32)> = None; // (cluster to end the chain at, first to free)
        let mut first = chain.first;
        if size == 0 {
            if first != 0 {
                self.vol.check_chain(first)?;
                release = Some((None, first));
            }
            first = 0;
        } else if trim {
            let cluster_bytes = u64::from(self.vol.geo.cluster_bytes);
            let needed = u32::try_from(u64::from(size).div_ceil(cluster_bytes))
                .map_err(|_| FsError::Damaged)?;
            let Reach::At(last) = self.vol.cluster_at(&mut chain, needed - 1)? else {
                return Err(FsError::Damaged); // shorter than the size needs
            };
            if let Some(extra) = self.vol.follow(last)? {
                self.vol.check_chain(extra)?;
                release = Some((Some(last), extra));
            }
        }

        // 3. Update the entry: this is the moment the change takes effect.
        let addr = self.entry_addr(parent, slot)?;
        let mut entry = [0u8; ENTRY];
        self.vol.read_bytes(addr, &mut entry)?;
        // The entry must still be the file's own: a live short entry, not a folder.
        if entry[0] == 0x00
            || entry[0] == 0xE5
            || entry[11] & 0x3F == 0x0F
            || entry[11] & ATTR_DIR != 0
        {
            return Err(FsError::Damaged);
        }
        set_cluster(&mut entry, first);
        entry[28..32].copy_from_slice(&size.to_le_bytes());
        touch(&mut entry, modified, true);
        self.vol.write_bytes(addr, &entry)?;
        // 4. The change is now on the disk.
        self.vol.flush()?;

        // 5. Give back what the cut released: first end the chain, then free the rest.
        if let Some((cut, extra)) = release {
            if let Some(last) = cut {
                self.vol.table.set(&mut self.vol.dev, last, END)?;
                self.vol.flush()?;
            }
            self.vol.free_chain(extra)?;
            self.vol.flush()?;
        }

        let node = self.nodes.get_mut(&id).ok_or(FsError::Gone)?;
        node.chain.first = first;
        node.chain.forget();
        node.dirty = false;
        node.trim = false;
        Ok(())
    }
}
