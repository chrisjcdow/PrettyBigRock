// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! A lock that's safe across a program's threads. Waiting threads give the processor away
//! rather than spinning hard.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A mutual-exclusion lock around a value.
pub struct Lock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: one thread at a time has access to the value.
unsafe impl<T: Send> Sync for Lock<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Send for Lock<T> {}

impl<T> Lock<T> {
    /// A new, unlocked lock.
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// Waits for the lock and takes it.
    pub fn lock(&self) -> Guard<'_, T> {
        let mut spins = 0u32;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spins += 1;
            if spins > 100 {
                crate::thread::yield_now();
                spins = 0;
            } else {
                core::hint::spin_loop();
            }
        }
        Guard { lock: self }
    }
}

/// Access to a locked value; unlocks when dropped.
pub struct Guard<'a, T> {
    lock: &'a Lock<T>,
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the guard means holding the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}
