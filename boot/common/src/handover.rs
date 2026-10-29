// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The hand-over: the last few instructions of every loader, which switch to the kernel's page
//! tables, set up the registers note 0003 requires, and jump to the kernel. There's no coming
//! back.

/// Everything the hand-over needs.
#[derive(Clone, Copy, Debug)]
pub struct Handover {
    /// The kernel's entry point (virtual).
    pub entry: u64,
    /// The boot information's physical address.
    pub boot_info: u64,
    /// Where physical memory is mapped in the kernel's address space.
    pub direct_map_base: u64,
    /// The top of the kernel's stack (virtual, 16-byte aligned).
    pub stack_top: u64,
    /// The top-level page table for the upper half (x86-64: the only one).
    pub upper_root: u64,
    /// The top-level page table for the lower half (AArch64 only).
    pub lower_root: u64,
    /// AArch64: a page (physical, 2 KiB-aligned) to hold the EL2 stub, if the processor is at
    /// EL2. Ignored on x86-64.
    pub el2_stub_page: u64,
}

#[cfg(target_arch = "x86_64")]
mod imp {
    use super::Handover;

    /// The loader's own GDT, kept in the loader's memory (LoaderReclaimable) rather than the
    /// firmware's, which the kernel may reuse: null, 64-bit code (0x08), data (0x10).
    static GDT: [u64; 3] = [0, 0x00AF_9A00_0000_FFFF, 0x00CF_9200_0000_FFFF];

    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }

    /// Hands over to the kernel.
    ///
    /// # Safety
    /// The page tables must map the loader's code (identity), the kernel and the stack as note
    /// 0003 describes, and nothing may still need the firmware.
    pub unsafe fn jump(h: &Handover) -> ! {
        let gdtr =
            Pointer { limit: (core::mem::size_of_val(&GDT) - 1) as u16, base: GDT.as_ptr() as u64 };
        let idtr = Pointer { limit: 0, base: 0 };
        // SAFETY: the caller promises the page tables are complete; everything below follows
        // note 0003's x86-64 entry state.
        unsafe {
            core::arch::asm!(
                "cli",
                "lgdt [{gdtr}]",
                "lidt [{idtr}]",
                // Reload CS with the new code segment, then the data segments.
                "push 0x08",
                "lea rax, [rip + 2f]",
                "push rax",
                "retfq",
                "2:",
                "mov ax, 0x10",
                "mov ds, ax",
                "mov es, ax",
                "mov ss, ax",
                "xor eax, eax",
                "mov fs, ax",
                "mov gs, ax",
                // No-execute on (EFER.NXE), write protection on (CR0.WP).
                "mov ecx, 0xC0000080",
                "rdmsr",
                "or eax, 1 << 11",
                "wrmsr",
                "mov rax, cr0",
                "or rax, 1 << 16",
                "mov cr0, rax",
                "mov cr3, r8",
                "mov rsp, r12",
                "push 0", // a return address that's never used: RSP is now 8 below the top
                "mov rdi, r10",
                "mov rsi, r11",
                "jmp r9",
                gdtr = in(reg) &gdtr,
                idtr = in(reg) &idtr,
                in("r8") h.upper_root,
                in("r9") h.entry,
                in("r10") h.boot_info,
                in("r11") h.direct_map_base,
                in("r12") h.stack_top,
                options(noreturn)
            )
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use super::Handover;
    use crate::paging::AARCH64_MAIR;

    // The EL2 stub (note 0003): a vector table where only "synchronous exception from EL1"
    // does anything. HVC #0 with X0 = 0 sets VBAR_EL2 to X1; everything else just returns.
    core::arch::global_asm!(
        ".section .rodata.pbr_el2_stub, \"a\"",
        ".balign 2048",
        ".global pbr_el2_stub_start",
        "pbr_el2_stub_start:",
        ".rept 8",
        "eret",
        ".balign 128",
        ".endr",
        // offset 0x400: synchronous exception from a lower level running AArch64
        "cmp x0, #0",
        "b.ne 1f",
        "msr vbar_el2, x1",
        "isb",
        "1: eret",
        ".balign 128",
        ".rept 7",
        "eret",
        ".balign 128",
        ".endr",
        ".global pbr_el2_stub_end",
        "pbr_el2_stub_end:",
    );

    unsafe extern "C" {
        static pbr_el2_stub_start: u8;
        static pbr_el2_stub_end: u8;
    }

    /// The exception level the processor is at now.
    pub fn current_el() -> u64 {
        let el: u64;
        // SAFETY: reading CurrentEL has no side effects.
        unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack)) };
        (el >> 2) & 3
    }

    fn tcr() -> u64 {
        let mmfr0: u64;
        // SAFETY: reading an ID register has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, ID_AA64MMFR0_EL1", out(reg) mmfr0, options(nomem, nostack))
        };
        let ips = (mmfr0 & 0xF).min(5); // physical address size; our tables reach 48 bits
        let t0 = 16 | (1 << 8) | (1 << 10) | (3 << 12); // 48-bit, write-back walks, inner shareable, 4 KiB
        let t1 = (16 << 16) | (1 << 24) | (1 << 26) | (3 << 28) | (2 << 30);
        t0 | t1 | (ips << 32)
    }

    /// Hands over to the kernel, first dropping from EL2 to EL1 if needed.
    ///
    /// # Safety
    /// As for x86-64; in addition `el2_stub_page` must be a free page if the processor is at EL2.
    pub unsafe fn jump(h: &Handover) -> ! {
        let tcr = tcr();
        if current_el() == 2 {
            // SAFETY: the stub symbols come from the global_asm above; the page is ours.
            unsafe {
                let start = &raw const pbr_el2_stub_start;
                let len = (&raw const pbr_el2_stub_end).offset_from(start) as usize;
                core::ptr::copy_nonoverlapping(start, h.el2_stub_page as *mut u8, len);
                // The stub will run with EL2's caches off, so push it all the way to memory.
                let mut line = h.el2_stub_page;
                while line < h.el2_stub_page + len as u64 {
                    core::arch::asm!("dc civac, {0}", in(reg) line);
                    line += 64;
                }
                core::arch::asm!("dsb sy", "ic iallu", "dsb sy", "isb");
            }
            // SAFETY: follows note 0003's AArch64 entry state, via an exception return to EL1.
            unsafe {
                core::arch::asm!(
                    "msr daifset, #0xf",
                    // EL2's own translation off: from now on the stub runs on physical addresses,
                    // so it doesn't depend on the firmware's page tables.
                    "mov x9, #0x0830",
                    "movk x9, #0x30C5, lsl #16",
                    "msr sctlr_el2, x9",
                    "isb",
                    "msr mair_el1, x2",
                    "msr tcr_el1, x3",
                    "msr ttbr0_el1, x4",
                    "msr ttbr1_el1, x5",
                    // SCTLR_EL1: reserved-one bits, plus MMU, data cache and instruction cache on.
                    "mov x9, #0x1805",
                    "movk x9, #0x30D0, lsl #16",
                    "msr sctlr_el1, x9",
                    "mov x9, #(1 << 31)", // HCR_EL2: EL1 runs AArch64; nothing trapped
                    "msr hcr_el2, x9",
                    "mrs x9, cnthctl_el2", // EL1 may use the physical timer and counter
                    "orr x9, x9, #3",
                    "msr cnthctl_el2, x9",
                    "msr cntvoff_el2, xzr",
                    "mov x9, #0x33ff", // CPTR_EL2: don't trap floating point
                    "msr cptr_el2, x9",
                    "mov x9, #(3 << 20)", // CPACR_EL1: don't trap floating point at EL1 either
                    "msr cpacr_el1, x9",
                    "msr vbar_el2, x6",
                    "msr sp_el1, x7",
                    "mov x9, #0x3c5", // return to EL1h with all interrupts masked
                    "msr spsr_el2, x9",
                    "msr elr_el2, x8",
                    "tlbi vmalle1",
                    "dsb ish",
                    "isb",
                    "mov x0, x10",
                    "mov x1, x11",
                    "eret",
                    in("x2") AARCH64_MAIR,
                    in("x3") tcr,
                    in("x4") h.lower_root,
                    in("x5") h.upper_root,
                    in("x6") h.el2_stub_page,
                    in("x7") h.stack_top,
                    in("x8") h.entry,
                    in("x10") h.boot_info,
                    in("x11") h.direct_map_base,
                    options(noreturn)
                )
            }
        }
        // SAFETY: follows note 0003's AArch64 entry state. The code runs from the identity map,
        // which exists both in the firmware's tables and ours, so it keeps running across the
        // switch; it touches no memory while the MMU is off.
        unsafe {
            core::arch::asm!(
                "msr daifset, #0xf",
                "mrs x9, sctlr_el1",
                "bic x9, x9, #1", // MMU off while the translation registers change
                "msr sctlr_el1, x9",
                "isb",
                "msr mair_el1, x2",
                "msr tcr_el1, x3",
                "msr ttbr0_el1, x4",
                "msr ttbr1_el1, x5",
                "isb",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                "orr x9, x9, #1",         // MMU on
                "orr x9, x9, #(1 << 2)",  // data cache on
                "orr x9, x9, #(1 << 12)", // instruction cache on
                "bic x9, x9, #(1 << 1)",  // alignment checking off
                "bic x9, x9, #(1 << 19)", // no "writable means never executable"
                "msr sctlr_el1, x9",
                "isb",
                "mov sp, x7",
                "mov x0, x10",
                "mov x1, x11",
                "br x8",
                in("x2") AARCH64_MAIR,
                in("x3") tcr,
                in("x4") h.lower_root,
                in("x5") h.upper_root,
                in("x7") h.stack_top,
                in("x8") h.entry,
                in("x10") h.boot_info,
                in("x11") h.direct_map_base,
                options(noreturn)
            )
        }
    }
}

pub use imp::*;
