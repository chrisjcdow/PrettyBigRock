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
/// Nanoseconds per counter tick, times 2^32.
static MULT: AtomicU64 = AtomicU64::new(0);

/// Starts the clock, once the architecture layer knows its counter's frequency.
pub fn init(freq: u64) {
    FREQ.store(freq, Ordering::Relaxed);
    MULT.store(((1_000_000_000u128 << 32) / u128::from(freq.max(1))) as u64, Ordering::Relaxed);
    START.store(arch::clock_counter(), Ordering::Relaxed);
}

/// Nanoseconds since the clock started.
///
/// It's `(ticks × mult) >> 32` rather than a division, because programs compute exactly the
/// same thing in the system page without entering the kernel (note 0007, section 1), and a
/// multiplication and a shift are the same on every processor.
pub fn now_ns() -> u64 {
    let ticks = arch::clock_counter().wrapping_sub(START.load(Ordering::Relaxed));
    ((u128::from(ticks) * u128::from(MULT.load(Ordering::Relaxed))) >> 32) as u64
}

/// What the system page needs to compute [`now_ns`] itself: (counter at start, multiplier).
pub fn parameters() -> (u64, u64) {
    (START.load(Ordering::Relaxed), MULT.load(Ordering::Relaxed))
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] // x86-64 core start-up uses it
/// Waits, spinning, for `ns` nanoseconds. For start-up code that runs before the scheduler.
pub fn spin_ns(ns: u64) {
    let end = now_ns() + ns;
    while now_ns() < end {
        core::hint::spin_loop();
    }
}
