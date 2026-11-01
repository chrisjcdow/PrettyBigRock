// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Device access objects (docs/design/0010-device-access.md): ranges of device memory, of
//! interrupt numbers and of I/O ports. The kernel doesn't know what devices exist. It hands the
//! root program one capability of each kind, covering everything except what the kernel keeps
//! for itself, and `*_slice` makes narrower ones.

use super::Error;
use super::budget::Charge;
use crate::mem::PAGE;
use crate::sync::Once;
use crate::{arch, hw};
use alloc::sync::Arc;
use pbr_bootinfo::{MemoryKind, MemoryRegion};
use pbr_ranges::{Range, RangeSet};

/// The interrupt numbers `[0, WIRED_END)` are real interrupt lines (global system interrupts
/// or GIC interrupt IDs); `[MSI_BASE, MSI_END)` are message-signalled vectors.
pub const WIRED_END: u32 = 1024;
/// The first message-signalled interrupt number.
pub const MSI_BASE: u32 = 1024;
/// One past the last message-signalled interrupt number.
pub const MSI_END: u32 = 2048;

/// The top of the physical addresses the root program's device memory covers.
const PHYSICAL_END: u64 = 1 << 48;

/// What the kernel keeps for itself (note 0010, section 1).
struct Owned {
    memory: RangeSet,
    ports: RangeSet,
}

static OWNED: Once<Owned> = Once::new();

/// A range of device memory: physical addresses that aren't RAM (type 9).
pub struct DeviceMemory {
    /// The physical addresses, page-aligned.
    pub range: Range,
    _charge: Option<Charge>,
}

/// A range of I/O ports (type 10).
pub struct IoPorts {
    /// The port numbers.
    pub range: Range,
    _charge: Option<Charge>,
}

/// What an interrupt capability's range means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum IrqKind {
    /// Real interrupt lines.
    Wired = 0,
    /// The pool message-signalled interrupts are made from.
    Pool = 1,
    /// One message-signalled interrupt.
    Msi = 2,
}

/// A message-signalled interrupt, given back to the pool when the last capability for it goes.
pub struct MsiVector {
    /// Its interrupt number (from `MSI_BASE`).
    pub id: u32,
    /// What the device writes...
    pub address: u64,
    /// ...and where.
    pub data: u32,
}

impl Drop for MsiVector {
    fn drop(&mut self) {
        arch::irqchip::msi_free(self.id);
    }
}

/// A range of interrupt numbers (type 8).
pub struct Interrupts {
    /// The numbers.
    pub range: Range,
    /// What they are.
    pub kind: IrqKind,
    /// For [`IrqKind::Msi`]: the vector, shared by every slice of it.
    pub msi: Option<Arc<MsiVector>>,
    _charge: Option<Charge>,
}

fn range_error(_: pbr_ranges::Error) -> Error {
    Error::InvalidArgument
}

/// Works out what the kernel keeps for itself. Called once at start-up, after the machine has
/// been described.
pub fn init(map: &[MemoryRegion], m: &hw::Machine) {
    let mut memory = RangeSet::new();
    let mut own = |base: u64, len: u64| {
        let start = base & !(PAGE - 1);
        let end = (base + len).div_ceil(PAGE) * PAGE;
        if let Ok(r) = Range::new(start, end - start) {
            memory.add(r);
        }
    };
    // Everything the kernel manages as memory, or keeps for itself. (The ACPI tables stay
    // readable: the device manager reads them through a device memory capability.)
    for r in map {
        let ram = matches!(
            r.kind,
            k if k == MemoryKind::Usable as u32
                || k == MemoryKind::Bad as u32
                || k == MemoryKind::LoaderReclaimable as u32
                || k == MemoryKind::Kernel as u32
                || k == MemoryKind::Module as u32
                || k == MemoryKind::FirmwareRuntime as u32
        );
        if ram && r.range.len != 0 {
            own(r.range.base, r.range.len);
        }
    }
    // The interrupt controllers and timers.
    if m.lapic != 0 {
        own(m.lapic, 0x1000);
    }
    for &(addr, _) in &m.ioapics {
        own(addr, 0x1000);
    }
    if let Some(h) = m.hpet {
        own(h, 0x1000);
    }
    if m.gicd != 0 {
        own(m.gicd, 0x10000);
    }
    if m.gicc != 0 {
        own(m.gicc, 0x2000);
    }
    if let Some((frame, _, _)) = m.msi_frame {
        own(frame, 0x1000);
    }
    if let Some((base, len)) = crate::console::uart_registers() {
        own(base, len);
    }
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
    let mut ports = RangeSet::new();
    #[cfg(target_arch = "x86_64")]
    {
        let mut port = |first: u64, count: u64| {
            if let Ok(r) = Range::new(first, count) {
                ports.add(r);
            }
        };
        port(0x20, 2); // the legacy interrupt controllers
        port(0xA0, 2);
        port(0x40, 4); // the timer the clock is calibrated against
        port(0x61, 1);
        if let Some(p) = crate::console::uart_port() {
            port(u64::from(p), 8);
        }
    }
    OWNED.set(Owned { memory, ports });
}

fn owned() -> &'static Owned {
    OWNED.get().expect("device access isn't set up yet")
}

/// True if `r` touches memory the kernel keeps.
pub fn memory_is_owned(r: &Range) -> bool {
    owned().memory.overlaps(r)
}

impl DeviceMemory {
    /// The root program's: all physical addresses (the parts the kernel keeps are refused when
    /// sliced or mapped).
    pub fn everything() -> Arc<Self> {
        let range = Range::new(0, PHYSICAL_END).expect("a valid range");
        Arc::new(DeviceMemory { range, _charge: None })
    }

    /// `length` bytes from `offset` into this range, charged as `charge`.
    pub fn slice(&self, offset: u64, length: u64, charge: Charge) -> Result<Arc<Self>, Error> {
        let range = self.range.slice(offset, length, PAGE).map_err(range_error)?;
        if memory_is_owned(&range) {
            return Err(Error::InvalidArgument);
        }
        Ok(Arc::new(DeviceMemory { range, _charge: Some(charge) }))
    }
}

impl IoPorts {
    /// The root program's: ports 0 to 65535 (x86-64 only).
    pub fn everything() -> Option<Arc<Self>> {
        if !cfg!(target_arch = "x86_64") {
            return None;
        }
        Some(Arc::new(IoPorts { range: Range::new(0, 0x1_0000).ok()?, _charge: None }))
    }

    /// `count` ports from `first` (an absolute port number inside this range).
    pub fn slice(&self, first: u64, count: u64, charge: Charge) -> Result<Arc<Self>, Error> {
        let offset = first.checked_sub(self.range.start()).ok_or(Error::InvalidArgument)?;
        let range = self.range.slice(offset, count, 1).map_err(range_error)?;
        if owned().ports.overlaps(&range) {
            return Err(Error::InvalidArgument);
        }
        Ok(Arc::new(IoPorts { range, _charge: Some(charge) }))
    }

    /// The port `offset` into the range, if `width` bytes there are inside it and aligned.
    fn port(&self, offset: u64, width: u64) -> Result<u16, Error> {
        if !matches!(width, 1 | 2 | 4) || !offset.is_multiple_of(width) {
            return Err(Error::InvalidArgument);
        }
        let end = offset.checked_add(width).ok_or(Error::InvalidArgument)?;
        if end > self.range.len() {
            return Err(Error::InvalidArgument);
        }
        let r = Range::new(self.range.start() + offset, width).map_err(range_error)?;
        if owned().ports.overlaps(&r) {
            return Err(Error::InvalidArgument);
        }
        Ok(r.start() as u16)
    }

    /// Reads `width` bytes from the port `offset` into the range.
    pub fn read(&self, offset: u64, width: u64) -> Result<u64, Error> {
        let port = self.port(offset, width)?;
        Ok(u64::from(arch::port_in(port, width as u8)))
    }

    /// Writes `width` bytes to the port `offset` into the range.
    pub fn write(&self, offset: u64, width: u64, value: u64) -> Result<(), Error> {
        let port = self.port(offset, width)?;
        arch::port_out(port, width as u8, value as u32);
        Ok(())
    }
}

impl Interrupts {
    /// The root program's range of real interrupt lines, if the machine has any.
    pub fn everything() -> Option<Arc<Self>> {
        let (first, count) = arch::irqchip::wired_range();
        let range = Range::new(u64::from(first), u64::from(count)).ok()?;
        Some(Arc::new(Interrupts { range, kind: IrqKind::Wired, msi: None, _charge: None }))
    }

    /// The root program's pool for message-signalled interrupts, if the machine has them.
    pub fn pool() -> Option<Arc<Self>> {
        if !arch::irqchip::msi_available() {
            return None;
        }
        let range = Range::new(u64::from(MSI_BASE), u64::from(MSI_END - MSI_BASE)).ok()?;
        Some(Arc::new(Interrupts { range, kind: IrqKind::Pool, msi: None, _charge: None }))
    }

    /// `count` numbers from `first` (absolute numbers inside this range).
    pub fn slice(&self, first: u64, count: u64, charge: Charge) -> Result<Arc<Self>, Error> {
        if self.kind == IrqKind::Pool {
            return Err(Error::WrongType);
        }
        let offset = first.checked_sub(self.range.start()).ok_or(Error::InvalidArgument)?;
        let range = self.range.slice(offset, count, 1).map_err(range_error)?;
        Ok(Arc::new(Interrupts {
            range,
            kind: self.kind,
            msi: self.msi.clone(),
            _charge: Some(charge),
        }))
    }

    /// A new message-signalled interrupt made from this pool.
    pub fn allocate(&self, charge: Charge) -> Result<Arc<Self>, Error> {
        if self.kind != IrqKind::Pool {
            return Err(Error::WrongType);
        }
        let v = arch::irqchip::msi_allocate()?;
        let range = Range::new(u64::from(v.id), 1).map_err(range_error)?;
        Ok(Arc::new(Interrupts {
            range,
            kind: IrqKind::Msi,
            msi: Some(Arc::new(v)),
            _charge: Some(charge),
        }))
    }

    /// The interrupt number at position `index` in the range. Ranges of other kinds can't be
    /// bound.
    pub fn number(&self, index: u64) -> Result<u32, Error> {
        if self.kind == IrqKind::Pool || index >= self.range.len() {
            return Err(Error::InvalidArgument);
        }
        let n = self.range.start() + index;
        if self.kind == IrqKind::Wired && !arch::irqchip::wired_usable(n as u32) {
            return Err(Error::InvalidArgument);
        }
        Ok(n as u32)
    }
}
