// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Random numbers for placing things in programs' address spaces (address space layout
//! randomisation). Seeded from the loader's random bytes and the clock.
//!
//! This is not a cryptographic generator: that waits for the cryptography decision (DESIGN.md
//! Q5). It's good enough that programs' addresses differ from boot to boot.

use core::sync::atomic::{AtomicU64, Ordering};

static STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Mixes `seed` into the generator.
pub fn seed(seed: &[u8]) {
    let mut s = STATE.load(Ordering::Relaxed);
    for chunk in seed.chunks(8) {
        let mut b = [0u8; 8];
        b[..chunk.len()].copy_from_slice(chunk);
        s = mix(s ^ u64::from_le_bytes(b));
    }
    STATE.store(s ^ mix(crate::arch::clock_counter()), Ordering::Relaxed);
}

fn mix(mut z: u64) -> u64 {
    // SplitMix64's finaliser.
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The next random number.
pub fn next() -> u64 {
    let s = STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    mix(s.wrapping_add(0x9E37_79B9_7F4A_7C15) ^ crate::arch::clock_counter().rotate_left(32))
}
