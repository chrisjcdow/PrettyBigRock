// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// One session (docs/design/0022-display-server.md, section 1). Served by the display server
/// to the session's sandbox manager, which asks it for a window capability for each app it
/// launches with the manifest key `windows` (note 0024).
interface pbr.display.session version 1;

/// What the sandbox manager says an app is (note 0022, section 8).
bits ClientFlags: u32 {
    /// Its windows may be `System` windows (the file picker, the permissions viewer).
    System @0,
    /// It is the session's desktop: backgrounds, panels, and the operations that arrange
    /// screens and act on any window.
    Desktop @1,
    /// It can reach all the user's files, and its title bars say so.
    Trusted @2,
}

/// A `pbr.window` capability for one app, derived from this one: revoking this one takes it
/// too, and the display server hears when it has gone and closes the app's windows. `app` is
/// the sandbox manager's number for the app, `title` its manifest's title, and `launched` when
/// the user launched it (monotonic nanoseconds; 0 if not by the user).
@1 fn client(app: u32, title: string<64>, flags: ClientFlags, launched: u64)
        -> (client: handle<pbr.window>) error SessionError;

/// What can go wrong.
flexible enum SessionError {
    /// Too many clients in the session at once (64).
    TooMany @1,
    BadArgument @2,
}
