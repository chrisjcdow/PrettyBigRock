// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Damaged and hostile volumes. Nothing read from a disk is trusted (note 0002): a volume may be
//! corrupt by accident or built to cause harm, and the filesystem server must not panic, hang,
//! read outside the volume, or hand out one file's bytes as another's. It may return errors.
//!
//! Two kinds of test: *crafted* damage, one flaw at a time, where the right answer is known; and
//! *fuzzing*, where random bytes of a good volume are changed and everything is tried, where the
//! only thing known is that nothing may go wrong. Every run has a time limit, to catch hangs.

use super::*;
use pbr_fs::SECTOR;
use std::string::ToString;
use std::sync::mpsc;
use std::time::Duration;

/// Runs `f` on another thread and fails if it panics or takes more than 60 seconds (a hang, or
/// a walk that never ends).
fn guarded<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(v) => v,
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{what}: hung"),
        Err(_) => {
            let _ = handle.join();
            panic!("{what}: panicked");
        }
    }
}

// ---- Placing damage ------------------------------------------------------------------------

/// Where things are in a volume made by `blank_with(.., 1, ..)`: the root folder is cluster 2.
struct Layout {
    fat: usize,
    data: usize,
}

fn layout(disk: &MemDisk) -> Layout {
    let fat_sectors = u32::from_le_bytes(disk.data[36..40].try_into().unwrap()) as usize;
    Layout { fat: 32 * SECTOR, data: (32 + 2 * fat_sectors) * SECTOR }
}

/// Byte address of cluster `c` (one sector to a cluster).
fn cluster_at(disk: &MemDisk, c: u32) -> usize {
    layout(disk).data + (c as usize - 2) * SECTOR
}

/// Sets FAT entry `c` in both copies.
fn set_fat(disk: &mut MemDisk, c: u32, v: u32) {
    let l = layout(disk);
    let fat_bytes = (u32::from_le_bytes(disk.data[36..40].try_into().unwrap()) as usize) * SECTOR;
    for copy in 0..2 {
        let at = l.fat + copy * fat_bytes + 4 * c as usize;
        disk.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn get_fat(disk: &MemDisk, c: u32) -> u32 {
    let at = layout(disk).fat + 4 * c as usize;
    u32::from_le_bytes(disk.data[at..at + 4].try_into().unwrap()) & 0x0FFF_FFFF
}

/// The address of the short entry for `name` (as shown, in a folder starting at `folder`).
fn short_entry_of(disk: &MemDisk, folder: u32, short: &str) -> usize {
    let at = cluster_at(disk, folder);
    for i in 0..16 {
        let e = &disk.data[at + i * 32..at + i * 32 + 11];
        if e == short.as_bytes() {
            return at + i * 32;
        }
    }
    panic!("{short} is not in cluster {folder}");
}

fn first_cluster(disk: &MemDisk, entry: usize) -> u32 {
    let hi = u16::from_le_bytes(disk.data[entry + 20..entry + 22].try_into().unwrap());
    let lo = u16::from_le_bytes(disk.data[entry + 26..entry + 28].try_into().unwrap());
    (u32::from(hi) << 16) | u32::from(lo)
}

fn set_cluster(disk: &mut MemDisk, entry: usize, c: u32) {
    disk.data[entry + 20..entry + 22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
    disk.data[entry + 26..entry + 28].copy_from_slice(&(c as u16).to_le_bytes());
}

fn set_size(disk: &mut MemDisk, entry: usize, size: u32) {
    disk.data[entry + 28..entry + 32].copy_from_slice(&size.to_le_bytes());
}

/// A good volume to damage: in the root folder, `HELLO.TXT` (3000 bytes), `OTHER.TXT` (2000
/// bytes), and `DIR` holding `INNER.TXT` and a folder `SUB`. Unmounted cleanly.
fn good() -> MemDisk {
    let mut fs = mount(blank_with(70_000, 1, "HOSTILE"));
    put(&mut fs, "HELLO.TXT", &bytes(1, 3000)).unwrap();
    put(&mut fs, "OTHER.TXT", &bytes(2, 2000)).unwrap();
    mkdir(&mut fs, "DIR").unwrap();
    put(&mut fs, "DIR/INNER.TXT", &bytes(3, 700)).unwrap();
    mkdir(&mut fs, "DIR/SUB").unwrap();
    put(&mut fs, "DIR/SUB/DEEP.TXT", &bytes(4, 100)).unwrap();
    fs.unmount().unwrap();
    fs.into_device()
}

/// The cluster of the entry called `short` in the root folder.
fn root_cluster_of(disk: &MemDisk, short: &str) -> u32 {
    first_cluster(disk, short_entry_of(disk, 2, short))
}

// ---- Trying everything -----------------------------------------------------------------------

/// Visits what can be reached, using up at most `budget` calls, and ignores every error: all
/// that is wanted is that nothing panics or hangs. Folders that loop are cut off by `depth`.
fn walk(fs: &mut Fat<MemDisk>, folder: NodeId, depth: u32, budget: &mut u32) {
    if depth > 8 || *budget == 0 {
        return;
    }
    *budget -= 1;
    let mut entries = Vec::new();
    let mut cursor = 0;
    for _ in 0..50 {
        let r = fs.list(folder, cursor, &mut |e| {
            entries.push(e);
            entries.len() < 300
        });
        match r {
            Ok(0) | Err(_) => break,
            Ok(next) => cursor = next,
        }
    }
    for e in entries {
        let Ok(id) = fs.lookup(folder, &e.name) else { continue };
        let _ = fs.stat(id);
        match e.kind {
            Kind::Folder => walk(fs, id, depth + 1, budget),
            Kind::File => {
                let mut buf = std::vec![0u8; 5000];
                let _ = fs.read(id, 0, &mut buf);
                let _ = fs.read(id, 4000, &mut buf);
                let _ = fs.read(id, u64::from(u32::MAX) - 10, &mut buf);
            }
        }
    }
}

/// Mounts the damaged volume and tries to use it every way there is. Returns the mounted
/// filesystem (or the error), for the callers that check particular answers.
fn exercise(disk: MemDisk) {
    let Ok(mut fs) = Fat::mount(disk, clock, false) else { return };
    let root = fs.root();
    let mut budget = 400;
    walk(&mut fs, root, 0, &mut budget);
    // Changes, in whatever state the volume is in. The answers do not matter.
    let mut targets = Vec::new();
    let _ = fs.list(root, 0, &mut |e| {
        targets.push(e);
        targets.len() < 6
    });
    for e in &targets {
        if let Ok(id) = fs.lookup(root, &e.name) {
            let _ = fs.write(id, 100, &bytes(9, 3000));
            let _ = fs.set_size(id, 10);
            let _ = fs.commit(id);
            let _ = fs.set_size(id, 9000);
            let _ = fs.commit(id);
            if e.kind == Kind::Folder {
                let _ = fs.create(id, "created in damage.txt", Kind::File);
                let _ = fs.create(id, "sub", Kind::Folder);
            }
        }
        let _ = fs.rename(root, &e.name, root, "renamed", true);
        let _ = fs.remove(root, "renamed");
        let _ = fs.remove(root, &e.name);
    }
    let _ = fs.create(root, "new file.txt", Kind::File);
    let _ = fs.create(root, "new folder", Kind::Folder);
    let _ = fs.space();
    let mut budget = 400;
    walk(&mut fs, root, 0, &mut budget);
    let _ = fs.unmount();
}

fn mounted(disk: MemDisk) -> Fat<MemDisk> {
    Fat::mount(disk, clock, false).unwrap()
}

fn lookup(fs: &mut Fat<MemDisk>, path: &str) -> NodeId {
    node_at(fs, path).unwrap()
}

// ---- Crafted damage: chains --------------------------------------------------------------------

#[test]
fn a_file_chain_that_loops() {
    let mut disk = good();
    let first = root_cluster_of(&disk, "HELLO   TXT");
    let second = get_fat(&disk, first);
    set_fat(&mut disk, second, first); // first -> second -> first -> ...
    let copy = disk.data.clone();
    guarded("loop", move || {
        let mut fs = mounted(MemDisk::from(copy));
        let f = lookup(&mut fs, "HELLO.TXT");
        assert_eq!(fs.stat(f).unwrap().size, 3000, "the entry itself is readable");
        let mut buf = std::vec![0u8; 3000];
        assert_eq!(fs.read(f, 0, &mut buf), Err(FsError::Damaged), "a loop is damage, not data");
        assert_eq!(fs.write(f, 0, b"x"), Err(FsError::Damaged));
        assert_eq!(fs.set_size(f, 0), Err(FsError::Damaged));
        // The other files are unaffected.
        let g = lookup(&mut fs, "OTHER.TXT");
        assert_eq!(read_all(&mut fs, g).unwrap(), bytes(2, 2000));
        // And the damaged file can still be removed (which needs to free a chain that loops).
        let root = fs.root();
        let r = fs.remove(root, "HELLO.TXT");
        assert!(r.is_ok() || r == Err(FsError::Damaged), "{r:?}");
    });
    guarded("loop, everything", move || exercise(disk));
}

#[test]
fn a_file_chain_that_leaves_the_volume() {
    for bad in [0x0FFF_FFF0, 0x0FFF_FFF7, 0x0FFF_FF00, 1, 0] {
        let mut disk = good();
        let first = root_cluster_of(&disk, "HELLO   TXT");
        set_fat(&mut disk, first, bad);
        let copy = disk.data.clone();
        guarded("outside", move || {
            let mut fs = mounted(MemDisk::from(copy));
            let f = lookup(&mut fs, "HELLO.TXT");
            let mut buf = std::vec![0u8; 3000];
            // Within the first cluster there is data; beyond the chain's end there is none.
            let r = fs.read(f, 0, &mut buf);
            assert_eq!(r, Err(FsError::Damaged), "next = {bad:#x}");
            assert_eq!(fs.stat(f).unwrap().kind, Kind::File);
            let g = lookup(&mut fs, "OTHER.TXT");
            assert_eq!(read_all(&mut fs, g).unwrap(), bytes(2, 2000));
        });
        guarded("outside, everything", move || exercise(disk));
    }
}

#[test]
fn a_first_cluster_that_is_wrong() {
    for bad in [0, 1, 0x0FFF_FFF7, 0x0FFF_FFFF, 70_100, 0xFFFF_FFFF] {
        let mut disk = good();
        let entry = short_entry_of(&disk, 2, "HELLO   TXT");
        set_cluster(&mut disk, entry, bad & 0x0FFF_FFFF);
        // Bits 28 to 31 of the first cluster are reserved; hostile volumes set them.
        disk.data[entry + 21] |= 0xF0;
        let copy = disk.data.clone();
        guarded("first cluster", move || {
            let mut fs = mounted(MemDisk::from(copy));
            // Either the lookup already finds the entry unusable, or every use of it fails.
            let root = fs.root();
            if let Ok(f) = fs.lookup(root, "HELLO.TXT") {
                let mut buf = std::vec![0u8; 3000];
                assert!(fs.read(f, 0, &mut buf).is_err(), "first = {bad:#x}");
                assert!(fs.write(f, 0, b"x").is_err());
            }
        });
        guarded("first cluster, everything", move || exercise(disk));
    }
}

#[test]
fn a_size_that_disagrees_with_the_chain() {
    let mut disk = good();
    let entry = short_entry_of(&disk, 2, "HELLO   TXT");
    // Claims 1 MiB, has 6 clusters: reading what is there works, going past it is damage, and
    // it never reads another file's clusters instead.
    set_size(&mut disk, entry, 1 << 20);
    let copy = disk.data.clone();
    guarded("size larger", move || {
        let mut fs = mounted(MemDisk::from(copy));
        let f = lookup(&mut fs, "HELLO.TXT");
        let mut buf = std::vec![0u8; 3000];
        assert_eq!(fs.read(f, 0, &mut buf), Ok(3000));
        assert!(buf == bytes(1, 3000));
        let mut far = std::vec![0u8; 512];
        assert_eq!(fs.read(f, 3500, &mut far), Err(FsError::Damaged));
        assert_eq!(fs.read(f, 1 << 19, &mut far), Err(FsError::Damaged));
    });
    guarded("size larger, everything", move || exercise(disk));

    // Claims 10 bytes, has 6 clusters: the file is 10 bytes. Writing grows it over its own
    // clusters; the surplus is freed at the commit.
    let mut disk = good();
    let entry = short_entry_of(&disk, 2, "HELLO   TXT");
    set_size(&mut disk, entry, 10);
    guarded("size smaller", move || {
        let mut fs = mounted(disk);
        let before = fs.info().free_clusters;
        let f = lookup(&mut fs, "HELLO.TXT");
        assert_eq!(read_all(&mut fs, f).unwrap(), bytes(1, 3000)[..10]);
        fs.write(f, 10, b"more").unwrap();
        fs.commit(f).unwrap();
        assert_eq!(fs.stat(f).unwrap().size, 14);
        assert!(fs.info().free_clusters > before, "the unused clusters were freed");
        let g = lookup(&mut fs, "OTHER.TXT");
        assert_eq!(read_all(&mut fs, g).unwrap(), bytes(2, 2000));
    });

    // A file with a size and no clusters, and an empty file with clusters.
    for (size, first) in [(100u32, 0u32), (0, 2)] {
        let mut disk = good();
        let entry = short_entry_of(&disk, 2, "HELLO   TXT");
        set_size(&mut disk, entry, size);
        set_cluster(&mut disk, entry, first);
        guarded("size and chain", move || exercise(disk));
    }
}

#[test]
fn files_that_share_clusters() {
    // Two files whose chains are the same clusters. A real checker would repair this; we only
    // promise to survive it: no panic, and the files' data never leaks past their sizes.
    let mut disk = good();
    let hello = root_cluster_of(&disk, "HELLO   TXT");
    let entry = short_entry_of(&disk, 2, "OTHER   TXT");
    set_cluster(&mut disk, entry, hello);
    set_size(&mut disk, entry, 3000);
    let copy = disk.data.clone();
    guarded("cross-linked", move || {
        let mut fs = mounted(MemDisk::from(copy));
        let a = lookup(&mut fs, "HELLO.TXT");
        let b = lookup(&mut fs, "OTHER.TXT");
        assert_eq!(read_all(&mut fs, a).unwrap(), read_all(&mut fs, b).unwrap());
        let root = fs.root();
        let _ = fs.remove(root, "HELLO.TXT");
        let _ = fs.remove(root, "OTHER.TXT");
        let _ = fs.space();
    });
    guarded("cross-linked, everything", move || exercise(disk));
}

// ---- Crafted damage: folders ---------------------------------------------------------------------

#[test]
fn a_folder_chain_that_loops_or_leaves() {
    for bad in [None, Some(0x0FFF_FFF0), Some(0), Some(0x0FFF_FFF7)] {
        let mut disk = good();
        let dir = root_cluster_of(&disk, "DIR        ");
        // DIR is one cluster; make it point at itself (a loop), or elsewhere.
        set_fat(&mut disk, dir, bad.unwrap_or(dir));
        let copy = disk.data.clone();
        guarded("folder chain", move || {
            let mut fs = mounted(MemDisk::from(copy));
            let root = fs.root();
            // The root is fine and still lists.
            assert_eq!(names(&mut fs, root).len(), 3);
            let d = lookup(&mut fs, "DIR");
            let mut seen = 0;
            let r = fs.list(d, 0, &mut |_| {
                seen += 1;
                true
            });
            // A cluster that says "carry on" with nothing after it is damage; a loop must be
            // noticed (not listed for ever).
            assert!(r.is_ok() || r == Err(FsError::Damaged), "{r:?}");
            assert!(seen < 10_000);
            let r = fs.create(d, "x.txt", Kind::File);
            assert!(r.is_ok() || r.is_err());
        });
        guarded("folder chain, everything", move || exercise(disk));
    }
}

#[test]
fn folders_that_contain_each_other() {
    // DIR's entry for SUB is made to point at the root folder's cluster, and at DIR itself.
    for target in [2u32, 0] {
        let mut disk = good();
        let dir = root_cluster_of(&disk, "DIR        ");
        let target = if target == 0 { dir } else { target };
        let sub = short_entry_of(&disk, dir, "SUB        ");
        set_cluster(&mut disk, sub, target);
        let copy = disk.data.clone();
        guarded("folder cycle", move || {
            let mut fs = mounted(MemDisk::from(copy));
            let mut budget = 2000;
            let root = fs.root();
            walk(&mut fs, root, 0, &mut budget);
            // Moving a folder into itself through the cycle must not loop either.
            let d = lookup(&mut fs, "DIR");
            let s = fs.lookup(d, "SUB").unwrap();
            let r = fs.rename(root, "DIR", s, "DIR", false);
            assert!(r.is_err(), "{r:?}");
        });
        guarded("folder cycle, everything", move || exercise(disk));
    }
}

#[test]
fn a_folder_without_dot_entries_or_with_wrong_ones() {
    let mut disk = good();
    let dir = root_cluster_of(&disk, "DIR        ");
    let at = cluster_at(&disk, dir);
    // `..` becomes junk: moving the folder needs to rewrite it, and must notice.
    disk.data[at + 32..at + 43].copy_from_slice(b"NOTDOTDOT  ");
    let copy = disk.data.clone();
    guarded("bad dotdot", move || {
        let mut fs = mounted(MemDisk::from(copy));
        let root = fs.root();
        let sub = lookup(&mut fs, "DIR/SUB");
        let r = fs.rename(root, "DIR", sub, "ELSEWHERE", false);
        assert!(r.is_err());
        let r = fs.rename(root, "DIR", root, "MOVED", false);
        assert!(r.is_ok() || r.is_err());
        let _ = names(&mut fs, root);
    });
    guarded("bad dotdot, everything", move || exercise(disk));
    // The whole first cluster of a folder is zeros (no dot entries at all, end of folder).
    let mut disk = good();
    let dir = root_cluster_of(&disk, "DIR        ");
    let at = cluster_at(&disk, dir);
    disk.data[at..at + SECTOR].fill(0);
    guarded("empty folder", move || exercise(disk));
    // And all 0xFF.
    let mut disk = good();
    let dir = root_cluster_of(&disk, "DIR        ");
    let at = cluster_at(&disk, dir);
    disk.data[at..at + SECTOR].fill(0xFF);
    guarded("0xFF folder", move || exercise(disk));
}

// ---- Crafted damage: names ----------------------------------------------------------------------

/// Puts `entries` in the root folder after the existing ones, as the next free slots. Returns
/// the slot of the first.
fn append(disk: &mut MemDisk, entries: &[[u8; 32]]) -> usize {
    let at = cluster_at(disk, 2);
    let first =
        (0..16).find(|&i| disk.data[at + i * 32] == 0).expect("a free slot in the root folder");
    assert!(first + entries.len() <= 16, "fits in the one cluster");
    for (i, e) in entries.iter().enumerate() {
        disk.data[at + (first + i) * 32..at + (first + i + 1) * 32].copy_from_slice(e);
    }
    first
}

fn short(name: &[u8; 11]) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = 0x20;
    e
}

fn lfn(order: u8, text: &[u16], sum: u8) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[0] = order;
    e[11] = 0x0F;
    e[13] = sum;
    let offsets = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (i, &at) in offsets.iter().enumerate() {
        let unit = text.get(i).copied().unwrap_or(if i == text.len() { 0 } else { 0xFFFF });
        e[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    e
}

fn sum_of(name: &[u8; 11]) -> u8 {
    crate::names::checksum(name)
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn long_names_that_do_not_belong_fall_back_to_the_short_name() {
    let cases: Vec<(&str, Vec<[u8; 32]>, &str)> = {
        let s = *b"SHORT   TXT";
        let sum = sum_of(&s);
        std::vec![
            (
                "a long name with the wrong checksum",
                std::vec![lfn(0x41, &units("Long.txt"), sum ^ 1), short(&s)],
                "SHORT.TXT"
            ),
            (
                "a long name that is not marked as the last part",
                std::vec![lfn(0x01, &units("Long.txt"), sum), short(&s)],
                "SHORT.TXT"
            ),
            (
                "a long name with parts out of order",
                std::vec![
                    lfn(0x42, &units("part two"), sum),
                    lfn(0x43, &units("part three"), sum),
                    short(&s)
                ],
                "SHORT.TXT"
            ),
            (
                "a long name with a part missing",
                std::vec![lfn(0x42, &units("second"), sum), short(&s)],
                "SHORT.TXT"
            ),
            (
                "a lone surrogate",
                std::vec![lfn(0x41, &[0x61, 0xD800, 0x62], sum), short(&s)],
                "SHORT.TXT"
            ),
            ("a slash", std::vec![lfn(0x41, &units("a/b"), sum), short(&s)], "SHORT.TXT"),
            (
                "a control character",
                std::vec![lfn(0x41, &units("a\u{1}b"), sum), short(&s)],
                "SHORT.TXT"
            ),
            ("dot dot", std::vec![lfn(0x41, &units(".."), sum), short(&s)], "SHORT.TXT"),
            ("an empty long name", std::vec![lfn(0x41, &[], sum), short(&s)], "SHORT.TXT"),
            (
                "a long name, then a deleted entry",
                std::vec![lfn(0x41, &units("Gone.txt"), sum), {
                    let mut d = short(&s);
                    d[0] = 0xE5;
                    d
                }],
                ""
            ),
            (
                "a long name with the orphan before an end marker",
                std::vec![lfn(0x41, &units("Orphan.txt"), sum)],
                ""
            ),
            ("a name made only of blanks", std::vec![short(b"           ")], ""),
            ("a short name with a control character", std::vec![short(b"AB\x01 TXT    ")], ""),
        ]
    };
    for (what, entries, expect) in cases {
        let mut disk = good();
        append(&mut disk, &entries);
        let copy = disk.data.clone();
        let what_copy = what.to_string();
        let expect = expect.to_string();
        guarded(what, move || {
            let mut fs = mounted(MemDisk::from(copy));
            let root = fs.root();
            let all = names(&mut fs, root);
            // Whatever is listed can be looked up under the same name.
            for n in &all {
                assert!(fs.lookup(root, n).is_ok(), "{what_copy}: {n:?} listed but not found");
                assert!(!n.contains('/') && !n.chars().any(char::is_control), "{what_copy}: {n:?}");
            }
            if expect.is_empty() {
                assert_eq!(all.len(), 3, "{what_copy}: only the real files: {all:?}");
            } else {
                assert!(all.contains(&expect), "{what_copy}: {all:?} should hold {expect}");
                assert!(!all.iter().any(|n| n.contains("Long") || n.contains("part")), "{all:?}");
            }
        });
        guarded(what, move || exercise(disk));
    }
}

#[test]
fn the_two_name_forms_of_one_file_agree() {
    // A correct long name is used; the file is found by it and by its short alias.
    let mut disk = good();
    let s = *b"LONGNA~1TXT";
    let sum = sum_of(&s);
    append(&mut disk, &[lfn(0x41, &units("Longname.txt"), sum), short(&s)]);
    let mut fs = mounted(disk);
    let root = fs.root();
    let by_long = fs.lookup(root, "Longname.txt").unwrap();
    let by_short = fs.lookup(root, "LONGNA~1.TXT").unwrap();
    assert_eq!(by_long, by_short, "one file, one id");
    assert!(names(&mut fs, root).contains(&"Longname.txt".to_string()));
}

#[test]
fn two_entries_with_one_name() {
    let mut disk = good();
    append(&mut disk, &[short(b"HELLO   TXT"), short(b"HELLO   TXT")]);
    guarded("duplicate names", move || {
        let mut fs = mounted(disk);
        let root = fs.root();
        let _ = names(&mut fs, root);
        let a = fs.lookup(root, "HELLO.TXT").unwrap();
        // The first one found is the one used, every time.
        assert_eq!(fs.lookup(root, "HELLO.TXT"), Ok(a));
        let r = fs.create(root, "hello.txt", Kind::File);
        assert_eq!(r, Err(FsError::AlreadyExists));
    });
}

// ---- Crafted damage: the volume ----------------------------------------------------------------------

#[test]
fn boot_sectors_with_every_field_wrong() {
    // Each field in turn is set to values that are zero, huge, or an off-by-one. Mounting must
    // fail or succeed, without panicking, and a volume that does mount must be usable.
    let fields: &[(usize, usize)] = &[
        (11, 2),  // bytes per sector
        (13, 1),  // sectors per cluster
        (14, 2),  // reserved sectors
        (16, 1),  // FAT count
        (17, 2),  // root entries
        (19, 2),  // total sectors (16 bit)
        (22, 2),  // FAT size (16 bit)
        (32, 4),  // total sectors
        (36, 4),  // FAT size
        (40, 2),  // flags
        (42, 2),  // version
        (44, 4),  // root cluster
        (48, 2),  // FSInfo sector
        (50, 2),  // backup boot sector
        (510, 2), // signature
    ];
    let values: &[u64] =
        &[0, 1, 2, 3, 0x7F, 0x80, 0xFF, 0x100, 0x1000, 0xFFFF, 0x1_0000, 0x7FFF_FFFF, 0xFFFF_FFFF];
    let base = blank_with(70_000, 1, "BOOT");
    for &(at, len) in fields {
        for &v in values {
            let mut disk = MemDisk::from(base.data.clone());
            disk.data[at..at + len].copy_from_slice(&v.to_le_bytes()[..len]);
            guarded("boot sector", move || exercise(disk));
        }
    }
    // Sector 0 as other things.
    for fill in [0x00u8, 0xFF, 0x55, 0xE5] {
        let mut disk = MemDisk::from(base.data.clone());
        disk.data[..SECTOR].fill(fill);
        guarded("boot sector filled", move || exercise(disk));
    }
    // A disk with fewer sectors than the boot sector says, and a disk of 1 sector, and none.
    let short_disk = MemDisk::from(base.data[..30_000 * SECTOR].to_vec());
    assert!(Fat::mount(short_disk, clock, false).is_err());
    let one = MemDisk::from(base.data[..SECTOR].to_vec());
    assert!(Fat::mount(one, clock, false).is_err());
    assert!(Fat::mount(MemDisk::from(std::vec::Vec::new()), clock, false).is_err());
}

#[test]
fn a_damaged_fat_and_fsinfo() {
    // The root cluster's own entry says free, or bad, or points outside.
    for bad in [0u32, 0x0FFF_FFF7, 0x0FFF_FFF0, 1, 70_500] {
        let mut disk = good();
        set_fat(&mut disk, 2, bad);
        guarded("root chain", move || {
            if let Ok(fs) = Fat::mount(disk, clock, false) {
                drop(fs);
            }
        });
    }
    // FSInfo claiming nonsense never makes the filesystem believe it has room it hasn't: a
    // volume whose count says "plenty" but is full still refuses with NoSpace, not a panic.
    let mut disk = good();
    disk.data[SECTOR + 488..SECTOR + 492].copy_from_slice(&60_000u32.to_le_bytes());
    let mut fs = mounted(disk);
    let f = put(&mut fs, "grow.bin", b"").unwrap();
    let mut at = 0u64;
    let chunk = bytes(1, 65536);
    // Fills all the room there is (about 34 MB), whatever the hint said.
    loop {
        match fs.write(f, at, &chunk) {
            Ok(_) => at += 65536,
            Err(FsError::NoSpace) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(at > 30_000_000);
}

#[test]
fn a_volume_of_garbage() {
    // Whole sectors of nonsense in the FAT and in the data area.
    for seed in 1..=6u64 {
        let mut disk = good();
        let l = layout(&disk);
        let junk = bytes(seed, 40 * SECTOR);
        disk.data[l.fat..l.fat + junk.len()].copy_from_slice(&junk);
        guarded("garbage fat", move || exercise(disk));
        let mut disk = good();
        let l = layout(&disk);
        disk.data[l.data..l.data + junk.len()].copy_from_slice(&junk);
        guarded("garbage data", move || exercise(disk));
    }
}

// ---- Fuzzing ----------------------------------------------------------------------------------------------

#[test]
fn random_damage_to_a_good_volume() {
    let base = good();
    let l = layout(&base);
    let fat_bytes = (u32::from_le_bytes(base.data[36..40].try_into().unwrap()) as usize) * SECTOR;
    let image = base.data;
    guarded("fuzzing", move || {
        let mut rng = Rng(0x5EED_1234);
        for round in 0..250 {
            let mut data = image.clone();
            for _ in 0..1 + rng.below(8) {
                // Where: the boot sector and FSInfo, the start of the FAT (used entries), the
                // start of the data area (the folders and files), or anywhere at all.
                let at = match rng.below(10) {
                    0..=1 => rng.below(2 * SECTOR as u64) as usize,
                    2..=4 => l.fat + rng.below(48 * 4) as usize,
                    5..=8 => l.data + rng.below(24 * SECTOR as u64) as usize,
                    _ => rng.below(l.data as u64 + 64 * SECTOR as u64) as usize,
                };
                let at = at.min(data.len() - 1);
                // The second FAT copy too, half the time, so that damage is not outvoted.
                let value = match rng.below(4) {
                    0 => 0,
                    1 => 0xFF,
                    2 => data[at] ^ (1 << rng.below(8)),
                    _ => rng.next() as u8,
                };
                data[at] = value;
                if (l.fat..l.fat + fat_bytes).contains(&at) && rng.below(2) == 0 {
                    data[at + fat_bytes] = value;
                }
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                exercise(MemDisk::from(data));
            }));
            assert!(result.is_ok(), "round {round} panicked");
        }
    });
}

#[test]
fn random_damage_to_entries() {
    // Damage aimed at whole directory entries: any of their 32 bytes in the root folder and in
    // DIR, which is where names, sizes and clusters are.
    let base = good();
    let l = layout(&base);
    let dir = first_cluster(&base, short_entry_of(&base, 2, "DIR        "));
    let places = [l.data, cluster_at(&base, dir)];
    let image = base.data;
    guarded("fuzzing entries", move || {
        let mut rng = Rng(0xD1_E47);
        for round in 0..400 {
            let mut data = image.clone();
            for _ in 0..1 + rng.below(4) {
                let folder = places[rng.below(2) as usize];
                let slot = rng.below(8) as usize;
                let byte = rng.below(32) as usize;
                data[folder + slot * 32 + byte] = rng.next() as u8;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                exercise(MemDisk::from(data));
            }));
            assert!(result.is_ok(), "round {round} panicked");
        }
    });
}

#[test]
fn a_disk_that_fails_in_the_middle() {
    // Reads and writes that fail at every possible point are errors, never panics, and the
    // volume is still readable afterwards.
    let base = good();
    let image = base.data;
    guarded("failing disk", move || {
        for fail in (0..60).step_by(1) {
            let mut disk = MemDisk::from(image.clone());
            disk.fail_after_writes = None;
            let mut fs = mount(disk);
            let writes = fs.device().writes;
            fs.device_mut().fail_after_writes = Some(writes + fail);
            let root = fs.root();
            let _ = fs.create(root, "a new file with a long name.txt", Kind::File);
            let _ = fs.create(root, "folder", Kind::Folder);
            if let Ok(f) = fs.lookup(root, "HELLO.TXT") {
                let _ = fs.write(f, 0, &bytes(1, 9000));
                let _ = fs.commit(f);
            }
            let _ = fs.rename(root, "OTHER.TXT", root, "moved.txt", false);
            let _ = fs.remove(root, "HELLO.TXT");
            let _ = fs.unmount();
            // The disk comes back: whatever was done, the volume mounts and can be walked.
            let mut disk = fs.into_device();
            disk.fail_after_writes = None;
            let mut fs = mount(disk);
            let mut budget = 400;
            let root = fs.root();
            walk(&mut fs, root, 0, &mut budget);
        }
    });
}
