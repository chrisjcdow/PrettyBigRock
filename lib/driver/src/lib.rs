// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What every driver is built on (docs/design/0011-device-manager-and-drivers.md, section 7).
//!
//! - [`Device`] reads the start-up block and the device record into typed, mapped parts.
//! - [`Mmio`] and [`Dma`] are the only ways a driver touches device registers and shared
//!   memory: bounds-checked, volatile, behind the [`Registers`] trait. They hold the `unsafe`
//!   code.
//! - [`serve`] is the event loop: requests on the driver's endpoint and interrupts, in order.
//!
//! The rules a driver follows (from the note, and worth repeating where drivers are written):
//! it initialises its device from reset and never assumes the device's state, because it may be
//! starting again after a crash; it treats everything a device says as hostile input; and it
//! keeps no state it can't rebuild.
//!
//! The library is only for PrettyBigRock programs.

#![no_std]

extern crate alloc;

pub use pbr_devices::pci::Config;
pub use pbr_devices::record::{self, Kind, Message, Record};
pub use pbr_devices::regs::Registers;

pub mod debug;
mod device;
pub mod display;
pub mod input;
mod regions;
mod serve;

pub use device::{Device, Irq, StartError, role};
pub use regions::{ConfigPage, Dma, Mmio};
pub use serve::{Service, host_ready, serve};
