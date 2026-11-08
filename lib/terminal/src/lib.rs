// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A text terminal without the hardware (docs/design/0018-supervision-shell-and-m5-tests.md,
//! section 5): everything the console service does that is a matter of rules, not of devices,
//! so that it can be tested on the host.
//!
//! - [`grid`]: the characters on the screen, a cursor, wrapping, scrolling, and a record of
//!   which rows changed so that only those are drawn again.
//! - [`editor`]: the line being typed (Backspace, Delete, the arrows, history) on abstract
//!   [`Key`]s.
//! - [`view`]: showing that line, with its prompt, on the grid, wherever it wraps or scrolls.
//! - [`input`]: turning a keyboard's 16-byte events into [`Key`]s, with Shift.
//! - [`mirror`]: the log records the console makes in self-test runs, so that the test runner
//!   can see what is on a screen it cannot see.
//! - [`terminal`]: all of the above together, as the console service uses it.
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries (the standard
//! library needs an operating system underneath), so a PrettyBigRock program can use it. The
//! tests run on the host and may use the standard library.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod editor;
pub mod grid;
pub mod input;
pub mod mirror;
pub mod terminal;
pub mod view;

pub use editor::{Key, LineEditor};
pub use grid::{Changes, Grid};
pub use input::KeyDecoder;
pub use mirror::Mirror;
pub use terminal::{Busy, Terminal};
pub use view::EditView;
