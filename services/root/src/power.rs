// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the power service is started with (docs/design/0024-the-session-desktop.md,
//! section 7): the machine capability, which lets the kernel restart the computer, and turn it
//! off on ARM64. A PC is turned off through the ACPI service, which the power service finds
//! through the supervisor (docs/design/0028-acpi.md, section 7), so it is given nothing else.

use alloc::string::String;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use granite::Handle;
use granite::process::role;
use granite::sys::rights;

/// The power service's capabilities: (role, capability, rights to give), and its arguments.
pub fn caps(start: &granite::Start) -> (Vec<(u32, Handle, u32)>, String) {
    let mut caps = Vec::new();
    // A copy: the start-up capability stays the root program's. (`ManuallyDrop` stops the
    // wrapper closing the original when it goes out of scope.)
    let copy = start
        .peek(role::POWER)
        .and_then(|raw| ManuallyDrop::new(Handle::from_raw(raw)).duplicate(rights::ALL).ok());
    match copy {
        Some(m) => caps.push((role::POWER, m, rights::ALL)),
        None => granite::println!("The root program has no machine capability to pass on."),
    }
    (caps, String::new())
}
