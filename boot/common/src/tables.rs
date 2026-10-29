// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The page tables a loader builds for the kernel (note 0003).

use crate::elf::Kernel;
use crate::{PAGE, align_up};
use pbr_bootinfo::MemoryRegion;
use pbr_platform::paging::{
    Attr, Format, FrameAlloc, MapError, Memory, PageTables, map_direct, map_identity,
};

/// The page tables a loader hands to the kernel.
pub struct Standard {
    /// Holds the upper half: the kernel and the direct map. On x86-64 it also holds the
    /// identity map, and is the only tree.
    pub upper: PageTables,
    /// On AArch64, the lower half (`TTBR0_EL1`): the identity map. On x86-64, the same tree as
    /// `upper`.
    pub lower_root: u64,
}

/// Builds the page tables note 0003 describes:
/// - the kernel, at the address it was linked at, with each segment's own permissions;
/// - the direct map of every region in `map`, by kind;
/// - an identity map of RAM, so the loader keeps running while it switches tables.
///
/// `kernel_phys` is where [`Kernel::load_into`] put the kernel's span.
pub fn build_standard(
    fa: &mut impl FrameAlloc,
    format: Format,
    kernel: &Kernel<'_>,
    kernel_phys: u64,
    map: &[MemoryRegion],
) -> Result<Standard, MapError> {
    let mut upper = PageTables::new(fa, format)?;
    let (lo, _) = kernel.span();
    for s in kernel.segments() {
        let attr = Attr { write: s.writable(), exec: s.executable(), mem: Memory::Normal };
        let len = align_up(s.vaddr + s.mem_len, PAGE) - s.vaddr;
        upper.map(fa, s.vaddr, kernel_phys + (s.vaddr - lo), len, attr)?;
    }
    map_direct(&mut upper, fa, map)?;
    let lower_root = match format {
        Format::X86_64 => {
            map_identity(&mut upper, fa, map)?;
            upper.root
        }
        Format::Aarch64 => {
            let mut lower = PageTables::new(fa, format)?;
            map_identity(&mut lower, fa, map)?;
            lower.root
        }
    };
    Ok(Standard { upper, lower_root })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DIRECT_MAP_BASE;
    use crate::elf::{EM_X86_64, PF_R, PF_W, PF_X, tests::make};
    use crate::memmap::MapBuilder;
    use pbr_bootinfo::{MemoryKind, PhysRange};

    struct FakeFrames(Vec<Box<[u64; 512]>>);

    impl FrameAlloc for FakeFrames {
        fn alloc(&mut self) -> Option<u64> {
            self.0.push(Box::new([0; 512]));
            Some(0x1000_0000 + (self.0.len() as u64) * PAGE)
        }
        fn table(&mut self, phys: u64) -> &mut [u64; 512] {
            &mut self.0[((phys - 0x1000_0000) / PAGE - 1) as usize]
        }
    }

    #[test]
    fn standard_layout() {
        let b = crate::KERNEL_BASE;
        let f =
            make(EM_X86_64, b, &[(b, b"code", 4, PF_R | PF_X), (b + 0x1000, b"d", 1, PF_R | PF_W)]);
        let k = Kernel::parse(&f, EM_X86_64).unwrap();
        let mut buf = vec![MemoryRegion { range: PhysRange::NONE, kind: 0, reserved: 0 }; 16];
        let mut m = MapBuilder::new(&mut buf);
        m.set(0, 0x800_0000, MemoryKind::Usable).unwrap();
        m.set(0x40_0000, 0x2000, MemoryKind::Kernel).unwrap();
        m.set(0x8000_0000, 0x30_0000, MemoryKind::Framebuffer).unwrap();
        let map = m.finish();
        for format in [Format::X86_64, Format::Aarch64] {
            let mut fa = FakeFrames(vec![]);
            let st = build_standard(&mut fa, format, &k, 0x40_0000, map).unwrap();
            let code = st.upper.translate(&mut fa, b + 2).unwrap();
            assert_eq!(code.0, 0x40_0002);
            assert!(code.1.exec && !code.1.write);
            let data = st.upper.translate(&mut fa, b + 0x1000).unwrap();
            assert!(!data.1.exec && data.1.write);
            assert!(st.upper.translate(&mut fa, DIRECT_MAP_BASE + 0x40_0000).is_some());
            let lower = PageTables::from_root(st.lower_root, format);
            let id = lower.translate(&mut fa, 0x7F_F000).unwrap();
            assert_eq!(id.0, 0x7F_F000);
            assert!(id.1.exec);
        }
    }
}
