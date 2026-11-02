// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What every filesystem implements, and what it runs on (docs/design/0016-filesystems.md,
//! section 1).
//!
//! A *filesystem* is a way of laying files and folders out on a disk: RockFS and FAT32 are two.
//! Each is a library implementing [`Filesystem`] over a [`BlockDevice`] (the disk's sectors).
//! The server half shared by all of them, `lib/fsserve`, deals with capabilities and talks
//! `pbr.file`; the filesystem only deals with the disk. In host tests a `BlockDevice` is a disk
//! image in memory, so a filesystem can be tested without PrettyBigRock running.
//!
//! These two traits are a contract between people writing different parts, so every method's
//! rules are spelled out below. (A *trait* is Rust's word for an interface: a list of methods a
//! type promises to have.)

#![no_std]

extern crate alloc;

use alloc::string::String;

/// Bytes in a sector.
pub const SECTOR: usize = 512;

/// A volume's sectors. In a filesystem server this is the storage service's `pbr.block` for
/// the volume; in tests, a disk image in memory.
///
/// Writes may reach the disk in any order until [`BlockDevice::flush`], which returns only
/// when everything written before it is on the disk (note 0015, section 5). A filesystem that
/// needs one write to land before another puts a flush between them.
pub trait BlockDevice {
    /// How many sectors the volume has.
    fn sectors(&self) -> u64;
    /// Whether writes are refused.
    fn read_only(&self) -> bool;
    /// Reads `buf.len() / SECTOR` sectors from `sector`. `buf.len()` is a multiple of
    /// [`SECTOR`] and at most 65536.
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError>;
    /// Writes whole sectors, at most 65536 bytes.
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError>;
    /// Returns once everything written so far is on the disk.
    fn flush(&mut self) -> Result<(), DeviceError>;
}

/// The volume couldn't be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceError;

/// A file or folder, as the filesystem names it while mounted.
///
/// Rule: a `NodeId` names one file or folder for as long as the filesystem is mounted, even if
/// it is renamed or moved. Once that file or folder is removed, its `NodeId` is never used for
/// anything else while mounted, and every method given it returns [`FsError::Gone`]. (RockFS
/// builds it from the inode number and the inode's generation, which changes on reuse.)
pub type NodeId = u64;

/// A file or a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Holds bytes.
    File,
    /// Holds named files and folders.
    Folder,
}

/// Facts about a file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    /// File or folder.
    pub kind: Kind,
    /// A file's size in bytes, including changes not yet committed; 0 for a folder.
    pub size: u64,
    /// When it was created and last changed, in seconds since 1970 UTC; 0 if not recorded.
    pub created: u64,
    /// See `created`.
    pub modified: u64,
    /// Marked hidden.
    pub hidden: bool,
}

/// One entry in a folder's listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, exactly as stored.
    pub name: String,
    /// File or folder.
    pub kind: Kind,
    /// As in [`Stat`].
    pub size: u64,
    /// As in [`Stat`].
    pub modified: u64,
    /// As in [`Stat`].
    pub hidden: bool,
}

/// The volume's size and free space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    /// Bytes in the volume.
    pub total: u64,
    /// Bytes free for ordinary saving (RockFS leaves out its reserve).
    pub free: u64,
    /// The volume's label.
    pub label: String,
    /// The filesystem's name: "RockFS", "FAT32".
    pub filesystem: &'static str,
}

/// What can go wrong. Every error leaves the volume as it was before the call, except
/// [`FsError::Disk`] and [`FsError::Damaged`], after which the filesystem may refuse further
/// writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    /// No such name in the folder.
    NotFound,
    /// The name is taken (and `replace` wasn't asked for, or can't apply).
    AlreadyExists,
    /// A folder was needed.
    NotAFolder,
    /// A file was needed.
    NotAFile,
    /// The name breaks this filesystem's rules.
    BadName,
    /// Only an empty folder can be removed.
    FolderNotEmpty,
    /// Not enough free space to do it safely.
    NoSpace,
    /// The volume can't be written (the medium, or a feature this code doesn't know).
    ReadOnly,
    /// Beyond what the filesystem can hold (FAT32's 4 GiB files).
    TooBig,
    /// The node was removed.
    Gone,
    /// A structure on the disk breaks the filesystem's rules. Nothing was changed.
    Damaged,
    /// The disk failed.
    Disk,
    /// Anything else the caller got wrong, such as moving a folder into itself.
    BadArgument,
}

impl From<DeviceError> for FsError {
    fn from(_: DeviceError) -> FsError {
        FsError::Disk
    }
}

/// A mounted filesystem. `lib/fsserve` calls these one at a time.
///
/// Durability (docs/design/0014-files-and-file-trees.md, section 6):
/// - a file's changes ([`write`](Filesystem::write), [`set_size`](Filesystem::set_size)) are
///   visible to later reads at once, and become durable together at
///   [`commit`](Filesystem::commit) or [`unmount`](Filesystem::unmount). On RockFS a crash
///   before then leaves the file exactly as it was at its last commit;
/// - folder changes ([`create`](Filesystem::create), [`remove`](Filesystem::remove),
///   [`rename`](Filesystem::rename)) are durable when the method returns.
///
/// Names given to these methods are single names (no `/`), already checked against the general
/// rules of note 0014 section 4 (UTF-8, 1 to 255 characters, no `/`, no control characters,
/// not `.` or `..`). Each filesystem refuses whatever else its rules forbid with
/// [`FsError::BadName`], and compares names by its own rule (RockFS: A-Z match a-z).
pub trait Filesystem {
    /// The root folder.
    fn root(&self) -> NodeId;
    /// Whether the volume is mounted read-only. Every change then fails with `ReadOnly`.
    fn read_only(&self) -> bool;
    /// The node called `name` in `folder`.
    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError>;
    /// Facts about `node`.
    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError>;
    /// Reads from `offset` into `buf` (at most 65536 bytes). Returns how many bytes were read:
    /// fewer than asked only at the end of the file, 0 at or past it.
    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    /// Writes `data` (at most 65536 bytes) at `offset`, growing the file if needed (a gap reads
    /// as zeros). Returns how many bytes were written: all of them, or an error.
    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError>;
    /// Makes the file `size` bytes long, cutting it or adding zeros.
    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError>;
    /// Makes the file's changes durable, all together. A file with no changes: nothing to do.
    fn commit(&mut self, node: NodeId) -> Result<(), FsError>;
    /// Lists `folder` from `cursor` (0 to start), calling `out` for each entry until it
    /// returns `false` (that entry wasn't taken). Returns the cursor to carry on from, or 0
    /// at the end. Cursors are opaque; a folder changed between calls may repeat or miss
    /// entries, but never invents one.
    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        out: &mut dyn FnMut(Entry) -> bool,
    ) -> Result<u64, FsError>;
    /// Creates an empty file or folder called `name` in `folder`.
    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError>;
    /// Removes the file or empty folder called `name` from `folder`. Its node is then
    /// [`FsError::Gone`].
    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError>;
    /// Renames or moves `from_name` in `from` to `to_name` in `to`, keeping its `NodeId`. With
    /// `replace`, an existing *file* at the destination is replaced in one step (its node is
    /// then `Gone`); a folder there is never replaced (`AlreadyExists`). Moving a folder into
    /// itself or anything inside it fails with `BadArgument`.
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
    /// Commits every file and marks the volume as shut down cleanly. Nothing is called after.
    fn unmount(&mut self) -> Result<(), FsError>;
}

/// A volume in memory, for tests of filesystems on the host: a `Vec` of sectors that can be
/// told to fail.
pub mod memdisk {
    use super::{BlockDevice, DeviceError, SECTOR};
    use alloc::vec;
    use alloc::vec::Vec;

    /// A disk image in memory.
    pub struct MemDisk {
        /// Its bytes.
        pub data: Vec<u8>,
        /// Refuse writes.
        pub read_only: bool,
        /// Fail every request once this many writes have been made (a "power cut"), if set.
        pub fail_after_writes: Option<usize>,
        /// Writes made so far.
        pub writes: usize,
        /// Flushes made so far.
        pub flushes: usize,
    }

    impl MemDisk {
        /// A zeroed disk of `sectors` sectors.
        pub fn new(sectors: u64) -> MemDisk {
            MemDisk::from(vec![0; sectors as usize * SECTOR])
        }
        /// A disk holding `data` (a whole number of sectors).
        pub fn from(data: Vec<u8>) -> MemDisk {
            MemDisk { data, read_only: false, fail_after_writes: None, writes: 0, flushes: 0 }
        }
        fn check(&self, sector: u64, len: usize) -> Result<usize, DeviceError> {
            let at = usize::try_from(sector).map_err(|_| DeviceError)?.checked_mul(SECTOR);
            let at = at.ok_or(DeviceError)?;
            if len == 0 || !len.is_multiple_of(SECTOR) || len > 65536 || at + len > self.data.len()
            {
                return Err(DeviceError);
            }
            if self.fail_after_writes.is_some_and(|n| self.writes >= n) {
                return Err(DeviceError);
            }
            Ok(at)
        }
    }

    impl BlockDevice for MemDisk {
        fn sectors(&self) -> u64 {
            (self.data.len() / SECTOR) as u64
        }
        fn read_only(&self) -> bool {
            self.read_only
        }
        fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
            let at = self.check(sector, buf.len())?;
            buf.copy_from_slice(&self.data[at..at + buf.len()]);
            Ok(())
        }
        fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
            if self.read_only {
                return Err(DeviceError);
            }
            let at = self.check(sector, data.len())?;
            self.data[at..at + data.len()].copy_from_slice(data);
            self.writes += 1;
            Ok(())
        }
        fn flush(&mut self) -> Result<(), DeviceError> {
            self.check(0, SECTOR)?;
            self.flushes += 1;
            Ok(())
        }
    }
}
