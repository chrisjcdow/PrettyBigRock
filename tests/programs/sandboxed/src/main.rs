// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `sandboxed`: the app the sandbox self-test launches through a session's sandbox manager
//! (docs/design/0017-users-sandbox-manager-and-manifests.md, section 8; note 0018, section 7,
//! test 6). It checks, from the inside, that it holds exactly what it was granted. It exits with
//! 0 if everything behaved, or the number of the first check that didn't, saying which in the
//! log.
//!
//! Its arguments are a mode, then the rest of the line:
//! - `view EXPECTED|WRITABLE|READONLY|ABSENT`: every capability it holds must be one it was
//!   given (11); walking its whole tree from `/` must find exactly `EXPECTED` (`path:d` or
//!   `path:f`, sorted, joined with `;`) (1); each path in `ABSENT` (joined with `;`) must not
//!   be there, by name (2) or by `..` from every entry (3); each entry in `WRITABLE` can be
//!   created in, written, renamed in and removed from (4); each entry in `READONLY` can't be
//!   created in (5) and nothing in it can be written, renamed or removed (6). `-` is an empty
//!   list.
//! - `entries EXPECTED`: its tree's entries are exactly `EXPECTED` (`path:d` or `path:f`,
//!   sorted, joined with `;`) (1).
//! - `lose`: opens `/Home/Documents/a.txt`, says it is ready by making
//!   `/Home/Documents/lose.ready`, and waits until the folder is revoked; then the file, the
//!   folder and opening through it must all fail (2 to 4).
//! - `hold`: makes `/Home/Documents/hold.ready`, waits for `/Home/Documents/hold.go`, and checks
//!   the folder still works (2 to 4): revoking another app's grant mustn't touch its own.
//! - `idle`: waits to be stopped.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::time;
use pbr_files::{Error, FileError, Files, READ, READ_WRITE, handle_kind};

granite::main!(main);

/// The kernel's type number for a memory object (note 0006): the program's own heap and stacks
/// are memory objects it made itself.
const MEMORY: u32 = 3;

type Outcome = Result<(), (i32, String)>;

fn main(mut start: granite::Start) -> i32 {
    let args = String::from(start.args());
    let (mode, rest) = args.split_once(' ').unwrap_or((args.as_str(), ""));
    let r = match mode {
        "view" => view(&mut start, rest),
        "entries" => entries(&mut start, rest),
        "lose" => lose(&mut start),
        "hold" => hold(&mut start),
        "idle" => loop {
            time::sleep_ms(1000);
        },
        _ => Err((99, format!("no mode in {args:?}"))),
    };
    match r {
        Ok(()) => 0,
        Err((n, why)) => {
            granite::println!("sandboxed: check {n} failed: {why}");
            n
        }
    }
}

fn check(n: i32, ok: bool, why: impl FnOnce() -> String) -> Outcome {
    if ok { Ok(()) } else { Err((n, why())) }
}

/// A list joined with `;`, or `-` for none.
fn list(s: &str) -> Vec<&str> {
    if s == "-" { Vec::new() } else { s.split(';').filter(|x| !x.is_empty()).collect() }
}

/// Every handle number this program can use names a capability it was given, or memory of its
/// own. Handles are a slot (bits 0 to 19) and a generation (bits 20 to 30), and slots are handed
/// out from the bottom, so the first 512 slots and their first few generations cover everything
/// a program this small holds.
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

/// `path:d` or `path:f` for each, sorted, joined with `;`.
fn describe(mut items: Vec<(String, bool)>) -> String {
    items.sort();
    let v: Vec<String> = items
        .iter()
        .map(|(p, folder)| format!("{p}:{}", if *folder { "d" } else { "f" }))
        .collect();
    v.join(";")
}

fn view(start: &mut granite::Start, rest: &str) -> Outcome {
    handle_scan(start)?;
    let files = Files::from_start(start);
    let fields: Vec<&str> = rest.split('|').collect();
    let [expected, writable, readonly, absent] = fields.as_slice() else {
        return Err((99, format!("view needs four fields, not {rest:?}")));
    };

    // Exactly what it was given, nothing more.
    let mut seen = Vec::new();
    walk(&files, "/", &mut seen).map_err(|e| (1, format!("walking the tree: {e}")))?;
    let found = describe(seen.clone());
    check(1, found == *expected, || format!("it sees {found}, not {expected}"))?;

    // No way to anything else: not by name, not by `..` from any entry.
    // Not there: outside every entry the tree says so, inside one the filesystem does.
    let gone = |e: &Error| matches!(e, Error::NotFound | Error::File(FileError::NotFound));
    let missing = |p: &str| {
        files.open(p, READ).err().as_ref().is_some_and(gone)
            && files.list(p).err().as_ref().is_some_and(gone)
    };
    let entries = files.entries();
    for a in list(absent) {
        check(2, missing(a), || format!("it reached {a}"))?;
        for (e, _) in &entries {
            let up = "../".repeat(e.matches('/').count());
            let p = format!("{e}/{up}{}", &a[1..]);
            check(3, missing(&p), || format!("{p} reached {a}"))?;
        }
    }

    // A writable entry: make, write, rename, read back, remove.
    for w in list(writable) {
        let (a, b) = (format!("{w}/probe.tmp"), format!("{w}/probe 2.tmp"));
        let r = (|| {
            let f = files.create(&a, false, READ_WRITE)?;
            f.save(b"probe")?;
            files.rename(&a, &b)?;
            let back = files.open(&b, READ)?.read_all()?;
            files.remove(&b)?;
            Ok::<_, Error>(back)
        })();
        check(4, r.as_deref() == Ok(b"probe"), || format!("using {w}: {r:?}"))?;
    }

    // A read-only entry: nothing in it can change.
    let no_right = |r: Result<(), Error>| matches!(r, Err(Error::File(FileError::NoRight)));
    for ro in list(readonly) {
        let folder = entries.iter().any(|(p, f)| p == ro && *f);
        if folder {
            let r = files.create(&format!("{ro}/new.tmp"), false, READ_WRITE).map(|_| ());
            check(5, no_right(r), || format!("created in {ro}"))?;
        }
        // Each file in it (or the entry itself, if it is a file).
        let inside = |p: &str| p == ro || p.starts_with(&format!("{ro}/"));
        for (p, _) in seen.iter().filter(|(p, f)| !*f && inside(p)) {
            // Opening for writing is refused, or gives something that can't be written.
            match files.open(p, READ_WRITE) {
                Err(Error::File(FileError::NoRight)) => {}
                Ok(f) => check(6, f.write_at(0, b"x").is_err(), || format!("wrote {p}"))?,
                Err(e) => return Err((6, format!("opening {p}: {e}"))),
            }
            if p != ro {
                check(6, no_right(files.remove(p)), || format!("removed {p}"))?;
                let to = format!("{p}.moved");
                check(6, no_right(files.rename(p, &to)), || format!("renamed {p}"))?;
            }
        }
    }
    Ok(())
}

fn entries(start: &mut granite::Start, expected: &str) -> Outcome {
    let files = Files::from_start(start);
    let found = describe(files.entries());
    check(1, found == expected, || format!("its entries are {found}, not {expected}"))
}

/// Waits up to a minute for `f` to be true, trying every 20 ms.
fn until(f: impl Fn() -> bool) -> bool {
    let end = time::after_ms(60_000);
    while time::now() < end {
        if f() {
            return true;
        }
        time::sleep_ms(20);
    }
    false
}

fn lose(start: &mut granite::Start) -> Outcome {
    let files = Files::from_start(start);
    let a = files.open("/Home/Documents/a.txt", READ).map_err(|e| (1, format!("{e}")))?;
    check(1, a.read_all().is_ok(), || "couldn't read a.txt at first".into())?;
    files
        .create("/Home/Documents/lose.ready", false, READ_WRITE)
        .map_err(|e| (1, format!("saying it's ready: {e}")))?;
    // Revoked: the file opened through the folder goes with it, and so does the folder.
    check(2, until(|| matches!(a.read_all(), Err(Error::Call(_)))), || {
        "a.txt still reads after a minute".into()
    })?;
    check(3, matches!(files.list("/Home/Documents"), Err(Error::Call(_))), || {
        "the revoked folder still lists".into()
    })?;
    check(4, files.open("/Home/Documents/a.txt", READ).is_err(), || "a.txt opens again".into())
}

fn hold(start: &mut granite::Start) -> Outcome {
    let files = Files::from_start(start);
    files
        .create("/Home/Documents/hold.ready", false, READ_WRITE)
        .map_err(|e| (1, format!("saying it's ready: {e}")))?;
    check(2, until(|| files.info("/Home/Documents/hold.go").is_ok()), || {
        "never told to carry on".into()
    })?;
    let a = files.open("/Home/Documents/a.txt", READ).map_err(|e| (3, format!("{e}")))?;
    check(3, a.read_all().as_deref() == Ok(b"hello"), || "a.txt doesn't read".into())?;
    check(4, files.list("/Home/Documents").is_ok(), || "the folder doesn't list".into())
}
