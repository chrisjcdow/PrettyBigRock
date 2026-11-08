// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the device manager and the drivers need to know about devices
//! (docs/design/0011-device-manager-and-drivers.md): reading PCI configuration space, driver
//! manifests and matching, and the device record a driver starts with.
//!
//! All of it is parsing of data that came from hardware or from files, so none of it trusts
//! its input: nothing here indexes, loops or adds without checking, and the tests include
//! random corruption. It reaches hardware only through the [`pci::Bus`] and [`pci::Config`]
//! traits, so it is tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod keys;
pub mod machine;
pub mod manifest;
pub mod msix;
pub mod pci;
pub mod record;
pub mod regs;

/// A small deterministic random number generator for tests (xorshift64*).
#[cfg(test)]
pub(crate) struct Rng(pub u64);

#[cfg(test)]
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}
