// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the other cores (docs/design/0005-kernel-core.md, section 7), through the
//! firmware's PSCI `CPU_ON`. A new core starts with its MMU off, possibly at EL2; the code
//! below takes it to EL1 with the kernel's page tables and jumps into the kernel.

use super::{exceptions, gic, timer};
use crate::mem::{self, vm};
use crate::percpu::{self, PerCpu};
use crate::{hw, kernel_image, sched, time};
use alloc::boxed::Box;
use core::sync::atomic::Ordering;

/// What a new core reads, with its MMU still off, to set itself up.
#[repr(C)]
struct StartRecord {
    ttbr0: u64,
    ttbr1: u64,
    tcr: u64,
    mair: u64,
    stack: u64,
    percpu: u64,
    entry: u64,
    el2_stub: u64,
}

core::arch::global_asm!(
    ".section .text.pbr_secondary, \"ax\"",
    ".balign 64",
    ".global pbr_secondary_entry",
    "pbr_secondary_entry:",
    "mov x19, x0", // the start record (physical)
    "mrs x9, CurrentEL",
    "lsr x9, x9, #2",
    "cmp x9, #2",
    "b.ne 1f",
    // At EL2: leave the kernel's own EL2 stub in charge and drop to EL1.
    "ldr x10, [x19, #56]",
    "msr vbar_el2, x10",
    "mov x9, #(1 << 31)",
    "msr hcr_el2, x9",
    "mrs x9, cnthctl_el2",
    "orr x9, x9, #3",
    "msr cnthctl_el2, x9",
    "msr cntvoff_el2, xzr",
    "mov x9, #0x33ff",
    "msr cptr_el2, x9",
    "mov x9, #0x0830",
    "movk x9, #0x30C5, lsl #16",
    "msr sctlr_el2, x9",
    "mov x9, #0x3c5",
    "msr spsr_el2, x9",
    "adr x9, 1f",
    "msr elr_el2, x9",
    "isb",
    "eret",
    "1:",
    "mov x9, #(3 << 20)",
    "msr cpacr_el1, x9",
    "ldr x10, [x19, #24]",
    "msr mair_el1, x10",
    "ldr x10, [x19, #16]",
    "msr tcr_el1, x10",
    "ldr x10, [x19, #0]",
    "msr ttbr0_el1, x10",
    "ldr x10, [x19, #8]",
    "msr ttbr1_el1, x10",
    // Read the rest of the record now: once the MMU is on, only the kernel image is mapped
    // in the lower half.
    "ldr x20, [x19, #32]",
    "ldr x21, [x19, #40]",
    "ldr x22, [x19, #48]",
    "isb",
    "tlbi vmalle1",
    "dsb ish",
    "isb",
    "mov x9, #0x1805", // MMU, data and instruction caches on
    "movk x9, #0x30D0, lsl #16",
    "msr sctlr_el1, x9",
    "isb",
    "mov sp, x20",
    "mov x0, x21",
    "br x22",
    ".global pbr_secondary_entry_end",
    "pbr_secondary_entry_end:",
    // The kernel's EL2 stub, the same as the loader's (note 0003).
    ".balign 2048",
    ".global pbr_kernel_el2_stub",
    "pbr_kernel_el2_stub:",
    ".rept 8",
    "eret",
    ".balign 128",
    ".endr",
    "cmp x0, #0",
    "b.ne 2f",
    "msr vbar_el2, x1",
    "isb",
    "2: eret",
    ".balign 128",
    ".rept 7",
    "eret",
    ".balign 128",
    ".endr",
    ".global pbr_kernel_el2_stub_end",
    "pbr_kernel_el2_stub_end:",
);

unsafe extern "C" {
    static pbr_secondary_entry: u8;
    static pbr_secondary_entry_end: u8;
    static pbr_kernel_el2_stub: u8;
    static pbr_kernel_el2_stub_end: u8;
}

/// Pushes memory all the way out to RAM, so a core with its caches off sees it.
fn clean_to_ram(virt: u64, len: u64) {
    let mut a = virt & !63;
    while a < virt + len {
        // SAFETY: cleaning a cache line has no visible effect on memory's contents.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) a, options(nostack)) };
        a += 64;
    }
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
}

fn psci_cpu_on(hvc: bool, mpidr: u64, entry: u64, context: u64) -> i64 {
    let r: i64;
    // SAFETY: PSCI CPU_ON (SMC64 function 0xC4000003) only starts the given core.
    unsafe {
        if hvc {
            core::arch::asm!("hvc #0", inout("x0") 0xC400_0003u64 as i64 => r, in("x1") mpidr, in("x2") entry, in("x3") context, options(nostack));
        } else {
            core::arch::asm!("smc #0", inout("x0") 0xC400_0003u64 as i64 => r, in("x1") mpidr, in("x2") entry, in("x3") context, options(nostack));
        }
    }
    r
}

/// Starts every core other than this one. Returns how many came up.
pub fn start_other_cores(m: &hw::Machine) -> usize {
    let cpus = percpu::all();
    let Some(hvc) = m.psci else { return 0 };
    if cpus.len() < 2 {
        return 0;
    }
    let (kphys, kvirt, klen) = kernel_image();
    let to_phys = |v: u64| kphys + (v - kvirt);
    let entry_v = &raw const pbr_secondary_entry as u64;
    let stub_v = &raw const pbr_kernel_el2_stub as u64;
    clean_to_ram(entry_v, &raw const pbr_secondary_entry_end as u64 - entry_v);
    clean_to_ram(stub_v, &raw const pbr_kernel_el2_stub_end as u64 - stub_v);
    let (tcr, mair): (u64, u64);
    // SAFETY: reading this core's translation settings.
    unsafe {
        core::arch::asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack));
        core::arch::asm!("mrs {}, mair_el1", out(reg) mair, options(nomem, nostack));
    }
    let identity = vm::identity_tree(kphys, klen);
    let mut started = 0;
    for cpu in cpus.iter().skip(1) {
        let stack = vm::alloc_stack(32).expect("out of memory for a core's stack");
        let rec = Box::leak(Box::new(StartRecord {
            ttbr0: identity,
            ttbr1: vm::root(),
            tcr: tcr & !(1 << 7), // lower-half walks on, for the identity map, until the core is in
            mair,
            stack,
            percpu: *cpu as *const PerCpu as u64,
            entry: secondary_main as *const () as u64,
            el2_stub: to_phys(stub_v),
        }));
        let rec_v = rec as *const StartRecord as u64;
        clean_to_ram(rec_v, core::mem::size_of::<StartRecord>() as u64);
        let r = psci_cpu_on(hvc, cpu.hw_id, to_phys(entry_v), rec_v - mem::phys_to_virt(0));
        if r != 0 {
            continue;
        }
        let deadline = time::now_ns() + 1_000_000_000;
        while !cpu.online.load(Ordering::Acquire) && time::now_ns() < deadline {
            core::hint::spin_loop();
        }
        if cpu.online.load(Ordering::Acquire) {
            started += 1;
        }
    }
    started
}

/// Where each new core arrives, at EL1 with the kernel's page tables, on its own stack.
extern "C" fn secondary_main(cpu: u64) -> ! {
    // SAFETY: the boot core passed this core's PerCpu, which lives for ever.
    let cpu: &'static PerCpu = unsafe { &*(cpu as *const PerCpu) };
    percpu::install(cpu);
    exceptions::install();
    // The identity map was only for getting here: from now on the lower half is programs'
    // (and empty until one runs here).
    super::user::init_this_core();
    timer::start(crate::hw_machine().vtimer_irq);
    let _ = gic::version();
    sched::init_this_core();
    cpu.online.store(true, Ordering::Release);
    sched::idle_loop()
}
