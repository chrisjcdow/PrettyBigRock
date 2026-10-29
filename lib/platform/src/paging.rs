// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Page tables for x86-64 and AArch64, built the way note 0003 requires.
//!
//! Both architectures use four levels of 512-entry tables with 4 KiB pages and 48-bit virtual
//! addresses, so one builder handles both; only the bits inside each entry differ.

use crate::{DIRECT_MAP_BASE, PAGE, align_down, align_up, is_ram};
use pbr_bootinfo::{MemoryKind, MemoryRegion};

const LARGE: u64 = 2 * 1024 * 1024;

/// The type of memory a mapping points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    /// Ordinary RAM: cached.
    Normal,
    /// Device registers: never cached, never reordered.
    Device,
    /// A framebuffer: not cached (Arm), or left to the memory-type registers (x86).
    Framebuffer,
}

/// What a mapping allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    /// May be written.
    pub write: bool,
    /// May be executed.
    pub exec: bool,
    /// The memory type.
    pub mem: Memory,
}

impl Attr {
    /// Read-write data.
    pub const DATA: Attr = Attr { write: true, exec: false, mem: Memory::Normal };
}

/// The page-table entry format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// x86-64 4-level paging.
    X86_64,
    /// AArch64 with a 4 KiB granule and 48-bit addresses.
    Aarch64,
}

/// Supplies zeroed 4 KiB frames for page tables.
pub trait FrameAlloc {
    /// A new zeroed frame's physical address, or `None` when out of memory.
    fn alloc(&mut self) -> Option<u64>;
    /// The 512 entries of the table in frame `phys`.
    fn table(&mut self, phys: u64) -> &mut [u64; 512];
}

/// Why a mapping failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// No more frames for tables.
    OutOfMemory,
    /// An address or length isn't page-aligned.
    Unaligned,
    /// Something is already mapped there.
    AlreadyMapped,
}

const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

// x86-64 entry bits
const X_P: u64 = 1;
const X_RW: u64 = 1 << 1;
const X_PWT: u64 = 1 << 3;
const X_PCD: u64 = 1 << 4;
const X_PS: u64 = 1 << 7;
const X_NX: u64 = 1 << 63;

// AArch64 descriptor bits
const A_VALID: u64 = 1;
const A_TABLE_OR_PAGE: u64 = 1 << 1;
const A_ATTR_SHIFT: u64 = 2;
const A_AP_RO: u64 = 1 << 7;
const A_SH_INNER: u64 = 3 << 8;
const A_AF: u64 = 1 << 10;
const A_PXN: u64 = 1 << 53;
const A_UXN: u64 = 1 << 54;

/// The `MAIR_EL1` value matching the attribute indexes used here (note 0003): 0 normal
/// write-back, 1 device nGnRnE, 2 normal non-cacheable.
pub const AARCH64_MAIR: u64 = 0xFF | (0x44 << 16); // index 1 is 0x00: device nGnRnE

impl Format {
    fn table_entry(self, phys: u64) -> u64 {
        match self {
            Format::X86_64 => phys | X_P | X_RW,
            Format::Aarch64 => phys | A_VALID | A_TABLE_OR_PAGE,
        }
    }

    fn leaf(self, phys: u64, attr: Attr, large: bool) -> u64 {
        match self {
            Format::X86_64 => {
                let mut e = phys | X_P;
                if attr.write {
                    e |= X_RW;
                }
                if !attr.exec {
                    e |= X_NX;
                }
                if large {
                    e |= X_PS;
                }
                if attr.mem == Memory::Device {
                    e |= X_PCD | X_PWT;
                }
                e
            }
            Format::Aarch64 => {
                let (index, sh) = match attr.mem {
                    Memory::Normal => (0, A_SH_INNER),
                    Memory::Device => (1, 0),
                    Memory::Framebuffer => (2, A_SH_INNER),
                };
                let mut e = phys | A_VALID | A_AF | sh | (index << A_ATTR_SHIFT) | A_UXN;
                if !large {
                    e |= A_TABLE_OR_PAGE;
                }
                if !attr.write {
                    e |= A_AP_RO;
                }
                if !attr.exec || attr.mem != Memory::Normal {
                    e |= A_PXN;
                }
                e
            }
        }
    }

    fn present(self, e: u64) -> bool {
        e & 1 != 0
    }

    /// True if entry `e` at `level` (3 = top, 0 = 4 KiB pages) maps memory rather than pointing
    /// at a further table.
    fn is_leaf(self, e: u64, level: u32) -> bool {
        if level == 0 {
            return true;
        }
        match self {
            Format::X86_64 => e & X_PS != 0,
            Format::Aarch64 => e & A_TABLE_OR_PAGE == 0,
        }
    }

    fn decode(self, e: u64) -> Attr {
        match self {
            Format::X86_64 => Attr {
                write: e & X_RW != 0,
                exec: e & X_NX == 0,
                mem: if e & X_PCD != 0 { Memory::Device } else { Memory::Normal },
            },
            Format::Aarch64 => Attr {
                write: e & A_AP_RO == 0,
                exec: e & A_PXN == 0,
                mem: match (e >> A_ATTR_SHIFT) & 7 {
                    0 => Memory::Normal,
                    1 => Memory::Device,
                    _ => Memory::Framebuffer,
                },
            },
        }
    }
}

fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * level)) & 511) as usize
}

/// One tree of page tables.
pub struct PageTables {
    /// The top table's physical address (for `CR3`, `TTBR0_EL1` or `TTBR1_EL1`).
    pub root: u64,
    format: Format,
}

impl PageTables {
    /// An empty tree.
    pub fn new(fa: &mut impl FrameAlloc, format: Format) -> Result<Self, MapError> {
        Ok(Self { root: fa.alloc().ok_or(MapError::OutOfMemory)?, format })
    }

    /// An existing tree.
    pub fn from_root(root: u64, format: Format) -> Self {
        Self { root, format }
    }

    /// The entry format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Removes the 4 KiB mapping at `virt`, if there is one. Returns whether there was. (Tables
    /// emptied by this aren't freed; the caller flushes the TLB.)
    pub fn unmap_page(&mut self, fa: &mut impl FrameAlloc, virt: u64) -> bool {
        let mut table = self.root;
        for level in (1..=3).rev() {
            let e = fa.table(table)[index(virt, level)];
            if !self.format.present(e) || self.format.is_leaf(e, level) {
                return false;
            }
            table = e & ADDR_MASK;
        }
        let slot = &mut fa.table(table)[index(virt, 0)];
        let was = self.format.present(*slot);
        *slot = 0;
        was
    }

    /// Maps `len` bytes at `virt` to `phys`, using 2 MiB pages where both addresses allow.
    pub fn map(
        &mut self,
        fa: &mut impl FrameAlloc,
        mut virt: u64,
        mut phys: u64,
        len: u64,
        attr: Attr,
    ) -> Result<(), MapError> {
        if !virt.is_multiple_of(PAGE) || !phys.is_multiple_of(PAGE) || !len.is_multiple_of(PAGE) {
            return Err(MapError::Unaligned);
        }
        let end = phys + len;
        while phys < end {
            let large =
                virt.is_multiple_of(LARGE) && phys.is_multiple_of(LARGE) && end - phys >= LARGE;
            self.map_one(fa, virt, phys, attr, large)?;
            let step = if large { LARGE } else { PAGE };
            virt = virt.wrapping_add(step);
            phys += step;
        }
        Ok(())
    }

    fn map_one(
        &mut self,
        fa: &mut impl FrameAlloc,
        virt: u64,
        phys: u64,
        attr: Attr,
        large: bool,
    ) -> Result<(), MapError> {
        let stop = if large { 1 } else { 0 };
        let mut table = self.root;
        for level in (stop + 1..=3).rev() {
            let i = index(virt, level);
            let e = fa.table(table)[i];
            table = if !self.format.present(e) {
                let t = fa.alloc().ok_or(MapError::OutOfMemory)?;
                fa.table(table)[i] = self.format.table_entry(t);
                t
            } else if self.format.is_leaf(e, level) {
                return Err(MapError::AlreadyMapped);
            } else {
                e & ADDR_MASK
            };
        }
        let i = index(virt, stop);
        let slot = &mut fa.table(table)[i];
        if self.format.present(*slot) {
            return Err(MapError::AlreadyMapped);
        }
        *slot = self.format.leaf(phys, attr, large);
        Ok(())
    }

    /// Looks up a virtual address: its physical address and what the mapping allows.
    pub fn translate(&self, fa: &mut impl FrameAlloc, virt: u64) -> Option<(u64, Attr)> {
        let mut table = self.root;
        for level in (0..=3).rev() {
            let e = fa.table(table)[index(virt, level)];
            if !self.format.present(e) {
                return None;
            }
            if self.format.is_leaf(e, level) {
                let size = PAGE << (9 * level);
                return Some((
                    (e & ADDR_MASK & !(size - 1)) | (virt & (size - 1)),
                    self.format.decode(e),
                ));
            }
            table = e & ADDR_MASK;
        }
        None
    }
}

/// The memory type the direct map uses for a region, or `None` if it isn't mapped at all.
pub fn direct_map_memory(format: Format, kind: u32) -> Option<Memory> {
    if is_ram(kind) {
        Some(Memory::Normal)
    } else if kind == MemoryKind::Framebuffer as u32 {
        Some(Memory::Framebuffer)
    } else if kind == MemoryKind::DeviceMemory as u32 {
        Some(Memory::Device)
    } else if format == Format::X86_64 && kind == MemoryKind::Reserved as u32 {
        // On PCs, firmware tables (SMBIOS, the ACPI root pointer) often sit in reserved
        // memory, and the memory-type registers keep device memory uncached anyway. On Arm,
        // reserved memory may belong to the secure world and must not be touched at all.
        Some(Memory::Normal)
    } else {
        None
    }
}

/// Maps every region of `map` into the direct map (note 0003), each by its kind.
pub fn map_direct(
    t: &mut PageTables,
    fa: &mut impl FrameAlloc,
    map: &[MemoryRegion],
) -> Result<(), MapError> {
    for r in map {
        let Some(mem) = direct_map_memory(t.format, r.kind) else { continue };
        let base = align_down(r.range.base, PAGE);
        let end = align_up(r.range.base + r.range.len, PAGE);
        let attr = Attr { write: true, exec: false, mem };
        map_skipping_overlaps(t, fa, DIRECT_MAP_BASE + base, base, end - base, attr)?;
    }
    Ok(())
}

/// Identity-maps every RAM region of `map`, executable: what a loader needs to keep running
/// while it switches page tables.
pub fn map_identity(
    t: &mut PageTables,
    fa: &mut impl FrameAlloc,
    map: &[MemoryRegion],
) -> Result<(), MapError> {
    for r in map.iter().filter(|r| is_ram(r.kind)) {
        let base = align_down(r.range.base, PAGE);
        let end = align_up(r.range.base + r.range.len, PAGE);
        let attr = Attr { write: true, exec: true, mem: Memory::Normal };
        map_skipping_overlaps(t, fa, base, base, end - base, attr)?;
    }
    Ok(())
}

/// Maps a range, page by page where it meets something already mapped. Regions that aren't
/// whole pages (allowed for non-usable kinds) can share a page with a neighbour; the first
/// mapping of a page wins.
fn map_skipping_overlaps(
    t: &mut PageTables,
    fa: &mut impl FrameAlloc,
    virt: u64,
    phys: u64,
    len: u64,
    attr: Attr,
) -> Result<(), MapError> {
    match t.map(fa, virt, phys, len, attr) {
        Err(MapError::AlreadyMapped) => {
            let mut o = 0;
            while o < len {
                // A failed large mapping may have left some pages mapped: map the rest singly.
                if t.translate(fa, virt + o).is_none() {
                    match t.map(fa, virt + o, phys + o, PAGE, attr) {
                        Ok(()) | Err(MapError::AlreadyMapped) => {}
                        Err(e) => return Err(e),
                    }
                }
                o += PAGE;
            }
            Ok(())
        }
        other => other,
    }
}

/// A frame allocator over one contiguous block of identity-mapped memory: what the loaders use.
pub struct BumpAlloc {
    next: u64,
    end: u64,
}

impl BumpAlloc {
    /// Hands out frames from `[base, base + len)`.
    ///
    /// # Safety
    /// The block must be identity-mapped, writable, page-aligned, and not used for anything
    /// else for as long as the frames are.
    pub unsafe fn new(base: u64, len: u64) -> Self {
        Self { next: base, end: base + len }
    }

    /// How much has been handed out so far, from the block's start.
    pub fn used_until(&self) -> u64 {
        self.next
    }
}

impl FrameAlloc for BumpAlloc {
    fn alloc(&mut self) -> Option<u64> {
        if self.next + PAGE > self.end {
            return None;
        }
        let f = self.next;
        self.next += PAGE;
        self.table(f).fill(0);
        Some(f)
    }

    fn table(&mut self, phys: u64) -> &mut [u64; 512] {
        // SAFETY: `phys` is a frame this allocator handed out, inside the identity-mapped block
        // that `new`'s caller promised is ours alone.
        unsafe { &mut *(phys as *mut [u64; 512]) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pbr_bootinfo::PhysRange;

    /// Frames in ordinary heap memory, at made-up physical addresses.
    pub struct FakeFrames(pub Vec<Box<[u64; 512]>>);

    impl FrameAlloc for FakeFrames {
        fn alloc(&mut self) -> Option<u64> {
            if self.0.len() >= 10_000 {
                return None;
            }
            self.0.push(Box::new([0; 512]));
            Some(0x1000_0000 + (self.0.len() as u64) * PAGE)
        }
        fn table(&mut self, phys: u64) -> &mut [u64; 512] {
            &mut self.0[((phys - 0x1000_0000) / PAGE - 1) as usize]
        }
    }

    #[test]
    fn maps_and_translates_both_formats() {
        for format in [Format::X86_64, Format::Aarch64] {
            let mut fa = FakeFrames(vec![]);
            let mut t = PageTables::new(&mut fa, format).unwrap();
            let ro = Attr { write: false, exec: true, mem: Memory::Normal };
            t.map(&mut fa, 0xFFFF_FFFF_8000_0000, 0x20_0000, 0x3000, ro).unwrap();
            t.map(&mut fa, 0xFFFF_8000_0000_0000, 0, 4 * LARGE, Attr::DATA).unwrap();
            assert_eq!(
                t.translate(&mut fa, 0xFFFF_FFFF_8000_1234),
                Some((0x20_1234, ro)),
                "{format:?}"
            );
            assert_eq!(t.translate(&mut fa, 0xFFFF_FFFF_8000_3000), None);
            assert_eq!(t.translate(&mut fa, 0xFFFF_8000_0065_4321), Some((0x65_4321, Attr::DATA)));
            // 4 large pages need only 3 tables below the top one.
            let dev = Attr { write: true, exec: false, mem: Memory::Device };
            t.map(&mut fa, 0x900_0000, 0x900_0000, PAGE, dev).unwrap();
            assert_eq!(t.translate(&mut fa, 0x900_0010), Some((0x900_0010, dev)));
            assert_eq!(t.map(&mut fa, 0x900_0000, 0, PAGE, dev), Err(MapError::AlreadyMapped));
            assert_eq!(t.map(&mut fa, 0x123, 0, PAGE, dev), Err(MapError::Unaligned));
        }
    }

    #[test]
    fn large_pages_are_used_when_aligned() {
        let mut fa = FakeFrames(vec![]);
        let mut t = PageTables::new(&mut fa, Format::X86_64).unwrap();
        t.map(&mut fa, 0, 0, 512 * LARGE, Attr::DATA).unwrap(); // 1 GiB
        assert_eq!(fa.0.len(), 3); // top, one PDPT, one PD: no 4 KiB tables at all
        let mut fa = FakeFrames(vec![]);
        let mut t = PageTables::new(&mut fa, Format::X86_64).unwrap();
        t.map(&mut fa, PAGE, PAGE, LARGE, Attr::DATA).unwrap(); // misaligned by a page
        assert_eq!(fa.0.len(), 5); // needs page tables
        assert_eq!(t.translate(&mut fa, LARGE + 5), Some((LARGE + 5, Attr::DATA)));
    }

    #[test]
    fn direct_and_identity_maps() {
        let r = |base: u64, len: u64, k: MemoryKind| MemoryRegion {
            range: PhysRange { base, len },
            kind: k as u32,
            reserved: 0,
        };
        let map = [
            r(0, 0x800_0000, MemoryKind::Usable),
            r(0x8000_0000, 0x30_0000, MemoryKind::Framebuffer),
            r(0xFEC0_0000, 0x1000, MemoryKind::DeviceMemory),
            r(0xFED0_0000, 0x1000, MemoryKind::Reserved),
        ];
        for format in [Format::X86_64, Format::Aarch64] {
            let mut fa = FakeFrames(vec![]);
            let mut t = PageTables::new(&mut fa, format).unwrap();
            map_direct(&mut t, &mut fa, &map).unwrap();
            map_identity(&mut t, &mut fa, &map).unwrap();
            assert_eq!(
                t.translate(&mut fa, DIRECT_MAP_BASE + 0x123_4567),
                Some((0x123_4567, Attr::DATA))
            );
            let fb = t.translate(&mut fa, DIRECT_MAP_BASE + 0x8000_0000).unwrap();
            assert!(fb.1.write && !fb.1.exec);
            let dev = t.translate(&mut fa, DIRECT_MAP_BASE + 0xFEC0_0000).unwrap();
            assert_eq!(dev.1.mem, Memory::Device);
            let reserved = t.translate(&mut fa, DIRECT_MAP_BASE + 0xFED0_0000);
            assert_eq!(reserved.is_some(), format == Format::X86_64);
            let id = t.translate(&mut fa, 0x7F_F000).unwrap();
            assert_eq!(id.0, 0x7F_F000);
            assert!(id.1.exec);
            assert_eq!(t.translate(&mut fa, 0x8000_0000), None); // framebuffer: no identity map
            assert!(t.unmap_page(&mut fa, DIRECT_MAP_BASE + 0xFEC0_0000));
            assert_eq!(t.translate(&mut fa, DIRECT_MAP_BASE + 0xFEC0_0000), None);
        }
    }
}
