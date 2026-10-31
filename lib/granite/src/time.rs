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

/// Sleeps for at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    let _ = sys::call(sys::num::THREAD_SLEEP, [after_ms(ms), 0, 0, 0, 0, 0]);
}
