// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What a filesystem server's owner (the storage service; a test) may ask of it
/// (docs/design/0015-storage-and-the-block-cache.md, section 4). Served on the server's
/// endpoint under a badge only the owner holds; the operation numbers are far from
/// `pbr.file`'s.
interface pbr.fs.control version 1;

/// Saves everything, marks the volume clean, and stops serving it.
@100 fn unmount() -> () error ControlError;

/// How many records (open capabilities' badges) the server holds: for self-tests, to see that
/// records go when their capabilities do.
@101 fn records() -> (count: u32) error ControlError;

/// What can go wrong.
flexible enum ControlError { DiskError @1 }
