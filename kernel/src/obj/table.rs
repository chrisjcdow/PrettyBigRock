// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A process's capability table (docs/design/0006-objects-and-capabilities.md, section 3).
//!
//! A handle is a `u32`: bits 0 to 19 are the slot, bits 20 to 30 the slot's generation, and
//! bit 31 is always 0. Slot 0 is never used, so handle 0 is never valid. Each time a slot is
//! freed its generation changes, so an old handle number never reaches a new capability.

use super::budget::Charge;
use super::{Error, Object, charge};
use alloc::vec::Vec;
use pbr_captree::NodeId;

/// The most slots a table can have.
pub const MAX_SLOTS: usize = 1 << 20;
const GENERATIONS: u32 = 1 << 11;
/// How many slots the table grows by at a time.
const GROW: usize = 64;

/// One capability.
#[derive(Clone)]
pub struct Entry {
    /// The object.
    pub object: Object,
    /// Its rights.
    pub rights: u32,
    /// Its badge (endpoints only; otherwise 0).
    pub badge: u64,
    /// Its node in the derivation tree.
    pub node: NodeId,
}

struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

/// A process's capabilities.
pub struct Table {
    slots: Vec<Slot>,
    free: Vec<u32>,
    used: usize,
    charge: Charge,
}

fn split(h: u32) -> (usize, u32) {
    ((h & 0xF_FFFF) as usize, (h >> 20) & 0x7FF)
}

impl Table {
    /// An empty table, charging its slots to `charge`'s budget.
    pub fn new(charge: Charge) -> Self {
        Self { slots: Vec::new(), free: Vec::new(), used: 0, charge }
    }

    /// The capability `h` names.
    pub fn get(&self, h: u32) -> Result<&Entry, Error> {
        let (i, generation) = split(h);
        if h >> 31 != 0 || i == 0 {
            return Err(Error::BadHandle);
        }
        let s = self.slots.get(i).ok_or(Error::BadHandle)?;
        if s.generation != generation {
            return Err(Error::BadHandle);
        }
        s.entry.as_ref().ok_or(Error::BadHandle)
    }

    /// How many more capabilities fit without the table growing beyond its maximum.
    pub fn room(&self) -> usize {
        MAX_SLOTS - 1 - self.used
    }

    /// Makes room for `n` more capabilities, charging any growth to the budget.
    pub fn reserve(&mut self, n: usize) -> Result<(), Error> {
        if n > self.room() {
            return Err(Error::TableFull);
        }
        // Every slot except slot 0 is either in use or on the free list.
        let spare = self.free.len();
        if spare >= n {
            return Ok(());
        }
        let base = self.slots.len().max(1);
        let want = (base + (n - spare)).next_multiple_of(GROW).min(MAX_SLOTS);
        let more = want - base;
        self.charge.grow(more as u64 * charge::TABLE_SLOT)?;
        if self.slots.is_empty() {
            self.slots.push(Slot { generation: 0, entry: None }); // slot 0, never used
        }
        while self.slots.len() < want {
            let i = self.slots.len() as u32;
            self.slots.push(Slot { generation: 0, entry: None });
            self.free.push(i);
        }
        Ok(())
    }

    /// Puts a capability in a free slot, growing the table if needed. Returns its handle.
    pub fn insert(&mut self, entry: Entry) -> Result<u32, Error> {
        self.reserve(1)?;
        let i = self.free.pop().ok_or(Error::TableFull)?;
        let s = &mut self.slots[i as usize];
        s.entry = Some(entry);
        self.used += 1;
        Ok((s.generation << 20) | i)
    }

    /// Takes the capability `h` out of the table.
    pub fn remove(&mut self, h: u32) -> Result<Entry, Error> {
        self.get(h)?;
        let (i, _) = split(h);
        let s = &mut self.slots[i];
        let e = s.entry.take().ok_or(Error::BadHandle)?;
        s.generation = (s.generation + 1) % GENERATIONS;
        self.free.push(i as u32);
        self.used -= 1;
        Ok(e)
    }

    /// True if `h` still names the capability whose tree node is `node`.
    pub fn holds(&self, h: u32, node: NodeId) -> bool {
        self.get(h).is_ok_and(|e| e.node == node)
    }

    /// Every (handle, capability), for tearing the table down.
    pub fn drain(&mut self) -> Vec<(u32, Entry)> {
        let mut out = Vec::new();
        for (i, s) in self.slots.iter_mut().enumerate() {
            if let Some(e) = s.entry.take() {
                out.push(((s.generation << 20) | i as u32, e));
                s.generation = (s.generation + 1) % GENERATIONS;
            }
        }
        self.free.clear();
        self.slots.clear();
        self.used = 0;
        out
    }
}
