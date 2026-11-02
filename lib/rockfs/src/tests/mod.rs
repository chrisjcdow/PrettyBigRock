// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests of the whole filesystem, on disk images in memory. This module holds what they share.
//!
//! The simulated power cuts that test the "never half old and half new" promise are written
//! separately, with a [`BlockDevice`] that loses writes between flushes; `RockFs` is generic over
//! the device so it slots in (see `helpers::fresh_on`).

mod basic;
mod check;
mod hostile;
mod mounting;
mod random;
mod space;
mod trees;

use crate::RockFs;
use core::sync::atomic::{AtomicU64, Ordering};
use pbr_fs::memdisk::MemDisk;
use pbr_fs::{BlockDevice, Filesystem, FsError, Kind, NodeId};
use std::string::String;
use std::vec::Vec;

/// A clock that moves forward one second each time it is read, so times can be compared.
pub fn clock() -> u64 {
    static TIME: AtomicU64 = AtomicU64::new(1_800_000_000);
    TIME.fetch_add(1, Ordering::Relaxed)
}

pub const UUID: [u8; 16] = *b"0123456789abcdef";

/// A formatted, unmounted disk of `blocks` 4 KiB blocks.
pub fn formatted(blocks: u64) -> MemDisk {
    let mut disk = MemDisk::new(blocks * 8);
    crate::format(&mut disk, "Test", UUID, 1_700_000_000).unwrap();
    disk
}

/// A freshly formatted volume, mounted.
pub fn fresh(blocks: u64) -> RockFs<MemDisk> {
    fresh_on(formatted(blocks))
}

/// Mounts a freshly formatted `disk` of any device type.
pub fn fresh_on<D: BlockDevice>(disk: D) -> RockFs<D> {
    RockFs::mount(disk, clock, false).unwrap()
}

/// Unmounts cleanly and mounts again, as a program restarting would.
pub fn remount(mut fs: RockFs<MemDisk>) -> RockFs<MemDisk> {
    fs.unmount().unwrap();
    RockFs::mount(fs.into_device(), clock, false).unwrap()
}

/// Walks a path of names from the root.
pub fn node_at(fs: &mut RockFs<MemDisk>, path: &str) -> Result<NodeId, FsError> {
    let mut node = fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        node = fs.lookup(node, part)?;
    }
    Ok(node)
}

/// Creates a file at `path` (its folder must exist) holding `data`, in the writes of up to
/// 64 KiB a server makes, and commits it.
pub fn put(fs: &mut RockFs<MemDisk>, path: &str, data: &[u8]) -> Result<NodeId, FsError> {
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
pub fn mkdir(fs: &mut RockFs<MemDisk>, path: &str) -> Result<NodeId, FsError> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let parent = node_at(fs, dir)?;
    fs.create(parent, name, Kind::Folder)
}

/// Reads a whole file.
pub fn read_all(fs: &mut RockFs<MemDisk>, node: NodeId) -> Result<Vec<u8>, FsError> {
    let size = fs.stat(node)?.size;
    let mut out = std::vec![0u8; size as usize];
    let mut at = 0;
    while at < out.len() {
        let end = (at + 65536).min(out.len());
        let got = fs.read(node, at as u64, &mut out[at..end])?;
        assert_eq!(got, end - at);
        at = end;
    }
    Ok(out)
}

/// The names in a folder, in the order listed.
pub fn names(fs: &mut RockFs<MemDisk>, folder: NodeId) -> Vec<String> {
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

/// Asserts a clean check, with the problems in the message if not.
pub fn assert_clean(fs: &mut RockFs<MemDisk>) {
    let problems = fs.check(false).unwrap();
    assert!(problems.is_empty(), "check found: {problems:#?}");
}
