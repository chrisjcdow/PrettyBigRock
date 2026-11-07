// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading volumes made by `mkfs.fat` and filled by `mtools`: other people's FAT writers, as
//! independent evidence that we read the format right. Every file is compared byte for byte.

use super::tools::{Image, TempDir, need_tools};
use super::*;
use std::path::Path;
use std::string::ToString;

/// Writes a file on the host, making its folders.
fn host_file(root: &Path, path: &str, data: &[u8]) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, data).unwrap();
}

/// The tree of everything in `root` on the host.
fn host_tree(root: &Path) -> Tree {
    fn walk(dir: &Path, prefix: &str, tree: &mut Tree) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let path = std::format!("{prefix}/{}", e.file_name().to_string_lossy());
            if e.file_type().unwrap().is_dir() {
                tree.insert(path.clone(), None);
                walk(&e.path(), &path, tree);
            } else {
                tree.insert(path, Some(std::fs::read(e.path()).unwrap()));
            }
        }
    }
    let mut tree = Tree::new();
    walk(root, "", &mut tree);
    tree
}

/// The files `mtools` is given in most tests below.
fn sample(root: &Path) {
    host_file(root, "readme.txt", b"Read me.\r\n");
    host_file(root, "UPPER.TXT", b"upper case 8.3");
    host_file(root, "Long File Name With Spaces.txt", &bytes(1, 777));
    host_file(root, "\u{dc}n\u{ef} c\u{f4}d\u{e9} \u{65e5}\u{672c}\u{8a9e}.txt", &bytes(2, 1234));
    // (Not outside the Basic Multilingual Plane, which `mtools` cannot write.)
    host_file(root, "symbols \u{263a}\u{2764}.txt", &bytes(3, 10));
    host_file(root, "empty", b"");
    host_file(root, "big.bin", &bytes(4, 3_000_000));
    host_file(root, "exactly one cluster.bin", &bytes(5, 512));
    host_file(root, "one cluster and a byte.bin", &bytes(6, 513));
    host_file(root, "dir/sub/deeper/file.dat", &bytes(7, 5000));
    host_file(root, "dir/Another Folder/x.txt", b"x");
    host_file(root, "dir/Another Folder/\u{e9} accent.txt", b"accent");
    // A name of 255 characters, the most there is: 20 long-name entries before the short one.
    let longest = std::format!("{}{}.txt", "abcdefghi ".repeat(24), "x".repeat(11));
    assert_eq!(longest.chars().count(), 255);
    host_file(root, &longest, b"long");
    for i in 0..100 {
        host_file(
            root,
            &std::format!("many/File {i:03}.dat"),
            &bytes(100 + i, 50 + i as usize * 13),
        );
    }
}

#[test]
fn reads_a_volume_made_by_mkfs_and_mtools() {
    need_tools!("mkfs.fat", "mcopy", "mattrib", "fsck.fat");
    let img = Image::mkfs(40_000, &["-s", "1", "-n", "MKFSVOL", "-i", "DEADBEEF"]);
    let src = TempDir::new();
    sample(src.path());
    img.copy_tree(src.path());
    img.mtools("mattrib", &["+h", "::/readme.txt"]);
    img.assert_clean("the image as mtools left it");

    let mut fs = mount(img.disk());
    let info = fs.info();
    assert_eq!(
        (info.label.as_str(), info.volume_id, info.cluster_bytes),
        ("MKFSVOL", 0xDEAD_BEEF, 512)
    );
    assert!(info.clean_at_mount && info.used_fsinfo);
    // `fsck.fat` says how many clusters are used and how many there are.
    let summary = img.fsck().text;
    let counts =
        summary.split(" files, ").nth(1).unwrap().split(" clusters").next().unwrap().to_string();
    let (used, total) = counts.split_once('/').unwrap();
    assert_eq!(info.cluster_count, total.parse::<u32>().unwrap());
    assert_eq!(info.free_clusters, info.cluster_count - used.parse::<u32>().unwrap());

    let want = host_tree(src.path());
    let got = tree_of(&mut fs);
    assert_same_tree(&got, &want, "the tree as we read it");
    // Long names keep their spelling exactly.
    let root = fs.root();
    let listed = names(&mut fs, root);
    for exact in [
        "Long File Name With Spaces.txt",
        "\u{dc}n\u{ef} c\u{f4}d\u{e9} \u{65e5}\u{672c}\u{8a9e}.txt",
        "symbols \u{263a}\u{2764}.txt",
        "exactly one cluster.bin",
    ] {
        assert!(listed.iter().any(|n| n == exact), "{exact} in {listed:?}");
    }
    // Hidden, and only that.
    let hidden: Vec<_> = list_all(&mut fs, root)
        .iter()
        .filter(|e| e.hidden)
        .map(|e| e.name.to_lowercase())
        .collect();
    assert_eq!(hidden, ["readme.txt"]);
    // The longest name, and sizes.
    assert!(listed.iter().any(|n| n.chars().count() == 255));
    let big = fs.lookup(root, "big.bin").unwrap();
    assert_eq!(fs.stat(big).unwrap().size, 3_000_000);
    // Reading in odd pieces gives the same bytes as in one go.
    let data = bytes(4, 3_000_000);
    let mut at = 1u64;
    let mut piece = 1usize;
    while (at as usize) < data.len() {
        let mut buf = std::vec![0u8; piece];
        let n = fs.read(big, at, &mut buf).unwrap();
        assert_eq!(&buf[..n], &data[at as usize..at as usize + n]);
        at += n as u64 + 7;
        piece = (piece * 3 + 1) % 40_000 + 1;
    }
}

#[test]
fn reads_fragmented_files() {
    need_tools!("mkfs.fat", "mcopy", "mdel");
    let img = Image::mkfs(40_000, &["-s", "1"]);
    // Many small files, every other one deleted, then a big file that must use the holes.
    let mut files = Vec::new();
    for i in 0..300 {
        let name = std::format!("small{i}.dat");
        let data = bytes(i, 1500);
        img.put(&std::format!("::/{name}"), &data);
        files.push((name, data));
    }
    for (i, (name, _)) in files.iter().enumerate() {
        if i % 2 == 0 {
            img.mtools("mdel", &[&std::format!("::/{name}")]);
        }
    }
    let mut want = Tree::new();
    for (i, (name, data)) in files.iter().enumerate() {
        if i % 2 == 1 {
            want.insert(std::format!("/{name}"), Some(data.clone()));
        }
    }
    // `mtools` carries on from the last cluster it used, so the holes are only used once
    // the end of the volume is full: fill all of it but four clusters, then write the big file.
    let free = mount(img.disk()).info().free_clusters as usize;
    let holes = 150 * 3;
    let filler = std::vec![0xF1u8; (free - holes - 4) * 512];
    img.put("::/filler.bin", &filler);
    want.insert("/filler.bin".into(), Some(filler));
    let data = bytes(1000, 150_000);
    img.put("::/big0.bin", &data);
    want.insert("/big0.bin".into(), Some(data));
    img.assert_clean("fragmented image");
    let mut fs = mount(img.disk());
    // It really is fragmented: the chain of big0.bin jumps about.
    let big = node_at(&mut fs, "big0.bin").unwrap();
    let mut cluster = fs.nodes[&big].chain.first;
    let mut jumps = 0;
    while let Some(next) = fs.vol.follow(cluster).unwrap() {
        jumps += usize::from(next != cluster + 1);
        cluster = next;
    }
    assert!(jumps > 50, "only {jumps} jumps");
    assert_same_tree(&tree_of(&mut fs), &want, "the fragmented tree");
}

#[test]
fn reads_other_layouts_mkfs_can_make() {
    need_tools!("mkfs.fat", "mcopy", "mmd", "fsck.fat");
    // Two sectors to a cluster, one copy of the FAT, a larger reserved area, a different
    // number of sectors per track: none of it should matter.
    let layouts: [&[&str]; 3] = [
        &["-s", "2", "-n", "TWO"],
        &["-s", "1", "-f", "1", "-R", "100"],
        &["-s", "4", "-r", "512", "-n", "FOUR"],
    ];
    let sizes = [70_000, 40_000, 140_000];
    for (extra, kib) in layouts.iter().zip(sizes) {
        let img = Image::mkfs(kib, extra);
        let src = TempDir::new();
        host_file(src.path(), "a/b/Some File.txt", &bytes(9, 9999));
        host_file(src.path(), "top.bin", &bytes(8, 100_000));
        img.copy_tree(src.path());
        let mut fs = mount(img.disk());
        let copies = if extra.contains(&"-f") { 1 } else { 2 };
        assert_eq!(fs.info().fat_copies, copies, "{extra:?}");
        assert_same_tree(&tree_of(&mut fs), &host_tree(src.path()), &std::format!("{extra:?}"));
    }
}

#[test]
fn an_empty_volume_made_by_mkfs() {
    need_tools!("mkfs.fat");
    // No label given: mkfs.fat writes "NO NAME", which means none.
    let img = Image::mkfs(40_000, &["-s", "1"]);
    let mut fs = mount(img.disk());
    assert_eq!(fs.info().label, "");
    let root = fs.root();
    assert!(names(&mut fs, root).is_empty());
    let space = fs.space().unwrap();
    assert_eq!(space.free + 512, space.total, "only the root folder is used");
}
