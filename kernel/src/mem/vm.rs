// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The kernel's own page tables, and its virtual areas: kernel stacks (each with an unmapped
//! guard page below it) and device registers (docs/design/0005-kernel-core.md, section 2).

use super::{PAGE, phys_ptr, pmm};
use crate::arch;
use crate::sync::{IrqSpinLock, Level};
use alloc::vec::Vec;
use pbr_bootinfo::MemoryRegion;
use pbr_platform::align_up;
use pbr_platform::paging::{Attr, FrameAlloc, Memory, PageTables, map_direct};

/// Where kernel stacks go.
pub const STACK_AREA: u64 = 0xFFFF_C000_0000_0000;
/// Where device registers are mapped.
pub const DEVICE_AREA: u64 = 0xFFFF_C800_0000_0000;
const AREA_END: u64 = 0xFFFF_D000_0000_0000;

/// Page-table frames from the page allocator, reached through the direct map.
pub struct Frames {
    /// Only hand out frames below this physical address.
    pub below: u64,
}

impl FrameAlloc for Frames {
    fn alloc(&mut self) -> Option<u64> {
        pmm::alloc(1, 1, self.below)
    }
    fn table(&mut self, phys: u64) -> &mut [u64; 512] {
        // SAFETY: page-table frames are pages this allocator handed out, used for nothing else.
        unsafe { &mut *phys_ptr::<[u64; 512]>(phys) }
    }
}

struct Kernel {
    tables: PageTables,
    next_stack: u64,
    next_device: u64,
    free_stacks: Vec<(u64, u64)>, // (top, pages)
}

static KERNEL: IrqSpinLock<Option<Kernel>> =
    IrqSpinLock::new(Level::KernelMemory, "kernel page tables", None);

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

/// Builds the kernel's page tables (the kernel image with its own permissions, plus the direct
/// map) and switches to them. `kernel_phys` and `kernel_virt` are where the kernel image is.
pub fn init(map: &[MemoryRegion], kernel_phys: u64, kernel_virt: u64) {
    // On x86-64 the start-up trampoline loads the root with a 32-bit instruction, so keep it
    // below 4 GiB.
    let mut fa = Frames { below: 1 << 32 };
    let mut t = PageTables::new(&mut fa, arch::PAGE_FORMAT).expect("out of memory for page tables");
    fa.below = u64::MAX;
    let sym = |s: *const u8| s as u64;
    let parts = {
        [
            (
                sym(&raw const __text_start),
                sym(&raw const __text_end),
                Attr { write: false, exec: true, mem: Memory::Normal },
            ),
            (
                sym(&raw const __rodata_start),
                sym(&raw const __rodata_end),
                Attr { write: false, exec: false, mem: Memory::Normal },
            ),
            (sym(&raw const __data_start), sym(&raw const __kernel_end), Attr::DATA),
        ]
    };
    for (start, end, attr) in parts {
        let phys = kernel_phys + (start - kernel_virt);
        t.map(&mut fa, start, phys, align_up(end, PAGE) - start, attr)
            .expect("can't map the kernel");
    }
    map_direct(&mut t, &mut fa, map).expect("can't build the direct map");
    arch::activate_kernel_tables(t.root);
    *KERNEL.lock() = Some(Kernel {
        tables: t,
        next_stack: STACK_AREA,
        next_device: DEVICE_AREA,
        free_stacks: Vec::new(),
    });
}

/// The kernel page tables' root, for the other cores.
pub fn root() -> u64 {
    KERNEL.lock().as_ref().expect("kernel page tables not set up").tables.root
}

/// Maps `len` bytes of device registers at physical address `phys`, uncached. Returns the
/// virtual address of `phys`.
pub fn map_device(phys: u64, len: u64) -> u64 {
    let base = phys & !(PAGE - 1);
    let size = align_up(phys + len, PAGE) - base;
    let mut g = KERNEL.lock();
    let k = g.as_mut().expect("kernel page tables not set up");
    let virt = k.next_device;
    assert!(virt + size <= AREA_END, "out of space for device mappings");
    k.next_device += size + PAGE; // leave a gap, so overruns fault
    let attr = Attr { write: true, exec: false, mem: Memory::Device };
    k.tables
        .map(&mut Frames { below: u64::MAX }, virt, base, size, attr)
        .expect("can't map a device");
    virt + (phys - base)
}

/// A kernel stack: `pages` pages, with an unmapped guard page below. Returns the address just
/// above its top.
pub fn alloc_stack(pages: u64) -> Option<u64> {
    let mut g = KERNEL.lock();
    let k = g.as_mut()?;
    if let Some(i) = k.free_stacks.iter().position(|&(_, p)| p == pages) {
        return Some(k.free_stacks.swap_remove(i).0);
    }
    let bottom = k.next_stack + PAGE; // the guard page stays unmapped
    if bottom + pages * PAGE > DEVICE_AREA {
        return None;
    }
    k.next_stack = bottom + pages * PAGE;
    let mut fa = Frames { below: u64::MAX };
    for i in 0..pages {
        let frame = pmm::alloc_page()?;
        k.tables.map(&mut fa, bottom + i * PAGE, frame, PAGE, Attr::DATA).ok()?;
    }
    Some(bottom + pages * PAGE)
}

/// Returns a stack from [`alloc_stack`] for reuse.
pub fn free_stack(top: u64, pages: u64) {
    if let Some(k) = KERNEL.lock().as_mut() {
        k.free_stacks.push((top, pages));
    }
}

#[cfg(target_arch = "x86_64")]
/// Temporarily identity-maps the physical pages `[phys, phys + len)` as executable, for code
/// that runs while a core switches its MMU on (x86-64 start-up trampoline).
pub fn identity_map(phys: u64, len: u64) {
    let mut g = KERNEL.lock();
    let k = g.as_mut().expect("kernel page tables not set up");
    let attr = Attr { write: true, exec: true, mem: Memory::Normal };
    k.tables
        .map(&mut Frames { below: u64::MAX }, phys, phys, align_up(len, PAGE), attr)
        .expect("can't identity-map");
}

#[cfg(target_arch = "x86_64")]
/// Undoes [`identity_map`].
pub fn identity_unmap(phys: u64, len: u64) {
    let mut g = KERNEL.lock();
    let k = g.as_mut().expect("kernel page tables not set up");
    let mut p = phys;
    while p < phys + len {
        k.tables.unmap_page(&mut Frames { below: u64::MAX }, p);
        arch::tlb_flush_page(p);
        p += PAGE;
    }
}

#[cfg(target_arch = "aarch64")]
/// A separate tree that identity-maps `[phys, phys + len)` as executable: what an ARM core
/// needs in its lower half while it switches its MMU on. Returns the tree's root.
pub fn identity_tree(phys: u64, len: u64) -> u64 {
    let mut fa = Frames { below: u64::MAX };
    let mut t = PageTables::new(&mut fa, arch::PAGE_FORMAT).expect("out of memory for page tables");
    let base = phys & !(PAGE - 1);
    let attr = Attr { write: true, exec: true, mem: Memory::Normal };
    t.map(&mut fa, base, base, align_up(phys + len, PAGE) - base, attr)
        .expect("can't identity-map");
    t.root
}
