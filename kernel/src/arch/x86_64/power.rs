// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Restarting a PC (docs/design/0019-kernel-1.3.md, section 1).
//!
//! There is no one way that works on every PC, so the kernel tries the usual ones in order,
//! giving each a moment: the reset register the firmware's ACPI tables name, the chipset's
//! reset port, the keyboard controller's reset line, and at last a "triple fault". Turning a
//! PC off needs a value from the ACPI tables' bytecode, which the kernel doesn't read: that is
//! the power service's job (note 0024), and here it is simply not supported.

use crate::mem::vm;
use crate::power::{Action, wait_until};
use pbr_platform::acpi::ResetRegister;

/// What [`power_prepare`] found out, for [`power_now`].
pub struct PowerPlan {
    /// The firmware's reset register: where to write (a port number, or a mapped address),
    /// whether that is a port, and the byte to write.
    reset: Option<(u64, bool, u8)>,
    /// There is a keyboard controller to ask.
    keyboard_controller: bool,
}

/// Whether, and how, this machine can do `action`. Called before anything is stopped, so it
/// may still map memory.
pub fn power_prepare(action: Action) -> Option<PowerPlan> {
    if action != Action::Restart {
        return None;
    }
    let m = crate::hw_machine();
    let reset = m.reset.map(|ResetRegister { io_port, address, value }| {
        // A register in memory has to be mapped before it can be written.
        let at = if io_port { address } else { vm::map_device(address, 1) };
        (at, io_port, value)
    });
    Some(PowerPlan { reset, keyboard_controller: m.ps2 })
}

/// The interrupt table's descriptor, as `lidt` reads it: a 16-bit limit and a 64-bit address.
/// `#[repr(C, packed)]` lays the fields out exactly so, with no padding between them.
#[repr(C, packed)]
struct TableDescriptor {
    limit: u16,
    base: u64,
}

/// Restarts the machine. Interrupts are off and the other cores are stopped.
pub fn power_now(plan: PowerPlan) -> ! {
    let never = || false;
    if let Some((at, io_port, value)) = plan.reset {
        if io_port {
            super::port_out(at as u16, 1, value.into());
        } else {
            // SAFETY: `power_prepare` mapped this one-byte device register, which the
            // firmware's tables say restarts the machine when written.
            unsafe { core::ptr::write_volatile(at as *mut u8, value) };
        }
        wait_until(never);
    }
    // The chipset's reset control port, on nearly every PC since the 1990s: bit 1 chooses a
    // full reset, and bit 2 going from 0 to 1 does it.
    super::port_out(0xCF9, 1, 0x02);
    super::port_out(0xCF9, 1, 0x06);
    wait_until(never);
    if plan.keyboard_controller {
        // The keyboard controller's command 0xFE pulses the processor's reset line: the oldest
        // way there is.
        super::port_out(0x64, 1, 0xFE);
        wait_until(never);
    }
    // A triple fault: with an empty interrupt table, the breakpoint can't be delivered, the
    // fault that causes can't either, and a processor that fails three times over resets.
    let empty = TableDescriptor { limit: 0, base: 0 };
    // SAFETY: this deliberately breaks the core so that the machine resets; nothing runs
    // afterwards.
    unsafe {
        core::arch::asm!("lidt [{}]", "int3", in(reg) &raw const empty, options(nostack));
    }
    panic!("the machine wouldn't restart");
}
