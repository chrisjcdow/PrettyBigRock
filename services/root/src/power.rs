// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the power service is started with (docs/design/0024-the-session-desktop.md,
//! section 7): the machine capability, and on x86-64 the two I/O ports that turn a PC off and
//! read-only slices of the ACPI tables that say how. The root program finds them here, with
//! `lib/platform`'s ACPI code, because it holds all device access; the power service gets only
//! these few.
//!
//! ACPI tables are in physical memory the kernel doesn't manage, so they are read through the
//! device memory capability, a page at a time (as the device manager reads the MCFG table).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use granite::Handle;
use granite::process::role;
use granite::sys::rights;

/// The roles the power service finds the x86 parts at (`services/power` uses the same numbers):
/// PM1a's and PM1b's control ports, then the tables from `TABLES` on.
#[cfg(target_arch = "x86_64")]
const PM1A: u32 = 1001;
#[cfg(target_arch = "x86_64")]
const PM1B: u32 = 1002;
#[cfg(target_arch = "x86_64")]
const TABLES: u32 = 1010;
/// The most tables passed (the DSDT and SSDTs).
#[cfg(target_arch = "x86_64")]
const MAX_TABLES: usize = 16;

/// A copy of the start-up capability at `r`, which stays the root program's.
fn copy(start: &granite::Start, r: u32) -> Option<Handle> {
    let raw = start.peek(r)?;
    // `ManuallyDrop` stops the wrapper closing the original when it goes out of scope.
    ManuallyDrop::new(Handle::from_raw(raw)).duplicate(rights::ALL).ok()
}

/// The power service's capabilities: (role, capability, rights to give), and its arguments.
pub fn caps(start: &granite::Start) -> (Vec<(u32, Handle, u32)>, String) {
    let mut caps = Vec::new();
    let mut args = String::new();
    match copy(start, role::POWER) {
        Some(m) => caps.push((role::POWER, m, rights::ALL)),
        None => granite::println!("The root program has no machine capability to pass on."),
    }
    #[cfg(target_arch = "x86_64")]
    if let Err(e) = x86(start, &mut caps, &mut args) {
        granite::println!("The power service won't be able to turn the computer off: {e}");
    }
    (caps, args)
}

#[cfg(target_arch = "x86_64")]
fn x86(
    start: &granite::Start,
    caps: &mut Vec<(u32, Handle, u32)>,
    args: &mut String,
) -> Result<(), String> {
    use granite::device::{DeviceMemory, IoPorts};
    use granite::memory::Memory;
    use pbr_platform::acpi;

    // Where the ACPI tables start, from the machine description the loader made.
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
    let (pm1a, pm1b) = acpi::fadt_pm1_control(&reader, fadt).ok_or("no PM1 control ports")?;
    let ports = IoPorts(copy(start, role::IO_PORTS).ok_or("no I/O ports")?);
    let port = |p: u16| ports.slice(u64::from(p), 2, rights::READ | rights::WRITE);
    caps.push((PM1A, port(pm1a).map_err(|e| format!("PM1a: {e}"))?.0, rights::ALL));
    if pm1b != 0 {
        caps.push((PM1B, port(pm1b).map_err(|e| format!("PM1b: {e}"))?.0, rights::ALL));
    }
    // The DSDT, then every SSDT: each a read-only slice of whole pages, with where the table
    // is in it.
    let mut tables: Vec<u64> = Vec::new();
    tables.extend(acpi::fadt_dsdt(&reader, fadt));
    acpi::each_table(&reader, machine.rsdp, b"SSDT", |t| tables.push(t));
    for (i, t) in tables.into_iter().take(MAX_TABLES).enumerate() {
        let Some(len) = acpi::table_length(&reader, t) else { continue };
        let first = t & !0xFFF;
        let offset = t - first;
        let pages = (offset + u64::from(len)).next_multiple_of(4096);
        match memory.slice(first, pages, rights::READ | rights::MAP) {
            Ok(slice) => {
                let r = TABLES + i as u32;
                caps.push((r, slice.0, rights::READ | rights::MAP));
                if !args.is_empty() {
                    args.push(' ');
                }
                args.push_str(&format!("table={r},{offset},{len}"));
            }
            Err(e) => granite::println!("The root program can't slice an ACPI table: {e}"),
        }
    }
    Ok(())
}
