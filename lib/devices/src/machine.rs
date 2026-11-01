// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The machine description page the kernel gives the root program (note 0011, section 2), as
//! the device manager reads it.

/// The size of the fixed part.
pub const HEADER: usize = 64;
/// The most framebuffers.
pub const MAX_FRAMEBUFFERS: u32 = 8;

/// Flag bits.
pub mod flags {
    /// The legacy PS/2 controller is present (x86-64, from the FADT).
    pub const PS2: u32 = 1 << 0;
    /// The message-signalled interrupt pool is present.
    pub const MSI: u32 = 1 << 1;
}

/// What the page says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    /// See [`flags`].
    pub flags: u32,
    /// The ACPI root pointer's physical address, or 0.
    pub rsdp: u64,
    /// The device tree's physical address, or 0.
    pub device_tree: u64,
    /// The device tree's length.
    pub device_tree_len: u64,
    /// How many interrupt numbers the interrupt controllers have.
    pub interrupts: u32,
    /// The number of framebuffer records after the header.
    pub framebuffers: u32,
}

/// What was wrong with the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than the fixed part, or than its framebuffer records need.
    TooShort,
    /// The magic is wrong.
    BadMagic,
    /// A major version this code doesn't know.
    BadVersion,
    /// More framebuffers than allowed.
    TooMany,
}

impl Machine {
    /// Reads the page. `record_size` is the size of one framebuffer record.
    pub fn decode(b: &[u8], record_size: usize) -> Result<Machine, Error> {
        if b.len() < HEADER {
            return Err(Error::TooShort);
        }
        if &b[0..8] != b"PBRMACH\0" {
            return Err(Error::BadMagic);
        }
        if u16::from_le_bytes([b[8], b[9]]) != 1 {
            return Err(Error::BadVersion);
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64_at = |o: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[o..o + 8]);
            u64::from_le_bytes(a)
        };
        let framebuffers = u32_at(44);
        if framebuffers > MAX_FRAMEBUFFERS {
            return Err(Error::TooMany);
        }
        let needed = HEADER.saturating_add(record_size.saturating_mul(framebuffers as usize));
        if b.len() < needed {
            return Err(Error::TooShort);
        }
        Ok(Machine {
            flags: u32_at(12),
            rsdp: u64_at(16),
            device_tree: u64_at(24),
            device_tree_len: u64_at(32),
            interrupts: u32_at(40),
            framebuffers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(fbs: u32) -> alloc::vec::Vec<u8> {
        let mut b = alloc::vec![0u8; HEADER + 40 * fbs as usize];
        b[..8].copy_from_slice(b"PBRMACH\0");
        b[8] = 1;
        b[12] = 3;
        b[16..24].copy_from_slice(&0xE_0000u64.to_le_bytes());
        b[40..44].copy_from_slice(&24u32.to_le_bytes());
        b[44..48].copy_from_slice(&fbs.to_le_bytes());
        b
    }

    #[test]
    fn reads_a_page() {
        let m = Machine::decode(&page(1), 40).unwrap();
        assert_eq!((m.flags, m.rsdp, m.interrupts, m.framebuffers), (3, 0xE_0000, 24, 1));
        assert_eq!(m.flags & flags::PS2, flags::PS2);
        assert_eq!(m.flags & flags::MSI, flags::MSI);
        // A newer minor version, with fields added at the end, is fine.
        let mut b = page(0);
        b[10] = 5;
        b.extend_from_slice(&[9; 100]);
        assert!(Machine::decode(&b, 40).is_ok());
    }

    #[test]
    fn refuses_what_is_wrong() {
        assert_eq!(Machine::decode(&page(0)[..63], 40), Err(Error::TooShort));
        assert_eq!(Machine::decode(&page(2)[..HEADER + 40], 40), Err(Error::TooShort));
        let mut b = page(0);
        b[0] = b'X';
        assert_eq!(Machine::decode(&b, 40), Err(Error::BadMagic));
        let mut b = page(0);
        b[8] = 2;
        assert_eq!(Machine::decode(&b, 40), Err(Error::BadVersion));
        let mut b = page(0);
        b[44] = 9;
        assert_eq!(Machine::decode(&b, 40), Err(Error::TooMany));
        let mut b = page(0);
        b[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Machine::decode(&b, usize::MAX), Err(Error::TooMany));
    }
}
