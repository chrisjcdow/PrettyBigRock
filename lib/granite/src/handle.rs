// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Handles: a program's names for its capabilities (docs/design/0006-objects-and-capabilities.md,
//! section 3). A [`Handle`] closes its capability when dropped.

use crate::sys::{self, Result, num};

/// A capability this program holds. Closed when dropped (unless given away with
/// [`Handle::into_raw`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Handle(u32);

impl Handle {
    /// Takes charge of the raw handle `h` (for example one received in a message). A number
    /// that isn't really a handle only makes later calls fail with `BadHandle`.
    pub fn from_raw(h: u32) -> Handle {
        Handle(h)
    }

    /// The raw handle number, keeping charge of it.
    pub fn raw(&self) -> u32 {
        self.0
    }

    /// The raw handle number, giving up charge of it: it won't be closed.
    pub fn into_raw(self) -> u32 {
        let h = self.0;
        core::mem::forget(self);
        h
    }

    /// A copy with only the rights in `mask`.
    pub fn duplicate(&self, mask: u32) -> Result<Handle> {
        sys::call(num::HANDLE_DUPLICATE, [self.0.into(), mask.into(), 0, 0, 0, 0])
            .map(|(h, _)| Handle(h as u32))
    }

    /// Destroys every copy made from this capability, wherever it went, and keeps this one.
    pub fn revoke(&self) -> Result<()> {
        sys::call(num::HANDLE_REVOKE, [self.0.into(), 0, 0, 0, 0, 0]).map(|_| ())
    }

    /// The object's type (see `sys::kind`) and this capability's rights.
    pub fn info(&self) -> Result<(u32, u32)> {
        sys::call(num::HANDLE_INFO, [self.0.into(), 0, 0, 0, 0, 0])
            .map(|(k, r)| (k as u32, r as u32))
    }

    /// Closes the capability now, reporting any error.
    pub fn close(self) -> Result<()> {
        let h = self.into_raw();
        sys::call(num::HANDLE_CLOSE, [h.into(), 0, 0, 0, 0, 0]).map(|_| ())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            let _ = sys::call(num::HANDLE_CLOSE, [self.0.into(), 0, 0, 0, 0, 0]);
        }
    }
}
