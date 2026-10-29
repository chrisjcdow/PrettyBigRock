// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! AArch64 (64-bit Arm), as used by the Raspberry Pi 5.

/// The name used in messages.
pub const NAME: &str = "ARM64";

/// The kernel's entry point. The loader jumps here at exception level 1, with the MMU on,
/// interrupts masked, a valid stack, the boot information's physical address in `x0` and the
/// direct map's base in `x1` (docs/design/0003-boot-information.md).
#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: u64, direct_map_base: u64) -> ! {
    crate::kernel_main(boot_info, direct_map_base)
}

/// Stops this core for good, waiting for interrupts with them masked (so: for ever).
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: masking interrupts and waiting only stops this core. It touches no memory.
        unsafe { core::arch::asm!("msr daifset, #0xf", "wfi", options(nomem, nostack)) };
    }
}

/// Facts about the processor worth reporting at start-up.
pub fn describe(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let (el, midr): (u64, u64);
    // SAFETY: reading these registers has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
        core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr, options(nomem, nostack));
    }
    write!(
        out,
        "exception level {}, implementer {:#04x}, part {:#05x}",
        (el >> 2) & 3,
        midr >> 24,
        (midr >> 4) & 0xFFF
    )
}

/// Calls the loader's EL2 stub with a request it ignores, to check it's there and answering.
///
/// # Safety
/// Only call this if the boot information has `FLAG_EL2_STUB`: without a stub, `HVC` is an
/// undefined instruction, and there's no exception handler yet.
pub unsafe fn ping_el2_stub() {
    // SAFETY: the caller promises the stub exists; request 1 does nothing (note 0003).
    unsafe { core::arch::asm!("hvc #0", inout("x0") 1u64 => _, options(nomem, nostack)) };
}
