// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Programs on x86-64: the system call entry, entering user mode, programs' floating-point
//! and vector registers, switching address spaces and keeping every core's TLB right, and the
//! system page's code (docs/design/0007-system-calls.md).
//!
//! On entry from a program the kernel swaps `GS` (`swapgs`) so the per-core data is reachable;
//! the program's own `GS` base waits in `IA32_KERNEL_GS_BASE` until it returns. Programs always
//! return through `iretq`, never `sysret`, which avoids `sysret`'s known pitfalls.

use super::cpu::{rdmsr, wrmsr};
use super::interrupts::{self, Frame};
use super::{apic, clock};
use crate::{percpu, sched};
use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;
const IA32_FS_BASE: u32 = 0xC000_0100;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// The user-mode code and data selectors (GDT entries 4 and 3, privilege level 3).
const USER_CS: u64 = 0x20 | 3;
const USER_SS: u64 = 0x18 | 3;

/// x86-64 shares one page-table tree between the kernel and a program.
pub const SHARES_TOP_LEVEL: bool = true;
/// The ELF machine number programs must have.
pub const ELF_MACHINE: u16 = 62;

static XSAVE: AtomicBool = AtomicBool::new(false);
static FP_SIZE: AtomicU64 = AtomicU64::new(512);
static SMAP: AtomicBool = AtomicBool::new(false);
static SMEP: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------------------------------------
// Per-core set-up

/// Sets up this core for programs: the `syscall` instruction, floating point, and the
/// protections that stop the kernel touching programs' memory by accident.
pub fn init_this_core() {
    let id1 = core::arch::x86_64::__cpuid(1);
    let id7 = core::arch::x86_64::__cpuid_count(7, 0);
    let has_xsave = id1.ecx & (1 << 26) != 0;
    let has_smep = id7.ebx & (1 << 7) != 0;
    let has_smap = id7.ebx & (1 << 20) != 0;
    // SAFETY: control register and MSR writes that only turn on features the processor says
    // it has, plus the standard system call set-up.
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1 << 2); // EM: floating point is real
        cr0 &= !(1 << 3); // TS: don't trap on its first use
        cr0 |= 1 << 1; // MP
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR, OSXMMEXCPT: SSE for programs
        if has_xsave {
            cr4 |= 1 << 18; // OSXSAVE
        }
        if has_smep {
            cr4 |= 1 << 20;
        }
        if has_smap {
            cr4 |= 1 << 21;
        }
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
        if has_xsave {
            // Every user state component this processor has, of x87, SSE, AVX and AVX-512.
            let supported = core::arch::x86_64::__cpuid_count(0xD, 0);
            let mask = u64::from(supported.eax) & 0b1110_0111;
            core::arch::asm!("xsetbv", in("ecx") 0u32, in("eax") mask as u32, in("edx") (mask >> 32) as u32, options(nomem, nostack));
            let size = core::arch::x86_64::__cpuid_count(0xD, 0).ebx;
            FP_SIZE.store(u64::from(size).max(576), Ordering::Relaxed);
        }
        wrmsr(IA32_EFER, rdmsr(IA32_EFER) | 1); // SCE: allow `syscall`
        // Kernel CS 0x08 (SS 0x10); for `sysret` (unused) CS 0x20 and SS 0x18.
        wrmsr(IA32_STAR, (0x10u64 << 48) | (0x08u64 << 32));
        wrmsr(IA32_LSTAR, pbr_syscall_entry as *const () as u64);
        // Mask TF, IF, DF, NT and AC on entry.
        wrmsr(IA32_FMASK, (1 << 8) | (1 << 9) | (1 << 10) | (1 << 14) | (1 << 18));
    }
    XSAVE.store(has_xsave, Ordering::Relaxed);
    SMEP.store(has_smep, Ordering::Relaxed);
    SMAP.store(has_smap, Ordering::Relaxed);
}

/// The protections in force, for the start-up report.
pub fn describe_protections() -> (&'static str, bool, bool) {
    (
        if XSAVE.load(Ordering::Relaxed) { "XSAVE" } else { "FXSAVE" },
        SMEP.load(Ordering::Relaxed),
        SMAP.load(Ordering::Relaxed),
    )
}

/// Records the top of the current thread's kernel stack, where entries from a program start.
pub fn set_kernel_stack(top: u64) {
    let cpu = percpu::this();
    cpu.kernel_sp.store(top, Ordering::Relaxed);
    super::cpu::set_tss_rsp0(cpu, top);
}

// ------------------------------------------------------------------------------------------
// Saved registers

/// A program thread's registers that a thread switch doesn't save: floating point and vector
/// registers, and the `FS` and `GS` bases.
pub struct UserRegs {
    fp: *mut u8,
    size: usize,
    fs: AtomicU64,
    gs: AtomicU64,
    _not_sync: UnsafeCell<()>,
}

// SAFETY: the save area is only touched by the core switching to or from the thread, with
// interrupts masked.
unsafe impl Send for UserRegs {}
// SAFETY: as above.
unsafe impl Sync for UserRegs {}

impl UserRegs {
    /// A clean set: everything zero, floating point in its initial state.
    pub fn new() -> Option<Self> {
        let size = FP_SIZE.load(Ordering::Relaxed) as usize;
        let layout = Layout::from_size_align(size, 64).ok()?;
        // SAFETY: a non-zero size.
        let fp = unsafe { alloc_zeroed(layout) };
        if fp.is_null() {
            return None;
        }
        // SAFETY: inside the area just allocated. The x87 control word and MXCSR at their
        // power-on values; the XSAVE header (all zero) means "initial state" for the rest.
        unsafe {
            (fp as *mut u16).write(0x037F);
            (fp.add(24) as *mut u32).write(0x1F80);
        }
        Some(UserRegs {
            fp,
            size,
            fs: AtomicU64::new(0),
            gs: AtomicU64::new(0),
            _not_sync: UnsafeCell::new(()),
        })
    }
}

impl Drop for UserRegs {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.size, 64).expect("valid when allocated");
        // SAFETY: allocated in `new` with this layout.
        unsafe { dealloc(self.fp, layout) };
    }
}

/// Saves the current program thread's floating-point state and segment bases into `r`.
pub fn save_user(r: &UserRegs) {
    // SAFETY: the area is big enough and 64-byte aligned; the MSRs exist.
    unsafe {
        if XSAVE.load(Ordering::Relaxed) {
            core::arch::asm!("xsave64 [{}]", in(reg) r.fp, in("eax") u32::MAX, in("edx") u32::MAX, options(nostack));
        } else {
            core::arch::asm!("fxsave64 [{}]", in(reg) r.fp, options(nostack));
        }
        r.fs.store(rdmsr(IA32_FS_BASE), Ordering::Relaxed);
        r.gs.store(rdmsr(IA32_KERNEL_GS_BASE), Ordering::Relaxed);
    }
}

/// Restores what [`save_user`] saved.
pub fn load_user(r: &UserRegs) {
    // SAFETY: as for `save_user`; the area holds a valid saved state (or the clean one).
    unsafe {
        if XSAVE.load(Ordering::Relaxed) {
            core::arch::asm!("xrstor64 [{}]", in(reg) r.fp, in("eax") u32::MAX, in("edx") u32::MAX, options(nostack));
        } else {
            core::arch::asm!("fxrstor64 [{}]", in(reg) r.fp, options(nostack));
        }
        wrmsr(IA32_FS_BASE, r.fs.load(Ordering::Relaxed));
        wrmsr(IA32_KERNEL_GS_BASE, r.gs.load(Ordering::Relaxed));
    }
}

/// Sets the program's thread pointer (the `FS` base), now and for when it next resumes.
pub fn set_thread_pointer(r: &UserRegs, value: u64) {
    r.fs.store(value, Ordering::Relaxed);
    // SAFETY: `value` is a lower-half address, so canonical.
    unsafe { wrmsr(IA32_FS_BASE, value) };
}

// ------------------------------------------------------------------------------------------
// Entering the kernel from a program, and leaving it

core::arch::global_asm!(
    ".section .text.pbr_syscall, \"ax\"",
    ".global pbr_syscall_entry",
    "pbr_syscall_entry:",
    "swapgs",
    "mov gs:[{user_sp}], rsp",
    "mov rsp, gs:[{kernel_sp}]",
    // The same frame an interrupt from user mode makes.
    "push {ss}",
    "push qword ptr gs:[{user_sp}]",
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
    "call {dispatch}",
    "jmp pbr_isr_exit",
    // pbr_enter_user(frame = rdi): starts a program with the registers in a frame built on
    // this stack.
    ".global pbr_enter_user",
    "pbr_enter_user:",
    "mov rsp, rdi",
    "jmp pbr_isr_exit",
    user_sp = const percpu::OFFSET_USER_SP,
    kernel_sp = const percpu::OFFSET_KERNEL_SP,
    ss = const USER_SS,
    cs = const USER_CS,
    dispatch = sym syscall_dispatch,
);

unsafe extern "C" {
    fn pbr_syscall_entry();
    fn pbr_enter_user(frame: *const Frame) -> !;
}

extern "C" fn syscall_dispatch(f: &mut Frame) {
    super::irq_enable();
    let (status, r1, r2) = crate::syscall::handle(f.rax, [f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9]);
    f.rax = status;
    f.rdx = r1;
    f.rsi = r2;
    super::irq_disable();
    returning_to_user();
}

/// The last things done before going back to a program: give way if another thread should
/// run, and exit instead if this thread has been asked to stop.
pub fn returning_to_user() {
    sched::interrupt_exit();
    crate::user::before_return();
}

/// Starts the current thread in user mode at `pc`, with stack `sp`, its argument in `rdi`, the
/// system page in `rsi` and its message page in `rdx` (note 0007, section 3).
pub fn enter_user(pc: u64, sp: u64, arg: u64, sys: u64, msg: u64) -> ! {
    super::irq_disable();
    let top = sched::current().stack_top().expect("a program thread has a kernel stack");
    // The frame goes at the top of the kernel stack, as an entry from the program would put it.
    let frame = (top - core::mem::size_of::<Frame>() as u64) as *mut Frame;
    // SAFETY: the top of this thread's own kernel stack, which nothing else uses: this
    // function's own frame is further down.
    unsafe {
        frame.write(Frame::user(pc, sp, USER_CS, USER_SS, 0x202, [arg, sys, msg]));
        pbr_enter_user(frame)
    }
}

// ------------------------------------------------------------------------------------------
// Address spaces and the TLB

/// Loads a program's page tables (or, for 0, the kernel's alone).
pub fn activate_space(root: u64) {
    let r = if root == 0 { crate::mem::vm::root() } else { root };
    // SAFETY: both kinds of tree map the kernel's upper half identically.
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) r, options(nostack)) };
}

static SHOOTDOWN: crate::sync::SpinLock<()> = crate::sync::SpinLock::new(());
static PENDING: AtomicU32 = AtomicU32::new(0);

/// Called by the shootdown interrupt: forgets every program translation on this core.
pub fn shootdown_interrupt() {
    // SAFETY: reloading CR3 only discards cached translations.
    unsafe {
        core::arch::asm!("mov {t}, cr3", "mov cr3, {t}", t = out(reg) _, options(nostack));
    }
    PENDING.fetch_sub(1, Ordering::SeqCst);
}

/// Makes every core forget cached translations of `pages` pages from `start` in the program
/// page tables `root`, after they've changed. Returns once they all have.
pub fn flush_user_pages(root: u64, start: u64, pages: u64) {
    let me = percpu::this();
    if me.loaded_root.load(Ordering::SeqCst) == root {
        if pages > 32 {
            activate_space(root);
        } else {
            for i in 0..pages {
                super::tlb_flush_page(start + i * crate::mem::PAGE);
            }
        }
    }
    let targets: alloc::vec::Vec<&percpu::PerCpu> = percpu::all()
        .iter()
        .copied()
        .filter(|c| {
            c.index != me.index
                && c.online.load(Ordering::Acquire)
                && c.loaded_root.load(Ordering::SeqCst) == root
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    // One shootdown at a time. Waiting is done with interrupts on, so a core waiting here can
    // still answer another's shootdown.
    let _g = SHOOTDOWN.lock();
    PENDING.store(targets.len() as u32, Ordering::SeqCst);
    for c in &targets {
        apic::send(c.hw_id as u32, u32::from(interrupts::IPI_SHOOTDOWN));
    }
    while PENDING.load(Ordering::SeqCst) != 0 {
        core::hint::spin_loop();
    }
}

// ------------------------------------------------------------------------------------------
// The system page (note 0007, section 1). Copied into a page of its own at start-up; the
// data page follows it in every program.

core::arch::global_asm!(
    ".section .rodata.pbr_syspage, \"a\"",
    ".balign 16",
    ".global pbr_syspage_start",
    "pbr_syspage_start:",
    ".ascii \"PBRSYSPG\"",
    ".short 1, 0",
    ".long 0",
    ".quad 64",
    ".quad 96",
    ".fill 32, 1, 0",
    // 64: call
    "syscall",
    "ret",
    ".fill 29, 1, 0xCC",
    // 96: clock_now
    "lea r8, [rip + pbr_syspage_start + 4096]",
    "mov rax, [r8]",
    "test rax, rax",
    "jz 1f",
    "lfence",
    "rdtsc",
    "shl rdx, 32",
    "or rax, rdx",
    "sub rax, [r8 + 8]",
    "mul qword ptr [r8 + 16]",
    "shrd rax, rdx, 32",
    "ret",
    "1:",
    "mov eax, 2",
    "syscall",
    "mov rax, rdx",
    "ret",
    ".global pbr_syspage_end",
    "pbr_syspage_end:",
);

unsafe extern "C" {
    static pbr_syspage_start: u8;
    static pbr_syspage_end: u8;
}

/// The system page's contents.
pub fn syspage_code() -> &'static [u8] {
    let s = &raw const pbr_syspage_start;
    let e = &raw const pbr_syspage_end;
    // SAFETY: the bytes between the two symbols, in the kernel's read-only data.
    unsafe { core::slice::from_raw_parts(s, e as usize - s as usize) }
}

/// True if programs can read the clock's counter themselves (an invariant TSC).
pub fn user_clock() -> bool {
    clock::is_tsc()
}
