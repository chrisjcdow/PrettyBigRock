// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Sessions (docs/design/0022-display-server.md, section 1). Served by the display server to
/// the users service, which the root program's lookup table alone gives it to.
///
/// A session is one user's windows. Only the active session's windows are shown and get the
/// keyboard and pointer. M6 has one session at a time; the interface has several so that
/// switching between logged-in users can come later.
interface pbr.display.sessions version 1;

/// What the About window shows of each screen.
struct ScreenSummary { connector: string<32>, width: u32, height: u32, scale: u32 }

/// A capability for a new session, for its sandbox manager. It is derived from the caller's,
/// so revoking the caller's takes it too, and the display server hears when it has gone (and
/// closes every window of the session then). `number` names it to `activate`. (A capability
/// passed back can't be told apart from another by the service that made it, so a number is
/// used; only the users service can call these, so it can't be used by anyone else.)
@1 fn start(user: string<32>) -> (session: handle<pbr.display.session>, number: u32)
    error SessionsError;
/// Shows session `number` and gives it the input.
@2 fn activate(number: u32) -> () error SessionsError;
/// Shows the shutting-down screen on every screen (note 0024, section 7) and stops showing
/// windows. Nothing else is drawn until the display server starts again.
@3 fn farewell(restarting: bool) -> () error SessionsError;
/// The screens, for the About window's system information (note 0026, section 3).
@4 fn screens() -> (screens: list<ScreenSummary, 16>) error SessionsError;

/// What can go wrong.
flexible enum SessionsError {
    /// Not called through the users service's capability.
    NotAllowed @1,
    /// Too many sessions at once (4).
    TooMany @2,
    /// No session with that number.
    BadSession @3,
}
