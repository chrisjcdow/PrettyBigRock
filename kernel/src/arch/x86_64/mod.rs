// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! x86-64 (AMD64 / Intel 64).

/// The kernel's entry point. The loader jumps here in 64-bit mode, with paging on, interrupts
/// off, a valid stack, and the boot information's physical address in `rdi`
/// (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64) -> ! {
    crate::kernel_main(boot_info)
}

/// Stops this core for good, sleeping until interrupts (which are off, so: for ever).
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this core. It touches no memory and no other state.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
