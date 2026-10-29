// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The kernel heap, behind Rust's `GlobalAlloc` (docs/design/0005-kernel-core.md, section 4).
//! Requests up to 2 KiB come from size classes carved out of whole pages; larger ones are
//! contiguous runs of pages, reached through the direct map.

use super::{PAGE, phys_to_virt, pmm};
use crate::sync::{IrqSpinLock, Level};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU64, Ordering};

const CLASSES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];

/// A free slot: the first bytes of a free block point at the next one.
struct FreeList {
    head: *mut u8,
}

// SAFETY: the free lists only hold pointers to heap memory, and are always used under a lock.
unsafe impl Send for FreeList {}

static LISTS: [IrqSpinLock<FreeList>; 8] =
    [const { IrqSpinLock::new(Level::Heap, "heap", FreeList { head: null_mut() }) }; 8];

/// Bytes handed out and not yet returned, for the self-test and reports.
pub static IN_USE: AtomicU64 = AtomicU64::new(0);

fn class_of(layout: &Layout) -> Option<usize> {
    let size = layout.size().max(layout.align());
    CLASSES.iter().position(|&c| size <= c)
}

/// The heap.
pub struct Heap;

// SAFETY: blocks are handed out at most once until returned, are at least as big and as aligned
// as asked (classes are powers of two and blocks are carved at multiples of their size from
// page-aligned pages), and the free lists are guarded by locks.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        IN_USE.fetch_add(layout.size() as u64, Ordering::Relaxed);
        match class_of(&layout) {
            Some(c) => {
                let mut list = LISTS[c].lock();
                if list.head.is_null() {
                    let Some(page) = pmm::alloc_page() else { return null_mut() };
                    let base = phys_to_virt(page) as *mut u8;
                    let size = CLASSES[c];
                    for i in (0..PAGE as usize / size).rev() {
                        // SAFETY: inside the fresh page; each slot holds the next pointer.
                        unsafe {
                            let slot = base.add(i * size);
                            *(slot as *mut *mut u8) = list.head;
                            list.head = slot;
                        }
                    }
                }
                let slot = list.head;
                // SAFETY: `slot` is a free block from this list; its first word is the next one.
                list.head = unsafe { *(slot as *mut *mut u8) };
                slot
            }
            None => {
                let pages = (layout.size() as u64).div_ceil(PAGE);
                let align = (layout.align() as u64).div_ceil(PAGE).max(1);
                match pmm::alloc(pages, align, u64::MAX) {
                    Some(p) => phys_to_virt(p) as *mut u8,
                    None => null_mut(),
                }
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        IN_USE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        match class_of(&layout) {
            Some(c) => {
                let mut list = LISTS[c].lock();
                // SAFETY: `ptr` came from this class (the same layout picks the same class).
                unsafe { *(ptr as *mut *mut u8) = list.head };
                list.head = ptr;
            }
            None => {
                let pages = (layout.size() as u64).div_ceil(PAGE);
                pmm::free(ptr as u64 - phys_to_virt(0), pages);
            }
        }
    }
}
