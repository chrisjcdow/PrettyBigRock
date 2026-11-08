// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The logic of the text shell (docs/design/0018-supervision-shell-and-m5-tests.md, section 6).
//!
//! The shell is a program (`apps/shell`) that reads a line from the terminal, runs it, and
//! prints what happened. Nearly everything that can be wrong with that is plain computation, so
//! it lives here, in a library that builds and is tested on the computer doing the build; the
//! program is only the glue that talks to the system.
//!
//! The pieces, in the order a line meets them:
//! - [`words`]: splitting a line into words, with quotes;
//! - [`command`]: turning the words into a typed [`Command`] with its arguments checked;
//! - [`help`]: the text of `help`, `about` and `potato`, and each command's usage line;
//! - [`layout`]: making paths absolute, and knowing which folders exist only because something
//!   is attached below them;
//! - [`fs`]: the trait ([`Fs`]) the program implements on its file tree;
//! - [`files`]: the file commands, written against that trait ([`FileShell`]);
//! - [`apps`]: which app a command means, and the sandbox manager's limits on paths;
//! - [`reason`]: errors in plain English, for every service the shell talks to;
//! - [`format`]: sizes, times, listings, tables and prompts.
//!
//! Every error line the shell prints is `<command>: <path>: <reason>` (or, for a command
//! with nothing to name, `<command>: <reason>`), with the reason in plain English, never a Rust
//! debug dump.
//!
//! The library is `no_std`: it uses Rust's `core` and `alloc` (strings, vectors) but not its
//! standard library, because a PrettyBigRock program has no operating system underneath yet to
//! provide one. Its tests run on the host, where `std` is available.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod apps;
pub mod command;
pub mod files;
pub mod format;
pub mod fs;
pub mod help;
pub mod layout;
pub mod reason;
pub mod words;

pub use command::{AppRef, Command, LineError, ParseError, Problem, parse, parse_line};
pub use files::FileShell;
pub use fs::{Fs, Item, Space, Stat};
pub use reason::{Failure, Reason, SandboxOp, UsersOp, sandbox_reason, users_reason};
