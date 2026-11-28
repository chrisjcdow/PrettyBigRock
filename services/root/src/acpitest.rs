// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The ACPI service's self-test (docs/design/0028-acpi.md, section 10): who may look it up,
//! that each badge may make only its own calls, that `devices` pages through everything the
//! namespace describes, and that `route` answers for a PCI function and refuses a pin that
//! doesn't exist. `off` is never called here: the self-test must not turn the machine off.

use crate::supervisor::{ACPI_DEVICES, ACPI_POWER, Kind, Supervisor, rule};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::interface::CallError;
use granite::notification::Notification;
use pbr_interfaces::pbr_acpi::{self as acpi_if, AcpiError};

/// Expects a call to have been refused with `want`.
fn refused<T: core::fmt::Debug>(
    what: &str,
    r: Result<T, CallError<AcpiError>>,
    want: AcpiError,
) -> Result<(), String> {
    match r {
        Err(CallError::Service(e)) if e == want => Ok(()),
        other => Err(format!("{what}: {other:?}, expected {want:?}")),
    }
}

/// Runs the test. Returns a short summary.
pub fn run(sup: &Supervisor) -> Result<String, String> {
    // Only the device manager and the power service may look the ACPI service up.
    for child in [Kind::Storage, Kind::Users, Kind::Console, Kind::Input, Kind::Display] {
        if rule(child, "acpi").is_some() {
            return Err(format!("{} may look up the ACPI service", child.program()));
        }
    }
    if rule(Kind::Devmgr, "acpi") != Some((Kind::Acpi, ACPI_DEVICES))
        || rule(Kind::Power, "acpi") != Some((Kind::Acpi, ACPI_POWER))
    {
        return Err("the device manager's or the power service's ACPI badge is wrong".into());
    }
    let client = |badge: u64| -> Result<acpi_if::Client, String> {
        let mut c = acpi_if::Client::new(sup.client(Kind::Acpi, badge, 30)?);
        c.set_timeout_ms(30_000);
        Ok(c)
    };
    let devices = client(ACPI_DEVICES)?;

    // Every device, a page at a time.
    let mut all: Vec<acpi_if::Device> = Vec::new();
    let mut first = 0;
    loop {
        let r = devices.devices(first).map_err(|e| format!("devices({first}): {e}"))?;
        if r.devices.len() > 16 {
            return Err(format!("a page of {} devices", r.devices.len()));
        }
        all.extend(r.devices);
        match r.next {
            Some(n) if n as usize == all.len() => first = n,
            Some(n) => return Err(format!("devices: next is {n} after {} devices", all.len())),
            None => break,
        }
    }
    // Past the end there are none.
    let past =
        devices.devices(all.len() as u32 + 10).map_err(|e| format!("devices past the end: {e}"))?;
    if !past.devices.is_empty() || past.next.is_some() {
        return Err("devices past the end gave some".into());
    }
    // What every test machine's firmware describes: a PCI host bridge; on a PC, a serial port
    // and the keyboard controller; on ARM64's virt, its PL011.
    let has = |id: &str| all.iter().any(|d| d.hid == id || d.compatible.iter().any(|c| c == id));
    let mut want = alloc::vec!["PNP0A08"];
    if cfg!(target_arch = "x86_64") {
        want.extend(["PNP0501", "PNP0303"]);
    } else {
        want.push("ARMH0011");
    }
    for id in want {
        if !has(id) {
            let ids: Vec<&str> = all.iter().map(|d| d.hid.as_str()).collect();
            return Err(format!("no {id} among the devices: {ids:?}"));
        }
    }

    // Device 1's INTA on bus 0 is wired somewhere (a level-triggered line on both machines);
    // there is no fifth pin, and no bus 200.
    let line = devices.route(0, 0, 1, 1).map_err(|e| format!("routing 00:01 INTA: {e}"))?;
    if !line.level {
        return Err(format!("00:01 INTA is edge-triggered: {line:?}"));
    }
    refused("routing pin 5", devices.route(0, 0, 1, 5), AcpiError::NotFound)?;
    refused("routing on bus 200", devices.route(0, 200, 1, 1), AcpiError::NotFound)?;

    // Each badge only its own calls; any other badge none.
    let note = || Notification::create().map_err(|e| format!("a notification: {e}"));
    refused("off with the device manager's badge", devices.off(), AcpiError::NotAllowed)?;
    refused(
        "watch_button with the device manager's badge",
        devices.watch_button(note()?, 1),
        AcpiError::NotAllowed,
    )?;
    let power = client(ACPI_POWER)?;
    refused("devices with the power service's badge", power.devices(0), AcpiError::NotAllowed)?;
    refused(
        "route with the power service's badge",
        power.route(0, 0, 1, 1),
        AcpiError::NotAllowed,
    )?;
    let other = client(ACPI_POWER + 1)?;
    refused("devices with another badge", other.devices(0), AcpiError::NotAllowed)?;
    refused("off with another badge", other.off(), AcpiError::NotAllowed)?;
    Ok(format!("{} devices, 00:01 INTA on line {}", all.len(), line.number))
}
