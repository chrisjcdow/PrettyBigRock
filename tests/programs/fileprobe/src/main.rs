// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `fileprobe`: a program that checks, from the inside, that it sees exactly the files it was
//! given (docs/design/0017-users-sandbox-manager-and-manifests.md, section 8; note 0018,
//! section 7). It exits with 0 if everything behaved, or the number of the first check that
//! didn't, saying which in the log.
//!
//! - `view EXPECTED`: before anything else, tries every handle number in its first 512 table
//!   slots and checks that every capability it holds (other than its own memory) is one it was
//!   given. Then walks its whole file tree from `/` and compares what it finds with `EXPECTED`
//!   (`path:d` or `path:f`, sorted, joined with `;`), then tries what it shouldn't be able to
//!   do. Its tree is set up by the root program's files test: `/Home/Documents` (read and
//!   write), `/Home/Pictures` (read only) and the single file `/Home/Notes/todo.txt`.
//! - `revoke`: opens `/Home/Documents/a.txt`, says it's ready, waits until it's told the folder
//!   has been revoked, and checks that the file and the folder no longer work.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::notification::Notification;
use granite::time;
use pbr_files::{Error, Files, READ, READ_WRITE, handle_kind};
use pbr_test_protocol::role;

granite::main!(main);

/// The kernel's type number for a memory object (note 0006): the program's own heap and stacks
/// are memory objects it made itself.
const MEMORY: u32 = 3;

fn main(mut start: granite::Start) -> i32 {
    let args: Vec<String> = start.args().split_whitespace().map(String::from).collect();
    let r = match args.first().map(String::as_str) {
        Some("view") => view(&mut start, args.get(1).map_or("", String::as_str)),
        Some("revoke") => revoke(&mut start),
        _ => Err((99, String::from("no mode"))),
    };
    match r {
        Ok(()) => 0,
        Err((n, why)) => {
            granite::println!("fileprobe: check {n} failed: {why}");
            n
        }
    }
}

type Outcome = Result<(), (i32, String)>;

fn check(n: i32, ok: bool, why: impl FnOnce() -> String) -> Outcome {
    if ok { Ok(()) } else { Err((n, why())) }
}

/// Every handle number this program can use names a capability it was given, or memory of its
/// own. Handles are a slot (bits 0 to 19) and a generation (bits 20 to 30), and slots are
/// handed out from the bottom, so the first 512 slots and their first few generations cover
/// everything a program this small holds.
fn handle_scan(start: &granite::Start) -> Outcome {
    let given: Vec<u32> = start.handles().iter().map(|&(_, h)| h).collect();
    for slot in 1..=512u32 {
        for generation in 0..4u32 {
            let h = generation << 20 | slot;
            if let Some((kind, _)) = handle_kind(h)
                && kind != MEMORY
                && !given.contains(&h)
            {
                return Err((11, format!("handle {h:#x} (type {kind}) was never given to it")));
            }
        }
    }
    Ok(())
}

/// Everything reachable from `path`, with whether each is a folder.
fn walk(files: &Files, path: &str, out: &mut Vec<(String, bool)>) -> Result<(), Error> {
    for item in files.list(path)? {
        let p =
            if path == "/" { format!("/{}", item.name) } else { format!("{path}/{}", item.name) };
        out.push((p.clone(), item.folder));
        if item.folder {
            walk(files, &p, out)?;
        }
    }
    Ok(())
}

fn view(start: &mut granite::Start, expected: &str) -> Outcome {
    handle_scan(start)?;
    let files = Files::from_start(start);

    // Exactly what it was given, nothing more.
    let mut seen = Vec::new();
    walk(&files, "/", &mut seen).map_err(|e| (1, format!("walking the tree: {e}")))?;
    // Sorted by path, then written as `path:d` or `path:f`.
    seen.sort();
    let seen: Vec<String> =
        seen.iter().map(|(p, folder)| format!("{p}:{}", if *folder { "d" } else { "f" })).collect();
    let seen = seen.join(";");
    check(1, seen == expected, || format!("it sees {seen}, not {expected}"))?;

    // No way out: not by name, not by `..`, not by guessing.
    let missing = |p: &str| matches!(files.open(p, READ), Err(Error::NotFound));
    check(2, missing("/Home/Secret/s.txt"), || "it reached /Home/Secret".into())?;
    check(3, missing("/Home/Documents/../Secret/s.txt"), || "`..` reached /Home/Secret".into())?;
    check(3, missing("/Home/Documents/../../Volumes"), || "`..` reached /Volumes".into())?;
    check(3, matches!(files.list("/Volumes"), Err(Error::NotFound)), || "listed /Volumes".into())?;
    // `..` that stays inside what it holds works, as anywhere else.
    check(4, files.open("/Home/Documents/../Pictures/p.png", READ).is_ok(), || {
        "a path through `..` to something it holds failed".into()
    })?;

    // A read-only folder stays read-only, and so does everything in it.
    let no_right =
        |r: Result<(), Error>| matches!(r, Err(Error::File(pbr_files::FileError::NoRight)));
    check(
        5,
        no_right(files.create("/Home/Pictures/new.txt", false, READ_WRITE).map(|_| ())),
        || "created a file in a read-only folder".into(),
    )?;
    check(6, no_right(files.open("/Home/Pictures/p.png", READ_WRITE).map(|_| ())), || {
        "opened a read-only file for writing".into()
    })?;
    check(6, no_right(files.remove("/Home/Pictures/p.png")), || "removed a read-only file".into())?;

    // The single file it was given works, read and write.
    let todo = files.open("/Home/Notes/todo.txt", READ_WRITE).map_err(|e| (7, format!("{e}")))?;
    todo.save(b"done").map_err(|e| (7, format!("saving: {e}")))?;
    let back = todo.read_all().map_err(|e| (7, format!("{e}")))?;
    check(7, back == b"done", || "todo.txt read back wrongly".into())?;

    // The folders above its entries can only be listed.
    let above = |r: Result<(), Error>| matches!(r, Err(Error::Above));
    check(8, above(files.create("/Home/x", false, READ_WRITE).map(|_| ())), || {
        "created in /Home".into()
    })?;
    check(8, above(files.remove("/Home/Documents")), || "removed an entry".into())?;

    // The read-write folder: write, rename, read back.
    let a = files.open("/Home/Documents/a.txt", READ_WRITE).map_err(|e| (9, format!("{e}")))?;
    a.write_at(5, b", world").map_err(|e| (9, format!("{e}")))?;
    files
        .rename("/Home/Documents/a.txt", "/Home/Documents/b.txt")
        .map_err(|e| (9, format!("{e}")))?;
    let b = files.open("b.txt", READ);
    check(9, b.is_err(), || "a relative path worked from / without a current folder".into())?;
    let b = files.open("/Home/Documents/b.txt", READ).map_err(|e| (9, format!("{e}")))?;
    let text = b.read_all().map_err(|e| (9, format!("{e}")))?;
    check(9, text == b"hello, world", || format!("b.txt says {text:?}"))?;
    files
        .rename("/Home/Documents/b.txt", "/Home/Documents/a.txt")
        .map_err(|e| (9, format!("{e}")))?;
    Ok(())
}

fn revoke(start: &mut granite::Start) -> Outcome {
    let ready = start.take(role::READY).map(Notification).ok_or((1, String::from("no READY")))?;
    let go = start.take(role::GO).map(Notification).ok_or((1, String::from("no GO")))?;
    let files = Files::from_start(start);
    let a = files.open("/Home/Documents/a.txt", READ).map_err(|e| (2, format!("{e}")))?;
    check(2, a.read_all().is_ok(), || "couldn't read a.txt at first".into())?;
    ready.signal(1).map_err(|e| (3, format!("{e}")))?;
    go.wait(time::after_ms(60_000)).map_err(|e| (3, format!("waiting to be told: {e}")))?;
    // The folder was revoked: the file opened through it went with it, and so did the folder.
    check(4, matches!(a.read_all(), Err(Error::Call(_))), || "a.txt still reads".into())?;
    check(5, matches!(files.list("/Home/Documents"), Err(Error::Call(_))), || {
        "the revoked folder still lists".into()
    })?;
    check(6, files.open("/Home/Documents/a.txt", READ).is_err(), || "a.txt opens again".into())
}
