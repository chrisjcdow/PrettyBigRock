// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the program self-test's programs (the root program, `echo` and `probe`) agree on: the
//! echo service's operations, and the start-up roles they give each other.

#![no_std]

/// The echo service's operations (message labels).
pub mod op {
    /// Replies with the same data, the attached buffer reversed, and every capability that
    /// came moved straight back.
    pub const ECHO: u32 = 1;
    /// Replies with the caller's badge (8 bytes).
    pub const BADGE: u32 = 2;
    /// Crashes the service (a write to an unmapped address).
    pub const CRASH: u32 = 3;
    /// Replies with how many calls the service has answered (8 bytes).
    pub const COUNT: u32 = 4;
    /// Replies with the rights each capability arrived with (4 bytes each), and closes them.
    pub const RIGHTS: u32 = 5;
}

/// Start-up roles (note 0009, section 4: 1000 and up are for programs to agree).
pub mod role {
    /// `echo`: the endpoint to serve. `probe`: an endpoint to the echo service.
    pub const ENDPOINT: u32 = 1000;
    /// `probe`: a second endpoint to the echo service, with another badge.
    pub const ENDPOINT2: u32 = 1001;
    /// `probe`: a notification to tell the root program it's ready.
    pub const READY: u32 = 1002;
    /// `probe`: a memory object.
    pub const MEMORY: u32 = 1003;
    /// `probe`: a memory object it may read but not map.
    pub const MEMORY_NO_MAP: u32 = 1004;
    /// `probe`: an endpoint capability without the send right.
    pub const NO_SEND: u32 = 1005;
}

/// Exit statuses: 0 means the probe's checks passed; anything else is the number of the
/// first check that failed.
pub const PASSED: i32 = 0;
