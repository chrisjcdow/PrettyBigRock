// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What we write must be read by other systems: after changes made with `lib/fat`,
//! `fsck.fat -n` must find nothing wrong (it also checks the two copies of the FAT agree, and
//! that every cluster is accounted for), and `mtools` must read back exactly the same tree.

use super::tools::{Image, TempDir, need_tools};
use super::*;

/// A volume being changed, and a simple model of what it should hold: the same operations are
/// applied to both.
struct Both {
    fs: Fat<MemDisk>,
    model: Tree,
}

impl Both {
    fn new(fs: Fat<MemDisk>) -> Both {
        let mut fs = fs;
        let model = tree_of(&mut fs);
        Both { fs, model }
    }

    fn mkdir(&mut self, path: &str) {
        mkdir(&mut self.fs, path).unwrap();
        self.model.insert(std::format!("/{path}"), None);
    }

    /// Creates or replaces a file, written as a server would, in pieces, and committed.
    fn put(&mut self, path: &str, data: &[u8]) {
        put(&mut self.fs, path, data).unwrap();
        self.model.insert(std::format!("/{path}"), Some(data.to_vec()));
    }

    /// Writes into an existing file without committing (the model follows at once).
    fn write_at(&mut self, path: &str, offset: usize, data: &[u8]) {
        let node = node_at(&mut self.fs, path).unwrap();
        self.fs.write(node, offset as u64, data).unwrap();
        self.fs.commit(node).unwrap();
        let content = self.model.get_mut(&std::format!("/{path}")).unwrap().as_mut().unwrap();
        if content.len() < offset + data.len() {
            content.resize(offset + data.len(), 0);
        }
        content[offset..offset + data.len()].copy_from_slice(data);
    }

    fn set_size(&mut self, path: &str, size: usize) {
        let node = node_at(&mut self.fs, path).unwrap();
        self.fs.set_size(node, size as u64).unwrap();
        self.fs.commit(node).unwrap();
        self.model.get_mut(&std::format!("/{path}")).unwrap().as_mut().unwrap().resize(size, 0);
    }

    fn remove(&mut self, path: &str) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = node_at(&mut self.fs, dir).unwrap();
        self.fs.remove(parent, name).unwrap();
        self.model.remove(&std::format!("/{path}")).unwrap();
    }

    fn rename(&mut self, from: &str, to: &str, replace: bool) {
        let split = |p: &str| {
            let (d, n) = p.rsplit_once('/').unwrap_or(("", p));
            (d.to_string(), n.to_string())
        };
        let (from_dir, from_name) = split(from);
        let (to_dir, to_name) = split(to);
        let a = node_at(&mut self.fs, &from_dir).unwrap();
        let b = node_at(&mut self.fs, &to_dir).unwrap();
        self.fs.rename(a, &from_name, b, &to_name, replace).unwrap();
        let (from, to) = (std::format!("/{from}"), std::format!("/{to}"));
        // The model keeps names in the spelling they were last given.
        let existing = self.model.keys().find(|k| k.eq_ignore_ascii_case(&to)).cloned();
        // (A name that only differs in case from the one it replaces is the same file.)
        if let Some(k) = existing
            && !k.eq_ignore_ascii_case(&from)
        {
            self.model.remove(&k);
        }
        let moved: Vec<String> = self
            .model
            .keys()
            .filter(|k| **k == from || k.starts_with(&std::format!("{from}/")))
            .cloned()
            .collect();
        for k in moved {
            let v = self.model.remove(&k).unwrap();
            self.model.insert(std::format!("{to}{}", &k[from.len()..]), v);
        }
    }

    /// Unmounts and checks the image with the outside tools.
    fn finish(mut self, img: &Image, what: &str) {
        self.fs.unmount().unwrap();
        let disk = self.fs.into_device();
        img.store(&disk);
        img.assert_clean(what);
        img.assert_holds(&self.model, what);
        // And our own reading of it, after a fresh mount, agrees too.
        let mut again = mount(disk);
        assert_same_tree(&tree_of(&mut again), &self.model, what);
        assert!(again.info().clean_at_mount, "{what}: marked clean");
    }
}

use std::string::{String, ToString};

fn start(extra: &[&str]) -> (Image, Both) {
    let img = Image::mkfs(40_000, extra);
    let both = Both::new(mount(img.disk()));
    (img, both)
}

#[test]
fn a_tree_written_here_is_valid_for_fsck_and_mtools() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    let (img, mut v) = start(&["-s", "1", "-n", "WRITTEN"]);
    for dir in ["docs", "docs/Long Folder Name", "Pictures", "a", "UPPERDIR", "Mixed Dir.d"] {
        v.mkdir(dir);
    }
    let sizes = [0usize, 1, 511, 512, 513, 1023, 1024, 1025, 4096, 70_000, 1_000_000];
    for (i, size) in sizes.iter().enumerate() {
        v.put(&std::format!("Pictures/size {size}.bin"), &bytes(i as u64, *size));
    }
    let longest = std::format!("{}{}.txt", "abcdefghi ".repeat(24), "x".repeat(11));
    for (i, name) in [
        "docs/Long Folder Name/Report Final (v2).txt",
        "UPPER.TXT",
        "lower.txt",
        "Mixed Case.TXT",
        "naïve café.txt",
        "日本語のファイル.txt",
        "dots.in.name.tar.gz",
        ".hidden-looking",
        "docs/a+b,c;d=e[f].txt",
        "UPPERDIR/NOEXT",
        "Mixed Dir.d/ünï.txt",
        "docs/Ä.txt",
    ]
    .iter()
    .enumerate()
    {
        v.put(name, &bytes(100 + i as u64, 100 * i));
    }
    v.put(&longest, b"the longest name there is");
    v.put(&std::format!("docs/{longest}"), b"and in a folder");
    for i in 0..30 {
        v.put(&std::format!("a/{i}"), &[i as u8; 3]);
    }
    // Several files with the same start must all get different short names.
    for i in 0..12 {
        v.put(&std::format!("docs/Quarterly financial report {i}.txt"), &bytes(i, 700));
    }
    v.finish(&img, "a written tree");
}

#[test]
fn changes_to_a_tree_are_valid_for_fsck_and_mtools() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    let (img, mut v) = start(&["-s", "1"]);
    v.mkdir("a");
    v.mkdir("a/b");
    v.mkdir("c");
    for i in 0..20 {
        v.put(&std::format!("a/file {i}.dat"), &bytes(i, 1000 + 733 * i as usize));
    }
    v.put("a/b/deep.txt", &bytes(50, 5000));
    v.put("c/other.txt", b"other");
    // Overwrite in the middle, across clusters, growing and shrinking.
    v.write_at("a/file 3.dat", 100, &bytes(60, 900));
    v.write_at("a/file 4.dat", 3000, &bytes(61, 10_000));
    v.write_at("a/file 5.dat", 20_000, b"far away");
    v.set_size("a/file 6.dat", 100);
    v.set_size("a/file 7.dat", 0);
    v.set_size("a/file 8.dat", 50_000);
    v.set_size("a/file 9.dat", 3000);
    v.set_size("a/file 9.dat", 9000);
    // Remove, rename, move, replace.
    v.remove("a/file 10.dat");
    v.remove("a/file 11.dat");
    v.rename("a/file 12.dat", "a/renamed file 12.dat", false);
    v.rename("a/file 13.dat", "FILE13.DAT", false);
    v.rename("a/file 14.dat", "c/moved file 14.dat", false);
    v.rename("a/file 15.dat", "c/other.txt", true);
    v.rename("a/b", "c/b moved", false);
    v.rename("c/b moved", "b top level", false);
    v.rename("a/file 16.dat", "a/FILE 16.DAT", false);
    v.remove("a/file 17.dat");
    v.put("a/file 17.dat", b"again, but smaller");
    for i in 0..10 {
        v.put(&std::format!("a/new file number {i}.bin"), &bytes(200 + i, 2000));
    }
    v.remove("b top level/deep.txt");
    v.remove("b top level");
    v.finish(&img, "a changed tree");
}

#[test]
fn changes_to_a_volume_made_by_other_tools() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    let img = Image::mkfs(40_000, &["-s", "1", "-n", "OTHERS"]);
    let src = TempDir::new();
    for i in 0..40 {
        let path = src.path().join(std::format!("dir {}/File {i}.txt", i % 4));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes(i, 300 * i as usize)).unwrap();
    }
    std::fs::write(src.path().join("top.txt"), bytes(99, 4000)).unwrap();
    img.copy_tree(src.path());
    let mut v = Both::new(mount(img.disk()));
    assert_eq!(v.model.len(), 4 + 40 + 1);
    v.put("dir 0/Added by us.txt", &bytes(1, 3333));
    v.write_at("dir 1/File 1.txt", 10, b"changed");
    v.write_at("top.txt", 4000, &bytes(2, 9000));
    v.remove("dir 2/File 2.txt");
    v.rename("dir 3/File 3.txt", "dir 0/File 3 moved.txt", false);
    v.rename("dir 1", "dir one", false);
    v.finish(&img, "a volume from other tools");
}

#[test]
fn many_files_in_one_folder() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    let (img, mut v) = start(&["-s", "1"]);
    v.mkdir("many");
    for i in 0..300 {
        v.put(
            &std::format!("many/A fairly long file name number {i}.txt"),
            &bytes(i, (i % 50) as usize),
        );
    }
    for i in (0..300).step_by(3) {
        v.remove(&std::format!("many/A fairly long file name number {i}.txt"));
    }
    for i in 0..60 {
        v.put(&std::format!("many/N{i}"), b"n");
    }
    v.finish(&img, "a big folder");
}

/// `fsck.fat -n` on a volume that was not unmounted may complain about two things only: the
/// dirty bit, and the free-cluster summary in the FSInfo sector (which is only written at
/// unmount).
fn assert_only_unclean(img: &Image, what: &str) {
    let out = img.fsck();
    let allowed = |l: &str| {
        l.starts_with("fsck.fat ")
            || l.trim().is_empty()
            || l.starts_with("Dirty bit is set")
            || l.starts_with(" Automatically removing dirty bit")
            || l.starts_with("Leaving filesystem unchanged")
            || l.starts_with("Free cluster summary")
            || l.starts_with("  Auto-correcting")
            || l.contains(" files, ")
    };
    let bad: Vec<&str> = out.text.lines().filter(|l| !allowed(l)).collect();
    assert!(bad.is_empty(), "{what}: fsck.fat -n found: {bad:?}\n{}", out.text);
}

#[test]
fn the_volume_is_valid_after_every_committed_step() {
    need_tools!("mkfs.fat", "fsck.fat", "mcopy");
    // After each step the disk is copied as it is (as a crash would leave it, with no unmount)
    // and checked: a finished folder change or commit never leaves anything but a dirty volume.
    let (img, mut v) = start(&["-s", "1"]);
    let check = |v: &mut Both, what: &str| {
        let snapshot = v.fs.device().data.clone();
        img.store(&MemDisk::from(snapshot));
        assert_only_unclean(&img, what);
        img.assert_holds(&v.model, what);
    };
    v.mkdir("one");
    check(&mut v, "mkdir");
    v.mkdir("one/two");
    check(&mut v, "nested mkdir");
    v.put("one/A long file name.txt", &bytes(1, 5000));
    check(&mut v, "first file");
    v.put("one/two/x.txt", &bytes(2, 100));
    check(&mut v, "second file");
    v.write_at("one/A long file name.txt", 4000, &bytes(3, 4000));
    check(&mut v, "growing write");
    v.set_size("one/A long file name.txt", 700);
    check(&mut v, "cut");
    v.set_size("one/A long file name.txt", 0);
    check(&mut v, "cut to nothing");
    v.rename("one/two/x.txt", "one/y.txt", false);
    check(&mut v, "move");
    v.put("one/z.txt", b"z");
    v.rename("one/y.txt", "one/z.txt", true);
    check(&mut v, "replace");
    v.rename("one/two", "two moved", false);
    check(&mut v, "move a folder");
    v.remove("one/z.txt");
    check(&mut v, "remove a file");
    v.remove("two moved");
    check(&mut v, "remove a folder");
    for i in 0..40 {
        v.put(&std::format!("one/Many files with long names {i}"), &bytes(i, 600));
    }
    check(&mut v, "many files");
    v.finish(&img, "the end");
}
