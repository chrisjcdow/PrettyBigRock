// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Testing the users service (docs/design/0018-supervision-shell-and-m5-tests.md, section 7).
/// Served only when it was started with `selftest` or `desktoptest`, under its own badge, to the root program,
/// which stands in for a user at the keyboard. The operation numbers are far from
/// `pbr.users`'s.
interface pbr.users.debug version 1;

/// The session for user `name`, started if it isn't running: its `pbr.users` capability, and
/// a `pbr.sandbox` capability like the one its shell will have.
@100 fn session(name: string<32>) -> (users: handle<any>, sandbox: handle<any>) error DebugError;

/// What a `pbr.sandbox` capability from `sandbox` may do (note 0024, section 9).
enum SandboxKind { Trusted @1, Desktop @2, Viewer @3, Picker @4 }

/// Another `pbr.sandbox` capability for user `name`'s running session, badged as `kind`'s
/// holders' are, so that the self-test can check what each kind may do.
@102 fn sandbox(name: string<32>, kind: SandboxKind) -> (sandbox: handle<any>) error DebugError;

/// Where the users are kept: the system volume's id, and whether it is in memory.
@101 fn system() -> (volume: u32, in_memory: bool) error DebugError;

/// What can go wrong.
flexible enum DebugError {
    NoSuchUser @1,
    /// The session couldn't be started.
    Failed @2,
}
