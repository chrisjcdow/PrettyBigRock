// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Notifications: 64 signal bits (docs/design/0006-objects-and-capabilities.md, section 8).

use crate::handle::Handle;
use crate::sys::{self, Result, num};

/// A notification.
#[derive(Debug)]
pub struct Notification(pub Handle);

impl Notification {
    /// A new notification.
    pub fn create() -> Result<Notification> {
        sys::call(num::NOTIFICATION_CREATE, [0; 6])
            .map(|(h, _)| Notification(Handle::from_raw(h as u32)))
    }

    /// Sets `bits`.
    pub fn signal(&self, bits: u64) -> Result<()> {
        sys::call(num::NOTIFICATION_SIGNAL, [self.0.raw().into(), bits, 0, 0, 0, 0]).map(|_| ())
    }

    /// Waits until a bit is set or `deadline` passes; takes and clears the bits.
    pub fn wait(&self, deadline: u64) -> Result<u64> {
        sys::call(num::NOTIFICATION_WAIT, [self.0.raw().into(), deadline, 0, 0, 0, 0])
            .map(|(b, _)| b)
    }
}
