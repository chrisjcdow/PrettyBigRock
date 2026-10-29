// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The GIC, version 2: the interrupt controller of the Pi 5 (GIC-400) and QEMU's `virt`
//! machine. GICv3 comes when a target needs it (docs/design/0005-kernel-core.md, section 5).

use crate::hw;
use crate::mem::vm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Software interrupt: "look at your run queue".
pub const SGI_RESCHEDULE: u32 = 0;
/// Software interrupt: "stop: the kernel has panicked".
pub const SGI_STOP: u32 = 1;

static DIST: AtomicU64 = AtomicU64::new(0);
static CPU: AtomicU64 = AtomicU64::new(0);
static VERSION: AtomicU8 = AtomicU8::new(0);

const GICD_CTLR: u64 = 0x000;
const GICD_ISENABLER: u64 = 0x100;
const GICD_IPRIORITYR: u64 = 0x400;
const GICD_SGIR: u64 = 0xF00;
const GICD_PIDR2: u64 = 0xFE8;
const GICC_CTLR: u64 = 0x000;
const GICC_PMR: u64 = 0x004;
const GICC_IAR: u64 = 0x00C;
const GICC_EOIR: u64 = 0x010;

fn d(reg: u64) -> *mut u32 {
    (DIST.load(Ordering::Relaxed) + reg) as *mut u32
}
fn c(reg: u64) -> *mut u32 {
    (CPU.load(Ordering::Relaxed) + reg) as *mut u32
}

/// Maps the GIC and turns on its distributor and this core's CPU interface.
pub fn init(m: &hw::Machine) {
    assert!(m.gicd != 0 && m.gicc != 0, "the firmware didn't describe the GIC");
    DIST.store(vm::map_device(m.gicd, 0x10000), Ordering::Relaxed);
    let version = if m.gic_version != 0 {
        m.gic_version
    } else {
        // SAFETY: the distributor's identification register.
        let pidr2 = unsafe { read_volatile(d(GICD_PIDR2)) };
        ((pidr2 >> 4) & 0xF) as u8
    };
    VERSION.store(version, Ordering::Relaxed);
    assert!(version <= 2, "GICv{version} isn't supported yet (only GICv2)");
    CPU.store(vm::map_device(m.gicc, 0x2000), Ordering::Relaxed);
    // SAFETY: the distributor's control register: forward interrupts to the CPU interfaces.
    unsafe { write_volatile(d(GICD_CTLR), 1) };
}

/// Lets interrupt `irq` (a per-core interrupt, such as the timer), and the software
/// interrupts, through to this core, and turns on its CPU interface.
pub fn enable_this_core(irq: u32) {
    // SAFETY: the banked (per-core) enable and priority registers for interrupts 0 to 31, and
    // this core's CPU interface.
    unsafe {
        write_volatile(d(GICD_ISENABLER), (1 << irq) | 0xFFFF);
        let prio = d(GICD_IPRIORITYR + u64::from(irq & !3));
        let shift = (irq % 4) * 8;
        write_volatile(prio, (read_volatile(prio) & !(0xFF << shift)) | (0x80 << shift));
        write_volatile(c(GICC_PMR), 0xF0);
        write_volatile(c(GICC_CTLR), 1);
    }
}

/// Takes the next interrupt: (the value to hand back to [`end`], its number), or `None` if
/// there wasn't one after all.
pub fn acknowledge() -> Option<(u32, u32)> {
    // SAFETY: reading the CPU interface's acknowledge register.
    let iar = unsafe { read_volatile(c(GICC_IAR)) };
    let id = iar & 0x3FF;
    (id < 1020).then_some((iar, id))
}

/// Signals the end of an interrupt.
pub fn end(iar: u32) {
    // SAFETY: the CPU interface's end-of-interrupt register.
    unsafe { write_volatile(c(GICC_EOIR), iar) };
}

/// Sends software interrupt `sgi` to the core whose CPU interface number is `interface`.
pub fn send_sgi(interface: u32, sgi: u32) {
    // SAFETY: the distributor's software-interrupt register: target list, then interrupt number.
    unsafe {
        core::arch::asm!("dsb ishst");
        write_volatile(d(GICD_SGIR), (1 << (16 + interface)) | sgi);
    }
}

/// The GIC version.
pub fn version() -> u8 {
    VERSION.load(Ordering::Relaxed)
}
