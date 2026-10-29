// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Locks that work across processor cores (docs/design/0005-kernel-core.md, section 9).
//!
//! - [`SpinLock`] is for data no interrupt handler touches. While it's held, the thread
//!   holding it can't be preempted, but interrupts still happen.
//! - [`IrqSpinLock`] also masks interrupts on this core while it's held. Use it for anything
//!   an interrupt handler touches.
//!
//! Every [`IrqSpinLock`] has a [`Level`]. In debug builds, taking a lock whose level isn't
//! above every level already held on this core is reported as a bug: that's how the lock
//! order is enforced rather than just written down.

use crate::{arch, percpu};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A lock's place in the lock order: a lock may only be taken while holding locks of lower
/// levels. The order is in docs/design/0005-kernel-core.md, section 9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// The list of every thread, for diagnostics (taken before anything else).
    ThreadList = 0,
    /// Wait queues (released before the scheduler is entered).
    WaitQueue = 1,
    /// A core's run queue.
    RunQueue = 2,
    /// A core's timer list.
    Timers = 3,
    /// The kernel's page tables and virtual areas.
    KernelMemory = 4,
    /// The kernel heap (which takes pages from the page allocator).
    Heap = 5,
    /// The physical page allocator.
    PageAllocator = 6,
    /// The console.
    Console = 7,
}

fn raw_lock(flag: &AtomicBool) {
    while flag.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while flag.load(Ordering::Relaxed) {
            core::hint::spin_loop();
        }
    }
}

/// A lock that waits by spinning, and stops the holder being preempted. For short critical
/// sections only.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock gives one core at a time access to the value, so sharing the lock between
// cores is sound as long as the value itself may be sent between them.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// A new, unlocked lock.
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// Waits for the lock and takes it.
    pub fn lock(&self) -> SpinGuard<'_, T> {
        percpu::preempt_disable();
        raw_lock(&self.locked);
        SpinGuard { lock: self }
    }
}

/// Access to a locked [`SpinLock`]'s value; unlocks when dropped.
pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the guard means holding the lock, so no one else has access.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`, and `&mut self` means this is the only use of the guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        percpu::preempt_enable();
    }
}

/// A spin lock that also masks interrupts on this core while it's held.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    level: Level,
    name: &'static str,
    value: UnsafeCell<T>,
}

// SAFETY: as for SpinLock.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// A new, unlocked lock at `level` in the lock order. `name` appears in lock-order reports.
    pub const fn new(level: Level, name: &'static str, value: T) -> Self {
        Self { locked: AtomicBool::new(false), level, name, value: UnsafeCell::new(value) }
    }

    /// Masks interrupts, waits for the lock and takes it.
    pub fn lock(&self) -> IrqGuard<'_, T> {
        let irq = arch::irq_save();
        percpu::lock_level_taken(self.level, self.name);
        raw_lock(&self.locked);
        IrqGuard { lock: self, irq }
    }

    /// Releases the lock without a guard, for when its holder will never run again.
    ///
    /// # Safety
    /// Only when the system is stopping (a panic): whoever held it mustn't touch the value again.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Takes the lock only if it's free right now.
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let irq = arch::irq_save();
        if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            percpu::lock_level_taken(self.level, self.name);
            Some(IrqGuard { lock: self, irq })
        } else {
            arch::irq_restore(irq);
            None
        }
    }
}

/// Access to a locked [`IrqSpinLock`]'s value; unlocks, and restores interrupts, when dropped.
pub struct IrqGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    irq: arch::IrqState,
}

impl<T> Deref for IrqGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the guard means holding the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        percpu::lock_level_released(self.lock.level);
        arch::irq_restore(self.irq);
    }
}

/// A value set once, early, and read freely afterwards.
pub struct Once<T> {
    set: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: the value is written once, before `set` is published with Release ordering, and only
// read after `set` is seen with Acquire ordering.
unsafe impl<T: Send + Sync> Sync for Once<T> {}

impl<T> Once<T> {
    /// Not yet set.
    pub const fn new() -> Self {
        Self { set: AtomicBool::new(false), value: UnsafeCell::new(None) }
    }

    /// Sets the value. Panics if it was already set.
    pub fn set(&self, v: T) {
        assert!(!self.set.load(Ordering::Acquire), "a Once was set twice");
        // SAFETY: not yet published, and only one core sets it (start-up runs on one core).
        unsafe { *self.value.get() = Some(v) };
        self.set.store(true, Ordering::Release);
    }

    /// The value, if it's been set.
    pub fn get(&self) -> Option<&T> {
        if self.set.load(Ordering::Acquire) {
            // SAFETY: published, and never written again.
            unsafe { (*self.value.get()).as_ref() }
        } else {
            None
        }
    }
}
