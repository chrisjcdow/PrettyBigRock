// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Split virtqueues (virtio 1.x specification, section 2.7): a table of descriptors, a ring of
//! buffers the driver has made available, and a ring of buffers the device has used.
//!
//! The device writes to the used ring, so nothing in it is trusted: [`Queue::pop`] checks that
//! the device hasn't used more buffers than were made available, that each buffer it used was
//! really in flight, and that it didn't claim to have written more than the buffer could
//! hold. After a lie the queue is broken: it refuses everything until the device is reset and
//! the queue made again.

use alloc::vec::Vec;
use pbr_devices::regs::Registers;

const F_NEXT: u16 = 1;
const F_WRITE: u16 = 2;

/// A piece of a request: some memory the device can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    /// Its bus address.
    pub bus: u64,
    /// Its length in bytes (not 0).
    pub len: u32,
    /// True if the device writes it, false if the device only reads it.
    pub device_writes: bool,
}

/// What can go wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// There aren't enough free descriptors.
    Full,
    /// The request has no buffers, too many, or an empty one.
    BadRequest,
    /// The queue is broken (the device lied earlier) and must be made again.
    Broken,
    /// The device says it used more buffers than are in flight.
    TooManyUsed,
    /// The device used a buffer that isn't in flight (an index out of range, or a request that
    /// was never made, or one already completed).
    NotInFlight,
    /// The device claims to have written more than the buffers can hold.
    BadLength,
}

/// Where the parts of a queue of `size` entries are in memory, relative to the start of the
/// queue's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The number of entries (a power of two).
    pub size: u16,
    /// The descriptor table.
    pub desc: usize,
    /// The available ring.
    pub avail: usize,
    /// The used ring.
    pub used: usize,
    /// The total bytes.
    pub total: usize,
}

impl Layout {
    /// The layout for `size` entries (a power of two, at most 32768).
    pub fn new(size: u16) -> Option<Layout> {
        if size == 0 || !size.is_power_of_two() || size > 32768 {
            return None;
        }
        let n = usize::from(size);
        let desc = 0;
        let avail = desc + 16 * n;
        let avail_end = avail + 4 + 2 * n + 2;
        let used = avail_end.next_multiple_of(4);
        let total = used + 4 + 8 * n + 2;
        Some(Layout { size, desc, avail, used, total })
    }
}

struct Chain {
    /// The descriptors, in order.
    descriptors: Vec<u16>,
    /// How many bytes the device may write in all.
    writable: u32,
}

/// A split virtqueue, driver side.
pub struct Queue {
    /// The parts, as offsets into the memory given to `new`.
    layout: Layout,
    /// Where the queue starts in that memory...
    base: usize,
    /// ...and as the device sees it.
    bus: u64,
    free: Vec<u16>,
    chains: Vec<Option<Chain>>,
    avail_idx: u16,
    last_used: u16,
    in_flight: u16,
    broken: bool,
}

/// A buffer the device has finished with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completed {
    /// The head of the chain, as [`Queue::push`] returned it.
    pub head: u16,
    /// How many bytes the device says it wrote (checked to be at most what was possible).
    pub written: u32,
}

impl Queue {
    /// A queue of `size` entries whose memory is `mem` from `offset` (at bus address `bus`).
    /// Zeroes the memory. `None` if `size` isn't usable or the memory is too small.
    pub fn new(mem: &impl Registers, offset: usize, bus: u64, size: u16) -> Option<Queue> {
        let layout = Layout::new(size)?;
        if offset.checked_add(layout.total)? > mem.len() || !offset.is_multiple_of(16) {
            return None;
        }
        let zeros = [0u8; 64];
        let mut at = 0;
        while at < layout.total {
            let n = (layout.total - at).min(zeros.len());
            mem.write_bytes(offset + at, &zeros[..n]);
            at += n;
        }
        let n = usize::from(size);
        Some(Queue {
            layout: Layout {
                desc: offset + layout.desc,
                avail: offset + layout.avail,
                used: offset + layout.used,
                ..layout
            },
            base: offset,
            bus,
            free: (0..size).rev().collect(),
            chains: (0..n).map(|_| None).collect(),
            avail_idx: 0,
            last_used: 0,
            in_flight: 0,
            broken: false,
        })
    }

    /// The bus addresses of the descriptor table, available ring and used ring.
    pub fn addresses(&self) -> (u64, u64, u64) {
        (
            self.bus + (self.layout.desc - self.base) as u64,
            self.bus + (self.layout.avail - self.base) as u64,
            self.bus + (self.layout.used - self.base) as u64,
        )
    }

    /// The number of entries.
    pub fn size(&self) -> u16 {
        self.layout.size
    }

    /// How many requests are in flight.
    pub fn in_flight(&self) -> u16 {
        self.in_flight
    }

    /// True once the device has lied.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Makes a request of one chained descriptor per buffer available to the device. Returns
    /// the chain's head. The caller must then notify the device.
    pub fn push(&mut self, mem: &impl Registers, bufs: &[Buffer]) -> Result<u16, Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        if bufs.is_empty()
            || bufs.len() > usize::from(self.layout.size)
            || bufs.iter().any(|b| b.len == 0)
        {
            return Err(Error::BadRequest);
        }
        if self.free.len() < bufs.len() {
            return Err(Error::Full);
        }
        let mut descriptors = Vec::with_capacity(bufs.len());
        for _ in bufs {
            descriptors.push(self.free.pop().ok_or(Error::Full)?);
        }
        let mut writable = 0u32;
        for (i, b) in bufs.iter().enumerate() {
            let d = self.layout.desc + 16 * usize::from(descriptors[i]);
            let last = i + 1 == bufs.len();
            let mut flags = if last { 0 } else { F_NEXT };
            if b.device_writes {
                flags |= F_WRITE;
                writable = writable.saturating_add(b.len);
            }
            mem.write64(d, b.bus);
            mem.write32(d + 8, b.len);
            mem.write16(d + 12, flags);
            mem.write16(d + 14, if last { 0 } else { descriptors[i + 1] });
        }
        let head = descriptors[0];
        self.chains[usize::from(head)] = Some(Chain { descriptors, writable });
        // Put the head in the ring, then (after the entry is visible) move the index.
        let slot = usize::from(self.avail_idx % self.layout.size);
        mem.write16(self.layout.avail + 4 + 2 * slot, head);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        mem.write16(self.layout.avail + 2, self.avail_idx);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.in_flight += 1;
        Ok(head)
    }

    fn lie<T>(&mut self, e: Error) -> Result<T, Error> {
        self.broken = true;
        Err(e)
    }

    /// Takes the next buffer the device has finished with, if any.
    pub fn pop(&mut self, mem: &impl Registers) -> Result<Option<Completed>, Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let used_idx = mem.read16(self.layout.used + 2);
        if used_idx == self.last_used {
            return Ok(None);
        }
        if used_idx.wrapping_sub(self.last_used) > self.in_flight {
            return self.lie(Error::TooManyUsed);
        }
        let slot = usize::from(self.last_used % self.layout.size);
        let entry = self.layout.used + 4 + 8 * slot;
        let (id, len) = (mem.read32(entry), mem.read32(entry + 4));
        let head = match u16::try_from(id) {
            Ok(h) if h < self.layout.size => h,
            _ => return self.lie(Error::NotInFlight),
        };
        let Some(chain) = self.chains[usize::from(head)].as_ref() else {
            return self.lie(Error::NotInFlight);
        };
        if len > chain.writable {
            return self.lie(Error::BadLength);
        }
        let chain = self.chains[usize::from(head)].take().expect("checked just above");
        for d in chain.descriptors.into_iter().rev() {
            self.free.push(d);
        }
        self.last_used = self.last_used.wrapping_add(1);
        self.in_flight -= 1;
        Ok(Some(Completed { head, written: len }))
    }
}
