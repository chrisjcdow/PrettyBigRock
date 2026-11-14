// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The files self-test (docs/design/0014-files-and-file-trees.md; note 0018, section 7, test
//! 3), with `memfs`, a filesystem server whose filesystem is in memory, standing in for RockFS
//! until its server exists (stage 4b):
//! - the root program, as a client, checks `pbr.file` end to end: derived capabilities, and
//!   records freed by gone notices;
//! - `fileprobe`, given a file tree, checks it sees exactly what it was given and can do
//!   nothing it shouldn't;
//! - revoking a folder given to `fileprobe` takes the files it opened through it.

use crate::selftest::{MIB, wait};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, Event};
use granite::interface::CallError;
use granite::notification::Notification;
use granite::process::{self, Spawn, Spawned, State, role as start_role};
use granite::sys::rights;
use granite::{Error, time};
use pbr_archive::Archive;
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeKind};
use pbr_interfaces::pbr_fs_control as control;
use pbr_interfaces::pbr_fs_host::{self as host, FsFailure};
use pbr_test_protocol::role;

const RW: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
const R: FileRights = FileRights::READ;

/// What `memfs` sends when it's ready.
#[derive(Default)]
struct Host {
    root: Option<file::Client>,
    control: Option<control::Client>,
}

impl host::Server for Host {
    fn ready(
        &mut self,
        root: Endpoint,
        control: Endpoint,
        _label: String,
        reply: host::ready::Reply,
    ) {
        // A server is handed plain endpoints; a client wraps one to make typed calls on it.
        self.root = Some(file::Client::new(root));
        self.control = Some(control::Client::new(control));
        let _ = reply.ok();
    }
    fn failed(&mut self, _reason: FsFailure, reply: host::failed::Reply) {
        let _ = reply.ok();
    }
    // (Servers here are never started to check their volumes.)
    fn checked(&mut self, _report: String, _problems: u32, reply: host::checked::Reply) {
        let _ = reply.err(host::HostError::BadState);
    }
}

fn spawn(
    archive: &Archive<'_>,
    name: &str,
    args: &str,
    caps: &[(u32, u32, u32, bool)],
    tree: &[(&str, u32, bool)],
) -> Result<Spawned, String> {
    let f = archive.find(name).ok_or_else(|| format!("{name} isn't in the archive"))?;
    process::spawn(&Spawn {
        name,
        program: f.data,
        budget: 16 * MIB,
        priority: 1,
        caps,
        log: true,
        args,
        tied: false,
        file_tree: tree,
    })
    .map_err(|e| format!("starting {name}: {e}"))
}

fn fail<T: core::fmt::Debug>(what: &str) -> impl Fn(T) -> String + '_ {
    move |e| format!("{what}: {e:?}")
}

/// Runs the files self-test. Returns a short summary.
pub fn run(archive: &Archive<'_>) -> Result<String, String> {
    // Start `memfs`, and take its root folder when it says it's ready.
    let own = Endpoint::create().map_err(fail("an endpoint"))?;
    let hosting = Endpoint::create().map_err(fail("an endpoint"))?;
    let server = spawn(
        archive,
        "memfs",
        "",
        &[
            (start_role::OWN, own.0.raw(), rights::ALL, false),
            (
                start_role::PARENT,
                hosting.0.raw(),
                rights::SEND | rights::TRANSFER | rights::DUPLICATE,
                false,
            ),
        ],
        &[],
    )?;
    drop(own);
    let mut h = Host::default();
    let mut buffer = alloc::vec![0u8; host::REQUEST_BUFFER];
    match hosting.receive(&mut buffer, time::after_ms(30_000)) {
        Ok(Event::Call(msg, reply)) => {
            let _ = host::dispatch(&mut h, msg, &buffer, reply);
        }
        _ => return Err("memfs never said it was ready".into()),
    }
    let (Some(mut root), Some(mut ctl)) = (h.root, h.control) else {
        return Err("memfs's ready call was malformed".into());
    };
    root.set_deadline(time::FOREVER);
    ctl.set_timeout_ms(30_000);
    let r = (|| -> Result<String, String> {
        let summary = client(&root, &ctl)?;
        sandboxed(archive, &root)?;
        Ok(summary)
    })();
    let _ = server.process.stop(0);
    let _ = wait(&server, 10);
    r
}

/// Opens `path` below `dir` with `rights`.
fn open(dir: &file::Client, path: &str, rights: FileRights) -> Result<file::Client, String> {
    dir.open(path, rights).map(|r| r.node).map_err(|e| format!("opening {path}: {e}"))
}

fn create(dir: &file::Client, path: &str, folder: bool) -> Result<file::Client, String> {
    let kind = if folder { NodeKind::Folder } else { NodeKind::File };
    dir.create(path, kind, RW).map(|r| r.node).map_err(|e| format!("creating {path}: {e}"))
}

/// Waits for the server's record count to come back to `want` (gone notices arrive after the
/// capabilities go, in the server's own time).
fn records_back(ctl: &control::Client, want: u32) -> Result<(), String> {
    let end = time::after_ms(10_000);
    loop {
        let n = ctl.records().map_err(fail("asking for the records"))?;
        if n == want {
            return Ok(());
        }
        if time::now() > end {
            return Err(format!(
                "{n} records, not {want}: records aren't freed when capabilities go"
            ));
        }
        time::sleep_ms(20);
    }
}

/// `pbr.file` from the root program: the tree the sandboxed tests use, derived capabilities
/// and records.
fn client(root: &file::Client, ctl: &control::Client) -> Result<String, String> {
    let home = create(root, "Home", true)?;
    for d in ["Documents", "Pictures", "Notes", "Secret"] {
        create(&home, d, true)?;
    }
    for (path, text) in [
        ("Documents/a.txt", "hello"),
        ("Pictures/p.png", "not really a picture"),
        ("Notes/todo.txt", "write M5"),
        ("Secret/s.txt", "nobody else may read this"),
    ] {
        let f = create(&home, path, false)?;
        f.write(0, text.as_bytes()).map_err(fail("writing"))?;
        f.sync().map_err(fail("syncing"))?;
    }
    let listing = home.list(0, 56).map_err(fail("listing"))?;
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    if names != ["Documents", "Notes", "Pictures", "Secret"] || listing.next != 0 {
        return Err(format!("Home lists as {names:?}"));
    }

    // A capability derived through another is revoked with it, and its record goes.
    let base = ctl.records().map_err(fail("asking for the records"))?;
    let docs = open(&home, "Documents", RW)?;
    let a = open(&docs, "a.txt", R)?;
    if a.read(0, 100).map_err(fail("reading"))? != b"hello" {
        return Err("a.txt read back wrongly".into());
    }
    docs.endpoint().0.revoke().map_err(fail("revoking"))?;
    match a.read(0, 100) {
        Err(CallError::Call(Error::BadHandle)) => {}
        other => {
            return Err(format!("a file opened through a revoked folder still answers: {other:?}"));
        }
    }
    drop(a);
    drop(docs);
    records_back(ctl, base)?;

    // Records go when capabilities are closed, however many there were.
    for _ in 0..200 {
        let f = open(&home, "Notes/todo.txt", R)?;
        drop(f);
    }
    records_back(ctl, base)?;

    // Rights only shrink, through the server too.
    let ro = home.reduce(R).map_err(fail("reducing"))?;
    match ro.create("Documents/new.txt", NodeKind::File, R) {
        Err(CallError::Service(FileError::NoRight)) => {}
        other => return Err(format!("a read-only folder created a file: {other:?}")),
    }
    match ro.open("Documents/a.txt", RW) {
        Err(CallError::Service(FileError::NoRight)) => {}
        other => return Err(format!("a read-only folder opened a file for writing: {other:?}")),
    }
    match home.open("../Secret", R) {
        Err(CallError::Service(FileError::BadPath)) => {}
        other => return Err(format!("`..` went to the server: {other:?}")),
    }
    Ok(format!("{base} records at rest"))
}

/// `fileprobe`, given a tree: sees exactly it, and loses what is revoked.
fn sandboxed(archive: &Archive<'_>, root: &file::Client) -> Result<(), String> {
    let home = open(root, "Home", RW)?;
    let docs = open(&home, "Documents", RW)?;
    let pics = open(&home, "Pictures", R)?;
    let todo = open(&home, "Notes/todo.txt", RW)?;
    let tree = [
        ("/Home/Documents", docs.endpoint().0.raw(), true),
        ("/Home/Pictures", pics.endpoint().0.raw(), true),
        ("/Home/Notes/todo.txt", todo.endpoint().0.raw(), false),
    ];
    let expected = "/Home:d;/Home/Documents:d;/Home/Documents/a.txt:f;/Home/Notes:d;\
                    /Home/Notes/todo.txt:f;/Home/Pictures:d;/Home/Pictures/p.png:f";
    let p = spawn(archive, "fileprobe", &format!("view {expected}"), &[], &tree)?;
    match wait(&p, 60)? {
        State::Exited(0) => {}
        State::Exited(n) => return Err(format!("fileprobe view: check {n} failed")),
        other => return Err(format!("fileprobe view ended {other:?}")),
    }

    // Revoking a folder a running program holds: it loses the folder, and what it opened
    // through it. `grant` is the root program's; the program gets a copy of it.
    let grant = open(&home, "Documents", R)?;
    let ready = Notification::create().map_err(fail("a notification"))?;
    let go = Notification::create().map_err(fail("a notification"))?;
    let caps = [
        (role::READY, ready.0.raw(), rights::ALL, false),
        (role::GO, go.0.raw(), rights::ALL, false),
    ];
    let p = spawn(
        archive,
        "fileprobe",
        "revoke",
        &caps,
        &[("/Home/Documents", grant.endpoint().0.raw(), true)],
    )?;
    ready.wait(time::after_ms(60_000)).map_err(fail("waiting for fileprobe"))?;
    grant.endpoint().0.revoke().map_err(fail("revoking"))?;
    go.signal(1).map_err(fail("signalling"))?;
    match wait(&p, 60)? {
        State::Exited(0) => {}
        State::Exited(n) => return Err(format!("fileprobe revoke: check {n} failed")),
        other => return Err(format!("fileprobe revoke ended {other:?}")),
    }
    // The root program's own folder still works.
    if docs.list(0, 10).is_err() {
        return Err("revoking a copy took the root program's own folder".into());
    }
    Ok(())
}
