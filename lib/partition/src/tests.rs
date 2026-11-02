// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests on the host: every kind of table and volume, damage of each kind, and random
//! corruption of valid disks, which must never panic or report a volume outside the disk.

extern crate std;

use super::*;
use alloc::string::ToString;
use std::vec::Vec;

/// A disk in memory. Reading a sector listed in `bad` fails, like a damaged disk.
struct Mem {
    data: Vec<u8>,
    bad: Vec<u64>,
}

impl Mem {
    fn new(sectors: u64) -> Mem {
        Mem { data: vec![0; sectors as usize * SECTOR], bad: Vec::new() }
    }
    fn put(&mut self, sector: u64, offset: usize, bytes: &[u8]) {
        let at = sector as usize * SECTOR + offset;
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
    }
}

impl Disk for Mem {
    fn sectors(&self) -> u64 {
        (self.data.len() / SECTOR) as u64
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        let count = (buf.len() / SECTOR) as u64;
        assert!(sector + count <= self.sectors(), "read past the end of the disk");
        if self.bad.iter().any(|b| (sector..sector + count).contains(b)) {
            return Err(ReadError);
        }
        let at = sector as usize * SECTOR;
        buf.copy_from_slice(&self.data[at..at + buf.len()]);
        Ok(())
    }
}

/// A RockFS superblock (only what identifying it looks at) with `label`.
fn rockfs(label: &str) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[0..8].copy_from_slice(b"ROCKFS!\0");
    b[8..10].copy_from_slice(&1u16.to_le_bytes());
    b[10..12].copy_from_slice(&12u16.to_le_bytes());
    b[128..128 + label.len()].copy_from_slice(label.as_bytes());
    let c = pbr_crc::crc32(&b[..1020]);
    b[1020..].copy_from_slice(&c.to_le_bytes());
    b
}

/// A FAT32 boot sector for a volume of `total` sectors, labelled `label`.
fn fat32(total: u32, label: &str) -> Vec<u8> {
    let mut s = vec![0u8; SECTOR];
    s[0] = 0xEB;
    s[11..13].copy_from_slice(&512u16.to_le_bytes());
    s[13] = 1;
    s[14..16].copy_from_slice(&32u16.to_le_bytes());
    s[16] = 2;
    s[32..36].copy_from_slice(&total.to_le_bytes());
    s[36..40].copy_from_slice(&(total / 256).to_le_bytes());
    s[44..48].copy_from_slice(&2u32.to_le_bytes());
    s[66] = 0x29;
    let mut l = [b' '; 11];
    l[..label.len()].copy_from_slice(label.as_bytes());
    s[71..82].copy_from_slice(&l);
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

/// An MBR with entries of (type, start, length).
fn mbr(disk: &mut Mem, entries: &[(u8, u32, u32)]) {
    for (i, &(kind, start, len)) in entries.iter().enumerate() {
        let e = 446 + 16 * i;
        disk.put(0, e + 4, &[kind]);
        disk.put(0, e + 8, &start.to_le_bytes());
        disk.put(0, e + 12, &len.to_le_bytes());
    }
    disk.put(0, 510, &[0x55, 0xAA]);
}

/// A GPT with partitions of (type, first, last), both copies, and a protective MBR.
fn gpt(disk: &mut Mem, parts: &[([u8; 16], u64, u64)]) {
    let total = disk.sectors();
    mbr(disk, &[(0xEE, 1, (total - 1).min(u32::MAX as u64) as u32)]);
    let mut list = vec![0u8; 128 * 128];
    for (i, &(kind, first, last)) in parts.iter().enumerate() {
        let e = &mut list[i * 128..][..128];
        e[..16].copy_from_slice(&kind);
        e[16] = i as u8 + 1; // a partition GUID, unique enough here
        e[32..40].copy_from_slice(&first.to_le_bytes());
        e[40..48].copy_from_slice(&last.to_le_bytes());
    }
    let list_crc = pbr_crc::crc32(&list);
    for (at, entries_at, other) in [(1, 2, total - 1), (total - 1, total - 33, 1)] {
        let mut h = vec![0u8; 92];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&at.to_le_bytes());
        h[32..40].copy_from_slice(&other.to_le_bytes());
        h[40..48].copy_from_slice(&34u64.to_le_bytes());
        h[48..56].copy_from_slice(&(total - 34).to_le_bytes());
        h[72..80].copy_from_slice(&entries_at.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&list_crc.to_le_bytes());
        let c = pbr_crc::crc32(&h);
        h[16..20].copy_from_slice(&c.to_le_bytes());
        disk.put(at, 0, &h);
        disk.put(entries_at, 0, &list);
    }
}

fn kinds(s: &Scan) -> Vec<(u32, u64, u64, Filesystem, &str)> {
    s.volumes
        .iter()
        .map(|v| (v.partition, v.start, v.sectors, v.filesystem, v.label.as_str()))
        .collect()
}

#[test]
fn empty_and_tiny_disks() {
    assert_eq!(scan(&mut Mem::new(0)).unwrap().volumes, []);
    for n in 1..40 {
        let s = scan(&mut Mem::new(n)).unwrap();
        assert_eq!(s.table, Table::None);
        assert_eq!(kinds(&s), [(0, 0, n, Filesystem::Empty, "")]);
    }
    // Something that isn't zeros, and isn't anything we know.
    let mut d = Mem::new(64);
    d.put(3, 0, b"hello");
    assert_eq!(kinds(&scan(&mut d).unwrap()), [(0, 0, 64, Filesystem::Unknown, "")]);
}

#[test]
fn whole_disk_volumes() {
    let mut d = Mem::new(4096);
    d.put(0, 0, &rockfs("Data"));
    assert_eq!(kinds(&scan(&mut d).unwrap()), [(0, 0, 4096, Filesystem::RockFs, "Data")]);

    // The main superblock damaged: the backup, in the last 4 KiB block, still identifies it.
    d.put(0, 100, b"scribble");
    d.put(4096 - 8, 0, &rockfs("Data"));
    assert_eq!(kinds(&scan(&mut d).unwrap()), [(0, 0, 4096, Filesystem::RockFs, "Data")]);

    let mut d = Mem::new(70_000);
    d.put(0, 0, &fat32(70_000, "STICK"));
    let s = scan(&mut d).unwrap();
    assert_eq!(
        (s.table, kinds(&s)),
        (Table::None, vec![(0, 0, 70_000, Filesystem::Fat32, "STICK")])
    );
    // A FAT32 boot sector claiming more sectors than the disk has isn't FAT32.
    // (Its sector still ends in 0x55AA, so it reads as an MBR with no partitions.)
    d.put(0, 32, &80_000u32.to_le_bytes());
    let s = scan(&mut d).unwrap();
    assert_eq!((s.table, s.volumes.len()), (Table::Mbr, 0));
}

#[test]
fn mbr_partitions() {
    let mut d = Mem::new(200_000);
    mbr(
        &mut d,
        &[(0x7F, 2048, 8192), (0x0C, 10_240, 70_000), (0x83, 80_240, 1000), (0x05, 90_000, 100)],
    );
    d.put(2048, 0, &rockfs("Data"));
    d.put(10_240, 0, &fat32(70_000, "USB"));
    d.put(80_240, 0, b"something");
    let s = scan(&mut d).unwrap();
    assert_eq!(s.table, Table::Mbr);
    assert_eq!(
        kinds(&s),
        [
            (1, 2048, 8192, Filesystem::RockFs, "Data"),
            (2, 10_240, 70_000, Filesystem::Fat32, "USB"),
            (3, 80_240, 1000, Filesystem::Unknown, ""),
        ]
    );
    assert_eq!(s.problems, [Problem::Extended(4)]);

    // Contents decide, not types: a partition typed RockFS holding zeros is empty, and one
    // typed Linux holding a RockFS superblock is RockFS.
    let mut d = Mem::new(20_000);
    mbr(&mut d, &[(0x7F, 2048, 4096), (0x83, 8192, 4096)]);
    d.put(8192, 0, &rockfs("Moved"));
    let s = scan(&mut d).unwrap();
    assert_eq!(s.volumes[0].filesystem, Filesystem::Empty);
    assert_eq!(s.volumes[1].filesystem, Filesystem::RockFs);
}

#[test]
fn damaged_mbrs() {
    for entries in [
        vec![(0x7F, 2048, 8192), (0x0C, 4096, 8192)], // overlapping
        vec![(0x7F, 2048, 1_000_000)],                // past the end
        vec![(0x7F, 0, 100)],                         // over the MBR itself
        vec![(0x7F, 2048, 0)],                        // empty
        vec![(0x7F, u32::MAX, u32::MAX)],             // wrapping
    ] {
        let mut d = Mem::new(20_000);
        mbr(&mut d, &entries);
        let s = scan(&mut d).unwrap();
        assert_eq!(s.volumes, [], "{entries:?}");
        assert!(matches!(s.problems[..], [Problem::Damaged(_)]), "{entries:?}");
    }
}

#[test]
fn gpt_partitions_and_the_backup() {
    let mut d = Mem::new(100_000);
    gpt(&mut d, &[(ROCKFS_GPT_TYPE, 2048, 10_239), (ESP_GPT_TYPE, 10_240, 80_239)]);
    d.put(2048, 0, &rockfs("Home"));
    d.put(10_240, 0, &fat32(70_000, "EFI"));
    let expect = [
        (1, 2048, 8192, Filesystem::RockFs, "Home"),
        (2, 10_240, 70_000, Filesystem::Fat32, "EFI"),
    ];
    let s = scan(&mut d).unwrap();
    assert_eq!((s.table, kinds(&s), s.problems.clone()), (Table::Gpt, expect.to_vec(), vec![]));

    // The main header damaged: the backup is used, and said so.
    let mut broken = Mem { data: d.data.clone(), bad: Vec::new() };
    broken.put(1, 40, &[0xFF]);
    let s = scan(&mut broken).unwrap();
    assert_eq!((kinds(&s), s.problems.clone()), (expect.to_vec(), vec![Problem::UsedBackupGpt]));
    // The main entry list damaged (its checksum no longer matches): the same.
    let mut broken = Mem { data: d.data.clone(), bad: Vec::new() };
    broken.put(2, 40, &[0x12]); // the first entry's last sector
    assert_eq!(scan(&mut broken).unwrap().problems, [Problem::UsedBackupGpt]);
    // Both damaged: nothing is used.
    broken.put(100_000 - 1, 40, &[0xFF]);
    let s = scan(&mut broken).unwrap();
    assert_eq!(s.volumes, []);
    assert!(matches!(s.problems[..], [Problem::Damaged(_)]));
    // A sector of the table that can't be read is an error, not a guess.
    let mut unreadable = Mem { data: d.data.clone(), bad: vec![2] };
    assert_eq!(scan(&mut unreadable), Err(ReadError));
}

#[test]
fn damaged_gpts() {
    for parts in [
        vec![(ROCKFS_GPT_TYPE, 2048, 10_000), (ESP_GPT_TYPE, 9000, 20_000)], // overlapping
        vec![(ROCKFS_GPT_TYPE, 2048, 99_990)],                               // past the usable end
        vec![(ROCKFS_GPT_TYPE, 10, 100)],                                    // over the table
        vec![(ROCKFS_GPT_TYPE, 5000, 4000)],                                 // backwards
    ] {
        let mut d = Mem::new(100_000);
        gpt(&mut d, &parts);
        let s = scan(&mut d).unwrap();
        assert_eq!(s.volumes, [], "{parts:?}");
        assert!(matches!(s.problems[..], [Problem::Damaged(_)]), "{parts:?}");
    }
}

#[test]
fn labels_are_text() {
    let mut d = Mem::new(4096);
    let mut sb = rockfs("");
    sb[128..132].copy_from_slice(&[0xFF, 0xFE, b'a', b'b']); // not UTF-8
    let c = pbr_crc::crc32(&sb[..1020]);
    sb[1020..].copy_from_slice(&c.to_le_bytes());
    d.put(0, 0, &sb);
    assert_eq!(scan(&mut d).unwrap().volumes[0].label, "");
    let mut d = Mem::new(4096);
    d.put(0, 0, &rockfs("Ünïcode"));
    assert_eq!(scan(&mut d).unwrap().volumes[0].label, "Ünïcode".to_string());
}

/// A small, repeatable random number generator (xorshift), so a failure can be reproduced.
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

/// Random damage to valid disks: whatever comes back must be inside the disk and not
/// overlapping, and nothing may panic or read past the end (the test disk asserts that).
#[test]
fn random_corruption() {
    // Small disks (12 MiB), so each round's copy is cheap.
    const N: u64 = 24_000;
    let mut originals = Vec::new();
    let mut d = Mem::new(N);
    gpt(&mut d, &[(ROCKFS_GPT_TYPE, 2048, 6143), (ESP_GPT_TYPE, 6144, 23_000)]);
    d.put(2048, 0, &rockfs("Home"));
    d.put(6144, 0, &fat32(16_857, "EFI"));
    originals.push(d);
    let mut d = Mem::new(N);
    mbr(&mut d, &[(0x7F, 2048, 4096), (0x0C, 6144, 16_000)]);
    d.put(2048, 0, &rockfs("Data"));
    d.put(6144, 0, &fat32(16_000, "USB"));
    originals.push(d);
    let mut d = Mem::new(N);
    d.put(0, 0, &fat32(N as u32, "STICK"));
    originals.push(d);

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // The interesting sectors: the tables at both ends, and the volumes' first sectors.
    let hot = [0u64, 1, 2, 3, 2048, 2049, 6144, N - 1, N - 2, N - 33];
    for round in 0..5_000 {
        let o = &originals[round % originals.len()];
        let mut d = Mem { data: o.data.clone(), bad: Vec::new() };
        for _ in 0..1 + rng.below(8) {
            let sector = hot[rng.below(hot.len() as u64) as usize];
            let byte = rng.below(SECTOR as u64) as usize;
            let value = match rng.below(4) {
                0 => 0,
                1 => 0xFF,
                _ => rng.next() as u8,
            };
            d.put(sector, byte, &[value]);
        }
        let Ok(s) = scan(&mut d) else { continue };
        let mut spans: Vec<(u64, u64)> = s.volumes.iter().map(|v| (v.start, v.sectors)).collect();
        spans.sort();
        for &(start, len) in &spans {
            assert!(len > 0 && start + len <= d.sectors(), "round {round}: {spans:?}");
        }
        for w in spans.windows(2) {
            assert!(w[0].0 + w[0].1 <= w[1].0, "round {round}: overlapping {spans:?}");
        }
    }
}
