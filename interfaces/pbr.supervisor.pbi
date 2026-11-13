// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What a service may ask of whoever started it
/// (docs/design/0018-supervision-shell-and-m5-tests.md, section 2). Each supervisor serves this
/// to its children at their start-up role 5, with a badge for each child (and each time it is
/// started), so it knows who is asking.
interface pbr.supervisor version 1;

/// An endpoint for a service this child may use: "devices", "storage", "console". After that
/// service is restarted, calls through the old capability fail with PEER_GONE, and the child
/// asks again.
@1 fn lookup(name: string<32>) -> (service: handle<any>) error SupervisorError;

/// Keep this memory alive until `release_kept` (for DMA memory: a device might still be
/// writing to it after the child has ended; section 4).
@2 fn keep(memory: handle<memory>) -> () error SupervisorError;

/// Release everything kept for this child's previous instances.
@3 fn release_kept() -> () error SupervisorError;

/// What can go wrong.
flexible enum SupervisorError {
    /// This child may not look that service up (or keep memory).
    NotAllowed @1,
    /// That service is starting or being restarted: ask again.
    NotReady @2,
    /// Too much kept already.
    TooMany @3,
    /// That service isn't running and won't be started again in this boot: it was never
    /// started (the console when the display server is, and the other way round), or it ended
    /// too often and was given up on (note 0024, section 8).
    Stopped @4,
}
