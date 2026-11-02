// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Errors and reports that are specific to RockFS: why a volume would not mount, what a check
//! found, and what `info` says about a mounted volume.

use alloc::string::String;
use core::fmt;

/// Why a volume could not be mounted.
///
/// (`enum` is Rust's "one of these": a value of this type is exactly one of the variants, and
/// the compiler makes every `match` on it handle all of them.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountError {
    /// The disk holds no RockFS: neither the first nor the last block looks like a superblock.
    NotRockFs,
    /// It is RockFS, but a kind this code cannot use safely (the reason is in the text): a
    /// newer version, a different block size, or a feature the volume needs and this code lacks.
    Unsupported(&'static str),
    /// It is RockFS, but too damaged to use (the reason is in the text): neither superblock is
    /// good, or what a good one says does not fit the disk.
    Damaged(&'static str),
    /// The disk could not be read.
    Disk,
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MountError::NotRockFs => f.write_str("this is not a RockFS volume"),
            MountError::Unsupported(why) => write!(f, "this RockFS volume can't be used: {why}"),
            MountError::Damaged(why) => write!(f, "this RockFS volume is damaged: {why}"),
            MountError::Disk => f.write_str("the disk could not be read"),
        }
    }
}

/// One thing a check found (ROCKFS.md, "Checking a volume").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// What is wrong, in plain words.
    pub text: String,
    /// Whether it is only leftover space or an empty leftover item that a save interrupted by a
    /// power cut leaves behind (blocks marked used that belong to nothing, an item in no folder),
    /// or a bitmap that is behind because the volume was not shut down cleanly. None of these
    /// loses or damages anything.
    pub harmless: bool,
    /// Whether the repair fixed it (always `false` for a check without repair).
    pub fixed: bool,
}

/// Facts about a mounted volume, for the `info` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// The volume name.
    pub label: String,
    /// The volume's ID.
    pub uuid: [u8; 16],
    /// Blocks in the volume.
    pub block_count: u64,
    /// Free blocks (including the reserve).
    pub free_blocks: u64,
    /// Blocks kept back for deleting and housekeeping.
    pub reserve_blocks: u64,
    /// Inode slots, including the unused inode 0.
    pub inode_count: u32,
    /// Free inode slots.
    pub free_inodes: u32,
    /// How many times the volume has been mounted for writing, including this time if it is
    /// mounted for writing (a read-only mount does not count itself).
    pub mount_count: u32,
    /// When the volume was formatted (seconds since 1970 UTC).
    pub created: u64,
    /// When it was last mounted before this time, or 0 if never (a mount rewrites it, so this
    /// is the value read from the disk, not the new one).
    pub last_mounted: u64,
    /// Whether the last session ended cleanly (`state` was 0 when the volume was mounted).
    pub clean: bool,
    /// Whether the main superblock was damaged and the backup was used.
    pub used_backup: bool,
    /// Whether this mount allows no changes.
    pub read_only: bool,
    /// How names are compared (1 is the only rule that exists).
    pub name_rule: u16,
}
