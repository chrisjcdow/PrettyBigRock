// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The half of every filesystem server that deals with capabilities
//! (docs/design/0014-files-and-file-trees.md, section 7), written once so its rules can't
//! differ between filesystems:
//! - [`core`]: the object table, rights, paths, and saving a file when its last writer goes,
//!   as plain logic tested on the host;
//! - [`serve`]: the program's main loop, turning `pbr.file` requests into calls on the core;
//! - [`memfs`]: a filesystem in memory, for tests.

#![no_std]

extern crate alloc;

pub mod core;
pub mod memfs;
pub mod serve;

#[cfg(test)]
mod tests;
