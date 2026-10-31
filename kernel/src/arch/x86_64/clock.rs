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
        let t0 = rdtsc();
        wait_reference_ns(20_000_000);
        let freq = (rdtsc() - t0) * 50;
        SOURCE.store(SOURCE_TSC, Ordering::Relaxed);
        freq
    } else {
        SOURCE.store(SOURCE_HPET, Ordering::Relaxed);
        1_000_000_000_000_000 / HPET_PERIOD.load(Ordering::Relaxed)
    }
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
