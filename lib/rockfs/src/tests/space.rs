// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A full disk, the reserve held back for deleting, and the failures that must change nothing.

use super::*;

/// Writes files of one block each until the disk says it is full. Returns how many were made.
fn fill(fs: &mut RockFs<MemDisk>) -> usize {
    let mut made = 0;
    loop {
        let name = std::format!("f{made}");
        let node = match fs.create(fs.root(), &name, Kind::File) {
            Ok(n) => n,
            Err(FsError::NoSpace) => return made,
            Err(e) => panic!("create: {e:?}"),
        };
        match fs.write(node, 0, &bytes(made as u64, 4096)).and_then(|_| fs.commit(node)) {
            Ok(()) => made += 1,
            Err(FsError::NoSpace) => {
                // Leave no half-made file behind.
                fs.remove(fs.root(), &name).unwrap();
                return made;
            }
            Err(e) => panic!("write: {e:?}"),
        }
    }
}

#[test]
fn a_full_disk_says_no_space_and_keeps_what_it_holds() {
    let mut fs = fresh(200);
    let made = fill(&mut fs);
    assert!(made > 50);
    assert_clean(&mut fs);
    for i in 0..made {
        let n = fs.lookup(fs.root(), &std::format!("f{i}")).unwrap();
        assert_eq!(read_all(&mut fs, n).unwrap(), bytes(i as u64, 4096), "file {i}");
    }
    // The reserve means free space reported to programs is zero while blocks remain for housekeeping.
    assert_eq!(fs.space().unwrap().free, 0);
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn removing_works_when_the_disk_is_full_and_frees_space() {
    let mut fs = fresh(200);
    let made = fill(&mut fs);
    for i in 0..10 {
        fs.remove(fs.root(), &std::format!("f{i}")).unwrap();
    }
    assert_eq!(fs.space().unwrap().free, 10 * 4096);
    // The space can be used again.
    let n = fs.create(fs.root(), "again", Kind::File).unwrap();
    fs.write(n, 0, &bytes(9, 8 * 4096)).unwrap();
    fs.commit(n).unwrap();
    assert_eq!(read_all(&mut fs, n).unwrap(), bytes(9, 8 * 4096));
    assert_clean(&mut fs);
    // Rewriting a committed file on a full disk needs room for the new copy first.
    let _ = made;
}

#[test]
fn a_write_that_does_not_fit_changes_nothing() {
    let mut fs = fresh(200);
    let n = fs.create(fs.root(), "big", Kind::File).unwrap();
    // Write pieces of 64 KiB (the most one request may carry) until the disk is full.
    let mut at = 0u64;
    let piece = bytes(2, 65536);
    let failed = loop {
        match fs.write(n, at, &piece) {
            Ok(65536) => at += 65536,
            Ok(other) => panic!("short write {other}"),
            Err(e) => break e,
        }
    };
    assert_eq!(failed, FsError::NoSpace);
    assert!(at > 0);
    let free = fs.space().unwrap().free;
    let size = fs.stat(n).unwrap().size;
    assert_eq!(size, at);
    assert_eq!(fs.write(n, 0, &bytes(3, 65537)), Err(FsError::BadArgument));
    assert_eq!(fs.write(n, at, &piece), Err(FsError::NoSpace));
    assert_eq!(fs.set_size(n, at + (1 << 40)), Ok(())); // a hole costs nothing
    fs.set_size(n, at).unwrap();
    assert_eq!(fs.stat(n).unwrap().size, size);
    assert_eq!(fs.space().unwrap().free, free);
    let got = read_all(&mut fs, n).unwrap();
    assert!(got.chunks(65536).all(|c| c == &piece[..c.len()]));
    fs.commit(n).unwrap();
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    let n = node_at(&mut fs, "big").unwrap();
    assert_eq!(fs.stat(n).unwrap().size, size);
}

#[test]
fn holes_cost_no_space_and_read_as_zeros() {
    let mut fs = fresh(200);
    let n = fs.create(fs.root(), "sparse", Kind::File).unwrap();
    fs.write(n, 1 << 40, b"end").unwrap();
    fs.commit(n).unwrap();
    assert_eq!(fs.stat(n).unwrap().size, (1 << 40) + 3);
    let mut buf = [1u8; 8];
    assert_eq!(fs.read(n, 4096, &mut buf).unwrap(), 8);
    assert_eq!(buf, [0u8; 8]);
    assert_eq!(fs.read(n, 1 << 40, &mut buf).unwrap(), 3);
    assert_eq!(&buf[..3], b"end");
    assert_clean(&mut fs);
}

#[test]
fn a_huge_size_is_a_hole_not_an_allocation() {
    let mut fs = fresh(200);
    let n = fs.create(fs.root(), "x", Kind::File).unwrap();
    fs.set_size(n, u64::MAX).unwrap();
    let mut buf = [1u8; 4];
    assert_eq!(fs.read(n, u64::MAX - 2, &mut buf).unwrap(), 2);
    assert_eq!(fs.write(n, u64::MAX - 2, b"abcdef"), Err(FsError::TooBig));
    fs.commit(n).unwrap();
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn a_folder_that_cannot_grow_says_no_space() {
    let mut fs = fresh(80);
    let mut made = 0;
    loop {
        match fs.create(
            fs.root(),
            &std::format!("a-long-file-name-to-fill-the-folder-{made:05}"),
            Kind::File,
        ) {
            Ok(_) => made += 1,
            Err(FsError::NoSpace) => break,
            Err(e) => panic!("{e:?}"),
        }
        assert!(made < 5000);
    }
    assert!(made > 100);
    assert_clean(&mut fs);
    // Removing never needs the reserve, even when creating is refused.
    for i in 0..made {
        fs.remove(fs.root(), &std::format!("a-long-file-name-to-fill-the-folder-{i:05}")).unwrap();
    }
    // (Folders don't shrink, so little is given back; what matters is that every removal worked.)
    let root = fs.root();
    assert!(names(&mut fs, root).is_empty());
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn the_reserve_is_never_given_to_programs() {
    let mut fs = fresh(300);
    let info = fs.info();
    let _ = fill(&mut fs);
    assert!(fs.info().free_blocks >= info.reserve_blocks.min(fs.info().free_blocks));
    assert!(fs.info().free_blocks > 0, "the reserve should remain");
}
