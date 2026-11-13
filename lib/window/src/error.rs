// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What can go wrong in this library.

use granite::interface::CallError;
use pbr_interfaces::pbr_window::WindowError;

/// Why a window operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The display server refused.
    Window(WindowError),
    /// The call itself failed (the display server is not answering, or has gone).
    Call(granite::Error),
    /// The display server said something this library does not understand.
    Protocol,
    /// The display server has gone.
    Disconnected,
    /// The app has no `pbr.window` capability (the manifest lacks `windows`).
    NoCapability,
    /// This window is not one the connection knows.
    UnknownWindow,
    /// The ring or a window's buffers are not what they should be.
    Broken,
    /// An argument does not make sense (a picture whose pixels are too few, say).
    BadArgument,
    /// More than the clipboard holds.
    TooBig,
    /// The clipboard's text is not UTF-8.
    NotText,
    /// Waited too long for the display server.
    TimedOut,
}

impl From<CallError<WindowError>> for Error {
    fn from(e: CallError<WindowError>) -> Error {
        match e {
            CallError::Service(e) => Error::Window(e),
            CallError::Call(e) => Error::Call(e),
            CallError::Unknown | CallError::Rejected | CallError::Failed | CallError::Malformed => {
                Error::Protocol
            }
        }
    }
}

impl From<granite::Error> for Error {
    fn from(e: granite::Error) -> Error {
        Error::Call(e)
    }
}
