// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The IDT, the entry stubs for all 256 vectors, and the handler they share.

use super::{apic, cpu};
use crate::{kprintln, percpu, sched};
use core::sync::atomic::{AtomicU64, Ordering};

/// The timer interrupt's vector.
pub const TIMER: u8 = 32;
/// "Look at your run queue."
pub const IPI_RESCHEDULE: u8 = 0xF0;
/// "Stop: the kernel has panicked."
pub const IPI_STOP: u8 = 0xF1;
/// The APIC's spurious-interrupt vector.
pub const SPURIOUS: u8 = 0xFF;

/// Breakpoints taken (the self-test checks this).
pub static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

// Each vector's stub is 16 bytes long, so the IDT can be filled in from one base address. Stubs
// for exceptions without an error code push a zero in its place, so every frame looks alike.
core::arch::global_asm!(
    ".section .text.pbr_isr, \"ax\"",
    ".balign 16",
    ".global pbr_isr_stubs",
    "pbr_isr_stubs:",
    ".set vec, 0",
    ".rept 256",
    ".balign 16",
    ".if (vec == 8) || (vec == 10) || (vec == 11) || (vec == 12) || (vec == 13) || (vec == 14) || (vec == 17) || (vec == 21) || (vec == 29) || (vec == 30)",
    ".else",
    "push 0",
    ".endif",
    ".byte 0x68", // push imm32 (always the long form, so 128-255 aren't sign-extended)
    ".long vec",
    "jmp pbr_isr_common",
    ".set vec, vec + 1",
    ".endr",
    "pbr_isr_common:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov rdi, rsp",
    "cld",
    "call {dispatch}",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "add rsp, 16",
    "iretq",
    dispatch = sym dispatch,
);

unsafe extern "C" {
    static pbr_isr_stubs: u8;
}

/// What the entry stub saves.
#[repr(C)]
pub struct Frame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    vector: u64,
    error: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[repr(C, align(16))]
struct Idt([[u64; 2]; 256]);

static mut IDT: Idt = Idt([[0; 2]; 256]);

/// Fills in the IDT. Called once, on the boot core.
pub fn build_idt() {
    let base = &raw const pbr_isr_stubs as u64;
    // Start-up runs on one core, before any core has loaded the IDT.
    let idt: *mut Idt = &raw mut IDT;
    // SAFETY: as above; nothing else refers to the IDT yet.
    let entries = unsafe { &mut (*idt).0 };
    for (v, e) in entries.iter_mut().enumerate() {
        let addr = base + 16 * v as u64;
        let ist: u64 = match v {
            8 => 1,  // double fault
            2 => 2,  // NMI
            18 => 3, // machine check
            _ => 0,
        };
        // Present, ring 0, 64-bit interrupt gate (interrupts masked on entry).
        e[0] = (addr & 0xFFFF)
            | (0x08 << 16)
            | (ist << 32)
            | (0x8E << 40)
            | (((addr >> 16) & 0xFFFF) << 48);
        e[1] = addr >> 32;
    }
}

/// The IDT's address, for loading it.
pub fn idt_address() -> u64 {
    &raw const IDT as u64
}

const NAMES: [&str; 22] = [
    "divide error",
    "debug",
    "non-maskable interrupt",
    "breakpoint",
    "overflow",
    "bound range exceeded",
    "invalid opcode",
    "device not available",
    "double fault",
    "coprocessor segment overrun",
    "invalid TSS",
    "segment not present",
    "stack-segment fault",
    "general protection fault",
    "page fault",
    "reserved",
    "x87 floating-point error",
    "alignment check",
    "machine check",
    "SIMD floating-point error",
    "virtualisation exception",
    "control protection",
];

extern "C" fn dispatch(f: &mut Frame) {
    let v = f.vector as u8;
    match v {
        3 => {
            BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        }
        2 if crate::panicking() => super::halt_forever(),
        0..=31 => fatal(f),
        TIMER => {
            apic::eoi();
            sched::tick();
        }
        IPI_RESCHEDULE => {
            apic::eoi();
            percpu::this().sched.need_resched.store(true, Ordering::Relaxed);
        }
        IPI_STOP => super::halt_forever(),
        SPURIOUS => {}
        _ => apic::eoi(),
    }
    if v >= 32 {
        sched::interrupt_exit();
    }
}

fn fatal(f: &Frame) -> ! {
    let name = NAMES.get(f.vector as usize).copied().unwrap_or("reserved exception");
    let cr2: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    let core = percpu::try_this().map(|c| c.index).unwrap_or(0);
    kprintln!("\nException on core {core}: {name} (vector {}, error {:#x})", f.vector, f.error);
    kprintln!(
        "  rip {:#018x}  rsp {:#018x}  rflags {:#x}  cr2 {:#018x}",
        f.rip,
        f.rsp,
        f.rflags,
        cr2
    );
    kprintln!(
        "  rax {:#018x}  rbx {:#018x}  rcx {:#018x}  rdx {:#018x}",
        f.rax,
        f.rbx,
        f.rcx,
        f.rdx
    );
    kprintln!(
        "  rsi {:#018x}  rdi {:#018x}  rbp {:#018x}  r8  {:#018x}",
        f.rsi,
        f.rdi,
        f.rbp,
        f.r8
    );
    kprintln!(
        "  r9  {:#018x}  r10 {:#018x}  r11 {:#018x}  r12 {:#018x}",
        f.r9,
        f.r10,
        f.r11,
        f.r12
    );
    kprintln!("  r13 {:#018x}  r14 {:#018x}  r15 {:#018x}", f.r13, f.r14, f.r15);
    let _ = cpu::rdmsr; // (keeps the import meaningful for future register dumps)
    panic!("{name} in the kernel");
}
