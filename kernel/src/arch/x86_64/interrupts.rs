// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The IDT and the handler every interrupt and exception shares. The stubs that reach it are
//! in the entry area (`entry.rs`, note 0027, section 1b).

use super::{apic, cpu};
use crate::{kprintln, percpu, sched};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The timer interrupt's vector.
pub const TIMER: u8 = 32;
/// "Look at your run queue."
pub const IPI_RESCHEDULE: u8 = 0xF0;
/// "Stop: the kernel has panicked."
pub const IPI_STOP: u8 = 0xF1;
/// "Forget your cached program translations" (see `user::flush_user_pages`).
pub const IPI_SHOOTDOWN: u8 = 0xF2;
/// The APIC's spurious-interrupt vector.
pub const SPURIOUS: u8 = 0xFF;

/// Breakpoints taken (the self-test checks this).
pub static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// What the entry stub saves (and the system call entry, which makes the same frame).
#[repr(C)]
pub struct Frame {
    pub(super) r15: u64,
    pub(super) r14: u64,
    pub(super) r13: u64,
    pub(super) r12: u64,
    pub(super) r11: u64,
    pub(super) r10: u64,
    pub(super) r9: u64,
    pub(super) r8: u64,
    pub(super) rbp: u64,
    pub(super) rdi: u64,
    pub(super) rsi: u64,
    pub(super) rdx: u64,
    pub(super) rcx: u64,
    pub(super) rbx: u64,
    pub(super) rax: u64,
    pub(super) vector: u64,
    pub(super) error: u64,
    pub(super) rip: u64,
    pub(super) cs: u64,
    pub(super) rflags: u64,
    pub(super) rsp: u64,
    pub(super) ss: u64,
}

impl Frame {
    /// A frame that starts a program: every register zero except the first three argument
    /// registers.
    pub(super) fn user(rip: u64, rsp: u64, cs: u64, ss: u64, rflags: u64, args: [u64; 3]) -> Self {
        Frame {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rbp: 0,
            rdi: args[0],
            rsi: args[1],
            rdx: args[2],
            rcx: 0,
            rbx: 0,
            rax: 0,
            vector: 0,
            error: 0,
            rip,
            cs,
            rflags,
            rsp,
            ss,
        }
    }

    fn is_from_user(&self) -> bool {
        self.cs & 3 == 3
    }
}

/// Fills in the IDT, in the entry area. Called once, on the boot core.
pub fn build_idt() {
    let base = super::entry::isr_stubs();
    for (v, e) in super::entry::idt_entries().iter_mut().enumerate() {
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
    super::entry::IDT
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

pub(super) extern "C" fn dispatch(f: &mut Frame) {
    let v = f.vector as u8;
    let user = f.is_from_user();
    match v {
        // A program's fault ends its process; machine-level trouble is never the program's.
        0..=31 if user && !matches!(v, 2 | 8 | 18) => user_fault(f),
        3 => {
            BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        }
        2 if crate::panicking() => super::halt_forever(),
        2 => asked_for_state(),
        0..=31 => fatal(f),
        TIMER => {
            apic::eoi();
            sched::tick();
        }
        IPI_RESCHEDULE => {
            apic::eoi();
            percpu::this().sched.need_resched.store(true, Ordering::Relaxed);
        }
        IPI_STOP => {
            crate::power::core_stopped();
            super::halt_forever()
        }
        IPI_SHOOTDOWN => {
            apic::eoi();
            super::user::shootdown_interrupt();
        }
        // The local APIC's spurious vector, and the masked 8259s': neither wants an EOI.
        SPURIOUS => {}
        v if apic::PIC_SPURIOUS.contains(&v) => {}
        v if super::irqchip::is_device_vector(v) => super::irqchip::interrupt(v),
        _ => apic::eoi(),
    }
    if v >= 32 {
        if user {
            super::user::returning_to_user();
        } else {
            sched::interrupt_exit();
        }
    }
}

/// Set once a non-maskable interrupt has printed the scheduler's state.
static STATE_PRINTED: AtomicBool = AtomicBool::new(false);

/// A non-maskable interrupt (NMI) arrives even while a core has interrupts masked. Outside a
/// panic nothing in the machine sends one, so it is the boot test runner asking, through
/// QEMU's `inject-nmi`, what the scheduler was doing when a boot stopped making progress: each
/// core's tick count, run queue and timers, and every thread. QEMU sends it to every core;
/// the first to arrive prints, once, and the rest carry on.
///
/// This is only for a machine that has hung already: printing takes the console's and the
/// heap's locks, and if the interrupted code on this core held one, this core waits for ever.
fn asked_for_state() {
    if STATE_PRINTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let core = percpu::try_this().map(|c| c.index).unwrap_or(0);
    kprintln!("NMI on core {core}: the scheduler's state, asked for by the test runner");
    sched::dump();
    kprintln!("NMI: end of the scheduler's state");
}

fn user_fault(f: &Frame) -> ! {
    use crate::obj::process::fault_kind as k;
    let cr2: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    let (kind, addr) = match f.vector {
        14 => (k::MEMORY, cr2),
        0 | 4 | 16 | 19 => (k::ARITHMETIC, 0),
        3 => (k::BREAKPOINT, f.rip),
        6 | 7 | 13 => (k::INSTRUCTION, f.rip),
        17 => (k::ALIGNMENT, 0),
        _ => (k::OTHER, 0),
    };
    crate::user::fault(kind, addr, f.rip)
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
