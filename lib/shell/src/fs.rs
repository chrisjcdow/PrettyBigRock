// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the shell needs from the file tree, as a trait.
//!
//! A *trait* is Rust's way of naming a set of abilities: any type that implements [`Fs`] can
//! be used wherever the shell wants "something I can stat, list, read and write files on".
//! The real program implements it on its file tree (`pbr_files::Files`, note 0014, section 8);
//! the tests implement it on a few lines of in-memory folders. That is what lets everything the
//! shell does to files (recursive copies and removals, moves between volumes, `cat`'s limits)
//! be tested on the host.
//!
//! Every path given to an [`Fs`] is absolute and tidy (see `path`), and names a real thing:
//! the folders that exist only because something is attached below them are handled before
//! the trait is asked (see `layout`). The one exception is [`Fs::space`], which is asked about
//! the top of each attached volume.

use crate::reason::Reason;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_file::{self as file, NodeInfo};

/// One thing in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its name (no `/`).
    pub name: String,
    /// A folder, not a file.
    pub folder: bool,
    /// Bytes, for a file; 0 for a folder.
    pub size: u64,
}

/// Facts about one file or folder (what `pbr.file`'s `info` says).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    /// A folder, not a file.
    pub folder: bool,
    /// Bytes, for a file.
    pub size: u64,
    /// The shell may read it (or list it).
    pub readable: bool,
    /// The shell may change it.
    pub writable: bool,
    /// Seconds since 1970 UTC, or 0 if the filesystem doesn't record it.
    pub created: u64,
    /// Seconds since 1970 UTC, or 0.
    pub modified: u64,
    /// The filesystem marks it hidden.
    pub hidden: bool,
    /// The volume can't be written at all.
    pub read_only: bool,
}

impl Stat {
    /// What a folder that only exists to hold other folders looks like: readable, nothing more.
    pub fn virtual_folder() -> Stat {
        Stat {
            folder: true,
            size: 0,
            readable: true,
            writable: false,
            created: 0,
            modified: 0,
            hidden: false,
            read_only: true,
        }
    }
}

impl From<&NodeInfo> for Stat {
    fn from(i: &NodeInfo) -> Stat {
        Stat {
            folder: i.kind == file::NodeKind::Folder,
            size: i.size,
            readable: i.rights.contains(file::FileRights::READ),
            writable: i.rights.contains(file::FileRights::WRITE),
            created: i.created,
            modified: i.modified,
            hidden: i.hidden,
            read_only: i.read_only,
        }
    }
}

/// A volume's size and free space, in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    /// Everything the volume can hold.
    pub total: u64,
    /// What is still free.
    pub free: u64,
    /// Its label (may be empty).
    pub label: String,
    /// The filesystem's name, such as `RockFS`.
    pub filesystem: String,
}

impl From<&file::Space> for Space {
    fn from(s: &file::Space) -> Space {
        Space {
            total: s.total,
            free: s.free,
            label: s.label.clone(),
            filesystem: s.filesystem.clone(),
        }
    }
}

/// The file operations the shell uses.
pub trait Fs {
    /// Facts about `path`.
    fn stat(&self, path: &str) -> Result<Stat, Reason>;
    /// Everything in the folder `path`, in any order.
    fn list(&self, path: &str) -> Result<Vec<Item>, Reason>;
    /// Up to `max` bytes of the file from `offset`; fewer only at the end of the file.
    fn read_at(&self, path: &str, offset: u64, max: u32) -> Result<Vec<u8>, Reason>;
    /// Makes the file `path` (if it isn't there) hold exactly `data`, and makes that durable.
    fn replace(&self, path: &str, data: &[u8]) -> Result<(), Reason>;
    /// Adds `data` at the end of the file `path`, making it if it isn't there, and makes that
    /// durable.
    fn append(&self, path: &str, data: &[u8]) -> Result<(), Reason>;
    /// Makes a folder. Its parent must exist.
    fn mkdir(&self, path: &str) -> Result<(), Reason>;
    /// Removes a file or an empty folder.
    fn remove(&self, path: &str) -> Result<(), Reason>;
    /// Renames or moves `from` to `to`, which must be in the same attached volume or folder
    /// as `from` (the shell copies between them instead). Never replaces anything.
    fn rename(&self, from: &str, to: &str) -> Result<(), Reason>;
    /// The size and free space of the volume `path` is on.
    fn space(&self, path: &str) -> Result<Space, Reason>;
}
