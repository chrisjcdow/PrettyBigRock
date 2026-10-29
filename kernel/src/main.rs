// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PrettyBigRock microkernel.
//!
//! The kernel does only what can't be done anywhere else: address spaces, threads and
//! scheduling, messages, capabilities, interrupts and timers (docs/DESIGN.md, section 6).
//! Everything else is an ordinary sandboxed program.
//!
//! Processor-specific code lives in [`arch`]; nothing outside it may assume which processor
//! it runs on (docs/DESIGN.md, section 10).
//!
//! Status: milestone M0. The kernel builds for every architecture and stops straight away.
//! Booting it for real arrives in M1.

#![no_std]
#![no_main]

mod arch;

use pbr_bootinfo::BootInfo;

/// The kernel's version, from the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where every architecture's start-up code goes once it has a stack: the first
/// architecture-independent code that runs.
///
/// `boot_info` is the physical address of the [`BootInfo`] the loader wrote. The loader's
/// promises about the machine's state at this point are in
/// `docs/design/0003-boot-information.md`.
pub fn kernel_main(boot_info: u64) -> ! {
    // M1 reads the boot information through the direct map and checks it with
    // `BootInfo::validate`. Until there's a loader, all the kernel can do is note that it was
    // given an address and stop.
    let _ = (boot_info, core::mem::size_of::<BootInfo>());
    arch::halt_forever()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    arch::halt_forever()
}
