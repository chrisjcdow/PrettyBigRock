// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The shared block cache (docs/design/0015-storage-and-the-block-cache.md, section 5).
//!
//! Disks are slow: reading from one takes thousands of times longer than reading memory. A
//! **block cache** keeps recently used parts of disks in memory, so reading them again is
//! quick, and lets writes return as soon as they're in memory ("write-back"), to reach the
//! disk later. One cache serves every disk and every filesystem, so memory goes where it's
//! used.
//!
//! The cache works in **pages** of 4 KiB: 8 sectors of 512 bytes, aligned to 4 KiB from the
//! start of the disk. When a page is needed and the cache is full, the page used least
//! recently is replaced; if it was changed ("dirty"), it's written to the disk first.
//!
//! **The ordering rules.** Write-back means writes can reach the disk in a different order from
//! the one they were made in. Filesystems that care about order (RockFS's copy-on-write saving
//! does) need a way to say "everything before this point must be on the disk before anything
//! after it". That's `flush`. For each volume (a partition, or a whole disk) the cache behaves
//! exactly like a disk with a write cache of its own:
//! 1. a read returns the data most recently written;
//! 2. writes may reach the disk in any order, at any time before the next flush;
//! 3. `flush` returns only when every write to the volume made before it was called is on the
//!    disk.
//!
//! This library is only the logic. The storage service supplies the disks through the
//! [`Backend`] trait; the tests supply a simulated disk with a write cache that can lose
//! power. It is single-threaded: the storage service handles one request at a time.

#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(test)]
mod tests;

/// Bytes in a sector.
pub const SECTOR: usize = 512;
/// Bytes in a page.
pub const PAGE: usize = 4096;
/// Sectors in a page.
pub const PAGE_SECTORS: u64 = (PAGE / SECTOR) as u64;
/// The most bytes one call to a disk moves (`pbr.block`'s limit).
pub const MAX_IO: usize = 65536;
/// The most bytes one read or write asks of the cache.
pub const MAX_REQUEST: usize = 65536;

/// The disks behind the cache.
pub trait Backend {
    /// Reads whole sectors of `disk` from `sector` into `buf` (at most [`MAX_IO`] bytes).
    fn read(&mut self, disk: u32, sector: u64, buf: &mut [u8]) -> Result<(), IoError>;
    /// Writes whole sectors (at most [`MAX_IO`] bytes).
    fn write(&mut self, disk: u32, sector: u64, data: &[u8]) -> Result<(), IoError>;
    /// Returns once everything written to `disk` so far is on it.
    fn flush(&mut self, disk: u32) -> Result<(), IoError>;
}

/// A disk failed a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// Why a request to the cache failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such volume or disk.
    NoSuchVolume,
    /// The sectors aren't all inside the volume.
    OutOfRange,
    /// Not a whole number of sectors, empty, or bigger than [`MAX_REQUEST`].
    BadArgument,
    /// The disk failed: now, or (for `flush`) while writing a page back earlier.
    Disk,
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Error {
        Error::Disk
    }
}

/// "No slot": the end of the replacement list.
const NIL: usize = usize::MAX;

/// One page's place in the cache.
#[derive(Clone, Copy)]
struct Slot {
    /// The (disk, page number) it holds, if any.
    key: Option<(u32, u64)>,
    /// Changed since it was read or last written back.
    dirty: bool,
    /// When it was first changed (for writing back pages that have been dirty a long time).
    dirty_since: u64,
    /// The volume that first changed it, which is charged for it (section 5, "Fairness").
    owner: u32,
    /// Its neighbours in the replacement list: `prev` is more recently used, `next` less.
    prev: usize,
    next: usize,
}

/// A volume: a run of sectors on one disk.
#[derive(Clone, Copy)]
struct Volume {
    disk: u32,
    start: u64,
    sectors: u64,
    /// Pages it has dirtied that aren't written back yet.
    dirty: usize,
    /// A write-back failed since its last flush; the next flush reports it.
    failed: bool,
}

/// The cache.
pub struct Cache {
    /// Every page's bytes, one after another.
    data: Vec<u8>,
    slots: Vec<Slot>,
    /// (disk, page number) → slot. A `BTreeMap` is a sorted map: finding a key takes a few
    /// dozen comparisons however big it is.
    map: BTreeMap<(u32, u64), usize>,
    /// The replacement list: `head` the most recently used slot, `tail` the least.
    head: usize,
    tail: usize,
    /// Slots holding nothing.
    free: Vec<usize>,
    disks: BTreeMap<u32, u64>,
    volumes: BTreeMap<u32, Volume>,
    /// The most pages one volume may have dirty.
    dirty_limit: usize,
    /// A scratch buffer for reading several pages at once.
    scratch: Vec<u8>,
}

impl Cache {
    /// A cache of `pages` pages (at least 32), using `pages * 4 KiB` of memory.
    pub fn new(pages: usize) -> Cache {
        let pages = pages.max(32);
        let empty =
            Slot { key: None, dirty: false, dirty_since: 0, owner: 0, prev: NIL, next: NIL };
        Cache {
            data: vec![0; pages * PAGE],
            slots: vec![empty; pages],
            map: BTreeMap::new(),
            head: NIL,
            tail: NIL,
            // Reversed, so slot 0 is handed out first (it makes no difference, but reads better
            // when debugging).
            free: (0..pages).rev().collect(),
            disks: BTreeMap::new(),
            volumes: BTreeMap::new(),
            dirty_limit: pages / 4,
            scratch: vec![0; MAX_IO],
        }
    }

    /// How many pages the cache holds.
    pub fn pages(&self) -> usize {
        self.slots.len()
    }

    /// Adds a disk of `sectors` sectors (or changes its size).
    pub fn add_disk(&mut self, disk: u32, sectors: u64) {
        self.disks.insert(disk, sectors);
    }

    /// Adds a volume: `sectors` sectors of `disk` from `start`. Fails if it isn't on the disk.
    pub fn add_volume(
        &mut self,
        id: u32,
        disk: u32,
        start: u64,
        sectors: u64,
    ) -> Result<(), Error> {
        let size = *self.disks.get(&disk).ok_or(Error::NoSuchVolume)?;
        match start.checked_add(sectors) {
            Some(end) if end <= size => {}
            _ => return Err(Error::OutOfRange),
        }
        self.volumes.insert(id, Volume { disk, start, sectors, dirty: 0, failed: false });
        Ok(())
    }

    /// How many pages volume `id` has dirty (for tests and reports).
    pub fn dirty_pages(&self, id: u32) -> usize {
        self.volumes.get(&id).map_or(0, |v| v.dirty)
    }

    /// Checks a request of `len` bytes at the volume's `sector`, and returns the volume and the
    /// request's first sector on its disk.
    fn place(&self, id: u32, sector: u64, len: usize) -> Result<(Volume, u64), Error> {
        let v = *self.volumes.get(&id).ok_or(Error::NoSuchVolume)?;
        if len == 0 || !len.is_multiple_of(SECTOR) || len > MAX_REQUEST {
            return Err(Error::BadArgument);
        }
        let count = (len / SECTOR) as u64;
        match sector.checked_add(count) {
            Some(end) if end <= v.sectors => Ok((v, v.start + sector)),
            _ => Err(Error::OutOfRange),
        }
    }

    /// How many sectors page `page` of `disk` has (the last page of a disk may be short).
    fn page_sectors(&self, disk: u32, page: u64) -> u64 {
        let size = self.disks.get(&disk).copied().unwrap_or(0);
        size.saturating_sub(page * PAGE_SECTORS).min(PAGE_SECTORS)
    }

    // --- The replacement list -------------------------------------------------------------

    fn unlink(&mut self, s: usize) {
        let Slot { prev, next, .. } = self.slots[s];
        if prev != NIL {
            self.slots[prev].next = next;
        } else {
            self.head = next;
        }
        if next != NIL {
            self.slots[next].prev = prev;
        } else {
            self.tail = prev;
        }
        self.slots[s].prev = NIL;
        self.slots[s].next = NIL;
    }

    fn push_front(&mut self, s: usize) {
        self.slots[s].prev = NIL;
        self.slots[s].next = self.head;
        if self.head != NIL {
            self.slots[self.head].prev = s;
        } else {
            self.tail = s;
        }
        self.head = s;
    }

    /// Marks slot `s` as just used.
    fn touch(&mut self, s: usize) {
        if self.head != s {
            self.unlink(s);
            self.push_front(s);
        }
    }

    // --- Pages ---------------------------------------------------------------------------

    fn bytes(&self, s: usize) -> &[u8] {
        &self.data[s * PAGE..(s + 1) * PAGE]
    }

    fn bytes_mut(&mut self, s: usize) -> &mut [u8] {
        &mut self.data[s * PAGE..(s + 1) * PAGE]
    }

    /// Writes slot `s` back to its disk, if it's dirty. A failure is remembered against every
    /// volume the page touches (the page is then treated as clean: its data is lost, and the
    /// next flush of those volumes says so).
    fn write_back(&mut self, io: &mut impl Backend, s: usize) {
        let slot = self.slots[s];
        let Some((disk, page)) = slot.key else { return };
        if !slot.dirty {
            return;
        }
        let n = self.page_sectors(disk, page);
        let result =
            io.write(disk, page * PAGE_SECTORS, &self.data[s * PAGE..][..n as usize * SECTOR]);
        self.slots[s].dirty = false;
        if let Some(v) = self.volumes.get_mut(&slot.owner) {
            v.dirty = v.dirty.saturating_sub(1);
        }
        if result.is_err() {
            let (first, end) = (page * PAGE_SECTORS, page * PAGE_SECTORS + n);
            for v in self.volumes.values_mut() {
                if v.disk == disk && v.start < end && first < v.start + v.sectors {
                    v.failed = true;
                }
            }
        }
    }

    /// A free slot, replacing the least recently used page if there's none. Pages in `keep`
    /// (a disk and a range of page numbers, the request being served) are never replaced.
    fn take_slot(&mut self, io: &mut impl Backend, keep: (u32, u64, u64)) -> usize {
        if let Some(s) = self.free.pop() {
            return s;
        }
        let mut s = self.tail;
        while s != NIL {
            let kept = self.slots[s]
                .key
                .is_some_and(|(d, p)| d == keep.0 && (keep.1..keep.2).contains(&p));
            if !kept {
                break;
            }
            s = self.slots[s].prev;
        }
        // The cache holds at least 32 pages and a request covers at most 17, so there's always
        // one that isn't kept.
        assert!(s != NIL, "every page in the cache is in use by one request");
        self.write_back(io, s);
        self.unlink(s);
        if let Some(key) = self.slots[s].key.take() {
            self.map.remove(&key);
        }
        s
    }

    /// Puts the page `(disk, page)` in slot `s`, as most recently used and clean.
    fn install(&mut self, s: usize, disk: u32, page: u64) {
        self.slots[s].key = Some((disk, page));
        self.slots[s].dirty = false;
        self.map.insert((disk, page), s);
        self.push_front(s);
    }

    /// Makes sure pages `first..end` of `disk` are in the cache, reading runs of missing pages
    /// with as few disk calls as possible. Pages in `keep` (the whole request this is part of)
    /// aren't replaced to make room.
    fn load(
        &mut self,
        io: &mut impl Backend,
        disk: u32,
        first: u64,
        end: u64,
        keep: (u32, u64, u64),
    ) -> Result<(), Error> {
        let mut p = first;
        while p < end {
            if let Some(&s) = self.map.get(&(disk, p)) {
                self.touch(s);
                p += 1;
                continue;
            }
            // A run of missing pages, up to one disk call's worth.
            let mut q = p;
            while q < end && q - p < (MAX_IO / PAGE) as u64 && !self.map.contains_key(&(disk, q)) {
                q += 1;
            }
            let sectors: u64 = (p..q).map(|x| self.page_sectors(disk, x)).sum();
            let len = sectors as usize * SECTOR;
            let mut buf = core::mem::take(&mut self.scratch);
            let r = io.read(disk, p * PAGE_SECTORS, &mut buf[..len]);
            if r.is_err() {
                self.scratch = buf;
                return Err(Error::Disk);
            }
            for x in p..q {
                let s = self.take_slot(io, keep);
                let at = ((x - p) * PAGE as u64) as usize;
                let n = self.page_sectors(disk, x) as usize * SECTOR;
                let page = self.bytes_mut(s);
                page[..n].copy_from_slice(&buf[at..at + n]);
                page[n..].fill(0);
                self.install(s, disk, x);
            }
            self.scratch = buf;
            p = q;
        }
        Ok(())
    }

    /// Reads `buf.len()` bytes (whole sectors) of volume `id` from `sector`.
    pub fn read(
        &mut self,
        io: &mut impl Backend,
        id: u32,
        sector: u64,
        buf: &mut [u8],
    ) -> Result<(), Error> {
        let (v, at) = self.place(id, sector, buf.len())?;
        let end = at + (buf.len() / SECTOR) as u64;
        let (first, last) = (at / PAGE_SECTORS, (end - 1) / PAGE_SECTORS + 1);
        self.load(io, v.disk, first, last, (v.disk, first, last))?;
        self.copy(v.disk, at, end, |page, out| out.copy_from_slice(page), buf);
        Ok(())
    }

    /// For each page of the disk sectors `at..end`, calls `f` with the page's part and the
    /// matching part of `buf`. The pages must be loaded.
    fn copy(&mut self, disk: u32, at: u64, end: u64, f: impl Fn(&[u8], &mut [u8]), buf: &mut [u8]) {
        let mut s = at;
        while s < end {
            let page = s / PAGE_SECTORS;
            let stop = ((page + 1) * PAGE_SECTORS).min(end);
            let slot = self.map[&(disk, page)];
            let from = ((s - page * PAGE_SECTORS) as usize) * SECTOR;
            let len = (stop - s) as usize * SECTOR;
            let out = ((s - at) as usize) * SECTOR;
            f(&self.bytes(slot)[from..from + len], &mut buf[out..out + len]);
            s = stop;
        }
    }

    /// Writes `data` (whole sectors) to volume `id` from `sector`. It reaches the disk later
    /// (rule 2); `now` is the time, for [`Cache::write_back_older`].
    pub fn write(
        &mut self,
        io: &mut impl Backend,
        id: u32,
        sector: u64,
        data: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        let (v, at) = self.place(id, sector, data.len())?;
        let end = at + (data.len() / SECTOR) as u64;
        let (first, last) = (at / PAGE_SECTORS, (end - 1) / PAGE_SECTORS + 1);
        // Pages the write covers completely needn't be read first: only the first and the last
        // may be partly covered.
        for page in first..last {
            if self.map.contains_key(&(v.disk, page)) {
                continue;
            }
            let (ps, pe) =
                (page * PAGE_SECTORS, page * PAGE_SECTORS + self.page_sectors(v.disk, page));
            if at <= ps && pe <= end {
                let s = self.take_slot(io, (v.disk, first, last));
                self.bytes_mut(s).fill(0);
                self.install(s, v.disk, page);
            } else {
                self.load(io, v.disk, page, page + 1, (v.disk, first, last))?;
            }
        }
        // Copy the data in, and mark the pages dirty.
        let mut s = at;
        while s < end {
            let page = s / PAGE_SECTORS;
            let stop = ((page + 1) * PAGE_SECTORS).min(end);
            let slot = self.map[&(v.disk, page)];
            let from = ((s - page * PAGE_SECTORS) as usize) * SECTOR;
            let len = (stop - s) as usize * SECTOR;
            let src = ((s - at) as usize) * SECTOR;
            self.bytes_mut(slot)[from..from + len].copy_from_slice(&data[src..src + len]);
            if !self.slots[slot].dirty {
                self.slots[slot].dirty = true;
                self.slots[slot].dirty_since = now;
                self.slots[slot].owner = id;
                if let Some(vol) = self.volumes.get_mut(&id) {
                    vol.dirty += 1;
                }
            }
            self.touch(slot);
            s = stop;
        }
        // Fairness: a volume over its share of dirty pages writes its oldest back now.
        while self.dirty_pages(id) > self.dirty_limit {
            let oldest = self.oldest_dirty(|slot| slot.owner == id);
            match oldest {
                Some(s) => self.write_back(io, s),
                None => break,
            }
        }
        Ok(())
    }

    /// The dirty slot that has been dirty longest, among those `which` accepts.
    fn oldest_dirty(&self, which: impl Fn(&Slot) -> bool) -> Option<usize> {
        (0..self.slots.len())
            .filter(|&s| self.slots[s].dirty && which(&self.slots[s]))
            .min_by_key(|&s| self.slots[s].dirty_since)
    }

    /// Rule 3: returns once every write to volume `id` made so far is on the disk. Fails if a
    /// page touching the volume failed to be written back at any time since its last flush.
    pub fn flush(&mut self, io: &mut impl Backend, id: u32) -> Result<(), Error> {
        let v = *self.volumes.get(&id).ok_or(Error::NoSuchVolume)?;
        // Every dirty page touching the volume, whoever dirtied it (a page can straddle two
        // partitions that don't start on a 4 KiB boundary), in disk order.
        let (first, end) = (v.start / PAGE_SECTORS, (v.start + v.sectors).div_ceil(PAGE_SECTORS));
        let pages: Vec<usize> =
            self.map.range((v.disk, first)..(v.disk, end)).map(|(_, &s)| s).collect();
        for s in pages {
            self.write_back(io, s);
        }
        let flushed = io.flush(v.disk);
        let vol = self.volumes.get_mut(&id).ok_or(Error::NoSuchVolume)?;
        let failed = core::mem::take(&mut vol.failed);
        if failed || flushed.is_err() { Err(Error::Disk) } else { Ok(()) }
    }

    /// Writes back (without flushing) every page dirty since before `before`: called now and
    /// then, so changes reach the disk even when nobody flushes.
    pub fn write_back_older(&mut self, io: &mut impl Backend, before: u64) {
        while let Some(s) = self.oldest_dirty(|slot| slot.dirty_since < before) {
            self.write_back(io, s);
        }
    }

    /// Writes back everything and flushes every disk: for shutting down. Fails if anything
    /// failed.
    pub fn flush_all(&mut self, io: &mut impl Backend) -> Result<(), Error> {
        let ids: Vec<u32> = self.volumes.keys().copied().collect();
        let mut result = Ok(());
        for id in ids {
            if self.flush(io, id).is_err() {
                result = Err(Error::Disk);
            }
        }
        result
    }
}
