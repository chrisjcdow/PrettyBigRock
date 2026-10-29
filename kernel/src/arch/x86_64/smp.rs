// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the other cores (docs/design/0005-kernel-core.md, section 7): a trampoline below
//! 1 MiB takes each from 16-bit real mode to 64-bit mode and into the kernel.

use super::{apic, cpu};
use crate::mem::{self, pmm, vm};
use crate::percpu::{self, PerCpu};
use crate::{hw, sched, time};
use core::sync::atomic::Ordering;

core::arch::global_asm!(
    ".section .text.pbr_trampoline, \"ax\"",
    ".code16",
    ".global pbr_ap_trampoline",
    "pbr_ap_trampoline:",
    "cli",
    "cld",
    "mov ax, cs",
    "mov ds, ax",
    "lgdt [pbr_tr_off_gdtr]",
    "mov eax, cr4",
    "or eax, 0x20", // PAE
    "mov cr4, eax",
    "mov eax, dword ptr [pbr_tr_off_cr3]",
    "mov cr3, eax",
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or eax, 0x900", // long mode, no-execute
    "wrmsr",
    "mov eax, cr0",
    "or eax, 0x80010001", // paging, write protection, protected mode
    "mov cr0, eax",
    ".byte 0x66, 0xFF, 0x2E", // jmp far dword [pbr_tr_far]
    ".word pbr_tr_far - pbr_ap_trampoline",
    ".code64",
    ".global pbr_tr_long",
    "pbr_tr_long:",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "mov rsp, qword ptr [rip + pbr_tr_stack]",
    "mov rdi, qword ptr [rip + pbr_tr_percpu]",
    "mov rax, qword ptr [rip + pbr_tr_entry]",
    "push 0", // as if called: the entry point sees the usual stack alignment
    "jmp rax",
    ".balign 8",
    ".global pbr_tr_gdt",
    "pbr_tr_gdt:",
    ".quad 0",
    ".quad 0x00AF9A000000FFFF",
    ".quad 0x00CF92000000FFFF",
    ".global pbr_tr_gdtr",
    "pbr_tr_gdtr:",
    ".word 23",
    ".global pbr_tr_gdt_base",
    "pbr_tr_gdt_base:",
    ".long 0",
    ".global pbr_tr_far",
    "pbr_tr_far:",
    ".long 0",
    ".word 0x08",
    ".global pbr_tr_cr3",
    "pbr_tr_cr3:",
    ".long 0",
    ".balign 8",
    ".global pbr_tr_stack",
    "pbr_tr_stack:",
    ".quad 0",
    ".global pbr_tr_percpu",
    "pbr_tr_percpu:",
    ".quad 0",
    ".global pbr_tr_entry",
    "pbr_tr_entry:",
    ".quad 0",
    ".global pbr_ap_trampoline_end",
    "pbr_ap_trampoline_end:",
    // Offsets within the trampoline, for its 16-bit code (which runs with DS at its start).
    ".set pbr_tr_off_gdtr, pbr_tr_gdtr - pbr_ap_trampoline",
    ".set pbr_tr_off_cr3, pbr_tr_cr3 - pbr_ap_trampoline",
    ".code64",
);

unsafe extern "C" {
    static pbr_ap_trampoline: u8;
    static pbr_ap_trampoline_end: u8;
    static pbr_tr_long: u8;
    static pbr_tr_gdt: u8;
    static pbr_tr_gdt_base: u8;
    static pbr_tr_far: u8;
    static pbr_tr_cr3: u8;
    static pbr_tr_stack: u8;
    static pbr_tr_percpu: u8;
    static pbr_tr_entry: u8;
}

const STACK_PAGES: u64 = 32;

/// Starts every core in `m` other than this one. Returns how many came up.
pub fn start_other_cores(m: &hw::Machine) -> usize {
    let cpus = percpu::all();
    if cpus.len() < 2 {
        return 0;
    }
    let start = &raw const pbr_ap_trampoline as u64;
    let len = &raw const pbr_ap_trampoline_end as u64 - start;
    let off = |s: *const u8| (s as u64 - start) as usize;
    let page =
        pmm::alloc(1, 1, 0x10_0000).expect("no memory below 1 MiB for the start-up trampoline");
    let code = mem::phys_ptr::<u8>(page);
    // SAFETY: the trampoline's bytes, copied into our own fresh page below 1 MiB.
    unsafe { core::ptr::copy_nonoverlapping(start as *const u8, code, len as usize) };
    let put32 = |o: usize, v: u32| {
        // SAFETY: inside the copied trampoline.
        unsafe { (code.add(o) as *mut u32).write_unaligned(v) }
    };
    let put64 = |o: usize, v: u64| {
        // SAFETY: inside the copied trampoline.
        unsafe { (code.add(o) as *mut u64).write_unaligned(v) }
    };
    put32(off(&raw const pbr_tr_gdt_base), (page + off(&raw const pbr_tr_gdt) as u64) as u32);
    put32(off(&raw const pbr_tr_far), (page + off(&raw const pbr_tr_long) as u64) as u32);
    put32(off(&raw const pbr_tr_cr3), vm::root() as u32);
    put64(off(&raw const pbr_tr_entry), ap_main as *const () as u64);
    vm::identity_map(page, mem::PAGE);

    let mut started = 0;
    for cpu in cpus.iter().skip(1) {
        let stack = vm::alloc_stack(STACK_PAGES).expect("out of memory for a core's stack");
        put64(off(&raw const pbr_tr_stack), stack);
        put64(off(&raw const pbr_tr_percpu), *cpu as *const PerCpu as u64);
        let id = cpu.hw_id as u32;
        apic::send_init(id);
        time::spin_ns(10_000_000);
        apic::send_startup(id, (page >> 12) as u32);
        time::spin_ns(200_000);
        if !cpu.online.load(Ordering::Acquire) {
            apic::send_startup(id, (page >> 12) as u32);
        }
        let deadline = time::now_ns() + 1_000_000_000;
        while !cpu.online.load(Ordering::Acquire) && time::now_ns() < deadline {
            core::hint::spin_loop();
        }
        if cpu.online.load(Ordering::Acquire) {
            started += 1;
        }
    }
    vm::identity_unmap(page, mem::PAGE);
    pmm::free(page, 1);
    let _ = m;
    started
}

/// Where each new core arrives, in 64-bit mode on the kernel's page tables and its own stack.
extern "C" fn ap_main(cpu: u64) -> ! {
    // SAFETY: the boot core passed this core's PerCpu, which lives for ever.
    let cpu: &'static PerCpu = unsafe { &*(cpu as *const PerCpu) };
    percpu::install(cpu);
    cpu::init_this_core(cpu);
    apic::enable_this_core();
    apic::start_timer();
    sched::init_this_core();
    cpu.online.store(true, Ordering::Release);
    sched::idle_loop()
}
