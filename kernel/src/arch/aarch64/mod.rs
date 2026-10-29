// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! AArch64 (64-bit Arm), as used by the Raspberry Pi 5.

/// The kernel's entry point. The loader jumps here at exception level 1, with the MMU on,
/// interrupts masked, a valid stack, and the boot information's physical address in `x0`
/// (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64) -> ! {
    crate::kernel_main(boot_info)
}

/// Stops this core for good, waiting for interrupts with them masked (so: for ever).
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: masking interrupts and waiting only stops this core. It touches no memory.
        unsafe { core::arch::asm!("msr daifset, #0xf", "wfi", options(nomem, nostack)) };
    }
}
