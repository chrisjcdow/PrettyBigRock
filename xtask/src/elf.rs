// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Just enough of the ELF format to check what the build produced.

/// ELF machine number for x86-64.
pub const EM_X86_64: u16 = 62;
/// ELF machine number for AArch64.
pub const EM_AARCH64: u16 = 183;

/// The parts of a 64-bit ELF header the build tool checks.
#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    /// The processor it's for.
    pub machine: u16,
    /// The entry point's address.
    pub entry: u64,
}

impl Header {
    /// Reads the header of a 64-bit little-endian executable.
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" {
            return Err("not an ELF file");
        }
        if bytes[4] != 2 {
            return Err("not a 64-bit ELF file");
        }
        if bytes[5] != 1 {
            return Err("not little-endian");
        }
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        if u16_at(16) != 2 {
            return Err("not an executable (ET_EXEC)");
        }
        let mut entry = [0; 8];
        entry.copy_from_slice(&bytes[24..32]);
        Ok(Header { machine: u16_at(18), entry: u64::from_le_bytes(entry) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(class: u8, kind: u16, machine: u16, entry: u64) -> Vec<u8> {
        let mut b = vec![0u8; 64];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = class;
        b[5] = 1;
        b[16..18].copy_from_slice(&kind.to_le_bytes());
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b[24..32].copy_from_slice(&entry.to_le_bytes());
        b
    }

    #[test]
    fn parses() {
        let h = Header::parse(&header(2, 2, EM_AARCH64, 0xFFFF_FFFF_8000_1000)).unwrap();
        assert_eq!(h, Header { machine: EM_AARCH64, entry: 0xFFFF_FFFF_8000_1000 });
    }

    #[test]
    fn rejects() {
        assert!(Header::parse(b"hello").is_err());
        assert!(Header::parse(&header(1, 2, EM_X86_64, 0)).is_err());
        assert!(Header::parse(&header(2, 3, EM_X86_64, 0)).is_err()); // shared object (PIE)
    }
}
