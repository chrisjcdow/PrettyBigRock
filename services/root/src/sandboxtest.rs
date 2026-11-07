// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the users service, and the users and sandbox self-test
//! (docs/design/0017-users-sandbox-manager-and-manifests.md, section 8; note 0018, section 7,
//! test 6): M5's first "done when", at the level of real apps.
//!
//! The build tool makes the GPT test disk's RockFS volume a system volume
//! (`xtask/src/testdisks.rs`, `SYSTEM`): `chris`, an administrator, and `alice`, a standard
//! user, with their homes and rules. The root program stands in for a person at the keyboard:
//! through `pbr.users.debug` it gets each session's `pbr.users` and `pbr.sandbox`, as the shell
//! will, and launches the test app `apps/sandboxed` (`tests/programs/sandboxed`), which checks
//! from the inside that it holds exactly what it was granted. The root program also holds the
//! storage service's `pbr.storage`, so it can look at the system volume directly, and change a
//! user's rules between launches (rules are read afresh at each launch).

use crate::storagetest::Storage;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::notification::Notification;
use granite::process::{self, Process, Spawn, Spawned, State, role as start_role};
use granite::sys::rights;
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeKind};
use pbr_interfaces::pbr_sandbox::{self as sandbox, GrantReason, SandboxError};
use pbr_interfaces::pbr_storage::{Filesystem, VolumeState};
use pbr_interfaces::pbr_users::{self as users, User, UserKind, UsersError};
use pbr_interfaces::pbr_users_debug as users_debug;

/// The users service's start-up role for `pbr.storage` (`services/users`'s `STORAGE`).
const STORAGE: u32 = 1001;
/// The badge of the root program's `pbr.users.debug` (`services/users`'s `DEBUG`).
const DEBUG: u64 = 1;
/// The users service's budget: up to three sessions of 16 MiB at once, and its own needs.
const BUDGET: u64 = 64 * 1024 * 1024;
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;
const R: FileRights = FileRights::READ;
const RW: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

/// The users service, and the root program's way to talk to it.
pub struct Users {
    pub process: Spawned,
    pub debug: users_debug::Client,
}

/// Starts the users service, giving it a copy of the root program's `pbr.storage` and the
/// start-up archive (its sandbox managers read apps from it).
pub fn start(
    archive: &Archive<'_>,
    archive_mem: u32,
    storage: &Storage,
    args: &str,
) -> Result<Users, String> {
    let program = archive.find("users").ok_or("users isn't in the archive")?;
    let ep = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;
    let st = storage
        .admin
        .endpoint()
        .0
        .duplicate(CLIENT)
        .map_err(|e| format!("copying the storage service's endpoint: {e}"))?;
    let caps = [
        (start_role::OWN, ep.0.raw(), rights::ALL, false),
        // The archive can be copied, as the users service passes it on to sandbox managers.
        (start_role::ARCHIVE, archive_mem, rights::READ | rights::MAP | CLIENT, false),
        (STORAGE, st.into_raw(), rights::ALL, true),
    ];
    let process = process::spawn(&Spawn {
        name: "users",
        program: program.data,
        budget: BUDGET,
        priority: 2,
        caps: &caps,
        log: true,
        args,
        tied: false,
        file_tree: &[],
    })
    .map_err(|e| format!("starting users: {e}"))?;
    let mut debug =
        users_debug::Client::new(ep.mint(DEBUG, CLIENT).map_err(|e| format!("minting: {e}"))?);
    // It finds the system volume (waiting for volumes still starting) before it answers.
    debug.set_timeout_ms(120_000);
    Ok(Users { process, debug })
}

/// A session, as its shell would hold it.
struct Session {
    users: users::Client,
    sandbox: sandbox::Client,
}

fn session(u: &Users, name: &str) -> Result<Session, String> {
    let r = u.debug.session(name).map_err(|e| format!("{name}'s session: {e}"))?;
    let mut users = users::Client::new(Endpoint(r.users));
    let mut sandbox = sandbox::Client::new(Endpoint(r.sandbox));
    users.set_timeout_ms(60_000);
    sandbox.set_timeout_ms(60_000);
    Ok(Session { users, sandbox })
}

/// `path:d` or `path:f` for each, sorted, joined with `;`: how `sandboxed` describes a tree.
fn tree(items: &[(&str, bool)]) -> String {
    let mut v: Vec<(&str, bool)> = items.to_vec();
    v.sort();
    let v: Vec<String> =
        v.iter().map(|(p, d)| format!("{p}:{}", if *d { "d" } else { "f" })).collect();
    v.join(";")
}

/// Expects `r` to be the service error `want`.
fn refused<T: core::fmt::Debug, E: core::fmt::Debug + PartialEq>(
    what: &str,
    r: Result<T, CallError<E>>,
    want: E,
) -> Result<(), String> {
    match r {
        Err(CallError::Service(e)) if e == want => Ok(()),
        other => Err(format!("{what}: expected {want:?}, got {other:?}")),
    }
}

/// Waits up to `secs` seconds for a process to end, and says how it did.
fn wait(p: &Process, secs: u64) -> Result<State, String> {
    let n = Notification::create().map_err(|e| format!("{e}"))?;
    p.watch(&n, 1).map_err(|e| format!("watching: {e}"))?;
    let _ = n.wait(time::after_ms(secs * 1000));
    match p.info().map(|i| i.state) {
        Ok(State::Running | State::Ending) => Err(format!("still running after {secs} seconds")),
        Ok(s) => Ok(s),
        Err(e) => Err(format!("{e}")),
    }
}

/// Launches `sandboxed` with `args` in session `s`, with the chosen files `chosen`, and expects
/// it to exit with 0.
fn run_app(s: &Session, args: &str, chosen: Vec<(&str, file::Client)>) -> Result<(), String> {
    let (_, p) = launch(s, args, chosen)?;
    match wait(&p, 60)? {
        State::Exited(0) => Ok(()),
        State::Exited(n) => Err(format!("sandboxed {args}: check {n} failed (see the log)")),
        other => Err(format!("sandboxed {args} ended {other:?}")),
    }
}

fn launch(
    s: &Session,
    args: &str,
    chosen: Vec<(&str, file::Client)>,
) -> Result<(u32, Process), String> {
    let id = s.sandbox.prepare("sandboxed", args).map_err(|e| format!("preparing: {e}"))?;
    for (path, node) in chosen {
        s.sandbox
            .choose(id, path, node.into_endpoint())
            .map_err(|e| format!("choosing {path}: {e}"))?;
    }
    let r = s.sandbox.start(id).map_err(|e| format!("starting sandboxed {args}: {e}"))?;
    Ok((r.app, r.process))
}

/// Every grant app `id` holds: (path, rights, reason).
fn grants(s: &Session, id: u32) -> Result<Vec<(String, u32, GrantReason)>, String> {
    let mut out = Vec::new();
    let mut from = 0;
    loop {
        let r = s.sandbox.grants(id, from).map_err(|e| format!("grants: {e}"))?;
        out.extend(r.grants.into_iter().map(|g| (g.path, g.rights.0, g.reason)));
        if r.next == 0 {
            return Ok(out);
        }
        from = r.next;
    }
}

/// The system volume, seen directly through `pbr.storage`.
struct Disk {
    root: file::Client,
}

impl Disk {
    fn open(&self, path: &str, rights: FileRights) -> Result<file::Client, String> {
        let mut c = self.root.open(path, rights).map_err(|e| format!("opening {path}: {e}"))?.node;
        c.set_timeout_ms(30_000);
        Ok(c)
    }

    fn exists(&self, path: &str) -> bool {
        self.root.open(path, R).is_ok()
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.open(path, R)?.read(0, 65536).map_err(|e| format!("reading {path}: {e}"))
    }

    /// Replaces the file at `path` (made if need be) with `text`.
    fn write(&self, path: &str, text: &str) -> Result<(), String> {
        let f = match self.root.create(path, NodeKind::File, RW) {
            Ok(r) => r.node,
            Err(CallError::Service(FileError::AlreadyExists)) => self.open(path, RW)?,
            Err(e) => return Err(format!("making {path}: {e}")),
        };
        let fail = |e| format!("writing {path}: {e}");
        f.set_size(0).map_err(fail)?;
        f.write(0, text.as_bytes()).map_err(fail)?;
        f.sync().map_err(fail)
    }

    fn remove(&self, path: &str) {
        let _ = self.root.remove(path);
    }

    /// Waits up to a minute for `path` to appear.
    fn appears(&self, path: &str) -> Result<(), String> {
        let end = time::after_ms(60_000);
        while time::now() < end {
            if self.exists(path) {
                return Ok(());
            }
            time::sleep_ms(20);
        }
        Err(format!("{path} never appeared"))
    }
}

fn user(name: &str, kind: UserKind, display: &str) -> User {
    User { name: name.into(), kind, display: display.into() }
}

/// The users and sandbox self-test. Returns a short summary.
pub fn run(archive: &Archive<'_>, archive_mem: u32, storage: &Storage) -> Result<String, String> {
    let u = start(archive, archive_mem, storage, "selftest")?;
    let r = with_system_volume(&u, storage).map_err(|e| {
        let state = u.process.process.info().map(|i| i.state);
        format!("{e} (the users service: {state:?})")
    });
    let _ = u.process.process.stop(0);
    let _ = wait(&u.process.process, 30);
    let summary = r?;
    without_system_volume(archive, archive_mem, storage)?;
    Ok(summary)
}

fn with_system_volume(u: &Users, storage: &Storage) -> Result<String, String> {
    // The system volume is the GPT disk's RockFS volume, `Home`.
    let list = storage.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
    let home = list
        .iter()
        .find(|v| v.filesystem == Filesystem::RockFs && v.label == "Home")
        .ok_or("no Home volume")?;
    let sys = u.debug.system().map_err(|e| format!("asking for the system volume: {e}"))?;
    if sys.volume != home.id || sys.in_memory {
        return Err(format!("the system volume is {sys:?}, not volume {}", home.id));
    }
    let mut root = file::Client::new(Endpoint(
        storage.admin.open(home.id).map_err(|e| format!("opening the system volume: {e}"))?,
    ));
    root.set_timeout_ms(30_000);
    let disk = Disk { root };

    // Two sessions, kept apart: who they are, and what each may ask the users service.
    let chris = session(u, "chris")?;
    let alice = session(u, "alice")?;
    let me = chris.users.whoami().map_err(|e| format!("whoami: {e}"))?;
    if me != user("chris", UserKind::Admin, "Chris") {
        return Err(format!("chris's session is {me:?}"));
    }
    let me = alice.users.whoami().map_err(|e| format!("whoami: {e}"))?;
    if me != user("alice", UserKind::Standard, "Alice") {
        return Err(format!("alice's session is {me:?}"));
    }
    let all = chris.users.list().map_err(|e| format!("listing users: {e}"))?;
    if all.len() != 2 || all[1] != me {
        return Err(format!("the users are {all:?}"));
    }
    let no = UsersError::NotAllowed;
    refused("alice listing users", alice.users.list(), no)?;
    refused("alice listing volumes", alice.users.volumes(), no)?;
    refused("alice adding a user", alice.users.add(&user("eve", UserKind::Admin, "Eve")), no)?;
    refused("alice setting up a volume", alice.users.setup(home.id, "eve"), no)?;
    refused("alice formatting", alice.users.format(home.id, "X"), no)?;

    // Launches the sandbox manager refuses.
    refused("an unknown app", chris.sandbox.prepare("nothing", ""), SandboxError::NoSuchApp)?;
    refused("a bad app name", chris.sandbox.prepare("../root", ""), SandboxError::NoSuchApp)?;
    refused("a broken manifest", chris.sandbox.prepare("broken", ""), SandboxError::BadManifest)?;
    refused("an unknown launch", chris.sandbox.start(999), SandboxError::NotFound)?;

    let places = places(&chris, &disk)?;
    let revoked = revoking(&chris, &disk)?;
    trust(&chris, &alice, &disk, storage)?;
    let added = adding(u, &chris, &disk, storage)?;
    Ok(format!("{places}, {revoked}, trust and rules, {added}"))
}

/// Chris's rules (`distrust sandboxed`, `deny sandboxed music`): the app gets the places its
/// manifest asks for except music, as a system app, and the file chosen for it.
fn places(chris: &Session, disk: &Disk) -> Result<String, String> {
    let todo = || disk.open("Users/chris/Notes/todo.txt", RW);
    // Choosing: the path must be tidy and absolute; what the app gets anyway is accepted
    // without being added; what would give it more than it gets, or contain what it gets, is
    // refused; a capability that isn't a file's is refused.
    let id = chris.sandbox.prepare("sandboxed", "idle").map_err(|e| format!("preparing: {e}"))?;
    let choose =
        |path: &str, node: file::Client| chris.sandbox.choose(id, path, node.into_endpoint());
    refused("a relative path", choose("Home/x", todo()?), SandboxError::BadPath)?;
    refused("an untidy path", choose("/Home/../Home/x", todo()?), SandboxError::BadPath)?;
    choose("/Home/Documents/a.txt", disk.open("Users/chris/Documents/a.txt", RW)?)
        .map_err(|e| format!("choosing a file the app gets anyway: {e}"))?;
    refused(
        "choosing more than the app gets",
        choose("/Home/Pictures/p.png", disk.open("Users/chris/Pictures/p.png", RW)?),
        SandboxError::Overlaps,
    )?;
    refused(
        "choosing a folder around what the app gets",
        choose("/Home", disk.open("Users/chris", RW)?),
        SandboxError::Overlaps,
    )?;
    // (An endpoint nobody serves any more: calls on it fail at once.)
    let gone = Endpoint::create().map_err(|e| format!("an endpoint: {e}"))?;
    let not_a_file = file::Client::new(gone.mint(1, CLIENT).map_err(|e| format!("minting: {e}"))?);
    drop(gone);
    refused(
        "choosing something that isn't a file",
        choose("/Home/x", not_a_file),
        SandboxError::BadCapability,
    )?;
    choose("/Home/Notes/todo.txt", todo()?).map_err(|e| format!("choosing: {e}"))?;
    let r = chris.sandbox.start(id).map_err(|e| format!("starting: {e}"))?;
    let (app, process) = (r.app, r.process);

    // What it holds, and why; what the caller can't do with its process.
    let held = grants(chris, app)?;
    let want = [
        ("/Home/Documents", RW.0, GrantReason::Manifest),
        ("/Home/Pictures", R.0, GrantReason::Manifest),
        ("/Home/Notes/todo.txt", RW.0, GrantReason::Chosen),
    ];
    if held.len() != want.len() || held.iter().zip(want).any(|(h, w)| (h.0.as_str(), h.1, h.2) != w)
    {
        return Err(format!("the app holds {held:?}"));
    }
    let apps = chris.sandbox.list().map_err(|e| format!("listing apps: {e}"))?;
    if !apps.iter().any(|a| a.id == app && a.app == "sandboxed" && !a.trusted) {
        return Err(format!("the running apps are {apps:?}"));
    }
    if process.stop(0).is_ok() {
        return Err("the launcher's process capability could stop the app".into());
    }
    chris.sandbox.stop(app).map_err(|e| format!("stopping: {e}"))?;
    if !matches!(wait(&process, 30)?, State::Stopped(_)) {
        return Err("the app wasn't stopped".into());
    }
    refused("an ended app's grants", chris.sandbox.grants(app, 0), SandboxError::NotFound)?;
    refused("stopping it again", chris.sandbox.stop(app), SandboxError::NotFound)?;

    // Inside, it sees exactly that: no music (denied), nothing else of chris's, nobody else's.
    let expected = tree(&[
        ("/Home", true),
        ("/Home/Documents", true),
        ("/Home/Documents/a.txt", false),
        ("/Home/Notes", true),
        ("/Home/Notes/todo.txt", false),
        ("/Home/Pictures", true),
        ("/Home/Pictures/p.png", false),
    ]);
    let absent = "/Home/Music;/Home/Music/m.ogg;/Home/Secret;/Home/Secret/s.txt;/Home/Videos;\
                  /Home/Notes/other.txt;/Users/alice;/Users/chris;/Volumes;/System/rules.txt;/System";
    let args = format!("view {expected}|/Home/Documents|/Home/Pictures|{absent}");
    run_app(chris, &args, alloc::vec![("/Home/Notes/todo.txt", todo()?)])?;
    if disk.read("Users/chris/Documents/a.txt")? != b"hello" {
        return Err("the app changed a.txt".into());
    }
    Ok(String::from("places, choosing and stopping"))
}

/// Revoking one app's grant takes it, and everything opened through it, while another app's
/// grant of the same folder keeps working.
fn revoking(chris: &Session, disk: &Disk) -> Result<String, String> {
    let docs = "Users/chris/Documents";
    let (lose, lp) = launch(chris, "lose", Vec::new())?;
    let (_, hp) = launch(chris, "hold", Vec::new())?;
    disk.appears(&format!("{docs}/lose.ready"))?;
    disk.appears(&format!("{docs}/hold.ready"))?;
    refused(
        "revoking what it doesn't hold",
        chris.sandbox.revoke(lose, "/Home/Music"),
        SandboxError::NotFound,
    )?;
    chris.sandbox.revoke(lose, "/Home/Documents").map_err(|e| format!("revoking: {e}"))?;
    let r = (|| {
        match wait(&lp, 90)? {
            State::Exited(0) => {}
            other => return Err(format!("sandboxed lose ended {other:?} (see the log)")),
        }
        disk.write(&format!("{docs}/hold.go"), "")?;
        match wait(&hp, 60)? {
            State::Exited(0) => Ok(()),
            other => Err(format!("sandboxed hold ended {other:?} (see the log)")),
        }
    })();
    for f in ["lose.ready", "hold.ready", "hold.go"] {
        disk.remove(&format!("{docs}/{f}"));
    }
    r?;
    Ok(String::from("revoking"))
}

/// Trust, and rules cutting rights down (rules are read at each launch, so the root program
/// changes them between launches).
fn trust(chris: &Session, alice: &Session, disk: &Disk, storage: &Storage) -> Result<(), String> {
    let chris_rules = "System/rules/chris.txt";
    let alice_rules = "System/rules/alice.txt";
    let before = disk.read(chris_rules)?;

    // An administrator's trusted app: the session's whole tree, every volume included.
    disk.write(chris_rules, "")?;
    let list = storage.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
    let ready: Vec<&str> =
        list.iter().filter(|v| v.state == VolumeState::Ready).map(|v| v.label.as_str()).collect();
    let names: Vec<String> =
        pbr_manifest::volume_names(&ready).iter().map(|n| format!("/Volumes/{n}")).collect();
    let mut entries: Vec<(&str, bool)> = alloc::vec![("/Home", true), ("/System/rules.txt", false)];
    entries.extend(names.iter().map(|n| (n.as_str(), true)));
    let r = run_app(chris, &format!("entries {}", tree(&entries)), Vec::new());
    disk.write(chris_rules, core::str::from_utf8(&before).unwrap_or(""))?;
    r?;

    // A standard user's trusted app: their whole tree, which holds no volume and nothing of
    // anyone else's; their rules can be read, not changed.
    let expected = tree(&[
        ("/Home", true),
        ("/Home/Documents", true),
        ("/Home/Documents/diary.txt", false),
        ("/Home/Downloads", true),
        ("/Home/Music", true),
        ("/Home/Pictures", true),
        ("/Home/Pictures/q.png", false),
        ("/Home/Videos", true),
        ("/System", true),
        ("/System/rules.txt", false),
    ]);
    let absent = "/Volumes;/Users/chris;/Users/alice;/Home/Secret;/System/users.txt;/System/rules";
    run_app(alice, &format!("view {expected}|/Home|/System/rules.txt|{absent}"), Vec::new())?;

    // Rules: distrusted, it gets its places; `allow` cuts documents down to reading.
    disk.write(alice_rules, "distrust sandboxed\nallow sandboxed documents read\n")?;
    let expected = tree(&[
        ("/Home", true),
        ("/Home/Documents", true),
        ("/Home/Documents/diary.txt", false),
        ("/Home/Music", true),
        ("/Home/Pictures", true),
        ("/Home/Pictures/q.png", false),
    ]);
    let ro = "/Home/Documents;/Home/Pictures;/Home/Music";
    let r = run_app(
        alice,
        &format!("view {expected}|-|{ro}|/System/rules.txt;/Volumes;/Home/Videos"),
        Vec::new(),
    );
    // A rules file that can't be read refuses every launch, rather than guessing.
    let r = r.and_then(|()| {
        disk.write(alice_rules, "allow sandboxed\n")?;
        refused(
            "a broken rules file",
            alice.sandbox.prepare("sandboxed", "idle"),
            SandboxError::BadRules,
        )
    });
    disk.write(alice_rules, "")?;
    r
}

/// Adding a user, and setting up another volume as a system volume.
fn adding(u: &Users, chris: &Session, disk: &Disk, storage: &Storage) -> Result<String, String> {
    let bob = user("bob", UserKind::Standard, "Bob");
    chris.users.add(&bob).map_err(|e| format!("adding bob: {e}"))?;
    refused("adding bob again", chris.users.add(&bob), UsersError::AlreadyExists)?;
    refused(
        "a bad name",
        chris.users.add(&user("Bob", UserKind::Standard, "B")),
        UsersError::BadName,
    )?;
    refused(
        "a bad display name",
        chris.users.add(&user("b", UserKind::Standard, "\"")),
        UsersError::BadName,
    )?;
    let text = disk.read("System/users.txt")?;
    let listed = pbr_manifest::users::parse(&text).map_err(|e| format!("users.txt: {e:?}"))?;
    if listed.len() != 3 || listed[2].name != "bob" {
        return Err(format!("users.txt says {listed:?}"));
    }
    for p in ["Users/bob/Documents", "Users/bob/Downloads", "System/rules/bob.txt"] {
        if !disk.exists(p) {
            return Err(format!("adding bob didn't make {p}"));
        }
    }
    let b = session(u, "bob")?;
    if b.users.whoami().map_err(|e| format!("whoami: {e}"))? != bob {
        return Err("bob's session isn't bob's".into());
    }

    // Setting up: the volume the storage test formatted becomes a system volume, from the
    // next start.
    let vols = chris.users.volumes().map_err(|e| format!("volumes: {e}"))?;
    let find = |label: &str| vols.iter().find(|v| v.label == label).map(|v| v.id);
    let formatted = find("Formatted").ok_or("no Formatted volume")?;
    let usb = find("USB").ok_or("no USB volume")?;
    let data = find("Data").ok_or("no Data volume")?;
    refused("a bad name", chris.users.setup(formatted, "Dave"), UsersError::BadName)?;
    refused("no such volume", chris.users.setup(9999, "dave"), UsersError::NoSuchVolume)?;
    refused("a FAT32 volume", chris.users.setup(usb, "dave"), UsersError::Unsuitable)?;
    refused(
        "the system volume",
        chris.users.setup(disk_id(storage, "Home")?, "dave"),
        UsersError::AlreadyExists,
    )?;
    refused(
        "formatting a volume with data",
        chris.users.format(data, "X"),
        UsersError::Unsuitable,
    )?;
    chris.users.setup(formatted, "dave").map_err(|e| format!("setting up: {e}"))?;
    refused(
        "setting it up again",
        chris.users.setup(formatted, "dave"),
        UsersError::AlreadyExists,
    )?;
    let mut root = file::Client::new(Endpoint(
        storage.admin.open(formatted).map_err(|e| format!("opening it: {e}"))?,
    ));
    root.set_timeout_ms(30_000);
    let it = Disk { root };
    let text = it.read("System/users.txt")?;
    let listed = pbr_manifest::users::parse(&text).map_err(|e| format!("its users.txt: {e:?}"))?;
    if listed.len() != 1
        || listed[0].name != "dave"
        || listed[0].kind != pbr_manifest::users::Kind::Admin
    {
        return Err(format!("the new users.txt says {listed:?}"));
    }
    for p in ["Users/dave/Documents", "System/rules/dave.txt", "storage test.txt"] {
        if !it.exists(p) {
            return Err(format!("after setting up, {p} isn't there"));
        }
    }
    Ok(String::from("adding a user, setting up a volume"))
}

fn disk_id(storage: &Storage, label: &str) -> Result<u32, String> {
    let list = storage.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
    list.iter().find(|v| v.label == label).map(|v| v.id).ok_or(format!("no {label} volume"))
}

/// With no system volume (here: told to ignore it), the users are kept in memory, with an
/// administrator called `owner`.
fn without_system_volume(
    archive: &Archive<'_>,
    archive_mem: u32,
    storage: &Storage,
) -> Result<(), String> {
    let u = start(archive, archive_mem, storage, "selftest nosystem")?;
    let r = (|| {
        let sys = u.debug.system().map_err(|e| format!("asking for the system volume: {e}"))?;
        let list = storage.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
        let v = list.iter().find(|v| v.id == sys.volume).ok_or("the system volume isn't listed")?;
        if !sys.in_memory || !v.in_memory || v.label != "Memory" {
            return Err(format!("without a system volume, the users are kept on {v:?}"));
        }
        let owner = session(&u, "owner")?;
        let me = owner.users.whoami().map_err(|e| format!("whoami: {e}"))?;
        if me != user("owner", UserKind::Admin, "Owner") {
            return Err(format!("the owner's session is {me:?}"));
        }
        Ok(())
    })();
    let _ = u.process.process.stop(0);
    let _ = wait(&u.process.process, 30);
    r
}
