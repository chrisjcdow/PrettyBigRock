// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The kernel's clock: nanoseconds since start-up (docs/design/0005-kernel-core.md, section 6).
//! The architecture layer supplies a counter that rises at a steady rate, and its frequency.

use crate::arch;
use core::sync::atomic::{AtomicU64, Ordering};

/// How often each core's timer interrupt ticks.
pub const TICK_HZ: u64 = 1000;

static FREQ: AtomicU64 = AtomicU64::new(0);
static START: AtomicU64 = AtomicU64::new(0);

/// Starts the clock, once the architecture layer knows its counter's frequency.
pub fn init(freq: u64) {
    FREQ.store(freq, Ordering::Relaxed);
    START.store(arch::clock_counter(), Ordering::Relaxed);
}

/// Nanoseconds since the clock started.
pub fn now_ns() -> u64 {
    let f = FREQ.load(Ordering::Relaxed);
    if f == 0 {
        return 0;
    }
    let ticks = arch::clock_counter().wrapping_sub(START.load(Ordering::Relaxed));
    (u128::from(ticks) * 1_000_000_000 / u128::from(f)) as u64
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] // x86-64 core start-up uses it
/// Waits, spinning, for `ns` nanoseconds. For start-up code that runs before the scheduler.
pub fn spin_ns(ns: u64) {
    let end = now_ns() + ns;
    while now_ns() < end {
        core::hint::spin_loop();
    }
}
