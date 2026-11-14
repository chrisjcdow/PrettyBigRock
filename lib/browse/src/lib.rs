// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Browsing folders: what the file picker (docs/design/0025-picker-clipboard-permissions.md,
//! section 1) and Files (docs/design/0026-apps-m6-tests-and-stages.md, section 1) share, as
//! plain logic tested on the host.
//!
//! - [`tree`]: the [`Folders`] trait, a file tree as browsing needs it, and [`MemTree`], one in
//!   memory for tests (the real one, over a program's own tree from `lib/files`, is built only
//!   for PrettyBigRock);
//! - [`item`]: one entry of a folder, and sorting them;
//! - [`filter`]: which files to show ("Pictures: bmp png"), always with "All files";
//! - [`places`]: Home, Documents, Pictures, Music, Videos, Downloads, and the volumes an
//!   administrator sees;
//! - [`kind`]: kinds of file, by their names, with their icons;
//! - [`path`]: joining, splitting and checking paths and names;
//! - [`format`]: sizes and dates as people read them.
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, so a
//! PrettyBigRock program can use it; its tests use the standard library.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(target_os = "prettybigrock")]
mod files;
pub mod filter;
pub mod format;
pub mod item;
pub mod kind;
pub mod path;
pub mod places;
pub mod tree;

pub use filter::Filter;
pub use item::{Item, Sort, SortBy};
pub use kind::Kind;
pub use places::{Place, PlaceKind};
pub use tree::{BrowseError, Folders, MemTree};
