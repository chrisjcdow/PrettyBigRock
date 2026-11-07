// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What each operation does, tested on volumes made by our own formatter (no outside tools).

use super::*;
use crate::dirent::short_entry;
use crate::{MountError, same_name, valid_name};
use pbr_fs::memdisk::MemDisk;

/// The names of the short entries in the root folder, as stored (11 bytes, as text).
fn shorts(fs: &mut Fat<MemDisk>, path: &str) -> Vec<String> {
    let node = node_at(fs, path).unwrap();
    let first = fs.nodes[&node].chain.first;
    let folder = fs.vol.load_folder(first).unwrap();
    folder
        .items()
        .iter()
        .filter(|i| !i.dots)
        .map(|i| String::from_utf8_lossy(&i.short).into_owned())
        .collect()
}

/// How many 32-byte entries the folder at `path` uses (up to its end marker).
fn entries_used(fs: &mut Fat<MemDisk>, path: &str) -> u32 {
    let node = node_at(fs, path).unwrap();
    let first = fs.nodes[&node].chain.first;
    fs.vol.load_folder(first).unwrap().end()
}

#[test]
fn mounting_a_fresh_volume() {
    let mut fs = fresh();
    let info = fs.info();
    assert_eq!(info.label, "TESTVOL");
    assert_eq!(info.volume_id, 0xCAFE_F00D);
    assert_eq!(info.cluster_bytes, 512);
    assert_eq!(info.fat_copies, 2);
    assert!(info.clean_at_mount && info.used_fsinfo && !info.read_only);
    assert_eq!(info.cluster_count, info.free_clusters + 1, "the root folder uses one cluster");
    let space = fs.space().unwrap();
    assert_eq!(space.filesystem, "FAT32");
    assert_eq!(space.label, "TESTVOL");
    assert_eq!(space.total, u64::from(info.cluster_count) * 512);
    assert_eq!(space.free, u64::from(info.free_clusters) * 512);
    let root = fs.root();
    let st = fs.stat(root).unwrap();
    assert_eq!((st.kind, st.size), (Kind::Folder, 0));
    // The label entry is not listed.
    assert!(names(&mut fs, root).is_empty());
    assert_eq!(fs.lookup(root, "TESTVOL"), Err(FsError::NotFound));
    assert!(!fs.read_only());
}

#[test]
fn a_label_comes_from_the_boot_sector_without_a_label_entry() {
    let mut disk = blank();
    let root = (32 + 2 * u32::from_le_bytes(disk.data[36..40].try_into().unwrap())) as usize * 512;
    disk.data[root..root + 32].fill(0);
    let mut fs = mount(disk);
    assert_eq!(fs.space().unwrap().label, "TESTVOL");
    // And "NO NAME", which mkfs.fat writes when given no label, means none.
    let mut disk = blank();
    disk.data[root..root + 32].fill(0);
    disk.data[71..82].copy_from_slice(b"NO NAME    ");
    assert_eq!(mount(disk).space().unwrap().label, "");
}

#[test]
fn mounting_refuses_what_is_not_fat32() {
    assert!(matches!(Fat::mount(MemDisk::new(1000), clock, false), Err(MountError::NotFat)));
    assert!(matches!(Fat::mount(MemDisk::new(0), clock, false), Err(MountError::NotFat)));
    let mut disk = blank();
    disk.data[510] = 0;
    assert!(matches!(Fat::mount(disk, clock, false), Err(MountError::NotFat)));
    // A disk too small for what its boot sector says.
    let small = MemDisk::from(blank().data[..1_000_000].to_vec());
    assert!(matches!(Fat::mount(small, clock, false), Err(MountError::Damaged(_))));
}

#[test]
fn create_write_read() {
    let mut fs = fresh();
    let root = fs.root();
    let f = fs.create(root, "hello.txt", Kind::File).unwrap();
    assert_eq!(fs.stat(f).unwrap().size, 0);
    assert_eq!(fs.write(f, 0, b"hello, world").unwrap(), 12);
    assert_eq!(fs.stat(f).unwrap().size, 12);
    let mut buf = [0u8; 100];
    assert_eq!(fs.read(f, 0, &mut buf).unwrap(), 12);
    assert_eq!(&buf[..12], b"hello, world");
    // Reads at and past the end give nothing; a read in the middle gives the rest.
    assert_eq!(fs.read(f, 12, &mut buf).unwrap(), 0);
    assert_eq!(fs.read(f, 1000, &mut buf).unwrap(), 0);
    assert_eq!(fs.read(f, 7, &mut buf).unwrap(), 5);
    assert_eq!(&buf[..5], b"world");
    assert_eq!(fs.read(f, 0, &mut []).unwrap(), 0);
    // A request bigger than the limit is refused.
    let mut big = std::vec![0u8; 65537];
    assert_eq!(fs.read(f, 0, &mut big), Err(FsError::BadArgument));
    assert_eq!(fs.write(f, 0, &big), Err(FsError::BadArgument));
    // Visible at once, durable at the commit.
    fs.commit(f).unwrap();
    let mut fs = remount(fs);
    let f = node_at(&mut fs, "hello.txt").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"hello, world");
    // Committing a file with nothing to commit, or a folder, does nothing.
    fs.commit(f).unwrap();
    fs.commit(fs.root()).unwrap();
}

#[test]
fn names_match_ignoring_a_to_z_case_only() {
    let mut fs = fresh();
    let root = fs.root();
    let a = fs.create(root, "Readme.TXT", Kind::File).unwrap();
    assert_eq!(fs.lookup(root, "README.txt"), Ok(a));
    assert_eq!(fs.lookup(root, "readme.txt"), Ok(a));
    assert_eq!(fs.create(root, "readme.TXT", Kind::File), Err(FsError::AlreadyExists));
    assert_eq!(fs.create(root, "README.TXT", Kind::Folder), Err(FsError::AlreadyExists));
    // The spelling is kept as given.
    assert_eq!(names(&mut fs, root), ["Readme.TXT"]);
    // Accented letters are different letters.
    let e1 = fs.create(root, "École", Kind::File).unwrap();
    let e2 = fs.create(root, "école", Kind::File).unwrap();
    assert_ne!(e1, e2);
    assert!(same_name("ABC", "abc") && !same_name("É", "é"));
}

#[test]
fn names_fat_forbids_are_refused() {
    let mut fs = fresh();
    let root = fs.root();
    let long = "x".repeat(256);
    let bad = [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "a:b",
        "a*b",
        "a?b",
        "a\"b",
        "a<b",
        "a>b",
        "a|b",
        "a ",
        "a.",
        "tab\t",
        "nul\0",
        "del\u{7f}",
        &long,
    ];
    for name in bad {
        assert!(!valid_name(name));
        assert_eq!(fs.create(root, name, Kind::File), Err(FsError::BadName), "{name:?}");
    }
    fs.create(root, "ok", Kind::File).unwrap();
    for name in bad {
        assert_eq!(fs.rename(root, "ok", root, name, false), Err(FsError::BadName), "{name:?}");
        // A bad name can't be found either, and that is just "not found".
        assert_eq!(fs.lookup(root, name), Err(FsError::NotFound), "{name:?}");
    }
    assert_eq!(names(&mut fs, root), ["ok"]);
    // The longest names that are allowed.
    let longest = "n".repeat(255);
    fs.create(root, &longest, Kind::File).unwrap();
    let emoji = "😀".repeat(127);
    fs.create(root, &emoji, Kind::File).unwrap();
    assert!(fs.lookup(root, &longest).is_ok() && fs.lookup(root, &emoji).is_ok());
    let mut fs = remount(fs);
    let root = fs.root();
    assert!(fs.lookup(root, &longest).is_ok() && fs.lookup(root, &emoji).is_ok());
}

#[test]
fn short_names_are_made_like_windows_makes_them() {
    let mut fs = fresh();
    let root = fs.root();
    // Already a valid 8.3 name: one entry. Anything else: a long name and a made-up short one.
    for name in ["PLAIN.TXT", "Mixed.txt", "A long file name.txt", "A long file name.doc", "ab"] {
        fs.create(root, name, Kind::File).unwrap();
    }
    for i in 0..3 {
        fs.create(root, &std::format!("Another long file name {i}.txt"), Kind::File).unwrap();
    }
    assert_eq!(
        shorts(&mut fs, ""),
        [
            "PLAIN   TXT",
            "MIXED   TXT",
            "ALONGF~1TXT",
            "ALONGF~1DOC",
            "ab         ".to_uppercase().as_str(),
            "ANOTHE~1TXT",
            "ANOTHE~2TXT",
            "ANOTHE~3TXT",
        ]
    );
    // PLAIN.TXT: 1 entry; Mixed.txt: 2; A long file name.txt (20): 2 + 1; .doc: 3; ab: 2
    // ("ab" is not capitals); 3 x ("Another long file name 0.txt" is 29 units: 3 parts + 1).
    assert_eq!(entries_used(&mut fs, ""), 1 + 2 + 3 + 3 + 2 + 3 * 4 + 1 /* the label */);
    // The short name finds the file too, and counts as taken.
    let long = fs.lookup(root, "A long file name.txt").unwrap();
    assert_eq!(fs.lookup(root, "ALONGF~1.TXT"), Ok(long));
    assert_eq!(fs.lookup(root, "alongf~1.txt"), Ok(long));
    assert_eq!(fs.create(root, "alongf~1.txt", Kind::File), Err(FsError::AlreadyExists));
    // They list under their long names only.
    assert!(!names(&mut fs, root).iter().any(|n| n.contains('~')));
}

#[test]
fn writes_grow_overwrite_and_leave_gaps_of_zeros() {
    let mut fs = fresh();
    let root = fs.root();
    let f = fs.create(root, "f", Kind::File).unwrap();
    let a = bytes(1, 3000);
    fs.write(f, 0, &a).unwrap();
    // Overwrite across cluster boundaries, unaligned.
    let b = bytes(2, 1100);
    fs.write(f, 700, &b).unwrap();
    let mut want = a.clone();
    want[700..1800].copy_from_slice(&b);
    assert_eq!(read_all(&mut fs, f).unwrap(), want);
    // A write past the end leaves a gap that reads as zeros, even over clusters that held
    // other files' data before.
    fs.write(f, 5000, b"tail").unwrap();
    want.resize(5000, 0);
    want.extend_from_slice(b"tail");
    assert_eq!(read_all(&mut fs, f).unwrap(), want);
    assert_eq!(fs.stat(f).unwrap().size, 5004);
    // Appending in many small pieces.
    for i in 0..40u8 {
        fs.write(f, 5004 + u64::from(i) * 10, &[i; 10]).unwrap();
        want.extend_from_slice(&[i; 10]);
    }
    assert_eq!(read_all(&mut fs, f).unwrap(), want);
    fs.commit(f).unwrap();
    let mut fs = remount(fs);
    let f = node_at(&mut fs, "f").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), want);
}

#[test]
fn growing_never_shows_old_data() {
    let mut fs = fresh();
    let root = fs.root();
    // Fill clusters with a pattern, free them, and let a new file take them.
    put(&mut fs, "old", &[0xAA; 20_000]).unwrap();
    fs.remove(root, "old").unwrap();
    let f = fs.create(root, "new", Kind::File).unwrap();
    fs.set_size(f, 10_000).unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), std::vec![0u8; 10_000]);
    let g = fs.create(root, "gap", Kind::File).unwrap();
    fs.write(g, 9_999, b"x").unwrap();
    let got = read_all(&mut fs, g).unwrap();
    assert!(got[..9_999].iter().all(|&b| b == 0) && got[9_999] == b'x');
    // Cut a file and grow it again: what was cut off does not come back.
    let h = put(&mut fs, "cut", &[0xBB; 3_000]).unwrap();
    fs.set_size(h, 100).unwrap();
    fs.set_size(h, 3_000).unwrap();
    let got = read_all(&mut fs, h).unwrap();
    assert!(got[..100].iter().all(|&b| b == 0xBB) && got[100..].iter().all(|&b| b == 0));
    // The same after the cut has been committed, and with a write past the new end.
    fs.set_size(h, 700).unwrap();
    fs.commit(h).unwrap();
    fs.write(h, 2_000, b"z").unwrap();
    let got = read_all(&mut fs, h).unwrap();
    assert!(got[..100].iter().all(|&b| b == 0xBB));
    assert!(got[100..2_000].iter().all(|&b| b == 0) && got[2_000] == b'z');
    // Cut to the middle of a cluster, grown by a few bytes: the rest of the cluster is zeros.
    let k = put(&mut fs, "mid", &[0xCC; 1_000]).unwrap();
    fs.set_size(k, 600).unwrap();
    fs.commit(k).unwrap();
    fs.set_size(k, 601).unwrap();
    assert_eq!(read_all(&mut fs, k).unwrap()[599..], [0xCC, 0]);
}

#[test]
fn cutting_and_growing_by_set_size() {
    let mut fs = fresh();
    let before = fs.info().free_clusters;
    let f = put(&mut fs, "f", &bytes(5, 10_000)).unwrap();
    let used = before - fs.info().free_clusters;
    assert_eq!(used, 20, "10000 bytes need 20 clusters of 512");
    // A cut only changes the file in memory until the commit; the clusters are then freed.
    fs.set_size(f, 1000).unwrap();
    assert_eq!(fs.stat(f).unwrap().size, 1000);
    assert_eq!(before - fs.info().free_clusters, 20);
    fs.commit(f).unwrap();
    assert_eq!(before - fs.info().free_clusters, 2);
    assert_eq!(read_all(&mut fs, f).unwrap(), bytes(5, 10_000)[..1000]);
    // Cut to nothing and the file has no cluster at all.
    fs.set_size(f, 0).unwrap();
    fs.commit(f).unwrap();
    assert_eq!(before, fs.info().free_clusters);
    assert_eq!(read_all(&mut fs, f).unwrap(), b"");
    // Cutting and growing again before a commit reuses the clusters.
    fs.write(f, 0, &bytes(6, 4000)).unwrap();
    fs.commit(f).unwrap();
    let free = fs.info().free_clusters;
    fs.set_size(f, 0).unwrap();
    fs.write(f, 0, &bytes(7, 4000)).unwrap();
    assert_eq!(fs.info().free_clusters, free, "the same clusters are reused");
    fs.commit(f).unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), bytes(7, 4000));
    let mut fs = remount(fs);
    let f = node_at(&mut fs, "f").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), bytes(7, 4000));
    assert_eq!(fs.info().free_clusters, free);
    // Same size: nothing happens.
    fs.set_size(f, 4000).unwrap();
}

#[test]
fn files_are_at_most_4_gib_minus_one() {
    let mut fs = fresh();
    let root = fs.root();
    let f = fs.create(root, "f", Kind::File).unwrap();
    assert_eq!(fs.write(f, 0xFFFF_FFFF, b"x"), Err(FsError::TooBig));
    assert_eq!(fs.write(f, u64::MAX, b"x"), Err(FsError::TooBig));
    assert_eq!(fs.write(f, u64::MAX - 1, b"xx"), Err(FsError::TooBig));
    assert_eq!(fs.set_size(f, 1 << 32), Err(FsError::TooBig));
    assert_eq!(fs.set_size(f, u64::MAX), Err(FsError::TooBig));
    assert_eq!(fs.stat(f).unwrap().size, 0);
    // A file that cannot fit the volume is refused without changing anything.
    let free = fs.info().free_clusters;
    assert_eq!(fs.set_size(f, 0x7FFF_0000), Err(FsError::NoSpace));
    assert_eq!(fs.info().free_clusters, free);
    assert_eq!(fs.stat(f).unwrap().size, 0);
}

#[test]
fn folders() {
    let mut fs = fresh();
    let root = fs.root();
    let d = fs.create(root, "docs", Kind::Folder).unwrap();
    let e = fs.create(d, "deep folder", Kind::Folder).unwrap();
    put(&mut fs, "docs/deep folder/note.txt", b"hi").unwrap();
    assert_eq!(fs.stat(d).unwrap().kind, Kind::Folder);
    assert_eq!(names(&mut fs, d), ["deep folder"]);
    assert_eq!(names(&mut fs, e), ["note.txt"]);
    // `.` and `..` are not listed, and cannot be looked up.
    assert_eq!(fs.lookup(d, "."), Err(FsError::NotFound));
    assert_eq!(fs.lookup(d, ".."), Err(FsError::NotFound));
    // Files are not folders and folders are not files.
    let f = node_at(&mut fs, "docs/deep folder/note.txt").unwrap();
    assert_eq!(fs.lookup(f, "x"), Err(FsError::NotAFolder));
    assert_eq!(fs.create(f, "x", Kind::File), Err(FsError::NotAFolder));
    assert_eq!(names_error(&mut fs, f), FsError::NotAFolder);
    assert_eq!(fs.read(d, 0, &mut [0u8; 4]), Err(FsError::NotAFile));
    assert_eq!(fs.write(d, 0, b"x"), Err(FsError::NotAFile));
    assert_eq!(fs.set_size(d, 5), Err(FsError::NotAFile));
    // A folder is only removed when empty.
    assert_eq!(fs.remove(root, "docs"), Err(FsError::FolderNotEmpty));
    assert_eq!(fs.remove(d, "deep folder"), Err(FsError::FolderNotEmpty));
    fs.remove(e, "note.txt").unwrap();
    fs.remove(d, "deep folder").unwrap();
    fs.remove(root, "docs").unwrap();
    assert!(names(&mut fs, root).is_empty());
    assert_eq!(fs.remove(root, "docs"), Err(FsError::NotFound));
    // All clusters came back.
    let info = fs.info();
    assert_eq!(info.cluster_count, info.free_clusters + 1);
}

fn names_error(fs: &mut Fat<MemDisk>, node: NodeId) -> FsError {
    fs.list(node, 0, &mut |_| true).unwrap_err()
}

#[test]
fn a_new_folder_has_dot_and_dotdot() {
    let mut fs = fresh();
    let root = fs.root();
    let a = fs.create(root, "a", Kind::Folder).unwrap();
    let b = fs.create(a, "b", Kind::Folder).unwrap();
    let a_first = fs.nodes[&a].chain.first;
    let b_first = fs.nodes[&b].chain.first;
    // Read the raw entries: `.` points at itself, `..` at the parent, and at 0 for the root.
    let entry = |fs: &mut Fat<MemDisk>, first: u32, n: usize| {
        let folder = fs.vol.load_folder(first).unwrap();
        let e = *folder.entry(n as u32).unwrap();
        let cluster = u32::from(u16::from_le_bytes([e[20], e[21]])) << 16
            | u32::from(u16::from_le_bytes([e[26], e[27]]));
        (e[..11].to_vec(), e[11], cluster)
    };
    assert_eq!(entry(&mut fs, a_first, 0), (b".          ".to_vec(), 0x10, a_first));
    assert_eq!(entry(&mut fs, a_first, 1), (b"..         ".to_vec(), 0x10, 0));
    assert_eq!(entry(&mut fs, b_first, 0), (b".          ".to_vec(), 0x10, b_first));
    assert_eq!(entry(&mut fs, b_first, 1), (b"..         ".to_vec(), 0x10, a_first));
    // The rest of the cluster is empty, so the folder ends there.
    let folder = fs.vol.load_folder(b_first).unwrap();
    assert_eq!(folder.end(), 2);
}

#[test]
fn a_folder_grows_by_clusters() {
    let mut fs = fresh();
    let root = fs.root();
    let d = fs.create(root, "many", Kind::Folder).unwrap();
    // 512-byte clusters hold 16 entries: 200 files with 2-entry names need several clusters.
    for i in 0..200 {
        put(&mut fs, &std::format!("many/File number {i}"), &[i as u8; 10]).unwrap();
    }
    let first = fs.nodes[&d].chain.first;
    assert!(fs.vol.check_chain(first).unwrap() > 10);
    let mut fs = remount(fs);
    let d = node_at(&mut fs, "many").unwrap();
    assert_eq!(names(&mut fs, d).len(), 200);
    for i in 0..200 {
        let f = node_at(&mut fs, &std::format!("many/File number {i}")).unwrap();
        assert_eq!(read_all(&mut fs, f).unwrap(), [i as u8; 10]);
    }
    // Remove half; a new name reuses the freed entries without growing the folder.
    let d = node_at(&mut fs, "many").unwrap();
    let before = fs.vol.check_chain(fs.nodes[&d].chain.first).unwrap();
    for i in (0..200).step_by(2) {
        fs.remove(d, &std::format!("File number {i}")).unwrap();
    }
    for i in 0..50 {
        put(&mut fs, &std::format!("many/Again {i}"), b"x").unwrap();
    }
    assert_eq!(fs.vol.check_chain(fs.nodes[&d].chain.first).unwrap(), before);
    assert_eq!(names(&mut fs, d).len(), 150);
}

#[test]
fn a_folder_has_at_most_65536_entries() {
    let mut fs = fresh();
    let root = fs.root();
    let d = fs.create(root, "full", Kind::Folder).unwrap();
    // Making 65534 files one by one would take for ever (each creation reads the folder), so
    // the folder is filled in one go, below the filesystem's interface.
    let first = fs.nodes[&d].chain.first;
    let mut folder = fs.vol.load_folder(first).unwrap();
    while folder.slots() < 65_536 {
        fs.vol.grow_folder(&mut folder).unwrap();
    }
    assert_eq!(fs.vol.grow_folder(&mut folder), Err(FsError::NoSpace), "the limit");
    let entries: Vec<[u8; 32]> = (2..65_536u32)
        .map(|i| {
            short_entry(std::format!("F{i:07}   ").as_bytes().try_into().unwrap(), 0x20, 0, 0, 0)
        })
        .collect();
    assert_eq!(folder.place(2, &entries).unwrap(), 65_534);
    fs.vol.write_slots(&folder, 2, 65_534).unwrap();
    fs.vol.flush().unwrap();
    let free = fs.info().free_clusters;
    assert_eq!(fs.create(d, "one more", Kind::File), Err(FsError::NoSpace));
    assert_eq!(fs.create(d, "x", Kind::Folder), Err(FsError::NoSpace));
    assert_eq!(fs.info().free_clusters, free, "nothing was kept by the failures");
    // Making room makes room.
    fs.remove(d, "F0000007").unwrap();
    // (A name in capitals needs one entry; "again" would need two.)
    assert_eq!(fs.create(d, "again", Kind::File), Err(FsError::NoSpace));
    fs.create(d, "AGAIN", Kind::File).unwrap();
    let mut fs = remount(fs);
    let d = node_at(&mut fs, "full").unwrap();
    assert_eq!(list_all(&mut fs, d).len(), 65_534);
}

#[test]
fn rename_within_a_folder_and_across() {
    let mut fs = fresh();
    let root = fs.root();
    let a = mkdir(&mut fs, "a").unwrap();
    let b = mkdir(&mut fs, "b").unwrap();
    let f = put(&mut fs, "a/File one.txt", b"one").unwrap();
    // Within: the node stays the same, whatever the new name's kind (long, short).
    fs.rename(a, "File one.txt", a, "renamed.txt", false).unwrap();
    assert_eq!(fs.lookup(a, "renamed.txt"), Ok(f));
    assert_eq!(fs.lookup(a, "File one.txt"), Err(FsError::NotFound));
    fs.rename(a, "renamed.txt", a, "SHORT.TXT", false).unwrap();
    assert_eq!(names(&mut fs, a), ["SHORT.TXT"]);
    let first = fs.nodes[&a].chain.first;
    assert_eq!(fs.vol.load_folder(first).unwrap().items().len(), 3, "dot, dotdot, and the file");
    // Across: to another folder, with a longer name.
    fs.rename(a, "SHORT.TXT", b, "A much longer name than before.txt", false).unwrap();
    assert!(names(&mut fs, a).is_empty());
    assert_eq!(names(&mut fs, b), ["A much longer name than before.txt"]);
    assert_eq!(fs.lookup(b, "A much longer name than before.txt"), Ok(f));
    assert_eq!(read_all(&mut fs, f).unwrap(), b"one");
    // The node keeps working: it is committed at its new place.
    fs.write(f, 3, b" more").unwrap();
    fs.commit(f).unwrap();
    let mut fs = remount(fs);
    let f = node_at(&mut fs, "b/A much longer name than before.txt").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"one more");
    // Only capitals changed.
    let b = node_at(&mut fs, "b").unwrap();
    fs.rename(
        b,
        "a MUCH longer name than before.txt",
        b,
        "a Much LONGER name than before.txt",
        false,
    )
    .unwrap();
    assert_eq!(names(&mut fs, b), ["a Much LONGER name than before.txt"]);
    assert_eq!(fs.lookup(b, "A MUCH LONGER NAME THAN BEFORE.TXT"), Ok(f));
    // To itself, exactly: nothing to do.
    fs.rename(
        b,
        "a Much LONGER name than before.txt",
        b,
        "a Much LONGER name than before.txt",
        false,
    )
    .unwrap();
    // To the root.
    fs.rename(b, "a Much LONGER name than before.txt", root, "x", false).unwrap();
    assert_eq!(fs.lookup(root, "x"), Ok(f));
    assert_eq!(fs.rename(b, "nothing", root, "y", false), Err(FsError::NotFound));
}

#[test]
fn rename_that_would_replace() {
    let mut fs = fresh();
    let root = fs.root();
    let d = mkdir(&mut fs, "d").unwrap();
    let new = put(&mut fs, "new.txt", b"NEW").unwrap();
    let old = put(&mut fs, "d/old.txt", b"OLD OLD OLD").unwrap();
    let free = fs.info().free_clusters;
    // Not allowed unless asked, and never over a folder.
    assert_eq!(fs.rename(root, "new.txt", d, "old.txt", false), Err(FsError::AlreadyExists));
    assert_eq!(fs.rename(root, "new.txt", root, "d", true), Err(FsError::AlreadyExists));
    assert_eq!(fs.rename(root, "d", root, "NEW.TXT", false), Err(FsError::AlreadyExists));
    assert_eq!(fs.info().free_clusters, free);
    // Asked: the old file is gone, the new one has its name.
    fs.rename(root, "new.txt", d, "OLD.TXT", true).unwrap();
    assert_eq!(fs.lookup(d, "old.txt"), Ok(new));
    assert_eq!(names(&mut fs, d), ["OLD.TXT"]);
    assert_eq!(fs.stat(old), Err(FsError::Gone));
    assert_eq!(fs.read(old, 0, &mut [0u8; 4]), Err(FsError::Gone));
    assert_eq!(read_all(&mut fs, new).unwrap(), b"NEW");
    assert_eq!(fs.info().free_clusters, free + 1, "the old file's cluster was freed");
    // A folder may replace a file.
    put(&mut fs, "target", b"t").unwrap();
    let sub = mkdir(&mut fs, "sub").unwrap();
    fs.rename(root, "sub", root, "target", true).unwrap();
    assert_eq!(fs.lookup(root, "target"), Ok(sub));
    assert_eq!(fs.stat(sub).unwrap().kind, Kind::Folder);
    // Replacing a file within a folder, where the new name is only a different spelling.
    put(&mut fs, "one.txt", b"1").unwrap();
    let two = put(&mut fs, "Two.txt", b"22").unwrap();
    fs.rename(root, "Two.txt", root, "ONE.TXT", true).unwrap();
    assert_eq!(fs.lookup(root, "one.txt"), Ok(two));
    let mut fs = remount(fs);
    let tree = tree_of(&mut fs);
    assert_eq!(tree["/ONE.TXT"], Some(b"22".to_vec()));
    assert!(!tree.contains_key("/Two.txt") && !tree.contains_key("/one.txt"));
}

#[test]
fn moving_folders() {
    let mut fs = fresh();
    let root = fs.root();
    let a = mkdir(&mut fs, "a").unwrap();
    let b = mkdir(&mut fs, "a/b").unwrap();
    let c = mkdir(&mut fs, "a/b/c").unwrap();
    let z = mkdir(&mut fs, "z").unwrap();
    put(&mut fs, "a/b/c/file", b"deep").unwrap();
    // Not into itself or below.
    assert_eq!(fs.rename(root, "a", a, "a2", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(root, "a", b, "a2", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(root, "a", c, "a2", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(a, "b", c, "b2", false), Err(FsError::BadArgument));
    // Beside is fine, and the moved folder's `..` follows it: check it through the raw entry.
    fs.rename(a, "b", z, "moved", false).unwrap();
    assert_eq!(fs.lookup(z, "moved"), Ok(b));
    let dotdot = |fs: &mut Fat<MemDisk>, folder: NodeId| {
        let first = fs.nodes[&folder].chain.first;
        let f = fs.vol.load_folder(first).unwrap();
        let e = *f.entry(1).unwrap();
        assert_eq!(&e[..11], b"..         ");
        u32::from(u16::from_le_bytes([e[20], e[21]])) << 16
            | u32::from(u16::from_le_bytes([e[26], e[27]]))
    };
    assert_eq!(dotdot(&mut fs, b), fs.nodes[&z].chain.first);
    // And a folder moved to the root has `..` = 0.
    fs.rename(z, "moved", root, "top", false).unwrap();
    assert_eq!(dotdot(&mut fs, b), 0);
    // Now the old parent can be moved into it... no: `top` is below `a`'s sibling only.
    fs.rename(root, "a", b, "inside", false).unwrap();
    assert_eq!(dotdot(&mut fs, a), fs.nodes[&b].chain.first);
    let mut fs = remount(fs);
    let tree = tree_of(&mut fs);
    assert_eq!(tree["/top/c/file"], Some(b"deep".to_vec()));
    assert!(tree.contains_key("/top/inside") && tree.contains_key("/z"));
}

#[test]
fn node_ids_follow_the_rules() {
    let mut fs = fresh();
    let root = fs.root();
    assert_eq!(fs.stat(12345), Err(FsError::Gone));
    let a = fs.create(root, "a", Kind::File).unwrap();
    assert_ne!(a, root);
    // Two lookups of one entry give one id, also after the entry was created through `create`.
    assert_eq!(fs.lookup(root, "a"), Ok(a));
    assert_eq!(fs.lookup(root, "A"), Ok(a));
    // Removing makes the id Gone, for everything, and it is never given to anything else.
    fs.remove(root, "a").unwrap();
    assert_eq!(fs.stat(a), Err(FsError::Gone));
    assert_eq!(fs.read(a, 0, &mut [0u8; 1]), Err(FsError::Gone));
    assert_eq!(fs.write(a, 0, b"x"), Err(FsError::Gone));
    assert_eq!(fs.set_size(a, 1), Err(FsError::Gone));
    assert_eq!(fs.commit(a), Err(FsError::Gone));
    assert_eq!(fs.lookup(a, "x"), Err(FsError::Gone));
    assert_eq!(fs.list(a, 0, &mut |_| true), Err(FsError::Gone));
    assert_eq!(fs.create(a, "x", Kind::File), Err(FsError::Gone));
    let mut seen = std::vec![root, a];
    for i in 0..20 {
        let n = fs.create(root, "a", Kind::File).unwrap();
        assert!(!seen.contains(&n), "{i}");
        seen.push(n);
        fs.remove(root, "a").unwrap();
    }
    // A removed folder, and a file in it.
    let d = mkdir(&mut fs, "d").unwrap();
    let f = fs.create(d, "f", Kind::File).unwrap();
    fs.remove(d, "f").unwrap();
    fs.remove(root, "d").unwrap();
    assert_eq!((fs.stat(d), fs.stat(f)), (Err(FsError::Gone), Err(FsError::Gone)));
    // A rename keeps the id; the id of a folder survives its children's renames.
    let d = mkdir(&mut fs, "d2").unwrap();
    let f = fs.create(d, "f", Kind::File).unwrap();
    fs.rename(d, "f", root, "g", false).unwrap();
    fs.rename(root, "d2", root, "d3", false).unwrap();
    assert_eq!((fs.lookup(root, "g"), fs.lookup(root, "d3")), (Ok(f), Ok(d)));
}

#[test]
fn listing_with_cursors() {
    let mut fs = fresh();
    let root = fs.root();
    for i in 0..25 {
        put(&mut fs, &std::format!("A name long enough for a long entry {i}"), &[i as u8]).unwrap();
    }
    mkdir(&mut fs, "folder").unwrap();
    // One at a time: the cursor is never 0 until the end, and nothing repeats or is missed.
    let mut seen = Vec::new();
    let mut cursor = 0;
    loop {
        let mut taken = false;
        cursor = fs
            .list(root, cursor, &mut |e| {
                if taken {
                    return false;
                }
                taken = true;
                seen.push(e);
                true
            })
            .unwrap();
        if cursor == 0 {
            break;
        }
    }
    let all = list_all(&mut fs, root);
    assert_eq!(seen, all);
    assert_eq!(all.len(), 26);
    assert_eq!(all.iter().filter(|e| e.kind == Kind::Folder).count(), 1);
    // A cursor for an entry that is removed meanwhile gives the entries after it, no more.
    let mut first = None;
    let cursor = fs
        .list(root, 0, &mut |e| {
            if first.is_some() {
                return false;
            }
            first = Some(e.name);
            true
        })
        .unwrap();
    assert_ne!(cursor, 0);
    fs.remove(root, &first.unwrap()).unwrap();
    let mut rest = Vec::new();
    fs.list(root, cursor, &mut |e| {
        rest.push(e.name);
        true
    })
    .unwrap();
    assert_eq!(rest.len(), 25, "everything from the second entry on");
    // A cursor far beyond the folder is just the end.
    assert_eq!(fs.list(root, u64::MAX, &mut |_| true).unwrap(), 0);
    assert_eq!(fs.list(root, 1 << 40, &mut |_| panic!("nothing is there")).unwrap(), 0);
}

#[test]
fn listing_shows_uncommitted_sizes() {
    let mut fs = fresh();
    let root = fs.root();
    let f = fs.create(root, "f.bin", Kind::File).unwrap();
    fs.write(f, 0, &[1u8; 777]).unwrap();
    let before = fs.stat(f).unwrap().modified;
    assert_eq!(list_all(&mut fs, root)[0].size, 777);
    assert_eq!(list_all(&mut fs, root)[0].modified, before);
}

#[test]
fn times() {
    let mut fs = fresh();
    let root = fs.root();
    let f = fs.create(root, "t", Kind::File).unwrap();
    let st = fs.stat(f).unwrap();
    assert!(st.created >= 1_800_000_000 && st.modified >= st.created.saturating_sub(1));
    assert_eq!(st.modified % 2, 0, "modification times are in two-second steps");
    let first = st.modified;
    fs.write(f, 0, b"x").unwrap();
    let st2 = fs.stat(f).unwrap();
    assert!(st2.modified > first);
    assert_eq!(st2.created, st.created);
    fs.commit(f).unwrap();
    let mut fs = remount(fs);
    let f = node_at(&mut fs, "t").unwrap();
    let st3 = fs.stat(f).unwrap();
    assert_eq!((st3.created, st3.modified), (st2.created, st2.modified));
    // The root has no times.
    let r = fs.stat(fs.root()).unwrap();
    assert_eq!((r.created, r.modified, r.hidden), (0, 0, false));
}

#[test]
fn hidden_files_are_reported() {
    let mut fs = fresh();
    put(&mut fs, "visible.txt", b"v").unwrap();
    put(&mut fs, "secret.txt", b"s").unwrap();
    let first = fs.vol.geo.root;
    let slot = fs
        .vol
        .load_folder(first)
        .unwrap()
        .items()
        .iter()
        .find(|i| i.name.as_deref() == Some("secret.txt"))
        .unwrap()
        .slot;
    fs.unmount().unwrap();
    // Mark the entry hidden on the disk, as `attrib +h` would.
    let mut disk = fs.into_device();
    let fat_sectors = u32::from_le_bytes(disk.data[36..40].try_into().unwrap());
    let at = (32 + 2 * fat_sectors) as usize * 512 + slot as usize * 32;
    disk.data[at + 11] |= 0x02;
    let mut fs = mount(disk);
    let root = fs.root();
    let secret = fs.lookup(root, "secret.txt").unwrap();
    let visible = fs.lookup(root, "visible.txt").unwrap();
    assert!(fs.stat(secret).unwrap().hidden);
    assert!(!fs.stat(visible).unwrap().hidden);
    let hidden: Vec<_> =
        list_all(&mut fs, root).iter().filter(|e| e.hidden).map(|e| e.name.clone()).collect();
    assert_eq!(hidden, ["secret.txt"]);
}

#[test]
fn a_read_only_mount_changes_nothing() {
    let mut fs = fresh();
    put(&mut fs, "f", b"data").unwrap();
    mkdir(&mut fs, "d").unwrap();
    fs.unmount().unwrap();
    let disk = fs.into_device();
    let image = disk.data.clone();
    let mut fs = Fat::mount(disk, clock, true).unwrap();
    assert!(fs.read_only() && fs.info().read_only);
    let root = fs.root();
    let f = fs.lookup(root, "f").unwrap();
    let d = fs.lookup(root, "d").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"data");
    assert_eq!(fs.create(root, "x", Kind::File), Err(FsError::ReadOnly));
    assert_eq!(fs.create(root, "!", Kind::File), Err(FsError::ReadOnly));
    assert_eq!(fs.remove(root, "f"), Err(FsError::ReadOnly));
    assert_eq!(fs.rename(root, "f", d, "g", false), Err(FsError::ReadOnly));
    assert_eq!(fs.write(f, 0, b"x"), Err(FsError::ReadOnly));
    assert_eq!(fs.set_size(f, 0), Err(FsError::ReadOnly));
    fs.commit(f).unwrap();
    fs.unmount().unwrap();
    assert_eq!(fs.into_device().data, image, "not one byte changed");
    // A read-only device is the same as asking for read-only.
    let mut disk = MemDisk::from(image);
    disk.read_only = true;
    let fs = Fat::mount(disk, clock, false).unwrap();
    assert!(fs.read_only());
}

#[test]
fn a_failed_write_stops_further_changes() {
    let mut fs = fresh();
    let root = fs.root();
    let f = put(&mut fs, "f", b"data").unwrap();
    let writes = fs.device().writes;
    fs.device_mut().fail_after_writes = Some(writes);
    assert_eq!(fs.create(root, "g", Kind::File), Err(FsError::Disk));
    fs.device_mut().fail_after_writes = None;
    // The disk works again, but the picture of it may be wrong: no more changes.
    assert_eq!(fs.create(root, "h", Kind::File), Err(FsError::Disk));
    assert_eq!(fs.write(f, 0, b"x"), Err(FsError::Disk));
    assert_eq!(fs.remove(root, "f"), Err(FsError::Disk));
    assert_eq!(fs.unmount(), Err(FsError::Disk));
    // Reading still works, and the volume was not marked clean.
    assert_eq!(read_all(&mut fs, f).unwrap(), b"data");
    let fs = Fat::mount(fs.into_device(), clock, false).unwrap();
    assert!(!fs.info().clean_at_mount);
}

#[test]
fn other_cluster_sizes() {
    for (spc, total) in [(2u8, 140_000u32), (8, 540_000)] {
        let mut fs = mount(blank_with(total, spc, "SIZES"));
        assert_eq!(fs.info().cluster_bytes, u32::from(spc) * 512);
        let root = fs.root();
        mkdir(&mut fs, "d").unwrap();
        let data = bytes(u64::from(spc), 100_000);
        put(&mut fs, "d/big file.bin", &data).unwrap();
        put(&mut fs, "small", b"s").unwrap();
        let free = fs.info().free_clusters;
        let mut fs = remount(fs);
        let f = node_at(&mut fs, "d/big file.bin").unwrap();
        assert_eq!(read_all(&mut fs, f).unwrap(), data);
        assert_eq!(fs.info().free_clusters, free);
        let space = fs.space().unwrap();
        assert_eq!(space.free, u64::from(free) * 512 * u64::from(spc));
        fs.remove(root, "small").unwrap();
    }
}
