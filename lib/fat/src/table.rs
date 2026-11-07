// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The File Allocation Table: which cluster follows which.
//!
//! The FAT is an array with one 32-bit entry per cluster (entry 0 and 1 are special). The entry
//! for cluster `n` says what comes after `n` in its file or folder: another cluster's number, or
//! "end of chain", or 0 if `n` is free. Only the low 28 bits count; the top four are reserved
//! and must be left as they are. Since the table is how everything is found, the volume keeps
//! two copies (usually) and we write both.
//!
//! Reading the table one 4-byte entry at a time from the disk would be far too slow, so this
//! module keeps a **cache** of recently used FAT sectors in memory: a *cache* is a small,
//! quickly reachable copy of data kept to avoid going back to the slow place it came from (note
//! 0015 explains the block cache underneath, which is a second, shared one). A changed sector
//! stays in memory, marked *dirty*, until [`FatTable::flush_dirty`] writes it to the disk; the
//! filesystem calls that before every device flush, so changes reach the disk in an order it
//! chooses and not an accidental one.

use crate::boot::Geometry;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError, SECTOR};

/// The 28 bits of a FAT entry that count.
pub(crate) const ENTRY_MASK: u32 = 0x0FFF_FFFF;
/// What we write to mean "end of chain" (any value from 0x0FFFFFF8 up means that).
pub(crate) const END: u32 = 0x0FFF_FFFF;
/// Bit 27 of FAT entry 1: set when the volume was cleanly unmounted (note 0016, section 3).
pub(crate) const CLEAN_BIT: u32 = 0x0800_0000;

/// FAT entries in a sector.
const PER_SECTOR: u32 = (SECTOR / 4) as u32;
/// Sectors read at a time when one is missing from the cache (a small read-ahead, since
/// scanning for a free cluster, the common case, touches sector after sector).
const READ_AHEAD: u32 = 16;
/// The most sectors the cache keeps (64 KiB: 16384 clusters' worth of entries).
const CACHE_SECTORS: usize = 128;
/// The most sectors written to the disk in one request (the device's limit is 64 KiB).
const WRITE_SECTORS: usize = 128;

/// What a FAT entry says about its cluster's successor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    /// The cluster is free (entry 0).
    Free,
    /// The chain goes on with this cluster.
    Next(u32),
    /// The chain ends here.
    End,
    /// Anything else: the "bad cluster" mark, a reserved value, or a cluster that is not in
    /// the volume. In the middle of a chain all of these mean the volume is damaged.
    Bad,
}

/// Reads a raw FAT entry for a volume whose last cluster is `last_cluster`.
pub(crate) fn classify(entry: u32, last_cluster: u32) -> Link {
    match entry & ENTRY_MASK {
        0 => Link::Free,
        n if (2..=last_cluster).contains(&n) => Link::Next(n),
        0x0FFF_FFF8..=0x0FFF_FFFF => Link::End,
        _ => Link::Bad,
    }
}

/// One cached sector of the FAT.
struct Cached {
    data: [u8; SECTOR],
    dirty: bool,
}

/// The FAT, cached.
pub(crate) struct FatTable {
    /// The first sector of the first copy.
    first_sector: u64,
    /// Sectors in one copy.
    fat_sectors: u32,
    /// How many copies are written.
    copies: u32,
    /// Entries, including the two reserved ones: one more than the highest cluster number.
    entries: u32,
    /// Cached sectors by their number within a copy.
    cache: BTreeMap<u32, Cached>,
}

impl FatTable {
    /// The table of a volume.
    pub fn new(geo: &Geometry) -> FatTable {
        FatTable {
            first_sector: u64::from(geo.reserved),
            fat_sectors: geo.fat_sectors,
            copies: if geo.mirror { geo.fats } else { 1 },
            entries: geo.clusters + 2,
            cache: BTreeMap::new(),
        }
    }

    /// Writes one run of whole FAT sectors, starting at sector `start` of the copy, to every
    /// copy.
    fn write_run<D: BlockDevice>(
        &self,
        dev: &mut D,
        start: u32,
        data: &[u8],
    ) -> Result<(), FsError> {
        for copy in 0..self.copies {
            let sector = self.first_sector
                + u64::from(copy) * u64::from(self.fat_sectors)
                + u64::from(start);
            dev.write(sector, data)?;
        }
        Ok(())
    }

    /// Makes room for `count` more sectors, writing out dirty ones if it has to.
    fn make_room<D: BlockDevice>(&mut self, dev: &mut D, count: usize) -> Result<(), FsError> {
        while self.cache.len() + count > CACHE_SECTORS {
            // Prefer a sector that needs no writing.
            let victim = self
                .cache
                .iter()
                .find(|(_, c)| !c.dirty)
                .or_else(|| self.cache.iter().next())
                .map(|(&k, _)| k);
            let Some(key) = victim else { break };
            if let Some(c) = self.cache.get(&key)
                && c.dirty
            {
                let data = c.data;
                self.write_run(dev, key, &data)?;
            }
            self.cache.remove(&key);
        }
        Ok(())
    }

    /// Brings the group of sectors holding sector `sector` into the cache.
    fn load_group<D: BlockDevice>(&mut self, dev: &mut D, sector: u32) -> Result<(), FsError> {
        let start = sector - sector % READ_AHEAD;
        let count = READ_AHEAD.min(self.fat_sectors - start) as usize;
        let mut raw = vec![0u8; count * SECTOR];
        dev.read(self.first_sector + u64::from(start), &mut raw)?;
        self.make_room(dev, count)?;
        for (i, piece) in raw.chunks_exact(SECTOR).enumerate() {
            // A sector already cached may be dirty, and is newer than the disk's.
            self.cache.entry(start + i as u32).or_insert_with(|| {
                let mut data = [0u8; SECTOR];
                data.copy_from_slice(piece);
                Cached { data, dirty: false }
            });
        }
        Ok(())
    }

    /// The cached sector `sector`, read first if need be.
    fn sector<D: BlockDevice>(&mut self, dev: &mut D, sector: u32) -> Result<&mut Cached, FsError> {
        if !self.cache.contains_key(&sector) {
            self.load_group(dev, sector)?;
        }
        // `load_group` has just inserted it, unless `sector` is outside the FAT, which `get`
        // and `set` rule out first.
        self.cache.get_mut(&sector).ok_or(FsError::Damaged)
    }

    /// The (28-bit) entry for `cluster`. Entries 0 and 1 may be read too.
    pub fn get<D: BlockDevice>(&mut self, dev: &mut D, cluster: u32) -> Result<u32, FsError> {
        if cluster >= self.entries {
            return Err(FsError::Damaged);
        }
        let at = (cluster % PER_SECTOR) as usize * 4;
        let c = self.sector(dev, cluster / PER_SECTOR)?;
        Ok(u32::from_le_bytes([c.data[at], c.data[at + 1], c.data[at + 2], c.data[at + 3]])
            & ENTRY_MASK)
    }

    /// Sets the entry for `cluster` to `value` (28 bits), leaving the reserved top four bits
    /// as they were. The change is in memory until [`FatTable::flush_dirty`].
    pub fn set<D: BlockDevice>(
        &mut self,
        dev: &mut D,
        cluster: u32,
        value: u32,
    ) -> Result<(), FsError> {
        if cluster >= self.entries {
            return Err(FsError::Damaged);
        }
        let at = (cluster % PER_SECTOR) as usize * 4;
        let c = self.sector(dev, cluster / PER_SECTOR)?;
        let old = u32::from_le_bytes([c.data[at], c.data[at + 1], c.data[at + 2], c.data[at + 3]]);
        let new = (old & !ENTRY_MASK) | (value & ENTRY_MASK);
        c.data[at..at + 4].copy_from_slice(&new.to_le_bytes());
        c.dirty = true;
        Ok(())
    }

    /// Writes every changed sector to every copy of the FAT, in runs. This does not flush the
    /// device: the caller does that after, when the order of writes matters.
    pub fn flush_dirty<D: BlockDevice>(&mut self, dev: &mut D) -> Result<(), FsError> {
        let dirty: Vec<u32> = self.cache.iter().filter(|(_, c)| c.dirty).map(|(&k, _)| k).collect();
        let mut i = 0;
        while i < dirty.len() {
            let start = dirty[i];
            let mut n = 1;
            while i + n < dirty.len() && dirty[i + n] == start + n as u32 && n < WRITE_SECTORS {
                n += 1;
            }
            let mut raw = Vec::with_capacity(n * SECTOR);
            for k in 0..n {
                raw.extend_from_slice(&self.cache.get(&dirty[i + k]).ok_or(FsError::Damaged)?.data);
            }
            self.write_run(dev, start, &raw)?;
            for k in 0..n {
                if let Some(c) = self.cache.get_mut(&dirty[i + k]) {
                    c.dirty = false;
                }
            }
            i += n;
        }
        Ok(())
    }

    /// The first free cluster at or after `from`, wrapping round to the start, or `None` if
    /// there is none. (Entries 0 and 1 are never offered.)
    pub fn find_free<D: BlockDevice>(
        &mut self,
        dev: &mut D,
        from: u32,
    ) -> Result<Option<u32>, FsError> {
        let from = from.clamp(2, self.entries.saturating_sub(1).max(2));
        for (lo, hi) in [(from, self.entries), (2, from)] {
            let mut c = lo;
            while c < hi {
                let sector = c / PER_SECTOR;
                let end = hi.min((sector + 1) * PER_SECTOR);
                let cached = self.sector(dev, sector)?;
                for cluster in c..end {
                    let at = (cluster % PER_SECTOR) as usize * 4;
                    let raw = [
                        cached.data[at],
                        cached.data[at + 1],
                        cached.data[at + 2],
                        cached.data[at + 3],
                    ];
                    if u32::from_le_bytes(raw) & ENTRY_MASK == 0 {
                        return Ok(Some(cluster));
                    }
                }
                c = end;
            }
        }
        Ok(None)
    }

    /// Counts the free clusters by reading the first copy of the FAT straight from the disk, in
    /// big pieces. Only for use before anything has been changed (at mount): it does not look at
    /// the cache.
    pub fn count_free<D: BlockDevice>(&self, dev: &mut D) -> Result<u32, FsError> {
        let mut free = 0u32;
        let mut index = 0u32; // the FAT entry number of the next entry read
        let mut sector = 0u32;
        while sector < self.fat_sectors && index < self.entries {
            let n = (self.fat_sectors - sector).min(WRITE_SECTORS as u32) as usize;
            let mut raw = vec![0u8; n * SECTOR];
            dev.read(self.first_sector + u64::from(sector), &mut raw)?;
            for entry in raw.chunks_exact(4) {
                if index >= self.entries {
                    break;
                }
                if index >= 2
                    && u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) & ENTRY_MASK
                        == 0
                {
                    free += 1;
                }
                index += 1;
            }
            sector += n as u32;
        }
        Ok(free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pbr_fs::memdisk::MemDisk;

    /// A FAT of 40 sectors (5120 entries, 5000 used), 2 copies, after 4 reserved sectors.
    fn table() -> (FatTable, MemDisk) {
        let t = FatTable {
            first_sector: 4,
            fat_sectors: 40,
            copies: 2,
            entries: 5000,
            cache: BTreeMap::new(),
        };
        (t, MemDisk::new(4 + 80 + 10))
    }

    #[test]
    fn classifying_entries() {
        let last = 1000;
        assert_eq!(classify(0, last), Link::Free);
        assert_eq!(classify(0xF000_0000, last), Link::Free); // the top bits do not count
        assert_eq!(classify(2, last), Link::Next(2));
        assert_eq!(classify(1000, last), Link::Next(1000));
        assert_eq!(classify(1001, last), Link::Bad);
        assert_eq!(classify(1, last), Link::Bad);
        assert_eq!(classify(0x0FFF_FFF6, last), Link::Bad);
        assert_eq!(classify(0x0FFF_FFF7, last), Link::Bad); // a cluster marked bad
        assert_eq!(classify(0x0FFF_FFF8, last), Link::End);
        assert_eq!(classify(0x0FFF_FFFF, last), Link::End);
        assert_eq!(classify(0xFFFF_FFFF, last), Link::End);
    }

    #[test]
    fn get_and_set_keep_the_reserved_bits() {
        let (mut t, mut d) = table();
        d.data[4 * SECTOR + 8..4 * SECTOR + 12].copy_from_slice(&0xA000_0123u32.to_le_bytes());
        assert_eq!(t.get(&mut d, 2).unwrap(), 0x123);
        t.set(&mut d, 2, 0x0ABC_DEF0).unwrap();
        assert_eq!(t.get(&mut d, 2).unwrap(), 0x0ABC_DEF0);
        t.flush_dirty(&mut d).unwrap();
        let raw = &d.data[4 * SECTOR + 8..4 * SECTOR + 12];
        assert_eq!(u32::from_le_bytes(raw.try_into().unwrap()), 0xA000_0000 | 0x0ABC_DEF0);
        assert_eq!(t.get(&mut d, 5000), Err(FsError::Damaged));
        assert_eq!(t.set(&mut d, 5000, 1), Err(FsError::Damaged));
        assert_eq!(t.get(&mut d, u32::MAX), Err(FsError::Damaged));
    }

    #[test]
    fn both_copies_are_written_in_runs() {
        let (mut t, mut d) = table();
        for c in [3u32, 200, 4000, 4001, 4999] {
            t.set(&mut d, c, c + 1).unwrap();
        }
        assert_eq!(d.writes, 0, "nothing reaches the disk before the flush");
        t.flush_dirty(&mut d).unwrap();
        for c in [3u32, 200, 4000, 4001, 4999] {
            for copy in 0..2 {
                let at = (4 + copy * 40) as usize * SECTOR + c as usize * 4;
                let v = u32::from_le_bytes(d.data[at..at + 4].try_into().unwrap());
                assert_eq!(v, c + 1);
            }
        }
        // The dirty sectors were 0, 1, 31 and 39: three runs, each written to two copies.
        assert_eq!(d.writes, 6);
        t.flush_dirty(&mut d).unwrap();
        assert_eq!(d.writes, 6, "nothing more to write");
    }

    #[test]
    fn a_small_cache_still_gives_the_right_answers() {
        let (mut t, _) = table();
        // Touch far more sectors than the cache holds, changing every one: evicted dirty
        // sectors must be written out and read back right.
        let mut big = MemDisk::new(4 + 2 * 2000 + 10);
        t.fat_sectors = 2000;
        t.entries = 2000 * PER_SECTOR;
        for c in (2..t.entries).step_by(PER_SECTOR as usize) {
            t.set(&mut big, c, c ^ 0x55).unwrap();
            assert!(t.cache.len() <= CACHE_SECTORS);
        }
        for c in (2..t.entries).step_by(PER_SECTOR as usize) {
            assert_eq!(t.get(&mut big, c).unwrap(), c ^ 0x55);
        }
    }

    #[test]
    fn finding_free_clusters() {
        let (mut t, mut d) = table();
        for c in 2..5000 {
            t.set(&mut d, c, END).unwrap();
        }
        assert_eq!(t.find_free(&mut d, 2).unwrap(), None);
        t.set(&mut d, 10, 0).unwrap();
        t.set(&mut d, 4999, 0).unwrap();
        assert_eq!(t.find_free(&mut d, 2).unwrap(), Some(10));
        assert_eq!(t.find_free(&mut d, 11).unwrap(), Some(4999));
        assert_eq!(t.find_free(&mut d, 4999).unwrap(), Some(4999));
        // Wraps round from the end; hints outside the volume are brought back inside.
        t.set(&mut d, 4999, END).unwrap();
        assert_eq!(t.find_free(&mut d, 4500).unwrap(), Some(10));
        assert_eq!(t.find_free(&mut d, u32::MAX).unwrap(), Some(10));
        assert_eq!(t.find_free(&mut d, 0).unwrap(), Some(10));
        // Entries 0 and 1 are never offered, nor are entries past the end of the volume.
        t.set(&mut d, 10, END).unwrap();
        assert_eq!(t.find_free(&mut d, 2).unwrap(), None);
    }

    #[test]
    fn counting_free_clusters_directly() {
        let (t, mut d) = table();
        // The disk is zeroed: all 4998 clusters (entries 2..5000) are free. Entries 0 and 1 and
        // the unused entries after 4999 are not counted.
        assert_eq!(t.count_free(&mut d).unwrap(), 4998);
        let at = 4 * SECTOR + 3 * 4;
        d.data[at..at + 4].copy_from_slice(&END.to_le_bytes());
        assert_eq!(t.count_free(&mut d).unwrap(), 4997);
        d.data[at..at + 4].copy_from_slice(&0xF000_0000u32.to_le_bytes());
        assert_eq!(t.count_free(&mut d).unwrap(), 4998);
    }

    #[test]
    fn disk_errors_are_reported() {
        let (mut t, mut d) = table();
        t.set(&mut d, 5, 6).unwrap();
        d.fail_after_writes = Some(0);
        assert_eq!(t.flush_dirty(&mut d), Err(FsError::Disk));
        // The sector is still dirty, so a later flush can try again.
        d.fail_after_writes = None;
        t.flush_dirty(&mut d).unwrap();
        assert_eq!(t.get(&mut d, 5).unwrap(), 6);
    }
}
