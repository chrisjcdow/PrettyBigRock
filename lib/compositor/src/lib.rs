// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The display server's rules (docs/design/0022-display-server.md), as plain computation that
//! is built and tested on the host, like `lib/shell` and `lib/seat`:
//!
//! - [`space`]: the screens and the one space of logical pixels they make together;
//! - [`desk`]: sessions, clients and their windows: stacking, the focus, buffers, frames,
//!   pings, the clipboard, and every limit and check on what a client asks;
//! - [`input`]: where keys and the pointer go;
//! - [`decor`]: where the parts of a window's frame and title bar are;
//! - [`draw`]: drawing decorations, the pointer and the window menu, and composing a screen;
//! - [`scene`]: what is on the screens, to draw and to describe in words for the tests;
//! - [`event`]: `pbr.window`'s 32-byte events.
//!
//! `services/display` is the program around them: it takes the screens and the seat, serves
//! `pbr.window` and the session interfaces, makes the window buffers and presents.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod decor;
pub mod desk;
pub mod draw;
pub mod event;
pub mod input;
pub mod scene;
pub mod space;

pub use desk::{
    Answer, Buffers, ClientId, Desk, Made, Menu, MenuItem, Out, Shown, Tablet, WindowId, WindowInfo,
};
pub use scene::{Scene, SceneWindow};
pub use space::{Output, Place, Screen, Space};
