// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! MSI-X, the way PCI Express functions raise message-signalled interrupts (note 0010,
//! section 4): a table in one of the function's BARs, one entry per vector, each holding the
//! address and data the function writes to raise it. The kernel hands a driver an address and
//! data for each interrupt; the driver writes them here.
//!
//! The capability's fields come from the device, so they are checked: the table must lie inside
//! its BAR and have no more entries than the capability allows.

use crate::pci::{Capability, Config, cap};
use crate::record::Message;
use crate::regs::{Registers, fits};

/// The most vectors a function may have (the specification's 2048).
pub const MAX_VECTORS: u32 = 2048;

/// Where a function's MSI-X table is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// The capability's offset in configuration space.
    pub cap_offset: u16,
    /// How many vectors the table has.
    pub vectors: u16,
    /// The BAR the table is in.
    pub bar: u8,
    /// The table's offset in the BAR.
    pub offset: u32,
}

/// Why MSI-X couldn't be set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The function has no MSI-X capability.
    NotSupported,
    /// The capability describes a table that doesn't fit its BAR, or a BAR we weren't given.
    BadTable,
    /// More messages than the table has vectors.
    TooMany,
}

/// Reads the MSI-X capability, if the function has one.
pub fn info(cfg: &mut impl Config, caps: &[Capability]) -> Option<Info> {
    let c = caps.iter().find(|c| c.id == cap::MSIX)?;
    let at = u16::from(c.offset);
    let head = cfg.read(at);
    let table = cfg.read(at + 4);
    Some(Info {
        cap_offset: at,
        vectors: (((head >> 16) & 0x7FF) + 1) as u16,
        bar: (table & 7) as u8,
        offset: table & !7,
    })
}

/// Checks the table against its BAR's size (0: not given to us).
pub fn check(info: &Info, bar_size: u64) -> Result<(), Error> {
    if bar_size == 0 || info.bar >= 6 || u32::from(info.vectors) > MAX_VECTORS {
        return Err(Error::BadTable);
    }
    if !fits(u64::from(info.offset), 16 * u64::from(info.vectors), bar_size) {
        return Err(Error::BadTable);
    }
    Ok(())
}

/// Sets the table up with `messages` in the first entries and every other entry masked, then
/// turns MSI-X on. Leaves the function unable to raise INTx.
pub fn enable(
    cfg: &mut impl Config,
    info: &Info,
    table: &impl Registers,
    messages: &[Message],
) -> Result<(), Error> {
    if messages.len() > usize::from(info.vectors) {
        return Err(Error::TooMany);
    }
    if !fits(u64::from(info.offset), 16 * u64::from(info.vectors), table.len() as u64) {
        return Err(Error::BadTable);
    }
    let at = info.cap_offset;
    let (enable, mask) = (1u32 << 31, 1u32 << 30);
    // Enabled, with the whole function masked while the table is filled in. Only the enable
    // and mask bits of the dword change (the rest is the capability ID, the next pointer and
    // the table size).
    let keep = cfg.read(at) & !(enable | mask);
    cfg.write(at, keep | enable | mask);
    let base = info.offset as usize;
    for v in 0..usize::from(info.vectors) {
        let e = base + 16 * v;
        match messages.get(v) {
            Some(m) => {
                table.write32(e + 12, 1); // masked while it is written
                table.write32(e, m.address as u32);
                table.write32(e + 4, (m.address >> 32) as u32);
                table.write32(e + 8, m.data);
                table.write32(e + 12, 0);
            }
            None => table.write32(e + 12, 1),
        }
    }
    cfg.write(at, keep | enable);
    Ok(())
}

/// Turns MSI-X off.
pub fn disable(cfg: &mut impl Config, info: &Info) {
    let keep = cfg.read(info.cap_offset) & 0x3FFF_FFFF;
    cfg.write(info.cap_offset, keep);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::Heap as Mem;

    struct Cfg([u32; 64]);
    impl Config for Cfg {
        fn read(&mut self, o: u16) -> u32 {
            self.0[usize::from(o) / 4]
        }
        fn write(&mut self, o: u16, v: u32) {
            self.0[usize::from(o) / 4] = v;
        }
    }

    fn cfg() -> (Cfg, [Capability; 1]) {
        let mut c = Cfg([0; 64]);
        // Capability at 0x50: ID 0x11, next 0, control: 4 vectors (3 + 1); table in BAR 1 at
        // 0x800, pending-bit array in BAR 1 at 0xC00.
        c.0[0x50 / 4] = 0x11 | (3 << 16);
        c.0[0x54 / 4] = 1 | 0x800;
        c.0[0x58 / 4] = 1 | 0xC00;
        (c, [Capability { id: cap::MSIX, offset: 0x50 }])
    }

    #[test]
    fn reads_and_checks_the_capability() {
        let (mut c, caps) = cfg();
        let i = info(&mut c, &caps).unwrap();
        assert_eq!(i, Info { cap_offset: 0x50, vectors: 4, bar: 1, offset: 0x800 });
        assert_eq!(check(&i, 0x1000), Ok(()));
        assert_eq!(check(&i, 0x820), Err(Error::BadTable), "4 entries need 64 bytes");
        assert_eq!(check(&i, 0), Err(Error::BadTable));
        let wild = Info { offset: !7, ..i };
        assert_eq!(check(&wild, 0x1000), Err(Error::BadTable));
        let bar7 = Info { bar: 7, ..i };
        assert_eq!(check(&bar7, 0x1000), Err(Error::BadTable));
        assert_eq!(info(&mut c, &[]), None);
    }

    #[test]
    fn fills_the_table() {
        let (mut c, caps) = cfg();
        let i = info(&mut c, &caps).unwrap();
        let t = Mem::new(0x1000);
        let m = [
            Message { address: 0xFEE0_1000, data: 0x31 },
            Message { address: 0x1_0802_0040, data: 77 },
        ];
        enable(&mut c, &i, &t, &m).unwrap();
        assert_eq!(c.0[0x50 / 4], 0x11 | (3 << 16) | (1 << 31), "enabled, not masked, ID kept");
        assert_eq!(
            (t.read32(0x800), t.read32(0x804), t.read32(0x808), t.read32(0x80C)),
            (0xFEE0_1000, 0, 0x31, 0)
        );
        assert_eq!(
            (t.read32(0x810), t.read32(0x814), t.read32(0x818), t.read32(0x81C)),
            (0x0802_0040, 1, 77, 0)
        );
        assert_eq!(t.read32(0x82C), 1, "unused entries are masked");
        assert_eq!(t.read32(0x83C), 1);
        let too_many = [m[0]; 5];
        assert_eq!(enable(&mut c, &i, &t, &too_many), Err(Error::TooMany));
        let small = Mem::new(0x820);
        assert_eq!(enable(&mut c, &i, &small, &m), Err(Error::BadTable));
        disable(&mut c, &i);
        assert_eq!(c.0[0x50 / 4], 0x11 | (3 << 16));
    }
}
