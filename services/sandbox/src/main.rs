// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A session's sandbox manager (docs/design/0017-users-sandbox-manager-and-manifests.md,
//! sections 5 to 7, and docs/design/0024-the-session-desktop.md, sections 3, 4, 6 and 9): it
//! launches the session's apps, each with only what it is granted, and can take back what it
//! gave.
//!
//! The users service starts one for each session, with the session's file tree (note 0014,
//! section 8): the user's home at `/Home`, their rules file at `/System/rules.txt` (read only),
//! and, for an administrator, every volume under `/Volumes`. That tree, the session's display
//! (or terminal) and its `pbr.users` are everything this program holds, so whatever goes wrong
//! in it, it can't give an app anything of another user's: it never had it.
//!
//! A capability is the right to use something, held by the kernel on a program's behalf
//! (note 0006). Launching an app, in note 0017's order:
//! 1. its manifest, `apps/NAME.app` in the start-up archive, says what it asks for;
//! 2. the user's rules, read afresh at every launch, cut that down (`pbr_manifest::decide`);
//!    a file or folder the user chose is added;
//! 3. for each grant, this program asks the filesystem for a new capability derived from the
//!    session's, with just the rights granted: one for this app alone, so revoking it touches
//!    no other app;
//! 4. those capabilities make up the app's file tree, at the session's paths;
//! 5. the app's process is created *tied* to this one (it ends when this does; note 0013) and
//!    started, with whatever else it is granted at its start-up roles (note 0024, section 10):
//!    a window capability, `pbr.sandbox`, `pbr.users`, the terminal;
//! 6. this program keeps each grant's capability. The app was given a copy of it, and
//!    everything the app opens through that copy is derived from it in turn, so revoking the
//!    kept capability (note 0013) takes back the grant and everything opened through it.
//!
//! **Windows** (note 0024, section 2). In a desktop session the users service gives this
//! program the session's display capability (`pbr.display.session`). For each app granted
//! `windows` it makes a copy of it and asks the display server, through the copy, for the app's
//! window capability: the display server derives that from the capability the request came
//! through, so revoking the copy takes exactly that app's windows. If the display server goes,
//! every app with windows is stopped (their windows went with it); the users service then
//! starts the session afresh with the new display server's session.
//!
//! **Its own folder** (note 0024, section 4). An app granted `data` gets
//! `/Home/Apps/APP`, made the first time.
//!
//! **The app kept running** (note 0018, section 6; note 0024, section 6). A desktop session
//! starts the desktop, `apps/desktop`; a session with the console's terminal starts the shell,
//! `apps/shell`. Either is launched like any other app, from its manifest, and started again
//! when it ends, with the restart policy every supervisor uses (`lib/supervise`), unless what
//! it needs (the display, the terminal) has gone.
//!
//! **Who may do what** (note 0024, section 9). It serves `pbr.sandbox` through capabilities
//! badged with what their holder may do: a badge is a number a service stamps on each
//! capability it hands out (note 0008), so it knows who is calling. The badge's top byte is the
//! kind of holder ([`badge`]), the rest the app's number. Each app's badge is *watched*: the
//! kernel says when every capability carrying it has gone, and the launches that app prepared
//! go with it.
//!
//! **The file picker** (note 0025, section 1). A desktop session also keeps `apps/picker`
//! running. This program makes the picker's endpoint and gives the picker its receiving end;
//! each app granted `picker` gets a capability to it whose badge is the app's number, minted
//! *watched*, so the picker knows which app is asking and hears when it has gone. The picker's
//! own `pbr.sandbox` capability may only `grant` (and ask `details`, for the app's title): when
//! the user chooses a file, the picker opens it and hands it here, and this program keeps the
//! capability, records the grant as `Chosen`, and gives the app one derived from it. So a
//! chosen file is in `grants` and `revoke` takes it back, like a grant at launch.
//!
//! **Records per grant** (note 0026, section 4). Each capability this program keeps for a grant
//! is made with `pbr.file`'s `limit`, not `reduce`: whatever the app opens through it can hold
//! at most 4096 of the filesystem server's records (16384 for each entry of a trusted app's
//! tree), so one app can't fill a volume's records and stop every other program opening files.
//!
//! **Never held up.** This program answers one request at a time, so it must never wait for
//! something it doesn't control. The only such waits are asking a chosen capability's server
//! about it (`choose`, `grant`): that happens on a helper thread, with a 2-second limit, and the
//! request is answered when it finishes (a reply capability can be kept and answered later,
//! note 0008), while other requests are answered meanwhile.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, Event};
use granite::handle::Handle;
use granite::interface::{CallError, OutCap};
use granite::lock::Lock;
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::{self, Process, Spawn, SpawnError, State, role};
use granite::sys::{self, perm, rights};
use granite::thread::{JoinHandle, Thread};
use granite::time;
use pbr_archive::Archive;
use pbr_files::{Error as FilesError, FileRights as FsRights, Files, NodeKind, READ};
use pbr_interfaces::pbr_display_session as display;
use pbr_interfaces::pbr_file as file;
use pbr_interfaces::pbr_sandbox::{
    self as sandbox, AppDetails, AppInfo, AppServices, Available, FileRights, Grant, GrantReason,
    SandboxError,
};
use pbr_interfaces::pbr_terminal as terminal;
use pbr_manifest::app::App;
use pbr_manifest::rules::Rules;
use pbr_manifest::{Reason, Refused, decide, is_name};
use pbr_supervise::{Decision, History};

granite::main!(main);

/// The start-up roles of what the users service gives this program besides its endpoint
/// (`role::OWN`): the session's `pbr.users`; the console's terminal, if this session owns it;
/// the session's display, in a desktop session; and the `pbr.users` capability for the
/// desktop, which can do less. `services/users` uses the same numbers.
const USERS: u32 = 1001;
const TERMINAL: u32 = 1002;
const DISPLAY: u32 = 1003;
const DESKTOP_USERS: u32 = 1006;
/// `pbr.sysinfo` (note 0026, section 3), for the system apps that ask for it.
const SYSINFO: u32 = 1007;

/// The roles an app finds what it is granted at (note 0024, section 10).
const APP_SANDBOX: u32 = 1000;
const APP_USERS: u32 = 1001;
const APP_TERMINAL: u32 = 1002;
const APP_WINDOW: u32 = 1003;
const APP_PICKER: u32 = 1004;
const APP_SYSINFO: u32 = 1005;

/// The badges on `pbr.sandbox` capabilities: the kind of holder in the top byte, the app's
/// number below. The users service mints some itself for the self-test: their number is 0, or
/// has the top bit set ([`badge::TEST`]), so each is a caller of its own.
mod badge {
    /// Set in the number of the self-test's capabilities: never an app's.
    pub const TEST: u32 = 1 << 31;
    /// Trusted apps: the shell, Terminal, Files. Everything but `grant`.
    pub const TRUSTED: u64 = 1;
    /// The desktop: launching, `list`, `details`, `stop`, `apps`.
    pub const DESKTOP: u64 = 2;
    /// The permissions viewer (`sandbox view`): `list`, `details`, `grants`, `revoke`, `stop`.
    pub const VIEWER: u64 = 3;
    /// The file picker: `grant`, and `details` for the title of the app asking (note 0025).
    pub const PICKER: u64 = 4;

    pub const fn make(kind: u64, app: u32) -> u64 {
        (kind << 56) | app as u64
    }
    pub const fn kind(b: u64) -> u64 {
        b >> 56
    }
    pub const fn app(b: u64) -> u32 {
        (b & 0xFFFF_FFFF) as u32
    }
}

/// `pbr.sandbox`'s operation numbers, for checking who may call which.
mod op {
    pub const PREPARE: u32 = 1;
    pub const CHOOSE: u32 = 2;
    pub const START: u32 = 3;
    pub const LIST: u32 = 4;
    pub const GRANTS: u32 = 5;
    pub const REVOKE: u32 = 6;
    pub const STOP: u32 = 7;
    pub const CANCEL: u32 = 8;
    pub const DETAILS: u32 = 9;
    pub const GRANT: u32 = 10;
    pub const APPS: u32 = 11;
}

/// Whether a holder of `kind` may call operation `op` (note 0024, section 9).
fn allowed(kind: u64, op: u32) -> bool {
    use op::*;
    match kind {
        badge::TRUSTED => op != GRANT,
        badge::DESKTOP => {
            matches!(op, PREPARE | CHOOSE | START | CANCEL | LIST | DETAILS | STOP | APPS)
        }
        badge::VIEWER => matches!(op, LIST | DETAILS | GRANTS | REVOKE | STOP),
        badge::PICKER => matches!(op, GRANT | DETAILS),
        _ => false,
    }
}

/// The apps kept running on the session's terminal or display.
const SHELL: &str = "shell";
const DESKTOP: &str = "desktop";
const PICKER: &str = "picker";
/// The permissions viewer (note 0025, section 4), which the desktop launches.
const VIEWER: &str = "permissions";
/// The "caller" of the launches this program makes itself (the shell's, the desktop's). Badge
/// 0 is never on a capability anyone is given, so no client can use these launches.
const ITSELF: u64 = 0;
/// How long to wait for the console when checking that the terminal still works, and for the
/// display server's answers.
const TERMINAL_CHECK_MS: u64 = 5_000;
const DISPLAY_MS: u64 = 10_000;

/// How many launches can be prepared at once by one caller and in all, how many chosen
/// capabilities can be being checked at once, and how many apps can run.
const MAX_LAUNCHES_EACH: usize = 4;
const MAX_LAUNCHES: usize = 16;
const MAX_CHECKING: usize = 16;
const MAX_APPS: usize = 32;
/// The most a program can be given as arguments (note 0009).
const MAX_ARGS: usize = 3072;
/// Where the user's rules are in the session's tree.
const RULES: &str = "/System/rules.txt";
/// Where apps' own folders are (note 0024, section 4).
const APPS_FOLDER: &str = "/Home/Apps";
/// Apps run at the priority of ordinary programs.
const APP_PRIORITY: u8 = 1;
/// How long a chosen capability's server has to say what it is (and make the app's copy).
const CHOSEN_TIMEOUT_MS: u64 = 2_000;
/// Each grant's share of its filesystem server's records (note 0026, section 4): an app's place
/// or chosen file, and each entry of a trusted app's tree.
const RECORDS: u32 = 4096;
const TRUSTED_RECORDS: u32 = 16384;
/// A capability that can be passed on: what clients are given.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;

/// The bits of [`Sandbox::signals`] besides the apps' (which use bits 0 to 31): a chosen
/// capability has been checked; the display server has gone.
const CHECKED: u64 = 1 << 62;
const DISPLAY_GONE: u64 = 1 << 61;

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

/// What a launch gives besides files, decided from the manifest.
#[derive(Clone, Copy)]
struct Extras {
    trusted: bool,
    terminal: bool,
    windows: bool,
    system: bool,
    desktop: bool,
    viewer: bool,
    picker: bool,
    sysinfo: bool,
    /// Its own folder: whether, and why.
    data: Option<GrantReason>,
}

/// A launch being prepared.
struct Launch {
    id: u32,
    /// Who prepared it (their badge): only they can add to it, start it or cancel it.
    caller: u64,
    name: String,
    title: String,
    budget: u64,
    args: String,
    extras: Extras,
    /// Everything it will hold, in order.
    grants: Vec<Held>,
}

/// A running app.
struct Running {
    id: u32,
    name: String,
    title: String,
    extras: Extras,
    services: AppServices,
    /// When it was started (monotonic nanoseconds).
    started: u64,
    /// Its bit in [`Sandbox::signals`].
    bit: u32,
    process: Process,
    /// Its main thread: kept so the thread capability lives as long as the record.
    _thread: Thread,
    grants: Vec<Held>,
    /// The copy of the session's display capability its window capability was asked for
    /// through: revoking it takes the app's windows.
    windows: Option<Handle>,
    /// Which of the apps this program keeps running it is (the shell, the desktop, the
    /// picker), if any.
    kept: Option<&'static str>,
}

/// A capability being checked on a helper thread: `choose`'s for a launch, or `grant`'s for a
/// running app.
struct Checking {
    job: u32,
    path: String,
    to: CheckFor,
    thread: Option<JoinHandle>,
}

/// Who a check is for, and how to answer.
enum CheckFor {
    Choose {
        caller: u64,
        launch: u32,
        reply: sandbox::choose::Reply,
    },
    /// `covered`: what the app already holds covers the path, so the answer is opened through
    /// that and nothing is recorded.
    Grant {
        app: u32,
        covered: bool,
        reply: sandbox::grant::Reply,
    },
}

/// What a helper thread found: whether it is a folder, its rights, and the capability made
/// from it (the one to keep, for a new grant; the app's own, when it was covered).
type Checked = (u32, Option<(bool, u32, Handle)>);

/// The app this program keeps running, and when it is due to be started again.
struct Kept {
    name: &'static str,
    history: History,
    due: Option<u64>,
}

struct Sandbox {
    /// Whose session this is, for messages.
    user: String,
    /// A test boot (`describe` among its arguments): the system apps the desktop boot's runner
    /// drives (the picker, the permissions viewer) are told to describe their windows.
    describe: bool,
    /// The endpoint it serves, with every right (so it can mint badges).
    endpoint: Endpoint,
    /// The session's `pbr.users`, for trusted apps, and the one for the desktop.
    users: Option<Endpoint>,
    desktop_users: Option<Endpoint>,
    /// `pbr.sysinfo`, for apps granted `sysinfo`.
    sysinfo: Option<Endpoint>,
    /// The console's terminal, if this session owns it.
    terminal: Option<terminal::Client>,
    /// The session's display, in a desktop session, while the display server is there.
    display: Option<Handle>,
    /// The file picker's endpoint, in a desktop session, with every right: the picker gets
    /// its receiving end, apps capabilities minted from it.
    picker: Option<Endpoint>,
    /// The apps it keeps running: the shell; or the desktop and the picker.
    kept: Vec<Kept>,
    /// The start-up archive, mapped for good.
    archive: Archive<'static>,
    /// The session's tree.
    files: Files,
    /// Signalled when an app ends (with the app's bit), when a chosen capability has been
    /// checked, and when the display server goes; bound to the endpoint, so a receive returns
    /// its bits.
    signals: Notification,
    launches: Vec<Launch>,
    apps: Vec<Running>,
    checking: Vec<Checking>,
    /// What the helper threads have found, waiting for the main thread.
    checked: Arc<Lock<Vec<Checked>>>,
    next_launch: u32,
    next_app: u32,
    next_job: u32,
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
    let describe = start.args().split_whitespace().skip(1).any(|a| a == "describe");
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let users = start.take(USERS).map(Endpoint);
    let desktop_users = start.take(DESKTOP_USERS).map(Endpoint);
    let sysinfo = start.take(SYSINFO).map(Endpoint);
    let terminal = start.take(TERMINAL).map(|h| {
        let mut c = terminal::Client::new(Endpoint(h));
        c.set_timeout_ms(TERMINAL_CHECK_MS);
        c
    });
    let display = start.take(DISPLAY);
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
    let signals = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&signals).map_err(|e| format!("binding a notification: {e}"))?;
    // Told when the display server goes (a receiver watch, note 0019): its windows have gone.
    if let Some(d) = &display {
        let ep = Endpoint(d.duplicate(rights::ALL).map_err(|e| format!("copying: {e}"))?);
        ep.watch_receiver(&signals, DISPLAY_GONE)
            .map_err(|e| format!("watching the display server: {e}"))?;
        // The watch belongs to this copy, which is kept for as long as the process runs.
        core::mem::forget(ep);
    }
    // In a desktop session the desktop and the picker are kept running; with the terminal, the
    // shell.
    let names: &[&'static str] = if display.is_some() {
        &[DESKTOP, PICKER]
    } else if terminal.is_some() {
        &[SHELL]
    } else {
        &[]
    };
    let kept = names
        .iter()
        .map(|&name| Kept { name, history: History::new(), due: Some(now_ms()) })
        .collect();
    let picker = match display {
        Some(_) => Some(Endpoint::create().map_err(|e| format!("the picker's endpoint: {e}"))?),
        None => None,
    };
    let mut s = Sandbox {
        user,
        describe,
        endpoint,
        users,
        desktop_users,
        sysinfo,
        terminal,
        display,
        picker,
        kept,
        archive,
        files,
        signals,
        launches: Vec::new(),
        apps: Vec::new(),
        checking: Vec::new(),
        checked: Arc::new(Lock::new(Vec::new())),
        next_launch: 1,
        next_app: 1,
        next_job: 1,
    };
    granite::println!(
        "sandbox ({}): the session holds {} entries{}.",
        s.user,
        s.files.entries().len(),
        if s.display.is_some() { " and the display" } else { "" }
    );
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
        s.start_kept_if_due();
        let wake = s
            .kept
            .iter()
            .filter_map(|k| k.due)
            .min()
            .map_or(time::FOREVER, |t| time::after_ms(t.saturating_sub(now_ms())));
        match ep.receive(&mut buffer, wake) {
            Ok(Event::Call(msg, reply)) => {
                // Who is calling, and may they call this operation? A badge of a kind this
                // program doesn't hand out, or of an app that has ended, isn't served at all.
                let b = msg.badge;
                let app = badge::app(b);
                let known =
                    app == 0 || app & badge::TEST != 0 || s.apps.iter().any(|a| a.id == app);
                let unknown = if known && badge::kind(b) != 0 {
                    sandbox::dispatch(s, msg, &buffer, reply)
                } else {
                    Some((msg, reply))
                };
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(bits)) => {
                if bits & CHECKED != 0 {
                    s.finish_checks();
                }
                if bits & DISPLAY_GONE != 0 {
                    s.display_gone();
                }
                s.ended(bits & !(CHECKED | DISPLAY_GONE));
            }
            // Every capability carrying a badge this program watches has gone: that app (or
            // test) can't start what it prepared.
            Ok(Event::Gone(b)) => s.launches.retain(|l| l.caller != b),
            Err(granite::Error::TimedOut) => {}
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

/// `pbr.file` rights for what a manifest or rule says.
fn fs_rights(r: pbr_manifest::Rights) -> u32 {
    match r {
        pbr_manifest::Rights::Read => FsRights::READ.0,
        pbr_manifest::Rights::ReadWrite => FsRights::READ.0 | FsRights::WRITE.0,
    }
}

/// Runs on a helper thread: asks a chosen capability's server what it is and for a capability
/// of this program's own, with `want` rights (or all it has, for `None`) and its share of
/// records, both within [`CHOSEN_TIMEOUT_MS`]. A server that doesn't answer as a filesystem
/// does, or a capability without the rights wanted, gives `None`.
fn check_chosen(node: Endpoint, want: Option<u32>) -> Option<(bool, u32, Handle)> {
    let end = now_ms() + CHOSEN_TIMEOUT_MS;
    let mut node = file::Client::new(node);
    node.set_timeout_ms(CHOSEN_TIMEOUT_MS);
    let info = node.info().ok()?;
    let folder = info.kind == NodeKind::Folder;
    let has = info.rights.0 & (FsRights::READ.0 | FsRights::WRITE.0);
    let rights = want.unwrap_or(has);
    if has & rights != rights {
        return None;
    }
    // Step 3 for a chosen file: a capability of its own, derived from the one the caller
    // passed, so revoking it later takes back only this app's, with the app's share of the
    // filesystem server's records (note 0026, section 4).
    node.set_timeout_ms(end.saturating_sub(now_ms()).max(1));
    let own = node.limit(file::FileRights(rights), RECORDS).ok()?;
    Some((folder, rights, own.into_endpoint().0))
}

/// Runs on a helper thread, for a `grant` that something the app holds already covers: opens
/// `rest` (empty for the same file or folder) through `held` with `rights`, within
/// [`CHOSEN_TIMEOUT_MS`]. What it gives is derived from the app's own grant, so it goes when
/// that does, and it counts against that grant's share of records.
fn open_through(held: Endpoint, rest: &str, rights: u32) -> Option<(bool, u32, Handle)> {
    let mut node = file::Client::new(held);
    node.set_timeout_ms(CHOSEN_TIMEOUT_MS);
    let (node, folder) = if rest.is_empty() {
        let folder = node.info().ok()?.kind == NodeKind::Folder;
        (node.reduce(file::FileRights(rights)).ok()?, folder)
    } else {
        let opened = node.open(rest, file::FileRights(rights)).ok()?;
        (opened.node, opened.info.kind == NodeKind::Folder)
    };
    Some((folder, rights, node.into_endpoint().0))
}

impl CheckFor {
    /// Answers with an error.
    fn fail(self, e: SandboxError) {
        answered(match self {
            CheckFor::Choose { reply, .. } => reply.err(e),
            CheckFor::Grant { reply, .. } => reply.err(e),
        })
    }
}

impl Sandbox {
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

    /// App `name`'s manifest, parsed, if it is in the start-up archive with its program.
    fn manifest(&self, name: &str) -> Result<App, SandboxError> {
        if !is_name(name) {
            return Err(SandboxError::NoSuchApp);
        }
        // (A manifest without its program can still be found to be broken.)
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
        Ok(app)
    }

    fn prepare(&mut self, caller: u64, name: &str, args: &str) -> Result<u32, SandboxError> {
        // The limits (note 0024, section 9): a few for each caller, and not many in all, so
        // that no caller can keep the others (or this program's own launches) from launching.
        let theirs = self.launches.iter().filter(|l| l.caller == caller).count();
        if theirs >= MAX_LAUNCHES_EACH || self.launches.len() >= MAX_LAUNCHES {
            return Err(SandboxError::TooMany);
        }
        // 1. The manifest.
        let app = self.manifest(name)?;
        // 2. What it gets. Every app in the start-up archive is a system app.
        let rules = self.rules().map_err(|e| {
            granite::println!(
                "sandbox ({}): the rules file {e}, so nothing is launched until it is fixed.",
                self.user
            );
            SandboxError::BadRules
        })?;
        let d = decide(name, &app, &rules, true).map_err(|Refused::SystemOnly| {
            granite::println!(
                "sandbox ({}): {name} asks for what only system apps may.",
                self.user
            );
            SandboxError::BadManifest
        })?;
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
                grants.push(Held {
                    path: g.place.path(),
                    folder: true,
                    rights: fs_rights(g.rights),
                    reason: reason(g.reason),
                    cap: None,
                });
            }
        }
        // Its own folder (a trusted app reaches it through `/Home` anyway: it is made, but
        // not added).
        if let Some((rights, why)) = d.data
            && !d.trusted
        {
            grants.push(Held {
                path: format!("{APPS_FOLDER}/{name}"),
                folder: true,
                rights: fs_rights(rights),
                reason: reason(why),
                cap: None,
            });
        }
        let mut args = match (app.args.is_empty(), args.is_empty()) {
            (_, true) => app.args.clone(),
            (true, false) => String::from(args),
            (false, false) => format!("{} {args}", app.args),
        };
        if self.describe && (name == PICKER || name == VIEWER) {
            args =
                if args.is_empty() { String::from("describe") } else { format!("{args} describe") };
        }
        if args.len() > MAX_ARGS {
            return Err(SandboxError::TooMany);
        }
        let id = self.next_launch;
        self.next_launch += 1;
        self.launches.push(Launch {
            id,
            caller,
            name: String::from(name),
            title: app.title.clone(),
            budget: app.budget,
            args,
            extras: Extras {
                trusted: d.trusted,
                terminal: d.terminal,
                windows: d.windows,
                system: d.system,
                desktop: d.desktop,
                viewer: d.sandbox_view,
                picker: d.picker,
                sysinfo: d.sysinfo,
                data: d.data.map(|(_, why)| reason(why)),
            },
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

    /// Adds what the user chose (note 0017, section 6): checks what can be checked at once, and
    /// hands the capability to a helper thread to ask its server what it is. The answer is
    /// given when that finishes ([`Sandbox::finish_checks`]).
    fn choose(
        &mut self,
        caller: u64,
        id: u32,
        path: String,
        node: Endpoint,
        reply: sandbox::choose::Reply,
    ) {
        let early = (|| {
            self.launch(caller, id)?;
            // A tidy absolute path, as everywhere in a tree: what normalising leaves unchanged.
            if pbr_filetree::normalize("/", &path).as_deref() != Ok(path.as_str()) || path == "/" {
                return Err(SandboxError::BadPath);
            }
            if self.checking.len() >= MAX_CHECKING {
                return Err(SandboxError::TooMany);
            }
            Ok(())
        })();
        if let Err(e) = early {
            return answered(reply.err(e));
        }
        self.check(path, CheckFor::Choose { caller, launch: id, reply }, move || {
            check_chosen(node, None)
        });
    }

    /// Runs `work` on a helper thread; when it signals, [`Sandbox::finish_checks`] answers.
    fn check(
        &mut self,
        path: String,
        to: CheckFor,
        work: impl FnOnce() -> Option<(bool, u32, Handle)> + Send + 'static,
    ) {
        let job = self.next_job;
        self.next_job += 1;
        let found = self.checked.clone();
        let signals = match self.signals.0.duplicate(rights::ALL) {
            Ok(h) => Notification(h),
            Err(_) => return to.fail(SandboxError::Failed),
        };
        // A *closure* (`move || …`) is a function made on the spot that takes what it uses
        // with it: here the capability and the shared list, owned by the new thread.
        let thread = granite::thread::spawn(move || {
            let result = work();
            found.lock().push((job, result));
            let _ = signals.signal(CHECKED);
        });
        match thread {
            Ok(t) => self.checking.push(Checking { job, path, to, thread: Some(t) }),
            Err(e) => {
                granite::println!("sandbox ({}): a helper thread: {e}", self.user);
                to.fail(SandboxError::NoMemory);
            }
        }
    }

    /// Answers the `choose` and `grant` requests whose capabilities have been checked.
    fn finish_checks(&mut self) {
        let done: Vec<Checked> = core::mem::take(&mut *self.checked.lock());
        for (job, result) in done {
            let Some(at) = self.checking.iter().position(|c| c.job == job) else { continue };
            let mut c = self.checking.remove(at);
            // The thread has finished (it signalled after its last step): this only collects it.
            if let Some(t) = c.thread.take() {
                let _ = t.join();
            }
            match c.to {
                CheckFor::Choose { caller, launch, reply } => {
                    let answer = match result {
                        Some((folder, rights, own)) => {
                            self.add_chosen(caller, launch, &c.path, folder, rights, own)
                        }
                        None => Err(SandboxError::BadCapability),
                    };
                    answered(match answer {
                        Ok(()) => reply.ok(),
                        Err(e) => reply.err(e),
                    });
                }
                CheckFor::Grant { app, covered, reply } => {
                    let answer = match result {
                        Some((_, _, given)) if covered => Ok(given),
                        Some((folder, rights, own)) => {
                            self.add_granted(app, &c.path, folder, rights, own)
                        }
                        None => Err(SandboxError::BadCapability),
                    };
                    answered(match answer {
                        Ok(cap) => reply.ok(OutCap::Cap(Endpoint(cap))),
                        Err(e) => reply.err(e),
                    });
                }
            }
        }
    }

    /// Gives a running app a file or folder the user chose in the picker (note 0025, section
    /// 1): checked here as far as it can be at once, then on a helper thread (the server must
    /// be asked), and answered by [`Sandbox::finish_checks`].
    fn grant(
        &mut self,
        id: u32,
        path: String,
        node: Endpoint,
        rights: u32,
        reply: sandbox::grant::Reply,
    ) {
        let user = self.user.clone();
        let early = (|| {
            if pbr_filetree::normalize("/", &path).as_deref() != Ok(path.as_str()) || path == "/" {
                return Err(SandboxError::BadPath);
            }
            let rw = FsRights::READ.0 | FsRights::WRITE.0;
            if rights & !rw != 0 || rights & FsRights::READ.0 == 0 {
                return Err(SandboxError::BadCapability);
            }
            if self.checking.len() >= MAX_CHECKING {
                return Err(SandboxError::TooMany);
            }
            let app = self.app(id)?;
            // Only an app granted the picker can be given what was chosen in it. (Its manifest's
            // grant, not the service itself: the self-test, with no display, has no picker to
            // hand out, and stands in for it.)
            if !app.extras.picker {
                return Err(SandboxError::NotAllowed);
            }
            // Something it holds already covers the path: the answer comes through that.
            for g in &app.grants {
                let rest = if g.path == path {
                    Some("")
                } else if g.folder && inside(&g.path, &path) {
                    Some(if g.path == "/" { &path[1..] } else { &path[g.path.len() + 1..] })
                } else {
                    None
                };
                if let Some(rest) = rest {
                    if g.rights & rights != rights {
                        return Err(SandboxError::Overlaps);
                    }
                    let held = g.cap.as_ref().ok_or(SandboxError::Failed)?;
                    let held = held.duplicate(rights::SEND).map_err(|_| SandboxError::Failed)?;
                    return Ok(Some((Endpoint(held), String::from(rest))));
                }
                // A chosen folder may not contain something the app gets: its tree can't hold
                // one entry inside another (note 0017, Changes).
                if inside(&path, &g.path) || (!g.folder && inside(&g.path, &path)) {
                    return Err(SandboxError::Overlaps);
                }
            }
            if app.grants.len() >= pbr_filetree::MAX_ENTRIES {
                return Err(SandboxError::TooMany);
            }
            Ok(None)
        })();
        match early {
            Err(e) => {
                granite::println!("sandbox ({user}): not granting {path} to app {id}: {e:?}");
                answered(reply.err(e))
            }
            Ok(Some((held, rest))) => {
                let to = CheckFor::Grant { app: id, covered: true, reply };
                self.check(path, to, move || open_through(held, &rest, rights));
            }
            Ok(None) => {
                let to = CheckFor::Grant { app: id, covered: false, reply };
                self.check(path, to, move || check_chosen(node, Some(rights)));
            }
        }
    }

    /// The rest of `grant`, once the chosen capability is known to be a file or folder and
    /// `own` is this program's capability for it: the grant is recorded, and the app is given
    /// a copy, derived from `own`, so that revoking `own` takes it back.
    fn add_granted(
        &mut self,
        id: u32,
        path: &str,
        folder: bool,
        rights: u32,
        own: Handle,
    ) -> Result<Handle, SandboxError> {
        let user = self.user.clone();
        // (The app may have ended, or been given something covering it, meanwhile.)
        let app = self.app(id)?;
        if app
            .grants
            .iter()
            .any(|g| g.path == path || inside(&g.path, path) || inside(path, &g.path))
        {
            return Err(SandboxError::Overlaps);
        }
        if app.grants.len() >= pbr_filetree::MAX_ENTRIES {
            return Err(SandboxError::TooMany);
        }
        let theirs = own.duplicate(CLIENT).map_err(|_| SandboxError::Failed)?;
        granite::println!(
            "sandbox ({user}): {} was given {path} ({}), chosen in the picker.",
            app.name,
            if rights & FsRights::WRITE.0 != 0 { "read and write" } else { "read" }
        );
        app.grants.push(Held {
            path: String::from(path),
            folder,
            rights,
            reason: GrantReason::Chosen,
            cap: Some(own),
        });
        Ok(theirs)
    }

    /// The rest of `choose`, once the chosen capability is known to be a file or folder with
    /// `rights`, and `own` is the app's copy of it.
    fn add_chosen(
        &mut self,
        caller: u64,
        id: u32,
        path: &str,
        folder: bool,
        rights: u32,
        own: Handle,
    ) -> Result<(), SandboxError> {
        // (The launch may have been started or cancelled meanwhile.)
        let launch = self.launch(caller, id)?;
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
        launch.grants.push(Held {
            path: String::from(path),
            folder,
            rights,
            reason: GrantReason::Chosen,
            cap: Some(own),
        });
        Ok(())
    }

    fn cancel(&mut self, caller: u64, id: u32) -> Result<(), SandboxError> {
        let at = self
            .launches
            .iter()
            .position(|l| l.id == id && l.caller == caller)
            .ok_or(SandboxError::NotFound)?;
        self.launches.remove(at);
        Ok(())
    }

    /// Makes sure `/Home/Apps/NAME` is there (note 0024, section 4).
    fn make_data_folder(&self, name: &str) -> Result<(), SandboxError> {
        for path in [String::from(APPS_FOLDER), format!("{APPS_FOLDER}/{name}")] {
            let rw = FsRights(FsRights::READ.0 | FsRights::WRITE.0);
            match self.files.create(&path, true, rw) {
                Ok(_) | Err(FilesError::File(file::FileError::AlreadyExists)) => {}
                Err(e) => {
                    granite::println!("sandbox ({}): making {path}: {e}", self.user);
                    return Err(SandboxError::Failed);
                }
            }
        }
        Ok(())
    }

    /// A window capability for an app, through a copy of the session's display capability
    /// made for it alone: (the app's capability, the copy, to keep).
    fn window_capability(
        &self,
        app: u32,
        title: &str,
        extras: Extras,
        by_user: bool,
    ) -> Option<(Endpoint, Handle)> {
        let session = self.display.as_ref()?;
        let copy = session
            .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE | rights::REVOKE)
            .ok()?;
        let mut client =
            display::Client::new(Endpoint(copy.duplicate(rights::SEND | rights::TRANSFER).ok()?));
        client.set_timeout_ms(DISPLAY_MS);
        let mut flags = 0;
        if extras.system {
            flags |= display::ClientFlags::SYSTEM.0;
        }
        if extras.desktop {
            flags |= display::ClientFlags::DESKTOP.0;
        }
        if extras.trusted {
            flags |= display::ClientFlags::TRUSTED.0;
        }
        // Launched by the user just now, so that its first window may take the focus (note
        // 0022, section 4); the apps this program starts itself weren't.
        let launched = if by_user { time::now() } else { 0 };
        match client.client(app, title, display::ClientFlags(flags), launched) {
            Ok(ep) => Some((ep, copy)),
            Err(e) => {
                granite::println!("sandbox ({}): no window capability for {title}: {e}", self.user);
                None
            }
        }
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
        let extras = launch.extras;
        if extras.data.is_some() {
            self.make_data_folder(&launch.name)?;
        }

        // 3. A capability for each grant, derived from the session's with the rights granted
        // and the grant's share of records (note 0026, section 4).
        let mut grants = Vec::new();
        for mut g in launch.grants.drain(..) {
            if g.cap.is_none() {
                let share =
                    if g.reason == GrantReason::Trusted { TRUSTED_RECORDS } else { RECORDS };
                let made = self
                    .files
                    .open(&g.path, FsRights(g.rights))
                    .and_then(|f| f.limit(FsRights(g.rights), share));
                match made {
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
        // the kernel log, without the right to take over the screen, and what it is granted
        // at its roles (note 0024, section 10).
        let app_id = self.next_app;
        let mut services = AppServices(0);
        if extras.data.is_some() {
            services.0 |= AppServices::DATA.0;
        }
        let mut keep: Vec<Handle> = Vec::new();
        let mut caps: Vec<process::Grant> = Vec::new();
        if granite::log::handle() != 0 {
            caps.push((role::LOG, granite::log::handle(), rights::ALL & !rights::CONTROL, false));
        }
        // The picker this program keeps running is told so by its badges and roles: its
        // `pbr.sandbox` may only grant, and it serves the picker's endpoint.
        let is_picker = caller == ITSELF && launch.name == PICKER;
        // `pbr.sandbox`, badged with what this app may do, and watched, so that its launches
        // go when it does.
        let kind = if is_picker {
            Some(badge::PICKER)
        } else if extras.trusted {
            Some(badge::TRUSTED)
        } else if extras.desktop {
            Some(badge::DESKTOP)
        } else if extras.viewer {
            Some(badge::VIEWER)
        } else {
            None
        };
        if let Some(kind) = kind {
            let own = self
                .endpoint
                .mint_watched(badge::make(kind, app_id), CLIENT)
                .map_err(|_| SandboxError::Failed)?;
            caps.push((APP_SANDBOX, own.0.raw(), rights::ALL, false));
            keep.push(own.0);
            services.0 |= AppServices::SANDBOX.0;
        }
        // `pbr.users`: all of the session's for a trusted app; the desktop's can only ask who
        // the user is, the settings and the power operations (note 0024, section 6).
        let users = if is_picker {
            None
        } else if extras.trusted {
            self.users.as_ref()
        } else if extras.desktop {
            self.desktop_users.as_ref()
        } else {
            None
        };
        if let Some(u) = users {
            caps.push((APP_USERS, u.0.raw(), CLIENT, false));
            services.0 |= AppServices::USERS.0;
        }
        // The picker: its endpoint's receiving end for the picker itself; for an app granted
        // `picker`, a capability whose badge is the app's number, watched, so that the picker
        // hears when the app has gone (note 0025, section 1).
        if let Some(ep) = &self.picker {
            if is_picker {
                let serve = ep.0.duplicate(rights::RECEIVE).map_err(|_| SandboxError::Failed)?;
                caps.push((APP_PICKER, serve.raw(), rights::RECEIVE, false));
                keep.push(serve);
            } else if extras.picker {
                let theirs = ep
                    .mint_watched(u64::from(app_id), rights::SEND | rights::TRANSFER)
                    .map_err(|_| SandboxError::Failed)?;
                caps.push((APP_PICKER, theirs.0.raw(), rights::SEND | rights::TRANSFER, false));
                keep.push(theirs.0);
                services.0 |= AppServices::PICKER.0;
            }
        }
        // System information, if it is granted.
        if extras.sysinfo
            && let Some(i) = &self.sysinfo
        {
            caps.push((APP_SYSINFO, i.0.raw(), CLIENT, false));
            services.0 |= AppServices::SYSINFO.0;
        }
        // The terminal, if it is granted and this session has it.
        if extras.terminal
            && let Some(t) = &self.terminal
        {
            caps.push((APP_TERMINAL, t.endpoint().0.raw(), CLIENT, false));
            services.0 |= AppServices::TERMINAL.0;
        }
        // A window capability, if it is granted and this session has the display. It is moved
        // to the app and can't be copied: the display server hears when the app is done with
        // it (note 0022, Changes).
        let mut windows = None;
        if extras.windows
            && let Some((client, copy)) =
                self.window_capability(app_id, &launch.title, extras, caller != ITSELF)
        {
            caps.push((APP_WINDOW, client.0.into_raw(), rights::SEND | rights::TRANSFER, true));
            windows = Some(copy);
            services.0 |= AppServices::WINDOWS.0;
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
            if let Some(w) = &windows {
                let _ = w.revoke();
            }
            match e {
                SpawnError::System(sys::Error::OutOfMemory) => SandboxError::NoMemory,
                _ => SandboxError::Failed,
            }
        })?;
        // (The copies given to it are its own now; ours of its badge must go, or the badge
        // would never be seen to have gone.)
        drop(keep);

        // 6. The record, and a watch for when it ends.
        let bit = (0..MAX_APPS as u32)
            .find(|b| !self.apps.iter().any(|a| a.bit == *b))
            .ok_or(SandboxError::TooMany)?;
        if let Err(e) = spawned.process.watch(&self.signals, 1 << bit) {
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
            "sandbox ({}): started {} as app {app_id}{}, holding {} entries{}.",
            self.user,
            launch.name,
            if extras.trusted { " (trusted)" } else { "" },
            grants.len(),
            if windows.is_some() { " and windows" } else { "" }
        );
        let kept = match caller {
            ITSELF => self.kept.iter().find(|k| k.name == launch.name).map(|k| k.name),
            _ => None,
        };
        self.apps.push(Running {
            id: app_id,
            name: launch.name,
            title: launch.title,
            extras,
            services,
            started: time::now(),
            bit,
            process: spawned.process,
            _thread: spawned.thread,
            grants,
            windows,
            kept,
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
        // Its windows go at once, even before its threads have stopped.
        if let Some(w) = &app.windows {
            let _ = w.revoke();
        }
        let _ = app.process.stop(0);
        granite::println!("sandbox ({}): stopped {}.", self.user, app.name);
        if let Some(name) = app.kept {
            self.kept_ended(name);
        }
        Ok(())
    }

    /// An app this program keeps running has ended: it is started again, with the restart
    /// policy, while what it needs is still there.
    fn kept_ended(&mut self, name: &str) {
        let user = &self.user;
        let Some(k) = self.kept.iter_mut().find(|k| k.name == name) else { return };
        match k.history.crashed(now_ms()) {
            Decision::Restart { delay_ms } => k.due = Some(now_ms() + delay_ms),
            Decision::GiveUp => {
                k.due = None;
                granite::println!(
                    "sandbox ({user}): the {} ended too often; it is left stopped.",
                    k.name
                );
            }
        }
    }

    /// Starts the shell, the desktop or the picker, if that is due.
    fn start_kept_if_due(&mut self) {
        let now = now_ms();
        let Some(k) = self.kept.iter_mut().find(|k| k.due.is_some_and(|t| t <= now)) else {
            return;
        };
        k.due = None;
        let name = k.name;
        if name == SHELL {
            let Some(t) = &self.terminal else { return };
            // A terminal that has gone stays gone: the users service starts this session again
            // with the new console's.
            if let Err(e) = t.size()
                && matches!(
                    e,
                    CallError::Call(
                        granite::Error::PeerGone
                            | granite::Error::Revoked
                            | granite::Error::BadHandle
                    )
                )
            {
                granite::println!("sandbox ({}): the terminal has gone.", self.user);
                self.terminal = None;
                return;
            }
        } else if self.display.is_none() {
            return;
        }
        if let Some(k) = self.kept.iter_mut().find(|k| k.name == name) {
            k.history.started(now_ms());
        }
        let started = self.prepare(ITSELF, name, "").and_then(|id| self.start(ITSELF, id));
        if let Err(e) = started {
            granite::println!("sandbox ({}): can't start the {name}: {e:?}", self.user);
            self.kept_ended(name);
        }
    }

    /// The display server has gone, and every window with it: the apps that had windows are
    /// stopped, and the desktop isn't started again (the users service starts this session
    /// afresh with the new display server's session; note 0022, section 11).
    fn display_gone(&mut self) {
        if self.display.take().is_none() {
            return;
        }
        granite::println!("sandbox ({}): the display server has gone.", self.user);
        let windowed: Vec<u32> = self
            .apps
            .iter()
            .filter(|a| a.services.0 & AppServices::WINDOWS.0 != 0)
            .map(|a| a.id)
            .collect();
        for id in windowed {
            let _ = self.stop(id);
        }
    }

    /// Some apps have ended: their records go (their capabilities went with their processes;
    /// the ones kept here are closed as the records are dropped).
    fn ended(&mut self, bits: u64) {
        if bits == 0 {
            return;
        }
        let user = &self.user;
        let mut kept_ended: Vec<&'static str> = Vec::new();
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
                    kept_ended.extend(a.kept);
                    false
                }
            }
        });
        for name in kept_ended {
            self.kept_ended(name);
        }
    }

    /// The apps in the start-up archive, for the start menu.
    fn available(&self) -> Vec<Available> {
        let mut out: Vec<Available> = Vec::new();
        for e in self.archive.entries() {
            let Some(name) = e.name.strip_prefix("apps/").and_then(|n| n.strip_suffix(".app"))
            else {
                continue;
            };
            // Only apps whose manifest reads and whose program is there.
            let Ok(app) = self.manifest(name) else { continue };
            out.push(Available {
                app: String::from(name),
                title: app.title,
                icon: app.icon.unwrap_or_default(),
                windows: app.windows,
                hidden: app.hidden,
            });
        }
        out.sort_by(|a, b| a.app.cmp(&b.app));
        out.truncate(32);
        out
    }
}

/// Whether the caller (by its reply's badge) may call operation `op`.
fn may(badge: u64, op: u32) -> bool {
    allowed(badge::kind(badge), op)
}

// The generated `Server` trait: one method per operation, each answering through its `reply`.
impl sandbox::Server for Sandbox {
    fn prepare(&mut self, app: String, args: String, reply: sandbox::prepare::Reply) {
        if !may(reply.badge(), op::PREPARE) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(match Sandbox::prepare(self, reply.badge(), &app, &args) {
            Ok(id) => reply.ok(id),
            Err(e) => reply.err(e),
        });
    }

    fn choose(&mut self, launch: u32, path: String, node: Endpoint, reply: sandbox::choose::Reply) {
        if !may(reply.badge(), op::CHOOSE) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        Sandbox::choose(self, reply.badge(), launch, path, node, reply);
    }

    fn start(&mut self, launch: u32, reply: sandbox::start::Reply) {
        if !may(reply.badge(), op::START) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(match Sandbox::start(self, reply.badge(), launch) {
            Ok((id, p)) => reply.ok(id, OutCap::Cap(p)),
            Err(e) => reply.err(e),
        });
    }

    fn list(&mut self, reply: sandbox::list::Reply) {
        if !may(reply.badge(), op::LIST) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        let apps: Vec<AppInfo> = self
            .apps
            .iter()
            .map(|a| AppInfo {
                id: a.id,
                app: a.name.clone(),
                trusted: a.extras.trusted,
                running: matches!(a.process.info().map(|i| i.state), Ok(State::Running)),
            })
            .collect();
        answered(reply.ok(&apps));
    }

    fn grants(&mut self, app: u32, from: u32, reply: sandbox::grants::Reply) {
        const PAGE: usize = 32;
        if !may(reply.badge(), op::GRANTS) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
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
        if !may(reply.badge(), op::REVOKE) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(match Sandbox::revoke(self, app, &path) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn stop(&mut self, app: u32, reply: sandbox::stop::Reply) {
        if !may(reply.badge(), op::STOP) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(match Sandbox::stop(self, app) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn cancel(&mut self, launch: u32, reply: sandbox::cancel::Reply) {
        if !may(reply.badge(), op::CANCEL) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(match Sandbox::cancel(self, reply.badge(), launch) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn details(&mut self, app: u32, reply: sandbox::details::Reply) {
        if !may(reply.badge(), op::DETAILS) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        let Some(a) = self.apps.iter().find(|a| a.id == app) else {
            return answered(reply.err(SandboxError::NotFound));
        };
        answered(reply.ok(&AppDetails {
            id: a.id,
            app: a.name.clone(),
            title: a.title.clone(),
            trusted: a.extras.trusted,
            system: a.extras.system,
            services: a.services,
            started: a.started,
        }));
    }

    fn grant(
        &mut self,
        app: u32,
        path: String,
        node: Endpoint,
        rights: FileRights,
        reply: sandbox::grant::Reply,
    ) {
        if !may(reply.badge(), op::GRANT) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        Sandbox::grant(self, app, path, node, rights.0, reply);
    }

    fn apps(&mut self, reply: sandbox::apps::Reply) {
        if !may(reply.badge(), op::APPS) {
            return answered(reply.err(SandboxError::NotAllowed));
        }
        answered(reply.ok(&self.available()));
    }
}
