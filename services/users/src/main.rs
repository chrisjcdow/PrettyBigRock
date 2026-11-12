// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The users service (docs/design/0017-users-sandbox-manager-and-manifests.md, sections 1 to
//! 3): it knows the users, keeps the system volume's `System` and `Users` folders, and starts
//! a **session** for each user who is logged in: a sandbox manager (`services/sandbox`) of
//! their own, holding their capabilities and nobody else's.
//!
//! A capability is the right to use something, held by the kernel on a program's behalf
//! (note 0006). This service holds the storage service's `pbr.storage`, the power over every
//! volume, and hands each session only its part:
//! - every user: their home, `Users/NAME` on the system volume, at `/Home`, and their rules
//!   file, `System/rules/NAME.txt`, read only, at `/System/rules.txt`;
//! - an administrator also: every volume's root folder, at `/Volumes/LABEL`.
//!
//! The **system volume** is the first RockFS volume the storage service lists that has
//! `System/users.txt`. Without one, the users are kept on a volume in memory, with one
//! administrator called `owner`, and the service says so. (A damaged `users.txt` is treated the
//! same way, so that someone can still log in, as `owner`, to repair it through `/Volumes`.)
//!
//! It serves, on one endpoint, told apart by badges (a badge is a number a service stamps on
//! each capability it hands out, so it knows who is calling; note 0008):
//! - [`SESSION_BASE`] + a session's number: `pbr.users`, for that session;
//! - [`DEBUG`]: `pbr.users.debug`, only in self-test runs, for the root program.
//!
//! Sessions are tied to this service (note 0013): if it ends, they end. A session whose sandbox
//! manager ends is started again (note 0018, section 3).
//!
//! **The terminal** (note 0018, sections 5 and 6). The console serves one terminal, on the
//! screen; this service finds it through the supervisor (`lookup("console")`) and gives it to
//! one session, the first administrator's (the one started at start-up), whose sandbox manager
//! starts the shell on it. If the console goes (it ended, and was restarted), the terminal stops
//! working: that session is started again with the new console's terminal. If there is no
//! console yet, this service keeps asking for one, and starts the session again once there is.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, Event};
use granite::handle::Handle;
use granite::interface::{CallError, OutCap};
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::{self, Spawn, Spawned, State, role};
use granite::sys::{perm, rights};
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeKind};
use pbr_interfaces::pbr_seat::{self as seat, SeatError};
use pbr_interfaces::pbr_storage::{self as storage, StorageError, VolumeState};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_interfaces::pbr_terminal as terminal;
use pbr_interfaces::pbr_users::{self as users_if, UserKind, UsersError};
use pbr_interfaces::pbr_users_debug::{self as debug_if, DebugError};
use pbr_manifest::settings::{self, Settings};
use pbr_manifest::users::{self as users_txt, Kind, User};
use pbr_manifest::{Place, is_name, volume_names};
use pbr_supervise::{Decision, History};

granite::main!(main);

/// The badge of the root program's `pbr.users.debug` (self-test runs only).
const DEBUG: u64 = 1;
/// Sessions' badges: this plus the session's number.
const SESSION_BASE: u64 = 0x100;

/// What a sandbox manager is started with (see `services/sandbox`): its session's `pbr.users`
/// at this role, and the badge of the `pbr.sandbox` capability minted for the session.
const SANDBOX_USERS: u32 = 1001;
const SANDBOX_SESSION: u64 = 1;
/// The role the session that owns the terminal gets its `pbr.terminal` at (`services/sandbox`
/// uses the same number).
const SANDBOX_TERMINAL: u32 = 1002;
/// A session's budget: the sandbox manager and every app it starts are paid from it.
const SESSION_BUDGET: u64 = 16 << 20;
/// Sandbox managers run at the priority of services.
const SESSION_PRIORITY: u8 = 2;
/// The volume in memory used when there is no system volume.
const RAM_VOLUME: u64 = 8 << 20;
/// The label of a volume `setup` has to format first.
const NEW_LABEL: &str = "PrettyBigRock";
/// A capability that can be passed on: what clients are given.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;
const READ: FileRights = FileRights::READ;
const READ_WRITE: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
/// How long to wait for the storage service and filesystem servers.
const CALL_MS: u64 = 60_000;
/// How often to check that the storage service and the volumes the sessions hold are still
/// there (note 0018, section 3), and how long such a check may wait.
const CHECK_MS: u64 = 500;
const CHECK_TIMEOUT_MS: u64 = 5_000;
/// The bit of `Service::ended` that the kernel signals when the input service has gone (a
/// receiver watch, note 0019). Sessions use the bits below it.
const INPUT_GONE: u64 = 1 << 63;

/// Where the users are kept.
struct System {
    volume: u32,
    in_memory: bool,
    root: file::Client,
}

/// A user's session.
struct Session {
    number: u32,
    user: User,
    /// Its bit in [`Service::ended`].
    bit: u32,
    sandbox: Spawned,
    /// In self-test runs, a `pbr.sandbox` capability for the session, for the root program.
    test_cap: Option<Endpoint>,
    /// The capabilities its tree was made from: the sandbox manager got copies, so these are
    /// the parents of everything the session holds. They are also how a volume going is
    /// noticed (`Service::check`).
    held: Vec<file::Client>,
    /// Its sandbox manager's crashes, for the restart policy.
    history: History,
}

/// A session waiting to be started again after its sandbox manager ended.
struct Pending {
    user: User,
    history: History,
    /// When (milliseconds on the clock).
    due: u64,
}

struct Service {
    endpoint: Endpoint,
    /// The supervisor (the root program), through which the storage service is found.
    parent: supervisor::Client,
    storage: storage::Client,
    /// Self-test only: behave as if there were no system volume.
    ignore_system: bool,
    /// Sessions to start again.
    pending: Vec<Pending>,
    /// The start-up archive, for sandbox managers (who read apps from it).
    archive: Handle,
    /// The sandbox manager's program.
    sandbox: Vec<u8>,
    system: System,
    users: Vec<User>,
    sessions: Vec<Session>,
    next_session: u32,
    /// The console's terminal, while there is one that answers.
    console: Option<terminal::Client>,
    /// Whose session gets the terminal: the first administrator's.
    console_user: Option<String>,
    /// Signalled with a session's bit when its sandbox manager ends; bound to the endpoint.
    ended: Notification,
    /// An administrator asked for `shutdown`.
    shut_down: bool,
    selftest: bool,
    /// The input service, through the capability that can set the keyboard layout and nothing
    /// else (note 0021, section 5), while it is known and answering.
    input: Option<seat::Client>,
    /// Whether the input service has been told the layout of the user at the keyboard. It
    /// starts with `us` every time it starts, so this is false again whenever it has gone.
    layout_set: bool,
}

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("users: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let args: Vec<&str> = start.args().split_whitespace().collect();
    let selftest = args.contains(&"selftest");
    // (Self-test only: behave as if there were no system volume.)
    let ignore_system = selftest && args.contains(&"nosystem");
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let mut parent = supervisor::Client::new(Endpoint(
        start.take(role::PARENT).ok_or("started without a supervisor")?,
    ));
    parent.set_timeout_ms(10_000);
    let storage = find_storage(&parent)?;
    let archive = start.take(role::ARCHIVE).ok_or("started without the start-up archive")?;
    let sandbox = program(&archive, "sandbox").ok_or("no sandbox manager in the archive")?;
    let ended = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&ended).map_err(|e| format!("binding a notification: {e}"))?;

    granite::println!("users: looking for the system volume.");
    let (system, users) = find_system(&storage, ignore_system)?;
    let console = find_console(&parent);
    let console_user = users.iter().find(|u| u.kind == Kind::Admin).map(|u| u.name.clone());
    let mut s = Service {
        endpoint,
        parent,
        storage,
        ignore_system,
        pending: Vec::new(),
        archive,
        sandbox,
        system,
        users,
        sessions: Vec::new(),
        next_session: 1,
        console,
        console_user,
        ended,
        shut_down: false,
        selftest,
        input: None,
        layout_set: false,
    };
    // At start, a session for the first administrator (M5 has no login yet; note 0017,
    // section 3).
    match s.users.iter().find(|u| u.kind == Kind::Admin).cloned() {
        Some(admin) => {
            if let Err(e) = s.start_session(&admin, History::new()) {
                granite::println!("users: can't start {}'s session: {e}", admin.name);
            }
        }
        None => granite::println!("users: there is no administrator, so nobody is logged in."),
    }
    serve(&mut s);
    Ok(())
}

/// A copy of the program called `name` in the start-up archive.
fn program(archive: &Handle, name: &str) -> Option<Vec<u8>> {
    let mem = Memory(archive.duplicate(rights::READ | rights::MAP).ok()?);
    let mapping = mem.map(perm::READ).ok()?;
    // SAFETY: the mapping is readable for its whole length while `mapping` lives, which is
    // longer than `bytes` is used, and nothing writes to it.
    let bytes = unsafe { core::slice::from_raw_parts(mapping.as_ptr(), mapping.len() as usize) };
    Some(Archive::parse(bytes).ok()?.find(name)?.data.to_vec())
}

// ------------------------------------------------------------------------------------------
// Files on a volume, through `pbr.file`.

/// A client that won't wait for ever for a filesystem server.
fn client(ep: Endpoint) -> file::Client {
    let mut c = file::Client::new(ep);
    c.set_timeout_ms(CALL_MS);
    c
}

fn open(
    dir: &file::Client,
    path: &str,
    rights: FileRights,
) -> Result<file::Client, CallError<FileError>> {
    dir.open(path, rights).map(|r| {
        let mut c = r.node;
        c.set_timeout_ms(CALL_MS);
        c
    })
}

/// Makes the folder at `path` below `dir`, unless it is there already.
fn folder(dir: &file::Client, path: &str) -> Result<(), String> {
    match dir.create(path, NodeKind::Folder, READ) {
        Ok(_) | Err(CallError::Service(FileError::AlreadyExists)) => Ok(()),
        Err(e) => Err(format!("making {path}: {e}")),
    }
}

/// Reads the whole file at `path` below `dir`, if it is at most `max` bytes.
fn read_file(dir: &file::Client, path: &str, max: usize) -> Result<Vec<u8>, CallError<FileError>> {
    let f = open(dir, path, READ)?;
    let mut out = Vec::new();
    loop {
        let chunk = f.read(out.len() as u64, 65536)?;
        if chunk.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(&chunk);
        if out.len() > max {
            return Err(CallError::Service(FileError::TooBig));
        }
    }
}

/// Replaces (or makes) the file at `path` below `dir` with `data`, durably: on RockFS, an
/// atomic save (note 0014, section 6).
fn write_file(dir: &file::Client, path: &str, data: &[u8], replace: bool) -> Result<(), String> {
    let f = match dir.create(path, NodeKind::File, READ_WRITE) {
        Ok(r) => r.node,
        Err(CallError::Service(FileError::AlreadyExists)) if replace => {
            open(dir, path, READ_WRITE).map_err(|e| format!("opening {path}: {e}"))?
        }
        Err(CallError::Service(FileError::AlreadyExists)) => return Ok(()),
        Err(e) => return Err(format!("making {path}: {e}")),
    };
    let fail = |e: CallError<FileError>| format!("writing {path}: {e}");
    f.set_size(0).map_err(fail)?;
    for (i, chunk) in data.chunks(65536).enumerate() {
        f.write((i * 65536) as u64, chunk).map_err(fail)?;
    }
    f.sync().map_err(fail)
}

/// Makes a user's home and its places, and their rules file if they have none.
fn make_home(root: &file::Client, name: &str) -> Result<(), String> {
    folder(root, "Users")?;
    folder(root, &format!("Users/{name}"))?;
    for p in Place::ALL {
        folder(root, &format!("Users/{name}/{}", p.folder()))?;
    }
    folder(root, "System/rules")?;
    let rules = format!("# Rules for {name} (docs/design/0017, section 5)\n");
    write_file(root, &format!("System/rules/{name}.txt"), rules.as_bytes(), false)
}

/// Makes the volume at `root` a system volume with one administrator, `name`: adds `System/`
/// and `Users/NAME/`, and touches nothing else.
fn set_up(root: &file::Client, name: &str, display: &str) -> Result<Vec<User>, String> {
    folder(root, "System")?;
    make_home(root, name)?;
    let users = vec![User { name: name.into(), kind: Kind::Admin, display: display.into() }];
    let text = users_txt::format(&users).ok_or("a bad name")?;
    // The users go last: until they are there, this isn't a system volume.
    write_file(root, "System/users.txt", text.as_bytes(), true)?;
    Ok(users)
}

// ------------------------------------------------------------------------------------------
// Volumes.

/// The storage service, found through the supervisor (note 0018, section 2), waiting up to a
/// minute for it to be running.
fn find_storage(parent: &supervisor::Client) -> Result<storage::Client, String> {
    let end = time::after_ms(60_000);
    loop {
        match parent.lookup("storage") {
            Ok(h) => {
                let mut c = storage::Client::new(Endpoint(h));
                c.set_timeout_ms(CALL_MS);
                return Ok(c);
            }
            Err(CallError::Service(SupervisorError::NotReady)) if time::now() < end => {
                time::sleep_ms(50)
            }
            Err(e) => return Err(format!("finding the storage service: {e}")),
        }
    }
}

/// The console's terminal, if the console is running: asked once, without waiting, since
/// nothing here depends on it (`Service::check` asks again later).
fn find_console(parent: &supervisor::Client) -> Option<terminal::Client> {
    match parent.lookup("console") {
        Ok(h) => {
            let mut c = terminal::Client::new(Endpoint(h));
            c.set_timeout_ms(CHECK_TIMEOUT_MS);
            Some(c)
        }
        Err(CallError::Service(SupervisorError::NotReady)) => None,
        Err(e) => {
            granite::println!("users: finding the console: {e}");
            None
        }
    }
}

/// Whether a call failed because what it called has gone (it can be found again).
fn lost<E>(e: &CallError<E>) -> bool {
    matches!(
        e,
        CallError::Call(
            granite::Error::PeerGone | granite::Error::Revoked | granite::Error::BadHandle
        )
    )
}

/// The storage service's volumes, once none is still starting (at most a minute after start).
fn settled(storage: &storage::Client) -> Result<Vec<storage::VolumeInfo>, String> {
    let end = time::after_ms(60_000);
    loop {
        let list = storage.list().map_err(|e| format!("listing volumes: {e}"))?;
        if !list.iter().any(|v| v.state == VolumeState::Starting) || time::now() > end {
            return Ok(list);
        }
        time::sleep_ms(50);
    }
}

/// Waits until volume `id` is served, and opens its root folder.
fn open_ready(storage: &storage::Client, id: u32) -> Result<file::Client, String> {
    let end = time::after_ms(30_000);
    loop {
        match storage.open(id) {
            Ok(h) => return Ok(client(Endpoint(h))),
            Err(CallError::Service(StorageError::NotReady)) if time::now() < end => {
                time::sleep_ms(50)
            }
            Err(e) => return Err(format!("opening volume {id}: {e}")),
        }
    }
}

/// Finds the system volume and its users, or sets up a volume in memory.
fn find_system(storage: &storage::Client, ignore: bool) -> Result<(System, Vec<User>), String> {
    let list = settled(storage)?;
    let candidates = list.iter().filter(|v| {
        !ignore && v.filesystem == storage::Filesystem::RockFs && v.state == VolumeState::Ready
    });
    for v in candidates {
        let Ok(root) = open_ready(storage, v.id) else { continue };
        match read_file(&root, "System/users.txt", users_txt::MAX_SIZE) {
            Ok(text) => match users_txt::parse(&text) {
                Ok(users) => {
                    granite::println!(
                        "users: the system volume is \"{}\", with {} users.",
                        v.label,
                        users.len()
                    );
                    let system = System { volume: v.id, in_memory: false, root };
                    return Ok((system, users));
                }
                Err((e, line)) => {
                    granite::println!(
                        "users: System/users.txt on \"{}\" is damaged (line {line}: {e:?}); \
                         log in as owner and repair it under /Volumes.",
                        v.label
                    );
                    break;
                }
            },
            Err(CallError::Service(FileError::NotFound | FileError::NotAFolder)) => {}
            Err(e) => granite::println!("users: reading volume {}: {e}", v.id),
        }
    }
    let id = storage
        .create_ram_volume(RAM_VOLUME, "Memory")
        .map_err(|e| format!("making a volume in memory: {e}"))?;
    let root = open_ready(storage, id)?;
    let users = set_up(&root, "owner", "Owner")?;
    granite::println!(
        "There's no system disk, so your files are kept in memory and are gone after a restart."
    );
    Ok((System { volume: id, in_memory: true, root }, users))
}

// ------------------------------------------------------------------------------------------
// Sessions.

impl Service {
    fn session(&self, badge: u64) -> Option<usize> {
        let n = badge.checked_sub(SESSION_BASE)?;
        self.sessions.iter().position(|s| u64::from(s.number) == n)
    }

    /// Starts `user`'s session: makes their tree and starts their sandbox manager with it.
    /// `history` is its sandbox manager's crash history so far.
    fn start_session(&mut self, user: &User, mut history: History) -> Result<usize, String> {
        let root = &self.system.root;
        let home_path = format!("Users/{}", user.name);
        let home = match open(root, &home_path, READ_WRITE) {
            Ok(h) => h,
            Err(CallError::Service(FileError::NotFound)) => {
                make_home(root, &user.name)?;
                open(root, &home_path, READ_WRITE)
                    .map_err(|e| format!("opening {home_path}: {e}"))?
            }
            Err(e) => return Err(format!("opening {home_path}: {e}")),
        };
        let rules = match open(root, &format!("System/rules/{}.txt", user.name), READ) {
            Ok(r) => Some(r),
            Err(CallError::Service(FileError::NotFound)) => None,
            Err(e) => return Err(format!("opening {}'s rules: {e}", user.name)),
        };
        // The tree: (path, capability, folder).
        let mut tree: Vec<(String, Handle, bool)> =
            vec![("/Home".into(), home.into_endpoint().0, true)];
        if let Some(r) = rules {
            tree.push(("/System/rules.txt".into(), r.into_endpoint().0, false));
        }
        if user.kind == Kind::Admin {
            let list = self.storage.list().map_err(|e| format!("listing volumes: {e}"))?;
            let ready: Vec<&storage::VolumeInfo> =
                list.iter().filter(|v| v.state == VolumeState::Ready).collect();
            let labels: Vec<&str> = ready.iter().map(|v| v.label.as_str()).collect();
            for (v, name) in ready.iter().zip(volume_names(&labels)) {
                match self.storage.open(v.id) {
                    Ok(h) => tree.push((format!("/Volumes/{name}"), h, true)),
                    Err(e) => granite::println!("users: volume {} can't be opened: {e}", v.id),
                }
            }
        }

        // Its sandbox manager: its own endpoint, this session's `pbr.users`, the archive, the
        // kernel log (without the right to take over the screen) and the tree.
        let number = self.next_session;
        // (Bit 63 is `INPUT_GONE`.)
        let bit = (0..63)
            .find(|b| !self.sessions.iter().any(|s| s.bit == *b))
            .ok_or("too many sessions")?;
        let ep = Endpoint::create().map_err(|e| format!("an endpoint: {e}"))?;
        let test_cap = if self.selftest {
            Some(ep.mint(SANDBOX_SESSION, CLIENT).map_err(|e| format!("minting: {e}"))?)
        } else {
            None
        };
        let users_cap = self
            .endpoint
            .mint(SESSION_BASE + u64::from(number), CLIENT)
            .map_err(|e| format!("minting: {e}"))?;
        let mut caps = vec![
            (role::OWN, ep.0.raw(), rights::ALL, false),
            (SANDBOX_USERS, users_cap.0.raw(), rights::ALL, false),
            (role::ARCHIVE, self.archive.raw(), rights::READ | rights::MAP, false),
        ];
        if granite::log::handle() != 0 {
            caps.push((role::LOG, granite::log::handle(), rights::ALL & !rights::CONTROL, false));
        }
        // The terminal, for the session that owns it. Its sandbox manager starts the shell on
        // it, and gives it to nothing but apps granted `terminal` (note 0017, section 5).
        let terminal = match &self.console {
            Some(c) if self.console_user.as_deref() == Some(user.name.as_str()) => {
                caps.push((SANDBOX_TERMINAL, c.endpoint().0.raw(), CLIENT, false));
                true
            }
            _ => false,
        };
        let file_tree: Vec<(&str, u32, bool)> =
            tree.iter().map(|(p, h, folder)| (p.as_str(), h.raw(), *folder)).collect();
        let sandbox = process::spawn(&Spawn {
            name: "sandbox",
            program: &self.sandbox,
            budget: SESSION_BUDGET,
            priority: SESSION_PRIORITY,
            caps: &caps,
            log: false,
            args: &user.name,
            tied: true,
            file_tree: &file_tree,
        })
        .map_err(|e| format!("starting a sandbox manager: {e}"))?;
        if let Err(e) = sandbox.process.watch(&self.ended, 1 << bit) {
            let _ = sandbox.process.stop(0);
            return Err(format!("watching the sandbox manager: {e}"));
        }
        self.next_session += 1;
        granite::println!(
            "users: {} ({}) is logged in, with {} entries{}.",
            user.name,
            if user.kind == Kind::Admin { "administrator" } else { "standard" },
            tree.len(),
            if terminal { " and the terminal" } else { "" }
        );
        history.started(now_ms());
        let held = tree
            .into_iter()
            .map(|(_, h, _)| {
                let mut c = file::Client::new(Endpoint(h));
                c.set_timeout_ms(CHECK_TIMEOUT_MS);
                c
            })
            .collect();
        self.sessions.push(Session {
            number,
            user: user.clone(),
            bit,
            sandbox,
            test_cap,
            held,
            history,
        });
        Ok(self.sessions.len() - 1)
    }

    /// Some sandbox managers have ended: their sessions are started again, with the restart
    /// policy every supervisor uses (note 0018, section 3). (A sandbox manager stopped here, to
    /// start its session afresh, is no longer in `sessions` when its end is noticed.)
    fn ended(&mut self, bits: u64) {
        let mut i = 0;
        while i < self.sessions.len() {
            let s = &self.sessions[i];
            let state = s.sandbox.process.info().map(|i| i.state);
            if bits & (1 << s.bit) == 0 || matches!(state, Ok(State::Running | State::Ending)) {
                i += 1;
                continue;
            }
            let mut s = self.sessions.remove(i);
            match s.history.crashed(now_ms()) {
                Decision::Restart { delay_ms } => {
                    granite::println!(
                        "users: {}'s session has ended ({state:?}); starting it again.",
                        s.user.name
                    );
                    self.pending.push(Pending {
                        user: s.user,
                        history: s.history,
                        due: now_ms() + delay_ms,
                    });
                }
                Decision::GiveUp => granite::println!(
                    "users: {}'s session has ended ({state:?}) too often; it is left logged out.",
                    s.user.name
                ),
            }
        }
    }

    /// Starts the sessions whose restart is due.
    fn restart_due(&mut self) {
        let now = now_ms();
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].due > now {
                i += 1;
                continue;
            }
            let p = self.pending.remove(i);
            if let Err(e) = self.start_session(&p.user, p.history) {
                granite::println!("users: can't start {}'s session again: {e}", p.user.name);
            }
        }
    }

    /// Checks that the storage service, the system volume and every volume a session holds are
    /// still there. A service or volume that went (its server was restarted) took everything
    /// opened through it, so the sessions that depended on it start afresh with new
    /// capabilities (note 0018, section 3): all of them if it was the storage service or the
    /// system volume, otherwise just the ones that held the volume.
    fn check(&mut self) {
        // After `shutdown` the volumes are closed on purpose: nothing is found again or
        // started again, so that nothing writes to them before the computer is turned off.
        if self.shut_down {
            return;
        }
        let storage_gone = self.storage.list().is_err_and(|e| lost(&e));
        let system_gone = self.system.root.info().is_err_and(|e| lost(&e));
        if storage_gone || system_gone {
            self.recover(storage_gone);
            return;
        }
        let mut i = 0;
        while i < self.sessions.len() {
            if !self.sessions[i].held.iter().any(|c| c.info().is_err_and(|e| lost(&e))) {
                i += 1;
                continue;
            }
            let s = self.sessions.remove(i);
            granite::println!(
                "users: a volume {}'s session held has gone; starting it again.",
                s.user.name
            );
            self.restart_session(s);
        }
        self.check_console();
        self.keep_layout();
    }

    // --------------------------------------------------------------------------------------
    // Settings and the keyboard layout (note 0024, section 5; note 0021, section 5).

    /// `name`'s settings, from `System/settings/<name>.txt`. A user who has never set
    /// anything has no file: every default. A file that can't be read is treated as empty,
    /// and the log says which line was wrong.
    fn read_settings(&self, name: &str) -> Settings {
        let path = format!("System/settings/{name}.txt");
        match read_file(&self.system.root, &path, settings::MAX_SIZE) {
            Ok(text) => Settings::parse(&text).unwrap_or_else(|e| {
                granite::println!("users: {name}'s settings are ignored: {e}.");
                Settings::new()
            }),
            Err(CallError::Service(FileError::NotFound)) => Settings::new(),
            Err(e) => {
                granite::println!("users: reading {name}'s settings: {e}");
                Settings::new()
            }
        }
    }

    /// The input service, looked up through the supervisor if it isn't known. `None` while it
    /// is starting or being started again. The capability this gets can set the layout and
    /// ask which there are; it can't take the seat (the root program's lookup table and the
    /// input service's check of the badge see to that).
    fn input(&mut self) -> Option<&seat::Client> {
        if self.input.is_none() {
            let endpoint = match self.parent.lookup("input") {
                Ok(h) => Endpoint(h),
                Err(CallError::Service(SupervisorError::NotReady)) => return None,
                Err(e) => {
                    granite::println!("users: finding the input service: {e}");
                    return None;
                }
            };
            // Ask to be told when it goes, so that its replacement is told the layout
            // again. (The watch belongs to this capability, which the client keeps.)
            if let Err(e) = endpoint.watch_receiver(&self.ended, INPUT_GONE) {
                granite::println!("users: can't watch the input service: {e}");
            }
            let mut client = seat::Client::new(endpoint);
            client.set_timeout_ms(CHECK_TIMEOUT_MS);
            self.input = Some(client);
        }
        self.input.as_ref()
    }

    /// The input service has gone (the kernel says so). The one that replaces it starts with
    /// the default layout, so it will need telling.
    fn input_gone(&mut self) {
        self.input = None;
        self.layout_set = false;
    }

    /// Tells the input service the layout of the user at the keyboard, if it hasn't been
    /// told. Called at every check, so it happens as soon as there is an input service to
    /// tell and a user to tell it about.
    ///
    /// There is one seat, so one user's layout applies: the user who has the terminal. (When
    /// the desktop has a login screen, in M6's stage 6, it will be whoever logged in there.)
    fn keep_layout(&mut self) {
        if self.layout_set {
            return;
        }
        let Some(owner) = self.console_user.clone() else { return };
        let wanted = String::from(self.read_settings(&owner).layout());
        let Some(input) = self.input() else { return };
        match input.set_layout(&wanted) {
            Ok(()) => self.layout_set = true,
            Err(CallError::Service(SeatError::NoSuchLayout)) => {
                // Note 0021, section 5: the default, and a line in the log.
                granite::println!(
                    "users: {owner}'s keyboard layout, {wanted}, isn't one there is; using {}.",
                    settings::DEFAULT_LAYOUT
                );
                let _ = input.set_layout(settings::DEFAULT_LAYOUT);
                self.layout_set = true;
            }
            Err(e) if lost(&e) => self.input_gone(),
            Err(e) => {
                // Not something trying again would mend (the root program gave this service
                // a capability that may not set layouts, say).
                granite::println!("users: can't set the keyboard layout: {e}");
                self.layout_set = true;
            }
        }
    }

    /// Checks the console's terminal: if the console has gone, or has come (again), the session
    /// that owns the terminal is started again with what there is now.
    fn check_console(&mut self) {
        let changed = match &self.console {
            Some(c) if c.size().is_err_and(|e| lost(&e)) => {
                granite::println!("users: the console has gone.");
                self.console = find_console(&self.parent);
                true
            }
            Some(_) => false,
            None => {
                self.console = find_console(&self.parent);
                self.console.is_some()
            }
        };
        let Some(owner) = self.console_user.clone() else { return };
        if !changed {
            return;
        }
        // (If the owner isn't logged in now, they get the terminal when they next are.)
        if let Some(i) = self.sessions.iter().position(|s| s.user.name == owner) {
            let s = self.sessions.remove(i);
            granite::println!("users: starting {owner}'s session again, for the terminal.");
            self.restart_session(s);
        }
    }

    /// Stops a session's sandbox manager and starts the session afresh, keeping its history.
    fn restart_session(&mut self, s: Session) {
        let _ = s.sandbox.process.stop(0);
        if let Err(e) = self.start_session(&s.user, s.history) {
            granite::println!("users: can't start {}'s session again: {e}", s.user.name);
        }
    }

    /// The storage service (if `storage_gone`) or the system volume's server has gone: finds
    /// them again, reads the users again, and starts every session that was running again.
    fn recover(&mut self, storage_gone: bool) {
        granite::println!(
            "users: the {} has gone; starting the sessions again.",
            if storage_gone { "storage service" } else { "system volume" }
        );
        let mut logged: Vec<(User, History)> = Vec::new();
        for s in self.sessions.drain(..) {
            let _ = s.sandbox.process.stop(0);
            logged.push((s.user, s.history));
        }
        logged.extend(self.pending.drain(..).map(|p| (p.user, p.history)));
        let end = time::after_ms(60_000);
        let mut find_storage_again = storage_gone;
        loop {
            let found = (|| {
                if find_storage_again {
                    self.storage = find_storage(&self.parent)?;
                }
                find_system(&self.storage, self.ignore_system)
            })();
            match found {
                Ok((system, users)) => {
                    self.system = system;
                    // The terminal belongs to the first administrator in the users read now.
                    self.console_user =
                        users.iter().find(|u| u.kind == Kind::Admin).map(|u| u.name.clone());
                    self.users = users;
                    break;
                }
                Err(e) if time::now() < end => {
                    granite::println!("users: {e}; trying again.");
                    find_storage_again = true;
                    time::sleep_ms(200);
                }
                Err(e) => {
                    granite::println!("users: {e}; nobody can log in.");
                    return;
                }
            }
        }
        for (user, history) in logged {
            // (Someone may have been removed from `users.txt` meanwhile.)
            if let Some(u) = self.users.iter().find(|u| u.name == user.name).cloned()
                && let Err(e) = self.start_session(&u, history)
            {
                granite::println!("users: can't start {}'s session again: {e}", u.name);
            }
        }
    }
}

fn now_ms() -> u64 {
    time::now() / 1_000_000
}

/// Answers requests for ever.
fn serve(s: &mut Service) {
    let ep = match s.endpoint.0.duplicate(rights::ALL) {
        Ok(h) => Endpoint(h),
        Err(e) => {
            granite::println!("users: can't copy its endpoint: {e}");
            return;
        }
    };
    let mut buffer = vec![0u8; 4096];
    let mut next_check = time::after_ms(CHECK_MS);
    loop {
        let mut wake = next_check;
        if let Some(due) = s.pending.iter().map(|p| p.due).min() {
            wake = wake.min(due * 1_000_000);
        }
        match ep.receive(&mut buffer, wake) {
            Ok(Event::Call(msg, reply)) => {
                let badge = msg.badge;
                let unknown = if badge == DEBUG && s.selftest {
                    debug_if::dispatch(s, msg, &buffer, reply)
                } else if let Some(i) = s.session(badge) {
                    users_if::dispatch(&mut Calls { s, session: i }, msg, &buffer, reply)
                } else {
                    Some((msg, reply))
                };
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(bits)) => {
                if bits & INPUT_GONE != 0 {
                    s.input_gone();
                }
                s.ended(bits & !INPUT_GONE)
            }
            Ok(Event::Gone(_)) | Err(granite::Error::TimedOut) => {}
            Err(e) => {
                granite::println!("users: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
        if time::now() >= next_check {
            s.check();
            next_check = time::after_ms(CHECK_MS);
        }
        s.restart_due();
    }
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("users: answer failed: {e}");
    }
}

// ------------------------------------------------------------------------------------------
// `pbr.users`, for one session.

struct Calls<'a> {
    s: &'a mut Service,
    session: usize,
}

fn to_interface(u: &User) -> users_if::User {
    users_if::User {
        name: u.name.clone(),
        kind: match u.kind {
            Kind::Admin => UserKind::Admin,
            Kind::Standard => UserKind::Standard,
        },
        display: u.display.clone(),
    }
}

fn storage_error(e: CallError<StorageError>) -> UsersError {
    match e {
        CallError::Service(StorageError::NoSuchVolume) => UsersError::NoSuchVolume,
        CallError::Service(StorageError::NotReady) => UsersError::NotReady,
        CallError::Service(StorageError::NotEmpty | StorageError::Unsupported) => {
            UsersError::Unsuitable
        }
        CallError::Service(StorageError::BadArgument) => UsersError::BadName,
        _ => UsersError::Failed,
    }
}

impl Calls<'_> {
    fn admin(&self) -> Result<(), UsersError> {
        match self.s.sessions[self.session].user.kind {
            Kind::Admin => Ok(()),
            Kind::Standard => Err(UsersError::NotAllowed),
        }
    }

    fn add(&mut self, user: users_if::User) -> Result<(), UsersError> {
        self.admin()?;
        let kind = match user.kind {
            UserKind::Admin => Kind::Admin,
            UserKind::Standard => Kind::Standard,
        };
        let user = User { name: user.name, kind, display: user.display };
        if !user.is_valid() {
            return Err(UsersError::BadName);
        }
        let s = &mut *self.s;
        if s.users.iter().any(|u| u.name == user.name) {
            return Err(UsersError::AlreadyExists);
        }
        if s.users.len() >= users_txt::MAX_USERS {
            return Err(UsersError::TooMany);
        }
        let fail = |e: String| {
            granite::println!("users: adding {}: {e}", user.name);
            UsersError::Failed
        };
        make_home(&s.system.root, &user.name).map_err(fail)?;
        let mut all = s.users.clone();
        all.push(user.clone());
        let text = users_txt::format(&all).ok_or(UsersError::BadName)?;
        // The users file is written last, so a failure part-way leaves no half-made user.
        write_file(&s.system.root, "System/users.txt", text.as_bytes(), true).map_err(fail)?;
        granite::println!("users: added {}.", user.name);
        s.users = all;
        Ok(())
    }

    fn setup(&mut self, volume: u32, name: &str) -> Result<(), UsersError> {
        self.admin()?;
        if !is_name(name) {
            return Err(UsersError::BadName);
        }
        let s = &mut *self.s;
        if volume == s.system.volume {
            return Err(UsersError::AlreadyExists);
        }
        let list = s.storage.list().map_err(storage_error)?;
        let v = list.iter().find(|v| v.id == volume).ok_or(UsersError::NoSuchVolume)?;
        let id = match v.filesystem {
            storage::Filesystem::RockFs => volume,
            storage::Filesystem::Empty => {
                s.storage.format(volume, NEW_LABEL).map_err(storage_error)?;
                // Formatting a whole disk makes a partition: a new volume on the same disk.
                let (disk, whole) = (v.disk, v.partition == 0);
                let end = time::after_ms(30_000);
                loop {
                    let list = s.storage.list().map_err(storage_error)?;
                    let found = list.iter().find(|n| {
                        n.disk == disk && n.label == NEW_LABEL && (!whole || n.id != volume)
                    });
                    if let Some(n) = found {
                        break n.id;
                    }
                    if time::now() > end {
                        return Err(UsersError::NotReady);
                    }
                    time::sleep_ms(50);
                }
            }
            _ => return Err(UsersError::Unsuitable),
        };
        let root = open_ready(&s.storage, id).map_err(|e| {
            granite::println!("users: setting up volume {id}: {e}");
            UsersError::NotReady
        })?;
        match read_file(&root, "System/users.txt", users_txt::MAX_SIZE) {
            Err(CallError::Service(FileError::NotFound)) => {}
            _ => return Err(UsersError::AlreadyExists),
        }
        set_up(&root, name, name).map_err(|e| {
            granite::println!("users: setting up volume {id}: {e}");
            UsersError::Failed
        })?;
        granite::println!(
            "users: volume {id} is set up; it is the system volume from the next start."
        );
        Ok(())
    }
}

impl Calls<'_> {
    /// `pbr.users.settings`: the caller's settings. The ones this version knows come first,
    /// with their defaults if never set, then whatever else the file holds.
    fn own_settings(&self) -> Vec<users_if::Setting> {
        let name = &self.s.sessions[self.session].user.name;
        let set = self.s.read_settings(name);
        let offset = set.timezone();
        let (sign, minutes) = if offset < 0 { ('-', -offset) } else { ('+', offset) };
        let mut list = vec![
            users_if::Setting { key: settings::LAYOUT.into(), value: set.layout().into() },
            users_if::Setting {
                key: settings::TIMEZONE.into(),
                value: format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60),
            },
        ];
        for (key, value) in set.pairs() {
            if key != settings::LAYOUT && key != settings::TIMEZONE && list.len() < 32 {
                list.push(users_if::Setting { key: key.into(), value: value.into() });
            }
        }
        list
    }

    /// `pbr.users.set`: checks, saves and applies one of the caller's settings.
    fn change_setting(&mut self, key: &str, value: &str) -> Result<(), UsersError> {
        if self.s.shut_down {
            return Err(UsersError::Failed);
        }
        // Only the settings this version knows can be set through here. (Others in the file
        // are kept: a later version may have put them there.)
        if key != settings::LAYOUT && key != settings::TIMEZONE {
            return Err(UsersError::BadSetting);
        }
        let name = self.s.sessions[self.session].user.name.clone();
        let mut set = self.s.read_settings(&name);
        set.set(key, value).map_err(|_| UsersError::BadSetting)?;
        if key == settings::LAYOUT {
            // It must be a layout the input service has, which only it knows.
            let input = self.s.input().ok_or(UsersError::NotReady)?;
            let known = match input.layouts() {
                Ok(l) => l.available.iter().any(|l| l.name == value),
                Err(e) if lost(&e) => {
                    self.s.input_gone();
                    return Err(UsersError::NotReady);
                }
                Err(e) => {
                    granite::println!("users: asking for the keyboard layouts: {e}");
                    return Err(UsersError::Failed);
                }
            };
            if !known {
                return Err(UsersError::BadSetting);
            }
        }
        // Saved first, applied second: if saving fails nothing has changed.
        let saved = folder(&self.s.system.root, "System/settings").and_then(|()| {
            let path = format!("System/settings/{name}.txt");
            write_file(&self.s.system.root, &path, set.to_text().as_bytes(), true)
        });
        if let Err(e) = saved {
            granite::println!("users: saving {name}'s settings: {e}");
            return Err(UsersError::Failed);
        }
        // A layout applies at once if this is the user at the keyboard; otherwise it waits
        // in the file until they are.
        if key == settings::LAYOUT && self.s.console_user.as_deref() == Some(name.as_str()) {
            self.s.layout_set = false;
            self.s.keep_layout();
        }
        Ok(())
    }
}

impl users_if::Server for Calls<'_> {
    fn settings(&mut self, reply: users_if::settings::Reply) {
        answered(reply.ok(&self.own_settings()));
    }

    fn set(&mut self, key: String, value: String, reply: users_if::set::Reply) {
        answered(match self.change_setting(&key, &value) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn whoami(&mut self, reply: users_if::whoami::Reply) {
        answered(reply.ok(&to_interface(&self.s.sessions[self.session].user)));
    }

    fn list(&mut self, reply: users_if::list::Reply) {
        answered(match self.admin() {
            Ok(()) => {
                let all: Vec<users_if::User> = self.s.users.iter().map(to_interface).collect();
                reply.ok(&all)
            }
            Err(e) => reply.err(e),
        });
    }

    fn setup(&mut self, volume: u32, name: String, reply: users_if::setup::Reply) {
        answered(match Calls::setup(self, volume, &name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn add(&mut self, user: users_if::User, reply: users_if::add::Reply) {
        answered(match Calls::add(self, user) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn volumes(&mut self, reply: users_if::volumes::Reply) {
        let r = self.admin().and_then(|()| self.s.storage.list().map_err(storage_error));
        answered(match r {
            Ok(list) => {
                let out: Vec<users_if::VolumeInfo> = list.iter().map(volume_info).collect();
                reply.ok(&out)
            }
            Err(e) => reply.err(e),
        });
    }

    fn format(&mut self, volume: u32, label: String, reply: users_if::format::Reply) {
        let r = self
            .admin()
            .and_then(|()| self.s.storage.format(volume, &label).map_err(storage_error));
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn shutdown(&mut self, reply: users_if::shutdown::Reply) {
        // Saving every volume can take a while on a slow disk: the storage service answers
        // only once every filesystem server has finished.
        let r = self.admin().and_then(|()| {
            granite::println!(
                "users: {} asked to shut down; saving every volume.",
                self.s.sessions[self.session].user.name
            );
            self.s.shut_down = true;
            self.s.storage.shutdown().map_err(storage_error)
        });
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }
}

/// A volume, in `pbr.users`'s copy of `pbr.storage`'s types.
fn volume_info(v: &storage::VolumeInfo) -> users_if::VolumeInfo {
    use users_if::{Filesystem as F, VolumeState as S};
    users_if::VolumeInfo {
        id: v.id,
        disk: v.disk,
        partition: v.partition,
        filesystem: match v.filesystem {
            storage::Filesystem::RockFs => F::RockFs,
            storage::Filesystem::Fat32 => F::Fat32,
            storage::Filesystem::Empty => F::Empty,
            storage::Filesystem::Unknown => F::Unknown,
        },
        state: match v.state {
            VolumeState::Ready => S::Ready,
            VolumeState::Starting => S::Starting,
            VolumeState::Unsupported => S::Unsupported,
            VolumeState::Damaged => S::Damaged,
            VolumeState::Failed => S::Failed,
        },
        label: v.label.clone(),
        size: v.size,
        read_only: v.read_only,
        in_memory: v.in_memory,
    }
}

// ------------------------------------------------------------------------------------------
// `pbr.users.debug`, for the self-test.

impl debug_if::Server for Service {
    fn session(&mut self, name: String, reply: debug_if::session::Reply) {
        let Some(user) = self.users.iter().find(|u| u.name == name).cloned() else {
            return answered(reply.err(DebugError::NoSuchUser));
        };
        let i = match self.sessions.iter().position(|s| s.user.name == name) {
            Some(i) => i,
            None => match self.start_session(&user, History::new()) {
                Ok(i) => i,
                Err(e) => {
                    granite::println!("users: can't start {name}'s session: {e}");
                    return answered(reply.err(DebugError::Failed));
                }
            },
        };
        let s = &self.sessions[i];
        let users = self.endpoint.mint(SESSION_BASE + u64::from(s.number), CLIENT);
        let sandbox = s.test_cap.as_ref().map(|c| c.0.duplicate(CLIENT));
        answered(match (users, sandbox) {
            (Ok(u), Some(Ok(b))) => reply.ok(OutCap::Cap(u.0), OutCap::Cap(b)),
            _ => reply.err(DebugError::Failed),
        });
    }

    fn system(&mut self, reply: debug_if::system::Reply) {
        answered(reply.ok(self.system.volume, self.system.in_memory));
    }
}
