// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Files and folders on one volume (docs/design/0014-files-and-file-trees.md, section 5).
/// Served by filesystem servers. A capability for a file or a folder is an endpoint capability
/// to the server whose badge names it; `open`, `create` and `reduce` answer with a capability
/// derived from the one the call came through, so revoking a folder revokes everything opened
/// through it (note 0013).
interface pbr.file version 1;

/// What a capability may do. Rights only ever shrink.
bits FileRights: u32 {
    /// Read a file; list a folder; open what's inside a folder for reading.
    Read @0,
    /// Change a file; create, remove and rename inside a folder; open inside it for writing.
    Write @1,
}

/// A file or a folder.
enum NodeKind { File @1, Folder @2 }

/// Facts about a file or folder, and this capability.
struct NodeInfo {
    kind: NodeKind,
    /// What this capability may do.
    rights: FileRights,
    /// Bytes, for a file; 0 for a folder.
    size: u64,
    /// Seconds since 1970 UTC; 0 if the filesystem doesn't record it.
    created: u64,
    modified: u64,
    hidden: bool,
    /// The volume can't be written (the medium, or a filesystem feature not understood).
    read_only: bool,
}

/// One entry in a folder.
struct Entry {
    name: string<1020>,
    kind: NodeKind,
    size: u64,
    modified: u64,
    hidden: bool,
}

/// The volume's size and free space, in bytes.
struct Space {
    total: u64,
    free: u64,
    label: string<64>,
    filesystem: string<16>,
}

/// Facts about it.
@1 fn info() -> (info: NodeInfo) error FileError;

/// A copy of this capability with fewer rights, derived from it (revoked with it).
@2 fn reduce(rights: FileRights) -> (node: handle<pbr.file>) error FileError;

/// The volume's size and free space.
@3 fn space() -> (space: Space) error FileError;

/// Up to `max` bytes from `offset` (fewer at the end of the file; none past it).
@10 fn read(offset: u64, max: u32) -> (data: bytes<65536>) error FileError;

/// Writes `data` at `offset`, growing the file if needed. Visible at once; durable at `sync`
/// or when the last capability through which the file was changed has gone (note 0014,
/// section 6).
@11 fn write(offset: u64, data: bytes<65536>) -> (written: u32) error FileError;

/// Cuts the file to `size` bytes, or adds zeros up to it.
@12 fn set_size(size: u64) -> () error FileError;

/// Makes this file's changes durable; returns when they are on the disk. On RockFS they take
/// effect together: after a crash the file is entirely as before or entirely as after.
@13 fn sync() -> () error FileError;

/// Opens what `path` (relative to this folder: names separated by `/`, no `.` or `..`) names,
/// with `rights` (no more than this folder's).
@20 fn open(path: string<4096>, rights: FileRights) -> (node: handle<pbr.file>, info: NodeInfo)
    error FileError;

/// Creates the last name in `path` as a file or folder (the folders before it must exist), and
/// opens it with `rights`.
@21 fn create(path: string<4096>, kind: NodeKind, rights: FileRights)
    -> (node: handle<pbr.file>, info: NodeInfo) error FileError;

/// Up to `max` entries from `cursor` (0 to start). `next` is 0 at the end.
@22 fn list(cursor: u64, max: u32) -> (entries: list<Entry, 56>, next: u64) error FileError;

/// Removes a file, or an empty folder. Capabilities for it fail with `Gone` from then on.
@23 fn remove(path: string<4096>) -> () error FileError;

/// Renames or moves within this folder's tree. With `replace`, an existing file at `to` is
/// replaced in one step.
@24 fn rename(from: string<4096>, to: string<4096>, replace: bool) -> () error FileError;

/// What can go wrong.
flexible enum FileError {
    NotFound @1,
    AlreadyExists @2,
    NotAFolder @3,
    NotAFile @4,
    /// This capability doesn't have the right.
    NoRight @5,
    /// A name the volume can't store.
    BadName @6,
    /// A path that breaks the rules (empty names, `.`, `..`, a leading `/`, too long).
    BadPath @7,
    FolderNotEmpty @8,
    /// There isn't enough free space to save this safely.
    NoSpace @9,
    /// The volume can't be written.
    ReadOnly @10,
    TooBig @11,
    /// It has been removed.
    Gone @12,
    /// The volume's structures are damaged here. Nothing was changed.
    Damaged @13,
    DiskError @14,
    /// The server holds as many open files and folders as it can.
    TooManyOpen @15,
    BadArgument @16,
}
