// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The memory allocator for programs, so `Box`, `Vec` and `String` work.
//!
//! Small requests (up to 2 KiB) come from size classes of 16 bytes to 2 KiB, each a free list
//! of slots carved from 64 KiB memory objects. Larger ones get a memory object of their own,
//! mapped for them and unmapped when freed.

use crate::lock::Lock;
use crate::memory::Memory;
use crate::sys::perm;
use core::alloc::{GlobalAlloc, Layout};

const CLASSES: usize = 8; // 16, 32, ... 2048
const CHUNK: u64 = 64 * 1024;
const BIG_HEADER: usize = 16;

struct Heap {
    free: [usize; CLASSES],
    cursor: usize,
    end: usize,
}

static HEAP: Lock<Heap> = Lock::new(Heap { free: [0; CLASSES], cursor: 0, end: 0 });

struct Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn class(size: usize) -> Option<usize> {
    let size = size.max(16).next_power_of_two();
    let c = size.trailing_zeros() as usize - 4;
    (c < CLASSES).then_some(c)
}

// SAFETY: every block handed out is either a slot of exactly its class's size, aligned to that
// size, used by nothing else until freed; or a fresh mapping of its own.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let want = layout.size().max(layout.align());
        if let Some(c) = class(want) {
            let size = 16usize << c;
            let mut h = HEAP.lock();
            let slot = h.free[c];
            if slot != 0 {
                // SAFETY: a free slot holds the address of the next free slot.
                h.free[c] = unsafe { *(slot as *const usize) };
                return slot as *mut u8;
            }
            let mut at = h.cursor.next_multiple_of(size);
            if h.cursor == 0 || at + size > h.end {
                let Ok(m) = Memory::create(CHUNK) else { return core::ptr::null_mut() };
                let Ok(mapping) = m.map(perm::READ_WRITE) else { return core::ptr::null_mut() };
                let base = mapping.leak() as usize;
                h.cursor = base;
                h.end = base + CHUNK as usize;
                at = base;
            }
            h.cursor = at + size;
            return at as *mut u8;
        }
        // A big block: its own memory object, with a header saying how to free it.
        if layout.align() > 4096 {
            return core::ptr::null_mut();
        }
        let offset = layout.align().max(BIG_HEADER);
        let len = (offset + layout.size()) as u64;
        let Ok(m) = Memory::create(len) else { return core::ptr::null_mut() };
        let Ok(mapping) = m.map(perm::READ_WRITE) else { return core::ptr::null_mut() };
        let (base, maplen) = (mapping.addr(), mapping.len());
        let _ = mapping.leak();
        let handle = m.0.into_raw();
        let p = (base as usize + offset) as *mut u8;
        // SAFETY: the header is the 16 bytes just before `p`, inside the new mapping.
        unsafe {
            (p.sub(16) as *mut u64).write(base);
            (p.sub(8) as *mut u64).write(maplen << 32 >> 32 | u64::from(handle) << 32);
        }
        // A mapping longer than 4 GiB can't be recorded this way.
        if maplen >> 32 != 0 {
            return core::ptr::null_mut();
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let want = layout.size().max(layout.align());
        if let Some(c) = class(want) {
            let mut h = HEAP.lock();
            // SAFETY: the slot is ours again; its first word links it into the free list.
            unsafe { *(ptr as *mut usize) = h.free[c] };
            h.free[c] = ptr as usize;
            return;
        }
        // SAFETY: `alloc` wrote this header.
        let (base, word) =
            unsafe { ((ptr.sub(16) as *const u64).read(), (ptr.sub(8) as *const u64).read()) };
        let (len, handle) = (word & 0xFFFF_FFFF, (word >> 32) as u32);
        let a = [crate::process::self_handle().into(), base, len, 0, 0, 0];
        let _ = crate::sys::call(crate::sys::num::MEMORY_UNMAP, a);
        drop(crate::handle::Handle::from_raw(handle));
    }
}
