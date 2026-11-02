// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests on the host, against a simulated disk that has a write cache of its own (as real
//! disks do): writes it accepts sit in `pending` until it's flushed. The model is simply what
//! each disk should hold. After random reads, writes and flushes, every read must match the
//! model (rule 1), and after a flush the disk's *stable* contents must match the model for that
//! volume (rule 3).

extern crate std;

use super::*;

/// A simulated disk.
struct SimDisk {
    stable: Vec<u8>,
    pending: Vec<(u64, Vec<u8>)>,
}

/// The simulated disks, counting the calls they get and failing some on request.
#[derive(Default)]
struct Sim {
    disks: BTreeMap<u32, SimDisk>,
    reads: usize,
    writes: usize,
    fail_writes: usize,
    fail_reads: usize,
}

impl Sim {
    fn add(&mut self, disk: u32, sectors: u64) {
        self.disks.insert(
            disk,
            SimDisk { stable: vec![0; sectors as usize * SECTOR], pending: Vec::new() },
        );
    }
    fn check(&self, disk: u32, sector: u64, len: usize) {
        assert!(
            len > 0 && len.is_multiple_of(SECTOR) && len <= MAX_IO,
            "a bad request: {len} bytes"
        );
        let size = self.disks[&disk].stable.len();
        assert!(sector as usize * SECTOR + len <= size, "a request past the end of disk {disk}");
    }
}

impl Backend for Sim {
    fn read(&mut self, disk: u32, sector: u64, buf: &mut [u8]) -> Result<(), IoError> {
        self.check(disk, sector, buf.len());
        self.reads += 1;
        if self.fail_reads > 0 {
            self.fail_reads -= 1;
            return Err(IoError);
        }
        // A disk's own cache answers reads with what was written last.
        let d = &self.disks[&disk];
        let at = sector as usize * SECTOR;
        buf.copy_from_slice(&d.stable[at..at + buf.len()]);
        for (s, data) in &d.pending {
            let (ps, pe) = (*s as usize * SECTOR, *s as usize * SECTOR + data.len());
            let (lo, hi) = (ps.max(at), pe.min(at + buf.len()));
            if lo < hi {
                buf[lo - at..hi - at].copy_from_slice(&data[lo - ps..hi - ps]);
            }
        }
        Ok(())
    }
    fn write(&mut self, disk: u32, sector: u64, data: &[u8]) -> Result<(), IoError> {
        self.check(disk, sector, data.len());
        self.writes += 1;
        if self.fail_writes > 0 {
            self.fail_writes -= 1;
            return Err(IoError);
        }
        self.disks.get_mut(&disk).unwrap().pending.push((sector, data.to_vec()));
        Ok(())
    }
    fn flush(&mut self, disk: u32) -> Result<(), IoError> {
        let d = self.disks.get_mut(&disk).unwrap();
        for (s, data) in core::mem::take(&mut d.pending) {
            let at = s as usize * SECTOR;
            d.stable[at..at + data.len()].copy_from_slice(&data);
        }
        Ok(())
    }
}

/// A small, repeatable random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Disk 1 (1003 sectors: its last page is short) holds volume 1 at sectors 1..501 and volume 2
/// at 501..1003, which share a page, since 501 isn't on a 4 KiB boundary. Disk 2 (300 sectors)
/// is volume 3.
fn setup(pages: usize) -> (Cache, Sim) {
    let mut c = Cache::new(pages);
    let mut sim = Sim::default();
    for (disk, sectors) in [(1, 1003), (2, 300)] {
        sim.add(disk, sectors);
        c.add_disk(disk, sectors);
    }
    c.add_volume(1, 1, 1, 500).unwrap();
    c.add_volume(2, 1, 501, 502).unwrap();
    c.add_volume(3, 2, 0, 300).unwrap();
    (c, sim)
}

const VOLUMES: [(u32, u32, u64, u64); 3] = [(1, 1, 1, 500), (2, 1, 501, 502), (3, 2, 0, 300)];

#[test]
fn random_operations_follow_the_rules() {
    for seed in 1..=4u64 {
        let (mut c, mut sim) = setup(32);
        let mut model: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        model.insert(1, vec![0; 1003 * SECTOR]);
        model.insert(2, vec![0; 300 * SECTOR]);
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for now in 0..6000u64 {
            let (id, disk, start, size) = VOLUMES[rng.below(3) as usize];
            let count = 1 + rng.below(size.min(128));
            let mut sector = rng.below(size - count + 1);
            // A quarter of the time, aim at the page volumes 1 and 2 share.
            if rng.below(4) == 0 {
                sector = if id == 1 { size - count } else { 0 };
            }
            let at = (start + sector) as usize * SECTOR;
            let len = count as usize * SECTOR;
            match rng.below(10) {
                0..=3 => {
                    let mut buf = vec![0u8; len];
                    c.read(&mut sim, id, sector, &mut buf).unwrap();
                    assert_eq!(buf, model[&disk][at..at + len], "seed {seed}, step {now}: read");
                }
                4..=7 => {
                    let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                    c.write(&mut sim, id, sector, &data, now).unwrap();
                    model.get_mut(&disk).unwrap()[at..at + len].copy_from_slice(&data);
                }
                8 => {
                    c.flush(&mut sim, id).unwrap();
                    let (lo, hi) = (start as usize * SECTOR, (start + size) as usize * SECTOR);
                    assert_eq!(
                        sim.disks[&disk].stable[lo..hi],
                        model[&disk][lo..hi],
                        "seed {seed}, step {now}: after flushing volume {id}"
                    );
                }
                _ => c.write_back_older(&mut sim, now.saturating_sub(50)),
            }
            for (id, ..) in VOLUMES {
                assert!(c.dirty_pages(id) <= c.pages() / 4, "volume {id} over its share");
            }
        }
        c.flush_all(&mut sim).unwrap();
        for disk in [1, 2] {
            assert_eq!(sim.disks[&disk].stable, model[&disk], "seed {seed}: at the end");
        }
    }
}

#[test]
fn reads_are_batched_and_cached() {
    let (mut c, mut sim) = setup(64);
    // 64 KiB from a page boundary: one disk call, then none.
    let mut buf = vec![0u8; MAX_REQUEST];
    c.read(&mut sim, 3, 0, &mut buf).unwrap();
    assert_eq!(sim.reads, 1);
    c.read(&mut sim, 3, 0, &mut buf).unwrap();
    assert_eq!(sim.reads, 1);
    // Not on a boundary, half cached: one more call, for the missing pages only.
    c.read(&mut sim, 3, 100, &mut buf).unwrap();
    assert_eq!(sim.reads, 2);
}

#[test]
fn whole_page_writes_read_nothing() {
    let (mut c, mut sim) = setup(64);
    c.write(&mut sim, 3, 8, &[7u8; 3 * PAGE], 0).unwrap();
    assert_eq!((sim.reads, sim.writes), (0, 0)); // nothing read, nothing written yet
    c.write(&mut sim, 3, 1, &[9u8; SECTOR], 0).unwrap(); // part of a page: read it first
    assert_eq!(sim.reads, 1);
    c.flush(&mut sim, 3).unwrap();
    assert_eq!(sim.writes, 4);
    assert!(sim.disks[&2].stable[8 * SECTOR..32 * SECTOR].iter().all(|&b| b == 7));
    assert!(sim.disks[&2].stable[SECTOR..2 * SECTOR].iter().all(|&b| b == 9));
}

#[test]
fn refusals() {
    let (mut c, mut sim) = setup(32);
    let mut buf = vec![0u8; SECTOR];
    assert_eq!(c.read(&mut sim, 9, 0, &mut buf), Err(Error::NoSuchVolume));
    assert_eq!(c.read(&mut sim, 1, 500, &mut buf), Err(Error::OutOfRange));
    assert_eq!(c.read(&mut sim, 1, u64::MAX, &mut buf), Err(Error::OutOfRange));
    assert_eq!(c.read(&mut sim, 1, 0, &mut buf[..100]), Err(Error::BadArgument));
    assert_eq!(c.read(&mut sim, 1, 0, &mut []), Err(Error::BadArgument));
    let mut big = vec![0u8; MAX_REQUEST + SECTOR];
    assert_eq!(c.read(&mut sim, 3, 0, &mut big), Err(Error::BadArgument));
    assert_eq!(c.write(&mut sim, 2, 502, &buf, 0), Err(Error::OutOfRange));
    assert_eq!(c.add_volume(4, 1, 1000, 10), Err(Error::OutOfRange));
    assert_eq!(c.add_volume(4, 7, 0, 10), Err(Error::NoSuchVolume));
    assert_eq!(sim.reads + sim.writes, 0);
}

#[test]
fn failures_are_reported() {
    let (mut c, mut sim) = setup(32);
    // A read that fails changes nothing, and the next one works.
    let mut buf = vec![0u8; PAGE];
    sim.fail_reads = 1;
    assert_eq!(c.read(&mut sim, 3, 0, &mut buf), Err(Error::Disk));
    c.read(&mut sim, 3, 0, &mut buf).unwrap();
    // A write-back that fails in the background is reported by the volume's next flush, once.
    c.write(&mut sim, 3, 0, &[1u8; PAGE], 0).unwrap();
    sim.fail_writes = 1;
    c.write_back_older(&mut sim, 1);
    assert_eq!(c.flush(&mut sim, 3), Err(Error::Disk));
    assert_eq!(c.flush(&mut sim, 3), Ok(()));
    // A failure on a page two volumes share is reported to both.
    c.write(&mut sim, 1, 499, &[2u8; SECTOR], 0).unwrap(); // sector 500 of disk 1: page 62
    sim.fail_writes = 1;
    c.write_back_older(&mut sim, 1);
    assert_eq!(c.flush(&mut sim, 2), Err(Error::Disk));
    assert_eq!(c.flush(&mut sim, 1), Err(Error::Disk));
}

#[test]
fn a_full_volume_writes_back_its_own_pages() {
    let (mut c, mut sim) = setup(32);
    // Volume 3 dirties more than its share (8 pages): its oldest go to the disk.
    for i in 0..20u64 {
        c.write(&mut sim, 3, i * 8, &[i as u8; PAGE], i).unwrap();
    }
    assert_eq!(c.dirty_pages(3), 8);
    assert_eq!(sim.writes, 12);
    // Volume 1's pages aren't touched by it.
    c.write(&mut sim, 1, 7, &[5u8; PAGE], 100).unwrap();
    assert_eq!(c.dirty_pages(1), 1);
}

/// Two volumes sharing a page: flushing one writes the page, whichever volume dirtied it first,
/// because the other's later write to its own sectors in the page must reach the disk too.
#[test]
fn a_shared_page_is_flushed_for_both() {
    let (mut c, mut sim) = setup(32);
    c.write(&mut sim, 2, 0, &[2u8; SECTOR], 0).unwrap(); // disk sector 501: page 62, owned by 2
    c.write(&mut sim, 1, 499, &[1u8; SECTOR], 0).unwrap(); // disk sector 500: the same page
    c.flush(&mut sim, 1).unwrap();
    assert!(sim.disks[&1].stable[500 * SECTOR..501 * SECTOR].iter().all(|&b| b == 1));
    c.flush(&mut sim, 2).unwrap();
    assert!(sim.disks[&1].stable[501 * SECTOR..502 * SECTOR].iter().all(|&b| b == 2));
}
