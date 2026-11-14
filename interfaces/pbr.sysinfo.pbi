// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Read-only facts about the computer, for About and Terminal's `mem`, `disk` and `fetch`
/// (docs/design/0026-apps-m6-tests-and-stages.md, section 3). Served by the users service; a
/// session's sandbox manager gives a copy, at start-up role 1005, to the system apps whose
/// manifest asks for `sysinfo`. Nothing in it says anything about other apps or other users.
/// Uptime isn't here: every program can read the monotonic clock.
interface pbr.sysinfo version 1;

/// One screen (as `pbr.display.sessions` describes it; copied until interfaces can import each
/// other, note 0008).
struct ScreenSummary { connector: string<32>, width: u32, height: u32, scale: u32 }

/// One volume the computer has, and how full it is.
struct VolumeInfo {
    label: string<64>,
    /// "RockFS", "FAT32", "empty" or "unknown".
    filesystem: string<16>,
    /// Bytes.
    size: u64,
    /// Free bytes, if its filesystem is being served (otherwise 0).
    free: u64,
    read_only: bool,
    in_memory: bool,
    /// The volume the system started from (with `System/users.txt`).
    system: bool,
}

/// The system's name and version: "PrettyBigRock", "2.0.0-alpha.1".
@1 fn version() -> (name: string<64>, version: string<32>) error SysinfoError;
/// The computer's memory and how much of it is free, in bytes.
@2 fn memory() -> (total: u64, available: u64) error SysinfoError;
/// The screens, if the display server is running.
@3 fn screens() -> (screens: list<ScreenSummary, 16>) error SysinfoError;
/// The volumes.
@4 fn volumes() -> (volumes: list<VolumeInfo, 32>) error SysinfoError;

/// What can go wrong.
flexible enum SysinfoError {
    /// What it comes from isn't running yet (or any more): ask again later.
    NotReady @1,
}
