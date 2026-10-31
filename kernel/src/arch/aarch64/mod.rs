// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! AArch64 (64-bit Arm), as used by the Raspberry Pi 5.

mod context;
mod exceptions;
mod gic;
mod smp;
mod timer;
mod user;

pub use context::{init_thread_stack, switch_context, switch_stack};
pub use smp::start_other_cores;
pub use user::{
    ELF_MACHINE, SHARES_TOP_LEVEL, UserRegs, activate_space, enter_user, flush_user_pages,
    load_user, save_user, set_kernel_stack, set_thread_pointer, syspage_code, user_clock,
};

use crate::{hw, percpu, time};
use core::sync::atomic::{AtomicU32, Ordering};

/// The name used in messages.
pub const NAME: &str = "ARM64";

/// The page-table format.
pub const PAGE_FORMAT: pbr_platform::paging::Format = pbr_platform::paging::Format::Aarch64;

/// The kernel's entry point. The loader jumps here at exception level 1, with the MMU on,
/// interrupts masked, a valid stack, the boot information's physical address in `x0` and the
/// direct map's base in `x1` (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64, direct_map_base: u64) -> ! {
    crate::kernel_main(boot_info, direct_map_base)
}

/// Stops this core for good.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: masking interrupts and waiting only stops this core.
        unsafe { core::arch::asm!("msr daifset, #0xf", "wfi", options(nomem, nostack)) };
    }
}

/// Whether interrupts were enabled, as saved by [`irq_save`].
#[derive(Clone, Copy)]
pub struct IrqState(u64);

/// Masks interrupts on this core, returning the previous state.
pub fn irq_save() -> IrqState {
    let daif: u64;
    // SAFETY: reads DAIF and masks IRQs; nothing else.
    unsafe {
        core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nomem, nostack))
    };
    IrqState(daif)
}

/// Restores what [`irq_save`] saved.
pub fn irq_restore(s: IrqState) {
    if s.0 & (1 << 7) == 0 {
        irq_enable();
    }
}

/// Enables interrupts on this core.
pub fn irq_enable() {
    // SAFETY: allowing IRQs is safe once the vectors are installed.
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack)) };
}

/// Masks interrupts on this core.
pub fn irq_disable() {
    // SAFETY: masking IRQs is always safe.
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack)) };
}

/// Waits for an interrupt and lets it run. Called with interrupts masked: WFI still wakes for
/// a pending interrupt, so one arriving just before can't be missed.
pub fn wait_for_interrupt() {
    // SAFETY: waiting, then unmasking so the pending interrupt is taken.
    unsafe { core::arch::asm!("wfi", "msr daifclr, #2", "isb", options(nomem, nostack)) };
}

/// Points `TPIDR_EL1` at this core's per-core data.
pub fn set_percpu(p: u64) {
    // SAFETY: TPIDR_EL1 is reserved for this.
    unsafe { core::arch::asm!("msr tpidr_el1, {}", in(reg) p, options(nomem, nostack)) };
}

/// This core's per-core data pointer, or 0 if not set yet.
pub fn get_percpu() -> u64 {
    let p: u64;
    // SAFETY: reading TPIDR_EL1 has no side effects. (It's zeroed at start-up by `zero_percpu`.)
    unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) p, options(nomem, nostack)) };
    p
}

/// Switches to the kernel's page tables, and stops the lower half being used at all.
pub fn activate_kernel_tables(root: u64) {
    // SAFETY: the new tables map the kernel and the direct map, which is all that's in use; the
    // kernel never uses lower-half addresses.
    unsafe {
        core::arch::asm!(
            "msr ttbr1_el1, {root}",
            "mrs {t}, tcr_el1",
            "orr {t}, {t}, #(1 << 7)", // EPD0: no lower-half walks
            "msr tcr_el1, {t}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            root = in(reg) root,
            t = out(reg) _,
            options(nostack)
        );
    }
}

/// Forgets any cached translation of `virt` on every core.
#[allow(dead_code)] // part of the architecture interface; only x86-64's start-up uses it so far
pub fn tlb_flush_page(virt: u64) {
    // SAFETY: only discards cached translations.
    unsafe {
        core::arch::asm!("dsb ishst", "tlbi vaae1is, {}", "dsb ish", "isb", in(reg) virt >> 12, options(nostack))
    };
}

/// The clock's counter: the generic timer's virtual count.
pub fn clock_counter() -> u64 {
    let c: u64;
    // SAFETY: reading the counter has no side effects.
    unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) c, options(nomem, nostack)) };
    c
}

/// Kinds of inter-core interrupt.
#[derive(Clone, Copy)]
pub enum Ipi {
    /// Look at your run queue.
    Reschedule,
    /// Stop (the kernel has panicked).
    Stop,
}

/// What AArch64 keeps per core: its GIC CPU interface number.
pub struct CpuArch {
    gic_interface: AtomicU32,
}

impl CpuArch {
    /// Not yet known.
    pub fn new() -> Self {
        Self { gic_interface: AtomicU32::new(0) }
    }
}

impl Default for CpuArch {
    fn default() -> Self {
        Self::new()
    }
}

/// Records what the firmware said about a core.
pub fn set_core_info(cpu: &percpu::PerCpu, core: &hw::Core) {
    cpu.arch.gic_interface.store(core.gic_interface, Ordering::Relaxed);
}

/// Sends an inter-core interrupt.
pub fn send_ipi(to: &percpu::PerCpu, kind: Ipi) {
    let sgi = match kind {
        Ipi::Reschedule => gic::SGI_RESCHEDULE,
        Ipi::Stop => gic::SGI_STOP,
    };
    gic::send_sgi(to.arch.gic_interface.load(Ordering::Relaxed), sgi);
}

/// This core's hardware ID: its MPIDR affinity bits.
pub fn this_core_hw_id() -> u64 {
    let m: u64;
    // SAFETY: reading MPIDR_EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) m, options(nomem, nostack)) };
    m & 0xFF_00FF_FFFF
}

/// Clears `TPIDR_EL1`, so [`get_percpu`] means "not set" until it's set. The firmware may
/// leave anything in it.
pub fn zero_percpu() {
    set_percpu(0);
}

/// Sets up the boot core: exceptions, the interrupt controller, the clock and its timer.
pub fn init_boot_core(m: &hw::Machine) {
    exceptions::install();
    user::init_this_core();
    exceptions::set_timer_irq(m.vtimer_irq);
    gic::init(m);
    time::init(timer::frequency());
    timer::start(m.vtimer_irq);
}

/// Executes a breakpoint instruction, which the exception handler counts and steps over.
pub fn breakpoint() {
    // SAFETY: BRK traps to our handler, which returns to the next instruction.
    unsafe { core::arch::asm!("brk #0", options(nomem, nostack)) };
}

/// Breakpoints taken so far.
pub fn breakpoints() -> u64 {
    exceptions::BREAKPOINTS.load(Ordering::Relaxed)
}

/// Facts about the processor worth reporting at start-up.
pub fn describe(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let (el, midr): (u64, u64);
    // SAFETY: reading these registers has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
        core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr, options(nomem, nostack));
    }
    write!(
        out,
        "exception level {}, implementer {:#04x}, part {:#05x}, GICv{}, clock: generic timer at {} MHz{}",
        (el >> 2) & 3,
        midr >> 24,
        (midr >> 4) & 0xFFF,
        gic::version(),
        timer::frequency() / 1_000_000,
        if user::has_pan() { ", PAN" } else { "" }
    )
}

/// Calls the loader's EL2 stub with a request it ignores, to check it's there and answering.
///
/// # Safety
/// Only call this if the boot information has `FLAG_EL2_STUB`: without a stub, `HVC` is an
/// undefined instruction.
pub unsafe fn ping_el2_stub() {
    // SAFETY: the caller promises the stub exists; request 1 does nothing (note 0003).
    unsafe { core::arch::asm!("hvc #0", inout("x0") 1u64 => _, options(nomem, nostack)) };
}
