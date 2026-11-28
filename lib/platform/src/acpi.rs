// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The little of ACPI a loader needs: checking the root pointer, finding a table, and reading
//! the serial console table (SPCR); and what the root program needs to give the power service
//! (docs/design/0024-the-session-desktop.md, section 7): the FADT's power management control
//! ports, and where the DSDT and SSDTs are.

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

/// Calls `f` with the address of every table with this signature (there can be several
/// SSDTs), in the order the XSDT (or RSDT) lists them. Returns how many there were. Looks at
/// most at 256 entries.
pub fn each_table(
    mem: &impl PhysMemory,
    rsdp: u64,
    sig: &[u8; 4],
    mut f: impl FnMut(u64),
) -> usize {
    let mut count = 0;
    each_listed_table(mem, rsdp, |t, s| {
        if &s == sig {
            f(t);
            count += 1;
        }
    });
    count
}

/// Calls `f` with the address and signature of every table the XSDT (or RSDT) lists, in its
/// order, at most 256 of them.
pub fn each_listed_table(mem: &impl PhysMemory, rsdp: u64, mut f: impl FnMut(u64, [u8; 4])) {
    let mut b = [0u8; 36];
    if !mem.read(rsdp, &mut b[..20]) {
        return;
    }
    let (root, entry) = if b[15] >= 2 && mem.read(rsdp, &mut b) {
        (u64::from_le_bytes(b[24..32].try_into().unwrap_or_default()), 8)
    } else {
        (u64::from(u32::from_le_bytes(b[16..20].try_into().unwrap_or_default())), 4)
    };
    let Some(len) = root.checked_add(4).and_then(|a| mem.read_u32(a)) else { return };
    let end = root.saturating_add(u64::from(len));
    let mut a = root.saturating_add(36);
    for _ in 0..256 {
        if a.saturating_add(entry) > end {
            break;
        }
        let t = if entry == 8 { mem.read_u64(a) } else { mem.read_u32(a).map(u64::from) };
        let mut s = [0u8; 4];
        if let Some(t) = t
            && mem.read(t, &mut s)
        {
            f(t, s);
        }
        a += entry;
    }
}

/// A table's length in bytes, from its header.
pub fn table_length(mem: &impl PhysMemory, table: u64) -> Option<u32> {
    mem.read_u32(table.checked_add(4)?)
}

/// From the FADT on x86: the I/O ports of the PM1a and PM1b control registers (0 for PM1b if
/// there is none), where the sleep type is written to turn the computer off. `None` if there
/// is no PM1a, or it is past the I/O port space.
pub fn fadt_pm1_control(mem: &impl PhysMemory, fadt: u64) -> Option<(u16, u16)> {
    let len = table_length(mem, fadt)?;
    if len < 72 {
        return None;
    }
    let a = mem.read_u32(fadt + 64)?;
    let b = mem.read_u32(fadt + 68)?;
    let port = |p: u32| u16::try_from(p).ok();
    let a = port(a).filter(|&p| p != 0)?;
    Some((a, port(b).unwrap_or(0)))
}

/// From the FADT: the DSDT's physical address (the 64-bit `X_DSDT` if the table has a non-zero
/// one, otherwise the 32-bit `DSDT`).
pub fn fadt_dsdt(mem: &impl PhysMemory, fadt: u64) -> Option<u64> {
    let len = table_length(mem, fadt)?;
    if len >= 148 {
        let x = mem.read_u64(fadt + 140)?;
        if x != 0 {
            return Some(x);
        }
    }
    if len < 44 {
        return None;
    }
    let d = mem.read_u32(fadt + 40)?;
    (d != 0).then_some(u64::from(d))
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
    /// An x86 interrupt source override: the old ISA interrupt `source` is really global
    /// system interrupt `gsi`, with the polarity and trigger in `flags` (bits 0-1 polarity:
    /// 1 high, 3 low; bits 2-3 trigger: 1 edge, 3 level; 0 means "as the bus usually does").
    InterruptOverride {
        /// The ISA interrupt number.
        source: u8,
        /// The global system interrupt it is wired to.
        gsi: u32,
        /// Polarity and trigger.
        flags: u16,
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
    /// An Arm GICv2m frame: where message-signalled interrupts are raised, and which shared
    /// interrupts it can raise.
    GicMsiFrame {
        /// The frame's physical address.
        base: u64,
        /// Bit 0: `spi_count` and `spi_base` are valid (otherwise ask the frame).
        flags: u32,
        /// How many interrupts the frame can raise.
        spi_count: u16,
        /// The first interrupt ID.
        spi_base: u16,
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
            2 if elen >= 10 => Some(MadtEntry::InterruptOverride {
                source: mem.read_u8(a + 3)?,
                gsi: mem.read_u32(a + 4)?,
                flags: u16::from_le_bytes([mem.read_u8(a + 8)?, mem.read_u8(a + 9)?]),
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
            0xD if elen >= 24 => Some(MadtEntry::GicMsiFrame {
                base: mem.read_u64(a + 8)?,
                flags: mem.read_u32(a + 16)?,
                spi_count: u16::from_le_bytes([mem.read_u8(a + 20)?, mem.read_u8(a + 21)?]),
                spi_base: u16::from_le_bytes([mem.read_u8(a + 22)?, mem.read_u8(a + 23)?]),
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

/// From the FADT on x86: the "IA-PC boot architecture flags" (bit 1: an 8042 keyboard
/// controller is present). `None` if the table is too short to have them.
pub fn fadt_iapc_flags(mem: &impl PhysMemory, fadt: u64) -> Option<u16> {
    let len = mem.read_u32(fadt + 4)?;
    if len < 111 {
        return None;
    }
    mem.read_u16(fadt + 109)
}

/// What the ACPI service needs from the FADT for the machine's fixed hardware (x86-64; ARM64's
/// "hardware-reduced" FADT has none of it): where the power-management registers are, the
/// interrupt they raise, and how to switch the firmware into ACPI mode
/// (docs/design/0028-acpi.md, sections 3 to 6). Zero for a register the machine hasn't got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FadtFixed {
    /// The SCI, the interrupt power-management events raise: an ISA line number on a PC.
    pub sci: u16,
    /// The port that calls the firmware's hidden handler (system management mode).
    pub smi_command: u32,
    /// The value that, written to `smi_command`, switches the firmware into ACPI mode.
    pub acpi_enable: u8,
    /// The PM1a event block, an I/O port: a status register, then an enable register, each
    /// half of `pm1_event_length`.
    pub pm1a_event: u16,
    /// The PM1b event block, if there is a second one.
    pub pm1b_event: u16,
    /// The length of each PM1 event block, in bytes.
    pub pm1_event_length: u8,
    /// The PM1a control register (sleep type, sleep enable, `SCI_EN`).
    pub pm1a_control: u16,
    /// The PM1b control register, if there is one.
    pub pm1b_control: u16,
    /// The first general-purpose event block: status registers, then as many enable
    /// registers.
    pub gpe0: u16,
    /// Its length in bytes.
    pub gpe0_length: u8,
    /// The second general-purpose event block.
    pub gpe1: u16,
    /// Its length in bytes.
    pub gpe1_length: u8,
    /// The fixed feature flags (bit 4: the power button is a control-method one; bit 20:
    /// hardware-reduced).
    pub flags: u32,
    /// The FACS's physical address (where the global lock is), or 0.
    pub facs: u64,
}

impl FadtFixed {
    /// True if the power button is a fixed one (its status and enable bits in PM1).
    pub fn fixed_power_button(&self) -> bool {
        self.flags & (1 << 4) == 0 && self.pm1a_event != 0
    }
    /// True for ARM64's kind of machine, without any of this hardware.
    pub fn hardware_reduced(&self) -> bool {
        self.flags & (1 << 20) != 0
    }
}

/// Reads [`FadtFixed`]. The 32-bit port fields are used; where one is zero and the table has
/// the 64-bit form (a "generic address" naming an I/O port), that is used instead.
pub fn fadt_fixed(mem: &impl PhysMemory, fadt: u64) -> Option<FadtFixed> {
    let len = u64::from(table_length(mem, fadt)?);
    if len < 116 {
        return None;
    }
    let u8_at = |o: u64| -> Option<u8> {
        let mut b = [0u8];
        mem.read(fadt + o, &mut b).then_some(b[0])
    };
    let u16_at = |o: u64| -> Option<u16> {
        let mut b = [0u8; 2];
        mem.read(fadt + o, &mut b).then_some(u16::from_le_bytes(b))
    };
    // A port: the 32-bit field at `old`, or the generic address at `new` (12 bytes: space 1
    // is I/O, the address at byte 4).
    let port = |old: u64, new: u64| -> Option<u16> {
        let p = mem.read_u32(fadt + old)?;
        if p != 0 {
            return u16::try_from(p).ok();
        }
        if len >= new + 12 && u8_at(new)? == 1 {
            return u16::try_from(mem.read_u64(fadt + new + 4)?).ok();
        }
        Some(0)
    };
    let mut facs = u64::from(mem.read_u32(fadt + 36)?);
    if len >= 140 {
        let x = mem.read_u64(fadt + 132)?;
        if x != 0 {
            facs = x;
        }
    }
    Some(FadtFixed {
        sci: u16_at(46)?,
        smi_command: mem.read_u32(fadt + 48)?,
        acpi_enable: u8_at(52)?,
        pm1a_event: port(56, 148)?,
        pm1b_event: port(60, 160)?,
        pm1_event_length: u8_at(88)?,
        pm1a_control: port(64, 172)?,
        pm1b_control: port(68, 184)?,
        gpe0: port(80, 220)?,
        gpe0_length: u8_at(92)?,
        gpe1: port(84, 232)?,
        gpe1_length: u8_at(93)?,
        flags: mem.read_u32(fadt + 112)?,
        facs,
    })
}

/// The FADT's reset register (ACPI 2.0 and later): writing `value` to it restarts the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetRegister {
    /// True if `address` is an I/O port; false if it is a physical memory address.
    pub io_port: bool,
    /// Where the one-byte register is.
    pub address: u64,
    /// The byte to write.
    pub value: u8,
}

/// The reset register, from the FADT: `None` if the table is too short to have one, its flags
/// don't say it is supported (bit 10 of the fixed feature flags), or it is of a kind this
/// doesn't know (the specification allows memory, I/O ports and PCI configuration space; PCI
/// configuration space isn't supported here), or isn't one byte wide.
pub fn fadt_reset(mem: &impl PhysMemory, fadt: u64) -> Option<ResetRegister> {
    let len = mem.read_u32(fadt + 4)?;
    if len < 129 {
        return None;
    }
    if mem.read_u32(fadt + 112)? & (1 << 10) == 0 {
        return None;
    }
    // A "generic address": the address space, the register's width and offset in bits, its
    // access size, then the 64-bit address.
    let space = mem.read_u8(fadt + 116)?;
    let (width, offset) = (mem.read_u8(fadt + 117)?, mem.read_u8(fadt + 118)?);
    let address = mem.read_u64(fadt + 120)?;
    if width != 8 || offset != 0 || address == 0 {
        return None;
    }
    let io_port = match space {
        0 => false,
        1 if address <= 0xFFFF => true,
        _ => return None,
    };
    Some(ResetRegister { io_port, address, value: mem.read_u8(fadt + 128)? })
}

/// One entry of the MCFG table: a region of PCI Express configuration space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcamRegion {
    /// Physical address of the region (bus `start_bus`, device 0, function 0).
    pub base: u64,
    /// The PCI segment group.
    pub segment: u16,
    /// The first bus the region covers.
    pub start_bus: u8,
    /// The last bus the region covers.
    pub end_bus: u8,
}

/// Calls `f` for each configuration space region in the MCFG table at `table`. Returns how
/// many there were, or `None` if the table can't be read. Entries that run past the table's
/// length, or whose bus range is backwards, are skipped. Reads at most 32 entries.
pub fn mcfg(mem: &impl PhysMemory, table: u64, mut f: impl FnMut(EcamRegion)) -> Option<usize> {
    let len = u64::from(mem.read_u32(table.checked_add(4)?)?);
    let mut count = 0;
    let mut at = 44u64;
    while count < 32 && at.checked_add(16)? <= len {
        let e = table.checked_add(at)?;
        let base = mem.read_u64(e)?;
        let segment = mem.read_u16(e.checked_add(8)?)?;
        let (start_bus, end_bus) =
            (mem.read_u8(e.checked_add(10)?)?, mem.read_u8(e.checked_add(11)?)?);
        at += 16;
        if start_bus > end_bus || base == 0 {
            continue;
        }
        // The region must fit in the physical address space.
        let size = (u64::from(end_bus) - u64::from(start_bus) + 1) << 20;
        if base.checked_add(size).is_none() {
            continue;
        }
        count += 1;
        f(EcamRegion { base, segment, start_bus, end_bus });
    }
    Some(count)
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

    /// A real machine's FADT, read from `tests/data/acpi/`.
    fn real_fadt(machine: &str) -> FakeMem {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/acpi");
        let mut m = FakeMem::default();
        m.put(0x1000, &std::fs::read(dir.join(machine).join("FACP")).unwrap());
        m
    }

    #[test]
    fn fixed_hardware_from_real_fadts() {
        // Roxy (as Linux's `dmesg` and the FADT's disassembly give it): the SCI on line 9, the
        // SMI command port 0xB2, PM1 at 0x400 and its control at 0x404, GPE0 at 0x420.
        let f = fadt_fixed(&real_fadt("roxy-h61mx"), 0x1000).unwrap();
        assert_eq!((f.sci, f.smi_command, f.pm1a_event, f.pm1a_control), (9, 0xB2, 0x400, 0x404));
        assert_eq!((f.pm1_event_length, f.gpe0, f.gpe0_length), (4, 0x420, 16));
        assert!(f.fixed_power_button() && !f.hardware_reduced() && f.facs != 0);
        // QEMU's q35: already in ACPI mode, its own blocks.
        let q = fadt_fixed(&real_fadt("qemu-q35"), 0x1000).unwrap();
        assert_eq!((q.sci, q.pm1a_event, q.pm1a_control), (9, 0x600, 0x604));
        assert!(q.fixed_power_button());
        // ARM64's virt: hardware-reduced, none of it.
        let v = fadt_fixed(&real_fadt("qemu-virt"), 0x1000).unwrap();
        assert!(v.hardware_reduced() && v.pm1a_event == 0);
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
    fn finds_the_power_parts() {
        let mut mem = FakeMem::default();
        // A FADT (ACPI 2.0 length) with PM1a at 0x604, no PM1b, and the DSDT at 0x7000 (both
        // the 32-bit and 64-bit fields), and two SSDTs.
        let mut fadt = vec![0u8; 276 - 36];
        fadt[40 - 36..44 - 36].copy_from_slice(&0x7000u32.to_le_bytes());
        fadt[64 - 36..68 - 36].copy_from_slice(&0x604u32.to_le_bytes());
        fadt[140 - 36..148 - 36].copy_from_slice(&0x7000u64.to_le_bytes());
        mem.put(0x6000, &table(b"FACP", &fadt));
        mem.put(0x7000, &table(b"DSDT", &[1, 2, 3]));
        mem.put(0x8000, &table(b"SSDT", &[4]));
        mem.put(0x9000, &table(b"SSDT", &[5, 6]));
        let mut list = vec![];
        for t in [0x6000u64, 0x8000, 0x9000] {
            list.extend_from_slice(&t.to_le_bytes());
        }
        mem.put(0x4000, &table(b"XSDT", &list));
        let mut r = vec![0u8; 36];
        r[..8].copy_from_slice(b"RSD PTR ");
        r[15] = 2;
        r[20..24].copy_from_slice(&36u32.to_le_bytes());
        r[24..32].copy_from_slice(&0x4000u64.to_le_bytes());
        fix_checksum(&mut r[..20], 8);
        fix_checksum(&mut r, 32);
        mem.put(0xE_0000, &r);
        let fadt = find_table(&mem, 0xE_0000, b"FACP").unwrap();
        assert_eq!(fadt_pm1_control(&mem, fadt), Some((0x604, 0)));
        assert_eq!(fadt_dsdt(&mem, fadt), Some(0x7000));
        assert_eq!(table_length(&mem, 0x7000), Some(39));
        let mut ssdts = vec![];
        assert_eq!(each_table(&mem, 0xE_0000, b"SSDT", |t| ssdts.push(t)), 2);
        assert_eq!(ssdts, vec![0x8000, 0x9000]);
        assert_eq!(each_table(&mem, 0xE_0000, b"APIC", |_| {}), 0);
        // Too short to have the ports; a port past the I/O space.
        mem.put(0xA000, &table(b"FACP", &[0; 20]));
        assert_eq!(fadt_pm1_control(&mem, 0xA000), None);
        let mut far = vec![0u8; 40];
        far[64 - 36..68 - 36].copy_from_slice(&0x1_0000u32.to_le_bytes());
        mem.put(0xB000, &table(b"FACP", &far));
        assert_eq!(fadt_pm1_control(&mem, 0xB000), None);
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
        // An override: ISA interrupt 0 is really interrupt 2, edge-triggered, active high.
        body.extend_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0b0101, 0]);
        let mut frame = vec![0u8; 24];
        frame[0] = 0xD;
        frame[1] = 24;
        frame[8..16].copy_from_slice(&0x0802_0000u64.to_le_bytes());
        frame[16] = 1;
        frame[20..22].copy_from_slice(&64u16.to_le_bytes());
        frame[22..24].copy_from_slice(&80u16.to_le_bytes());
        body.extend_from_slice(&frame);
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
                MadtEntry::InterruptOverride { source: 0, gsi: 2, flags: 0b0101 },
                MadtEntry::GicMsiFrame { base: 0x0802_0000, flags: 1, spi_count: 64, spi_base: 80 },
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

    #[test]
    fn reads_the_mcfg() {
        let mut mem = FakeMem::default();
        let mut body = vec![0u8; 8]; // reserved
        let mut entry = |base: u64, seg: u16, a: u8, b: u8| {
            body.extend_from_slice(&base.to_le_bytes());
            body.extend_from_slice(&seg.to_le_bytes());
            body.extend_from_slice(&[a, b, 0, 0, 0, 0]);
        };
        entry(0xB000_0000, 0, 0, 255);
        entry(0xC000_0000, 1, 4, 3); // backwards: skipped
        entry(0, 0, 0, 1); // no base: skipped
        entry(u64::MAX - 0xFFFF, 2, 0, 1); // runs off the end of memory: skipped
        entry(0xD000_0000, 3, 8, 9);
        mem.put(0x6000, &table(b"MCFG", &body));
        let mut got = Vec::new();
        assert_eq!(mcfg(&mem, 0x6000, |r| got.push(r)), Some(2));
        assert_eq!(
            got,
            [
                EcamRegion { base: 0xB000_0000, segment: 0, start_bus: 0, end_bus: 255 },
                EcamRegion { base: 0xD000_0000, segment: 3, start_bus: 8, end_bus: 9 },
            ]
        );
        // A table that claims to be longer than it is just yields what it has.
        mem.put(0x6004, &0x10_0000u32.to_le_bytes());
        let mut n = 0;
        assert!(mcfg(&mem, 0x6000, |_| n += 1).is_some());
        assert!(n <= 32);
    }

    #[test]
    fn reads_the_fadt_reset_register() {
        // A FADT body starts 36 bytes into the table, so table offset N is body[N - 36].
        let fadt = |flags: u32, space: u8, width: u8, address: u64, value: u8| {
            let mut body = vec![0u8; 129 - 36];
            body[112 - 36..116 - 36].copy_from_slice(&flags.to_le_bytes());
            body[116 - 36] = space;
            body[117 - 36] = width;
            body[120 - 36..128 - 36].copy_from_slice(&address.to_le_bytes());
            body[128 - 36] = value;
            table(b"FACP", &body)
        };
        let mut mem = FakeMem::default();
        // QEMU's: I/O port 0xCF9, value 0x0F.
        mem.put(0x7000, &fadt(1 << 10, 1, 8, 0xCF9, 0x0F));
        assert_eq!(
            fadt_reset(&mem, 0x7000),
            Some(ResetRegister { io_port: true, address: 0xCF9, value: 0x0F })
        );
        mem.put(0x7000, &fadt(1 << 10, 0, 8, 0xFED0_0000, 1));
        assert_eq!(
            fadt_reset(&mem, 0x7000),
            Some(ResetRegister { io_port: false, address: 0xFED0_0000, value: 1 })
        );
        // Not supported by its flags; PCI configuration space; a 16-bit register; a port
        // number that isn't one; no address; a table too short to have the register.
        for bad in [
            fadt(0, 1, 8, 0xCF9, 6),
            fadt(1 << 10, 2, 8, 0xCF9, 6),
            fadt(1 << 10, 1, 16, 0xCF9, 6),
            fadt(1 << 10, 1, 8, 0x1_0000, 6),
            fadt(1 << 10, 1, 8, 0, 6),
            table(b"FACP", &[0u8; 80]),
        ] {
            mem.put(0x7000, &bad);
            assert_eq!(fadt_reset(&mem, 0x7000), None);
        }
    }

    #[test]
    fn reads_the_fadt_boot_flags() {
        let mut mem = FakeMem::default();
        let mut body = vec![0u8; 80]; // the table is 36 + 80 = 116 bytes
        body[109 - 36..111 - 36].copy_from_slice(&0x0002u16.to_le_bytes());
        mem.put(0x7000, &table(b"FACP", &body));
        assert_eq!(fadt_iapc_flags(&mem, 0x7000), Some(2));
        // Too short to have the field (an ACPI 1.0 table).
        mem.put(0x8000, &table(b"FACP", &[0u8; 40]));
        assert_eq!(fadt_iapc_flags(&mem, 0x8000), None);
    }
}
