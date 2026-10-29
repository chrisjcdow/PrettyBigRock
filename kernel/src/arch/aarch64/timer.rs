// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The generic timer: its virtual counter is the clock, and its virtual timer is each core's
//! timer interrupt.

use super::gic;
use crate::time::TICK_HZ;

/// The counter's frequency in hertz.
pub fn frequency() -> u64 {
    let f: u64;
    // SAFETY: reading CNTFRQ_EL0 has no side effects.
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack)) };
    f
}

fn arm() {
    let ticks = frequency() / TICK_HZ;
    // SAFETY: sets this core's virtual timer to fire after `ticks`, and enables it unmasked.
    unsafe {
        core::arch::asm!("msr cntv_tval_el0, {}", "msr cntv_ctl_el0, {}", "isb", in(reg) ticks, in(reg) 1u64, options(nomem, nostack))
    };
}

/// Starts this core's timer interrupt at [`TICK_HZ`].
pub fn start(irq: u32) {
    gic::enable_this_core(irq);
    arm();
}

/// Sets the timer for the next tick (called from its interrupt).
pub fn rearm() {
    arm();
}
