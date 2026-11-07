// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! FAT32 with long names, in Rust (docs/design/0016-filesystems.md, sections 3 and 5).
//!
//! FAT32 is the filesystem of USB sticks, SD cards, cameras and the EFI system partition, so
//! almost every other system can read it. This library implements [`pbr_fs::Filesystem`] over
//! any [`pbr_fs::BlockDevice`] (a disk's sectors), so the same code serves a real volume inside
//! the FAT server and a disk image in memory or in a file in the host's tests.
//!
//! What it promises, and where to read how:
//! - **Nothing on the disk is trusted.** Every number in the boot sector is checked against the
//!   others and the volume's size (`boot.rs`); a chain of clusters is followed with a limit, so
//!   a loop is an error and not a hang (`chain.rs`); names, dates and folder entries are
//!   validated before they reach anyone (`dirent.rs`, `names.rs`, `time.rs`). A problem is an
//!   error for that request, never a panic or a wild allocation.
//! - **Changes are written in the order that does least harm** if the power fails (`ops.rs`,
//!   `file.rs`), but FAT cannot promise as much as RockFS does: a crash can leave a file
//!   partly changed or space lost, which other systems' checking tools fix. The volume's
//!   "not cleanly unmounted" flag tells them to look.
//! - **FAT volumes are not checked or repaired here** (not in M5): a volume found unclean is
//!   used as it is, and the fact is reported by [`Info::clean_at_mount`].
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, not the
//! standard library (which needs an operating system underneath), so it builds for the
//! filesystem server, which runs on PrettyBigRock, and for the host. Its tests use `std`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod boot;
mod chain;
mod dirent;
mod error;
mod file;
mod folder;
mod mount;
mod names;
mod ops;
mod table;
mod time;
mod volume;

#[cfg(test)]
mod tests;

pub use error::{Info, MountError};
pub use mount::Fat;
pub use names::{same_name, valid_name};
