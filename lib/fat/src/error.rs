// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Errors and reports that are specific to FAT32: why a volume would not mount, and what `info`
//! says about a mounted one.

use alloc::string::String;
use core::fmt;

/// Why a volume could not be mounted.
///
/// (`enum` is Rust's "one of these": a value of this type is exactly one of the variants, and
/// the compiler makes every `match` on it handle all of them.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountError {
    /// The disk holds no FAT: there is no boot sector signature, or the boot sector's numbers
    /// are nothing like a FAT volume's.
    NotFat,
    /// It is FAT (or exFAT), but a kind this code does not do (the reason is in the text):
    /// FAT12, FAT16, exFAT, a sector size other than 512, a newer version of the format.
    Unsupported(&'static str),
    /// It claims to be FAT32, but the boot sector contradicts itself or the size of the volume
    /// (the reason is in the text).
    Damaged(&'static str),
    /// The disk could not be read.
    Disk,
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MountError::NotFat => f.write_str("this is not a FAT volume"),
            MountError::Unsupported(why) => write!(f, "this FAT volume can't be used: {why}"),
            MountError::Damaged(why) => write!(f, "this FAT32 volume is damaged: {why}"),
            MountError::Disk => f.write_str("the disk could not be read"),
        }
    }
}

/// Facts about a mounted volume, for the `info` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// The volume's label (the root folder's label entry, else the boot sector's), or empty.
    pub label: String,
    /// The volume's serial number from the boot sector (0 if it has none).
    pub volume_id: u32,
    /// Bytes in a cluster (the unit in which files get space).
    pub cluster_bytes: u32,
    /// Clusters in the data area.
    pub cluster_count: u32,
    /// Free clusters.
    pub free_clusters: u32,
    /// How many copies of the FAT (the table of which cluster follows which) the volume keeps.
    pub fat_copies: u32,
    /// Whether the volume was cleanly unmounted by whoever used it last (bit 27 of the FAT's
    /// second entry). If not, it is used as it is: M5 neither checks nor repairs FAT volumes,
    /// so run a checker (`fsck.fat`, Windows' Check Disk) on it elsewhere.
    pub clean_at_mount: bool,
    /// Whether the free-space figures in the FSInfo sector were believed (they are only hints,
    /// and are recounted if they are impossible or the volume was not clean).
    pub used_fsinfo: bool,
    /// Whether this mount allows no changes.
    pub read_only: bool,
}
