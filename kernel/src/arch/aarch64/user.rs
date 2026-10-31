// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Programs on AArch64: entering user mode (EL0), programs' floating-point and vector
//! registers, switching address spaces (`TTBR0_EL1`) and keeping every core's TLB right, the
//! protections that stop the kernel touching programs' memory, and the system page's code
//! (docs/design/0007-system-calls.md).
//!
//! Every program's pages are tagged with address-space number 0 and are never global, so
//! switching programs flushes only their translations, not the kernel's.

use super::exceptions::{self, Frame};
use crate::{percpu, sched};
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// AArch64 gives programs a page-table tree of their own (`TTBR0_EL1`).
pub const SHARES_TOP_LEVEL: bool = false;
/// The ELF machine number programs must have.
pub const ELF_MACHINE: u16 = 183;

static EMPTY_ROOT: AtomicU64 = AtomicU64::new(0);
static PAN: AtomicBool = AtomicBool::new(false);

/// Sets up this core for programs. Called on every core, after its exception vectors.
pub fn init_this_core() {
    if EMPTY_ROOT.load(Ordering::Acquire) == 0 {
        let p = crate::mem::pmm::alloc_page().expect("out of memory for page tables");
        let _ = EMPTY_ROOT.compare_exchange(0, p, Ordering::AcqRel, Ordering::Acquire);
    }
    let emergency = crate::mem::vm::alloc_stack(4).expect("out of memory for an emergency stack");
    percpu::this().emergency_sp.store(emergency, Ordering::Relaxed);
    let mmfr1: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64MMFR1_EL1", out(reg) mmfr1, options(nomem, nostack))
    };
    let has_pan = (mmfr1 >> 20) & 0xF != 0;
    // SAFETY: system register writes that set up EL0 and turn on features the processor has.
    unsafe {
        core::arch::asm!(
            "mov {t}, #(3 << 20)", // CPACR_EL1.FPEN: floating point at EL0 and EL1
            "msr cpacr_el1, {t}",
            "mrs {t}, cntkctl_el1",
            "orr {t}, {t}, #2", // EL0VCTEN: programs may read the virtual counter
            "msr cntkctl_el1, {t}",
            "mrs {t}, tcr_el1",
            "bic {t}, {t}, #(1 << 7)", // EPD0 off: programs' tables are walked
            "msr tcr_el1, {t}",
            "msr ttbr0_el1, {root}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            t = out(reg) _,
            root = in(reg) EMPTY_ROOT.load(Ordering::Acquire),
            options(nostack)
        );
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
        sctlr |= 1 << 4; // SA0: a misaligned stack at EL0 faults
        sctlr |= 1 << 14; // DZE: programs may use DC ZVA (fast zeroing)
        sctlr |= 1 << 15; // UCT: programs may read the cache type
        sctlr |= 1 << 16; // nTWI: WFI at EL0 just waits
        sctlr |= 1 << 18; // nTWE: so does WFE (used by spin loops)
        sctlr |= 1 << 26; // UCI: programs may clean caches (for code they generate)
        sctlr &= !(1 << 24); // E0E: programs are little-endian
        if has_pan {
            sctlr &= !(1 << 23); // SPAN clear: every exception entry sets PAN
        }
        core::arch::asm!("msr sctlr_el1, {}", "isb", in(reg) sctlr, options(nostack));
        if has_pan {
            core::arch::asm!(".inst 0xd500419f", "isb", options(nostack)); // MSR PAN, #1
        }
    }
    PAN.store(has_pan, Ordering::Relaxed);
}

/// Whether PAN is on, for the start-up report.
pub fn has_pan() -> bool {
    PAN.load(Ordering::Relaxed)
}

/// Nothing to do on AArch64: an entry from EL0 uses `SP_EL1`, which is the current thread's
/// own kernel stack. Recorded anyway, for diagnostics.
pub fn set_kernel_stack(top: u64) {
    percpu::this().kernel_sp.store(top, Ordering::Relaxed);
}

/// A program thread's registers that a thread switch doesn't save: `V0` to `V31`, `FPCR`,
/// `FPSR`, and the thread pointer `TPIDR_EL0`.
pub struct UserRegs {
    fp: UnsafeCell<Box<[u128; 33]>>,
    tp: AtomicU64,
}

// SAFETY: the save area is only touched by the core switching to or from the thread, with
// interrupts masked.
unsafe impl Send for UserRegs {}
// SAFETY: as above.
unsafe impl Sync for UserRegs {}

impl UserRegs {
    /// A clean set: everything zero.
    pub fn new() -> Option<Self> {
        Some(UserRegs { fp: UnsafeCell::new(Box::new([0; 33])), tp: AtomicU64::new(0) })
    }
}

/// Saves the current program thread's floating-point state and thread pointer into `r`.
pub fn save_user(r: &UserRegs) {
    // SAFETY: the area holds 33 16-byte slots; only this core touches it now.
    unsafe {
        let p = (*r.fp.get()).as_mut_ptr();
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{p}, #0]",
            "stp q2, q3, [{p}, #32]",
            "stp q4, q5, [{p}, #64]",
            "stp q6, q7, [{p}, #96]",
            "stp q8, q9, [{p}, #128]",
            "stp q10, q11, [{p}, #160]",
            "stp q12, q13, [{p}, #192]",
            "stp q14, q15, [{p}, #224]",
            "stp q16, q17, [{p}, #256]",
            "stp q18, q19, [{p}, #288]",
            "stp q20, q21, [{p}, #320]",
            "stp q22, q23, [{p}, #352]",
            "stp q24, q25, [{p}, #384]",
            "stp q26, q27, [{p}, #416]",
            "stp q28, q29, [{p}, #448]",
            "stp q30, q31, [{p}, #480]",
            "mrs {t}, fpcr",
            "str {t}, [{p}, #512]",
            "mrs {t}, fpsr",
            "str {t}, [{p}, #520]",
            p = in(reg) p,
            t = out(reg) _,
            options(nostack)
        );
        let tp: u64;
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack));
        r.tp.store(tp, Ordering::Relaxed);
    }
}

/// Restores what [`save_user`] saved.
pub fn load_user(r: &UserRegs) {
    // SAFETY: as for `save_user`.
    unsafe {
        let p = (*r.fp.get()).as_ptr();
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "ldp q0, q1, [{p}, #0]",
            "ldp q2, q3, [{p}, #32]",
            "ldp q4, q5, [{p}, #64]",
            "ldp q6, q7, [{p}, #96]",
            "ldp q8, q9, [{p}, #128]",
            "ldp q10, q11, [{p}, #160]",
            "ldp q12, q13, [{p}, #192]",
            "ldp q14, q15, [{p}, #224]",
            "ldp q16, q17, [{p}, #256]",
            "ldp q18, q19, [{p}, #288]",
            "ldp q20, q21, [{p}, #320]",
            "ldp q22, q23, [{p}, #352]",
            "ldp q24, q25, [{p}, #384]",
            "ldp q26, q27, [{p}, #416]",
            "ldp q28, q29, [{p}, #448]",
            "ldp q30, q31, [{p}, #480]",
            "ldr {t}, [{p}, #512]",
            "msr fpcr, {t}",
            "ldr {t}, [{p}, #520]",
            "msr fpsr, {t}",
            p = in(reg) p,
            t = out(reg) _,
            options(nostack)
        );
        core::arch::asm!("msr tpidr_el0, {}", in(reg) r.tp.load(Ordering::Relaxed), options(nomem, nostack));
    }
}

/// Sets the program's thread pointer (`TPIDR_EL0`), now and for when it next resumes.
pub fn set_thread_pointer(r: &UserRegs, value: u64) {
    r.tp.store(value, Ordering::Relaxed);
    // SAFETY: TPIDR_EL0 is the program's own register.
    unsafe { core::arch::asm!("msr tpidr_el0, {}", in(reg) value, options(nomem, nostack)) };
}

/// The last things done before going back to a program: give way if another thread should
/// run, and exit instead if this thread has been asked to stop.
pub fn returning_to_user() {
    sched::interrupt_exit();
    crate::user::before_return();
}

/// Starts the current thread at EL0 at `pc`, with stack `sp`, its argument in `x0`, the
/// system page in `x1` and its message page in `x2` (note 0007, section 3).
pub fn enter_user(pc: u64, sp: u64, arg: u64, sys: u64, msg: u64) -> ! {
    super::irq_disable();
    let top = sched::current().stack_top().expect("a program thread has a kernel stack");
    let frame = (top - core::mem::size_of::<Frame>() as u64) as *mut Frame;
    // SAFETY: the top of this thread's own kernel stack, which nothing else uses: this
    // function's own frame is further down. Returning through the frame leaves `SP_EL1` at
    // the top, where the next entry from EL0 starts.
    unsafe {
        frame.write(Frame::user(pc, sp, [arg, sys, msg]));
        exceptions::return_through(frame)
    }
}

/// Loads a program's page tables (or, for 0, none: an empty tree).
pub fn activate_space(root: u64) {
    let r = if root == 0 { EMPTY_ROOT.load(Ordering::Acquire) } else { root };
    // SAFETY: only the lower half changes. Programs' translations all carry address-space
    // number 0 and aren't global, so flushing that number forgets exactly them.
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {}",
            "isb",
            "tlbi aside1, xzr",
            "dsb nsh",
            "isb",
            in(reg) r,
            options(nostack)
        )
    };
}

/// Makes every core forget cached translations of `pages` pages from `start`, after a
/// program's page tables changed. AArch64 broadcasts TLB maintenance to every core itself.
pub fn flush_user_pages(_root: u64, start: u64, pages: u64) {
    // SAFETY: only discards cached translations.
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        if pages > 64 {
            core::arch::asm!("tlbi aside1is, xzr", options(nostack));
        } else {
            for i in 0..pages {
                let va = (start + i * crate::mem::PAGE) >> 12;
                core::arch::asm!("tlbi vae1is, {}", in(reg) va, options(nostack));
            }
        }
        core::arch::asm!("dsb ish", "isb", options(nostack));
    }
}

// ------------------------------------------------------------------------------------------
// The system page (note 0007, section 1). Copied into a page of its own at start-up; the
// data page follows it in every program.

core::arch::global_asm!(
    ".section .rodata.pbr_syspage, \"a\"",
    ".balign 16",
    ".global pbr_syspage_start",
    "pbr_syspage_start:",
    ".ascii \"PBRSYSPG\"",
    ".short 1, 0",
    ".long 0",
    ".quad 64",
    ".quad 96",
    ".fill 32, 1, 0",
    // 64: call
    "svc #0",
    "ret",
    ".fill 24, 1, 0",
    // 96: clock_now
    "adr x9, pbr_syspage_start + 4096",
    "ldr x10, [x9]",
    "cbz x10, 1f",
    "isb",
    "mrs x11, cntvct_el0",
    "ldr x12, [x9, #8]",
    "sub x11, x11, x12",
    "ldr x12, [x9, #16]",
    "mul x13, x11, x12",
    "umulh x14, x11, x12",
    "extr x0, x14, x13, #32",
    "ret",
    "1:",
    "mov x8, #2",
    "svc #0",
    "mov x0, x1",
    "ret",
    ".global pbr_syspage_end",
    "pbr_syspage_end:",
);

unsafe extern "C" {
    static pbr_syspage_start: u8;
    static pbr_syspage_end: u8;
}

/// The system page's contents.
pub fn syspage_code() -> &'static [u8] {
    let s = &raw const pbr_syspage_start;
    let e = &raw const pbr_syspage_end;
    // SAFETY: the bytes between the two symbols, in the kernel's read-only data.
    unsafe { core::slice::from_raw_parts(s, e as usize - s as usize) }
}

/// Programs can always read the generic timer's counter (`CNTKCTL_EL1.EL0VCTEN`).
pub fn user_clock() -> bool {
    true
}
