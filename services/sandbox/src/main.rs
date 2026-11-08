// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A session's sandbox manager (docs/design/0017-users-sandbox-manager-and-manifests.md,
//! sections 5 to 7): it launches the session's apps, each with only what it is granted, and
//! can take back what it gave.
//!
//! The users service starts one for each session, with the session's file tree (note 0014,
//! section 8): the user's home at `/Home`, their rules file at `/System/rules.txt` (read only),
//! and, for an administrator, every volume under `/Volumes`. That tree is everything this
//! program holds, so whatever goes wrong in it, it can't give an app anything of another
//! user's: it never had it.
//!
//! A capability is the right to use something, held by the kernel on a program's behalf
//! (note 0006). Launching an app, in the note's order:
//! 1. its manifest, `apps/NAME.app` in the start-up archive, says what it asks for;
//! 2. the user's rules, read afresh at every launch, cut that down (`pbr_manifest::decide`);
//!    a file or folder the user chose is added;
//! 3. for each grant, this program asks the filesystem for a new capability derived from the
//!    session's, with just the rights granted: one for this app alone, so revoking it touches
//!    no other app;
//! 4. those capabilities make up the app's file tree, at the session's paths;
//! 5. the app's process is created *tied* to this one (it ends when this does; note 0013) and
//!    started;
//! 6. this program keeps each grant's capability. The app was given a copy of it, and
//!    everything the app opens through that copy is derived from it in turn, so revoking the
//!    kept capability (note 0013) takes back the grant and everything opened through it.
//!
//! **The shell** (note 0018, section 6). The session that owns the console's terminal (the
//! first administrator's) gets it from the users service. Then this program starts the shell,
//! `apps/shell`, launched like any other app (its manifest asks for `trusted` and `terminal`),
//! and starts it again when it ends, with the restart policy every supervisor uses
//! (`lib/supervise`). If the terminal itself has gone (the console ended), it doesn't: the users
//! service will start this session again with the new console's terminal. The terminal is given
//! only to apps granted `terminal` (note 0017, section 5).
//!
//! It serves `pbr.sandbox` to the session's trusted programs (the shell), told apart by
//! badges: a badge is a number a service stamps on each capability it hands out
//! (note 0008), so it knows who is calling. [`SESSION`] is the one the users service mints for
//! the session itself; each trusted app gets [`APP_BASE`] + its number.

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
use granite::process::{self, Process, Spawn, SpawnError, State, role};
use granite::sys::{self, perm, rights};
use granite::thread::Thread;
use granite::time;
use pbr_archive::Archive;
use pbr_files::{Error as FilesError, FileRights as FsRights, Files, NodeKind, READ};
use pbr_interfaces::pbr_file as file;
use pbr_interfaces::pbr_sandbox::{
    self as sandbox, AppInfo, FileRights, Grant, GrantReason, SandboxError,
};
use pbr_interfaces::pbr_terminal as terminal;
use pbr_manifest::app::App;
use pbr_manifest::rules::Rules;
use pbr_manifest::{Reason, decide, is_name};
use pbr_supervise::{Decision, History};

granite::main!(main);

/// The start-up role of the session's `pbr.users` capability (its own endpoint is
/// `role::OWN`). `services/users` uses the same numbers.
const USERS: u32 = 1001;
/// The start-up role of the console's terminal, if this session owns it.
const TERMINAL: u32 = 1002;

/// The roles a trusted app finds its `pbr.sandbox` and `pbr.users` capabilities at, and an app
/// granted `terminal` its `pbr.terminal`.
const APP_SANDBOX: u32 = 1000;
const APP_USERS: u32 = 1001;
const APP_TERMINAL: u32 = 1002;

/// The app started on the terminal.
const SHELL: &str = "shell";
/// The "caller" of the launches this program makes itself (the shell's). Badge 0 is never on a
/// capability anyone is given, so no client can use these launches.
const ITSELF: u64 = 0;
/// How long to wait for the console when checking that the terminal still works.
const TERMINAL_CHECK_MS: u64 = 5_000;

/// The badge of the capability the users service mints for the session (`services/users`
/// uses the same number).
const SESSION: u64 = 1;
/// Trusted apps' badges: this plus the app's number.
const APP_BASE: u64 = 0x1_0000;

/// How many launches can be prepared at once, and how many apps can run.
const MAX_LAUNCHES: usize = 8;
const MAX_APPS: usize = 32;
/// The most a program can be given as arguments (note 0009).
const MAX_ARGS: usize = 3072;
/// Where the user's rules are in the session's tree.
const RULES: &str = "/System/rules.txt";
/// Apps run at the priority of ordinary programs.
const APP_PRIORITY: u8 = 1;
/// How long to wait for a chosen capability's server to answer.
const CHOSEN_TIMEOUT_MS: u64 = 10_000;
/// A capability that can be passed on: what clients are given.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;

/// One thing an app holds (or will): where it is in its tree, and the capability it comes from.
struct Held {
    /// Its path in the app's tree (and the session's).
    path: String,
    /// Folder or file.
    folder: bool,
    /// `pbr.file` rights.
    rights: u32,
    reason: GrantReason,
    /// The capability this program keeps. Before the launch starts, a grant from the session's
    /// tree has none yet (it is made in `start`, step 3); a chosen one already has its own.
    cap: Option<Handle>,
}

/// A launch being prepared.
struct Launch {
    id: u32,
    /// Who prepared it: only they can add to it and start it.
    caller: u64,
    name: String,
    budget: u64,
    args: String,
    trusted: bool,
    /// Whether it is granted the terminal.
    terminal: bool,
    /// Everything it will hold, in order.
    grants: Vec<Held>,
}

/// A running app.
struct Running {
    id: u32,
    name: String,
    trusted: bool,
    /// Its bit in [`Sandbox::ended`].
    bit: u32,
    process: Process,
    /// Its main thread: kept so the thread capability lives as long as the record.
    _thread: Thread,
    grants: Vec<Held>,
    /// Whether this is the shell this program keeps running on the terminal.
    shell: bool,
}

struct Sandbox {
    /// Whose session this is, for messages.
    user: String,
    /// The endpoint it serves, with every right (so it can mint trusted apps' badges).
    endpoint: Endpoint,
    /// The session's `pbr.users`, for trusted apps.
    users: Option<Endpoint>,
    /// The console's terminal, if this session owns it.
    terminal: Option<terminal::Client>,
    /// The shell's crash history, and when it is due to be started again (milliseconds).
    shell_history: History,
    shell_due: Option<u64>,
    /// The start-up archive, mapped for good.
    archive: Archive<'static>,
    /// The session's tree.
    files: Files,
    /// Signalled when an app ends, with the app's bit; bound to the endpoint, so a receive
    /// returns its bits.
    ended: Notification,
    launches: Vec<Launch>,
    apps: Vec<Running>,
    next_launch: u32,
    next_app: u32,
}

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("sandbox: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let user = String::from(start.args().split_whitespace().next().unwrap_or("?"));
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let users = start.take(USERS).map(Endpoint);
    let terminal = start.take(TERMINAL).map(|h| {
        let mut c = terminal::Client::new(Endpoint(h));
        c.set_timeout_ms(TERMINAL_CHECK_MS);
        c
    });
    let archive = start.take(role::ARCHIVE).ok_or("started without the start-up archive")?;
    let mapping =
        Memory(archive).map(perm::READ).map_err(|e| format!("can't map the archive: {e}"))?;
    let len = mapping.len() as usize;
    let at = mapping.leak();
    // SAFETY: the mapping is readable for `len` bytes, nothing writes to it, and `leak` keeps it
    // mapped for the rest of this process's life, so a `'static` slice of it stays valid.
    let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(at as *const u8, len) };
    let archive = Archive::parse(bytes).map_err(|e| format!("the archive is damaged: {e}"))?;
    let files = Files::from_start(start);
    let ended = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&ended).map_err(|e| format!("binding a notification: {e}"))?;
    let mut s = Sandbox {
        user,
        endpoint,
        users,
        terminal,
        shell_history: History::new(),
        shell_due: None,
        archive,
        files,
        ended,
        launches: Vec::new(),
        apps: Vec::new(),
        next_launch: 1,
        next_app: 1,
    };
    granite::println!(
        "sandbox ({}): the session holds {} entries.",
        s.user,
        s.files.entries().len()
    );
    if s.terminal.is_some() {
        s.shell_due = Some(now_ms());
    }
    serve(&mut s);
    Ok(())
}

/// Answers requests for ever.
fn serve(s: &mut Sandbox) {
    let ep = match s.endpoint.0.duplicate(rights::ALL) {
        Ok(h) => Endpoint(h),
        Err(e) => {
            granite::println!("sandbox ({}): can't copy its endpoint: {e}", s.user);
            return;
        }
    };
    let mut buffer = vec![0u8; 8192];
    loop {
        s.start_shell_if_due();
        let wake =
            s.shell_due.map_or(time::FOREVER, |t| time::after_ms(t.saturating_sub(now_ms())));
        match ep.receive(&mut buffer, wake) {
            Ok(Event::Call(msg, reply)) => {
                let unknown = if s.may_call(msg.badge) {
                    sandbox::dispatch(s, msg, &buffer, reply)
                } else {
                    Some((msg, reply))
                };
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(bits)) => s.ended(bits),
            Ok(Event::Gone(_)) | Err(granite::Error::TimedOut) => {}
            Err(e) => {
                granite::println!("sandbox ({}): receive failed: {e}", s.user);
                time::sleep_ms(10);
            }
        }
    }
}

fn now_ms() -> u64 {
    time::now() / 1_000_000
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("sandbox: answer failed: {e}");
    }
}

/// Whether `inner` is inside the folder at `outer` (both tidy absolute paths).
fn inside(outer: &str, inner: &str) -> bool {
    if outer == "/" {
        return inner != "/";
    }
    inner.len() > outer.len() && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'/'
}

fn reason(r: Reason) -> GrantReason {
    match r {
        Reason::Manifest => GrantReason::Manifest,
        Reason::Rule => GrantReason::Rule,
    }
}

impl Sandbox {
    /// Whether a request may come with `badge`: the session's, or a running trusted app's.
    fn may_call(&self, badge: u64) -> bool {
        badge == SESSION
            || badge
                .checked_sub(APP_BASE)
                .is_some_and(|id| self.apps.iter().any(|a| u64::from(a.id) == id && a.trusted))
    }

    /// The user's rules, read afresh. No rules file means no rules; one that can't be read or
    /// is malformed is an error (and the launch is refused: see `lib/manifest`'s rules).
    fn rules(&self) -> Result<Rules, String> {
        let f = match self.files.open(RULES, READ) {
            Ok(f) => f,
            Err(FilesError::NotFound | FilesError::File(file::FileError::NotFound)) => {
                return Ok(Rules::none());
            }
            Err(e) => return Err(format!("can't be opened: {e}")),
        };
        let size = f.info().map_err(|e| format!("can't be read: {e}"))?.size;
        if size > pbr_manifest::rules::MAX_SIZE as u64 {
            return Err(format!("is {size} bytes, more than a rules file may be"));
        }
        let text = f.read_all().map_err(|e| format!("can't be read: {e}"))?;
        Rules::parse_at(&text).map_err(|(e, line)| match line {
            0 => format!("is malformed: {e:?}"),
            n => format!("is malformed on line {n}: {e:?}"),
        })
    }

    fn prepare(&mut self, caller: u64, name: &str, args: &str) -> Result<u32, SandboxError> {
        if !is_name(name) {
            return Err(SandboxError::NoSuchApp);
        }
        if self.launches.len() >= MAX_LAUNCHES {
            // There is no way to cancel a launch, so one a caller gave up on (a `choose` that
            // failed, say) would otherwise wait for ever. When they're all taken, the caller's
            // own oldest goes; nobody else's.
            let oldest = self.launches.iter().position(|l| l.caller == caller);
            match oldest {
                Some(i) => drop(self.launches.remove(i)),
                None => return Err(SandboxError::TooMany),
            }
        }
        // 1. The manifest. (A manifest without its program can still be found to be broken.)
        let program = self.archive.find(&format!("apps/{name}"));
        let Some(text) = self.archive.find(&format!("apps/{name}.app")) else {
            if program.is_some() {
                granite::println!("sandbox ({}): {name} has no manifest.", self.user);
                return Err(SandboxError::BadManifest);
            }
            return Err(SandboxError::NoSuchApp);
        };
        let app = App::parse(text.data).map_err(|e| {
            granite::println!("sandbox ({}): {name}'s manifest is malformed: {e:?}", self.user);
            SandboxError::BadManifest
        })?;
        if program.is_none() {
            return Err(SandboxError::NoSuchApp);
        }
        // 2. What it gets. Every app in the start-up archive is a system app.
        let rules = self.rules().map_err(|e| {
            granite::println!(
                "sandbox ({}): the rules file {e}, so nothing is launched until it is fixed.",
                self.user
            );
            SandboxError::BadRules
        })?;
        let d = decide(name, &app, &rules, true);
        let mut grants = Vec::new();
        if d.trusted {
            // The session's whole tree, each entry with the rights the session has for it.
            for (path, folder) in self.files.entries() {
                let rights = self
                    .files
                    .info(&path)
                    .map_err(|e| {
                        granite::println!("sandbox ({}): {path}: {e}", self.user);
                        SandboxError::Failed
                    })?
                    .rights;
                let reason = GrantReason::Trusted;
                grants.push(Held { path, folder, rights: rights.0, reason, cap: None });
            }
        } else {
            for g in &d.places {
                let rights = match g.rights {
                    pbr_manifest::Rights::Read => FsRights::READ.0,
                    pbr_manifest::Rights::ReadWrite => FsRights::READ.0 | FsRights::WRITE.0,
                };
                grants.push(Held {
                    path: g.place.path(),
                    folder: true,
                    rights,
                    reason: reason(g.reason),
                    cap: None,
                });
            }
        }
        let args = match (app.args.is_empty(), args.is_empty()) {
            (_, true) => app.args.clone(),
            (true, false) => String::from(args),
            (false, false) => format!("{} {args}", app.args),
        };
        if args.len() > MAX_ARGS {
            return Err(SandboxError::TooMany);
        }
        let id = self.next_launch;
        self.next_launch += 1;
        self.launches.push(Launch {
            id,
            caller,
            name: String::from(name),
            budget: app.budget,
            args,
            trusted: d.trusted,
            terminal: d.terminal,
            grants,
        });
        Ok(id)
    }

    fn launch(&mut self, caller: u64, id: u32) -> Result<&mut Launch, SandboxError> {
        self.launches
            .iter_mut()
            .find(|l| l.id == id && l.caller == caller)
            .ok_or(SandboxError::NotFound)
    }

    /// Adds what the user chose (note 0017, section 6).
    fn choose(
        &mut self,
        caller: u64,
        id: u32,
        path: &str,
        node: Endpoint,
    ) -> Result<(), SandboxError> {
        let launch = self.launch(caller, id)?;
        // A tidy absolute path, as everywhere in a tree: what normalising leaves unchanged.
        if pbr_filetree::normalize("/", path).as_deref() != Ok(path) || path == "/" {
            return Err(SandboxError::BadPath);
        }
        // The capability came from the caller: ask its server what it is, and don't wait for
        // ever if it isn't really a filesystem server.
        let mut node = file::Client::new(node);
        node.set_timeout_ms(CHOSEN_TIMEOUT_MS);
        let info = node.info().map_err(|_| SandboxError::BadCapability)?;
        let folder = info.kind == NodeKind::Folder;
        let rights = info.rights.0 & (FsRights::READ.0 | FsRights::WRITE.0);
        for g in &launch.grants {
            if g.path == path || (g.folder && inside(&g.path, path)) {
                // Already reached through something it gets: fine if that gives these rights.
                return if g.rights & rights == rights {
                    Ok(())
                } else {
                    Err(SandboxError::Overlaps)
                };
            }
            if inside(path, &g.path) || (!g.folder && inside(&g.path, path)) {
                return Err(SandboxError::Overlaps);
            }
        }
        if launch.grants.len() >= pbr_filetree::MAX_ENTRIES {
            return Err(SandboxError::TooMany);
        }
        // Step 3 for a chosen file: a capability of its own, derived from the one the caller
        // passed, so revoking it later takes back only this app's.
        let own = node.reduce(file::FileRights(rights)).map_err(|_| SandboxError::BadCapability)?;
        launch.grants.push(Held {
            path: String::from(path),
            folder,
            rights,
            reason: GrantReason::Chosen,
            cap: Some(own.into_endpoint().0),
        });
        Ok(())
    }

    /// Starts a prepared launch: steps 3 to 6.
    fn start(&mut self, caller: u64, id: u32) -> Result<(u32, Process), SandboxError> {
        let at = self
            .launches
            .iter()
            .position(|l| l.id == id && l.caller == caller)
            .ok_or(SandboxError::NotFound)?;
        if self.apps.len() >= MAX_APPS {
            return Err(SandboxError::TooMany);
        }
        let mut launch = self.launches.remove(at);
        let program = self
            .archive
            .find(&format!("apps/{}", launch.name))
            .ok_or(SandboxError::NoSuchApp)?
            .data;

        // 3. A capability for each grant, derived from the session's with the rights granted.
        let mut grants = Vec::new();
        for mut g in launch.grants.drain(..) {
            if g.cap.is_none() {
                match self.files.open(&g.path, FsRights(g.rights)) {
                    Ok(f) => g.cap = Some(f.into_handle()),
                    Err(FilesError::File(file::FileError::NotFound)) => {
                        // A place this user's home doesn't have: nothing to give.
                        granite::println!(
                            "sandbox ({}): {} asked for {}, which isn't there.",
                            self.user,
                            launch.name,
                            g.path
                        );
                        continue;
                    }
                    Err(e) => {
                        granite::println!("sandbox ({}): {}: {e}", self.user, g.path);
                        return Err(SandboxError::Failed);
                    }
                }
            }
            grants.push(g);
        }

        // 4. Its file tree: each grant at its path.
        let tree: Vec<(&str, u32, bool)> = grants
            .iter()
            .filter_map(|g| g.cap.as_ref().map(|c| (g.path.as_str(), c.raw(), g.folder)))
            .collect();

        // 5. The process, tied to this one (`spawn` also starts it). Besides its tree it gets
        // the kernel log, without the right to take over the screen; a trusted app also gets
        // this session's `pbr.sandbox` (with its own badge) and `pbr.users`; an app granted the
        // terminal gets it.
        let app_id = self.next_app;
        let mut keep: Vec<Handle> = Vec::new();
        let mut caps: Vec<process::Grant> = Vec::new();
        if granite::log::handle() != 0 {
            caps.push((role::LOG, granite::log::handle(), rights::ALL & !rights::CONTROL, false));
        }
        if launch.trusted {
            let own = self
                .endpoint
                .mint(APP_BASE + u64::from(app_id), CLIENT)
                .map_err(|_| SandboxError::Failed)?;
            caps.push((APP_SANDBOX, own.0.raw(), rights::ALL, false));
            keep.push(own.0);
            if let Some(u) = &self.users {
                caps.push((APP_USERS, u.0.raw(), CLIENT, false));
            }
        }
        // The terminal, if it is granted and this session has it.
        if launch.terminal
            && let Some(t) = &self.terminal
        {
            caps.push((APP_TERMINAL, t.endpoint().0.raw(), CLIENT, false));
        }
        let spawned = process::spawn(&Spawn {
            name: &launch.name,
            program,
            budget: launch.budget,
            priority: APP_PRIORITY,
            caps: &caps,
            log: false,
            args: &launch.args,
            tied: true,
            file_tree: &tree,
        })
        .map_err(|e| {
            granite::println!("sandbox ({}): can't start {}: {e}", self.user, launch.name);
            match e {
                SpawnError::System(sys::Error::OutOfMemory) => SandboxError::NoMemory,
                _ => SandboxError::Failed,
            }
        })?;
        // (The copies given to it are its own now; ours of its badge can go.)
        drop(keep);

        // 6. The record, and a watch for when it ends.
        let bit = (0..64)
            .find(|b| !self.apps.iter().any(|a| a.bit == *b))
            .ok_or(SandboxError::TooMany)?;
        if let Err(e) = spawned.process.watch(&self.ended, 1 << bit) {
            granite::println!("sandbox ({}): can't watch {}: {e}", self.user, launch.name);
            let _ = spawned.process.stop(0);
            return Err(SandboxError::Failed);
        }
        // What the caller gets: the process, which it can watch and ask about, not stop.
        let theirs = spawned
            .process
            .0
            .duplicate(rights::INSPECT | rights::WATCH | rights::TRANSFER | rights::DUPLICATE)
            .map_err(|_| SandboxError::Failed)?;
        self.next_app += 1;
        granite::println!(
            "sandbox ({}): started {} as app {app_id}{}, holding {} entries.",
            self.user,
            launch.name,
            if launch.trusted { " (trusted)" } else { "" },
            grants.len()
        );
        self.apps.push(Running {
            id: app_id,
            name: launch.name,
            trusted: launch.trusted,
            bit,
            process: spawned.process,
            _thread: spawned.thread,
            grants,
            shell: caller == ITSELF,
        });
        Ok((app_id, Process(theirs)))
    }

    fn app(&mut self, id: u32) -> Result<&mut Running, SandboxError> {
        self.apps.iter_mut().find(|a| a.id == id).ok_or(SandboxError::NotFound)
    }

    /// Takes back one grant: revoking the kept capability takes the app's copy and everything
    /// it opened through it (note 0013); then the kept one is closed too.
    fn revoke(&mut self, id: u32, path: &str) -> Result<(), SandboxError> {
        let user = self.user.clone();
        let app = self.app(id)?;
        let at = app.grants.iter().position(|g| g.path == path).ok_or(SandboxError::NotFound)?;
        let g = app.grants.remove(at);
        if let Some(cap) = g.cap
            && let Err(e) = cap.revoke()
        {
            granite::println!("sandbox ({user}): revoking {path}: {e}");
            return Err(SandboxError::Failed);
        }
        granite::println!("sandbox ({user}): took {path} back from {}.", app.name);
        Ok(())
    }

    fn stop(&mut self, id: u32) -> Result<(), SandboxError> {
        let at = self.apps.iter().position(|a| a.id == id).ok_or(SandboxError::NotFound)?;
        let app = self.apps.remove(at);
        let _ = app.process.stop(0);
        granite::println!("sandbox ({}): stopped {}.", self.user, app.name);
        if app.shell {
            self.shell_ended();
        }
        Ok(())
    }

    /// The shell has ended: it is started again, with the restart policy, while the terminal
    /// still works.
    fn shell_ended(&mut self) {
        match self.shell_history.crashed(now_ms()) {
            Decision::Restart { delay_ms } => self.shell_due = Some(now_ms() + delay_ms),
            Decision::GiveUp => {
                self.shell_due = None;
                granite::println!(
                    "sandbox ({}): the shell ended too often; it is left stopped.",
                    self.user
                );
            }
        }
    }

    /// Starts the shell on the terminal, if that is due.
    fn start_shell_if_due(&mut self) {
        if self.shell_due.is_none_or(|t| t > now_ms()) {
            return;
        }
        self.shell_due = None;
        let Some(t) = &self.terminal else { return };
        // A terminal that has gone stays gone: the users service starts this session again
        // with the new console's.
        if let Err(e) = t.size()
            && matches!(
                e,
                CallError::Call(
                    granite::Error::PeerGone | granite::Error::Revoked | granite::Error::BadHandle
                )
            )
        {
            granite::println!("sandbox ({}): the terminal has gone.", self.user);
            self.terminal = None;
            return;
        }
        self.shell_history.started(now_ms());
        let started = self.prepare(ITSELF, SHELL, "").and_then(|id| self.start(ITSELF, id));
        if let Err(e) = started {
            granite::println!("sandbox ({}): can't start the shell: {e:?}", self.user);
            self.shell_ended();
        }
    }

    /// Some apps have ended: their records go (their capabilities went with their processes;
    /// the ones kept here are closed as the records are dropped).
    fn ended(&mut self, bits: u64) {
        let user = &self.user;
        let mut shell_ended = false;
        self.apps.retain(|a| {
            if bits & (1 << a.bit) == 0 {
                return true;
            }
            // A signal for a bit can arrive after its app was stopped and the bit reused, so
            // only an app that really has ended goes.
            match a.process.info().map(|i| i.state) {
                Ok(State::Running | State::Ending) => true,
                state => {
                    granite::println!("sandbox ({user}): {} ended: {state:?}.", a.name);
                    shell_ended |= a.shell;
                    false
                }
            }
        });
        // Launches prepared by trusted apps that have ended can never be started.
        let apps = &self.apps;
        self.launches.retain(|l| {
            l.caller == ITSELF
                || l.caller == SESSION
                || l.caller
                    .checked_sub(APP_BASE)
                    .is_some_and(|id| apps.iter().any(|a| u64::from(a.id) == id))
        });
        if shell_ended {
            self.shell_ended();
        }
    }
}

// The generated `Server` trait: one method per operation, each answering through its `reply`.
impl sandbox::Server for Sandbox {
    fn prepare(&mut self, app: String, args: String, reply: sandbox::prepare::Reply) {
        answered(match Sandbox::prepare(self, reply.badge(), &app, &args) {
            Ok(id) => reply.ok(id),
            Err(e) => reply.err(e),
        });
    }

    fn choose(&mut self, launch: u32, path: String, node: Endpoint, reply: sandbox::choose::Reply) {
        answered(match Sandbox::choose(self, reply.badge(), launch, &path, node) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn start(&mut self, launch: u32, reply: sandbox::start::Reply) {
        answered(match Sandbox::start(self, reply.badge(), launch) {
            Ok((id, p)) => reply.ok(id, OutCap::Cap(p)),
            Err(e) => reply.err(e),
        });
    }

    fn list(&mut self, reply: sandbox::list::Reply) {
        let apps: Vec<AppInfo> = self
            .apps
            .iter()
            .map(|a| AppInfo {
                id: a.id,
                app: a.name.clone(),
                trusted: a.trusted,
                running: matches!(a.process.info().map(|i| i.state), Ok(State::Running)),
            })
            .collect();
        answered(reply.ok(&apps));
    }

    fn grants(&mut self, app: u32, from: u32, reply: sandbox::grants::Reply) {
        const PAGE: usize = 32;
        let Some(a) = self.apps.iter().find(|a| a.id == app) else {
            return answered(reply.err(SandboxError::NotFound));
        };
        let from = from as usize;
        let page: Vec<Grant> = a
            .grants
            .iter()
            .skip(from)
            .take(PAGE)
            .map(|g| Grant { path: g.path.clone(), rights: FileRights(g.rights), reason: g.reason })
            .collect();
        let next = if from + PAGE < a.grants.len() { (from + PAGE) as u32 } else { 0 };
        answered(reply.ok(&page, next));
    }

    fn revoke(&mut self, app: u32, path: String, reply: sandbox::revoke::Reply) {
        answered(match Sandbox::revoke(self, app, &path) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn stop(&mut self, app: u32, reply: sandbox::stop::Reply) {
        answered(match Sandbox::stop(self, app) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }
}
