// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PC's fixed power-management hardware (docs/design/0028-acpi.md, sections 3, 5 and 6):
//! ACPI mode, the event registers, the power button and turning off.
//!
//! The **PM1** registers are the core of it. The event block is a status register and an
//! enable register: an event whose status and enable bits are both set raises the **SCI**, the
//! ACPI interrupt; writing 1 to a status bit clears it. The control register holds `SCI_EN`
//! (ACPI mode is on) and, for sleeping and turning off, the sleep type and the sleep-enable bit.
//! The **general-purpose events** (GPEs) are blocks of status and enable bits too, for events
//! the firmware's methods handle; M7 turns them all off.

use crate::host::Machine;
use granite::time;
use pbr_aml::Aml;
use pbr_interfaces::pbr_acpi::AcpiError;
use pbr_platform::acpi::FadtFixed;

/// PM1 control's `SCI_EN`: ACPI mode is on.
const SCI_EN: u64 = 1 << 0;
/// PM1 status and enable: the power button.
const PWRBTN: u64 = 1 << 8;
/// PM1 status: the machine woke (cleared before sleeping).
const WAK_STS: u64 = 1 << 15;
/// PM1 control: the sleep type's bits, and sleep enable.
const SLP_TYP: u64 = 7 << 10;
const SLP_EN: u64 = 1 << 13;

/// The PM1 event blocks there are, with where each one's enable register is.
fn pm1_blocks(f: &FadtFixed) -> impl Iterator<Item = (u16, u16)> + '_ {
    let half = u16::from(f.pm1_event_length / 2);
    [f.pm1a_event, f.pm1b_event].into_iter().filter(|&p| p != 0).map(move |p| (p, p + half))
}

/// Switches the firmware into ACPI mode, if it isn't already (note 0028, section 3, step 2):
/// writes `ACPI_ENABLE` to the SMI command port, then waits up to 3 seconds for `SCI_EN`.
pub fn enable_acpi_mode(host: &Machine, f: &FadtFixed) {
    let on = |h: &Machine| h.port_in(f.pm1a_control, 16).is_some_and(|v| v & SCI_EN != 0);
    if on(host) {
        return;
    }
    let Ok(port) = u16::try_from(f.smi_command) else { return };
    if port == 0 || f.acpi_enable == 0 {
        granite::println!("acpi: not in ACPI mode, and no way to switch it on.");
        return;
    }
    let _ = host.port_out(port, 8, u64::from(f.acpi_enable));
    let end = time::after_ms(3000);
    while time::now() < end {
        if on(host) {
            granite::println!("acpi: switched into ACPI mode.");
            return;
        }
        time::sleep_ms(10);
    }
    granite::println!("acpi: the firmware didn't switch into ACPI mode.");
}

/// Turns off every event M7 doesn't handle (note 0028, section 3, step 5): each GPE block's
/// enable registers written to zero and its status cleared, and every fixed event's enable
/// bit off and its status cleared.
pub fn quiet(host: &Machine, f: &FadtFixed) {
    for (block, length) in [(f.gpe0, f.gpe0_length), (f.gpe1, f.gpe1_length)] {
        if block == 0 || length == 0 {
            continue;
        }
        let half = u16::from(length / 2);
        for i in 0..half {
            let _ = host.port_out(block + half + i, 8, 0);
            let _ = host.port_out(block + i, 8, 0xFF);
        }
    }
    for (status, enable) in pm1_blocks(f) {
        let _ = host.port_out(enable, 16, 0);
        let _ = host.port_out(status, 16, 0xFFFF);
    }
}

/// Turns the power button's event on (note 0028, section 5).
pub fn enable_power_button(host: &Machine, f: &FadtFixed) {
    for (status, enable) in pm1_blocks(f) {
        let _ = host.port_out(status, 16, PWRBTN);
        let _ = host.port_out(enable, 16, PWRBTN);
    }
}

/// True if the SCI was the power button; its status is cleared.
pub fn power_button_pressed(host: &Machine, f: &FadtFixed) -> bool {
    let mut pressed = false;
    for (status, _) in pm1_blocks(f) {
        if host.port_in(status, 16).is_some_and(|v| v & PWRBTN != 0) {
            let _ = host.port_out(status, 16, PWRBTN);
            pressed = true;
        }
    }
    pressed
}

/// Turns the machine off (note 0028, section 6): `\_PTS(5)` if the firmware has it, then
/// `\_S5`'s sleep types into PM1a and PM1b control, first without and then with sleep enable,
/// as ACPICA writes them. If the machine is still running 3 seconds later, it couldn't.
pub fn off(aml: &mut Aml<Machine>, f: &FadtFixed) -> AcpiError {
    if aml.find(None, "\\_PTS").is_some()
        && let Err(e) = aml.evaluate("\\_PTS", &[pbr_aml::Value::Integer(5)])
    {
        granite::println!("acpi: \\_PTS(5) failed: {e}");
    }
    let types = match aml.sleep_type(5) {
        Ok(t) => t,
        Err(e) => {
            granite::println!("acpi: no way to turn this computer off: \\_S5: {e}");
            return AcpiError::NotFound;
        }
    };
    granite::println!("acpi: turning the computer off (sleep type {}, {}).", types.a, types.b);
    let host = aml.host();
    for (status, _) in pm1_blocks(f) {
        let _ = host.port_out(status, 16, WAK_STS);
    }
    let controls = [(f.pm1a_control, types.a), (f.pm1b_control, types.b)];
    let mut values = [0u64; 2];
    for (i, (port, t)) in controls.iter().enumerate() {
        if *port == 0 {
            continue;
        }
        let v = host.port_in(*port, 16).unwrap_or(0) & !(SLP_TYP | SLP_EN);
        values[i] = v | (u64::from(*t & 7) << 10);
        let _ = host.port_out(*port, 16, values[i]);
    }
    for (i, (port, _)) in controls.iter().enumerate() {
        if *port != 0 {
            let _ = host.port_out(*port, 16, values[i] | SLP_EN);
        }
    }
    time::sleep_ms(3000);
    granite::println!("acpi: still running 3 seconds after turning off.");
    AcpiError::Failed
}
