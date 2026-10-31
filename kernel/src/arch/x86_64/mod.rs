// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! x86-64 (AMD64 / Intel 64).

mod apic;
mod clock;
mod context;
mod cpu;
mod interrupts;
mod serial;
mod smp;
mod user;

pub use context::{init_thread_stack, switch_context, switch_stack};
pub use cpu::CpuArch;
pub use serial::Port16550;
pub use smp::start_other_cores;
pub use user::{
    ELF_MACHINE, SHARES_TOP_LEVEL, UserRegs, activate_space, enter_user, flush_user_pages,
    load_user, save_user, set_kernel_stack, set_thread_pointer, syspage_code, user_clock,
};

use crate::{hw, percpu, time};

/// The name used in messages.
pub const NAME: &str = "x86-64";

/// The page-table format.
pub const PAGE_FORMAT: pbr_platform::paging::Format = pbr_platform::paging::Format::X86_64;

/// The kernel's entry point. The loader jumps here in 64-bit mode, with paging on, interrupts
/// off, a valid stack, the boot information's physical address in `rdi` and the direct map's
/// base in `rsi` (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64, direct_map_base: u64) -> ! {
    crate::kernel_main(boot_info, direct_map_base)
}

/// Stops this core for good.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this core.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Whether interrupts were enabled, as saved by [`irq_save`].
#[derive(Clone, Copy)]
pub struct IrqState(bool);

/// Masks interrupts on this core, returning whether they were enabled.
pub fn irq_save() -> IrqState {
    let flags: u64;
    // SAFETY: reads RFLAGS and clears IF; nothing else.
    unsafe { core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem)) };
    IrqState(flags & (1 << 9) != 0)
}

/// Restores what [`irq_save`] saved.
pub fn irq_restore(s: IrqState) {
    if s.0 {
        irq_enable();
    }
}

/// Enables interrupts on this core.
pub fn irq_enable() {
    // SAFETY: allowing interrupts is always safe once the IDT is loaded.
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}

/// Masks interrupts on this core.
pub fn irq_disable() {
    // SAFETY: masking interrupts is always safe.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// Enables interrupts and waits for one, as a single step (so one arriving just before can't
/// be missed).
pub fn wait_for_interrupt() {
    // SAFETY: `sti; hlt` is the standard way to wait; the interrupt runs before `hlt` returns.
    unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
}

const IA32_GS_BASE: u32 = 0xC000_0101;

/// Points this core's `GS` at its per-core data.
pub fn set_percpu(p: u64) {
    // SAFETY: GS isn't otherwise used by the kernel.
    unsafe { cpu::wrmsr(IA32_GS_BASE, p) };
}

/// Clears the `GS` base, so [`get_percpu`] means "not set" until it's set. The firmware may
/// leave anything in it.
pub fn zero_percpu() {
    set_percpu(0);
}

/// Records what the firmware said about a core (nothing extra is needed on x86-64).
pub fn set_core_info(_cpu: &percpu::PerCpu, _core: &hw::Core) {}

/// This core's per-core data pointer, or 0 if not set yet.
pub fn get_percpu() -> u64 {
    // GS's base is read with RDMSR rather than through GS, so it's safe before it's set.
    // SAFETY: reading the GS base MSR has no side effects.
    unsafe { cpu::rdmsr(IA32_GS_BASE) }
}

/// Switches to the kernel's page tables.
pub fn activate_kernel_tables(root: u64) {
    // SAFETY: the new tables map the kernel and the direct map, which is all that's in use.
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack)) };
}

/// Forgets any cached translation of `virt` on this core.
pub fn tlb_flush_page(virt: u64) {
    // SAFETY: INVLPG only discards a cached translation.
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack)) };
}

/// The clock's counter (see `crate::time`).
pub fn clock_counter() -> u64 {
    clock::counter()
}

/// Kinds of inter-core interrupt.
#[derive(Clone, Copy)]
pub enum Ipi {
    /// Look at your run queue.
    Reschedule,
    /// Stop (the kernel has panicked).
    Stop,
}

/// Sends an inter-core interrupt.
pub fn send_ipi(to: &percpu::PerCpu, kind: Ipi) {
    let v = match kind {
        Ipi::Reschedule => interrupts::IPI_RESCHEDULE,
        Ipi::Stop => interrupts::IPI_STOP,
    };
    apic::send(to.hw_id as u32, u32::from(v));
}

/// This core's hardware ID, as the firmware tables list it.
pub fn this_core_hw_id() -> u64 {
    u64::from(core::arch::x86_64::__cpuid(1).ebx >> 24)
}

/// Sets up the boot core: exceptions, the interrupt controller, the clock and its timer.
pub fn init_boot_core(m: &hw::Machine) {
    interrupts::build_idt();
    cpu::init_this_core(percpu::this());
    apic::init(m.lapic);
    let freq = clock::init(m.hpet);
    time::init(freq);
    apic::start_timer();
}

/// Executes a breakpoint instruction, which the exception handler counts and steps over.
pub fn breakpoint() {
    // SAFETY: INT3 traps to our handler, which returns straight after it.
    unsafe { core::arch::asm!("int3", options(nomem, nostack)) };
}

/// Breakpoints taken so far.
pub fn breakpoints() -> u64 {
    interrupts::BREAKPOINTS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Facts about the processor worth reporting at start-up.
pub fn describe(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let mut vendor = [0u8; 12];
    let r = core::arch::x86_64::__cpuid(0);
    vendor[..4].copy_from_slice(&r.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&r.edx.to_le_bytes());
    vendor[8..].copy_from_slice(&r.ecx.to_le_bytes());
    write!(
        out,
        "{}, {}",
        core::str::from_utf8(&vendor).unwrap_or("unknown vendor"),
        apic::describe()
    )?;
    write!(out, ", clock: {}", clock::describe())?;
    let (fp, smep, smap) = user::describe_protections();
    write!(out, ", {fp}{}{}", if smep { ", SMEP" } else { "" }, if smap { ", SMAP" } else { "" })
}
