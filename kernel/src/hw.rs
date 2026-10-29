// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the kernel learns about the machine from the firmware's tables at start-up: the cores,
//! the interrupt controllers and the timers. ACPI for now; device trees come with the
//! Raspberry Pi (milestone M10).

use crate::mem::DirectMapped;
use alloc::vec::Vec;
use pbr_platform::acpi::{self, MadtEntry};

/// One core the firmware listed.
#[derive(Clone, Copy, Debug)]
pub struct Core {
    /// x86 APIC ID, or Arm MPIDR affinity bits.
    pub hw_id: u64,
    /// Arm only: the GIC CPU interface number.
    pub gic_interface: u32,
}

/// The machine, as far as the kernel core needs to know it.
#[derive(Debug, Default)]
pub struct Machine {
    /// Usable cores, the boot core first.
    pub cores: Vec<Core>,
    /// x86: the local APIC's physical address.
    pub lapic: u64,
    /// x86: the HPET's physical address, if there is one.
    pub hpet: Option<u64>,
    /// Arm: the GIC distributor and CPU interface physical addresses, and the GIC version.
    pub gicd: u64,
    /// Arm: see `gicd`.
    pub gicc: u64,
    /// Arm: see `gicd`.
    pub gic_version: u8,
    /// Arm: the virtual timer's interrupt number.
    pub vtimer_irq: u32,
    /// Arm: PSCI is available, and is called with HVC (otherwise SMC).
    pub psci: Option<bool>,
}

/// Reads the ACPI tables. `boot_hw_id` is the boot core's hardware ID, put first in the list.
pub fn discover(rsdp: u64, boot_hw_id: u64) -> Machine {
    let mem = DirectMapped;
    let mut m = Machine { vtimer_irq: 27, ..Machine::default() };
    if rsdp == 0 || !acpi::rsdp_valid(&mem, rsdp) {
        m.cores.push(Core { hw_id: boot_hw_id, gic_interface: 0 });
        return m;
    }
    if let Some(madt) = acpi::find_table(&mem, rsdp, b"APIC") {
        let mut lapic_override = None;
        let header = acpi::madt(&mem, madt, |e| match e {
            MadtEntry::LocalApic { apic_id, enabled: true }
            | MadtEntry::X2Apic { apic_id, enabled: true } => {
                if !m.cores.iter().any(|c| c.hw_id == u64::from(apic_id)) {
                    m.cores.push(Core { hw_id: u64::from(apic_id), gic_interface: 0 });
                }
            }
            MadtEntry::LapicAddress(a) => lapic_override = Some(a),
            MadtEntry::Gicc { interface, mpidr, base, enabled: true } => {
                m.cores.push(Core { hw_id: mpidr & 0xFF_00FF_FFFF, gic_interface: interface });
                m.gicc = base;
            }
            MadtEntry::Gicd { base, version } => {
                m.gicd = base;
                m.gic_version = version;
            }
            _ => {}
        });
        m.lapic = lapic_override.or(header).unwrap_or(0xFEE0_0000);
    }
    if let Some(t) = acpi::find_table(&mem, rsdp, b"HPET") {
        m.hpet = acpi::hpet_base(&mem, t);
    }
    if let Some(t) = acpi::find_table(&mem, rsdp, b"GTDT") {
        m.vtimer_irq = acpi::gtdt_virtual_timer(&mem, t).filter(|&i| i != 0).unwrap_or(27);
    }
    if let Some(t) = acpi::find_table(&mem, rsdp, b"FACP") {
        m.psci = acpi::fadt_psci(&mem, t).and_then(|(psci, hvc)| psci.then_some(hvc));
    }
    // The boot core goes first, so core numbers match start-up order.
    if let Some(i) = m.cores.iter().position(|c| c.hw_id == boot_hw_id) {
        let boot = m.cores.remove(i);
        m.cores.insert(0, boot);
    } else {
        m.cores.insert(0, Core { hw_id: boot_hw_id, gic_interface: 0 });
    }
    m
}
