// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! PCI Express configuration space through ECAM (note 0011, section 3): each bus is 1 MiB of
//! device memory, mapped when first touched.

use alloc::vec::Vec;
use granite::device::DeviceMemory;
use granite::sys::{perm, rights};
use pbr_devices::pci::{Address, Bus};
use pbr_devices::regs::Registers;
use pbr_driver::Mmio;
use pbr_platform::acpi::EcamRegion;

const BUS_SIZE: u64 = 1 << 20;

/// One ECAM region as a [`Bus`].
pub struct Ecam<'a> {
    memory: &'a DeviceMemory,
    region: EcamRegion,
    mapped: Vec<Option<Mmio>>,
    failed: [bool; 256],
}

impl<'a> Ecam<'a> {
    /// The region's configuration space, read through `memory` (all physical addresses).
    pub fn new(memory: &'a DeviceMemory, region: EcamRegion) -> Self {
        Ecam { memory, region, mapped: (0..256).map(|_| None).collect(), failed: [false; 256] }
    }

    /// The device memory for the 4 KiB of configuration space of the function at `addr`, with
    /// every right the device manager has (it is given on to a driver in part).
    pub fn function(&self, addr: Address) -> Option<DeviceMemory> {
        if addr.bus < self.region.start_bus || addr.bus > self.region.end_bus {
            return None;
        }
        let offset = u64::from(addr.bus - self.region.start_bus) * BUS_SIZE
            + (addr.ecam_offset() & (BUS_SIZE - 1));
        self.memory.slice(self.region.base.checked_add(offset)?, 4096, u32::MAX).ok()
    }

    fn bus(&mut self, bus: u8) -> Option<&Mmio> {
        let i = usize::from(bus);
        if bus < self.region.start_bus || bus > self.region.end_bus || self.failed[i] {
            return None;
        }
        if self.mapped[i].is_none() {
            let offset =
                self.region.base.checked_add(u64::from(bus - self.region.start_bus) * BUS_SIZE);
            let mapping = offset
                .and_then(|o| {
                    self.memory.slice(o, BUS_SIZE, rights::READ | rights::WRITE | rights::MAP).ok()
                })
                .and_then(|s| s.map(0, BUS_SIZE, perm::READ_WRITE).ok());
            match mapping {
                Some(m) => self.mapped[i] = Some(Mmio::new(m)),
                None => {
                    self.failed[i] = true;
                    return None;
                }
            }
        }
        self.mapped[i].as_ref()
    }
}

impl Bus for Ecam<'_> {
    fn read(&mut self, addr: Address, offset: u16) -> u32 {
        let within = (addr.ecam_offset() & (BUS_SIZE - 1)) as usize + usize::from(offset & 0xFFC);
        match self.bus(addr.bus) {
            Some(m) => m.read32(within),
            None => u32::MAX,
        }
    }

    fn write(&mut self, addr: Address, offset: u16, value: u32) {
        let within = (addr.ecam_offset() & (BUS_SIZE - 1)) as usize + usize::from(offset & 0xFFC);
        if let Some(m) = self.bus(addr.bus) {
            m.write32(within, value);
        }
    }
}
