// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Per-core data (docs/design/0005-kernel-core.md, section 7), reached through `GS` on x86-64
//! and `TPIDR_EL1` on AArch64.

use crate::arch;
use crate::sched::CpuSched;
use crate::sync::{Level, Once};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// One core's data.
#[repr(C)]
pub struct PerCpu {
    /// Points at this structure itself (x86-64 reads it through `GS:[0]`).
    self_ptr: *const PerCpu,
    /// The core's number: 0 for the boot core, then 1, 2, ... in start-up order.
    pub index: u32,
    /// The hardware's name for the core: the x86 APIC ID, or the Arm MPIDR affinity bits.
    pub hw_id: u64,
    /// Set by the core itself once it's running the scheduler.
    pub online: AtomicBool,
    /// Above zero while this core's current thread mustn't be preempted.
    preempt_count: AtomicU32,
    /// Bit `n` set while a lock of level `n` is held on this core (debug builds check the order).
    held_levels: AtomicU32,
    /// Timer interrupts so far on this core.
    pub ticks: AtomicU64,
    /// The scheduler's state for this core.
    pub sched: CpuSched,
    /// What the architecture layer keeps per core.
    pub arch: arch::CpuArch,
}

// SAFETY: the fields other cores may touch are atomics or locked; the rest is only used by the
// core itself.
unsafe impl Sync for PerCpu {}
// SAFETY: as above.
unsafe impl Send for PerCpu {}

static CPUS: Once<Vec<&'static PerCpu>> = Once::new();

/// Creates the per-core data for every core (`hw_ids[0]` is the boot core).
pub fn create(hw_ids: &[u64]) {
    let mut all = Vec::new();
    for (i, &hw) in hw_ids.iter().enumerate() {
        let p = Box::leak(Box::new(PerCpu {
            self_ptr: core::ptr::null(),
            index: i as u32,
            hw_id: hw,
            online: AtomicBool::new(false),
            preempt_count: AtomicU32::new(0),
            held_levels: AtomicU32::new(0),
            ticks: AtomicU64::new(0),
            sched: CpuSched::new(),
            arch: arch::CpuArch::new(),
        }));
        p.self_ptr = p;
        all.push(&*p);
    }
    CPUS.set(all);
}

/// Every core's data, in core-number order.
pub fn all() -> &'static [&'static PerCpu] {
    CPUS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// The number of cores (whether or not they're online yet).
pub fn count() -> usize {
    all().len()
}

/// Makes `cpu` this core's data. Called once on each core, early.
pub fn install(cpu: &'static PerCpu) {
    arch::set_percpu(cpu as *const PerCpu as u64);
}

/// This core's data, or `None` before [`install`].
pub fn try_this() -> Option<&'static PerCpu> {
    let p = arch::get_percpu();
    // SAFETY: a non-zero value is only ever set by `install`, to a leaked (so 'static) PerCpu.
    (p != 0).then(|| unsafe { &*(p as *const PerCpu) })
}

/// This core's data.
pub fn this() -> &'static PerCpu {
    try_this().expect("per-core data not set up")
}

/// Stops the current thread being preempted until [`preempt_enable`].
pub fn preempt_disable() {
    // Masked, so the thread can't move to another core between finding "this core" and
    // counting on it.
    let irq = arch::irq_save();
    if let Some(c) = try_this() {
        c.preempt_count.fetch_add(1, Ordering::Relaxed);
    }
    arch::irq_restore(irq);
}

/// Undoes one [`preempt_disable`].
pub fn preempt_enable() {
    let irq = arch::irq_save();
    if let Some(c) = try_this() {
        c.preempt_count.fetch_sub(1, Ordering::Relaxed);
    }
    arch::irq_restore(irq);
}

/// True if the current thread may be preempted now.
pub fn preemptible() -> bool {
    try_this().is_some_and(|c| {
        c.preempt_count.load(Ordering::Relaxed) == 0 && c.held_levels.load(Ordering::Relaxed) == 0
    })
}

/// Records that a lock of `level` is being taken on this core, checking the lock order in
/// debug builds. Interrupts are already masked.
pub fn lock_level_taken(level: Level, name: &str) {
    let Some(c) = try_this() else { return };
    let bit = 1u32 << level as u32;
    if crate::panicking() {
        return; // reporting a panic mustn't trip over the locks the panicking code held
    }
    let held = c.held_levels.load(Ordering::Relaxed);
    if cfg!(debug_assertions) && held >= bit {
        let highest = 31 - held.leading_zeros();
        panic!(
            "lock order broken: taking \"{name}\" (level {}) while holding a lock of level {highest}",
            level as u32
        );
    }
    c.held_levels.store(held | bit, Ordering::Relaxed);
}

/// Records that a lock of `level` was released on this core.
pub fn lock_level_released(level: Level) {
    if let Some(c) = try_this() {
        c.held_levels.fetch_and(!(1u32 << level as u32), Ordering::Relaxed);
    }
}
