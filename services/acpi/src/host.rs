// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The machine as the AML interpreter reaches it (docs/design/0028-acpi.md, section 4).
//!
//! Physical memory is reached through the device memory capability, a page at a time. The
//! kernel refuses pages it manages (ordinary memory) and its own devices, so a mapping that
//! fails is a refusal, which the interpreter treats as Windows does: a read gives 0, a write
//! does nothing. I/O ports go through the I/O ports capability, less the ones Windows keeps
//! from AML. PCI configuration space is reached through the MCFG's regions, or (on a PC
//! without one) through ports `0xCF8` and `0xCFC`, which only this file touches.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use granite::device::{DeviceMemory, IoPorts};
use granite::memory::Mapping;
use granite::sys::{perm, rights};
use granite::time;
use pbr_aml::{Host, PciAddress, Refused};
use pbr_platform::PhysMemory;
use pbr_platform::acpi::EcamRegion;

/// The most pages kept mapped at once.
const MAX_PAGES: usize = 128;

/// The I/O ports Windows doesn't let AML touch, and so ACPICA doesn't either (its
/// `AcpiProtectedPorts`): the old DMA controllers, the interrupt controllers and their trigger
/// registers, the timer, the clock and CMOS, the fast A20 gate, and the PCI configuration
/// ports. Firmware is tested against Windows and never relies on them.
const PROTECTED: &[(u16, u16)] = &[
    (0x0000, 0x000F),
    (0x0020, 0x0021),
    (0x0040, 0x0043),
    (0x0048, 0x004B),
    (0x0070, 0x0076),
    (0x0081, 0x0083),
    (0x0087, 0x0087),
    (0x0089, 0x008B),
    (0x008F, 0x008F),
    (0x0092, 0x0092),
    (0x00A0, 0x00A1),
    (0x00C0, 0x00DF),
    (0x04D0, 0x04D1),
    (0x0CF8, 0x0CFF),
];

/// True if any of the `bytes` ports from `port` is protected.
fn protected(port: u16, bytes: u16) -> bool {
    let last = port.saturating_add(bytes.saturating_sub(1));
    PROTECTED.iter().any(|&(a, b)| port <= b && last >= a)
}

/// The machine.
pub struct Machine {
    memory: DeviceMemory,
    pages: RefCell<BTreeMap<u64, Mapping>>,
    ports: Option<IoPorts>,
    ecam: Vec<EcamRegion>,
    /// The FACS's lock word's physical address, or 0 without one.
    lock: u64,
    /// PM1a and PM1b control (0 if absent), where `GBL_RLS` tells the firmware the lock is free.
    pm1_control: (u16, u16),
    /// Whether verbose messages are logged (`acpi=verbose`).
    pub verbose: bool,
}

impl Machine {
    pub fn new(memory: DeviceMemory, ports: Option<IoPorts>) -> Machine {
        Machine {
            memory,
            pages: RefCell::new(BTreeMap::new()),
            ports,
            ecam: Vec::new(),
            lock: 0,
            pm1_control: (0, 0),
            verbose: false,
        }
    }

    /// Where PCI Express configuration space is (the MCFG's regions).
    pub fn set_ecam(&mut self, regions: Vec<EcamRegion>) {
        self.ecam = regions;
    }

    /// Where the global lock is: the FACS (if it is long enough to hold one), and the PM1
    /// control registers.
    pub fn set_global_lock(&mut self, facs: u64, pm1_control: (u16, u16)) {
        if facs != 0 && self.read_u32(facs + 4).is_some_and(|len| len >= 24) {
            self.lock = facs + 16;
        }
        self.pm1_control = pm1_control;
    }

    /// The address the page at `page` is mapped at (read and write), mapping it if needed.
    fn page(&self, page: u64) -> Option<usize> {
        let mut pages = self.pages.borrow_mut();
        if let Some(m) = pages.get(&page) {
            return Some(m.addr() as usize);
        }
        if pages.len() >= MAX_PAGES {
            pages.clear();
        }
        let slice =
            self.memory.slice(page, 4096, rights::READ | rights::WRITE | rights::MAP).ok()?;
        let mapping = slice.map(0, 4096, perm::READ_WRITE).ok()?;
        let at = mapping.addr() as usize;
        pages.insert(page, mapping);
        Some(at)
    }

    /// Reads `width` bits at a physical address: in one access if it is aligned, otherwise a
    /// byte at a time.
    fn load(&self, address: u64, width: u8) -> Option<u64> {
        let bytes = u64::from(width / 8);
        if bytes == 0 || bytes > 8 {
            return None;
        }
        let within = address & 0xFFF;
        if !address.is_multiple_of(bytes) || within + bytes > 4096 {
            let mut v = 0u64;
            for i in 0..bytes {
                v |= self.load(address.checked_add(i)?, 8)? << (8 * i);
            }
            return Some(v);
        }
        let p = self.page(address & !0xFFF)? + within as usize;
        // SAFETY: `p` is inside the page mapped at `page(...)` (`within + bytes <= 4096`), kept
        // mapped while `pages` holds it (it is only cleared before a new page is mapped, not
        // during this read), and aligned to `bytes`. Device registers are read with one access
        // of the width asked for, which `read_volatile` makes.
        Some(unsafe {
            match bytes {
                1 => u64::from(read_volatile(p as *const u8)),
                2 => u64::from(read_volatile(p as *const u16)),
                4 => u64::from(read_volatile(p as *const u32)),
                _ => read_volatile(p as *const u64),
            }
        })
    }

    /// Writes `width` bits at a physical address, as [`Machine::load`] reads them.
    fn store(&self, address: u64, width: u8, value: u64) -> Option<()> {
        let bytes = u64::from(width / 8);
        if bytes == 0 || bytes > 8 {
            return None;
        }
        let within = address & 0xFFF;
        if !address.is_multiple_of(bytes) || within + bytes > 4096 {
            for i in 0..bytes {
                self.store(address.checked_add(i)?, 8, value >> (8 * i))?;
            }
            return Some(());
        }
        let p = self.page(address & !0xFFF)? + within as usize;
        // SAFETY: as in `load`: inside a mapped, writable page, aligned to the width.
        unsafe {
            match bytes {
                1 => write_volatile(p as *mut u8, value as u8),
                2 => write_volatile(p as *mut u16, value as u16),
                4 => write_volatile(p as *mut u32, value as u32),
                _ => write_volatile(p as *mut u64, value),
            }
        }
        Some(())
    }

    /// Reads a port the service itself uses (not limited by [`PROTECTED`]).
    pub fn port_in(&self, port: u16, width: u8) -> Option<u64> {
        self.ports.as_ref()?.read(u64::from(port), u64::from(width / 8)).ok()
    }

    /// Writes one.
    pub fn port_out(&self, port: u16, width: u8, value: u64) -> Option<()> {
        self.ports.as_ref()?.write(u64::from(port), u64::from(width / 8), value).ok()
    }

    /// The physical address of a function's configuration space byte `offset`, if an MCFG
    /// region covers it.
    fn ecam_address(&self, at: PciAddress, offset: u16) -> Option<u64> {
        let r = self
            .ecam
            .iter()
            .find(|r| r.segment == at.segment && (r.start_bus..=r.end_bus).contains(&at.bus))?;
        let within = (u64::from(at.bus - r.start_bus) << 20)
            | (u64::from(at.device & 0x1F) << 15)
            | (u64::from(at.function & 7) << 12)
            | u64::from(offset & 0xFFF);
        r.base.checked_add(within)
    }

    /// The `0xCF8` address of a function's double word at `offset` (segment 0 only).
    fn cf8(at: PciAddress, offset: u16) -> Option<u64> {
        if at.segment != 0 || offset > 0xFF {
            return None;
        }
        Some(
            0x8000_0000
                | (u64::from(at.bus) << 16)
                | (u64::from(at.device & 0x1F) << 11)
                | (u64::from(at.function & 7) << 8)
                | u64::from(offset & 0xFC),
        )
    }

    /// The lock word, as an atomic integer, if there is a global lock.
    fn lock_word(&self) -> Option<&AtomicU32> {
        if self.lock == 0 {
            return None;
        }
        let p = self.page(self.lock & !0xFFF)? + (self.lock & 0xFFF) as usize;
        if !p.is_multiple_of(4) {
            return None;
        }
        // SAFETY: `p` is a 4-byte-aligned address inside a mapped, writable page that stays
        // mapped while the lock is used (the lock's page is used on every access, and pages are
        // only dropped all at once when a new one is needed, never while this reference is
        // held: it lives only inside the caller's single compare-and-swap loop). The firmware
        // changes the word only with locked instructions too, as the specification requires.
        Some(unsafe { AtomicU32::from_ptr(p as *mut u32) })
    }
}

impl PhysMemory for Machine {
    fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        for (i, b) in buf.iter_mut().enumerate() {
            match addr.checked_add(i as u64).and_then(|a| self.load(a, 8)) {
                Some(v) => *b = v as u8,
                None => return false,
            }
        }
        true
    }
}

impl Host for Machine {
    fn read_memory(&mut self, address: u64, width: u8) -> Result<u64, Refused> {
        self.load(address, width).ok_or(Refused)
    }

    fn write_memory(&mut self, address: u64, width: u8, value: u64) -> Result<(), Refused> {
        self.store(address, width, value).ok_or(Refused)
    }

    fn read_io(&mut self, port: u16, width: u8) -> Result<u64, Refused> {
        if protected(port, u16::from(width / 8)) {
            return Err(Refused);
        }
        self.port_in(port, width).ok_or(Refused)
    }

    fn write_io(&mut self, port: u16, width: u8, value: u64) -> Result<(), Refused> {
        if protected(port, u16::from(width / 8)) {
            return Err(Refused);
        }
        self.port_out(port, width, value).ok_or(Refused)
    }

    fn read_pci(&mut self, at: PciAddress, offset: u16, width: u8) -> Result<u64, Refused> {
        if let Some(a) = self.ecam_address(at, offset) {
            return self.load(a, width).ok_or(Refused);
        }
        let address = Machine::cf8(at, offset).ok_or(Refused)?;
        self.port_out(0xCF8, 32, address).ok_or(Refused)?;
        self.port_in(0xCFC + (offset & 3), width).ok_or(Refused)
    }

    fn write_pci(
        &mut self,
        at: PciAddress,
        offset: u16,
        width: u8,
        value: u64,
    ) -> Result<(), Refused> {
        if let Some(a) = self.ecam_address(at, offset) {
            return self.store(a, width, value).ok_or(Refused);
        }
        let address = Machine::cf8(at, offset).ok_or(Refused)?;
        self.port_out(0xCF8, 32, address).ok_or(Refused)?;
        self.port_out(0xCFC + (offset & 3), width, value).ok_or(Refused)
    }

    fn now(&mut self) -> u64 {
        time::now()
    }

    fn sleep(&mut self, ms: u64) {
        time::sleep_ms(ms);
    }

    fn stall(&mut self, us: u64) {
        // A busy wait: `Stall` is for waits too short to give the processor up for.
        let end = time::now().saturating_add(us.saturating_mul(1000));
        while time::now() < end {
            core::hint::spin_loop();
        }
    }

    /// The specification's protocol (ACPI 6.5, section 5.2.10.1): set "owned"; if it already
    /// was, set "pending" too, and the firmware will say when it lets go.
    fn acquire_global_lock(&mut self) -> bool {
        let Some(word) = self.lock_word() else { return true };
        let mut old = word.load(Ordering::Acquire);
        loop {
            let owned = old & 2 != 0;
            let new = (old & !1) | 2 | u32::from(owned);
            match word.compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return !owned,
                Err(now) => old = now,
            }
        }
    }

    fn release_global_lock(&mut self) {
        let Some(word) = self.lock_word() else { return };
        let mut old = word.load(Ordering::Acquire);
        loop {
            match word.compare_exchange(old, old & !3, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(now) => old = now,
            }
        }
        // The firmware asked to be told: `GBL_RLS`, bit 2 of PM1 control.
        if old & 1 != 0 {
            for port in [self.pm1_control.0, self.pm1_control.1] {
                if port != 0
                    && let Some(v) = self.port_in(port, 16)
                {
                    let _ = self.port_out(port, 16, (v & !(1 << 13)) | (1 << 2));
                }
            }
        }
    }

    fn log(&mut self, verbose: bool, message: fmt::Arguments) {
        if !verbose || self.verbose {
            granite::println!("acpi: {message}");
        }
    }
}
