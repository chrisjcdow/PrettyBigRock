// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The modern virtio PCI transport (virtio 1.x specification, section 4.1): the device's
//! structures are found through vendor-specific PCI capabilities, each pointing at a window of
//! one of the device's BARs.

use pbr_devices::pci::{Capability, Config, cap};
use pbr_devices::regs::{Registers, fits};

/// The device status bits.
pub mod status {
    /// The driver has noticed the device.
    pub const ACKNOWLEDGE: u8 = 1;
    /// The driver knows how to drive it.
    pub const DRIVER: u8 = 2;
    /// The driver is ready.
    pub const DRIVER_OK: u8 = 4;
    /// The driver has accepted the features.
    pub const FEATURES_OK: u8 = 8;
    /// The device has hit an error and needs a reset.
    pub const NEEDS_RESET: u8 = 0x40;
    /// The driver has given up.
    pub const FAILED: u8 = 0x80;
}

/// The feature bit that says the device speaks the modern interface.
pub const VERSION_1: u64 = 1 << 32;

/// The queue vector value that means "no vector".
pub const NO_VECTOR: u16 = 0xFFFF;

/// How many times the status register is read waiting for a reset to finish.
const RESET_POLLS: u32 = 100_000;

/// The common configuration's registers.
mod common {
    pub const DEVICE_FEATURE_SELECT: usize = 0;
    pub const DEVICE_FEATURE: usize = 4;
    pub const DRIVER_FEATURE_SELECT: usize = 8;
    pub const DRIVER_FEATURE: usize = 12;
    pub const CONFIG_MSIX_VECTOR: usize = 16;
    pub const NUM_QUEUES: usize = 18;
    pub const DEVICE_STATUS: usize = 20;
    pub const CONFIG_GENERATION: usize = 21;
    pub const QUEUE_SELECT: usize = 22;
    pub const QUEUE_SIZE: usize = 24;
    pub const QUEUE_MSIX_VECTOR: usize = 26;
    pub const QUEUE_ENABLE: usize = 28;
    pub const QUEUE_NOTIFY_OFF: usize = 30;
    pub const QUEUE_DESC: usize = 32;
    pub const QUEUE_DRIVER: usize = 40;
    pub const QUEUE_DEVICE: usize = 48;
    pub const LENGTH: u32 = 56;
}

/// Why the transport couldn't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The device doesn't have one of the structures it needs (the common, notify or ISR
    /// configuration).
    Missing,
    /// A structure's capability points outside its BAR, is too short, or is misaligned.
    BadStructure,
    /// The device doesn't offer `VERSION_1`: it's an old (legacy-only) device.
    NotModern,
    /// The device didn't accept the features.
    FeaturesRejected,
    /// The device didn't finish resetting.
    ResetTimeout,
    /// The queue doesn't exist, or reports a size that isn't usable.
    BadQueue,
    /// The device didn't take the MSI-X vector.
    NoVector,
    /// The device says it needs a reset.
    NeedsReset,
    /// An offset outside the device-specific configuration.
    OutOfRange,
}

/// A structure's place: a window of a BAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Which BAR.
    pub bar: u8,
    /// Where in the BAR.
    pub offset: u32,
    /// How long.
    pub length: u32,
}

/// Where the structures are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The common configuration.
    pub common: Window,
    /// The notification area.
    pub notify: Window,
    /// Each queue's notification address is `notify_off * multiplier` into the area.
    pub notify_multiplier: u32,
    /// The interrupt status byte.
    pub isr: Window,
    /// The device-specific configuration, if any.
    pub device: Option<Window>,
}

/// Reads the vendor-specific capabilities of a virtio function and checks them against the
/// size of each BAR the driver was given (0: not given).
pub fn find_layout(
    cfg: &mut impl Config,
    caps: &[Capability],
    bar_sizes: &[u64; 6],
) -> Result<Layout, Error> {
    let (mut common_w, mut notify_w, mut isr_w, mut device_w) = (None, None, None, None);
    let mut multiplier = 0;
    for c in caps.iter().filter(|c| c.id == cap::VENDOR) {
        let at = u16::from(c.offset);
        let head = cfg.read(at);
        let (len, kind) = ((head >> 16) & 0xFF, (head >> 24) as u8);
        if len < 16 {
            continue;
        }
        let w = Window {
            bar: cfg.read(at + 4) as u8,
            offset: cfg.read(at + 8),
            length: cfg.read(at + 12),
        };
        let slot = match kind {
            1 => &mut common_w,
            2 => &mut notify_w,
            3 => &mut isr_w,
            4 => &mut device_w,
            _ => continue,
        };
        if slot.is_some() {
            continue; // the first of each kind is the one used
        }
        if usize::from(w.bar) >= 6
            || !fits(w.offset.into(), w.length.into(), bar_sizes[usize::from(w.bar)])
        {
            return Err(Error::BadStructure);
        }
        if !w.offset.is_multiple_of(4) {
            return Err(Error::BadStructure);
        }
        if kind == 2 {
            if len < 20 {
                return Err(Error::BadStructure);
            }
            multiplier = cfg.read(at + 16);
        }
        *slot = Some(w);
    }
    let (common_w, notify_w, isr_w) = (
        common_w.ok_or(Error::Missing)?,
        notify_w.ok_or(Error::Missing)?,
        isr_w.ok_or(Error::Missing)?,
    );
    if common_w.length < common::LENGTH || isr_w.length < 1 || notify_w.length < 2 {
        return Err(Error::BadStructure);
    }
    Ok(Layout {
        common: common_w,
        notify: notify_w,
        notify_multiplier: multiplier,
        isr: isr_w,
        device: device_w,
    })
}

/// The largest power of two that is at most `x` (0 for 0).
pub fn round_down_pow2(x: u16) -> u16 {
    if x == 0 { 0 } else { 1 << (15 - x.leading_zeros()) }
}

/// A virtio device's transport: its structures, in the BARs the driver mapped.
pub struct Transport<R: Registers> {
    bars: [Option<R>; 6],
    layout: Layout,
}

impl<R: Registers> Transport<R> {
    /// Takes the mapped BARs and checks the layout against what is really mapped.
    pub fn new(layout: Layout, bars: [Option<R>; 6]) -> Result<Self, Error> {
        let windows = [Some(layout.common), Some(layout.notify), Some(layout.isr), layout.device];
        for w in windows.into_iter().flatten() {
            let bar =
                bars.get(usize::from(w.bar)).and_then(Option::as_ref).ok_or(Error::BadStructure)?;
            if !fits(w.offset.into(), w.length.into(), bar.len() as u64) {
                return Err(Error::BadStructure);
            }
        }
        Ok(Transport { bars, layout })
    }

    fn bar(&self, w: &Window) -> &R {
        // `new` checked that every window's BAR is mapped.
        self.bars[usize::from(w.bar)].as_ref().expect("a checked BAR")
    }

    fn common(&self) -> (&R, usize) {
        (self.bar(&self.layout.common), self.layout.common.offset as usize)
    }

    fn r8(&self, reg: usize) -> u8 {
        let (b, o) = self.common();
        b.read8(o + reg)
    }
    fn r16(&self, reg: usize) -> u16 {
        let (b, o) = self.common();
        b.read16(o + reg)
    }
    fn r32(&self, reg: usize) -> u32 {
        let (b, o) = self.common();
        b.read32(o + reg)
    }
    fn w8(&self, reg: usize, v: u8) {
        let (b, o) = self.common();
        b.write8(o + reg, v)
    }
    fn w16(&self, reg: usize, v: u16) {
        let (b, o) = self.common();
        b.write16(o + reg, v)
    }
    fn w32(&self, reg: usize, v: u32) {
        let (b, o) = self.common();
        b.write32(o + reg, v)
    }
    fn w64(&self, reg: usize, v: u64) {
        let (b, o) = self.common();
        // Two 32-bit writes: the specification doesn't promise 64-bit accesses work.
        b.write32(o + reg, v as u32);
        b.write32(o + reg + 4, (v >> 32) as u32);
    }

    /// The status register.
    pub fn status(&self) -> u8 {
        self.r8(common::DEVICE_STATUS)
    }

    /// Sets status bits (adds to what is there).
    fn add_status(&self, bits: u8) {
        self.w8(common::DEVICE_STATUS, self.status() | bits);
    }

    /// Tells the device the driver has given up.
    pub fn fail(&self) {
        self.add_status(status::FAILED);
    }

    /// Resets the device and waits for it to finish.
    pub fn reset(&self) -> Result<(), Error> {
        self.w8(common::DEVICE_STATUS, 0);
        for _ in 0..RESET_POLLS {
            if self.status() == 0 {
                return Ok(());
            }
        }
        Err(Error::ResetTimeout)
    }

    /// The handshake up to and including feature negotiation: acknowledges the device,
    /// requires `VERSION_1`, accepts the features in `wanted` that the device offers, and
    /// checks the device agrees. Returns the features accepted.
    pub fn negotiate(&self, wanted: u64) -> Result<u64, Error> {
        self.add_status(status::ACKNOWLEDGE);
        self.add_status(status::DRIVER);
        let mut offered = 0u64;
        for half in 0..2u32 {
            self.w32(common::DEVICE_FEATURE_SELECT, half);
            offered |= u64::from(self.r32(common::DEVICE_FEATURE)) << (32 * half);
        }
        if offered & VERSION_1 == 0 {
            self.fail();
            return Err(Error::NotModern);
        }
        let accepted = (offered & wanted) | VERSION_1;
        for half in 0..2u32 {
            self.w32(common::DRIVER_FEATURE_SELECT, half);
            self.w32(common::DRIVER_FEATURE, (accepted >> (32 * half)) as u32);
        }
        self.add_status(status::FEATURES_OK);
        if self.status() & status::FEATURES_OK == 0 {
            self.fail();
            return Err(Error::FeaturesRejected);
        }
        Ok(accepted)
    }

    /// Declares the driver ready. Queues must be set up first.
    pub fn driver_ok(&self) -> Result<(), Error> {
        self.add_status(status::DRIVER_OK);
        if self.status() & status::NEEDS_RESET != 0 {
            return Err(Error::NeedsReset);
        }
        Ok(())
    }

    /// How many queues the device has.
    pub fn queue_count(&self) -> u16 {
        self.r16(common::NUM_QUEUES)
    }

    /// The largest size queue `index` can have (0 if it doesn't exist).
    pub fn queue_max(&self, index: u16) -> Result<u16, Error> {
        if index >= self.queue_count() {
            return Err(Error::BadQueue);
        }
        self.w16(common::QUEUE_SELECT, index);
        let max = self.r16(common::QUEUE_SIZE);
        if max == 0 { Err(Error::BadQueue) } else { Ok(max) }
    }

    /// Where interrupts about configuration changes go (`None`: nowhere).
    pub fn set_config_vector(&self, vector: Option<u16>) -> Result<(), Error> {
        let want = vector.unwrap_or(NO_VECTOR);
        self.w16(common::CONFIG_MSIX_VECTOR, want);
        if vector.is_some() && self.r16(common::CONFIG_MSIX_VECTOR) != want {
            return Err(Error::NoVector);
        }
        Ok(())
    }

    /// Sets up and enables queue `index` with `size` entries (a power of two, at most
    /// `queue_max`) at these bus addresses, with its interrupt on `vector`. Returns the queue's
    /// notification offset into the notify area.
    pub fn enable_queue(
        &self,
        index: u16,
        size: u16,
        addresses: (u64, u64, u64),
        vector: Option<u16>,
    ) -> Result<u32, Error> {
        let max = self.queue_max(index)?;
        if size == 0 || size > max || !size.is_power_of_two() {
            return Err(Error::BadQueue);
        }
        self.w16(common::QUEUE_SELECT, index);
        self.w16(common::QUEUE_SIZE, size);
        let want = vector.unwrap_or(NO_VECTOR);
        self.w16(common::QUEUE_MSIX_VECTOR, want);
        if vector.is_some() && self.r16(common::QUEUE_MSIX_VECTOR) != want {
            return Err(Error::NoVector);
        }
        self.w64(common::QUEUE_DESC, addresses.0);
        self.w64(common::QUEUE_DRIVER, addresses.1);
        self.w64(common::QUEUE_DEVICE, addresses.2);
        // Read the offset before enabling: it's a property of the queue the device reports.
        let off = u32::from(self.r16(common::QUEUE_NOTIFY_OFF));
        let at = u64::from(off) * u64::from(self.layout.notify_multiplier);
        if !fits(at, 2, u64::from(self.layout.notify.length)) {
            return Err(Error::BadQueue);
        }
        self.w16(common::QUEUE_ENABLE, 1);
        Ok(at as u32)
    }

    /// Tells the device queue `index` has new buffers. `notify_offset` is what
    /// [`Transport::enable_queue`] returned.
    pub fn notify(&self, index: u16, notify_offset: u32) {
        let w = &self.layout.notify;
        self.bar(w).write16((w.offset + notify_offset) as usize, index);
    }

    /// Reads the interrupt status, which also clears it. Bit 0: a queue has used buffers; bit 1:
    /// the configuration changed.
    pub fn isr(&self) -> u8 {
        let w = &self.layout.isr;
        self.bar(w).read8(w.offset as usize)
    }

    /// Reads the device-specific configuration with `f`, retrying if it changes while being
    /// read (the generation counter). `f` is given an accessor that is checked against the
    /// configuration's length.
    pub fn read_config<T>(
        &self,
        f: impl Fn(&DeviceConfig<'_, R>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let w = self.layout.device.ok_or(Error::Missing)?;
        for _ in 0..8 {
            let before = self.r8(common::CONFIG_GENERATION);
            let dc = DeviceConfig { bar: self.bar(&w), window: w };
            let v = f(&dc)?;
            if self.r8(common::CONFIG_GENERATION) == before {
                return Ok(v);
            }
        }
        Err(Error::NeedsReset) // a configuration that never settles
    }
}

/// Checked reads of the device-specific configuration.
pub struct DeviceConfig<'a, R: Registers> {
    bar: &'a R,
    window: Window,
}

impl<R: Registers> DeviceConfig<'_, R> {
    /// The window's length.
    pub fn len(&self) -> u32 {
        self.window.length
    }

    /// True if the configuration is empty.
    pub fn is_empty(&self) -> bool {
        self.window.length == 0
    }

    fn at(&self, offset: u32, width: u32) -> Result<usize, Error> {
        if !offset.is_multiple_of(width)
            || !fits(offset.into(), width.into(), self.window.length.into())
        {
            return Err(Error::OutOfRange);
        }
        Ok((self.window.offset + offset) as usize)
    }

    /// Writes 8 bits (some devices have selector bytes in their configuration).
    pub fn write8(&self, offset: u32, value: u8) -> Result<(), Error> {
        self.bar.write8(self.at(offset, 1)?, value);
        Ok(())
    }

    /// Writes 32 bits (a register the driver acknowledges events in, such as the display
    /// device's `events_clear`).
    pub fn write32(&self, offset: u32, value: u32) -> Result<(), Error> {
        self.bar.write32(self.at(offset, 4)?, value);
        Ok(())
    }

    /// Reads 8 bits.
    pub fn read8(&self, offset: u32) -> Result<u8, Error> {
        Ok(self.bar.read8(self.at(offset, 1)?))
    }

    /// Reads 32 bits.
    pub fn read32(&self, offset: u32) -> Result<u32, Error> {
        Ok(self.bar.read32(self.at(offset, 4)?))
    }

    /// Reads 64 bits (as two 32-bit reads, low first).
    pub fn read64(&self, offset: u32) -> Result<u64, Error> {
        let o = self.at(offset, 8)?;
        Ok(u64::from(self.bar.read32(o)) | (u64::from(self.bar.read32(o + 4)) << 32))
    }
}
