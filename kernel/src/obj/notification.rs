// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Notifications (docs/design/0006-objects-and-capabilities.md, section 8): 64 signal bits.
//! Signalling never blocks and never fills up; waiting takes the bits that are set and clears
//! them.

use super::Error;
use super::budget::Charge;
use super::endpoint::Endpoint;
use crate::sched::{self, Thread, Woken};
use crate::sync::SpinLock;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// A notification.
pub struct Notification {
    bits: AtomicU64,
    waiters: SpinLock<Vec<Arc<Thread>>>,
    bound: SpinLock<Option<Weak<Endpoint>>>,
    _charge: Option<Charge>,
}

impl Notification {
    /// A new notification with no bits set.
    pub fn new(charge: Option<Charge>) -> Arc<Self> {
        Arc::new(Notification {
            bits: AtomicU64::new(0),
            waiters: SpinLock::new(Vec::new()),
            bound: SpinLock::new(None),
            _charge: charge,
        })
    }

    /// Sets `bits`, and wakes whoever is waiting: on the notification itself, or receiving on
    /// the endpoint it's bound to.
    pub fn signal(&self, bits: u64) {
        if bits == 0 {
            return;
        }
        self.bits.fetch_or(bits, Ordering::SeqCst);
        let waiting: Vec<Arc<Thread>> = self.waiters.lock().clone();
        for t in &waiting {
            sched::wake(t);
        }
        let ep = self.bound.lock().as_ref().and_then(Weak::upgrade);
        if let Some(ep) = ep {
            ep.wake_receivers();
        }
    }

    /// Takes and clears the bits that are set (0 if none).
    pub fn take(&self) -> u64 {
        self.bits.swap(0, Ordering::SeqCst)
    }

    /// True if any bit is set.
    pub fn pending(&self) -> bool {
        self.bits.load(Ordering::SeqCst) != 0
    }

    /// Ties the notification to an endpoint, so receiving on it also returns the bits.
    pub fn bind(&self, ep: &Arc<Endpoint>) -> Result<(), Error> {
        let mut b = self.bound.lock();
        if b.as_ref().is_some_and(|w| w.strong_count() > 0) {
            return Err(Error::AlreadyExists);
        }
        *b = Some(Arc::downgrade(ep));
        Ok(())
    }

    /// Waits until a bit is set or `deadline` passes, then takes the bits.
    /// `still_held` is checked as the thread starts to wait, so a revoked capability can't
    /// leave it waiting.
    pub fn wait(&self, deadline: u64, still_held: impl Fn() -> bool) -> Result<u64, Error> {
        let me = sched::current();
        loop {
            self.waiters.lock().push(me.clone());
            let woken = sched::wait_event(deadline, || self.pending() || !still_held());
            self.waiters.lock().retain(|t| !Arc::ptr_eq(t, &me));
            let bits = self.take();
            if bits != 0 {
                return Ok(bits);
            }
            if !still_held() {
                return Err(Error::Revoked);
            }
            match woken {
                Woken::Ready => continue, // another waiter took the bits first
                Woken::Stopped => return Err(Error::BadState),
                Woken::TimedOut => return Err(Error::TimedOut),
            }
        }
    }

    /// Wakes every thread waiting on the notification, so it can see its capability is gone.
    pub fn wake_waiters(&self) {
        let waiting: Vec<Arc<Thread>> = self.waiters.lock().clone();
        for t in &waiting {
            sched::wake(t);
        }
    }
}
