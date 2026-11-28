// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What the firmware's ACPI tables say, as the ACPI service's interpreter finds it
/// (docs/design/0028-acpi.md, section 6). Served by the ACPI service: to the device manager
/// (badge `DEVICES`: `devices` and `route`) and the power service (badge `POWER`: `off` and
/// `watch_button`); anything else is `NotAllowed`.
interface pbr.acpi version 1;

/// A range of I/O ports.
struct Ports {
    first: u16,
    count: u16,
}

/// A range of physical memory.
struct Range {
    base: u64,
    length: u64,
}

/// An interrupt line, as the firmware describes it.
struct Line {
    /// x86-64: the global system interrupt; ARM64: the GIC's interrupt ID.
    number: u32,
    /// Level-triggered (otherwise edge).
    level: bool,
    /// Active low (otherwise high).
    active_low: bool,
}

/// A device the namespace describes that is present, has a hardware ID, and isn't a PCI
/// function (those are found on the bus, docs/design/0011-device-manager-and-drivers.md).
struct Device {
    /// Its full name, such as `\_SB.PCI0.LPCB.UAR2`.
    path: string<128>,
    /// `_HID`, as text (`PNP0501`; an EISA number is converted, as the specification says).
    hid: string<16>,
    /// `_CID`, its compatible IDs.
    compatible: list<string<16>, 4>,
    /// `_UID`, if it has one (as text: some firmware gives a string). Empty if none.
    uid: string<16>,
    /// What `_CRS` returned.
    ports: list<Ports, 8>,
    memory: list<Range, 8>,
    lines: list<Line, 4>,
}

/// The devices, from `first` (0 to start); `next` is where to continue, if there are more.
@1 fn devices(first: u32) -> (devices: list<Device, 16>, next: option<u32>) error AcpiError;

/// Where a PCI function's interrupt pin is wired. `pin` is 1 to 4 (INTA to INTD), as the
/// function's configuration space says.
@2 fn route(segment: u16, bus: u8, device: u8, pin: u8) -> (line: Line) error AcpiError;

/// Turns the machine off (x86-64). Answers only if it can't.
@3 fn off() -> () error AcpiError;

/// Signals `bits` on `note` each time the power button is pressed. Replaces any earlier watch.
@4 fn watch_button(note: handle<notification>, bits: u64) -> () error AcpiError;

/// What can go wrong.
flexible enum AcpiError {
    /// No such device, no route for that pin, or no `\_S5`.
    NotFound @1,
    /// Not allowed through this capability.
    NotAllowed @2,
    /// The firmware's method failed or ran too long (the log says which).
    Failed @3,
    /// This machine can't do it this way (ARM64's `off`; no fixed power button).
    NotSupported @4,
}
