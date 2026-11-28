// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Operation regions and fields: how AML reads and writes hardware (ACPI 6.5, sections 5.5.2.4
//! and 19.6.46).
//!
//! An **operation region** is a range of an address space: system memory, I/O ports, a PCI
//! function's configuration space. A **field** names some bits of a region, such as "bits 10
//! to 12 of the 16-bit register at port `0x404`". Reading or writing the field reads or writes
//! the region in *datums* of the field's access width (a byte, a word, a double or quad
//! word), each at an address that is a multiple of the width; a write that covers only part of
//! a datum fills the rest as the field's update rule says (keep what is there, ones or zeros).
//!
//! Two kinds of field reach a region indirectly: an **index field** writes the datum's offset
//! to one field and then reads or writes another (the classic index and data port pair of a
//! board's I/O chip), and a **bank field** first writes a bank number to a field that chooses
//! which registers the region shows.
//!
//! Every access is checked against its region's length, and goes through the [`Host`], which
//! may refuse it (note 0028, section 4); a refused read gives 0 and a refused write does
//! nothing, logged once per region.

use crate::code::Code;
use crate::namespace::{NodeId, Object};
use crate::value::{BufferField, Value};
use crate::{Aml, Error, Host, PciAddress, Refused, Result, limits};
use alloc::vec;
use alloc::vec::Vec;

/// The address spaces (ACPI 6.5, section 19.6.100).
pub(crate) mod space {
    pub const MEMORY: u8 = 0;
    pub const IO: u8 = 1;
    pub const PCI: u8 = 2;
    pub const EMBEDDED: u8 = 3;
    pub const SMBUS: u8 = 4;
    pub const CMOS: u8 = 5;
}

/// The name of an address space, for the log.
fn space_name(s: u8) -> &'static str {
    match s {
        space::MEMORY => "SystemMemory",
        space::IO => "SystemIO",
        space::PCI => "PCI_Config",
        space::EMBEDDED => "EmbeddedControl",
        space::SMBUS => "SMBus",
        space::CMOS => "SystemCMOS",
        6 => "PciBarTarget",
        7 => "IPMI",
        8 => "GeneralPurposeIO",
        9 => "GenericSerialBus",
        10 => "PCC",
        _ => "an OEM space",
    }
}

/// An operation region.
pub(crate) struct Region {
    pub space: u8,
    pub place: Place,
    /// For a PCI configuration region, the function, worked out at its first use.
    pub pci: Option<PciAddress>,
}

/// Where a region is. A region declared in a table, outside any method, has its offset and
/// length worked out at its first use, as ACPICA does: they may name objects that come later
/// in the tables.
pub(crate) enum Place {
    Known { offset: u64, length: u64 },
    Later { code: Code, scope: NodeId },
}

/// A field: which bits, of what, and how they are reached.
pub(crate) struct FieldUnit {
    pub reach: Reach,
    /// Its first bit, from the start of the region (or of the index field's whole range).
    pub bit: u64,
    pub bits: u64,
    /// The access width in bytes (1, 2, 4 or 8), or 0 for "any".
    pub access: u8,
    /// Whether the global lock must be held around each access.
    pub lock: bool,
    /// What a partial write puts in the rest of a datum: 0 keep, 1 ones, 2 zeros.
    pub update: u8,
}

pub(crate) enum Reach {
    Region(NodeId),
    Index { index: NodeId, data: NodeId },
    Bank { region: NodeId, bank: NodeId, value: u64 },
}

/// Bits `n` (at most 64) of `src` from bit `at`, as an integer.
fn get_bits(src: &[u8], at: u64, n: u64) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        let b = at + i;
        let byte = src.get((b / 8) as usize).copied().unwrap_or(0);
        v |= u64::from((byte >> (b % 8)) & 1) << i;
    }
    v
}

/// Puts the low `n` bits (at most 64) of `v` into `dst` from bit `at`.
fn put_bits(dst: &mut [u8], at: u64, n: u64, v: u64) {
    for i in 0..n {
        let b = at + i;
        if let Some(byte) = dst.get_mut((b / 8) as usize) {
            let bit = 1u8 << (b % 8);
            if (v >> i) & 1 != 0 {
                *byte |= bit;
            } else {
                *byte &= !bit;
            }
        }
    }
}

/// A mask of the low `n` bits.
fn mask(n: u64) -> u64 {
    if n >= 64 { u64::MAX } else { (1 << n) - 1 }
}

impl<H: Host> Aml<H> {
    /// The width, in bytes, each datum of `f` is read and written in.
    fn width(&mut self, f: &FieldUnit) -> Result<u64> {
        if f.access != 0 {
            return Ok(u64::from(f.access));
        }
        // "Any": the narrowest that holds the whole field in one aligned datum, as long as the
        // region (if a plain one) is that long; bytes if none does.
        let limit = match &f.reach {
            Reach::Region(r) | Reach::Bank { region: r, .. } => {
                let (space, _, length) = self.region_place(*r)?;
                let most = if space == space::MEMORY { 8 } else { 4 };
                length.min(most)
            }
            Reach::Index { .. } => 1,
        };
        for w in [1u64, 2, 4, 8] {
            if w > limit {
                break;
            }
            let bits = w * 8;
            if f.bit / bits == (f.bit + f.bits.max(1) - 1) / bits {
                return Ok(w);
            }
        }
        Ok(1)
    }

    /// Reads a field.
    pub(crate) fn read_field(&mut self, node: NodeId, f: &FieldUnit) -> Result<Value> {
        if f.bits / 8 > limits::OBJECT as u64 {
            return Err(Error::Limit("a field's length"));
        }
        let w = self.width(f)?;
        let locked = f.lock && self.acquire_global(0xFFFF)?;
        let result = self.read_field_locked(node, f, w);
        if locked {
            self.release_global();
        }
        let out = result?;
        if f.bits <= if self.wide { 64 } else { 32 } {
            Ok(Value::Integer(get_bits(&out, 0, f.bits)))
        } else {
            self.new_buffer(out)
        }
    }

    fn read_field_locked(&mut self, node: NodeId, f: &FieldUnit, w: u64) -> Result<Vec<u8>> {
        if let Reach::Bank { bank, value, .. } = &f.reach {
            self.store_node(*bank, Value::Integer(*value))?;
        }
        let wbits = w * 8;
        let mut out = vec![0u8; f.bits.div_ceil(8) as usize];
        if f.bits == 0 {
            return Ok(out);
        }
        let end = f.bit + f.bits;
        for k in f.bit / wbits..=(end - 1) / wbits {
            self.tick()?;
            let datum = self.read_datum(node, f, k * w, w)?;
            let start = k * wbits;
            let lo = f.bit.max(start);
            let hi = end.min(start + wbits);
            put_bits(&mut out, lo - f.bit, hi - lo, (datum >> (lo - start)) & mask(hi - lo));
        }
        Ok(out)
    }

    /// Writes a field: an integer's bytes, or a buffer's or string's, cut or padded with zeros
    /// to the field's length.
    pub(crate) fn write_field(&mut self, node: NodeId, f: &FieldUnit, v: &Value) -> Result<()> {
        if f.bits / 8 > limits::OBJECT as u64 {
            return Err(Error::Limit("a field's length"));
        }
        let mut src = match self.deref_element(v)? {
            Value::String(s) => s.into_bytes(),
            other => self.bytes(&other)?,
        };
        src.resize(f.bits.div_ceil(8) as usize, 0);
        let w = self.width(f)?;
        let locked = f.lock && self.acquire_global(0xFFFF)?;
        let result = self.write_field_locked(node, f, w, &src);
        if locked {
            self.release_global();
        }
        result
    }

    fn write_field_locked(
        &mut self,
        node: NodeId,
        f: &FieldUnit,
        w: u64,
        src: &[u8],
    ) -> Result<()> {
        if let Reach::Bank { bank, value, .. } = &f.reach {
            self.store_node(*bank, Value::Integer(*value))?;
        }
        if f.bits == 0 {
            return Ok(());
        }
        let wbits = w * 8;
        let end = f.bit + f.bits;
        for k in f.bit / wbits..=(end - 1) / wbits {
            self.tick()?;
            let start = k * wbits;
            let lo = f.bit.max(start);
            let hi = end.min(start + wbits);
            let piece = get_bits(src, lo - f.bit, hi - lo);
            let m = mask(hi - lo) << (lo - start);
            let datum = if m == mask(wbits) {
                piece << (lo - start)
            } else {
                let base = match f.update {
                    1 => u64::MAX,
                    2 => 0,
                    _ => self.read_datum(node, f, k * w, w)?,
                };
                (base & !m) | ((piece << (lo - start)) & m)
            };
            self.write_datum(node, f, k * w, w, datum & mask(wbits))?;
        }
        Ok(())
    }

    /// Reads one datum of `w` bytes at byte `offset` of the field's range.
    fn read_datum(&mut self, node: NodeId, f: &FieldUnit, offset: u64, w: u64) -> Result<u64> {
        match &f.reach {
            Reach::Region(r) | Reach::Bank { region: r, .. } => self.region_read(*r, offset, w),
            Reach::Index { index, data } => {
                self.store_node(*index, Value::Integer(offset))?;
                let v = self.call_or_read(*data, &[])?;
                Ok(self.integer(&v)? & mask(w * 8))
            }
        }
        .inspect_err(|e| self.report_failure(node, e))
    }

    fn write_datum(
        &mut self,
        node: NodeId,
        f: &FieldUnit,
        offset: u64,
        w: u64,
        v: u64,
    ) -> Result<()> {
        match &f.reach {
            Reach::Region(r) | Reach::Bank { region: r, .. } => self.region_write(*r, offset, w, v),
            Reach::Index { index, data } => {
                self.store_node(*index, Value::Integer(offset))?;
                self.store_node(*data, Value::Integer(v))
            }
        }
        .inspect_err(|e| self.report_failure(node, e))
    }

    /// Logs, in verbose mode, which field an access failed in.
    fn report_failure(&mut self, node: NodeId, e: &Error) {
        if self.verbose {
            let path = self.ns.path(node);
            self.host.log(true, format_args!("field {path}: {e}"));
        }
    }

    /// The region's space, offset and length, working the offset and length out if this is
    /// its first use.
    pub(crate) fn region_place(&mut self, r: NodeId) -> Result<(u8, u64, u64)> {
        let region = match self.ns.object(r) {
            Some(Object::Region(region)) => region.clone(),
            _ => return Err(Error::WrongType("a field's region isn't a region")),
        };
        let later = match &region.borrow().place {
            Place::Known { offset, length } => {
                return Ok((region.borrow().space, *offset, *length));
            }
            Place::Later { code, scope } => (code.clone(), *scope),
        };
        let (offset, length) = self.region_arguments(later.0, later.1)?;
        let mut reg = region.borrow_mut();
        reg.place = Place::Known { offset, length };
        Ok((reg.space, offset, length))
    }

    /// Checks an access of `w` bytes at `offset` against the region, and logs its first use in
    /// verbose mode. The absolute address, or `None` if the access is outside the region.
    fn region_access(&mut self, r: NodeId, offset: u64, w: u64) -> Result<(u8, u64)> {
        let (space, base, length) = self.region_place(r)?;
        if offset.checked_add(w).is_none_or(|end| end > length) {
            let path = self.ns.path(r);
            if self.reported.insert((r, 3)) {
                self.host.log(false, format_args!("region {path}: access past its end"));
            }
            return Err(Error::OutOfRange);
        }
        if self.verbose && self.reported.insert((r, 0)) {
            let path = self.ns.path(r);
            let end = base.wrapping_add(length).wrapping_sub(1);
            let name = space_name(space);
            self.host.log(true, format_args!("region {path}: {name} {base:#x}-{end:#x}"));
        }
        Ok((space, base.wrapping_add(offset)))
    }

    /// Logs once per region that an access was refused, or that its space isn't reached.
    fn report_region(&mut self, r: NodeId, refused: bool) {
        if self.reported.insert((r, if refused { 1 } else { 2 })) {
            let path = self.ns.path(r);
            let space = match self.ns.object(r) {
                Some(Object::Region(region)) => region.borrow().space,
                _ => 0,
            };
            let name = space_name(space);
            if refused {
                self.host
                    .log(false, format_args!("region {path} ({name}): access refused, ignored"));
            } else {
                self.host
                    .log(false, format_args!("region {path}: {name} isn't reached in M7, ignored"));
            }
        }
    }

    /// Reads `w` bytes at `offset` in region `r`.
    pub(crate) fn region_read(&mut self, r: NodeId, offset: u64, w: u64) -> Result<u64> {
        let (space, address) = self.region_access(r, offset, w)?;
        let width = (w * 8) as u8;
        let result = match space {
            space::MEMORY => self.host.read_memory(address, width),
            space::IO => {
                let port = address as u16;
                if w == 8 {
                    let lo = self.host.read_io(port, 32);
                    let hi = self.host.read_io(port.wrapping_add(4), 32);
                    lo.and_then(|lo| hi.map(|hi| lo | (hi << 32)))
                } else {
                    self.host.read_io(port, width)
                }
            }
            space::PCI => {
                let at = self.pci_address(r)?;
                let offset = address as u16;
                if w == 8 {
                    let lo = self.host.read_pci(at, offset, 32);
                    let hi = self.host.read_pci(at, offset.wrapping_add(4), 32);
                    lo.and_then(|lo| hi.map(|hi| lo | (hi << 32)))
                } else {
                    self.host.read_pci(at, offset, width)
                }
            }
            _ => {
                self.report_region(r, false);
                return Ok(0);
            }
        };
        match result {
            Ok(v) => Ok(v & mask(w * 8)),
            Err(Refused) => {
                self.report_region(r, true);
                Ok(0)
            }
        }
    }

    /// Writes `w` bytes at `offset` in region `r`.
    pub(crate) fn region_write(&mut self, r: NodeId, offset: u64, w: u64, v: u64) -> Result<()> {
        let (space, address) = self.region_access(r, offset, w)?;
        let width = (w * 8) as u8;
        let result = match space {
            space::MEMORY => self.host.write_memory(address, width, v),
            space::IO => {
                let port = address as u16;
                if w == 8 {
                    let lo = self.host.write_io(port, 32, v & 0xFFFF_FFFF);
                    lo.and(self.host.write_io(port.wrapping_add(4), 32, v >> 32))
                } else {
                    self.host.write_io(port, width, v)
                }
            }
            space::PCI => {
                let at = self.pci_address(r)?;
                let offset = address as u16;
                if w == 8 {
                    let lo = self.host.write_pci(at, offset, 32, v & 0xFFFF_FFFF);
                    lo.and(self.host.write_pci(at, offset.wrapping_add(4), 32, v >> 32))
                } else {
                    self.host.write_pci(at, offset, width, v)
                }
            }
            _ => {
                self.report_region(r, false);
                return Ok(());
            }
        };
        if result.is_err() {
            self.report_region(r, true);
        }
        Ok(())
    }

    /// The PCI function a configuration-space region is in (ACPICA's `AcpiHwDerivePciId`):
    /// the device and function from its device's `_ADR`; the segment and bus from the host
    /// bridge above it (`_SEG`, `_BBN`); then down through each PCI-to-PCI bridge between, to
    /// the bus behind it.
    fn pci_address(&mut self, r: NodeId) -> Result<PciAddress> {
        if let Some(Object::Region(region)) = self.ns.object(r)
            && let Some(at) = region.borrow().pci
        {
            return Ok(at);
        }
        let device = self.ns.parent(r).ok_or(Error::BadState("a region has no device"))?;
        // The chain of devices from the region's up to the host bridge.
        let mut chain = Vec::new();
        let mut at = Some(device);
        let mut bridge = None;
        while let Some(n) = at {
            chain.push(n);
            if self.is_host_bridge(n) {
                bridge = Some(n);
                break;
            }
            at = self.ns.parent(n);
        }
        let mut pci = PciAddress { segment: 0, bus: 0, device: 0, function: 0 };
        if let Some(b) = bridge {
            pci.segment = self.integer_of(b, "_SEG")?.unwrap_or(0) as u16;
            pci.bus = self.integer_of(b, "_BBN")?.unwrap_or(0) as u8;
        } else {
            chain.clear();
            chain.push(device);
        }
        // `chain` runs from the region's device up to the bridge; walk it the other way, the
        // bridge itself left out unless the region is in it.
        let last = chain.len();
        for (i, &n) in chain.iter().rev().enumerate() {
            let is_bridge = bridge == Some(n);
            if is_bridge && last > 1 {
                continue;
            }
            let adr = self.integer_of(n, "_ADR")?.unwrap_or(0);
            pci.device = ((adr >> 16) & 0x1F) as u8;
            pci.function = (adr & 0x7) as u8;
            if i + 1 < last {
                // A device between: if it is a PCI-to-PCI (or CardBus) bridge, the devices
                // below it are on the bus behind it.
                let header = self.host.read_pci(pci, 0x0E, 8).unwrap_or(0) & 0x7F;
                if header == 1 || header == 2 {
                    pci.bus = self.host.read_pci(pci, 0x19, 8).unwrap_or(0) as u8;
                }
            }
        }
        if let Some(Object::Region(region)) = self.ns.object(r) {
            region.borrow_mut().pci = Some(pci);
        }
        Ok(pci)
    }

    /// Reads a buffer field.
    pub(crate) fn read_buffer_field(&mut self, f: &BufferField) -> Result<Value> {
        let b = f.buffer.borrow();
        if f.bit.checked_add(f.bits).is_none_or(|end| end > b.len() * 8) {
            return Err(Error::OutOfRange);
        }
        if !f.always_buffer && f.bits <= if self.wide { 64 } else { 32 } {
            return Ok(Value::Integer(get_bits(&b, f.bit as u64, f.bits as u64)));
        }
        let mut out = vec![0u8; f.bits.div_ceil(8)];
        let mut done = 0;
        while done < f.bits {
            let n = (f.bits - done).min(64);
            put_bits(
                &mut out,
                done as u64,
                n as u64,
                get_bits(&b, (f.bit + done) as u64, n as u64),
            );
            done += n;
        }
        drop(b);
        self.new_buffer(out)
    }

    /// Writes a buffer field.
    pub(crate) fn write_buffer_field(&mut self, f: &BufferField, v: &Value) -> Result<()> {
        let mut src = match self.deref_element(v)? {
            Value::String(s) => s.into_bytes(),
            other => self.bytes(&other)?,
        };
        src.resize(f.bits.div_ceil(8), 0);
        let mut b = f.buffer.borrow_mut();
        if f.bit.checked_add(f.bits).is_none_or(|end| end > b.len() * 8) {
            return Err(Error::OutOfRange);
        }
        let mut done = 0;
        while done < f.bits {
            let n = (f.bits - done).min(64);
            put_bits(
                &mut b,
                (f.bit + done) as u64,
                n as u64,
                get_bits(&src, done as u64, n as u64),
            );
            done += n;
        }
        Ok(())
    }
}
