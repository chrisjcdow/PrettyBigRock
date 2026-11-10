// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning the machine off and restarting it (docs/design/0019-kernel-1.3.md, section 1).
//!
//! Only the holder of the machine capability can: the kernel gives it to the root program
//! (start-up role [`crate::process::role::POWER`]), which passes it to the power service and to
//! nothing else.

use crate::handle::Handle;
use crate::sys::{self, Error, num};

/// The machine capability.
#[derive(Debug)]
pub struct Machine(pub Handle);

/// What to do with the machine. The numbers are the kernel's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Power {
    /// Turn it off.
    Off = 1,
    /// Restart it.
    Restart = 2,
}

impl Machine {
    /// Turns the machine off or restarts it, at once. **The kernel saves nothing**: save
    /// everything first.
    ///
    /// On success this never returns, so all it can give back is the reason it didn't happen:
    /// `NoRight` without the `POWER` right, or `NotSupported` where the kernel can't do it
    /// (turning off on x86-64, which the power service does itself; either on an ARM64
    /// machine without PSCI).
    pub fn power(&self, action: Power) -> Error {
        let a = [self.0.raw().into(), action as u64, 0, 0, 0, 0];
        match sys::call(num::MACHINE_POWER, a) {
            Err(e) => e,
            // The kernel never answers this call with success.
            Ok(_) => Error::BadState,
        }
    }
}
