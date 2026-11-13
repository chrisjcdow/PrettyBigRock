// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Launching apps and managing what they hold
/// (docs/design/0017-users-sandbox-manager-and-manifests.md, section 7). Served by each
/// session's sandbox manager, through capabilities badged with what their holder may do
/// (docs/design/0024-the-session-desktop.md, section 9): trusted apps (the shell, Terminal,
/// Files) may do everything but `grant`; the desktop may launch, `cancel`, `list`, `details`,
/// `stop` and `apps`; the permissions viewer may `list`, `details`, `grants`, `revoke` and
/// `stop`. Anything else answers `NotAllowed`.
interface pbr.sandbox version 1;

/// What a capability may do (copied from `pbr.file` until interfaces can import each other;
/// note 0008).
bits FileRights: u32 {
    Read @0,
    Write @1,
}

/// Why an app holds something.
enum GrantReason {
    /// Its manifest asked, and as a system app it gets what it asks for unless a rule says no.
    Manifest @1,
    /// A rule allowed it.
    Rule @2,
    /// The user chose it.
    Chosen @3,
    /// It is trusted: it holds the session's whole tree.
    Trusted @4,
}

/// One thing an app holds: where it is in its tree, and what it may do with it.
struct Grant { path: string<1024>, rights: FileRights, reason: GrantReason }

/// One running app.
struct AppInfo { id: u32, app: string<64>, trusted: bool, running: bool }

/// What an app holds besides files, each at a start-up role (note 0024, section 10).
bits AppServices: u32 {
    /// `pbr.window`, at role 1003.
    Windows @0,
    /// `pbr.picker`, at role 1004.
    Picker @1,
    /// `pbr.sysinfo`, at role 1005.
    Sysinfo @2,
    /// The console's terminal, at role 1002.
    Terminal @3,
    /// Its own folder, `/Home/Apps/APP`.
    Data @4,
    /// `pbr.users`, at role 1001.
    Users @5,
    /// `pbr.sandbox`, at role 1000.
    Sandbox @6,
}

/// More about a running app than `AppInfo`.
struct AppDetails {
    id: u32,
    app: string<64>,
    /// Its manifest's title.
    title: string<64>,
    trusted: bool,
    /// Its windows may be system windows.
    system: bool,
    services: AppServices,
    /// When it was started (nanoseconds on the monotonic clock).
    started: u64,
}

/// An app that can be launched: what the start menu and the desktop list (note 0024,
/// section 6).
struct Available {
    app: string<32>,
    /// Its manifest's title.
    title: string<64>,
    /// Its manifest's `icon`, or empty.
    icon: string<32>,
    /// Its manifest asks for `windows`.
    windows: bool,
    /// Its manifest says `hidden`.
    hidden: bool,
}

/// Starts preparing a launch of `apps/APP`, with `args` after its manifest's own arguments.
/// Reads the manifest and decides what the app gets. Each caller may have 4 launches waiting
/// at once, and there are at most 16 in all (`TooMany`); they go when `start` or `cancel` is
/// called, or when the caller's capability goes.
@1 fn prepare(app: string<64>, args: string<3072>) -> (launch: u32) error SandboxError;

/// Adds a file or folder the user chose, at `path` in the app's tree. If something the app
/// already gets covers it, with the rights this capability has, nothing more is added. The
/// capability's server is asked what it is with a 2-second limit, on a helper thread: other
/// requests are answered meanwhile.
@2 fn choose(launch: u32, path: string<1024>, node: handle<pbr.file>) -> () error SandboxError;

/// Starts the app. The process capability can be watched and inspected, not stopped.
@3 fn start(launch: u32) -> (app: u32, process: handle<process>) error SandboxError;

/// The running apps.
@4 fn list() -> (apps: list<AppInfo, 32>) error SandboxError;

/// What an app holds, and why: the permissions viewer. An app can hold up to 256 things (a
/// trusted one holds every volume), more than fit in one answer: this gives them from number
/// `from`, and `next` is where to carry on (0 when that was all).
@5 fn grants(app: u32, from: u32) -> (grants: list<Grant, 32>, next: u32) error SandboxError;

/// Takes back what an app holds at `path` (and everything it opened through it).
@6 fn revoke(app: u32, path: string<1024>) -> () error SandboxError;

/// Stops an app.
@7 fn stop(app: u32) -> () error SandboxError;

/// Gives up a prepared launch.
@8 fn cancel(launch: u32) -> () error SandboxError;

/// More about a running app than `list` says.
@9 fn details(app: u32) -> (details: AppDetails) error SandboxError;

// @10, `grant`, comes with the file picker (note 0025, M6 stage 8).

/// The apps in the start-up archive, in name order, with what the desktop shows of them.
@11 fn apps() -> (apps: list<Available, 32>) error SandboxError;

/// What can go wrong.
flexible enum SandboxError {
    /// There's no app by that name.
    NoSuchApp @1,
    /// Its manifest is malformed (the log says why).
    BadManifest @2,
    /// The user's rules file is malformed (the log says why): nothing is launched until it's
    /// fixed.
    BadRules @3,
    /// No launch, app or grant with that number or path.
    NotFound @4,
    /// A path that breaks the rules, or isn't absolute.
    BadPath @5,
    /// The chosen file or folder would contain something the app already gets, or sit inside
    /// it with more rights.
    Overlaps @6,
    /// Too many launches, apps or grants at once.
    TooMany @7,
    /// Not enough memory for it.
    NoMemory @8,
    /// The chosen capability doesn't answer as a file or folder.
    BadCapability @9,
    /// The program couldn't be started (the log says why).
    Failed @10,
    /// The caller's capability doesn't allow this operation.
    NotAllowed @11,
}
