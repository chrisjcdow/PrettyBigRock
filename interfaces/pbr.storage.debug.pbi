// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Testing the storage service (docs/design/0018-supervision-shell-and-m5-tests.md, section 7).
/// Served only when it was started with `selftest`, under its own badge. The operation numbers
/// are far from `pbr.storage`'s.
interface pbr.storage.debug version 1;

/// A `pbr.block` capability for one volume: what its filesystem server would be given.
@100 fn volume(id: u32) -> (block: handle<pbr.block>) error DebugError;

/// The cache: how many pages it holds, and how many of them are waiting to be written.
@101 fn cache() -> (pages: u32, dirty: u32) error DebugError;

/// Stops volume `id`'s filesystem server, as if it had crashed (the supervision test, note
/// 0018, section 7).
@102 fn stop_server(id: u32) -> () error DebugError;

/// Writes every dirty page in the cache to its disk, so that a test can crash this service
/// without losing what filesystems have already saved.
@103 fn flush() -> () error DebugError;

/// What can go wrong.
flexible enum DebugError {
    NoSuchVolume @1,
}
