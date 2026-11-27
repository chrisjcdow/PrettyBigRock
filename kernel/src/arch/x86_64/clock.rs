// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The clock's counter: the TSC when it ticks at a constant rate, otherwise the HPET's main
//! counter. The HPET (or, without one, the PIT) is also the reference for calibrating the TSC
//! and the APIC timer.

use crate::mem::vm;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

const SOURCE_TSC: u8 = 1;
const SOURCE_HPET: u8 = 2;

static SOURCE: AtomicU8 = AtomicU8::new(0);
/// The HPET's registers (virtual address), or 0.
static HPET: AtomicU64 = AtomicU64::new(0);
/// Femtoseconds per HPET tick.
static HPET_PERIOD: AtomicU64 = AtomicU64::new(0);

fn hpet_read(reg: u64) -> u64 {
    // SAFETY: only called once HPET holds the registers' mapped address.
    unsafe { core::ptr::read_volatile((HPET.load(Ordering::Relaxed) + reg) as *const u64) }
}

fn rdtsc() -> u64 {
    // SAFETY: RDTSC only reads the time-stamp counter.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The TSC, read in order: the processor may otherwise read it early or late around the
/// memory accesses beside it, which the cores' comparison below must not allow.
fn rdtsc_ordered() -> u64 {
    // SAFETY: LFENCE only orders instructions.
    unsafe { core::arch::x86_64::_mm_lfence() };
    let t = rdtsc();
    // SAFETY: as above.
    unsafe { core::arch::x86_64::_mm_lfence() };
    t
}

/// Runs `measure` three times and returns the middle result (note 0027, section 5).
pub fn middle_of_three(mut measure: impl FnMut() -> u64) -> u64 {
    let mut m = [measure(), measure(), measure()];
    m.sort_unstable();
    m[1]
}

/// Waits `ns` nanoseconds by the reference clock (HPET or PIT), before the kernel clock runs.
pub fn wait_reference_ns(ns: u64) {
    if HPET.load(Ordering::Relaxed) != 0 {
        let ticks = ns * 1_000_000 / HPET_PERIOD.load(Ordering::Relaxed);
        let start = hpet_read(0xF0);
        while hpet_read(0xF0).wrapping_sub(start) < ticks {
            core::hint::spin_loop();
        }
    } else {
        pit_wait_ns(ns);
    }
}

/// The PIT's channel 2, in one-shot mode, polled through port 0x61. At most about 50 ms.
fn pit_wait_ns(ns: u64) {
    let count = (1_193_182 * ns / 1_000_000_000).clamp(1, 0xFFFF) as u16;
    // SAFETY: the PIT and the speaker-control port, as every PC has them; the speaker stays off.
    unsafe {
        let mut gate: u8;
        core::arch::asm!("in al, 0x61", out("al") gate, options(nomem, nostack));
        gate = (gate & !0x02) | 0x01; // channel 2's gate on, speaker off
        core::arch::asm!("out 0x61, al", in("al") gate, options(nomem, nostack));
        core::arch::asm!("out 0x43, al", in("al") 0b1011_0000u8, options(nomem, nostack)); // ch 2, mode 0
        core::arch::asm!("out 0x42, al", in("al") (count & 0xFF) as u8, options(nomem, nostack));
        core::arch::asm!("out 0x42, al", in("al") (count >> 8) as u8, options(nomem, nostack));
        loop {
            let s: u8;
            core::arch::asm!("in al, 0x61", out("al") s, options(nomem, nostack));
            if s & 0x20 != 0 {
                break;
            }
        }
    }
}

/// Chooses the clock source and returns its frequency in hertz.
pub fn init(hpet_phys: Option<u64>) -> u64 {
    if let Some(p) = hpet_phys {
        let v = vm::map_device(p, 0x400);
        HPET.store(v, Ordering::Relaxed);
        let caps = hpet_read(0);
        HPET_PERIOD.store(caps >> 32, Ordering::Relaxed);
        // SAFETY: the general configuration register; bit 0 starts the main counter.
        unsafe { core::ptr::write_volatile((v + 0x10) as *mut u64, hpet_read(0x10) | 1) };
        let wide = caps & (1 << 13) != 0;
        if !wide || HPET_PERIOD.load(Ordering::Relaxed) == 0 {
            HPET.store(0, Ordering::Relaxed); // a 32-bit HPET wraps too soon to be the clock
        }
    }
    let invariant = core::arch::x86_64::__cpuid(0x8000_0000).eax >= 0x8000_0007
        && core::arch::x86_64::__cpuid(0x8000_0007).edx & (1 << 8) != 0;
    if invariant || HPET.load(Ordering::Relaxed) == 0 {
        let freq = middle_of_three(|| {
            let t0 = rdtsc();
            wait_reference_ns(10_000_000);
            rdtsc() - t0
        }) * 100;
        SOURCE.store(SOURCE_TSC, Ordering::Relaxed);
        freq
    } else {
        SOURCE.store(SOURCE_HPET, Ordering::Relaxed);
        1_000_000_000_000_000 / HPET_PERIOD.load(Ordering::Relaxed)
    }
}

/// Every core's TSC must agree for the TSC to be the clock, since programs read it on whatever
/// core they run (note 0027, section 5). Each new core checks its own against the boot core's
/// with a handshake: the boot core reads its TSC, signals, and reads it again when the new
/// core answers with its own reading. A TSC that agrees reads between the two; one that
/// doesn't reads outside them, by more than the handshake took.
static PING: AtomicU64 = AtomicU64::new(0);
static PONG: AtomicU64 = AtomicU64::new(0);
static THEIR_TSC: AtomicU64 = AtomicU64::new(0);
/// The number of rounds of the handshake.
const ROUNDS: u64 = 5;

/// The boot core's half of the handshake, with a core that has just started. True if that
/// core's TSC agrees (or the TSC isn't the clock, or the core didn't answer in time).
pub fn tsc_agrees_with_new_core() -> bool {
    if !is_tsc() {
        return true;
    }
    // Rounds are numbered 1 to ROUNDS in the low byte, above a count of cores in the rest, so
    // no answer left over from one core is mistaken for the next core's.
    let base = PING.load(Ordering::Relaxed) & !0xFF;
    let mut agrees = true;
    for round in base + 1..=base + ROUNDS {
        let before = rdtsc_ordered();
        PING.store(round, Ordering::Release);
        let deadline = crate::time::now_ns() + 100_000_000;
        while PONG.load(Ordering::Acquire) != round && crate::time::now_ns() < deadline {
            core::hint::spin_loop();
        }
        if PONG.load(Ordering::Acquire) != round {
            break; // no answer: nothing to compare
        }
        let theirs = THEIR_TSC.load(Ordering::Relaxed);
        let after = rdtsc_ordered();
        agrees &= before <= theirs && theirs <= after;
    }
    // Round 0 of the next count means "no round yet".
    PING.store(base + 0x100, Ordering::Release);
    agrees
}

/// A new core's half of the handshake, before it does anything else that takes time.
pub fn tsc_answer_boot_core() {
    if !is_tsc() {
        return;
    }
    let mut last = PONG.load(Ordering::Relaxed);
    for _ in 0..ROUNDS {
        let deadline = rdtsc().wrapping_add(1 << 32); // a second or so
        let round = loop {
            let r = PING.load(Ordering::Acquire);
            if r != last && r & 0xFF != 0 {
                break r;
            }
            if rdtsc() > deadline {
                return;
            }
            core::hint::spin_loop();
        };
        THEIR_TSC.store(rdtsc_ordered(), Ordering::Relaxed);
        PONG.store(round, Ordering::Release);
        last = round;
    }
}

/// Stops using the TSC as the clock, if the HPET can take over, because the cores' TSCs
/// disagree. Returns the HPET's frequency, or `None` if there is no HPET to fall back on.
pub fn fall_back_to_hpet() -> Option<u64> {
    if HPET.load(Ordering::Relaxed) == 0 {
        return None;
    }
    SOURCE.store(SOURCE_HPET, Ordering::Relaxed);
    Some(1_000_000_000_000_000 / HPET_PERIOD.load(Ordering::Relaxed))
}

/// True if the clock is the TSC, which programs can read themselves.
pub fn is_tsc() -> bool {
    SOURCE.load(Ordering::Relaxed) == SOURCE_TSC
}

/// The clock's counter.
pub fn counter() -> u64 {
    match SOURCE.load(Ordering::Relaxed) {
        SOURCE_HPET => hpet_read(0xF0),
        _ => rdtsc(),
    }
}

/// A short description for the start-up report.
pub fn describe() -> &'static str {
    match SOURCE.load(Ordering::Relaxed) {
        SOURCE_HPET => "HPET",
        SOURCE_TSC if HPET.load(Ordering::Relaxed) != 0 => "TSC (calibrated by the HPET)",
        SOURCE_TSC => "TSC (calibrated by the PIT)",
        _ => "none",
    }
}
