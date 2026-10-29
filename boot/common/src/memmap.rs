// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Building the memory map that goes into the boot information (note 0003): sorted, without
//! overlaps, neighbours merged, and Usable memory rounded inwards to whole pages.

use crate::{PAGE, align_down, align_up};
use pbr_bootinfo::{MemoryKind, MemoryRegion, PhysRange};

/// The buffer given to [`MapBuilder`] is full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapFull;

/// Builds a memory map in a caller-supplied buffer, so it works before any allocator exists.
pub struct MapBuilder<'a> {
    buf: &'a mut [MemoryRegion],
    len: usize,
}

fn region(base: u64, end: u64, kind: u32) -> MemoryRegion {
    MemoryRegion { range: PhysRange { base, len: end - base }, kind, reserved: 0 }
}

impl<'a> MapBuilder<'a> {
    /// Starts an empty map that can hold `buf.len()` regions.
    pub fn new(buf: &'a mut [MemoryRegion]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, r: MemoryRegion) -> Result<(), MapFull> {
        let slot = self.buf.get_mut(self.len).ok_or(MapFull)?;
        *slot = r;
        self.len += 1;
        Ok(())
    }

    /// Marks `[base, base + len)` as `kind`, replacing whatever was there. Later calls win, so
    /// add the firmware's map first, then the loader's own regions (kernel, framebuffers…).
    pub fn set(&mut self, base: u64, len: u64, kind: MemoryKind) -> Result<(), MapFull> {
        if len == 0 {
            return Ok(());
        }
        let end = base.saturating_add(len);
        let mut i = 0;
        while i < self.len {
            let r = self.buf[i];
            let (rb, re) = (r.range.base, r.range.base + r.range.len);
            if re <= base || rb >= end {
                i += 1;
                continue;
            }
            // `r` overlaps the new range: keep only the parts of it outside.
            self.len -= 1;
            self.buf[i] = self.buf[self.len];
            if rb < base {
                self.push(region(rb, base, r.kind))?;
            }
            if re > end {
                self.push(region(end, re, r.kind))?;
            }
            // Don't advance: slot `i` now holds a region not yet examined. (Pieces pushed above
            // don't overlap the new range, so seeing them again is harmless.)
        }
        self.push(region(base, end, kind as u32))
    }

    /// Finishes the map: rounds Usable regions inwards to pages, sorts, and merges neighbours
    /// of the same kind.
    pub fn finish(self) -> &'a [MemoryRegion] {
        let Self { buf, mut len } = self;
        let mut i = 0;
        while i < len {
            let r = &mut buf[i];
            if r.kind == MemoryKind::Usable as u32 {
                let b = align_up(r.range.base, PAGE);
                let e = align_down(r.range.base + r.range.len, PAGE);
                if e <= b {
                    len -= 1;
                    buf[i] = buf[len];
                    continue;
                }
                *r = region(b, e, r.kind);
            }
            i += 1;
        }
        buf[..len].sort_unstable_by_key(|r| r.range.base);
        let mut out = 0;
        for i in 0..len {
            let r = buf[i];
            if out > 0 {
                let p = &mut buf[out - 1];
                if p.kind == r.kind && p.range.base + p.range.len == r.range.base {
                    p.range.len += r.range.len;
                    continue;
                }
            }
            buf[out] = r;
            out += 1;
        }
        &buf[..out]
    }
}

/// Converts a UEFI memory type (as it is after `ExitBootServices`).
pub fn from_uefi(t: u32) -> MemoryKind {
    match t {
        3..=4 | 7 => MemoryKind::Usable, // boot services code/data, conventional memory
        1..=2 => MemoryKind::LoaderReclaimable, // loader code/data
        5..=6 => MemoryKind::FirmwareRuntime, // runtime services code/data
        8 => MemoryKind::Bad,
        9 => MemoryKind::AcpiReclaimable,
        10 => MemoryKind::AcpiNvs,
        11..=12 => MemoryKind::DeviceMemory, // memory-mapped I/O and I/O port space
        _ => MemoryKind::Reserved,           // reserved, PAL code, persistent, unaccepted…
    }
}

/// Converts a BIOS `E820` memory type.
pub fn from_e820(t: u32) -> MemoryKind {
    match t {
        1 => MemoryKind::Usable,
        3 => MemoryKind::AcpiReclaimable,
        4 => MemoryKind::AcpiNvs,
        5 => MemoryKind::Bad,
        _ => MemoryKind::Reserved,
    }
}

/// True for memory the kernel's direct map should treat as ordinary RAM.
pub fn is_ram(kind: u32) -> bool {
    use MemoryKind::*;
    matches!(
        MemoryRegion { range: PhysRange::NONE, kind, reserved: 0 }.kind(),
        Some(
            Usable
                | AcpiReclaimable
                | AcpiNvs
                | LoaderReclaimable
                | Kernel
                | Module
                | FirmwareRuntime
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use MemoryKind::*;

    fn kinds(m: &[MemoryRegion]) -> Vec<(u64, u64, u32)> {
        m.iter().map(|r| (r.range.base, r.range.len, r.kind)).collect()
    }

    fn blank() -> Vec<MemoryRegion> {
        vec![MemoryRegion { range: PhysRange::NONE, kind: 0, reserved: 0 }; 16]
    }

    #[test]
    fn carves_sorts_and_merges() {
        let mut buf = blank();
        let mut m = MapBuilder::new(&mut buf);
        m.set(0x10_0000, 0x70_0000, Usable).unwrap(); // 1 MiB .. 8 MiB
        m.set(0, 0x9_F000, Usable).unwrap();
        m.set(0x20_0000, 0x10_0000, Kernel).unwrap(); // in the middle: splits it
        m.set(0x7F_0000, 0x1_0000, LoaderReclaimable).unwrap(); // at the end
        m.set(0x30_0000, 0x10_0000, Usable).unwrap(); // touching usable: merges
        let out = m.finish();
        assert_eq!(
            kinds(out),
            vec![
                (0, 0x9_F000, Usable as u32),
                (0x10_0000, 0x10_0000, Usable as u32),
                (0x20_0000, 0x10_0000, Kernel as u32),
                (0x30_0000, 0x4F_0000, Usable as u32),
                (0x7F_0000, 0x1_0000, LoaderReclaimable as u32),
            ]
        );
    }

    #[test]
    fn rounds_usable_inwards_only() {
        let mut buf = blank();
        let mut m = MapBuilder::new(&mut buf);
        m.set(0x1234, 0x3000, Usable).unwrap();
        m.set(0x8100, 0x100, Usable).unwrap(); // less than a page: dropped
        m.set(0x9123, 0x10, AcpiNvs).unwrap(); // not usable: kept exactly
        let out = m.finish();
        assert_eq!(
            kinds(out),
            vec![(0x2000, 0x2000, Usable as u32), (0x9123, 0x10, AcpiNvs as u32)]
        );
    }

    #[test]
    fn replacing_a_whole_region_and_covering_several() {
        let mut buf = blank();
        let mut m = MapBuilder::new(&mut buf);
        m.set(0x0000, 0x1000, Usable).unwrap();
        m.set(0x1000, 0x1000, Reserved).unwrap();
        m.set(0x2000, 0x1000, Usable).unwrap();
        m.set(0x0800, 0x2000, Framebuffer).unwrap();
        let out = m.finish();
        // Usable [0, 0x800) rounds away; Framebuffer [0x800, 0x2800); Usable [0x2800, 0x3000)
        // rounds to nothing as well.
        assert_eq!(kinds(out), vec![(0x800, 0x2000, Framebuffer as u32)]);
    }

    #[test]
    fn reports_a_full_buffer() {
        let mut buf = vec![MemoryRegion { range: PhysRange::NONE, kind: 0, reserved: 0 }; 2];
        let mut m = MapBuilder::new(&mut buf);
        m.set(0, 0x10000, Usable).unwrap();
        assert_eq!(m.set(0x4000, 0x1000, Kernel), Err(MapFull));
    }

    #[test]
    fn conversions() {
        assert_eq!(from_uefi(7), Usable);
        assert_eq!(from_uefi(2), LoaderReclaimable);
        assert_eq!(from_uefi(6), FirmwareRuntime);
        assert_eq!(from_uefi(11), DeviceMemory);
        assert_eq!(from_uefi(99), Reserved);
        assert_eq!(from_e820(1), Usable);
        assert_eq!(from_e820(2), Reserved);
        assert!(is_ram(Kernel as u32));
        assert!(!is_ram(Framebuffer as u32));
        assert!(!is_ram(12345));
    }
}
