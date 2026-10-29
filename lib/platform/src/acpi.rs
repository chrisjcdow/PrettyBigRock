// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The little of ACPI a loader needs: checking the root pointer, finding a table, and reading
//! the serial console table (SPCR).

use crate::PhysMemory;
use pbr_bootinfo::EarlyConsole;

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0
}

/// True if a valid ACPI root pointer (RSDP) is at `addr`.
pub fn rsdp_valid(mem: &impl PhysMemory, addr: u64) -> bool {
    let mut b = [0u8; 36];
    if !mem.read(addr, &mut b[..20]) || &b[..8] != b"RSD PTR " || !checksum_ok(&b[..20]) {
        return false;
    }
    if b[15] >= 2 {
        // ACPI 2.0 and later: the extended structure has its own checksum.
        return mem.read(addr, &mut b) && checksum_ok(&b);
    }
    true
}

/// Searches the BIOS areas where a PC's root pointer can be: the first KiB of the extended
/// BIOS data area, and 0xE0000 to 0xFFFFF. Returns its address.
pub fn find_rsdp_bios(mem: &impl PhysMemory) -> Option<u64> {
    let mut seg = [0u8; 2];
    let ebda = if mem.read(0x40E, &mut seg) { u64::from(u16::from_le_bytes(seg)) << 4 } else { 0 };
    let areas = [(ebda, 1024), (0xE0000, 0x20000)];
    for (start, len) in areas {
        if start == 0 {
            continue;
        }
        let mut a = start;
        while a < start + len {
            if rsdp_valid(mem, a) {
                return Some(a);
            }
            a += 16;
        }
    }
    None
}

/// Finds the ACPI table with this signature, through the XSDT (or the RSDT on ACPI 1.0).
pub fn find_table(mem: &impl PhysMemory, rsdp: u64, sig: &[u8; 4]) -> Option<u64> {
    let mut b = [0u8; 36];
    mem.read(rsdp, &mut b[..20]).then_some(())?;
    let (root, entry) = if b[15] >= 2 && mem.read(rsdp, &mut b) {
        (u64::from_le_bytes(b[24..32].try_into().ok()?), 8)
    } else {
        (u64::from(u32::from_le_bytes(b[16..20].try_into().ok()?)), 4)
    };
    let len = u64::from(mem.read_u32(root + 4)?);
    let mut a = root + 36;
    while a + entry <= root + len {
        let t = if entry == 8 { mem.read_u64(a)? } else { u64::from(mem.read_u32(a)?) };
        let mut s = [0u8; 4];
        if mem.read(t, &mut s) && &s == sig {
            return Some(t);
        }
        a += entry;
    }
    None
}

/// Reads the serial port description from the SPCR table, if it's one the kernel can use.
pub fn spcr_console(mem: &impl PhysMemory, spcr: u64) -> Option<EarlyConsole> {
    let mut t = [0u8; 52];
    mem.read(spcr, &mut t).then_some(())?;
    let interface = t[36];
    let space = t[40];
    let width = t[41];
    let addr = u64::from_le_bytes(t[44..52].try_into().ok()?);
    let kind = match (interface, space) {
        (3 | 0x0E | 0x0F, 0) => 3, // Arm PL011 (and SBSA UARTs, which are compatible)
        (0 | 1, 1) => 1,           // 16550 on I/O ports
        (0 | 1 | 0x12, 0) => 2,    // 16550 memory-mapped
        _ => return None,
    };
    let register_width = if width >= 32 { 4 } else { 1 };
    (addr != 0).then_some(EarlyConsole { kind, register_width, address: addr })
}

/// One entry in the MADT (the "APIC" table), which lists the cores and interrupt controllers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MadtEntry {
    /// An x86 core, by local APIC ID.
    LocalApic {
        /// Its APIC ID.
        apic_id: u32,
        /// Usable now (rather than only after hot-plugging).
        enabled: bool,
    },
    /// An x86 core with an APIC ID too big for the older entry.
    X2Apic {
        /// Its APIC ID.
        apic_id: u32,
        /// Usable now.
        enabled: bool,
    },
    /// An x86 I/O APIC.
    IoApic {
        /// Its physical address.
        address: u64,
        /// The first interrupt it handles.
        gsi_base: u32,
    },
    /// A 64-bit address for the local APICs, overriding the header's.
    LapicAddress(u64),
    /// An Arm core and its GIC CPU interface.
    Gicc {
        /// The CPU interface number (the target bit for GICv2 software interrupts).
        interface: u32,
        /// The core's MPIDR affinity bits.
        mpidr: u64,
        /// The GICv2 CPU interface's physical address.
        base: u64,
        /// Usable now.
        enabled: bool,
    },
    /// The Arm GIC distributor.
    Gicd {
        /// Its physical address.
        base: u64,
        /// The GIC version (0 means "find out from the hardware").
        version: u8,
    },
}

/// Calls `f` for each MADT entry PrettyBigRock understands. Returns the local APIC address from
/// the header (x86; meaningless on Arm).
pub fn madt(mem: &impl PhysMemory, table: u64, mut f: impl FnMut(MadtEntry)) -> Option<u64> {
    let len = u64::from(mem.read_u32(table + 4)?);
    let lapic = u64::from(mem.read_u32(table + 36)?);
    let mut a = table + 44;
    while a + 2 <= table + len {
        let kind = mem.read_u8(a)?;
        let elen = u64::from(mem.read_u8(a + 1)?);
        if elen < 2 {
            break;
        }
        let e = match kind {
            0 => Some(MadtEntry::LocalApic {
                apic_id: u32::from(mem.read_u8(a + 3)?),
                enabled: mem.read_u32(a + 4)? & 1 != 0,
            }),
            1 => Some(MadtEntry::IoApic {
                address: u64::from(mem.read_u32(a + 4)?),
                gsi_base: mem.read_u32(a + 8)?,
            }),
            5 => Some(MadtEntry::LapicAddress(mem.read_u64(a + 4)?)),
            9 => Some(MadtEntry::X2Apic {
                apic_id: mem.read_u32(a + 4)?,
                enabled: mem.read_u32(a + 8)? & 1 != 0,
            }),
            0xB if elen >= 76 => Some(MadtEntry::Gicc {
                interface: mem.read_u32(a + 4)?,
                enabled: mem.read_u32(a + 12)? & 1 != 0,
                base: mem.read_u64(a + 32)?,
                mpidr: mem.read_u64(a + 68)?,
            }),
            0xC => {
                Some(MadtEntry::Gicd { base: mem.read_u64(a + 8)?, version: mem.read_u8(a + 20)? })
            }
            _ => None,
        };
        if let Some(e) = e {
            f(e);
        }
        a += elen;
    }
    Some(lapic)
}

/// The HPET's physical address, from the "HPET" table.
pub fn hpet_base(mem: &impl PhysMemory, table: u64) -> Option<u64> {
    (mem.read_u8(table + 40)? == 0).then(|| mem.read_u64(table + 44)).flatten()
}

/// The Arm virtual timer's interrupt number, from the GTDT.
pub fn gtdt_virtual_timer(mem: &impl PhysMemory, table: u64) -> Option<u32> {
    mem.read_u32(table + 64)
}

/// From the FADT on Arm: (PSCI is available, PSCI is called with HVC rather than SMC).
pub fn fadt_psci(mem: &impl PhysMemory, fadt: u64) -> Option<(bool, bool)> {
    let len = mem.read_u32(fadt + 4)?;
    if len < 131 {
        return None;
    }
    let flags = mem.read_u16(fadt + 129)?;
    Some((flags & 1 != 0, flags & 2 != 0))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Sparse fake physical memory.
    #[derive(Default)]
    pub struct FakeMem(pub BTreeMap<u64, u8>);

    impl FakeMem {
        pub fn put(&mut self, addr: u64, bytes: &[u8]) {
            for (i, &b) in bytes.iter().enumerate() {
                self.0.insert(addr + i as u64, b);
            }
        }
    }

    impl PhysMemory for FakeMem {
        fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = *self.0.get(&(addr + i as u64)).unwrap_or(&0);
            }
            true
        }
    }

    fn fix_checksum(b: &mut [u8], at: usize) {
        b[at] = 0;
        let s = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[at] = 0u8.wrapping_sub(s);
    }

    fn table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; 36];
        t[..4].copy_from_slice(sig);
        t.extend_from_slice(body);
        let len = t.len() as u32;
        t[4..8].copy_from_slice(&len.to_le_bytes());
        fix_checksum(&mut t, 9);
        t
    }

    /// Fake ACPI 2.0 tables at 0xE_0000 (the RSDP), with an XSDT listing an SPCR.
    pub fn fake_acpi(mem: &mut FakeMem) -> u64 {
        let mut spcr_body = vec![0u8; 16];
        spcr_body[0] = 3; // PL011
        spcr_body[4] = 0; // memory space
        spcr_body[5] = 32; // 32-bit registers
        spcr_body[8..16].copy_from_slice(&0x0900_0000u64.to_le_bytes());
        mem.put(0x5000, &table(b"SPCR", &spcr_body));
        mem.put(0x4000, &table(b"XSDT", &0x5000u64.to_le_bytes()));
        let mut r = vec![0u8; 36];
        r[..8].copy_from_slice(b"RSD PTR ");
        r[15] = 2;
        r[20..24].copy_from_slice(&36u32.to_le_bytes());
        r[24..32].copy_from_slice(&0x4000u64.to_le_bytes());
        fix_checksum(&mut r[..20], 8);
        fix_checksum(&mut r, 32);
        mem.put(0xE_0000, &r);
        0xE_0000
    }

    #[test]
    fn finds_the_root_pointer_and_spcr() {
        let mut mem = FakeMem::default();
        let rsdp = fake_acpi(&mut mem);
        assert!(rsdp_valid(&mem, rsdp));
        assert_eq!(find_rsdp_bios(&mem), Some(rsdp));
        let spcr = find_table(&mem, rsdp, b"SPCR").unwrap();
        assert_eq!(spcr, 0x5000);
        assert_eq!(find_table(&mem, rsdp, b"APIC"), None);
        assert_eq!(
            spcr_console(&mem, spcr),
            Some(EarlyConsole { kind: 3, register_width: 4, address: 0x0900_0000 })
        );
    }

    #[test]
    fn reads_the_madt() {
        let mut mem = FakeMem::default();
        let mut body = vec![0u8; 8];
        body[..4].copy_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // local APIC 0, enabled
        body.extend_from_slice(&[0, 8, 1, 3, 0, 0, 0, 0]); // local APIC 3, disabled
        let mut gicc = vec![0u8; 80];
        gicc[0] = 0xB;
        gicc[1] = 80;
        gicc[4] = 2;
        gicc[12] = 1;
        gicc[32..40].copy_from_slice(&0x0801_0000u64.to_le_bytes());
        gicc[68..76].copy_from_slice(&0x0000_0102u64.to_le_bytes());
        body.extend_from_slice(&gicc);
        let mut gicd = vec![0u8; 24];
        gicd[0] = 0xC;
        gicd[1] = 24;
        gicd[8..16].copy_from_slice(&0x0800_0000u64.to_le_bytes());
        gicd[20] = 2;
        body.extend_from_slice(&gicd);
        mem.put(0x7000, &table(b"APIC", &body));
        let mut seen = vec![];
        assert_eq!(madt(&mem, 0x7000, |e| seen.push(e)), Some(0xFEE0_0000));
        assert_eq!(
            seen,
            vec![
                MadtEntry::LocalApic { apic_id: 0, enabled: true },
                MadtEntry::LocalApic { apic_id: 3, enabled: false },
                MadtEntry::Gicc { interface: 2, mpidr: 0x102, base: 0x0801_0000, enabled: true },
                MadtEntry::Gicd { base: 0x0800_0000, version: 2 },
            ]
        );
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let mut mem = FakeMem::default();
        let rsdp = fake_acpi(&mut mem);
        mem.put(rsdp + 9, b"X");
        assert!(!rsdp_valid(&mem, rsdp));
        assert_eq!(find_rsdp_bios(&mem), None);
    }
}
