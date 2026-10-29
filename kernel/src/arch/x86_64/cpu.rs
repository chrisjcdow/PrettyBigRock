// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Each core's GDT and TSS, model-specific registers, and loading the IDT.

use super::interrupts;
use crate::mem::vm;
use crate::percpu::PerCpu;
use core::cell::UnsafeCell;

/// Reads a model-specific register.
///
/// # Safety
/// The register must exist.
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: the caller promises the MSR exists.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack))
    };
    u64::from(lo) | (u64::from(hi) << 32)
}

/// Writes a model-specific register.
///
/// # Safety
/// The register must exist, and the value must be valid for it.
pub unsafe fn wrmsr(msr: u32, v: u64) {
    // SAFETY: the caller promises the MSR and value are valid.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32, options(nomem, nostack))
    };
}

/// The task-state segment: only its interrupt stacks are used.
#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap: u16,
}

/// What x86-64 keeps per core: its GDT and TSS.
pub struct CpuArch {
    gdt: UnsafeCell<[u64; 7]>,
    tss: UnsafeCell<Tss>,
}

impl CpuArch {
    /// Empty, filled in by [`init_this_core`].
    pub fn new() -> Self {
        Self {
            gdt: UnsafeCell::new([0; 7]),
            tss: UnsafeCell::new(Tss {
                reserved0: 0,
                rsp: [0; 3],
                reserved1: 0,
                ist: [0; 7],
                reserved2: 0,
                reserved3: 0,
                iomap: 104,
            }),
        }
    }
}

impl Default for CpuArch {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C, packed)]
struct Pointer {
    limit: u16,
    base: u64,
}

/// Loads this core's GDT (with its TSS) and the shared IDT.
pub fn init_this_core(cpu: &PerCpu) {
    // Separate stacks for double faults, NMIs and machine checks (IST 1 to 3).
    // SAFETY: only this core touches its own GDT and TSS, before loading them.
    let (gdt, tss) = unsafe { (&mut *cpu.arch.gdt.get(), &mut *cpu.arch.tss.get()) };
    for i in 0..3 {
        tss.ist[i] = vm::alloc_stack(4).expect("out of memory for interrupt stacks");
    }
    let base = tss as *const Tss as u64;
    let limit = (core::mem::size_of::<Tss>() - 1) as u64;
    gdt[0] = 0;
    gdt[1] = 0x00AF_9A00_0000_FFFF; // 0x08: kernel code
    gdt[2] = 0x00CF_9200_0000_FFFF; // 0x10: kernel data
    gdt[3] = 0x00CF_F200_0000_FFFF; // 0x18: user data (from M3)
    gdt[4] = 0x00AF_FA00_0000_FFFF; // 0x20: user code (from M3)
    gdt[5] = (limit & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x89 << 40)
        | (((base >> 24) & 0xFF) << 56); // 0x28: TSS
    gdt[6] = base >> 32;
    let gdtr = Pointer { limit: (7 * 8 - 1) as u16, base: gdt.as_ptr() as u64 };
    let idtr = Pointer { limit: (256 * 16 - 1) as u16, base: interrupts::idt_address() };
    // SAFETY: the GDT has the same code and data selectors the kernel already uses, the TSS
    // descriptor points at this core's TSS, and the IDT is complete.
    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            "push 0x08",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov ax, 0x28",
            "ltr ax",
            "lidt [{idtr}]",
            gdtr = in(reg) &gdtr,
            idtr = in(reg) &idtr,
            out("rax") _,
        );
    }
}
