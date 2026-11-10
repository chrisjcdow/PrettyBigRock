// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning an Arm machine off and restarting it (docs/design/0019-kernel-1.3.md, section 1).
//!
//! Both go through PSCI, the firmware interface Arm machines use for power (the kernel also
//! starts the other cores with it, see `smp`). PSCI is reached with the `hvc` or `smc`
//! instruction, whichever the ACPI tables say, and only the kernel's privilege level may use
//! those: which is why this is a system call and not a program's job.

use crate::power::Action;

/// PSCI's `SYSTEM_OFF` and `SYSTEM_RESET` function numbers.
const SYSTEM_OFF: u64 = 0x8400_0008;
const SYSTEM_RESET: u64 = 0x8400_0009;

/// What [`power_prepare`] found out, for [`power_now`].
pub struct PowerPlan {
    function: u64,
    /// PSCI is called with `hvc` (otherwise `smc`).
    hvc: bool,
}

/// Whether, and how, this machine can do `action`: only if it has PSCI.
pub fn power_prepare(action: Action) -> Option<PowerPlan> {
    let hvc = crate::hw_machine().psci?;
    let function = match action {
        Action::Off => SYSTEM_OFF,
        Action::Restart => SYSTEM_RESET,
    };
    Some(PowerPlan { function, hvc })
}

/// Turns the machine off or restarts it. Interrupts are off and the other cores are stopped.
pub fn power_now(plan: PowerPlan) -> ! {
    let r: i64;
    // SAFETY: a PSCI call follows the Arm calling convention (it may change x0 to x3 and
    // nothing else), and these two functions only take the machine down.
    unsafe {
        if plan.hvc {
            core::arch::asm!("hvc #0", inout("x0") plan.function as i64 => r, out("x1") _, out("x2") _, out("x3") _, options(nostack));
        } else {
            core::arch::asm!("smc #0", inout("x0") plan.function as i64 => r, out("x1") _, out("x2") _, out("x3") _, options(nostack));
        }
    }
    panic!("the firmware wouldn't take the machine down (PSCI answered {r})");
}
