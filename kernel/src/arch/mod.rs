// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The architecture layer: everything that differs between processor families.
//!
//! Each architecture is a module with the same set of functions. Adding an architecture
//! means adding a module here, a linker script, and an entry in the build tool; the checklist
//! is in docs/design/0001-repository-layout.md.
//!
//! The functions every architecture provides (this list grows with each milestone):
//! - the entry point `_start`, which takes the boot information's physical address in the
//!   first argument register and calls [`crate::kernel_main`];
//! - [`halt_forever`]: stop this core, waiting for interrupts, never returning.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("PrettyBigRock doesn't support this architecture yet: see src/arch/mod.rs");
