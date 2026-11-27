// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The entry area (docs/design/0027-kernel-1.4.md, section 1b): everything the processor must
//! reach to get from a program into the kernel, and the code that does it.
//!
//! On processors affected by Meltdown a program can read whatever the kernel has mapped, so
//! while a program runs the kernel is (almost) not mapped: each process has two top-level page
//! tables, the kernel's copy and the program's copy (`lib/platform`'s `UserCopy`). The
//! program's copy maps only the program's half and this area, which holds:
//! - the **entry code**: the interrupt stubs, the system call entry and the way out, in their
//!   own linker section (`.text.pbr_entry`), mapped here a second time, read and execute;
//! - the shared **IDT** (the table of interrupt handlers), read-only;
//! - for each core, a **slot** with that core's GDT and TSS (which the processor reads on every
//!   entry from a program), a small **entry stack**, the three **IST stacks** (separate stacks
//!   for double faults, NMIs and machine checks) with unmapped guard pages between, and an
//!   **entry block**: the few values the entry code needs before it can switch tables.
//!
//! The two top-level tables are allocated together, the kernel's first, so the program's is at
//! the kernel's address plus 4096: the entry code switches between them by setting or clearing
//! bit 12 of `CR3` (the register holding the top table's address), without reading memory.
//! When KPTI is off, the same area and code are used and only those two writes are skipped, so
//! QEMU's runs without it still test everything else.
//!
//! `GS` points at the core's entry block, whose first word is the address of the core's
//! `PerCpu`, so the per-core data stays unmapped while programs run. The entry code runs here,
//! not at the address it was linked at: it uses only jumps within itself and absolute calls.

use crate::mem::{PAGE, phys_ptr, pmm, vm};
use crate::percpu::PerCpu;
use core::sync::atomic::{AtomicU64, Ordering};
use pbr_platform::paging::{Attr, Memory};

/// Where the entry area starts, and its size: one top-level entry's worth would be 512 GiB, but
/// 1 GiB is plenty (note 0005, section 2).
pub const AREA: u64 = 0xFFFF_D000_0000_0000;
/// The area's end.
pub const AREA_END: u64 = AREA + (1 << 30);
/// The top-level page-table entry holding it, the only one of the kernel's half a program's
/// copy has.
pub const TOP_INDEX: usize = ((AREA >> 39) & 511) as usize;
/// Where the entry code is mapped.
const CODE: u64 = AREA;
/// Where the IDT is.
pub const IDT: u64 = AREA + 0x10_0000;
/// Where the per-core slots start.
const SLOTS: u64 = AREA + 0x20_0000;
/// Each core's slot: 32 pages.
const SLOT: u64 = 32 * PAGE;

// A slot's pages: 0 the entry block, GDT and TSS; 1 a guard; 2 and 3 the entry stack; 4 a
// guard; then three IST stacks of four pages, each followed by a guard.
const BLOCK_PAGE: u64 = 0;
const ENTRY_STACK: (u64, u64) = (2, 2);
const IST_STACKS: [(u64, u64); 3] = [(5, 4), (10, 4), (15, 4)];
/// Where the GDT and TSS are in the slot's first page.
const GDT_OFFSET: u64 = 0x100;
const TSS_OFFSET: u64 = 0x200;

/// The values the entry code needs, at the start of the slot, read through `GS`. The first
/// three are where `PerCpu`'s are (`percpu::OFFSET_*`), so code that reads them through `GS`
/// needn't care which it is.
#[repr(C)]
pub struct Block {
    /// The core's `PerCpu` (`GS:[0]`).
    pub percpu: AtomicU64,
    /// A program's stack pointer, kept here by the system call entry (`GS:[8]`).
    pub user_sp: AtomicU64,
    /// The top of the current thread's kernel stack (`GS:[16]`).
    pub kernel_sp: AtomicU64,
    /// Where the return frame goes on the entry stack, on the way back to a program (`GS:[24]`).
    pub frame: AtomicU64,
    /// Bit 0: switch page tables (KPTI); bit 1: clear the buffers on return (MDS) (`GS:[32]`).
    pub flags: AtomicU64,
    /// Somewhere to keep `rax` for a moment on entry (`GS:[40]`).
    pub scratch: AtomicU64,
}

const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(Block, user_sp) == crate::percpu::OFFSET_USER_SP);
    assert!(offset_of!(Block, kernel_sp) == crate::percpu::OFFSET_KERNEL_SP);
    assert!(offset_of!(Block, frame) == 24);
    assert!(offset_of!(Block, flags) == 32);
    assert!(offset_of!(Block, scratch) == 40);
};

/// The flag bits in [`Block::flags`].
pub const FLAG_KPTI: u64 = 1 << 0;
/// See [`FLAG_KPTI`].
pub const FLAG_MDS: u64 = 1 << 1;

/// The size of the frame the processor pushes on the way back to a program, which `iretq`
/// pops: the program's `rip`, `cs`, `rflags`, `rsp` and `ss`.
const RETURN_FRAME: u64 = 5 * 8;

unsafe extern "C" {
    static __entry_start: u8;
    static __entry_end: u8;
    static pbr_isr_stubs: u8;
    static pbr_isr_exit: u8;
    static pbr_syscall_entry: u8;
}

/// The address the entry code at `sym` (linked in the kernel image) runs at, here.
fn here(sym: *const u8) -> u64 {
    CODE + (sym as u64 - &raw const __entry_start as u64)
}

/// Where the interrupt stubs run.
pub fn isr_stubs() -> u64 {
    here(&raw const pbr_isr_stubs)
}

/// Where the way back to a program (or the kernel) runs.
pub fn isr_exit() -> u64 {
    here(&raw const pbr_isr_exit)
}

/// Where the system call entry runs, for `IA32_LSTAR`.
pub fn syscall_entry() -> u64 {
    here(&raw const pbr_syscall_entry)
}

/// The physical address of the kernel image's page at `virt`.
fn kernel_phys(virt: u64) -> u64 {
    let (phys, base, _) = crate::kernel_image();
    phys + (virt - base)
}

/// Maps the entry code and a page for the IDT. Called once, on the boot core, before any
/// slot.
pub fn init() {
    let start = &raw const __entry_start as u64;
    let end = &raw const __entry_end as u64;
    let code = Attr { write: false, exec: true, mem: Memory::Normal, user: false };
    vm::map_at(CODE, kernel_phys(start), end - start, code);
    let idt = pmm::alloc_page().expect("out of memory for the IDT");
    IDT_PHYS.store(idt, Ordering::Relaxed);
    let read_only = Attr { write: false, exec: false, mem: Memory::Normal, user: false };
    vm::map_at(IDT, idt, PAGE, read_only);
}

/// The IDT's physical page, which the boot core fills in through the direct map.
static IDT_PHYS: AtomicU64 = AtomicU64::new(0);

/// The IDT's 256 entries, for filling in.
pub fn idt_entries() -> &'static mut [[u64; 2]; 256] {
    // SAFETY: the page allocated in `init`, reached through the direct map; only the boot core
    // fills it in, once, before any core loads it.
    unsafe { &mut *phys_ptr::<[[u64; 2]; 256]>(IDT_PHYS.load(Ordering::Relaxed)) }
}

/// Makes core `index`'s slot: its pages mapped, its entry block filled in. Returns the slot's
/// address (also its entry block's).
pub fn make_slot(index: u32, cpu: &PerCpu, flags: u64) -> u64 {
    let slot = SLOTS + u64::from(index) * SLOT;
    assert!(slot + SLOT <= AREA_END, "too many cores for the entry area");
    let map = |first: u64, pages: u64| {
        for p in first..first + pages {
            let frame = pmm::alloc_page().expect("out of memory for the entry area");
            vm::map_at(slot + p * PAGE, frame, PAGE, Attr::DATA);
        }
    };
    map(BLOCK_PAGE, 1);
    map(ENTRY_STACK.0, ENTRY_STACK.1);
    for (first, pages) in IST_STACKS {
        map(first, pages);
    }
    let b = block(slot);
    let entry_top = slot + (ENTRY_STACK.0 + ENTRY_STACK.1) * PAGE;
    b.percpu.store(cpu as *const PerCpu as u64, Ordering::Relaxed);
    b.frame.store(entry_top - RETURN_FRAME, Ordering::Relaxed);
    b.flags.store(flags, Ordering::Relaxed);
    slot
}

/// The entry block at the start of `slot`.
pub fn block(slot: u64) -> &'static Block {
    // SAFETY: the slot's first page is mapped for as long as the kernel runs, and holds the
    // block at its start; its fields are atomics, so sharing it is sound.
    unsafe { &*(slot as *const Block) }
}

/// Where the slot's GDT and TSS are.
pub fn gdt_and_tss(slot: u64) -> (u64, u64) {
    (slot + GDT_OFFSET, slot + TSS_OFFSET)
}

/// The top of the slot's entry stack, and of its three IST stacks.
pub fn stack_tops(slot: u64) -> (u64, [u64; 3]) {
    let top = |(first, pages): (u64, u64)| slot + (first + pages) * PAGE;
    (top(ENTRY_STACK), IST_STACKS.map(top))
}

/// Every core's slot is in one 1 GiB region, so `cpu::init_this_core` can tell from an address
/// whether it is one.
pub fn is_slot(addr: u64) -> bool {
    (SLOTS..AREA_END).contains(&addr)
}

// The entry code. Everything from `__entry_start` to `__entry_end` runs at `CODE`, so it jumps
// only within itself (relative jumps come out the same) and reaches the rest of the kernel by
// absolute address, once the kernel's tables are loaded.
//
// Each vector's stub is 16 bytes long, so the IDT can be filled in from one base address. Stubs
// for exceptions without an error code push a zero in its place, so every frame looks alike.
core::arch::global_asm!(
    ".section .text.pbr_entry, \"ax\"",
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
    // Double faults, NMIs and machine checks have their own stacks and their own path.
    ".if (vec == 2) || (vec == 8) || (vec == 18)",
    "jmp pbr_ist_common",
    ".else",
    "jmp pbr_isr_common",
    ".endif",
    ".set vec, vec + 1",
    ".endr",
    // An interrupt or exception. From a program (the saved CS has privilege 3), the processor
    // has switched to this core's entry stack (the TSS says so): switch `GS` and the page
    // tables to the kernel's, and move the frame to the thread's kernel stack. `lfence` on
    // both paths stops the processor guessing past the test and running on with the wrong
    // `GS` (the "SWAPGS" variant of Spectre).
    "pbr_isr_common:",
    "test byte ptr [rsp + 24], 3",
    "jz 1f",
    "swapgs",
    "lfence",
    "mov gs:[40], rax",
    "test byte ptr gs:[32], 1",
    "jz 2f",
    "mov rax, cr3",
    "and rax, -4097", // bit 12 clear: the kernel's copy
    "mov cr3, rax",
    "2:",
    "mov rax, rsp",
    "mov rsp, gs:[16]",
    "push qword ptr [rax + 48]", // ss
    "push qword ptr [rax + 40]", // rsp
    "push qword ptr [rax + 32]", // rflags
    "push qword ptr [rax + 24]", // cs
    "push qword ptr [rax + 16]", // rip
    "push qword ptr [rax + 8]",  // error code
    "push qword ptr [rax]",      // vector
    "mov rax, gs:[40]",
    "jmp 3f",
    "1:",
    "lfence",
    "3:",
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
    "movabs rax, offset {dispatch}",
    "call rax",
    // Where every return from the kernel, to the kernel or to a program, goes, with the
    // registers on the kernel stack.
    ".global pbr_isr_exit",
    "pbr_isr_exit:",
    "test byte ptr [rsp + 144], 3", // the saved CS, after 15 registers, vector and error
    "jz 4f",
    // To a program: copy the return frame to the entry stack while there are registers to
    // spare, restore them, then switch to the program's page tables and `GS`, clear the
    // buffers (MDS), and return.
    "mov rax, gs:[24]",
    "mov rbx, [rsp + 136]",
    "mov [rax], rbx",
    "mov rbx, [rsp + 144]",
    "mov [rax + 8], rbx",
    "mov rbx, [rsp + 152]",
    "mov [rax + 16], rbx",
    "mov rbx, [rsp + 160]",
    "mov [rax + 24], rbx",
    "mov rbx, [rsp + 168]",
    "mov [rax + 32], rbx",
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
    "mov rsp, gs:[24]",
    "test byte ptr gs:[32], 1",
    "jz 5f",
    "push rax",
    "mov rax, cr3",
    "or rax, 4096", // bit 12 set: the program's copy
    "mov cr3, rax",
    "pop rax",
    "5:",
    "test byte ptr gs:[32], 2",
    "swapgs", // doesn't change the flags the test set
    "jz 6f",
    // With the microcode's MD_CLEAR, this old instruction also empties the buffers.
    "verw word ptr [rip + pbr_entry_data_selector]",
    "6:",
    "iretq",
    // To the kernel: just return.
    "4:",
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
    // A double fault, NMI or machine check, on its own stack. These can arrive at any
    // instruction, including between a system call's `syscall` and its switch of tables, where
    // the saved privilege level says "kernel" but `GS` and `CR3` are still the program's. So
    // they don't trust it: they look at `GS` and `CR3` themselves, switch to the kernel's if
    // either is a program's, and on the way out put back exactly what they found.
    "pbr_ist_common:",
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
    "mov ecx, 0xC0000101", // IA32_GS_BASE
    "rdmsr",
    "xor ebx, ebx",
    "test edx, edx",
    "js 1f", // a kernel address: the kernel's GS already
    "swapgs",
    "mov ebx, 1",
    "1:",
    "lfence",
    "mov r12, cr3",
    "mov rax, r12",
    "and rax, -4097",
    "cmp rax, r12",
    "je 2f",
    "mov cr3, rax",
    "2:",
    "mov r13, rbx",
    "mov rdi, rsp",
    "cld",
    "movabs rax, offset {dispatch}",
    "call rax",
    "mov rax, cr3",
    "cmp rax, r12",
    "je 3f",
    "mov cr3, r12",
    "3:",
    "test r13, 1",
    "jz 4f",
    "test byte ptr gs:[32], 2",
    "swapgs",
    "jz 4f",
    "verw word ptr [rip + pbr_entry_data_selector]",
    "4:",
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
    // A system call. `syscall` leaves the stack pointer as the program's, so it's kept in the
    // entry block and the stack pointer itself is the scratch register for switching tables.
    ".global pbr_syscall_entry",
    "pbr_syscall_entry:",
    "swapgs",
    "mov gs:[8], rsp",
    "test byte ptr gs:[32], 1",
    "jz 1f",
    "mov rsp, cr3",
    "and rsp, -4097",
    "mov cr3, rsp",
    "1:",
    "mov rsp, gs:[16]",
    // The same frame an interrupt from a program makes.
    "push {ss}",
    "push qword ptr gs:[8]",
    "push r11",
    "push {cs}",
    "push rcx",
    "push 0",
    "push 0x100",
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
    "movabs rax, offset {syscall}",
    "call rax",
    "jmp pbr_isr_exit",
    // The kernel's data selector, for `verw`: it only needs a valid selector to check.
    ".balign 2",
    "pbr_entry_data_selector:",
    ".word 0x10",
    dispatch = sym super::interrupts::dispatch,
    syscall = sym super::user::syscall_dispatch,
    ss = const super::user::USER_SS,
    cs = const super::user::USER_CS,
);

/// Tells the processor this core's kernel stack: the next entry from a program copies its frame
/// there.
pub fn set_kernel_stack(slot: u64, top: u64) {
    block(slot).kernel_sp.store(top, Ordering::Relaxed);
}

/// Makes a program's own top-level table at `user` from the kernel's `kernel`: the entry
/// area's entry, and nothing else of the kernel's half.
pub fn fill_user_copy(kernel: u64, user: u64) {
    // SAFETY: both are top-level page-table pages, reached through the direct map; the
    // program's is new, and the kernel's entry for the area never changes.
    unsafe {
        let k = &*phys_ptr::<[u64; 512]>(kernel);
        let u = &mut *phys_ptr::<[u64; 512]>(user);
        u[TOP_INDEX] = k[TOP_INDEX];
    }
}

/// True if a program's own table at `user` maps nothing of the kernel's half but the entry
/// area, and (without SMEP) the kernel's copy at `kernel` marks the program's half
/// no-execute at the top (the kernel's self-test, note 0027, section 9).
pub fn check_user_copy(kernel: u64, user: u64, smep: bool) -> Result<(), &'static str> {
    // SAFETY: as for `fill_user_copy`; only read.
    let (k, u) = unsafe { (&*phys_ptr::<[u64; 512]>(kernel), &*phys_ptr::<[u64; 512]>(user)) };
    if (256..512).any(|i| i != TOP_INDEX && u[i] != 0) {
        return Err("a program's page tables map the kernel");
    }
    if u[TOP_INDEX] != k[TOP_INDEX] {
        return Err("a program's page tables don't map the entry area");
    }
    let nx = |e: u64| e & (1 << 63) != 0;
    for i in 0..256 {
        if u[i] != 0 && (k[i] & !(1 << 63)) != u[i] {
            return Err("the program's half differs between its two tables");
        }
        if u[i] != 0 && !smep && !nx(k[i]) {
            return Err("without SMEP, the kernel's copy of a program's half is executable");
        }
        if nx(u[i]) {
            return Err("the program's own table marks its half no-execute");
        }
    }
    Ok(())
}
