// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests for the file commands, on a filesystem kept in memory.
//!
//! The in-memory filesystem answers like a real server (note 0014, section 5): the same errors
//! for a missing parent, a file where a folder is needed, a folder that isn't empty, a name
//! that is taken. It also panics if the shell ever asks it about one of the folders that only
//! hold other folders (`/`, `/Volumes`, `/System`), because the shell must answer those itself.

extern crate std;

use super::*;
use crate::fs::Space;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec;
use core::cell::RefCell;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Folder,
    File(Vec<u8>),
}

/// Where things are attached: the home folder, the rules file (read only), a data volume and a
/// volume that can't be written.
const ENTRIES: [(&str, bool); 4] = [
    ("/Home", true),
    ("/System/rules.txt", false),
    ("/Volumes/Data", true),
    ("/Volumes/Usb", true),
];

#[derive(Default)]
struct MemFs {
    nodes: RefCell<BTreeMap<String, Node>>,
    renames: RefCell<Vec<(String, String)>>,
    /// Paths that fail to be removed (as if the disk refused).
    stuck: RefCell<Vec<String>>,
    /// How many times `replace` and `append` were called.
    writes: RefCell<usize>,
}

impl MemFs {
    fn new() -> MemFs {
        let fs = MemFs::default();
        for (path, folder) in ENTRIES {
            if folder {
                fs.nodes.borrow_mut().insert(path.to_string(), Node::Folder);
            }
        }
        fs.nodes
            .borrow_mut()
            .insert("/System/rules.txt".to_string(), Node::File(b"allow\n".to_vec()));
        fs
    }

    /// Panics for a path the shell must not pass on; says which volume rules apply.
    fn real(&self, path: &str) {
        let ok = path == "/System/rules.txt"
            || layout::is_within("/Home", path)
            || layout::is_within("/Volumes/Data", path)
            || layout::is_within("/Volumes/Usb", path);
        assert!(ok, "the shell asked the filesystem about {path}, which only holds other folders");
    }

    fn check_writable(&self, path: &str) -> Result<(), Reason> {
        if path == "/System/rules.txt" {
            return Err(Reason::NoRight);
        }
        if layout::is_within("/Volumes/Usb", path) {
            return Err(Reason::ReadOnly);
        }
        Ok(())
    }

    fn check_parent(&self, path: &str) -> Result<(), Reason> {
        match self.nodes.borrow().get(layout::parent(path)) {
            Some(Node::Folder) => Ok(()),
            Some(Node::File(_)) => Err(Reason::NotAFolder),
            None => Err(Reason::NotFound),
        }
    }

    fn put(&self, path: &str, text: &str) {
        self.nodes.borrow_mut().insert(path.to_string(), Node::File(text.as_bytes().to_vec()));
    }

    fn put_bytes(&self, path: &str, data: Vec<u8>) {
        self.nodes.borrow_mut().insert(path.to_string(), Node::File(data));
    }

    fn dir(&self, path: &str) {
        self.nodes.borrow_mut().insert(path.to_string(), Node::Folder);
    }

    fn text(&self, path: &str) -> Option<String> {
        match self.nodes.borrow().get(path) {
            Some(Node::File(d)) => Some(String::from_utf8_lossy(d).into_owned()),
            _ => None,
        }
    }

    fn bytes(&self, path: &str) -> Option<Vec<u8>> {
        match self.nodes.borrow().get(path) {
            Some(Node::File(d)) => Some(d.clone()),
            _ => None,
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.nodes.borrow().contains_key(path)
    }
}

impl Fs for MemFs {
    fn stat(&self, path: &str) -> Result<Stat, Reason> {
        self.real(path);
        let nodes = self.nodes.borrow();
        let node = nodes.get(path).ok_or(Reason::NotFound)?;
        let writable = self.check_writable(path).is_ok();
        Ok(Stat {
            folder: *node == Node::Folder,
            size: if let Node::File(d) = node { d.len() as u64 } else { 0 },
            readable: true,
            writable,
            created: 0,
            modified: 0,
            hidden: false,
            read_only: layout::is_within("/Volumes/Usb", path),
        })
    }

    fn list(&self, path: &str) -> Result<Vec<Item>, Reason> {
        self.real(path);
        let nodes = self.nodes.borrow();
        match nodes.get(path) {
            None => return Err(Reason::NotFound),
            Some(Node::File(_)) => return Err(Reason::NotAFolder),
            Some(Node::Folder) => {}
        }
        Ok(nodes
            .iter()
            .filter(|(p, _)| p.as_str() != path && layout::parent(p) == path)
            .map(|(p, n)| Item {
                name: layout::base_name(p).unwrap().to_string(),
                folder: *n == Node::Folder,
                size: if let Node::File(d) = n { d.len() as u64 } else { 0 },
            })
            .collect())
    }

    fn read_at(&self, path: &str, offset: u64, max: u32) -> Result<Vec<u8>, Reason> {
        self.real(path);
        match self.nodes.borrow().get(path) {
            None => Err(Reason::NotFound),
            Some(Node::Folder) => Err(Reason::NotAFile),
            Some(Node::File(d)) => {
                let start = (offset as usize).min(d.len());
                let end = (start + max as usize).min(d.len());
                Ok(d[start..end].to_vec())
            }
        }
    }

    fn replace(&self, path: &str, data: &[u8]) -> Result<(), Reason> {
        self.real(path);
        *self.writes.borrow_mut() += 1;
        self.check_writable(path)?;
        self.check_parent(path)?;
        if self.nodes.borrow().get(path) == Some(&Node::Folder) {
            return Err(Reason::NotAFile);
        }
        self.put_bytes(path, data.to_vec());
        Ok(())
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), Reason> {
        self.real(path);
        *self.writes.borrow_mut() += 1;
        self.check_writable(path)?;
        self.check_parent(path)?;
        match self.nodes.borrow_mut().entry(path.to_string()).or_insert(Node::File(Vec::new())) {
            Node::File(d) => {
                d.extend_from_slice(data);
                Ok(())
            }
            Node::Folder => Err(Reason::NotAFile),
        }
    }

    fn mkdir(&self, path: &str) -> Result<(), Reason> {
        self.real(path);
        self.check_writable(path)?;
        self.check_parent(path)?;
        if self.exists(path) {
            return Err(Reason::AlreadyExists);
        }
        self.dir(path);
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<(), Reason> {
        self.real(path);
        self.check_writable(path)?;
        if self.stuck.borrow().iter().any(|s| s == path) {
            return Err(Reason::DiskError);
        }
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(path) {
            return Err(Reason::NotFound);
        }
        if nodes.keys().any(|p| p != path && layout::parent(p) == path) {
            return Err(Reason::FolderNotEmpty);
        }
        nodes.remove(path);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), Reason> {
        self.real(from);
        self.real(to);
        self.check_writable(from)?;
        self.check_writable(to)?;
        self.check_parent(to)?;
        self.renames.borrow_mut().push((from.to_string(), to.to_string()));
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(from) {
            return Err(Reason::NotFound);
        }
        if nodes.contains_key(to) {
            return Err(Reason::AlreadyExists);
        }
        let moved: Vec<String> =
            nodes.keys().filter(|p| layout::is_within(from, p)).cloned().collect();
        for old in moved {
            let node = nodes.remove(&old).unwrap();
            nodes.insert(alloc::format!("{to}{}", &old[from.len()..]), node);
        }
        Ok(())
    }

    fn space(&self, path: &str) -> Result<Space, Reason> {
        match path {
            "/Home" => Ok(Space {
                total: 100 * 1024 * 1024,
                free: 40 * 1024 * 1024,
                label: "Home".to_string(),
                filesystem: "RockFS".to_string(),
            }),
            "/Volumes/Data" => Ok(Space {
                total: 2 * 1024 * 1024 * 1024,
                free: 1024 * 1024 * 1024,
                label: String::new(),
                filesystem: "FAT32".to_string(),
            }),
            _ => Err(Reason::ServiceGone),
        }
    }
}

fn shell() -> FileShell<MemFs> {
    let entries: Vec<(String, bool)> = ENTRIES.iter().map(|(p, f)| (p.to_string(), *f)).collect();
    FileShell::new(MemFs::new(), &entries)
}

/// The error line a failed command would print after its name.
fn line<T: core::fmt::Debug>(r: Result<T, Failure>) -> String {
    r.expect_err("should have failed").to_string()
}

#[test]
fn starts_in_home() {
    assert_eq!(shell().cwd(), "/Home");
    // Without a home, at the top.
    let bare = FileShell::new(MemFs::new(), &[]);
    assert_eq!(bare.cwd(), "/");
}

#[test]
fn ls_sorts_and_hides() {
    let s = shell();
    s.fs().put("/Home/b.txt", "bb");
    s.fs().put("/Home/A.txt", "");
    s.fs().put("/Home/.secret", "x");
    s.fs().dir("/Home/Zoo");
    s.fs().dir("/Home/apps");
    s.fs().dir("/Home/.cache");
    assert_eq!(s.ls(false, None).unwrap(), "apps/\nZoo/\nA.txt  0 bytes\nb.txt  2 bytes\n");
    assert_eq!(
        s.ls(true, None).unwrap(),
        ".cache/\napps/\nZoo/\n.secret  1 byte\nA.txt  0 bytes\nb.txt  2 bytes\n"
    );
    assert_eq!(s.ls(false, Some("/Home/Zoo")).unwrap(), "");
    assert_eq!(s.ls(false, Some("Zoo/..")).unwrap(), s.ls(false, None).unwrap());
}

#[test]
fn ls_of_a_file_is_one_line() {
    let s = shell();
    s.fs().put("/Home/b.txt", "bb");
    assert_eq!(s.ls(false, Some("b.txt")).unwrap(), "b.txt  2 bytes\n");
    assert_eq!(s.ls(false, Some("/Home/b.txt")).unwrap(), "b.txt  2 bytes\n");
}

#[test]
fn ls_of_the_folders_above_the_volumes() {
    let s = shell();
    assert_eq!(s.ls(false, Some("/")).unwrap(), "Home/\nSystem/\nVolumes/\n");
    assert_eq!(s.ls(false, Some("..")).unwrap(), "Home/\nSystem/\nVolumes/\n");
    assert_eq!(s.ls(false, Some("/Volumes")).unwrap(), "Data/\nUsb/\n");
    assert_eq!(s.ls(false, Some("/System")).unwrap(), "rules.txt  6 bytes\n");
    assert_eq!(s.ls(false, Some("/System/rules.txt")).unwrap(), "rules.txt  6 bytes\n");
    assert_eq!(s.ls(false, Some("/Volumes/Data")).unwrap(), "");
}

#[test]
fn ls_errors() {
    let s = shell();
    assert_eq!(line(s.ls(false, Some("nothing"))), "nothing: not found");
    assert_eq!(line(s.ls(false, Some("/Nothing"))), "/Nothing: not found");
    assert_eq!(line(s.ls(false, Some("/Volumes/Nope"))), "/Volumes/Nope: not found");
    assert_eq!(line(s.ls(false, Some("a\u{1}b"))), "a\u{1}b: that isn't a valid path");
}

#[test]
fn cd_and_pwd() {
    let mut s = shell();
    s.fs().dir("/Home/Docs");
    s.fs().put("/Home/f", "x");
    s.cd(Some("Docs")).unwrap();
    assert_eq!(s.cwd(), "/Home/Docs");
    s.cd(Some("..")).unwrap();
    s.cd(Some("../..")).unwrap();
    assert_eq!(s.cwd(), "/");
    s.cd(Some("Volumes")).unwrap();
    assert_eq!(s.cwd(), "/Volumes");
    assert_eq!(s.ls(false, None).unwrap(), "Data/\nUsb/\n");
    s.cd(Some("Data")).unwrap();
    assert_eq!(s.cwd(), "/Volumes/Data");
    s.cd(None).unwrap();
    assert_eq!(s.cwd(), "/Home");
    s.cd(Some("/System")).unwrap();
    assert_eq!(s.cwd(), "/System");
    // Failing leaves the folder alone.
    assert_eq!(line(s.cd(Some("/Home/f"))), "/Home/f: not a folder");
    assert_eq!(line(s.cd(Some("/System/rules.txt"))), "/System/rules.txt: not a folder");
    assert_eq!(line(s.cd(Some("missing"))), "missing: not found");
    assert_eq!(line(s.cd(Some("/Elsewhere"))), "/Elsewhere: not found");
    assert_eq!(s.cwd(), "/System");
}

#[test]
fn relative_paths_follow_the_folder() {
    let mut s = shell();
    s.fs().dir("/Home/Docs");
    s.cd(Some("Docs")).unwrap();
    s.write("a.txt", "hi").unwrap();
    assert!(s.fs().exists("/Home/Docs/a.txt"));
    s.write("../b.txt", "ho").unwrap();
    assert!(s.fs().exists("/Home/b.txt"));
    // Paths full of `..` end up at the top and never beyond it.
    let up = "../".repeat(50);
    assert_eq!(
        line(s.write(&alloc::format!("{up}c.txt"), "no")),
        alloc::format!("{up}c.txt: it would have to go in a folder that only holds other folders")
    );
    assert_eq!(s.cat("/../../Home/b.txt").unwrap(), "ho\n");
}

#[test]
fn cat_shows_files() {
    let s = shell();
    s.fs().put("/Home/a", "one\ntwo\n");
    s.fs().put("/Home/b", "no end");
    s.fs().put("/Home/empty", "");
    s.fs().put("/Home/dos", "a\r\nb\r\n");
    s.fs().put_bytes("/Home/bin", vec![b'a', 0xFF, b'b']);
    assert_eq!(s.cat("a").unwrap(), "one\ntwo\n");
    assert_eq!(s.cat("b").unwrap(), "no end\n");
    assert_eq!(s.cat("empty").unwrap(), "");
    assert_eq!(s.cat("dos").unwrap(), "a\nb\n");
    assert_eq!(s.cat("bin").unwrap(), "a\u{FFFD}b\n");
    assert_eq!(s.cat("/System/rules.txt").unwrap(), "allow\n");
}

#[test]
fn cat_limits_and_errors() {
    let s = shell();
    s.fs().put_bytes("/Home/exact", vec![b'x'; 65536]);
    s.fs().put_bytes("/Home/big", vec![b'x'; 65537]);
    s.fs().dir("/Home/d");
    assert_eq!(s.cat("exact").unwrap().len(), 65537);
    assert_eq!(line(s.cat("big")), "big: too big to show (64 KB at most)");
    assert_eq!(line(s.cat("d")), "d: not a file");
    assert_eq!(line(s.cat("/")), "/: not a file");
    assert_eq!(line(s.cat("/Volumes")), "/Volumes: not a file");
    assert_eq!(line(s.cat("missing")), "missing: not found");
}

#[test]
fn info_describes() {
    let s = shell();
    s.fs().put("/Home/a", "abc");
    let text = s.info("a").unwrap();
    assert!(text.starts_with("Path: /Home/a\nKind: file\nSize: 3 bytes\nRights: read and write\n"));
    let rules = s.info("/System/rules.txt").unwrap();
    assert!(rules.contains("Rights: read\n"), "{rules}");
    let usb = s.info("/Volumes/Usb").unwrap();
    assert!(usb.contains("Kind: folder\n") && usb.ends_with("Volume: read only\n"), "{usb}");
    let top = s.info("/").unwrap();
    assert!(top.contains("Kind: folder\n") && top.contains("Rights: read\n"), "{top}");
    assert_eq!(line(s.info("nope")), "nope: not found");
}

#[test]
fn mkdir_makes_folders() {
    let s = shell();
    s.mkdir("new").unwrap();
    assert!(s.fs().exists("/Home/new"));
    s.mkdir("new/inner").unwrap();
    assert_eq!(line(s.mkdir("new")), "new: already exists");
    assert_eq!(line(s.mkdir("missing/inner")), "missing/inner: not found");
    s.fs().put("/Home/f", "");
    assert_eq!(line(s.mkdir("f/x")), "f/x: not a folder");
    assert_eq!(line(s.mkdir("f")), "f: already exists");
}

#[test]
fn nothing_can_be_made_above_the_volumes() {
    let s = shell();
    let above = "it would have to go in a folder that only holds other folders";
    assert_eq!(line(s.mkdir("/Stuff")), alloc::format!("/Stuff: {above}"));
    assert_eq!(line(s.mkdir("/Volumes/New")), alloc::format!("/Volumes/New: {above}"));
    assert_eq!(line(s.mkdir("/System/x")), alloc::format!("/System/x: {above}"));
    assert_eq!(line(s.write("/x.txt", "a")), alloc::format!("/x.txt: {above}"));
    assert_eq!(line(s.append("/Volumes/x.txt", "a")), alloc::format!("/Volumes/x.txt: {above}"));
    // Where they already are, they exist.
    assert_eq!(line(s.mkdir("/")), "/: already exists");
    assert_eq!(line(s.mkdir("/Volumes")), "/Volumes: already exists");
    assert_eq!(line(s.mkdir("/Home")), "/Home: already exists");
    assert_eq!(line(s.mkdir("/Volumes/Data")), "/Volumes/Data: already exists");
    // Below something that isn't there at all.
    assert_eq!(line(s.mkdir("/Nope/x")), "/Nope/x: not found");
    // Nothing was changed.
    assert_eq!(s.ls(false, Some("/")).unwrap(), "Home/\nSystem/\nVolumes/\n");
}

#[test]
fn write_and_append() {
    let s = shell();
    s.write("a.txt", "hello world").unwrap();
    assert_eq!(s.fs().text("/Home/a.txt").as_deref(), Some("hello world\n"));
    s.write("a.txt", "again").unwrap();
    assert_eq!(s.fs().text("/Home/a.txt").as_deref(), Some("again\n"));
    s.append("a.txt", "more").unwrap();
    s.append("a.txt", "").unwrap();
    assert_eq!(s.fs().text("/Home/a.txt").as_deref(), Some("again\nmore\n\n"));
    s.append("new.txt", "first").unwrap();
    assert_eq!(s.fs().text("/Home/new.txt").as_deref(), Some("first\n"));
    s.write("empty.txt", "").unwrap();
    assert_eq!(s.fs().text("/Home/empty.txt").as_deref(), Some("\n"));
}

#[test]
fn write_errors() {
    let s = shell();
    s.fs().dir("/Home/d");
    assert_eq!(line(s.write("d", "x")), "d: not a file");
    assert_eq!(line(s.append("d", "x")), "d: not a file");
    assert_eq!(line(s.write("/Home", "x")), "/Home: not a file");
    assert_eq!(line(s.write("/", "x")), "/: not a file");
    assert_eq!(line(s.write("/Volumes", "x")), "/Volumes: not a file");
    assert_eq!(line(s.write("missing/a", "x")), "missing/a: not found");
    assert_eq!(
        line(s.write("/System/rules.txt", "x")),
        "/System/rules.txt: you don't have the right to do that"
    );
    assert_eq!(line(s.write("/Volumes/Usb/a", "x")), "/Volumes/Usb/a: read only");
    assert_eq!(s.fs().text("/System/rules.txt").as_deref(), Some("allow\n"));
}

#[test]
fn rm_removes() {
    let s = shell();
    s.fs().put("/Home/a", "");
    s.fs().dir("/Home/d");
    s.rm(false, "a").unwrap();
    s.rm(false, "d").unwrap();
    assert!(!s.fs().exists("/Home/a") && !s.fs().exists("/Home/d"));
    assert_eq!(line(s.rm(false, "a")), "a: not found");
    assert_eq!(line(s.rm(true, "a")), "a: not found");
}

#[test]
fn rm_of_a_full_folder_needs_r() {
    let s = shell();
    s.fs().dir("/Home/d");
    s.fs().dir("/Home/d/e");
    s.fs().put("/Home/d/e/f", "x");
    s.fs().put("/Home/d/g", "x");
    s.fs().put("/Home/keep", "x");
    assert_eq!(line(s.rm(false, "d")), "d: the folder isn't empty");
    assert!(s.fs().exists("/Home/d/e/f"));
    s.rm(true, "d").unwrap();
    assert!(!s.fs().exists("/Home/d") && !s.fs().exists("/Home/d/e/f"));
    assert!(s.fs().exists("/Home/keep"));
    // -r on a file is just removing it.
    s.rm(true, "keep").unwrap();
    assert!(!s.fs().exists("/Home/keep"));
}

#[test]
fn rm_refuses_mount_points_and_folders_above() {
    let s = shell();
    for (path, reason) in [
        ("/Home", "that is where something is attached, so it can't be removed, moved or replaced"),
        (
            "/Volumes/Data",
            "that is where something is attached, so it can't be removed, moved or replaced",
        ),
        (
            "/System/rules.txt",
            "that is where something is attached, so it can't be removed, moved or replaced",
        ),
        ("/", "that folder only holds other folders, so it can't be changed"),
        ("/Volumes", "that folder only holds other folders, so it can't be changed"),
        ("/System", "that folder only holds other folders, so it can't be changed"),
        ("..", "that folder only holds other folders, so it can't be changed"),
        (
            "/Home/../Home/.",
            "that is where something is attached, so it can't be removed, moved or replaced",
        ),
    ] {
        for recursive in [false, true] {
            assert_eq!(line(s.rm(recursive, path)), alloc::format!("{path}: {reason}"));
        }
    }
    assert!(s.fs().exists("/Home") && s.fs().exists("/Volumes/Data"));
    assert_eq!(line(s.rm(true, "/Nothing")), "/Nothing: not found");
}

#[test]
fn rm_gives_up_on_very_deep_folders() {
    let s = shell();
    let mut path = String::from("/Home");
    for _ in 0..70 {
        path.push_str("/d");
        s.fs().dir(&path);
    }
    let failure = s.rm(true, "d").unwrap_err();
    assert_eq!(failure.reason, Reason::TooDeep);
    // Nothing was half removed from the top: the deepest folders were tried first, so
    // everything still exists down to where it gave up.
    assert!(s.fs().exists("/Home/d"));
}

#[test]
fn rm_failure_names_the_item() {
    let s = shell();
    s.fs().dir("/Home/d");
    s.fs().put("/Home/d/a", "");
    s.fs().put("/Home/d/b", "");
    s.fs().stuck.borrow_mut().push("/Home/d/b".to_string());
    assert_eq!(line(s.rm(true, "d")), "/Home/d/b: the disk gave an error");
    assert_eq!(line(s.rm(false, "d/b")), "d/b: the disk gave an error");
}

#[test]
fn cp_copies_a_file() {
    let s = shell();
    s.fs().put("/Home/a", "data");
    s.cp(false, "a", "b").unwrap();
    assert_eq!(s.fs().text("/Home/b").as_deref(), Some("data"));
    assert_eq!(s.fs().text("/Home/a").as_deref(), Some("data"));
    // Replaces what is there.
    s.fs().put("/Home/c", "old");
    s.cp(false, "a", "c").unwrap();
    assert_eq!(s.fs().text("/Home/c").as_deref(), Some("data"));
    // Into a folder keeps the name.
    s.fs().dir("/Home/d");
    s.cp(false, "a", "d").unwrap();
    assert_eq!(s.fs().text("/Home/d/a").as_deref(), Some("data"));
    // An empty file is copied as an empty file.
    s.fs().put("/Home/empty", "");
    s.cp(false, "empty", "empty2").unwrap();
    assert_eq!(s.fs().text("/Home/empty2").as_deref(), Some(""));
}

#[test]
fn cp_between_volumes_and_from_virtual_places() {
    let s = shell();
    s.fs().put("/Home/a", "data");
    s.cp(false, "a", "/Volumes/Data").unwrap();
    assert_eq!(s.fs().text("/Volumes/Data/a").as_deref(), Some("data"));
    s.cp(false, "/System/rules.txt", "rules-copy").unwrap();
    assert_eq!(s.fs().text("/Home/rules-copy").as_deref(), Some("allow\n"));
    // A whole virtual folder can be copied out with -r.
    s.cp(true, "/System", "/Home").unwrap();
    assert_eq!(s.fs().text("/Home/System/rules.txt").as_deref(), Some("allow\n"));
    // And copying into one is refused.
    let above = "it would have to go in a folder that only holds other folders";
    assert_eq!(line(s.cp(false, "a", "/Volumes/New")), alloc::format!("/Volumes/New: {above}"));
    assert_eq!(line(s.cp(false, "a", "/Volumes")), alloc::format!("/Volumes/a: {above}"));
    assert_eq!(line(s.cp(false, "a", "/System")), alloc::format!("/System/a: {above}"));
    // Nor onto a read-only volume, or the read-only rules file.
    assert_eq!(line(s.cp(false, "a", "/Volumes/Usb")), "/Volumes/Usb/a: read only");
    assert_eq!(
        line(s.cp(false, "a", "/System/rules.txt")),
        "/System/rules.txt: you don't have the right to do that"
    );
}

#[test]
fn cp_of_big_files_goes_in_chunks() {
    let s = shell();
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    s.fs().put_bytes("/Home/big", data.clone());
    s.cp(false, "big", "/Volumes/Data/big").unwrap();
    assert_eq!(s.fs().bytes("/Volumes/Data/big"), Some(data));
    // 200000 bytes is four chunks of 64 KB (the last is short): one replace, three appends.
    assert_eq!(*s.fs().writes.borrow(), 4);
    // A file that is exactly one chunk long is copied whole, and the empty read after it ends it.
    let exact = vec![7u8; 65536];
    s.fs().put_bytes("/Home/exact", exact.clone());
    s.cp(false, "exact", "exact2").unwrap();
    assert_eq!(s.fs().bytes("/Home/exact2"), Some(exact));
}

#[test]
fn cp_errors() {
    let s = shell();
    s.fs().put("/Home/a", "x");
    s.fs().dir("/Home/d");
    assert_eq!(line(s.cp(false, "missing", "b")), "missing: not found");
    assert_eq!(line(s.cp(true, "missing", "b")), "missing: not found");
    assert_eq!(
        line(s.cp(false, "d", "e")),
        "d: it is a folder (use -r to include what is inside it)"
    );
    assert_eq!(line(s.cp(false, "a", "a")), "a: those are the same place");
    assert_eq!(line(s.cp(false, "a", "./a")), "a: those are the same place");
    assert_eq!(line(s.cp(false, "a", "missing/x")), "missing/x: not found");
    assert_eq!(line(s.cp(false, "a", "a/x")), "a/x: not a folder");
    // A file can't replace a folder.
    s.fs().dir("/Home/e");
    s.fs().dir("/Home/e/a");
    assert_eq!(line(s.cp(false, "a", "e")), "/Home/e/a: not a file");
}

#[test]
fn cp_r_copies_trees() {
    let s = shell();
    s.fs().dir("/Home/src");
    s.fs().dir("/Home/src/sub");
    s.fs().dir("/Home/src/empty");
    s.fs().put("/Home/src/a", "A");
    s.fs().put("/Home/src/sub/b", "B");
    s.cp(true, "src", "copy").unwrap();
    assert_eq!(s.fs().text("/Home/copy/a").as_deref(), Some("A"));
    assert_eq!(s.fs().text("/Home/copy/sub/b").as_deref(), Some("B"));
    assert!(s.fs().exists("/Home/copy/empty"));
    assert!(s.fs().exists("/Home/src/sub/b"));
    // Into an existing folder: a folder with the same name goes inside it.
    s.cp(true, "src", "/Volumes/Data").unwrap();
    assert_eq!(s.fs().text("/Volumes/Data/src/sub/b").as_deref(), Some("B"));
    // Copying again merges and replaces.
    s.fs().put("/Home/src/a", "A2");
    s.cp(true, "src", "/Volumes/Data").unwrap();
    assert_eq!(s.fs().text("/Volumes/Data/src/a").as_deref(), Some("A2"));
    // A folder can't replace a file.
    s.fs().put("/Home/file", "");
    assert_eq!(line(s.cp(true, "src", "file")), "file: not a folder");
}

#[test]
fn cp_r_never_copies_a_folder_into_itself() {
    let s = shell();
    s.fs().dir("/Home/d");
    s.fs().dir("/Home/d/inner");
    s.fs().put("/Home/d/f", "x");
    let before = s.fs().nodes.borrow().clone();
    let itself = "a folder can't go inside itself";
    assert_eq!(line(s.cp(true, "d", "d/inner")), alloc::format!("d: {itself}"));
    assert_eq!(line(s.cp(true, "d", "d/new")), alloc::format!("d: {itself}"));
    assert_eq!(line(s.cp(true, "d", "d")), alloc::format!("d: {itself}"));
    assert_eq!(line(s.cp(true, "/Home", "/Home/d")), alloc::format!("/Home: {itself}"));
    assert_eq!(line(s.cp(true, "/", "/Home/x")), alloc::format!("/: {itself}"));
    // The top of the tree has no name to copy under.
    assert_eq!(line(s.cp(true, "/", "/")), "/: the filesystem can't do that");
    assert_eq!(*s.fs().nodes.borrow(), before);
}

#[test]
fn cp_r_gives_up_on_very_deep_folders() {
    let s = shell();
    let mut path = String::from("/Home");
    for _ in 0..70 {
        path.push_str("/d");
        s.fs().dir(&path);
    }
    assert_eq!(s.cp(true, "d", "/Volumes/Data/copy").unwrap_err().reason, Reason::TooDeep);
}

#[test]
fn mv_renames_within_a_volume() {
    let s = shell();
    s.fs().put("/Home/a", "x");
    s.fs().dir("/Home/d");
    s.fs().put("/Home/d/in", "y");
    s.mv("a", "b").unwrap();
    assert!(!s.fs().exists("/Home/a") && s.fs().text("/Home/b").as_deref() == Some("x"));
    s.mv("b", "d").unwrap();
    assert_eq!(s.fs().text("/Home/d/b").as_deref(), Some("x"));
    s.mv("d", "e").unwrap();
    assert_eq!(s.fs().text("/Home/e/in").as_deref(), Some("y"));
    assert_eq!(s.fs().text("/Home/e/b").as_deref(), Some("x"));
    assert!(!s.fs().exists("/Home/d"));
    // Everything was done with the filesystem's own rename.
    assert_eq!(s.fs().renames.borrow().len(), 3);
}

#[test]
fn mv_between_volumes_copies_then_removes() {
    let s = shell();
    s.fs().put("/Home/a", "x");
    s.fs().dir("/Home/d");
    s.fs().dir("/Home/d/sub");
    s.fs().put("/Home/d/sub/f", "y");
    s.mv("a", "/Volumes/Data/a2").unwrap();
    assert_eq!(s.fs().text("/Volumes/Data/a2").as_deref(), Some("x"));
    assert!(!s.fs().exists("/Home/a"));
    s.mv("d", "/Volumes/Data").unwrap();
    assert_eq!(s.fs().text("/Volumes/Data/d/sub/f").as_deref(), Some("y"));
    assert!(!s.fs().exists("/Home/d") && !s.fs().exists("/Home/d/sub/f"));
    assert!(s.fs().renames.borrow().is_empty());
}

#[test]
fn mv_leaves_the_original_if_it_cannot_be_removed() {
    let s = shell();
    s.fs().put("/Home/a", "x");
    s.fs().stuck.borrow_mut().push("/Home/a".to_string());
    assert_eq!(
        line(s.mv("a", "/Volumes/Data/a")),
        "a: copied, but the original could not be removed: the disk gave an error"
    );
    assert!(s.fs().exists("/Home/a") && s.fs().exists("/Volumes/Data/a"));
    // Out of a volume that can't be written, the copy works and the removal says why.
    s.fs().put("/Volumes/Usb/u", "z");
    assert_eq!(
        line(s.mv("/Volumes/Usb/u", "u")),
        "/Volumes/Usb/u: copied, but the original could not be removed: read only"
    );
    assert_eq!(s.fs().text("/Home/u").as_deref(), Some("z"));
}

#[test]
fn mv_refuses_what_it_should() {
    let s = shell();
    s.fs().put("/Home/a", "x");
    s.fs().put("/Home/b", "y");
    s.fs().dir("/Home/d");
    s.fs().dir("/Home/d/in");
    s.fs().put("/Volumes/Data/b", "z");
    assert_eq!(line(s.mv("a", "b")), "b: already exists");
    assert_eq!(line(s.mv("a", "/Volumes/Data/b")), "/Volumes/Data/b: already exists");
    assert_eq!(line(s.mv("a", "a")), "a: those are the same place");
    assert_eq!(line(s.mv("d", "d/in")), "d: a folder can't go inside itself");
    assert_eq!(line(s.mv("missing", "x")), "missing: not found");
    assert_eq!(line(s.mv("a", "missing/x")), "missing/x: not found");
    assert_eq!(line(s.mv("a", "/System/rules.txt")), "/System/rules.txt: already exists");
    let above = "it would have to go in a folder that only holds other folders";
    assert_eq!(line(s.mv("a", "/Elsewhere")), alloc::format!("/Elsewhere: {above}"));
    assert_eq!(line(s.mv("a", "/Volumes")), alloc::format!("/Volumes/a: {above}"));
    let attached = "that is where something is attached, so it can't be removed, moved or replaced";
    assert_eq!(line(s.mv("/Home", "/Volumes/Data/h")), alloc::format!("/Home: {attached}"));
    assert_eq!(line(s.mv("/Volumes/Data", "x")), alloc::format!("/Volumes/Data: {attached}"));
    assert_eq!(
        line(s.mv("/System/rules.txt", "x")),
        alloc::format!("/System/rules.txt: {attached}")
    );
    let virt = "that folder only holds other folders, so it can't be changed";
    assert_eq!(line(s.mv("/", "x")), alloc::format!("/: {virt}"));
    assert_eq!(line(s.mv("/Volumes", "x")), alloc::format!("/Volumes: {virt}"));
    // Nothing moved.
    assert_eq!(s.fs().text("/Home/a").as_deref(), Some("x"));
    assert_eq!(s.fs().text("/Home/b").as_deref(), Some("y"));
    assert!(s.fs().renames.borrow().is_empty());
}

#[test]
fn df_lists_each_volume() {
    let s = shell();
    let (text, failures) = s.df();
    assert_eq!(
        text,
        "/Home  100 MB total, 40 MB free  (RockFS, \"Home\")\n\
         /Volumes/Data  2 GB total, 1 GB free  (FAT32)\n"
    );
    // The volume that can't answer is a failure of its own, not a reason to print nothing.
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].to_string(), "/Volumes/Usb: the service holding it has gone");
}

#[test]
fn the_filesystem_is_never_asked_about_virtual_folders() {
    // MemFs panics if it is; run every command at every awkward path.
    let mut s = shell();
    let paths = [
        "/",
        "/Volumes",
        "/System",
        "..",
        "/Volumes/..",
        "/System/.",
        "/Nothing",
        "/Volumes/Nope/x",
    ];
    for p in paths {
        let _ = s.ls(true, Some(p));
        let _ = s.cat(p);
        let _ = s.info(p);
        let _ = s.mkdir(p);
        let _ = s.write(p, "x");
        let _ = s.append(p, "x");
        let _ = s.rm(true, p);
        let _ = s.cp(true, p, "/Home/copy");
        let _ = s.cp(true, "/Home", p);
        let _ = s.mv(p, "/Home/moved");
        let _ = s.mv("/Home", p);
        let _ = s.cd(Some(p));
    }
    let _ = s.df();
}
