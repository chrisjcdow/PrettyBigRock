// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Switching between threads (docs/design/0005-kernel-core.md, section 8).

core::arch::global_asm!(
    ".section .text.pbr_switch, \"ax\"",
    // pbr_switch_context(prev_sp: *mut u64 = x0, next_sp: u64 = x1)
    ".global pbr_switch_context",
    "pbr_switch_context:",
    "sub sp, sp, #96",
    "stp x19, x20, [sp, #0]",
    "stp x21, x22, [sp, #16]",
    "stp x23, x24, [sp, #32]",
    "stp x25, x26, [sp, #48]",
    "stp x27, x28, [sp, #64]",
    "stp x29, x30, [sp, #80]",
    "mov x9, sp",
    "str x9, [x0]",
    "mov sp, x1",
    "ldp x19, x20, [sp, #0]",
    "ldp x21, x22, [sp, #16]",
    "ldp x23, x24, [sp, #32]",
    "ldp x25, x26, [sp, #48]",
    "ldp x27, x28, [sp, #64]",
    "ldp x29, x30, [sp, #80]",
    "add sp, sp, #96",
    "ret",
    // Where a new thread's first switch "returns" to: x19 holds its Thread pointer.
    ".global pbr_thread_start",
    "pbr_thread_start:",
    "mov x0, x19",
    "bl {entry}",
    "brk #1",
    entry = sym crate::sched::thread_entry,
);

unsafe extern "C" {
    fn pbr_switch_context(prev_sp: *mut u64, next_sp: u64);
    fn pbr_thread_start();
}

/// Saves this thread's registers on its stack, stores its stack pointer in `*prev_sp`, and
/// resumes the thread whose stack pointer is `next_sp`.
///
/// # Safety
/// Interrupts must be masked; `next_sp` must come from a previous switch or
/// [`init_thread_stack`].
pub unsafe fn switch_context(prev_sp: *mut u64, next_sp: u64) {
    // SAFETY: as the caller promises.
    unsafe { pbr_switch_context(prev_sp, next_sp) }
}

/// Lays out a new thread's stack so that switching to it starts `sched::thread_entry(arg)`.
/// Returns the stack pointer to switch to.
pub fn init_thread_stack(top: u64, arg: u64) -> u64 {
    let sp = top - 96;
    let regs = sp as *mut u64;
    // SAFETY: the stack is freshly mapped and ours; these are its top twelve words.
    unsafe {
        for i in 0..12 {
            regs.add(i).write(0);
        }
        regs.write(arg); // x19
        regs.add(11).write(pbr_thread_start as *const () as u64); // x30, the return address
    }
    sp
}

/// Switches to the stack whose top is `top` and calls `f` on it.
pub fn switch_stack(top: u64, f: extern "C" fn() -> !) -> ! {
    // SAFETY: `top` is a mapped, 16-byte-aligned stack top; `f` never returns.
    unsafe {
        core::arch::asm!("mov sp, {top}", "blr {f}", "brk #1", top = in(reg) top, f = in(reg) f, options(noreturn))
    }
}
