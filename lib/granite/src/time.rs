// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Time: the monotonic clock, and deadlines (docs/design/0007-system-calls.md, section 4).

use crate::sys;

/// A deadline meaning "don't wait".
pub const NOW: u64 = 0;
/// A deadline meaning "wait for ever".
pub const FOREVER: u64 = u64::MAX;

/// Nanoseconds since the system started.
pub fn now() -> u64 {
    sys::clock_now()
}

/// The deadline `ms` milliseconds from now.
pub fn after_ms(ms: u64) -> u64 {
    now().saturating_add(ms.saturating_mul(1_000_000))
}

/// The wall-clock time now, in nanoseconds since 1 January 1970 UTC, or `None` if the
/// machine didn't say (docs/design/0013-kernel-1.2.md, section 4). It's only as right as the
/// machine's battery clock; time zones are for programs to apply.
pub fn wall_now() -> Option<u64> {
    match sys::wall_clock_start() {
        0 => None,
        start => Some(start.saturating_add(now())),
    }
}

/// The wall-clock time now in whole seconds since 1970 UTC (what filesystems store), or 0 if
/// unknown.
pub fn wall_seconds() -> u64 {
    wall_now().map_or(0, |ns| ns / 1_000_000_000)
}

/// Sleeps for at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    let _ = sys::call(sys::num::THREAD_SLEEP, [after_ms(ms), 0, 0, 0, 0, 0]);
}
