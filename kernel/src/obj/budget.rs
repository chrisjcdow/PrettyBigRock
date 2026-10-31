// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Memory budgets (docs/design/0006-objects-and-capabilities.md, section 9).
//!
//! Every process has a [`Budget`], carved out of its creator's. Everything a process creates is
//! charged to its budget with a [`Charge`], which gives the amount back when it's dropped. A
//! child's budget is itself a charge on its parent's, held until the child's budget is dropped:
//! that is, until the child has ended *and* everything charged to it has been freed. So the
//! accounting stays right even when a child's shared memory outlives it.

use super::Error;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

/// A process's memory budget.
pub struct Budget {
    limit: u64,
    used: AtomicU64,
    /// What this budget costs its parent's (`None` for the root program's).
    _from_parent: Option<Charge>,
}

impl Budget {
    /// The root program's budget.
    pub fn root(limit: u64) -> Arc<Budget> {
        Arc::new(Budget { limit, used: AtomicU64::new(0), _from_parent: None })
    }

    /// A child budget of `limit` bytes, taken out of `parent`.
    pub fn child(parent: &Arc<Budget>, limit: u64) -> Result<Arc<Budget>, Error> {
        let c = parent.charge(limit)?;
        Ok(Arc::new(Budget { limit, used: AtomicU64::new(0), _from_parent: Some(c) }))
    }

    /// Takes `amount` bytes from the budget, or fails with `OUT_OF_MEMORY` if it won't fit.
    pub fn charge(self: &Arc<Self>, amount: u64) -> Result<Charge, Error> {
        self.take(amount)?;
        Ok(Charge { budget: self.clone(), amount })
    }

    fn take(&self, amount: u64) -> Result<(), Error> {
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let new =
                used.checked_add(amount).filter(|&n| n <= self.limit).ok_or(Error::OutOfMemory)?;
            match self.used.compare_exchange_weak(used, new, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(u) => used = u,
            }
        }
    }

    /// The budget's size, in bytes.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// How much of it is in use, in bytes.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }
}

/// An amount taken from a budget, given back when dropped.
pub struct Charge {
    budget: Arc<Budget>,
    amount: u64,
}

impl Charge {
    /// Takes `more` bytes from the same budget into this charge.
    pub fn grow(&mut self, more: u64) -> Result<(), Error> {
        self.budget.take(more)?;
        self.amount += more;
        Ok(())
    }

    /// Gives `less` bytes of this charge back to the budget.
    pub fn shrink(&mut self, less: u64) {
        let less = less.min(self.amount);
        self.budget.used.fetch_sub(less, Ordering::AcqRel);
        self.amount -= less;
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.amount, Ordering::AcqRel);
    }
}
