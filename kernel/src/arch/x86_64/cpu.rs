// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Each core's GDT and TSS, model-specific registers, and loading the IDT.

use super::{entry, interrupts};
use crate::percpu::PerCpu;
use core::sync::atomic::{AtomicU64, Ordering};

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

/// What x86-64 keeps per core: where its slot in the entry area is, which holds its GDT, TSS,
/// stacks for entering the kernel, and entry block (`entry.rs`).
pub struct CpuArch {
    slot: AtomicU64,
}

impl CpuArch {
    /// Empty: the slot is made when the core is described (`set_core_info`).
    pub fn new() -> Self {
        Self { slot: AtomicU64::new(0) }
    }

    /// The core's slot in the entry area.
    pub fn slot(&self) -> u64 {
        self.slot.load(Ordering::Relaxed)
    }

    /// Records the core's slot.
    pub fn set_slot(&self, slot: u64) {
        self.slot.store(slot, Ordering::Relaxed);
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

/// Makes entry 4 of this core's page attribute table write-combining (note 0010, section 3),
/// which page-table entries with `Memory::Framebuffer` select. The caches are written back and
/// emptied afterwards, so no line cached under the old meaning of the entry is left behind.
fn init_pat() {
    // SAFETY: IA32_PAT (MSR 0x277) exists on every x86-64 processor; the value changes only the
    // meaning of entry 4, which the kernel's tables use only for framebuffers.
    unsafe {
        wrmsr(0x277, pbr_platform::paging::X86_PAT);
        core::arch::asm!("wbinvd", options(nomem, nostack));
    }
}

/// Loads this core's GDT (with its TSS) and the shared IDT, all in the entry area.
pub fn init_this_core(cpu: &PerCpu) {
    init_pat();
    let slot = cpu.arch.slot();
    assert!(entry::is_slot(slot), "a core without a slot in the entry area");
    let (gdt_at, tss_at) = entry::gdt_and_tss(slot);
    // SAFETY: the slot's first page is mapped writable, holds room for both at these offsets,
    // and only this core touches its own, before loading them.
    let (gdt, tss) = unsafe { (&mut *(gdt_at as *mut [u64; 7]), &mut *(tss_at as *mut Tss)) };
    // Entries from a program arrive on the entry stack; double faults, NMIs and machine checks
    // on their own stacks (IST 1 to 3).
    let (entry_top, ist) = entry::stack_tops(slot);
    *tss = Tss {
        reserved0: 0,
        rsp: [entry_top, 0, 0],
        reserved1: 0,
        ist: [ist[0], ist[1], ist[2], 0, 0, 0, 0],
        reserved2: 0,
        reserved3: 0,
        iomap: 104,
    };
    let base = tss_at;
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
    super::user::init_this_core();
}
