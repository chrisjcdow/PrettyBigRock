// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The device record (note 0011, section 6): the page a driver reads at start-up to learn what
//! its device is and what each of its capabilities is. The device manager writes it; the
//! driver reads it, and checks every field, because a bug in the device manager must not become
//! a crash in a driver.

/// The size of an encoded record.
pub const SIZE: usize = 232;
/// The most interrupts a record can describe.
pub const MAX_INTERRUPTS: usize = 8;
/// The most BARs.
pub const MAX_BARS: usize = 6;

/// What kind of device a record describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    /// A PCI Express function.
    Pci = 1,
    /// A legacy device (ports and a numbered interrupt).
    Legacy = 2,
    /// The firmware's framebuffer.
    Framebuffer = 3,
}

/// What to program into a device so it raises an interrupt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// The address (legacy interrupts: 0).
    pub address: u64,
    /// The data (legacy interrupts: the interrupt number).
    pub data: u32,
}

/// The framebuffer's shape (kind 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
    /// 1: blue, green, red, unused; 2: red, green, blue, unused (`pbr.display`'s `PixelFormat`).
    pub format: u32,
}

/// A device record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// What kind of device.
    pub kind: Kind,
    /// PCI vendor ID.
    pub vendor: u16,
    /// PCI device ID.
    pub device: u16,
    /// PCI class code.
    pub class: u8,
    /// PCI sub-class.
    pub subclass: u8,
    /// PCI programming interface.
    pub prog_if: u8,
    /// PCI revision.
    pub revision: u8,
    /// The size of each BAR the driver was given (0: not given).
    pub bars: [u64; MAX_BARS],
    /// The interrupts the driver was given, in role order.
    pub interrupts: [Message; MAX_INTERRUPTS],
    /// How many of `interrupts` are real.
    pub interrupt_count: u8,
    /// The bus address of the DMA memory (0: none).
    pub dma_bus: u64,
    /// The size of the DMA memory.
    pub dma_size: u64,
    /// For [`Kind::Framebuffer`].
    pub screen: Screen,
}

/// What was wrong with a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than [`SIZE`].
    TooShort,
    /// The magic is wrong.
    BadMagic,
    /// A version this code doesn't know.
    BadVersion,
    /// An unknown kind.
    BadKind,
    /// A count is out of range or disagrees with the fields.
    BadCount,
    /// A reserved field is not zero.
    NotZero,
}

impl Record {
    /// An empty record of this kind.
    pub fn new(kind: Kind) -> Record {
        Record {
            kind,
            vendor: 0,
            device: 0,
            class: 0,
            subclass: 0,
            prog_if: 0,
            revision: 0,
            bars: [0; MAX_BARS],
            interrupts: [Message::default(); MAX_INTERRUPTS],
            interrupt_count: 0,
            dma_bus: 0,
            dma_size: 0,
            screen: Screen::default(),
        }
    }

    /// The layout of note 0011, section 6.
    pub fn encode(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        b[0..4].copy_from_slice(b"PBRD");
        b[4..6].copy_from_slice(&1u16.to_le_bytes());
        b[6..8].copy_from_slice(&(self.kind as u16).to_le_bytes());
        b[8..10].copy_from_slice(&self.vendor.to_le_bytes());
        b[10..12].copy_from_slice(&self.device.to_le_bytes());
        b[12..16].copy_from_slice(&[self.class, self.subclass, self.prog_if, self.revision]);
        b[16] = self.bars.iter().filter(|&&s| s != 0).count() as u8;
        b[17] = self.interrupt_count;
        for (i, s) in self.bars.iter().enumerate() {
            b[24 + 8 * i..32 + 8 * i].copy_from_slice(&s.to_le_bytes());
        }
        for (i, m) in self.interrupts.iter().enumerate() {
            b[72 + 16 * i..80 + 16 * i].copy_from_slice(&m.address.to_le_bytes());
            b[80 + 16 * i..84 + 16 * i].copy_from_slice(&m.data.to_le_bytes());
        }
        b[200..208].copy_from_slice(&self.dma_bus.to_le_bytes());
        b[208..216].copy_from_slice(&self.dma_size.to_le_bytes());
        b[216..220].copy_from_slice(&self.screen.width.to_le_bytes());
        b[220..224].copy_from_slice(&self.screen.height.to_le_bytes());
        b[224..228].copy_from_slice(&self.screen.stride.to_le_bytes());
        b[228..232].copy_from_slice(&self.screen.format.to_le_bytes());
        b
    }

    /// Reads and checks a record. Extra bytes after [`SIZE`] are ignored (later versions only
    /// add fields at the end); a newer minor version is fine, a different major is not.
    pub fn decode(b: &[u8]) -> Result<Record, Error> {
        if b.len() < SIZE {
            return Err(Error::TooShort);
        }
        if &b[0..4] != b"PBRD" {
            return Err(Error::BadMagic);
        }
        if u16::from_le_bytes([b[4], b[5]]) != 1 {
            return Err(Error::BadVersion);
        }
        let kind = match u16::from_le_bytes([b[6], b[7]]) {
            1 => Kind::Pci,
            2 => Kind::Legacy,
            3 => Kind::Framebuffer,
            _ => return Err(Error::BadKind),
        };
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64_at = |o: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[o..o + 8]);
            u64::from_le_bytes(a)
        };
        let mut r = Record::new(kind);
        r.vendor = u16::from_le_bytes([b[8], b[9]]);
        r.device = u16::from_le_bytes([b[10], b[11]]);
        (r.class, r.subclass, r.prog_if, r.revision) = (b[12], b[13], b[14], b[15]);
        for i in 0..MAX_BARS {
            r.bars[i] = u64_at(24 + 8 * i);
        }
        if usize::from(b[16]) != r.bars.iter().filter(|&&s| s != 0).count() {
            return Err(Error::BadCount);
        }
        r.interrupt_count = b[17];
        if usize::from(r.interrupt_count) > MAX_INTERRUPTS {
            return Err(Error::BadCount);
        }
        if b[18..24].iter().any(|&x| x != 0) {
            return Err(Error::NotZero);
        }
        for i in 0..MAX_INTERRUPTS {
            r.interrupts[i] = Message { address: u64_at(72 + 16 * i), data: u32_at(80 + 16 * i) };
        }
        r.dma_bus = u64_at(200);
        r.dma_size = u64_at(208);
        r.screen = Screen {
            width: u32_at(216),
            height: u32_at(220),
            stride: u32_at(224),
            format: u32_at(228),
        };
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    fn sample() -> Record {
        let mut r = Record::new(Kind::Pci);
        r.vendor = 0x1af4;
        r.device = 0x1042;
        (r.class, r.subclass, r.prog_if, r.revision) = (1, 0, 0, 2);
        r.bars[1] = 0x1000;
        r.bars[4] = 0x4000;
        r.interrupts[0] = Message { address: 0xFEE0_0000, data: 0x31 };
        r.interrupt_count = 1;
        r.dma_bus = 0x1234_5000;
        r.dma_size = 0x10_0000;
        r
    }

    #[test]
    fn round_trips() {
        let r = sample();
        let b = r.encode();
        assert_eq!(&b[0..4], b"PBRD");
        assert_eq!(b[16], 2, "two BARs given");
        assert_eq!(Record::decode(&b), Ok(r));
        let mut f = Record::new(Kind::Framebuffer);
        f.screen = Screen { width: 1280, height: 800, stride: 5120, format: 1 };
        assert_eq!(Record::decode(&f.encode()), Ok(f));
    }

    #[test]
    fn refuses_what_is_wrong() {
        let good = sample().encode();
        assert_eq!(Record::decode(&good[..SIZE - 1]), Err(Error::TooShort));
        let mut b = good;
        b[0] = b'X';
        assert_eq!(Record::decode(&b), Err(Error::BadMagic));
        let mut b = good;
        b[4] = 2;
        assert_eq!(Record::decode(&b), Err(Error::BadVersion));
        let mut b = good;
        b[6] = 9;
        assert_eq!(Record::decode(&b), Err(Error::BadKind));
        let mut b = good;
        b[16] = 5;
        assert_eq!(Record::decode(&b), Err(Error::BadCount));
        let mut b = good;
        b[17] = 9;
        assert_eq!(Record::decode(&b), Err(Error::BadCount));
        let mut b = good;
        b[20] = 1;
        assert_eq!(Record::decode(&b), Err(Error::NotZero));
        // Longer is fine.
        let mut long = good.to_vec();
        long.extend_from_slice(&[0xAA; 64]);
        assert!(Record::decode(&long).is_ok());
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut rng = Rng(7);
        for _ in 0..2000 {
            let mut b = sample().encode();
            for _ in 0..1 + rng.below(8) {
                b[rng.below(SIZE as u64) as usize] = rng.next() as u8;
            }
            if let Ok(r) = Record::decode(&b) {
                assert!(usize::from(r.interrupt_count) <= MAX_INTERRUPTS);
            }
        }
    }
}
