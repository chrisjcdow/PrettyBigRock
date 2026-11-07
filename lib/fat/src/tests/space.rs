// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A full disk. Running out of room is an ordinary event, not a disaster: the call that cannot
//! be done fails with `NoSpace` and changes nothing, what is already saved stays readable, and
//! removing files makes room again.

use super::tools::{Image, need_tools};
use super::*;

/// Appends to `path` in ever smaller pieces until not one more cluster fits. Returns how much
/// the file now holds (all of it written and read back later by the callers).
fn fill(fs: &mut Fat<MemDisk>, path: &str, seed: u64) -> Vec<u8> {
    let node = if let Ok(n) = node_at(fs, path) { n } else { put(fs, path, b"").unwrap() };
    let mut data = Vec::new();
    let mut piece = 65536usize;
    // Making random bytes is slow in a debug build, so make a pool once and cut each piece from
    // a different place in it.
    let pool = bytes(seed, 65536 + 128);
    while piece >= 1 {
        let from = data.len() / 512 % 128;
        let chunk = pool[from..from + piece].to_vec();
        match fs.write(node, data.len() as u64, &chunk) {
            Ok(n) => {
                assert_eq!(n, piece);
                data.extend_from_slice(&chunk);
            }
            Err(FsError::NoSpace) => piece /= 2,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    fs.commit(node).unwrap();
    data
}

/// The smallest volume FAT32 allows is 65525 clusters; this is a bit more.
fn small() -> Fat<MemDisk> {
    mount(blank_with(67_000, 1, "FULL"))
}

#[test]
fn filling_the_disk_and_emptying_it() {
    let mut fs = small();
    let root = fs.root();
    let before = fs.space().unwrap();
    mkdir(&mut fs, "keep").unwrap();
    let kept = bytes(1, 3000);
    put(&mut fs, "keep/saved.txt", &kept).unwrap();
    let big = fill(&mut fs, "big.bin", 7);
    assert_eq!(fs.info().free_clusters, 0, "the disk is exactly full");
    assert_eq!(fs.space().unwrap().free, 0);
    assert!(big.len() as u64 > before.total / 2);

    // Nothing more fits, and each refusal leaves things as they were.
    let big_node = fs.lookup(root, "big.bin").unwrap();
    assert_eq!(fs.write(big_node, big.len() as u64, b"x"), Err(FsError::NoSpace));
    assert_eq!(fs.set_size(big_node, big.len() as u64 + 1), Err(FsError::NoSpace));
    assert_eq!(fs.stat(big_node).unwrap().size, big.len() as u64);
    // A new file's name fits in the root folder's one cluster (no growth needed) and an empty
    // file needs no data cluster, so that works; a folder needs a cluster and cannot be made.
    assert_eq!(mkdir(&mut fs, "nope"), Err(FsError::NoSpace));
    assert_eq!(fs.lookup(root, "nope"), Err(FsError::NotFound));
    let f = fs.create(root, "empty.txt", Kind::File).unwrap();
    assert_eq!(fs.write(f, 0, b"x"), Err(FsError::NoSpace));
    assert_eq!(fs.stat(f).unwrap().size, 0);

    // What was saved is intact, before and after a remount.
    let n = node_at(&mut fs, "keep/saved.txt").unwrap();
    assert_eq!(read_all(&mut fs, n).unwrap(), kept);
    let mut fs = remount(fs);
    assert_eq!(fs.info().free_clusters, 0);
    let n = node_at(&mut fs, "big.bin").unwrap();
    assert_eq!(read_all(&mut fs, n).unwrap(), big);

    // Removing works on a full disk and gives the room back.
    fs.remove(root, "big.bin").unwrap();
    assert_eq!(fs.space().unwrap().free, before.free - 512 - 3000_u64.div_ceil(512) * 512);
    mkdir(&mut fs, "nope").unwrap();
    put(&mut fs, "again.bin", &bytes(9, 100_000)).unwrap();
}

#[test]
fn a_folder_that_cannot_grow() {
    let mut fs = small();
    let root = fs.root();
    // Fill the root folder's cluster (16 entries: the label, 14 files and the big one)...
    for i in 0..14 {
        fs.create(root, &std::format!("F{i}.TXT"), Kind::File).unwrap();
    }
    fill(&mut fs, "/BIG", 3);
    assert_eq!(fs.info().free_clusters, 0);
    // ...so that one more name needs a second cluster that does not exist.
    let names_before = names(&mut fs, root);
    assert_eq!(fs.create(root, "G.TXT", Kind::File), Err(FsError::NoSpace));
    assert_eq!(names(&mut fs, root), names_before);
    // A rename to a longer name that needs more entries fails the same way, and the old name
    // stays.
    let f = fs.lookup(root, "F0.TXT").unwrap();
    assert_eq!(
        fs.rename(root, "F0.TXT", root, "A much longer name than before.txt", false),
        Err(FsError::NoSpace)
    );
    assert_eq!(fs.lookup(root, "F0.TXT"), Ok(f));
    // Remove something big and it works.
    fs.remove(root, "BIG").unwrap();
    fs.create(root, "G.TXT", Kind::File).unwrap();
    fs.rename(root, "F0.TXT", root, "A much longer name than before.txt", false).unwrap();
    assert_eq!(fs.lookup(root, "A much longer name than before.txt"), Ok(f));
}

#[test]
fn growth_is_all_or_nothing() {
    // A write that needs more clusters than are free writes none of its bytes and takes none of
    // the clusters.
    let mut fs = small();
    let free = fs.info().free_clusters;
    let f = put(&mut fs, "f", b"head").unwrap();
    let data = bytes(5, 65536);
    // Take almost all the room: leave 10 clusters.
    let hog = put(&mut fs, "hog", b"").unwrap();
    let mut at = 0u64;
    while fs.info().free_clusters > 10 + 128 {
        fs.write(hog, at, &data).unwrap();
        at += 65536;
    }
    while fs.info().free_clusters > 10 {
        fs.write(hog, at, &data[..512]).unwrap();
        at += 512;
    }
    fs.commit(hog).unwrap();
    assert_eq!(fs.info().free_clusters, 10);
    assert_eq!(fs.write(f, 4, &data), Err(FsError::NoSpace));
    assert_eq!(fs.info().free_clusters, 10, "no cluster was kept");
    assert_eq!(fs.stat(f).unwrap().size, 4);
    assert_eq!(read_all(&mut fs, f).unwrap(), b"head");
    // What does fit, goes in.
    fs.write(f, 4, &data[..11 * 512 - 4]).unwrap();
    assert_eq!(fs.info().free_clusters, 0);
    fs.commit(f).unwrap();
    assert!(free > 10);
}

#[test]
fn a_full_disk_passes_fsck() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    let img = Image::mkfs(34_000, &[]);
    let mut fs = mount(img.disk());
    let root = fs.root();
    mkdir(&mut fs, "d").unwrap();
    put(&mut fs, "d/small.txt", b"small").unwrap();
    let big = fill(&mut fs, "big.bin", 11);
    assert_eq!(fs.info().free_clusters, 0);
    let _ = fs.create(root, "no room", Kind::Folder);
    fs.unmount().unwrap();
    img.store(fs.device());
    img.assert_clean("full");
    let mut fs = mount(fs.into_device());
    let mut tree = tree_of(&mut fs);
    assert_eq!(tree["/big.bin"], Some(big));
    img.assert_holds(&tree, "full");
    fs.remove(root, "big.bin").unwrap();
    tree.remove("/big.bin");
    fs.unmount().unwrap();
    img.store(fs.device());
    img.assert_clean("emptied again");
    img.assert_holds(&tree, "emptied again");
}
