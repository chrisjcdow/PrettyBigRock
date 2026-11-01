// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding and describing PCI Express functions (note 0011, section 3).
//!
//! Everything read here came from hardware and may be garbage, so every loop is bounded, every
//! offset is checked and every sum is checked. A function that can't be understood is reported
//! as a [`Problem`] and skipped; it never stops the scan.

use alloc::vec::Vec;

/// The page size BARs have to be aligned to (and large enough for) to be handed to a driver.
pub const PAGE: u64 = 4096;
/// The most functions a scan will list.
pub const MAX_FUNCTIONS: usize = 512;
/// The most capabilities read from one function (the list can't be longer: 48 dwords).
pub const MAX_CAPABILITIES: usize = 48;
/// The largest BAR a driver is given (a sanity limit; the biggest real ones are GPU apertures).
pub const MAX_BAR_SIZE: u64 = 1 << 40;

/// Reads and writes the configuration space of one function: 4096 bytes in dwords.
pub trait Config {
    /// The dword at `offset` (a multiple of 4, below 4096). A function that isn't there reads as
    /// all ones.
    fn read(&mut self, offset: u16) -> u32;
    /// Writes the dword at `offset`.
    fn write(&mut self, offset: u16, value: u32);
}

/// The configuration space of all functions on one segment.
pub trait Bus {
    /// The dword at `offset` of the function at `addr`; all ones if there is none.
    fn read(&mut self, addr: Address, offset: u16) -> u32;
    /// Writes a dword.
    fn write(&mut self, addr: Address, offset: u16, value: u32);
}

/// Where a function is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    /// The bus.
    pub bus: u8,
    /// The device (0 to 31).
    pub device: u8,
    /// The function (0 to 7).
    pub function: u8,
}

impl Address {
    /// The offset of this function's 4 KiB in an ECAM region that starts at bus 0.
    pub fn ecam_offset(self) -> u64 {
        (u64::from(self.bus) << 20)
            | (u64::from(self.device & 31) << 15)
            | (u64::from(self.function & 7) << 12)
    }
}

/// One function of a [`Bus`] as a [`Config`].
pub struct At<'a, B: Bus> {
    /// The bus.
    pub bus: &'a mut B,
    /// The function.
    pub addr: Address,
}

impl<B: Bus> Config for At<'_, B> {
    fn read(&mut self, offset: u16) -> u32 {
        self.bus.read(self.addr, offset)
    }
    fn write(&mut self, offset: u16, value: u32) {
        self.bus.write(self.addr, offset, value)
    }
}

/// What kind of address space a BAR is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarKind {
    /// 32-bit memory.
    Memory32,
    /// 64-bit memory (uses two BAR slots).
    Memory64,
    /// I/O ports.
    Io,
}

/// A base address register, sized and checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    /// Which BAR (0 to 5).
    pub index: u8,
    /// What kind.
    pub kind: BarKind,
    /// Prefetchable memory.
    pub prefetchable: bool,
    /// Where the firmware put it.
    pub base: u64,
    /// How big it is (a power of two).
    pub size: u64,
}

impl Bar {
    /// True if a driver can be given this BAR: memory, page-aligned and whole pages, so that
    /// no other device's registers share its pages.
    pub fn mappable(&self) -> bool {
        self.kind != BarKind::Io && self.size >= PAGE && self.base.is_multiple_of(PAGE)
    }
}

/// Bits of the command register.
pub mod command {
    /// Respond to I/O port accesses.
    pub const IO: u16 = 1 << 0;
    /// Respond to memory accesses.
    pub const MEMORY: u16 = 1 << 1;
    /// Act as a bus master (do DMA, write MSI messages).
    pub const BUS_MASTER: u16 = 1 << 2;
    /// Don't raise the older INTx interrupt lines.
    pub const INTX_DISABLE: u16 = 1 << 10;
}

/// Sets the command register's bits in `set` and clears those in `clear`. Only the command
/// half of the dword is written: the status half has bits that are cleared by writing ones.
pub fn update_command(cfg: &mut impl Config, set: u16, clear: u16) {
    let now = (cfg.read(4) & 0xFFFF) as u16;
    cfg.write(4, u32::from((now | set) & !clear));
}

/// A capability in the function's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    /// Its ID (5 MSI, 9 vendor-specific, 0x10 PCI Express, 0x11 MSI-X).
    pub id: u8,
    /// Where it is in configuration space.
    pub offset: u8,
}

/// Capability IDs.
pub mod cap {
    /// Message-signalled interrupts.
    pub const MSI: u8 = 0x05;
    /// Vendor-specific (virtio's structures).
    pub const VENDOR: u8 = 0x09;
    /// PCI Express.
    pub const EXPRESS: u8 = 0x10;
    /// Extended message-signalled interrupts.
    pub const MSIX: u8 = 0x11;
}

/// A PCI function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Where.
    pub addr: Address,
    /// Vendor ID.
    pub vendor: u16,
    /// Device ID.
    pub device: u16,
    /// Class code.
    pub class: u8,
    /// Sub-class.
    pub subclass: u8,
    /// Programming interface.
    pub prog_if: u8,
    /// Revision.
    pub revision: u8,
    /// The header type (0 endpoint, 1 bridge), without the multi-function bit.
    pub header_type: u8,
    /// The BARs that are implemented and assigned.
    pub bars: Vec<Bar>,
    /// The capability list.
    pub capabilities: Vec<Capability>,
    /// For a bridge: the bus behind it.
    pub secondary_bus: Option<u8>,
}

impl Function {
    /// The BAR with this index, if it's implemented and assigned.
    pub fn bar(&self, index: u8) -> Option<&Bar> {
        self.bars.iter().find(|b| b.index == index)
    }

    /// The first capability with this ID.
    pub fn capability(&self, id: u8) -> Option<Capability> {
        self.capabilities.iter().copied().find(|c| c.id == id)
    }
}

/// Something wrong found during a scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// A memory BAR is implemented but the firmware didn't assign it an address.
    UnassignedBar(u8),
    /// A BAR's size or address makes no sense (not a power of two, overflows, misaligned).
    BadBar(u8),
    /// The capability list loops, leaves configuration space or is too long.
    BadCapabilities,
    /// A bridge's bus numbers make no sense, or lead somewhere already scanned.
    BadBridge,
    /// More functions than [`MAX_FUNCTIONS`].
    TooMany,
}

/// A [`ProblemKind`] and where it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The function.
    pub addr: Address,
    /// What.
    pub kind: ProblemKind,
}

/// What a scan found.
#[derive(Debug, Default)]
pub struct Scan {
    /// The functions, in bus order.
    pub functions: Vec<Function>,
    /// What was wrong along the way.
    pub problems: Vec<Problem>,
}

/// Walks the capability list of a function whose status register says it has one. Stops, with
/// what it had, at the first sign of trouble: an offset that is below 0x40 or runs off the
/// end, or more entries than the space can hold (which means a loop). Returns the capabilities
/// and whether the list was sound.
pub fn capabilities(cfg: &mut impl Config) -> (Vec<Capability>, bool) {
    let mut out = Vec::new();
    if cfg.read(4) >> 16 & 0x10 == 0 {
        return (out, true); // no list
    }
    let mut next = (cfg.read(0x34) & 0xFC) as u16;
    while next != 0 {
        if !(0x40..=0xFC).contains(&next) || out.len() >= MAX_CAPABILITIES {
            return (out, false);
        }
        let head = cfg.read(next);
        out.push(Capability { id: head as u8, offset: next as u8 });
        next = ((head >> 8) & 0xFC) as u16;
    }
    (out, true)
}

/// Sizes the BAR at slot `slot` (0 to 5) of a type 0 header, and restores it. Returns the BAR
/// and how many slots it used, or `Ok(None)` with the slots for one that isn't implemented.
/// The function's memory and I/O decoding must already be off.
fn size_bar(cfg: &mut impl Config, slot: u8) -> Result<(Option<Bar>, u8), ProblemKind> {
    let reg = 0x10 + 4 * u16::from(slot);
    let low = cfg.read(reg);
    cfg.write(reg, 0xFFFF_FFFF);
    let probe = cfg.read(reg);
    cfg.write(reg, low);
    if probe == 0 || probe == 0xFFFF_FFFF && low == 0xFFFF_FFFF {
        return Ok((None, 1)); // not implemented (or nothing there)
    }
    if low & 1 == 1 {
        // I/O ports: 32-bit, bits 0 and 1 are flags.
        let mask = u64::from(probe & !3);
        let size = lowest_bit(mask, 0xFFFF_FFFF).ok_or(ProblemKind::BadBar(slot))?;
        let base = u64::from(low & !3);
        return Ok((
            Some(Bar { index: slot, kind: BarKind::Io, prefetchable: false, base, size }),
            1,
        ));
    }
    let prefetchable = low & 8 != 0;
    match (low >> 1) & 3 {
        0 => {
            let mask = u64::from(probe & !0xF);
            let size = lowest_bit(mask, 0xFFFF_FFFF).ok_or(ProblemKind::BadBar(slot))?;
            let base = u64::from(low & !0xF);
            Ok((Some(Bar { index: slot, kind: BarKind::Memory32, prefetchable, base, size }), 1))
        }
        2 => {
            if slot >= 5 {
                return Err(ProblemKind::BadBar(slot)); // no slot left for the upper half
            }
            let hi_reg = reg + 4;
            let high = cfg.read(hi_reg);
            cfg.write(hi_reg, 0xFFFF_FFFF);
            let hi_probe = cfg.read(hi_reg);
            cfg.write(hi_reg, high);
            let mask = (u64::from(hi_probe) << 32) | u64::from(probe & !0xF);
            let size = lowest_bit(mask, u64::MAX).ok_or(ProblemKind::BadBar(slot))?;
            let base = (u64::from(high) << 32) | u64::from(low & !0xF);
            Ok((Some(Bar { index: slot, kind: BarKind::Memory64, prefetchable, base, size }), 2))
        }
        _ => Err(ProblemKind::BadBar(slot)), // reserved type
    }
}

/// The size a BAR's probe mask means: the mask must be all ones from some bit up to the top of
/// the register (`top`), nothing else. `None` if not.
fn lowest_bit(mask: u64, top: u64) -> Option<u64> {
    let mask = mask & top;
    if mask == 0 {
        return None;
    }
    let size = mask & mask.wrapping_neg();
    (mask == (top & !(size - 1))).then_some(size)
}

/// Reads the identity, BARs and capabilities of the function at `addr`, whose vendor ID
/// register is already known to be valid. Memory decoding is switched off while the BARs are
/// sized and restored afterwards.
fn read_function(bus: &mut impl Bus, addr: Address, problems: &mut Vec<Problem>) -> Function {
    let mut cfg = At { bus, addr };
    let id = cfg.read(0);
    let class = cfg.read(8);
    let header = (cfg.read(0xC) >> 16) as u8 & 0x7F;
    let mut f = Function {
        addr,
        vendor: id as u16,
        device: (id >> 16) as u16,
        class: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
        prog_if: (class >> 8) as u8,
        revision: class as u8,
        header_type: header,
        bars: Vec::new(),
        capabilities: Vec::new(),
        secondary_bus: None,
    };
    let mut report = |kind| problems.push(Problem { addr, kind });
    let (caps, sound) = capabilities(&mut cfg);
    f.capabilities = caps;
    if !sound {
        report(ProblemKind::BadCapabilities);
    }
    let slots = match header {
        0 => 6,
        1 => 2,
        _ => 0,
    };
    if slots > 0 {
        // Only the command half of the register is written: the status half has bits that
        // are cleared by writing ones, and writing zeros leaves them alone.
        let command = cfg.read(4) & 0xFFFF;
        cfg.write(4, command & !0x3); // memory and I/O decoding off while sizing
        let mut slot = 0;
        while slot < slots {
            match size_bar(&mut cfg, slot) {
                Ok((bar, used)) => {
                    if let Some(bar) = bar {
                        match check_bar(&bar) {
                            Ok(()) => f.bars.push(bar),
                            Err(k) => report(k),
                        }
                    }
                    slot += used;
                }
                Err(k) => {
                    report(k);
                    slot += 1;
                }
            }
        }
        cfg.write(4, command);
    }
    if header == 1 {
        let buses = cfg.read(0x18);
        let (primary, secondary, subordinate) =
            (buses as u8, (buses >> 8) as u8, (buses >> 16) as u8);
        if secondary != 0 && secondary <= subordinate && secondary > primary {
            f.secondary_bus = Some(secondary);
        } else if secondary != 0 || subordinate != 0 {
            report(ProblemKind::BadBridge);
        }
    }
    f
}

fn check_bar(bar: &Bar) -> Result<(), ProblemKind> {
    let bad = ProblemKind::BadBar(bar.index);
    if bar.size > MAX_BAR_SIZE {
        return Err(bad);
    }
    if bar.kind == BarKind::Io {
        return Ok(());
    }
    if bar.base == 0 {
        return Err(ProblemKind::UnassignedBar(bar.index));
    }
    // Aligned to its size, as the specification says, and inside the physical address space.
    if !bar.base.is_multiple_of(bar.size)
        || bar.base.checked_add(bar.size).is_none_or(|e| e > 1 << 48)
    {
        return Err(bad);
    }
    Ok(())
}

/// Scans the buses behind `first_bus` on one segment, following bridges, and describes every
/// function found.
pub fn scan(bus: &mut impl Bus, first_bus: u8, last_bus: u8) -> Scan {
    let mut out = Scan::default();
    let mut seen = [false; 256];
    let mut todo = alloc::vec![first_bus];
    while let Some(b) = todo.pop() {
        if seen[usize::from(b)] || b < first_bus || b > last_bus {
            continue;
        }
        seen[usize::from(b)] = true;
        for device in 0..32u8 {
            let function0 = Address { bus: b, device, function: 0 };
            let id = bus.read(function0, 0);
            if id & 0xFFFF == 0xFFFF || id == 0 {
                continue;
            }
            let multi = bus.read(function0, 0xC) >> 16 & 0x80 != 0;
            for function in 0..if multi { 8u8 } else { 1 } {
                let addr = Address { bus: b, device, function };
                if function != 0 && bus.read(addr, 0) & 0xFFFF == 0xFFFF {
                    continue;
                }
                if out.functions.len() >= MAX_FUNCTIONS {
                    out.problems.push(Problem { addr, kind: ProblemKind::TooMany });
                    return out;
                }
                let f = read_function(bus, addr, &mut out.problems);
                if let Some(s) = f.secondary_bus {
                    if seen[usize::from(s)] || s < first_bus || s > last_bus {
                        out.problems.push(Problem { addr, kind: ProblemKind::BadBridge });
                    } else {
                        todo.push(s);
                    }
                }
                out.functions.push(f);
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Rng;
    use alloc::collections::BTreeMap;

    /// A fake function: 1024 dwords of configuration space, with BARs that behave as real ones
    /// do (writes keep only the bits the size allows).
    #[derive(Clone)]
    pub struct Fake {
        pub regs: [u32; 1024],
        /// For each BAR slot: the mask that is writable (0: hard-wired to 0).
        pub masks: [u32; 6],
        /// For each BAR slot: the read-only low bits (the type).
        pub fixed: [u32; 6],
    }

    impl Fake {
        pub fn new(vendor: u16, device: u16, class: u32) -> Fake {
            let mut regs = [0u32; 1024];
            regs[0] = u32::from(vendor) | (u32::from(device) << 16);
            regs[2] = class << 8;
            Fake { regs, masks: [0; 6], fixed: [0; 6] }
        }
        /// A 32-bit memory BAR of `size` bytes at `base`.
        pub fn bar32(mut self, slot: usize, base: u32, size: u32) -> Fake {
            self.masks[slot] = !(size - 1);
            self.regs[4 + slot] = base;
            self
        }
        /// A 64-bit memory BAR (prefetchable) at slots `slot` and `slot + 1`.
        pub fn bar64(mut self, slot: usize, base: u64, size: u64) -> Fake {
            let m = !(size - 1);
            self.masks[slot] = m as u32;
            self.masks[slot + 1] = (m >> 32) as u32;
            self.fixed[slot] = 0xC;
            self.regs[4 + slot] = base as u32 | 0xC;
            self.regs[5 + slot] = (base >> 32) as u32;
            self
        }
        /// An I/O BAR.
        pub fn io(mut self, slot: usize, base: u32, size: u32) -> Fake {
            self.masks[slot] = !(size - 1) & !3;
            self.fixed[slot] = 1;
            self.regs[4 + slot] = base | 1;
            self
        }
        /// A capability list: (id, bytes after the two header bytes) at the given offsets.
        pub fn caps(mut self, caps: &[(u8, u8)]) -> Fake {
            self.regs[1] |= 0x0010_0000; // status: capability list
            self.regs[0x34 / 4] = u32::from(caps[0].1);
            for (i, &(id, at)) in caps.iter().enumerate() {
                let next = caps.get(i + 1).map_or(0, |c| c.1);
                self.regs[usize::from(at) / 4] = u32::from(id) | (u32::from(next) << 8);
            }
            self
        }
        pub fn bridge(mut self, primary: u8, secondary: u8, subordinate: u8) -> Fake {
            self.regs[3] |= 1 << 16;
            self.regs[6] =
                u32::from(primary) | u32::from(secondary) << 8 | u32::from(subordinate) << 16;
            self
        }
        pub fn multi(mut self) -> Fake {
            self.regs[3] |= 0x80 << 16;
            self
        }
        fn slot(offset: u16) -> Option<usize> {
            (0x10..0x28).contains(&offset).then(|| usize::from(offset - 0x10) / 4)
        }
    }

    #[derive(Default)]
    pub struct FakeBus {
        pub functions: BTreeMap<Address, Fake>,
        /// Writes to the command register seen, to check decoding is restored.
        pub commands: Vec<u32>,
    }

    impl Bus for FakeBus {
        fn read(&mut self, addr: Address, offset: u16) -> u32 {
            match self.functions.get(&addr) {
                Some(f) if offset < 4096 => {
                    f.regs.get(usize::from(offset) / 4).copied().unwrap_or(0)
                }
                _ => 0xFFFF_FFFF,
            }
        }
        fn write(&mut self, addr: Address, offset: u16, value: u32) {
            let Some(f) = self.functions.get_mut(&addr) else { return };
            if offset == 4 {
                self.commands.push(value);
            }
            let i = usize::from(offset) / 4;
            if let Some(s) = Fake::slot(offset) {
                f.regs[i] = f.fixed[s] | (value & f.masks[s]);
            } else if offset == 4 {
                f.regs[1] = (f.regs[1] & 0xFFFF_0000) | (value & 0xFFFF);
            } else if i < f.regs.len() && offset != 0 {
                f.regs[i] = value;
            }
        }
    }

    fn a(bus: u8, device: u8, function: u8) -> Address {
        Address { bus, device, function }
    }

    /// A small machine: a host bridge, a virtio disk with a 64-bit BAR and capabilities, a
    /// network card behind a PCI bridge, and a multi-function device.
    pub fn machine() -> FakeBus {
        let mut bus = FakeBus::default();
        bus.functions.insert(a(0, 0, 0), Fake::new(0x8086, 0x29c0, 0x060000));
        bus.functions.insert(
            a(0, 1, 0),
            Fake::new(0x1af4, 0x1042, 0x010000)
                .bar32(1, 0xC100_0000, 0x1000)
                .bar64(4, 0x8_0000_0000, 0x4000)
                .io(0, 0xC000, 0x40)
                .caps(&[(0x09, 0x40), (0x11, 0x50), (0x05, 0x60)]),
        );
        bus.functions.insert(
            a(0, 2, 0),
            Fake::new(0x1234, 0x1111, 0x030000).bar32(0, 0xFD00_0000, 0x100_0000),
        );
        bus.functions.insert(a(0, 3, 0), Fake::new(0x1b36, 0x0001, 0x060400).bridge(0, 1, 2));
        bus.functions.insert(
            a(1, 0, 0),
            Fake::new(0x8086, 0x100e, 0x020000).bar32(0, 0xC200_0000, 0x2_0000),
        );
        bus.functions.insert(
            a(0, 4, 0),
            Fake::new(0x8086, 0x2934, 0x0c0300).bar32(4, 0xC300_0000, 0x1000).multi(),
        );
        bus.functions
            .insert(a(0, 4, 1), Fake::new(0x8086, 0x2935, 0x0c0300).bar32(4, 0xC301_0000, 0x1000));
        bus
    }

    #[test]
    fn finds_the_machine() {
        let mut bus = machine();
        let s = scan(&mut bus, 0, 255);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let addrs: Vec<_> = s.functions.iter().map(|f| f.addr).collect();
        assert_eq!(
            addrs,
            [a(0, 0, 0), a(0, 1, 0), a(0, 2, 0), a(0, 3, 0), a(0, 4, 0), a(0, 4, 1), a(1, 0, 0)]
        );
        let disk = &s.functions[1];
        assert_eq!((disk.vendor, disk.device, disk.class, disk.subclass), (0x1af4, 0x1042, 1, 0));
        assert_eq!(
            disk.bar(1),
            Some(&Bar {
                index: 1,
                kind: BarKind::Memory32,
                prefetchable: false,
                base: 0xC100_0000,
                size: 0x1000
            })
        );
        assert_eq!(
            disk.bar(4),
            Some(&Bar {
                index: 4,
                kind: BarKind::Memory64,
                prefetchable: true,
                base: 0x8_0000_0000,
                size: 0x4000
            })
        );
        assert_eq!(disk.bar(0).map(|b| (b.kind, b.size)), Some((BarKind::Io, 0x40)));
        assert!(disk.bar(0).is_some_and(|b| !b.mappable()));
        assert!(disk.bar(4).is_some_and(Bar::mappable));
        assert_eq!(disk.bar(5), None, "the upper half of a 64-bit BAR isn't a BAR");
        assert_eq!(
            disk.capabilities,
            [
                Capability { id: 9, offset: 0x40 },
                Capability { id: 0x11, offset: 0x50 },
                Capability { id: 5, offset: 0x60 }
            ]
        );
        assert_eq!(disk.capability(cap::MSIX), Some(Capability { id: 0x11, offset: 0x50 }));
        assert_eq!(s.functions[3].secondary_bus, Some(1));
        // The BAR values and the command register are as they were.
        assert_eq!(bus.functions[&a(0, 1, 0)].regs[5], 0xC100_0000);
        assert_eq!(bus.functions[&a(0, 1, 0)].regs[8], 0x8_0000_0000u64 as u32 | 0xC);
        assert_eq!(bus.functions[&a(0, 1, 0)].regs[9], 8);
        // Decoding was switched off and back on once for each of the seven functions.
        assert_eq!(bus.commands.len(), 2 * 7);
        assert!(bus.commands.chunks(2).all(|c| c[0] & 3 == 0 && c[1] == 0));
    }

    #[test]
    fn stays_within_the_buses_it_was_given() {
        let mut bus = machine();
        let s = scan(&mut bus, 0, 0);
        assert!(s.functions.iter().all(|f| f.addr.bus == 0));
        assert!(s.problems.iter().any(|p| p.kind == ProblemKind::BadBridge));
    }

    #[test]
    fn unassigned_and_odd_bars_are_reported() {
        let mut bus = FakeBus::default();
        bus.functions.insert(
            a(0, 1, 0),
            Fake::new(1, 1, 0x010000).bar32(0, 0, 0x1000).bar32(1, 0xC000_0000, 0x1000),
        );
        bus.functions.insert(a(0, 2, 0), Fake::new(2, 2, 0x010000).bar32(0, 0xC000_0100, 0x1000)); // misaligned
        let mut odd = Fake::new(3, 3, 0x010000).bar32(0, 0xC000_0000, 0x1000);
        odd.masks[0] = !0x4000u32 & !0xFFF; // not contiguous: a hole at bit 14
        bus.functions.insert(a(0, 3, 0), odd);
        let mut last = Fake::new(4, 4, 0x010000);
        last.masks[5] = 0xFFFF_F000;
        last.fixed[5] = 0xC;
        last.regs[9] = 0xC | 0xFFFF_0000; // a 64-bit BAR in the last slot
        bus.functions.insert(a(0, 4, 0), last);
        let s = scan(&mut bus, 0, 0);
        let kinds: Vec<_> = s.problems.iter().map(|p| (p.addr.device, p.kind)).collect();
        assert!(kinds.contains(&(1, ProblemKind::UnassignedBar(0))), "{kinds:?}");
        assert!(kinds.contains(&(2, ProblemKind::BadBar(0))), "{kinds:?}");
        assert!(kinds.contains(&(3, ProblemKind::BadBar(0))), "{kinds:?}");
        assert!(kinds.contains(&(4, ProblemKind::BadBar(5))), "{kinds:?}");
        // The good BAR on the first function is still there.
        assert_eq!(s.functions[0].bars.len(), 1);
        assert_eq!(s.functions[0].bars[0].index, 1);
    }

    #[test]
    fn capability_loops_and_wild_pointers_stop() {
        let mut f = Fake::new(1, 1, 0x010000).caps(&[(9, 0x40), (9, 0x50)]);
        f.regs[0x50 / 4] = 9 | (0x40 << 8); // loops back
        let mut bus = FakeBus::default();
        bus.functions.insert(a(0, 1, 0), f);
        let mut wild = Fake::new(2, 2, 0x010000).caps(&[(9, 0x40)]);
        wild.regs[0x40 / 4] = 9 | (0xFF << 8); // 0xFC after masking; reads past the 1024-dword fake as zero, id 0, next 0
        bus.functions.insert(a(0, 2, 0), wild);
        let mut low = Fake::new(3, 3, 0x010000).caps(&[(9, 0x40)]);
        low.regs[0x40 / 4] = 9 | (0x08 << 8); // points into the standard header
        bus.functions.insert(a(0, 3, 0), low);
        let s = scan(&mut bus, 0, 0);
        let bad: Vec<_> = s
            .problems
            .iter()
            .filter(|p| p.kind == ProblemKind::BadCapabilities)
            .map(|p| p.addr.device)
            .collect();
        assert_eq!(bad, [1, 3]);
        assert_eq!(s.functions[0].capabilities.len(), MAX_CAPABILITIES);
        assert_eq!(
            s.functions[2].capabilities.len(),
            1,
            "what was read before the bad pointer is kept"
        );
    }

    #[test]
    fn bridge_loops_are_refused() {
        let mut bus = FakeBus::default();
        bus.functions.insert(a(0, 1, 0), Fake::new(1, 1, 0x060400).bridge(0, 1, 5));
        bus.functions.insert(a(1, 0, 0), Fake::new(1, 2, 0x060400).bridge(1, 1, 5)); // secondary == own bus
        bus.functions.insert(a(1, 1, 0), Fake::new(1, 3, 0x060400).bridge(1, 0, 5)); // back to the root
        bus.functions.insert(a(1, 2, 0), Fake::new(1, 4, 0x060400).bridge(1, 7, 3)); // backwards
        let s = scan(&mut bus, 0, 255);
        assert_eq!(s.functions.len(), 4);
        assert_eq!(s.problems.iter().filter(|p| p.kind == ProblemKind::BadBridge).count(), 3);
    }

    #[test]
    fn nothing_there_reads_as_nothing() {
        let mut bus = FakeBus::default();
        let s = scan(&mut bus, 0, 255);
        assert!(s.functions.is_empty() && s.problems.is_empty());
    }

    #[test]
    fn the_command_register_is_updated_without_touching_status() {
        let mut bus = machine();
        bus.functions.get_mut(&a(0, 1, 0)).unwrap().regs[1] = 0x0010_0000 | 0x0003; // status, command
        let mut cfg = At { bus: &mut bus, addr: a(0, 1, 0) };
        update_command(&mut cfg, command::BUS_MASTER | command::INTX_DISABLE, command::IO);
        let now = cfg.read(4);
        assert_eq!(now & 0xFFFF, 0x0002 | 0x0004 | 0x0400);
        assert_eq!(now >> 16, 0x0010, "the status half is as it was");
        assert_eq!(*cfg.bus.commands.last().unwrap() >> 16, 0, "and was written as zeros");
    }

    #[test]
    fn addresses_in_ecam() {
        assert_eq!(a(0, 0, 0).ecam_offset(), 0);
        assert_eq!(a(1, 2, 3).ecam_offset(), 0x10_0000 + 2 * 0x8000 + 3 * 0x1000);
        assert_eq!(a(255, 31, 7).ecam_offset(), 0x1000_0000 - 0x1000);
    }

    /// Random garbage in every register of every function: the scan must end, within its limits,
    /// without panicking, and everything it reports must make sense.
    #[test]
    fn random_corruption_is_survived() {
        let mut rng = Rng(0x5eed);
        for round in 0..300 {
            let mut bus = machine();
            for f in bus.functions.values_mut() {
                for _ in 0..rng.below(12) {
                    let i = rng.below(64) as usize;
                    f.regs[i] = match rng.below(4) {
                        0 => rng.next() as u32,
                        1 => 0xFFFF_FFFF,
                        2 => 0,
                        _ => f.regs[i] ^ (1 << rng.below(32)),
                    };
                }
                for m in f.masks.iter_mut() {
                    if rng.below(8) == 0 {
                        *m = rng.next() as u32;
                    }
                }
            }
            let first = rng.below(3) as u8;
            let s = scan(&mut bus, first, 255);
            assert!(s.functions.len() <= MAX_FUNCTIONS, "round {round}");
            for f in &s.functions {
                assert!(f.capabilities.len() <= MAX_CAPABILITIES);
                assert!(f.capabilities.iter().all(|c| c.offset >= 0x40 && c.offset % 4 == 0));
                for b in &f.bars {
                    assert!(b.size.is_power_of_two() && b.size <= MAX_BAR_SIZE, "{b:?}");
                    assert!(b.index < 6);
                    if b.kind != BarKind::Io {
                        assert!(
                            b.base % b.size == 0 && b.base != 0 && b.base + b.size <= 1 << 48,
                            "{b:?}"
                        );
                    }
                }
                if let Some(sec) = f.secondary_bus {
                    assert!(sec > f.addr.bus);
                }
            }
        }
    }

    /// A whole function of random bytes, through the capability walker alone.
    #[test]
    fn capability_walk_on_random_space() {
        let mut rng = Rng(11);
        struct Raw([u32; 1024]);
        impl Config for Raw {
            fn read(&mut self, o: u16) -> u32 {
                self.0.get(usize::from(o) / 4).copied().unwrap_or(0xFFFF_FFFF)
            }
            fn write(&mut self, _: u16, _: u32) {}
        }
        for _ in 0..2000 {
            let mut raw = Raw([0; 1024]);
            for w in raw.0.iter_mut() {
                *w = rng.next() as u32;
            }
            let (caps, _) = capabilities(&mut raw);
            assert!(caps.len() <= MAX_CAPABILITIES);
        }
    }
}
