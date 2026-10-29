// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The physical page allocator: a bitmap with one bit per 4 KiB page (1 = in use).

use super::{PAGE, phys_ptr};
use crate::sync::{IrqSpinLock, Level};
use pbr_bootinfo::{MemoryKind, MemoryRegion};
use pbr_platform::{align_down, align_up};

struct Bitmap {
    words: &'static mut [u64],
    pages: u64,
    free: u64,
    total: u64,
    hint: u64,
}

impl Bitmap {
    fn used(&self, p: u64) -> bool {
        self.words[(p / 64) as usize] & (1 << (p % 64)) != 0
    }
    fn set(&mut self, p: u64, used: bool) {
        let w = &mut self.words[(p / 64) as usize];
        let bit = 1 << (p % 64);
        if used { *w |= bit } else { *w &= !bit }
    }
}

static PMM: IrqSpinLock<Option<Bitmap>> =
    IrqSpinLock::new(Level::PageAllocator, "page allocator", None);

/// Sets up the allocator with the Usable memory in `map`. Everything else starts as in use.
pub fn init(map: &[MemoryRegion]) {
    let usable = |r: &&MemoryRegion| r.kind == MemoryKind::Usable as u32;
    let top = map
        .iter()
        .filter(|r| {
            r.kind == MemoryKind::Usable as u32 || r.kind == MemoryKind::LoaderReclaimable as u32
        })
        .map(|r| r.range.base + r.range.len)
        .max()
        .unwrap_or(0);
    let pages = top / PAGE;
    let bytes = align_up(pages.div_ceil(64) * 8, PAGE);
    let home = map
        .iter()
        .filter(usable)
        .find(|r| r.range.base >= PAGE && r.range.len >= bytes)
        .expect("no room for the page allocator's bitmap");
    let words_ptr = phys_ptr::<u64>(home.range.base);
    // SAFETY: Usable memory nobody else is using yet, reached through the direct map.
    let words = unsafe { core::slice::from_raw_parts_mut(words_ptr, (bytes / 8) as usize) };
    words.fill(!0);
    let mut b = Bitmap { words, pages, free: 0, total: 0, hint: 1 };
    for r in map.iter().filter(usable) {
        release(&mut b, r.range.base, r.range.len);
    }
    for p in home.range.base / PAGE..(home.range.base + bytes) / PAGE {
        if !b.used(p) {
            b.set(p, true);
            b.free -= 1;
        }
    }
    b.total = b.free;
    *PMM.lock() = Some(b);
}

fn release(b: &mut Bitmap, base: u64, len: u64) {
    let first = align_up(base, PAGE) / PAGE;
    let end = (align_down(base + len, PAGE) / PAGE).min(b.pages);
    for p in first.max(1)..end {
        if b.used(p) {
            b.set(p, false);
            b.free += 1;
        }
    }
}

/// Gives memory to the allocator that it didn't start with, such as the loader's memory once
/// the kernel has finished with it.
pub fn add(base: u64, len: u64) {
    let mut g = PMM.lock();
    let b = g.as_mut().expect("page allocator not set up");
    let before = b.free;
    release(b, base, len);
    b.total += b.free - before;
}

/// One page, zeroed. Returns its physical address.
pub fn alloc_page() -> Option<u64> {
    alloc(1, 1, u64::MAX)
}

/// `count` contiguous pages, zeroed, starting on a multiple of `align` pages and ending below
/// physical address `below`. Returns the first page's physical address.
pub fn alloc(count: u64, align: u64, below: u64) -> Option<u64> {
    let phys = {
        let mut g = PMM.lock();
        let b = g.as_mut()?;
        let limit = (below / PAGE).min(b.pages);
        let found =
            search(b, b.hint, limit, count, align).or_else(|| search(b, 1, limit, count, align))?;
        for p in found..found + count {
            b.set(p, true);
        }
        b.free -= count;
        if count == 1 {
            b.hint = found + 1;
        }
        found * PAGE
    };
    // SAFETY: just allocated, so ours alone, and reached through the direct map.
    unsafe { core::ptr::write_bytes(phys_ptr::<u8>(phys), 0, (count * PAGE) as usize) };
    Some(phys)
}

fn search(b: &Bitmap, from: u64, limit: u64, count: u64, align: u64) -> Option<u64> {
    let mut p = align_up(from.max(1), align.max(1));
    while p + count <= limit {
        // Skip whole words that are full, for speed.
        if p.is_multiple_of(64) && b.words[(p / 64) as usize] == !0 {
            p = align_up(p + 64, align.max(1));
            continue;
        }
        match (p..p + count).find(|&q| b.used(q)) {
            None => return Some(p),
            Some(q) => p = align_up(q + 1, align.max(1)),
        }
    }
    None
}

/// Returns `count` pages starting at `phys`.
pub fn free(phys: u64, count: u64) {
    let mut g = PMM.lock();
    let b = g.as_mut().expect("page allocator not set up");
    for p in phys / PAGE..phys / PAGE + count {
        assert!(b.used(p), "freeing a page that isn't in use: {:#x}", p * PAGE);
        b.set(p, false);
    }
    b.free += count;
    b.hint = b.hint.min(phys / PAGE);
}

/// (free pages, total pages).
pub fn stats() -> (u64, u64) {
    PMM.lock().as_ref().map(|b| (b.free, b.total)).unwrap_or((0, 0))
}
