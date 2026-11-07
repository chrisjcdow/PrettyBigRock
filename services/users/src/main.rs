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
//! Sessions are tied to this service (note 0013): if it ends, they end. Starting a session
//! again when its sandbox manager ends is supervision, which comes in stage 7.

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
use pbr_interfaces::pbr_storage::{self as storage, StorageError, VolumeState};
use pbr_interfaces::pbr_users::{self as users_if, UserKind, UsersError};
use pbr_interfaces::pbr_users_debug::{self as debug_if, DebugError};
use pbr_manifest::users::{self as users_txt, Kind, User};
use pbr_manifest::{Place, is_name, volume_names};

granite::main!(main);

/// The start-up role of the storage service's `pbr.storage` (this service's own endpoint is
/// `role::OWN`). The root program uses the same number.
const STORAGE: u32 = 1001;
/// The badge of the root program's `pbr.users.debug` (self-test runs only).
const DEBUG: u64 = 1;
/// Sessions' badges: this plus the session's number.
const SESSION_BASE: u64 = 0x100;

/// What a sandbox manager is started with (see `services/sandbox`): its session's `pbr.users`
/// at this role, and the badge of the `pbr.sandbox` capability minted for the session.
const SANDBOX_USERS: u32 = 1001;
const SANDBOX_SESSION: u64 = 1;
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
    /// the parents of everything the session holds.
    _held: Vec<Handle>,
}

struct Service {
    endpoint: Endpoint,
    storage: storage::Client,
    /// The start-up archive, for sandbox managers (who read apps from it).
    archive: Handle,
    /// The sandbox manager's program.
    sandbox: Vec<u8>,
    system: System,
    users: Vec<User>,
    sessions: Vec<Session>,
    next_session: u32,
    /// Signalled with a session's bit when its sandbox manager ends; bound to the endpoint.
    ended: Notification,
    selftest: bool,
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
    let mut storage = storage::Client::new(Endpoint(
        start.take(STORAGE).ok_or("started without the storage service")?,
    ));
    storage.set_timeout_ms(CALL_MS);
    let archive = start.take(role::ARCHIVE).ok_or("started without the start-up archive")?;
    let sandbox = program(&archive, "sandbox").ok_or("no sandbox manager in the archive")?;
    let ended = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&ended).map_err(|e| format!("binding a notification: {e}"))?;

    granite::println!("users: looking for the system volume.");
    let (system, users) = find_system(&storage, ignore_system)?;
    let mut s = Service {
        endpoint,
        storage,
        archive,
        sandbox,
        system,
        users,
        sessions: Vec::new(),
        next_session: 1,
        ended,
        selftest,
    };
    // At start, a session for the first administrator (M5 has no login yet; note 0017,
    // section 3).
    match s.users.iter().find(|u| u.kind == Kind::Admin).cloned() {
        Some(admin) => {
            if let Err(e) = s.start_session(&admin) {
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
    fn start_session(&mut self, user: &User) -> Result<usize, String> {
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
        let bit = (0..64)
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
            "users: {} ({}) is logged in, with {} entries.",
            user.name,
            if user.kind == Kind::Admin { "administrator" } else { "standard" },
            tree.len()
        );
        self.sessions.push(Session {
            number,
            user: user.clone(),
            bit,
            sandbox,
            test_cap,
            _held: tree.into_iter().map(|(_, h, _)| h).collect(),
        });
        Ok(self.sessions.len() - 1)
    }

    /// Some sandbox managers have ended: their sessions go. (Starting them again is stage 7.)
    fn ended(&mut self, bits: u64) {
        self.sessions.retain(|s| {
            if bits & (1 << s.bit) == 0 {
                return true;
            }
            match s.sandbox.process.info().map(|i| i.state) {
                Ok(State::Running | State::Ending) => true,
                state => {
                    granite::println!("users: {}'s session has ended: {state:?}.", s.user.name);
                    false
                }
            }
        });
    }
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
    loop {
        match ep.receive(&mut buffer, time::FOREVER) {
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
            Ok(Event::Signal(bits)) => s.ended(bits),
            Ok(Event::Gone(_)) => {}
            Err(e) => {
                granite::println!("users: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
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

impl users_if::Server for Calls<'_> {
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
            None => match self.start_session(&user) {
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
