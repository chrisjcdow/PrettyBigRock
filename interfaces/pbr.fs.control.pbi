// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What the storage service asks of a filesystem server, on the server's own endpoint under a
/// badge only the storage service holds (docs/design/0015-storage-and-the-block-cache.md,
/// section 4). The operation numbers are far from `pbr.file`'s.
interface pbr.fs.control version 1;

/// Save everything, mark the volume clean, and stop: every capability for it then fails.
@100 fn unmount() -> () error ControlError;

/// How many open files and folders the server is keeping records for (for tests and the
/// permissions viewer).
@101 fn records() -> (count: u32) error ControlError;

/// What can go wrong.
flexible enum ControlError {
    /// The volume couldn't be saved cleanly.
    Failed @1,
}
