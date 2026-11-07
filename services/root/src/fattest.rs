// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The FAT32 boot test (docs/design/0016-filesystems.md, section 5): the "USB" volume on the
//! MBR test disk, a FAT32 volume made by the build tool (`xtask/src/testdisks.rs`), is served
//! by its own `fat` server. Here the root program reads the file the build tool put there,
//! then writes, renames and moves files, with long and non-ASCII names, through `pbr.file`.
//! The storage service is shut down later (by `classictest`), which has the server mark the
//! volume clean. After the boot the test runner checks the volume with `fsck.fat -n` and reads
//! everything back with `mtools` (`tools/check_fat_after.py`, which must agree with
//! [`HELLO`], [`UNICODE`] and [`big`]).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::time;
use pbr_interfaces::pbr_file::{self as file, FileRights, NodeKind};
use pbr_interfaces::pbr_storage::{self as storage, Filesystem, VolumeState};

/// What the build tool wrote in `README.TXT` (`README` in xtask/src/testdisks.rs).
const README: &[u8] = b"A FAT32 test volume for PrettyBigRock's storage tests.\n";
/// The volume: partition 2 of the MBR test disk, 34 MiB.
const SIZE: u64 = 34 * 1024 * 1024;

pub const HELLO: &[u8] = b"Hello, FAT, from PrettyBigRock 2.\n";
/// A long name that isn't ASCII, so it can only be stored in long-name entries.
pub const UNICODE: &str = "Ünïcödé — a long name for a file.txt";
/// `big.bin`: byte `i` is `i * 13 % 253`, 200,000 bytes (many clusters).
pub fn big() -> Vec<u8> {
    (0..200_000usize).map(|i| (i * 13 % 253) as u8).collect()
}

const ALL: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

fn read_all(f: &file::Client) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let chunk = f.read(out.len() as u64, 65536).map_err(|e| format!("reading: {e}"))?;
        if chunk.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(&chunk);
    }
}

/// Makes a file at `path` holding `data`, saved.
fn put(root: &file::Client, path: &str, data: &[u8]) -> Result<(), String> {
    let f = root.create(path, NodeKind::File, ALL).map_err(|e| format!("making {path}: {e}"))?.node;
    for (i, chunk) in data.chunks(65536).enumerate() {
        f.write((i * 65536) as u64, chunk).map_err(|e| format!("writing {path}: {e}"))?;
    }
    f.sync().map_err(|e| format!("saving {path}: {e}"))
}

fn get(root: &file::Client, path: &str) -> Result<Vec<u8>, String> {
    let f = root.open(path, FileRights::READ).map_err(|e| format!("opening {path}: {e}"))?;
    read_all(&f.node)
}

/// Runs the test. Returns a short summary.
pub fn run(admin: &storage::Client) -> Result<String, String> {
    let end = time::after_ms(30_000);
    let volume = loop {
        let list = admin.list().map_err(|e| format!("listing volumes: {e}"))?;
        let found = list.iter().find(|v| {
            v.filesystem == Filesystem::Fat32
                && v.label == "USB"
                && v.partition == 2
                && v.size == SIZE
        });
        match found {
            Some(v) if v.state == VolumeState::Ready => break v.clone(),
            Some(v) if v.state != VolumeState::Starting => {
                return Err(format!("the FAT32 volume is {:?}", v.state));
            }
            _ if time::now() > end => {
                return Err(format!("the FAT32 volume isn't served: {list:?}"));
            }
            _ => time::sleep_ms(100),
        }
    };
    let mut root = file::Client::new(Endpoint(
        admin.open(volume.id).map_err(|e| format!("opening the FAT32 volume: {e}"))?,
    ));
    root.set_timeout_ms(60_000);
    let space = root.space().map_err(|e| format!("its space: {e}"))?;
    if space.filesystem != "FAT32" || space.label != "USB" {
        return Err(format!("the FAT32 volume says it is {space:?}"));
    }

    // What the build tool wrote, found whatever its case.
    if get(&root, "readme.txt")? != README {
        return Err("README.TXT reads back wrongly".into());
    }

    // New files, a folder, long and non-ASCII names, a file across many clusters.
    let fail = |what: &'static str| {
        move |e: granite::interface::CallError<file::FileError>| format!("{what}: {e}")
    };
    root.create("From PrettyBigRock 2", NodeKind::Folder, ALL).map_err(fail("making a folder"))?;
    put(&root, "From PrettyBigRock 2/hello.txt", HELLO)?;
    put(&root, &format!("From PrettyBigRock 2/{UNICODE}"), b"")?;
    let data = big();
    put(&root, "From PrettyBigRock 2/big.bin", &data)?;
    // A rename within the folder, and a move out of it.
    root.rename("From PrettyBigRock 2/hello.txt", "From PrettyBigRock 2/Hello again.txt", false)
        .map_err(fail("renaming"))?;
    root.rename("From PrettyBigRock 2/big.bin", "big.bin", false).map_err(fail("moving"))?;
    // Names FAT can't hold are refused.
    if root.create("bad:name.txt", NodeKind::File, ALL).is_ok() {
        return Err("a name with ':' was accepted".into());
    }
    // Everything reads back.
    if get(&root, "From PrettyBigRock 2/Hello again.txt")? != HELLO
        || get(&root, "BIG.BIN")? != data
        || !get(&root, &format!("From PrettyBigRock 2/{UNICODE}"))?.is_empty()
        || root.open("From PrettyBigRock 2/hello.txt", FileRights::READ).is_ok()
    {
        return Err("a change reads back wrongly".into());
    }
    let listing = root.list(0, 56).map_err(fail("listing"))?;
    let mut names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    if names != ["From PrettyBigRock 2", "README.TXT", "big.bin"] {
        return Err(format!("the volume lists as {names:?}"));
    }
    Ok("read what the build tool wrote; long and non-ASCII names, a folder, a large file, a rename and a move".into())
}
