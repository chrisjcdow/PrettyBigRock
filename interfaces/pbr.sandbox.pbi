// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Launching apps and managing what they hold
/// (docs/design/0017-users-sandbox-manager-and-manifests.md, section 7). Served by each
/// session's sandbox manager to the session's trusted programs (the shell).
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

/// Starts preparing a launch of `apps/APP`, with `args` after its manifest's own arguments.
/// Reads the manifest and decides what the app gets. A few launches can wait at once; when
/// they are all taken, the caller's own oldest one is dropped to make room.
@1 fn prepare(app: string<64>, args: string<3072>) -> (launch: u32) error SandboxError;

/// Adds a file or folder the user chose, at `path` in the app's tree. If something the app
/// already gets covers it, with the rights this capability has, nothing more is added.
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
}
