// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The architecture layer: everything that differs between processor families.
//!
//! Each architecture is a module with the same set of functions. Adding an architecture
//! means adding a module here, a linker script, and an entry in the build tool; the checklist
//! is in docs/design/0001-repository-layout.md.
//!
//! What every architecture provides (this list grows with each milestone):
//! - the entry point `_start`, which takes the boot information's physical address in the
//!   first argument register and the direct map's base in the second, and calls
//!   [`crate::kernel_main`];
//! - `NAME`, `PAGE_FORMAT`, and `describe`, which writes a line about the processor;
//! - stopping and waiting: `halt_forever`, `wait_for_interrupt`;
//! - interrupt masking: `IrqState`, `irq_save`, `irq_restore`, `irq_enable`, `irq_disable`;
//! - per-core data: `CpuArch`, `set_core_info`, `set_percpu`, `get_percpu`, `zero_percpu`,
//!   `this_core_hw_id`;
//! - memory: `activate_kernel_tables`, `tlb_flush_page`;
//! - time: `clock_counter`;
//! - start-up: `init_boot_core`, `start_other_cores`;
//! - threads: `init_thread_stack`, `switch_context`, `switch_stack`;
//! - inter-core interrupts: `Ipi`, `send_ipi`;
//! - the self-test's hooks: `breakpoint`, `breakpoints`.

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
