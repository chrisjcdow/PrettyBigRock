// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Running an app in PrettyBigRock: the event loop of `lib/window` (note 0023, section 4) with
//! sandstone's [`Runtime`] handling each event.

use crate::app::{App, Settings};
use crate::runtime::Runtime;
use pbr_window::Connection;

/// Why an app couldn't run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunError {
    /// No window capability at start-up role 1003, or the display server didn't answer.
    NoDisplay,
    /// The display server refused the main window.
    NoWindow,
}

/// Runs `app` until it ends: takes the app's window capability from its start-up handles,
/// opens the main window, and handles events until the app quits, its main window is closed,
/// or the display server goes.
#[cfg(target_os = "prettybigrock")]
pub fn run<A: App>(start: &mut granite::Start, app: A, settings: Settings) -> Result<(), RunError> {
    let conn = Connection::from_start(start).map_err(|_| RunError::NoDisplay)?;
    run_on(conn, app, settings)
}

/// [`run`], on a connection the app already made.
pub fn run_on<A: App>(mut conn: Connection, app: A, settings: Settings) -> Result<(), RunError> {
    let mut rt = Runtime::new(app, settings);
    if !rt.open(&mut conn) {
        return Err(RunError::NoWindow);
    }
    // `run` gives the handler the connection each time; it is the `Platform` sandstone draws
    // through. `&mut *c` lends it on for the one call.
    conn.run(|c, event| rt.handle(&mut *c, event));
    Ok(())
}
