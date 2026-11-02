// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The system pages (docs/design/0007-system-calls.md, section 1): a code page every program
//! calls the kernel through, and a data page after it with what the clock needs. One copy of
//! each is shared, read-only, by every process.
//!
//! The code itself is in the architecture layer (`arch::SYSPAGE`): a header, then `call` and
//! `clock_now`. Its layout in the data page: offset 0, whether programs may read the clock's
//! counter themselves (1) or must ask the kernel (0); 8, the counter when the clock started;
//! 16, nanoseconds per tick times 2^32; 24 (from interface version 1.2,
//! docs/design/0013-kernel-1.2.md, section 4), the wall-clock time when the monotonic clock
//! started, in nanoseconds since 1970 UTC, or 0 if the loader didn't know it.

use crate::mem::object::MemoryObject;
use crate::sync::Once;
use crate::{arch, time};
use alloc::sync::Arc;

static PAGES: Once<(Arc<MemoryObject>, Arc<MemoryObject>)> = Once::new();

/// Builds the system pages. Called once, after the clock starts and before any process.
/// `boot_time_ns` is the wall-clock time the loader read (0 if unknown). The loader ran a
/// moment before the clock started, so it stands for the time the clock read 0, to within
/// the loader's hand-over (well under a second).
pub fn init(boot_time_ns: u64) {
    let code = MemoryObject::kernel(1).expect("out of memory for the system pages");
    let data = MemoryObject::kernel(1).expect("out of memory for the system pages");
    let bytes = arch::syspage_code();
    assert!(bytes.len() <= 4096 && &bytes[..8] == b"PBRSYSPG", "the system page's code is wrong");
    code.write(0, bytes).expect("fits in a page");
    // The header's interface version comes from the one place it's defined, so the system
    // page and the `version` call can never disagree.
    let (major, minor) = crate::syscall::VERSION;
    let mut v = [0u8; 4];
    v[0..2].copy_from_slice(&(major as u16).to_le_bytes());
    v[2..4].copy_from_slice(&(minor as u16).to_le_bytes());
    code.write(8, &v).expect("fits in a page");
    let (start, mult) = time::parameters();
    let mut d = [0u8; 32];
    d[0..8].copy_from_slice(&u64::from(arch::user_clock()).to_le_bytes());
    d[8..16].copy_from_slice(&start.to_le_bytes());
    d[16..24].copy_from_slice(&mult.to_le_bytes());
    d[24..32].copy_from_slice(&boot_time_ns.to_le_bytes());
    data.write(0, &d).expect("fits in a page");
    PAGES.set((code, data));
}

/// The (code, data) pages, for mapping into a new process.
pub fn pages() -> (Arc<MemoryObject>, Arc<MemoryObject>) {
    let (c, d) = PAGES.get().expect("the system pages aren't built yet");
    (c.clone(), d.clone())
}
