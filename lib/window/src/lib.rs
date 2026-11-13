// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Windows for programs: the lowest layer an app uses to talk to the display server
//! (docs/design/0023-draw-window-and-sandstone.md, section 4; protocol in note 0022).
//!
//! A [`Connection`] makes windows, handles their configure events by itself (asking for new
//! frames when the size or scale changes), lends a [`pbr_draw::Canvas`] over the frame the app
//! may draw in, and turns the display server's ring of events into [`Event`]s, with timers and
//! the app's own wake-up bits in the same loop.
//!
//! Everything that is plain computation (decoding events, the timer queue, which frame is the
//! app's and what to copy into it) is in modules without system calls, tested on the host.
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries (no operating
//! system underneath it), except when the tests are built.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod clip;
mod connection;
mod error;
mod event;
pub mod frames;
pub mod timers;

pub use connection::{Connection, ROLE, WindowFlags, WindowKind};
pub use error::Error;
pub use event::{Event, Incoming, KeyEvent, PointerEvent, PointerKind, Win, decode, is_typed};
pub use pbr_draw::{Canvas, Damage};
pub use pbr_interfaces::pbr_window::{self as wire, Cursor, Edge, WindowError};
