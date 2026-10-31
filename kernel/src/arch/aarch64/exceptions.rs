// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The exception vector table, and the handler every exception goes to.

use super::{gic, timer};
use crate::{kprintln, percpu, sched};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Breakpoints taken (the self-test checks this).
pub static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
/// The timer's interrupt number.
static TIMER_IRQ: AtomicU32 = AtomicU32::new(27);

// Sixteen entries of 128 bytes. Each saves x0 and x1, puts its own number in x1, and jumps to
// the common code, which saves everything else into a 288-byte frame on the current stack.
//
// Entries for exceptions taken in the kernel (4 to 7) first check the kernel stack hasn't
// overflowed. Every kernel stack sits at the top of a 256 KiB slot aligned to its size, with
// the bottom half never mapped (crate::mem::vm::STACK_SLOT), so a stack pointer with bit 17
// clear has run off its stack. The check uses no register: it parks x0 inside the stack
// pointer for a moment, as Linux does (docs/design/0007-system-calls.md, section 10).
core::arch::global_asm!(
    ".section .text.pbr_vectors, \"ax\"",
    ".balign 2048",
    ".global pbr_vectors",
    "pbr_vectors:",
    ".set kind, 0",
    ".rept 16",
    ".balign 128",
    "sub sp, sp, #288",
    ".if (kind >= 4) && (kind <= 7)",
    "add sp, sp, x0",
    "sub x0, sp, x0",
    "tbnz x0, #17, 1f",
    "sub x0, sp, x0",
    "sub sp, sp, x0",
    "b pbr_stack_overflow",
    "1:",
    "sub x0, sp, x0",
    "sub sp, sp, x0",
    ".endif",
    "stp x0, x1, [sp, #0]",
    "mov x1, #kind",
    "b pbr_exception_common",
    ".set kind, kind + 1",
    ".endr",
    "pbr_exception_common:",
    "stp x2, x3, [sp, #16]",
    "stp x4, x5, [sp, #32]",
    "stp x6, x7, [sp, #48]",
    "stp x8, x9, [sp, #64]",
    "stp x10, x11, [sp, #80]",
    "stp x12, x13, [sp, #96]",
    "stp x14, x15, [sp, #112]",
    "stp x16, x17, [sp, #128]",
    "stp x18, x19, [sp, #144]",
    "stp x20, x21, [sp, #160]",
    "stp x22, x23, [sp, #176]",
    "stp x24, x25, [sp, #192]",
    "stp x26, x27, [sp, #208]",
    "stp x28, x29, [sp, #224]",
    "str x30, [sp, #240]",
    "mrs x10, elr_el1",
    "mrs x11, spsr_el1",
    "stp x10, x11, [sp, #248]",
    "mrs x12, esr_el1",
    "mrs x13, far_el1",
    "stp x12, x13, [sp, #264]",
    "mrs x14, sp_el0",
    "str x14, [sp, #280]",
    "mov x0, sp",
    "bl {handler}",
    // Where every return from an exception goes, and where a program first starts.
    ".global pbr_exception_exit",
    "pbr_exception_exit:",
    "ldr x10, [sp, #280]",
    "msr sp_el0, x10",
    "ldp x10, x11, [sp, #248]",
    "msr elr_el1, x10",
    "msr spsr_el1, x11",
    "ldp x2, x3, [sp, #16]",
    "ldp x4, x5, [sp, #32]",
    "ldp x6, x7, [sp, #48]",
    "ldp x8, x9, [sp, #64]",
    "ldp x10, x11, [sp, #80]",
    "ldp x12, x13, [sp, #96]",
    "ldp x14, x15, [sp, #112]",
    "ldp x16, x17, [sp, #128]",
    "ldp x18, x19, [sp, #144]",
    "ldp x20, x21, [sp, #160]",
    "ldp x22, x23, [sp, #176]",
    "ldp x24, x25, [sp, #192]",
    "ldp x26, x27, [sp, #208]",
    "ldp x28, x29, [sp, #224]",
    "ldr x30, [sp, #240]",
    "ldp x0, x1, [sp, #0]",
    "add sp, sp, #288",
    "eret",
    // The kernel stack overflowed: carry on on this core's emergency stack, and report it.
    // TPIDRRO_EL0 holds x0 for a moment (programs only ever see it as 0, and this is fatal).
    "pbr_stack_overflow:",
    "msr tpidrro_el0, x0",
    "mrs x0, tpidr_el1",
    "ldr x0, [x0, #{emergency}]",
    "mov sp, x0",
    "mrs x0, tpidrro_el0",
    "sub sp, sp, #288",
    "stp x0, x1, [sp, #0]",
    "mov x1, #99",
    "b pbr_exception_common",
    handler = sym handle,
    emergency = const crate::percpu::OFFSET_EMERGENCY_SP,
);

unsafe extern "C" {
    static pbr_vectors: u8;
}

/// What the vector code saves.
#[repr(C)]
pub struct Frame {
    x: [u64; 31],
    elr: u64,
    spsr: u64,
    esr: u64,
    far: u64,
    /// The program's stack pointer, `SP_EL0`.
    sp_el0: u64,
}

impl Frame {
    /// A frame that starts a program at EL0: every register zero except `x0` to `x2`,
    /// interrupts unmasked.
    pub fn user(pc: u64, sp: u64, args: [u64; 3]) -> Self {
        let mut x = [0; 31];
        x[..3].copy_from_slice(&args);
        Frame { x, elr: pc, spsr: 0, esr: 0, far: 0, sp_el0: sp }
    }
}

/// Returns from an exception through `frame`, which is at the top of the current kernel
/// stack: how a program's thread first gets to EL0.
///
/// # Safety
/// `frame` must be a valid frame at the top of the current thread's kernel stack.
pub unsafe fn return_through(frame: *const Frame) -> ! {
    // SAFETY: as the caller promises.
    unsafe {
        core::arch::asm!("mov sp, {}", "b pbr_exception_exit", in(reg) frame, options(noreturn))
    }
}

/// Points this core's `VBAR_EL1` at the vector table.
pub fn install() {
    // SAFETY: the table is complete and 2 KiB-aligned.
    unsafe {
        core::arch::asm!("msr vbar_el1, {}", "isb", in(reg) &raw const pbr_vectors, options(nostack))
    };
}

/// Records which interrupt is the timer.
pub fn set_timer_irq(irq: u32) {
    TIMER_IRQ.store(irq, Ordering::Relaxed);
}

extern "C" fn handle(f: &mut Frame, kind: u64) {
    match kind {
        4 => sync(f), // synchronous, from EL1 using SP_EL1
        5 => {
            irq();
            sched::interrupt_exit();
        }
        8 => {
            // Synchronous, from a program: a system call or a fault.
            sync_user(f);
            super::user::returning_to_user();
        }
        9 => {
            irq();
            super::user::returning_to_user();
        }
        99 => fatal(f, kind, "kernel stack overflow"),
        _ => fatal(f, kind, "unexpected exception"),
    }
}

fn sync_user(f: &mut Frame) {
    use crate::obj::process::fault_kind as k;
    let ec = f.esr >> 26;
    if ec == 0x15 {
        // SVC: a system call. ELR already points past the SVC.
        super::irq_enable();
        let (status, r1, r2) =
            crate::syscall::handle(f.x[8], [f.x[0], f.x[1], f.x[2], f.x[3], f.x[4], f.x[5]]);
        f.x[0] = status;
        f.x[1] = r1;
        f.x[2] = r2;
        super::irq_disable();
        return;
    }
    let (kind, addr) = match ec {
        0x20 | 0x24 => (k::MEMORY, f.far),
        0x22 | 0x26 => (k::ALIGNMENT, f.far),
        0x00 | 0x0E | 0x18 | 0x01 => (k::INSTRUCTION, f.elr),
        0x2C => (k::ARITHMETIC, 0),
        0x3C => (k::BREAKPOINT, f.elr),
        _ => (k::OTHER, 0),
    };
    crate::user::fault(kind, addr, f.elr)
}

fn sync(f: &mut Frame) {
    let ec = f.esr >> 26;
    match ec {
        0x3C => {
            BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
            f.elr += 4; // carry on after the BRK
        }
        0x25 => fatal(f, 4, "data abort (page fault)"),
        0x21 => fatal(f, 4, "instruction abort (page fault)"),
        0x00 => fatal(f, 4, "undefined instruction"),
        0x26 => fatal(f, 4, "stack pointer misaligned"),
        0x22 => fatal(f, 4, "code address misaligned"),
        _ => fatal(f, 4, "synchronous exception"),
    }
}

fn irq() {
    while let Some((iar, id)) = gic::acknowledge() {
        if id == TIMER_IRQ.load(Ordering::Relaxed) {
            timer::rearm();
            gic::end(iar);
            sched::tick();
        } else if id == gic::SGI_RESCHEDULE {
            gic::end(iar);
            percpu::this().sched.need_resched.store(true, Ordering::Relaxed);
        } else if id == gic::SGI_STOP {
            super::halt_forever();
        } else {
            gic::end(iar);
        }
    }
}

fn fatal(f: &Frame, kind: u64, what: &str) -> ! {
    let core = percpu::try_this().map(|c| c.index).unwrap_or(0);
    kprintln!(
        "\nException on core {core}: {what} (vector {kind}, ESR {:#x}, class {:#x})",
        f.esr,
        f.esr >> 26
    );
    kprintln!("  elr {:#018x}  far {:#018x}  spsr {:#x}", f.elr, f.far, f.spsr);
    for i in (0..31).step_by(4) {
        kprintln!(
            "  x{:<2} {:#018x}  x{:<2} {:#018x}  x{:<2} {:#018x}  x{:<2} {:#018x}",
            i,
            f.x[i],
            i + 1,
            f.x.get(i + 1).copied().unwrap_or(0),
            i + 2,
            f.x.get(i + 2).copied().unwrap_or(0),
            i + 3,
            f.x.get(i + 3).copied().unwrap_or(0)
        );
    }
    panic!("{what} in the kernel");
}
