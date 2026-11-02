// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What filesystems and the file layer share (docs/design/0016-filesystems.md, section 1).
//!
//! A filesystem server is two halves: `lib/fsserve`, which deals with capabilities, rights and
//! the `pbr.file` protocol, and a filesystem library (RockFS, FAT32), which knows a disk
//! format and nothing about capabilities. They meet at the two traits here:
//! - [`BlockDevice`]: a volume's sectors, which the filesystem reads and writes. In a server
//!   it's the storage service's `pbr.block`; in tests, a disk image in memory or a file.
//! - [`Filesystem`]: what a mounted filesystem can do, which `lib/fsserve` calls.
//!
//! A *trait* in Rust describes what a type can do, like an interface in Java or C#; any type
//! that implements every function in it can be used wherever the trait is asked for.
//!
//! [`memfs`] is a small filesystem kept in memory, for testing `lib/fsserve` and the file
//! layer without a disk.

#![no_std]

extern crate alloc;

pub mod memfs;

use alloc::string::String;
use alloc::vec::Vec;

/// Bytes in a sector.
pub const SECTOR: usize = 512;

/// The disk failed a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceError;

/// A volume's sectors, numbered from 0.
pub trait BlockDevice {
    /// How many sectors the volume has.
    fn sectors(&self) -> u64;
    /// Whether writes are refused.
    fn read_only(&self) -> bool;
    /// Reads `buf.len() / SECTOR` whole sectors from `sector` (at most 64 KiB at a time).
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError>;
    /// Writes whole sectors (at most 64 KiB at a time). It may reach the disk later.
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError>;
    /// Returns once every write made before it is on the disk (note 0015, section 5, rule 3).
    fn flush(&mut self) -> Result<(), DeviceError>;
}

/// A filesystem's name for a file or folder. It stays the same while the filesystem is
/// mounted, and is never given to another file or folder in that time: a node that has been
/// removed is reported as [`FsError::Gone`], never mistaken for a newer one (RockFS puts the
/// inode's generation in it).
pub type NodeId = u64;

/// A file or a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A file: bytes.
    File,
    /// A folder: names of other nodes.
    Folder,
}

/// Facts about a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    /// File or folder.
    pub kind: Kind,
    /// Bytes, for a file; 0 for a folder.
    pub size: u64,
    /// When it was made, in seconds since 1970 UTC; 0 if the filesystem doesn't record it.
    pub created: u64,
    /// When its contents last changed, the same way.
    pub modified: u64,
    /// Not shown in ordinary listings.
    pub hidden: bool,
}

/// One entry in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, as stored.
    pub name: String,
    /// File or folder.
    pub kind: Kind,
    /// Bytes, for a file.
    pub size: u64,
    /// When its contents last changed (seconds since 1970 UTC, or 0).
    pub modified: u64,
    /// Not shown in ordinary listings.
    pub hidden: bool,
}

/// A volume's size and free space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    /// Its size in bytes.
    pub total: u64,
    /// Bytes free for ordinary saving.
    pub free: u64,
    /// Its name.
    pub label: String,
    /// "RockFS", "FAT32".
    pub filesystem: &'static str,
}

/// What can go wrong. `lib/fsserve` turns these into `pbr.file`'s `FileError`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    /// No such name.
    NotFound,
    /// The name is taken.
    AlreadyExists,
    /// A folder was needed.
    NotAFolder,
    /// A file was needed.
    NotAFile,
    /// A name this filesystem can't store.
    BadName,
    /// Only empty folders can be removed.
    FolderNotEmpty,
    /// There isn't enough free space to save this safely.
    NoSpace,
    /// The volume can't be written.
    ReadOnly,
    /// Bigger than this filesystem allows (FAT32's 4 GiB files).
    TooBig,
    /// The node has been removed.
    Gone,
    /// A structure on the disk breaks the filesystem's rules. Nothing was changed.
    Damaged,
    /// The disk failed.
    Disk,
    /// An argument made no sense (moving a folder into itself, say).
    BadArgument,
}

impl From<DeviceError> for FsError {
    fn from(_: DeviceError) -> FsError {
        FsError::Disk
    }
}

/// A mounted filesystem.
///
/// **Durability** (note 0014, section 6): changes made by `write` and `set_size` are visible
/// at once, and become durable together when `commit` is called for the node (`lib/fsserve`
/// calls it on `sync`, and when the last capability through which the node was changed has
/// gone) or at `unmount`. On RockFS a crash leaves the file exactly as of its last commit.
/// Folder changes (`create`, `remove`, `rename`) are durable when they return.
pub trait Filesystem {
    /// The root folder.
    fn root(&self) -> NodeId;
    /// Whether the volume can't be written.
    fn read_only(&self) -> bool;
    /// The node called `name` in `folder`, compared by this filesystem's rule.
    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError>;
    /// Facts about `node`.
    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError>;
    /// Reads from `offset` into `buf`; returns how many bytes (0 at or past the end).
    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    /// Writes `data` at `offset`, growing the file if needed; returns how many bytes.
    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError>;
    /// Makes the file `size` bytes long, cutting it or adding zeros.
    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError>;
    /// Makes `node`'s changes durable. Nothing to do if there are none.
    fn commit(&mut self, node: NodeId) -> Result<(), FsError>;
    /// Up to `max` entries of `folder` from `cursor` (0 to start), and the cursor to continue
    /// from (0 when there are no more). Cursors are the filesystem's own.
    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        max: usize,
    ) -> Result<(Vec<Entry>, u64), FsError>;
    /// Makes an empty file or folder called `name` in `folder`.
    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError>;
    /// Removes the file, or empty folder, called `name` from `folder`.
    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError>;
    /// Moves `from_name` in `from` to `to_name` in `to`. With `replace`, an existing file there
    /// is replaced in one step; otherwise that's `AlreadyExists`. A folder can't be moved into
    /// itself or anything inside it (`BadArgument`).
    fn rename(
        &mut self,
        from: NodeId,
        from_name: &str,
        to: NodeId,
        to_name: &str,
        replace: bool,
    ) -> Result<(), FsError>;
    /// The volume's size and free space.
    fn space(&mut self) -> Result<Space, FsError>;
    /// Commits everything and marks the volume shut down cleanly.
    fn unmount(&mut self) -> Result<(), FsError>;
}

/// The rules every name must follow, whatever the filesystem (note 0014, section 4): UTF-8
/// (guaranteed by `&str`), 1 to 255 characters, no `/`, no control characters, not `.` or
/// `..`. Filesystems refuse more with `BadName`.
pub fn check_name(name: &str) -> Result<(), FsError> {
    let chars = name.chars().count();
    let ok = (1..=255).contains(&chars)
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || (c as u32) < 32 || c as u32 == 127);
    if ok { Ok(()) } else { Err(FsError::BadName) }
}
