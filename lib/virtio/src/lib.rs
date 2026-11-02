// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Virtio over PCI, the modern (version 1) interface only
//! (docs/design/0011-device-manager-and-drivers.md, section 7): finding the device's
//! structures, the status handshake, split virtqueues, and the block and input devices.
//!
//! Everything a device tells the driver is checked: capability offsets and lengths, queue
//! sizes, the indices and lengths in the used ring. An impossible value is an error, and
//! after one a queue is [broken](queue::Queue::is_broken) until the device is reset. The
//! code reaches the hardware only through [`pbr_devices::regs::Registers`], so the host tests
//! run it against a software device that lies.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod blk;
pub mod input;
pub mod queue;
pub mod transport;

#[cfg(test)]
mod model;
#[cfg(test)]
mod tests;
