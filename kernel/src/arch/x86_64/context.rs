// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Switching between threads (docs/design/0005-kernel-core.md, section 8).

core::arch::global_asm!(
    ".section .text.pbr_switch, \"ax\"",
    // pbr_switch_context(prev_sp: *mut u64 = rdi, next_sp: u64 = rsi)
    ".global pbr_switch_context",
    "pbr_switch_context:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    // Where a new thread's first switch "returns" to: r12 holds its Thread pointer.
    ".global pbr_thread_start",
    "pbr_thread_start:",
    "mov rdi, r12",
    "call {entry}",
    "ud2",
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
    let sp = top - 8 - 6 * 8;
    let regs = sp as *mut u64;
    // SAFETY: the stack is freshly mapped and ours; these are its top seven words.
    unsafe {
        for i in 0..6 {
            regs.add(i).write(0);
        }
        regs.add(3).write(arg); // r12 (popped fourth)
        regs.add(6).write(pbr_thread_start as *const () as u64); // the return address
    }
    sp
}

/// Switches to the stack whose top is `top` and calls `f` on it.
pub fn switch_stack(top: u64, f: extern "C" fn() -> !) -> ! {
    // SAFETY: `top` is a mapped, 16-byte-aligned stack top; `f` never returns.
    unsafe {
        core::arch::asm!("mov rsp, {top}", "call {f}", "ud2", top = in(reg) top, f = in(reg) f, options(noreturn))
    }
}
