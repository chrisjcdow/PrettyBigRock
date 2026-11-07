// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The test with a data disk made by PrettyBigRock 1.3 itself
//! (docs/design/0016-filesystems.md, section 4, layer 4; note 0018, section 7).
//!
//! `cargo xtask classic-disk` made the disk by booting 1.3 and typing at it; the boot tests
//! attach a fresh copy. Here PrettyBigRock 2 finds the volume, compares every folder and file
//! with the manifest taken when it was made, and then changes it: a new folder with new files
//! (one of them large), a line added to an existing file, a rename within a folder, a move to
//! another folder, and a removal. The storage service is then shut down, which has every
//! server save its volume and mark it shut down cleanly. After the boot, the test runner checks
//! the disk with `tools/rockfs.py` and with 1.3's own C code, and reads the changes back
//! (xtask/src/qemu.rs, `check_classic_disk`). The two must agree on [`CHANGES`].

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::time;
use pbr_interfaces::pbr_file::{self as file, FileRights, NodeKind};
use pbr_interfaces::pbr_storage::{self as storage, Filesystem, VolumeState};

/// Every folder and file on the disk when 1.3 had made it: `D 0 00000000 /path` or
/// `F size crc32 /path`, from `cargo xtask classic-disk`. (`include_str!` puts a file's text
/// into the program when it is built.)
const MANIFEST: &str = include_str!("../../../tests/data/classic-1.3.manifest");

/// The volume's size: the 16 MiB disk less the first MiB, which 1.3 leaves before its
/// partition.
const SIZE: u64 = 15 * 1024 * 1024;

/// What this test changes, which the test runner checks afterwards. Keep it the same as
/// `CHANGES` in tools/check_classic_after.py.
pub const CHANGES: &[(&str, &str)] = &[
    ("new", "/From PrettyBigRock 2/hello.txt"),
    ("new", "/From PrettyBigRock 2/big.bin"),
    ("append", "/Documents/letter.txt"),
    ("rename", "/Projects/Rock/moved.txt -> /Projects/Rock/renamed by 2.txt"),
    ("move", "/Projects/Rock/Deep/notes.txt -> /Documents/notes.txt"),
    ("remove", "/Fill/f01"),
];

const HELLO: &[u8] = b"Hello from PrettyBigRock 2.\n";
const PS: &[u8] = b"PS from PrettyBigRock 2.\n";
/// `big.bin`'s size and contents: byte `i` is `i * 7 % 251` (251 is prime, so the pattern
/// doesn't line up with blocks and a misplaced block shows).
const BIG: usize = 300_000;

fn big() -> Vec<u8> {
    (0..BIG).map(|i| (i * 7 % 251) as u8).collect()
}

const ALL: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

fn open(root: &file::Client, path: &str) -> Result<(file::Client, file::NodeInfo), String> {
    root.open(path.trim_start_matches('/'), ALL)
        .map(|r| (r.node, r.info))
        .map_err(|e| format!("opening {path}: {e}"))
}

/// All of a file.
fn read_all(f: &file::Client, path: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let chunk = f.read(out.len() as u64, 65536).map_err(|e| format!("reading {path}: {e}"))?;
        if chunk.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(&chunk);
    }
}

/// Every folder and file below `folder`, as manifest lines (sorted by the caller).
fn walk(f: &file::Client, path: &str, out: &mut Vec<String>) -> Result<(), String> {
    let mut cursor = 0;
    loop {
        let page = f.list(cursor, 56).map_err(|e| format!("listing {path}: {e}"))?;
        for e in &page.entries {
            let p = format!("{path}/{}", e.name);
            let (child, _) = open(f, &e.name)?;
            if e.kind == NodeKind::Folder {
                out.push(format!("D 0 00000000 {p}"));
                walk(&child, &p, out)?;
            } else {
                let data = read_all(&child, &p)?;
                out.push(format!("F {} {:08x} {p}", data.len(), pbr_crc::crc32(&data)));
            }
        }
        if page.next == 0 {
            return Ok(());
        }
        cursor = page.next;
    }
}

/// Runs the test, then shuts the storage service down. Returns a short summary.
pub fn run(admin: &storage::Client) -> Result<String, String> {
    // The volume's server mounts it at start-up; wait until it says it's ready.
    let end = time::after_ms(30_000);
    let volume = loop {
        let list = admin.list().map_err(|e| format!("listing volumes: {e}"))?;
        let found = list.iter().find(|v| {
            v.filesystem == Filesystem::RockFs
                && v.label == "Data"
                && v.partition == 1
                && v.size == SIZE
        });
        match found {
            Some(v) if v.state == VolumeState::Ready => break v.clone(),
            Some(v) if v.state != VolumeState::Starting => {
                return Err(format!("the disk 1.3 made is {:?}", v.state));
            }
            _ if time::now() > end => {
                return Err(format!("the disk 1.3 made isn't served: {list:?}"));
            }
            _ => time::sleep_ms(100),
        }
    };
    let mut root = file::Client::new(Endpoint(
        admin.open(volume.id).map_err(|e| format!("opening the volume: {e}"))?,
    ));
    root.set_deadline(time::after_ms(60_000));

    // Exactly what 1.3 left, byte for byte.
    let mut found = Vec::new();
    walk(&root, "", &mut found)?;
    found.sort();
    let mut want: Vec<&str> = MANIFEST.lines().filter(|l| !l.is_empty()).collect();
    want.sort();
    if found != want {
        let missing: Vec<&&str> = want.iter().filter(|l| !found.iter().any(|f| f == **l)).collect();
        let extra: Vec<&String> = found.iter().filter(|f| !want.contains(&f.as_str())).collect();
        return Err(format!(
            "the disk 1.3 made differs: missing {missing:?}, unexpected {extra:?}"
        ));
    }
    let items = found.len();

    // Changes, as a program would make them.
    let fail = |what: &'static str| {
        move |e: granite::interface::CallError<file::FileError>| format!("{what}: {e}")
    };
    root.create("From PrettyBigRock 2", NodeKind::Folder, ALL).map_err(fail("making a folder"))?;
    let hello = root
        .create("From PrettyBigRock 2/hello.txt", NodeKind::File, ALL)
        .map_err(fail("making a file"))?
        .node;
    hello.write(0, HELLO).map_err(fail("writing"))?;
    hello.sync().map_err(fail("saving"))?;
    let big_file = root
        .create("From PrettyBigRock 2/big.bin", NodeKind::File, ALL)
        .map_err(fail("making a file"))?
        .node;
    let data = big();
    for (i, chunk) in data.chunks(65536).enumerate() {
        big_file.write((i * 65536) as u64, chunk).map_err(fail("writing a large file"))?;
    }
    big_file.sync().map_err(fail("saving a large file"))?;
    let (letter, info) = open(&root, "/Documents/letter.txt")?;
    letter.write(info.size, PS).map_err(fail("adding to a file"))?;
    letter.sync().map_err(fail("saving"))?;
    root.rename("Projects/Rock/moved.txt", "Projects/Rock/renamed by 2.txt", false)
        .map_err(fail("renaming"))?;
    root.rename("Projects/Rock/Deep/notes.txt", "Documents/notes.txt", false)
        .map_err(fail("moving"))?;
    root.remove("Fill/f01").map_err(fail("removing"))?;
    // They read back at once.
    if read_all(&big_file, "big.bin")? != data || !read_all(&letter, "letter.txt")?.ends_with(PS) {
        return Err("a change read back wrongly".into());
    }
    drop((hello, big_file, letter, root));

    // Shutting down saves every volume and marks it clean; the runner checks the disk after.
    admin.shutdown().map_err(|e| format!("shutting the storage service down: {e}"))?;
    Ok(format!(
        "{items} folders and files as 1.3 left them, {} changes, shut down cleanly",
        CHANGES.len()
    ))
}
