// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The volume as a whole: reading and writing bytes anywhere on it, and handing out clusters.
//!
//! `Volume` is what every other module uses to reach the disk. It owns the device, the checked
//! boot sector numbers and the cached FAT, and keeps the count of free clusters. The chain,
//! folder and file code (`chain.rs`, `folder.rs`, `file.rs`) is built on it.

use crate::boot::Geometry;
use crate::table::{END, FatTable, Link, classify};
use alloc::vec;
use pbr_fs::{BlockDevice, FsError, SECTOR};

/// The most bytes a device accepts in one request.
const MAX_TRANSFER: usize = 65_536;

/// A mounted volume's disk, geometry and FAT.
///
/// `D` is a *type parameter*: this works with any type that implements the `BlockDevice` trait
/// (Rust's word for an interface), such as a real disk in the FAT server or a disk image in
/// memory in a test. Rust makes a separate, fully optimised copy of this code for each `D` it
/// is used with, so the flexibility costs nothing at run time.
pub(crate) struct Volume<D: BlockDevice> {
    /// The disk.
    pub dev: D,
    /// The boot sector's numbers, checked.
    pub geo: Geometry,
    /// The FAT, cached.
    pub table: FatTable,
    /// How many clusters are free. Kept up to date as clusters come and go; it starts from the
    /// FSInfo hint or a count, so after a hostile or damaged disk it can be wrong, which only
    /// makes the free-space figure wrong (allocation looks at the FAT itself).
    pub free: u32,
    /// Where to start looking for a free cluster.
    pub next_free: u32,
}

impl<D: BlockDevice> Volume<D> {
    /// Reads `buf.len()` bytes starting at byte `addr` of the volume. The disk only reads whole
    /// sectors, so a piece that starts or ends inside a sector is read through a one-sector
    /// buffer.
    pub fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let mut addr = addr;
        let mut rest = buf;
        while !rest.is_empty() {
            let skip = (addr % SECTOR as u64) as usize;
            if skip == 0 && rest.len() >= SECTOR {
                let whole = (rest.len() / SECTOR * SECTOR).min(MAX_TRANSFER);
                let (head, tail) = rest.split_at_mut(whole);
                self.dev.read(addr / SECTOR as u64, head)?;
                addr += whole as u64;
                rest = tail;
            } else {
                let mut sector = [0u8; SECTOR];
                self.dev.read(addr / SECTOR as u64, &mut sector)?;
                let n = (SECTOR - skip).min(rest.len());
                let (head, tail) = rest.split_at_mut(n);
                head.copy_from_slice(&sector[skip..skip + n]);
                addr += n as u64;
                rest = tail;
            }
        }
        Ok(())
    }

    /// Writes `data` starting at byte `addr` of the volume. A piece that starts or ends inside
    /// a sector reads the sector first and writes it back with the new bytes in place
    /// (*read-modify-write*), since the disk writes only whole sectors.
    pub fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), FsError> {
        let mut addr = addr;
        let mut rest = data;
        while !rest.is_empty() {
            let skip = (addr % SECTOR as u64) as usize;
            if skip == 0 && rest.len() >= SECTOR {
                let whole = (rest.len() / SECTOR * SECTOR).min(MAX_TRANSFER);
                let (head, tail) = rest.split_at(whole);
                self.dev.write(addr / SECTOR as u64, head)?;
                addr += whole as u64;
                rest = tail;
            } else {
                let mut sector = [0u8; SECTOR];
                self.dev.read(addr / SECTOR as u64, &mut sector)?;
                let n = (SECTOR - skip).min(rest.len());
                let (head, tail) = rest.split_at(n);
                sector[skip..skip + n].copy_from_slice(head);
                self.dev.write(addr / SECTOR as u64, &sector)?;
                addr += n as u64;
                rest = tail;
            }
        }
        Ok(())
    }

    /// Makes everything written so far durable: first the changed FAT sectors, then a flush of
    /// the device (the point after which the disk has everything written before it, note 0015).
    pub fn flush(&mut self) -> Result<(), FsError> {
        self.table.flush_dirty(&mut self.dev)?;
        self.dev.flush()?;
        Ok(())
    }

    /// The cluster after `cluster` in its chain: `Some(next)`, or `None` at the end of the
    /// chain. A free or bad entry, or one pointing outside the volume, in the middle of a
    /// chain is [`FsError::Damaged`], as is a `cluster` that is not a data cluster.
    pub fn follow(&mut self, cluster: u32) -> Result<Option<u32>, FsError> {
        if !self.geo.is_data_cluster(cluster) {
            return Err(FsError::Damaged);
        }
        let raw = self.table.get(&mut self.dev, cluster)?;
        match classify(raw, self.geo.last_cluster()) {
            Link::Next(next) => Ok(Some(next)),
            Link::End => Ok(None),
            Link::Free | Link::Bad => Err(FsError::Damaged),
        }
    }

    /// Takes a free cluster, marks it as the end of a chain and, if `prev` is given, links it
    /// after `prev`. A cluster right after `prev` is preferred so that files stay in one piece,
    /// which makes them faster to read. [`FsError::NoSpace`] if there is none. The new
    /// cluster's contents are whatever was there before.
    pub fn alloc_cluster(&mut self, prev: Option<u32>) -> Result<u32, FsError> {
        let beside = prev
            .and_then(|p| p.checked_add(1))
            .filter(|&n| self.geo.is_data_cluster(n))
            .filter(|&n| matches!(self.table.get(&mut self.dev, n), Ok(0)));
        let cluster = match beside {
            Some(n) => n,
            None => self.table.find_free(&mut self.dev, self.next_free)?.ok_or(FsError::NoSpace)?,
        };
        self.table.set(&mut self.dev, cluster, END)?;
        if let Some(p) = prev {
            self.table.set(&mut self.dev, p, cluster)?;
        }
        self.next_free = if cluster >= self.geo.last_cluster() { 2 } else { cluster + 1 };
        self.free = self.free.saturating_sub(1);
        Ok(cluster)
    }

    /// Writes zeros over the whole of `cluster`.
    pub fn zero_cluster(&mut self, cluster: u32) -> Result<(), FsError> {
        let at = self.geo.cluster_offset(cluster).ok_or(FsError::Damaged)?;
        let zeros = vec![0u8; self.geo.cluster_bytes as usize];
        self.write_bytes(at, &zeros)
    }

    /// Like [`Volume::alloc_cluster`], and the new cluster is zeroed (a folder's clusters must
    /// start empty).
    pub fn alloc_zeroed(&mut self, prev: Option<u32>) -> Result<u32, FsError> {
        let cluster = self.alloc_cluster(prev)?;
        self.zero_cluster(cluster)?;
        Ok(cluster)
    }

    /// Frees the chain starting at `first`: every cluster in it is marked free. Stops with
    /// [`FsError::Damaged`] if the chain is not a proper one (see [`Volume::check_chain`], which
    /// callers use first so that nothing is half done).
    pub fn free_chain(&mut self, first: u32) -> Result<(), FsError> {
        let mut cluster = first;
        for _ in 0..=self.geo.clusters {
            // Read the link before erasing it.
            let next = self.follow(cluster)?;
            self.table.set(&mut self.dev, cluster, 0)?;
            self.free = self.free.saturating_add(1).min(self.geo.clusters);
            self.next_free = self.next_free.min(cluster);
            match next {
                Some(n) => cluster = n,
                None => return Ok(()),
            }
        }
        Err(FsError::Damaged)
    }

    /// Undoes an allocation that did not work out: the chain that starts at `first_new` was
    /// added after `last` (or is the whole chain, if `last` is `None`).
    pub fn release_tail(&mut self, last: Option<u32>, first_new: u32) -> Result<(), FsError> {
        if let Some(l) = last {
            self.table.set(&mut self.dev, l, END)?;
        }
        self.free_chain(first_new)
    }
}
