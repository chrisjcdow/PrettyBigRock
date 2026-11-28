// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the ACPI service is started with (docs/design/0028-acpi.md, section 4). The root
//! program holds all device access (note 0010, section 8) and passes on:
//! - the machine description, which says where the ACPI tables are;
//! - device memory for every physical address, **without** the right to make DMA memory: the
//!   firmware's AML may name a region anywhere, so nothing short of all of it would do, and the
//!   kernel still refuses ordinary memory and its own devices;
//! - on x86-64, every I/O port (the service keeps the few Windows protects from AML away from
//!   it), and the SCI: the one interrupt line the FADT names, which it may bind and acknowledge
//!   but nothing else.

use alloc::string::String;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use granite::Handle;
use granite::process::role;
use granite::sys::rights;

/// The role the ACPI service finds the SCI at (`services/acpi`'s `SCI`).
#[cfg(target_arch = "x86_64")]
const SCI: u32 = 1001;

/// A copy of the start-up capability at `r`, which stays the root program's.
fn copy(start: &granite::Start, r: u32) -> Option<Handle> {
    let raw = start.peek(r)?;
    // `ManuallyDrop` stops the wrapper closing the original when it goes out of scope.
    ManuallyDrop::new(Handle::from_raw(raw)).duplicate(rights::ALL).ok()
}

/// The ACPI service's capabilities: (role, capability, rights to give), and its arguments
/// (`verbose` when the kernel's command line has `acpi=verbose`).
pub fn caps(start: &granite::Start) -> (Vec<(u32, Handle, u32)>, String) {
    let mut caps = Vec::new();
    if let Some(m) = copy(start, role::MACHINE) {
        caps.push((role::MACHINE, m, rights::READ | rights::MAP));
    }
    if let Some(m) = copy(start, role::DEVICE_MEMORY) {
        caps.push((role::DEVICE_MEMORY, m, rights::ALL & !rights::DMA));
    }
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(p) = copy(start, role::IO_PORTS) {
            caps.push((role::IO_PORTS, p, rights::READ | rights::WRITE));
        }
        match sci(start) {
            Ok(line) => caps.push((SCI, line, rights::BIND | rights::ACKNOWLEDGE)),
            Err(e) => granite::println!("The ACPI service won't see the power button: {e}"),
        }
    }
    let verbose = start.args().split_whitespace().any(|w| w == "acpi=verbose");
    (caps, String::from(if verbose { "verbose" } else { "" }))
}

/// The SCI, as a one-line slice of the interrupts: the FADT's `SCI_INT`, an ISA line number,
/// which the interrupts capability numbers from its first line as the device manager's legacy
/// devices are.
#[cfg(target_arch = "x86_64")]
fn sci(start: &granite::Start) -> Result<Handle, String> {
    use alloc::format;
    use granite::device::{DeviceMemory, Interrupts};
    use granite::memory::Memory;
    use pbr_platform::acpi;

    let description = Memory(copy(start, role::MACHINE).ok_or("no machine description")?);
    let size = description.size().map_err(|e| format!("{e}"))?.min(4096) as usize;
    let mut page = alloc::vec![0u8; size];
    description.read(0, &mut page).map_err(|e| format!("reading the description: {e}"))?;
    let machine =
        pbr_devices::machine::Machine::decode(&page, pbr_devices::machine::FRAMEBUFFER_RECORD)
            .map_err(|e| format!("the machine description: {e:?}"))?;
    if machine.rsdp == 0 {
        return Err("there are no ACPI tables".into());
    }
    let memory = DeviceMemory(copy(start, role::DEVICE_MEMORY).ok_or("no device memory")?);
    let reader = crate::phys::PhysReader::new(&memory);
    let fadt = acpi::find_table(&reader, machine.rsdp, b"FACP").ok_or("no FADT")?;
    let fixed = acpi::fadt_fixed(&reader, fadt).ok_or("the FADT is too short")?;
    let lines = Interrupts(copy(start, role::INTERRUPTS).ok_or("no interrupts")?);
    let (_, _, first) = lines.info().map_err(|e| format!("the interrupts: {e}"))?;
    let line = lines
        .slice(first + u64::from(fixed.sci), 1, rights::ALL)
        .map_err(|e| format!("the SCI (line {}): {e}", fixed.sci))?;
    Ok(line.0)
}
