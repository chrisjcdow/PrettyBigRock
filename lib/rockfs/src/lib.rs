// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! RockFS version 1, PrettyBigRock's own filesystem, in Rust (docs/ROCKFS.md is the
//! specification; docs/design/0016-filesystems.md, sections 2 and 4, says what this library
//! does and how it is checked).
//!
//! A *filesystem* is a way of laying files and folders out on a disk. This library implements
//! [`pbr_fs::Filesystem`] over any [`pbr_fs::BlockDevice`] (a disk's sectors), so the same code
//! serves a real disk inside a filesystem server and a disk image in memory or in a file in
//! the host's tests and tools.
//!
//! What it promises, and where to read how:
//! - **A file is never half old and half new**, even if the power fails at any moment. Changes
//!   go to new blocks and take effect in one sector write; flushes are placed where the order
//!   of writes matters (`content.rs`).
//! - **Anything on the disk may be wrong**, and is checked before it is trusted (`tree.rs`,
//!   `folder.rs`, `superblock.rs`). A problem is reported as an error for that request, never
//!   as a crash, a loop or a wild memory access.
//! - **A volume left "in use" is checked and repaired before it is used again** (`check.rs`).
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, not the
//! standard library (which needs an operating system underneath), so it builds for the
//! filesystem server, which runs on PrettyBigRock, and for the host. Its tests use `std`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod bitmap;
mod check;
mod content;
mod dirs;
mod error;
mod extents;
mod folder;
mod format;
mod inode;
mod inodes;
mod layout;
mod names;
mod ops;
mod runs;
mod superblock;
mod tree;
mod volume;

#[cfg(test)]
mod tests;

pub use error::{Info, MountError, Problem};
pub use format::format;
pub use layout::{BLOCK, MIN_BLOCKS};
pub use names::{same_name, valid_name};
pub use volume::RockFs;
