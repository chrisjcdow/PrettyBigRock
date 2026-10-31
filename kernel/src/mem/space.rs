// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Programs' address spaces (docs/design/0006-objects-and-capabilities.md, section 6).
//!
//! A space is a tree of page tables for the lower half, plus a list of mappings: each one part
//! of a memory object, mapped at an address with some permissions. The kernel never touches a
//! program's memory through the program's own addresses. It finds the mapping, then the
//! memory object's page, and copies through the direct map ([`Space::copy_in`],
//! [`Space::copy_out`]); so a bad address is an error for the call, never a fault in the
//! kernel, and the processor's protections against the kernel touching program memory
//! (SMEP and SMAP, PAN) never need switching off.

use super::object::MemoryObject;
use super::{PAGE, phys_ptr, pmm};
use crate::obj::budget::Charge;
use crate::obj::{Error, charge};
use crate::{arch, percpu};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use pbr_captree::NodeId;
use pbr_platform::paging::{Attr, FrameAlloc, Memory, PageTables};

/// The lowest address a program may use: the first 64 KiB are never mapped.
pub const USER_START: u64 = 0x1_0000;
/// One past the highest address a program may use.
pub const USER_END: u64 = 0x0000_7FFF_FFFF_F000;

/// Permission bits for mappings (note 0007, section 6).
pub mod perm {
    /// Readable.
    pub const READ: u32 = 1;
    /// Writable.
    pub const WRITE: u32 = 2;
    /// Executable.
    pub const EXECUTE: u32 = 4;
}

/// Part of a memory object, mapped into a space.
pub struct Mapping {
    /// The first address.
    pub start: u64,
    /// Its length in bytes (whole pages).
    pub len: u64,
    /// The memory object.
    pub mem: Arc<MemoryObject>,
    /// Where in the memory object it starts (whole pages).
    pub offset: u64,
    /// What it allows now (`perm` bits).
    pub perms: u32,
    /// The most it may ever allow, from the rights of the capability used to map it.
    pub max: u32,
    /// The hidden node in the derivation tree that revocation destroys (none for the system
    /// pages, which no capability grants).
    pub node: Option<NodeId>,
}

/// Page-table pages for a space, charged to its process's budget.
struct Frames<'a>(&'a mut Charge);

impl FrameAlloc for Frames<'_> {
    fn alloc(&mut self) -> Option<u64> {
        self.0.grow(charge::PAGE_TABLE).ok()?;
        match pmm::alloc_page() {
            Some(p) => Some(p),
            None => {
                self.0.shrink(charge::PAGE_TABLE);
                None
            }
        }
    }
    fn table(&mut self, phys: u64) -> &mut [u64; 512] {
        // SAFETY: page-table pages are pages this allocator handed out, used for nothing else.
        unsafe { &mut *phys_ptr::<[u64; 512]>(phys) }
    }
    fn free(&mut self, phys: u64) {
        pmm::free(phys, 1);
        self.0.shrink(charge::PAGE_TABLE);
    }
}

/// A program's address space.
pub struct Space {
    tables: PageTables,
    maps: BTreeMap<u64, Mapping>,
    charge: Charge,
    dead: bool,
}

fn attr(perms: u32) -> Attr {
    Attr {
        write: perms & perm::WRITE != 0,
        exec: perms & perm::EXECUTE != 0,
        mem: Memory::Normal,
        user: true,
    }
}

impl Space {
    /// An empty space, its page tables charged to `charge`'s budget.
    pub fn new(mut charge: Charge) -> Result<Space, Error> {
        let mut fa = Frames(&mut charge);
        let mut tables =
            PageTables::new(&mut fa, arch::PAGE_FORMAT).map_err(|_| Error::OutOfMemory)?;
        if arch::SHARES_TOP_LEVEL {
            // x86-64 has one tree for both halves: share the kernel's upper half.
            tables.copy_top_entries(&mut fa, super::vm::root(), 256..512);
        }
        Ok(Space { tables, maps: BTreeMap::new(), charge, dead: false })
    }

    /// The page tables' root, for loading on a core.
    pub fn root(&self) -> u64 {
        self.tables.root
    }

    /// The mapping containing `addr`, if any.
    pub fn find(&self, addr: u64) -> Option<&Mapping> {
        self.maps.range(..=addr).next_back().map(|(_, m)| m).filter(|m| addr < m.start + m.len)
    }

    fn free(&self, start: u64, len: u64) -> bool {
        let end = start + len;
        if self.maps.range(start..end).next().is_some() {
            return false;
        }
        self.maps.range(..start).next_back().is_none_or(|(_, m)| m.start + m.len <= start)
    }

    /// Picks a free, random place for `len` bytes, with at least one unmapped page on each
    /// side.
    pub fn choose(&self, len: u64) -> Result<u64, Error> {
        const LOW: u64 = 0x1000_0000; // the first 256 MiB are left for programs to choose
        let span = USER_END - LOW - len - 2 * PAGE;
        for _ in 0..64 {
            let at = LOW + PAGE + (crate::rng::next() % (span / PAGE)) * PAGE;
            if self.free(at - PAGE, len + 2 * PAGE) {
                return Ok(at);
            }
        }
        Err(Error::OutOfMemory)
    }

    /// Maps `m` at `m.start`. The range must be free and inside the program half.
    pub fn map(&mut self, m: Mapping) -> Result<(), Error> {
        if self.dead {
            return Err(Error::BadState);
        }
        let end = m.start.checked_add(m.len).ok_or(Error::InvalidArgument)?;
        if m.start < USER_START || end > USER_END || m.len == 0 {
            return Err(Error::InvalidArgument);
        }
        if !self.free(m.start, m.len) {
            return Err(Error::AlreadyExists);
        }
        let a = attr(m.perms);
        let pages = m.len / PAGE;
        for i in 0..pages {
            let phys = m.mem.page(m.offset / PAGE + i).ok_or(Error::InvalidArgument)?;
            let r =
                self.tables.map(&mut Frames(&mut self.charge), m.start + i * PAGE, phys, PAGE, a);
            if r.is_err() {
                // Undo what was mapped, so failure changes nothing.
                for j in 0..i {
                    self.tables.unmap(&mut Frames(&mut self.charge), m.start + j * PAGE);
                }
                arch::flush_user_pages(self.root(), m.start, i);
                return Err(Error::OutOfMemory);
            }
        }
        self.maps.insert(m.start, m);
        Ok(())
    }

    /// The mappings exactly covering `[start, start + len)`: the range must start at a mapping
    /// and end at the end of one, with no gaps.
    fn whole(&self, start: u64, len: u64) -> Result<Vec<u64>, Error> {
        let end = start.checked_add(len).ok_or(Error::InvalidArgument)?;
        let mut at = start;
        let mut out = Vec::new();
        for (&s, m) in self.maps.range(start..end) {
            if s != at {
                return Err(Error::InvalidArgument);
            }
            at = s + m.len;
            out.push(s);
        }
        if at != end || out.is_empty() {
            return Err(Error::InvalidArgument);
        }
        Ok(out)
    }

    /// Removes the mappings exactly covering `[start, start + len)`, and returns them (their
    /// tree nodes are for the caller to remove).
    pub fn unmap(&mut self, start: u64, len: u64) -> Result<Vec<Mapping>, Error> {
        let starts = self.whole(start, len)?;
        Ok(starts.into_iter().filter_map(|s| self.remove(s)).collect())
    }

    /// Removes the mapping starting at `start`, if there is one.
    pub fn remove(&mut self, start: u64) -> Option<Mapping> {
        let m = self.maps.remove(&start)?;
        let pages = m.len / PAGE;
        for i in 0..pages {
            self.tables.unmap(&mut Frames(&mut self.charge), m.start + i * PAGE);
        }
        arch::flush_user_pages(self.root(), m.start, pages);
        Some(m)
    }

    /// Changes the permissions of the mappings exactly covering `[start, start + len)`.
    pub fn protect(&mut self, start: u64, len: u64, perms: u32) -> Result<(), Error> {
        let starts = self.whole(start, len)?;
        if starts.iter().any(|s| perms & !self.maps[s].max != 0) {
            return Err(Error::NoRight);
        }
        for s in starts {
            let m = self.maps.get_mut(&s).expect("listed just now");
            m.perms = perms;
            let (ms, pages) = (m.start, m.len / PAGE);
            for i in 0..pages {
                self.tables.protect(&mut Frames(&mut self.charge), ms + i * PAGE, attr(perms));
            }
            arch::flush_user_pages(self.tables.root, ms, pages);
        }
        Ok(())
    }

    /// The physical address of the byte at `addr`, if it's mapped with `need` permissions.
    fn phys(&self, addr: u64, need: u32) -> Result<u64, Error> {
        let m = self.find(addr).ok_or(Error::BadAddress)?;
        if m.perms & need != need {
            return Err(Error::BadAddress);
        }
        let off = addr - m.start + m.offset;
        let page = m.mem.page(off / PAGE).ok_or(Error::BadAddress)?;
        Ok(page + off % PAGE)
    }

    /// Checks `[addr, addr + len)` is all mapped with `need` permissions.
    pub fn check(&self, addr: u64, len: u64, need: u32) -> Result<(), Error> {
        if len == 0 {
            return Ok(());
        }
        let end = addr.checked_add(len).ok_or(Error::BadAddress)?;
        if end > USER_END {
            return Err(Error::BadAddress);
        }
        let mut at = addr;
        while at < end {
            let m = self.find(at).ok_or(Error::BadAddress)?;
            if m.perms & need != need {
                return Err(Error::BadAddress);
            }
            at = m.start + m.len;
        }
        Ok(())
    }

    /// Copies the program's memory at `addr` into `buf`. It must be readable.
    pub fn copy_in(&self, addr: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.check(addr, buf.len() as u64, perm::READ)?;
        let mut done = 0;
        while done < buf.len() {
            let a = addr + done as u64;
            let n = ((PAGE - a % PAGE) as usize).min(buf.len() - done);
            let p = self.phys(a, perm::READ)?;
            // SAFETY: `p` is a page of a memory object mapped here, reached through the direct
            // map, and `n` bytes stay inside that page.
            unsafe {
                core::ptr::copy_nonoverlapping(phys_ptr::<u8>(p), buf[done..].as_mut_ptr(), n)
            };
            done += n;
        }
        Ok(())
    }

    /// Copies `data` into the program's memory at `addr`. It must be writable.
    pub fn copy_out(&self, addr: u64, data: &[u8]) -> Result<(), Error> {
        self.check(addr, data.len() as u64, perm::WRITE)?;
        let mut done = 0;
        while done < data.len() {
            let a = addr + done as u64;
            let n = ((PAGE - a % PAGE) as usize).min(data.len() - done);
            let p = self.phys(a, perm::WRITE)?;
            // SAFETY: as for `copy_in`.
            unsafe { core::ptr::copy_nonoverlapping(data[done..].as_ptr(), phys_ptr::<u8>(p), n) };
            done += n;
        }
        Ok(())
    }

    /// Copies `len` bytes from `from` at `src` to `to` at `dst`, page by page through the
    /// direct map. Both ranges must already have been checked.
    pub fn copy_between(
        from: &Space,
        src: u64,
        to: &Space,
        dst: u64,
        len: u64,
    ) -> Result<(), Error> {
        let mut done = 0;
        while done < len {
            let (a, b) = (src + done, dst + done);
            let n = (PAGE - a % PAGE).min(PAGE - b % PAGE).min(len - done);
            let pa = from.phys(a, perm::READ)?;
            let pb = to.phys(b, perm::WRITE)?;
            // SAFETY: both are pages of memory objects mapped in those spaces (whose locks the
            // caller holds), and `n` bytes stay inside each page. They may be the same page.
            unsafe { core::ptr::copy(phys_ptr::<u8>(pa), phys_ptr::<u8>(pb), n as usize) };
            done += n;
        }
        Ok(())
    }

    /// Removes every mapping and frees the page tables' lower half: the space can't be used
    /// again. It must not be loaded on any core. Returns the mappings.
    pub fn clear(&mut self) -> Vec<Mapping> {
        unload_everywhere(self.tables.root);
        let starts: Vec<u64> = self.maps.keys().copied().collect();
        let out: Vec<Mapping> = starts.into_iter().filter_map(|s| self.remove(s)).collect();
        self.tables.free_tables(&mut Frames(&mut self.charge), 0..256);
        self.dead = true;
        out
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        if !self.dead {
            self.clear();
        }
        unload_everywhere(self.tables.root);
        pmm::free(self.tables.root, 1);
        self.charge.shrink(charge::PAGE_TABLE);
    }
}

/// Makes sure no core has `root` loaded, before its tables are freed. Every thread of its
/// process has exited by then, so another core can only still have it loaded for the moment
/// between switching away from that process's last thread and loading the next thread's
/// tables; this core may have it loaded itself if it ran that thread.
fn unload_everywhere(root: u64) {
    let me = percpu::this();
    if me.loaded_root.load(Ordering::SeqCst) == root {
        activate(None);
    }
    for c in percpu::all() {
        while c.index != me.index && c.loaded_root.load(Ordering::SeqCst) == root {
            core::hint::spin_loop();
        }
    }
}

/// Loads `root` (a program's tables) on this core, or the kernel's alone for `None`.
pub fn activate(root: Option<u64>) {
    let cpu = percpu::this();
    let want = root.unwrap_or(0);
    if cpu.loaded_root.load(Ordering::Relaxed) == want && want != 0 {
        return;
    }
    cpu.loaded_root.store(want, Ordering::SeqCst);
    arch::activate_space(want);
}
