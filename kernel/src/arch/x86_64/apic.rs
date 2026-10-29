// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The local APIC: each core's interrupt controller and timer. Used in x2APIC mode (through
//! model-specific registers) when the processor has it, otherwise through its registers in
//! memory.

use super::{clock, cpu};
use crate::mem::vm;
use crate::time::TICK_HZ;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const IA32_APIC_BASE: u32 = 0x1B;
const TPR: u32 = 0x80;
const EOI: u32 = 0xB0;
const SVR: u32 = 0xF0;
const ICR_LOW: u32 = 0x300;
const ICR_HIGH: u32 = 0x310;
const LVT_TIMER: u32 = 0x320;
const TIMER_INITIAL: u32 = 0x380;
const TIMER_CURRENT: u32 = 0x390;
const TIMER_DIVIDE: u32 = 0x3E0;

/// 1 = x2APIC; otherwise the virtual address of the registers (0 = not set up).
static MODE: AtomicU64 = AtomicU64::new(0);
static PHYS: AtomicU64 = AtomicU64::new(0);
/// APIC timer ticks per timer interrupt (the same on every core).
static TIMER_COUNT: AtomicU32 = AtomicU32::new(0);

fn read(reg: u32) -> u32 {
    match MODE.load(Ordering::Relaxed) {
        1 => {
            // SAFETY: x2APIC registers are MSRs from 0x800.
            unsafe { cpu::rdmsr(0x800 + (reg >> 4)) as u32 }
        }
        // SAFETY: the registers are mapped there, uncached.
        base => unsafe { core::ptr::read_volatile((base + u64::from(reg)) as *const u32) },
    }
}

fn write(reg: u32, v: u32) {
    match MODE.load(Ordering::Relaxed) {
        1 => {
            // SAFETY: as for `read`.
            unsafe { cpu::wrmsr(0x800 + (reg >> 4), u64::from(v)) }
        }
        // SAFETY: as for `read`.
        base => unsafe { core::ptr::write_volatile((base + u64::from(reg)) as *mut u32, v) },
    }
}

fn has_x2apic() -> bool {
    core::arch::x86_64::__cpuid(1).ecx & (1 << 21) != 0
}

/// Turns on this core's local APIC (and, on the boot core, chooses the mode).
pub fn init(phys: u64) {
    // The legacy PIC: mask everything, so only the APIC delivers interrupts.
    // SAFETY: the 8259 PIC's data ports; masking is harmless even if there's no PIC.
    unsafe {
        core::arch::asm!("out 0x21, al", "out 0xA1, al", in("al") 0xFFu8, options(nomem, nostack));
    }
    if MODE.load(Ordering::Relaxed) == 0 {
        PHYS.store(phys, Ordering::Relaxed);
        let mode = if has_x2apic() { 1 } else { vm::map_device(phys, 0x1000) };
        MODE.store(mode, Ordering::Relaxed);
    }
    enable_this_core();
}

/// Turns on this core's local APIC, in the mode already chosen.
pub fn enable_this_core() {
    let x2 = MODE.load(Ordering::Relaxed) == 1;
    // SAFETY: IA32_APIC_BASE exists on every x86-64 processor; bit 11 enables the APIC and
    // bit 10 selects x2APIC mode.
    unsafe {
        let base = cpu::rdmsr(IA32_APIC_BASE);
        cpu::wrmsr(IA32_APIC_BASE, base | (1 << 11) | if x2 { 1 << 10 } else { 0 });
    }
    write(TPR, 0);
    write(SVR, 0x100 | u32::from(super::interrupts::SPURIOUS));
}

/// Signals the end of an interrupt.
pub fn eoi() {
    write(EOI, 0);
}

fn send_raw(dest: u32, low: u32) {
    if MODE.load(Ordering::Relaxed) == 1 {
        // SAFETY: the x2APIC interrupt command register is MSR 0x830, written in one go.
        unsafe { cpu::wrmsr(0x830, (u64::from(dest) << 32) | u64::from(low)) };
    } else {
        // Two writes: an interrupt in between that sends its own IPI would change the target.
        let irq = super::irq_save();
        write(ICR_HIGH, dest << 24);
        write(ICR_LOW, low);
        while read(ICR_LOW) & (1 << 12) != 0 {
            core::hint::spin_loop(); // wait for delivery
        }
        super::irq_restore(irq);
    }
}

/// Sends interrupt `vector` to the core with APIC ID `dest`.
pub fn send(dest: u32, vector: u32) {
    send_raw(dest, vector);
}

/// Sends an INIT signal (the first step in starting a core).
pub fn send_init(dest: u32) {
    send_raw(dest, 0x4500);
}

/// Sends a start-up signal: the core starts in real mode at `page << 12`.
pub fn send_startup(dest: u32, page: u32) {
    send_raw(dest, 0x4600 | page);
}

/// Starts this core's timer interrupt at [`TICK_HZ`], calibrating it first if this is the
/// first core.
pub fn start_timer() {
    write(TIMER_DIVIDE, 0b1011); // divide by 1
    if TIMER_COUNT.load(Ordering::Relaxed) == 0 {
        write(LVT_TIMER, 1 << 16); // masked, one-shot
        write(TIMER_INITIAL, u32::MAX);
        clock::wait_reference_ns(10_000_000);
        let elapsed = u32::MAX - read(TIMER_CURRENT);
        write(TIMER_INITIAL, 0);
        let per_tick = (u64::from(elapsed) * 100 / TICK_HZ).max(1000) as u32;
        TIMER_COUNT.store(per_tick, Ordering::Relaxed);
    }
    write(LVT_TIMER, u32::from(super::interrupts::TIMER) | (1 << 17)); // periodic
    write(TIMER_INITIAL, TIMER_COUNT.load(Ordering::Relaxed));
}

/// A short description for the start-up report.
pub fn describe() -> &'static str {
    if MODE.load(Ordering::Relaxed) == 1 { "x2APIC" } else { "xAPIC" }
}
