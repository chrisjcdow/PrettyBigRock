// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Files and folders on a volume (docs/design/0014-files-and-file-trees.md, section 5). Served
/// by filesystem servers. A capability for a file or folder is an endpoint capability to its
/// server; everything opened through it is derived from it, so revoking it revokes them too.
/// Paths are relative to the folder called: names separated by `/`, never `.` or `..`.
interface pbr.file version 1;

/// What a capability may do.
bits FileRights: u32 {
    /// Read a file; list a folder, and open what's inside it for reading.
    Read @0,
    /// Change a file; create, remove and rename inside a folder, and open for writing.
    Write @1,
}

/// File or folder.
enum NodeKind { File @1, Folder @2 }

/// Facts about a file or folder.
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
    /// The volume can't be written.
    read_only: bool,
}

/// One entry in a folder.
struct Entry { name: string<1020>, kind: NodeKind, size: u64, modified: u64, hidden: bool }

/// The volume's size and free space, in bytes.
struct Space { total: u64, free: u64, label: string<64>, filesystem: string<16> }

/// Facts about this file or folder.
@1 fn info() -> (info: NodeInfo) error FileError;
/// A copy of this capability with fewer rights, revoked with this one.
@2 fn reduce(rights: FileRights) -> (node: handle<pbr.file>) error FileError;
/// The volume's size and free space.
@3 fn space() -> (space: Space) error FileError;

/// Reads up to `max` bytes (at most 65536) from `offset`. Fewer only at the end of the file.
@10 fn read(offset: u64, max: u32) -> (data: bytes<65536>) error FileError;
/// Writes `data` at `offset`, growing the file if needed.
@11 fn write(offset: u64, data: bytes<65536>) -> (written: u32) error FileError;
/// Makes the file `size` bytes long.
@12 fn set_size(size: u64) -> () error FileError;
/// Makes this file's changes durable. Returns when they are on the disk.
@13 fn sync() -> () error FileError;

/// Opens what `path` names, with at most this capability's rights.
@20 fn open(path: string<4096>, rights: FileRights) -> (node: handle<pbr.file>, info: NodeInfo)
    error FileError;
/// Creates the last name in `path` (the folders before it must exist).
@21 fn create(path: string<4096>, kind: NodeKind, rights: FileRights)
    -> (node: handle<pbr.file>, info: NodeInfo) error FileError;
/// Up to `max` entries from `cursor` (0 to start); `next` is 0 at the end.
@22 fn list(cursor: u64, max: u32) -> (entries: list<Entry, 56>, next: u64) error FileError;
/// Removes a file, or an empty folder.
@23 fn remove(path: string<4096>) -> () error FileError;
/// Renames or moves, within this folder's tree. With `replace`, a file at `to` is replaced in
/// one step.
@24 fn rename(from: string<4096>, to: string<4096>, replace: bool) -> () error FileError;

/// What can go wrong.
flexible enum FileError {
    NotFound @1,
    AlreadyExists @2,
    NotAFolder @3,
    NotAFile @4,
    /// This capability doesn't have the right.
    NoRight @5,
    BadName @6,
    /// Empty names, `.`, `..`, a leading `/`, or too long.
    BadPath @7,
    FolderNotEmpty @8,
    NoSpace @9,
    /// The volume can't be written.
    ReadOnly @10,
    TooBig @11,
    /// It was removed.
    Gone @12,
    /// The volume's structures are damaged here.
    Damaged @13,
    DiskError @14,
    /// The server holds as many open files and folders as it can.
    TooManyOpen @15,
    BadArgument @16,
}
