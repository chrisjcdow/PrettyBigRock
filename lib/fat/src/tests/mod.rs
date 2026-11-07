// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests of the whole filesystem, on disk images in memory. This module holds what they share.
//!
//! - `semantics`: what each operation does, on volumes made by our own formatter below;
//! - `mkfs`: reading volumes made by `mkfs.fat` and filled by `mtools`;
//! - `write`: what we write must pass `fsck.fat` and read back through `mtools`;
//! - `xtask`: reading what the build tool's FAT32 writer makes (the EFI system partition);
//! - `model`: random operations checked against a simple model, with remounts;
//! - `space`: full disks;
//! - `hostile`: damaged and hostile volumes;
//! - `clean`: the "cleanly unmounted" flag and what a crash leaves.
//!
//! The tests that need `mkfs.fat`, `fsck.fat` or `mtools` print a line saying they were skipped
//! if the tool is missing, and pass (`tools.rs`).

mod clean;
mod format;
mod hostile;
mod mkfs;
mod model;
mod semantics;
mod space;
mod tools;
mod write;
mod xtask;

pub use format::{blank, blank_with};

use crate::Fat;
use core::sync::atomic::{AtomicU64, Ordering};
use pbr_fs::memdisk::MemDisk;
use pbr_fs::{Filesystem, FsError, Kind, NodeId};
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

/// A clock that moves forward two seconds each time it is read (FAT keeps two-second steps),
/// so times can be compared.
pub fn clock() -> u64 {
    static TIME: AtomicU64 = AtomicU64::new(1_800_000_000);
    TIME.fetch_add(2, Ordering::Relaxed)
}

/// Mounts `disk` for writing.
pub fn mount(disk: MemDisk) -> Fat<MemDisk> {
    Fat::mount(disk, clock, false).unwrap()
}

/// A freshly formatted volume with 512-byte clusters, mounted.
pub fn fresh() -> Fat<MemDisk> {
    mount(blank())
}

/// Unmounts cleanly and mounts again, as a program restarting would.
pub fn remount(mut fs: Fat<MemDisk>) -> Fat<MemDisk> {
    fs.unmount().unwrap();
    mount(fs.into_device())
}

/// Walks a path of names from the root.
pub fn node_at(fs: &mut Fat<MemDisk>, path: &str) -> Result<NodeId, FsError> {
    let mut node = fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        node = fs.lookup(node, part)?;
    }
    Ok(node)
}

/// Creates (or empties) the file at `path`, whose folder must exist, writes `data` in the pieces
/// of up to 64 KiB a server makes, and commits it.
pub fn put(fs: &mut Fat<MemDisk>, path: &str, data: &[u8]) -> Result<NodeId, FsError> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let parent = node_at(fs, dir)?;
    let node = match fs.lookup(parent, name) {
        Ok(n) => {
            fs.set_size(n, 0)?;
            n
        }
        Err(FsError::NotFound) => fs.create(parent, name, Kind::File)?,
        Err(e) => return Err(e),
    };
    for (i, chunk) in data.chunks(65536).enumerate() {
        fs.write(node, i as u64 * 65536, chunk)?;
    }
    fs.commit(node)?;
    Ok(node)
}

/// Makes a folder at `path` (its parent must exist).
pub fn mkdir(fs: &mut Fat<MemDisk>, path: &str) -> Result<NodeId, FsError> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let parent = node_at(fs, dir)?;
    fs.create(parent, name, Kind::Folder)
}

/// Reads a whole file.
pub fn read_all(fs: &mut Fat<MemDisk>, node: NodeId) -> Result<Vec<u8>, FsError> {
    let size = fs.stat(node)?.size as usize;
    let mut out = std::vec![0u8; size];
    let mut at = 0;
    while at < out.len() {
        let end = (at + 65536).min(out.len());
        let got = fs.read(node, at as u64, &mut out[at..end])?;
        assert_eq!(got, end - at);
        at = end;
    }
    Ok(out)
}

/// The names in a folder, in the order listed, fetched three at a time to try the cursor.
pub fn names(fs: &mut Fat<MemDisk>, folder: NodeId) -> Vec<String> {
    let mut all = Vec::new();
    let mut cursor = 0;
    loop {
        let mut page = 0;
        cursor = fs
            .list(folder, cursor, &mut |e| {
                if page == 3 {
                    return false;
                }
                page += 1;
                all.push(e.name);
                true
            })
            .unwrap();
        if cursor == 0 {
            return all;
        }
    }
}

/// Deterministic bytes: the same `(seed, len)` always gives the same data.
pub fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// A small random number generator for tests.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A whole tree: the path of every file and folder (starting with `/`, parts joined by `/`) and
/// the file's contents, or `None` for a folder.
pub type Tree = BTreeMap<String, Option<Vec<u8>>>;

/// Lists a folder completely, in one call.
pub fn list_all(fs: &mut Fat<MemDisk>, folder: NodeId) -> Vec<pbr_fs::Entry> {
    let mut entries = Vec::new();
    let end = fs
        .list(folder, 0, &mut |e| {
            entries.push(e);
            true
        })
        .unwrap();
    assert_eq!(end, 0, "an accepting callback gets the whole folder");
    entries
}

/// Walks the whole volume through the `Filesystem` interface.
pub fn tree_of(fs: &mut Fat<MemDisk>) -> Tree {
    let mut tree = Tree::new();
    let mut stack = std::vec![(String::new(), fs.root())];
    while let Some((path, folder)) = stack.pop() {
        for e in list_all(fs, folder) {
            let full = std::format!("{path}/{}", e.name);
            let id = fs.lookup(folder, &e.name).unwrap();
            match e.kind {
                Kind::Folder => {
                    tree.insert(full.clone(), None);
                    stack.push((full, id));
                }
                Kind::File => {
                    let data = read_all(fs, id).unwrap();
                    assert_eq!(e.size, data.len() as u64, "{full}: size in the listing");
                    tree.insert(full, Some(data));
                }
            }
        }
    }
    tree
}

/// The same tree with every path folded to lower case (A-Z only), for comparing with a tool that
/// may spell a short name in capitals.
pub fn folded(tree: &Tree) -> Tree {
    tree.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect()
}

/// Asserts two trees are the same, ignoring A-Z case in names, with a message that names the
/// first difference (not a dump of every byte).
#[track_caller]
pub fn assert_same_tree(got: &Tree, want: &Tree, what: &str) {
    let (got, want) = (folded(got), folded(want));
    let missing: Vec<&String> = want.keys().filter(|k| !got.contains_key(*k)).collect();
    let extra: Vec<&String> = got.keys().filter(|k| !want.contains_key(*k)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{what}: missing {missing:?}, unexpected {extra:?}"
    );
    for (name, data) in &want {
        let have = &got[name];
        match (have, data) {
            (Some(a), Some(b)) => {
                let at = a.iter().zip(b).position(|(x, y)| x != y);
                assert!(
                    a.len() == b.len() && at.is_none(),
                    "{what}: {name}: {} bytes against {}, first difference at {at:?}",
                    a.len(),
                    b.len()
                );
            }
            _ => assert!(have.is_some() == data.is_some(), "{what}: {name}: file or folder?"),
        }
    }
}
