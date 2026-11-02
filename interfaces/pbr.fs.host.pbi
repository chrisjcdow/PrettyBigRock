// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What a filesystem server says to whoever started it, normally the storage service
/// (docs/design/0015-storage-and-the-block-cache.md, section 4).
interface pbr.fs.host version 1;

/// The volume is mounted: its root folder, and the server's control capability.
@1 fn ready(root: handle<pbr.file>, control: handle<pbr.fs.control>, label: string<64>) -> ()
    error HostError;

/// The volume can't be served.
@2 fn failed(reason: FsFailure) -> () error HostError;

/// Why not.
flexible enum FsFailure {
    /// Its structures are damaged.
    Damaged @1,
    /// It has a feature this server doesn't know.
    Unsupported @2,
    /// The disk failed.
    Disk @3,
}

/// What can go wrong.
flexible enum HostError { BadState @1 }
