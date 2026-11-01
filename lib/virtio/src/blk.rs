// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio block device (virtio 1.x specification, section 5.2): requests are a header, the
//! data and a status byte, in one chain on the one request queue.
//!
//! One request is in flight at a time, in a buffer in the driver's DMA memory. The driver
//! calls [`Block::submit`], waits for an interrupt (or polls), and calls [`Block::poll`]. The
//! device is not trusted: a status byte that isn't one of the three defined, a short or
//! over-long completion, or a capacity that makes no sense are all errors.

use crate::queue::{self, Buffer, Queue};
use crate::transport::{self, Layout as TransportLayout, Transport};
use pbr_devices::regs::Registers;

/// Bytes in a sector (the unit capacity and sector numbers are in, whatever the device's block
/// size).
pub const SECTOR: usize = 512;
/// The most sectors in one request.
pub const MAX_SECTORS: u32 = 128;

/// The device accepts being read-only.
pub const F_RO: u64 = 1 << 5;
/// The device has a write cache that a flush request empties.
pub const F_FLUSH: u64 = 1 << 9;

const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const T_FLUSH: u32 = 4;

/// The status byte's value before the device has written it (not one it may use).
const PENDING: u8 = 0xFF;

/// Where things are in the DMA memory.
const QUEUE_AT: usize = 0;
const HEADER_AT: usize = 4096;
const STATUS_AT: usize = HEADER_AT + 16;
const DATA_AT: usize = 8192;
/// The DMA memory a block device needs.
pub const DMA_NEEDED: usize = DATA_AT + SECTOR * MAX_SECTORS as usize;
/// The queue size used (or less, if the device allows less).
const QUEUE_ENTRIES: u16 = 64;

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The transport failed.
    Transport(transport::Error),
    /// The queue failed (the device lied).
    Queue(queue::Error),
    /// The DMA memory is too small or unaligned.
    DmaMemory,
    /// The device reports a capacity that makes no sense.
    Capacity,
    /// The request isn't on the disk.
    OutOfRange,
    /// The disk is read-only.
    ReadOnly,
    /// The request is empty, too big or not whole sectors.
    BadArgument,
    /// A request is already in flight.
    Busy,
    /// There is no request in flight.
    Idle,
    /// The device failed the request.
    Io,
    /// The device's answer is impossible: a status byte it may not use, or a completion that
    /// is the wrong size or for the wrong request.
    BadCompletion,
}

impl From<transport::Error> for Error {
    fn from(e: transport::Error) -> Self {
        Error::Transport(e)
    }
}

impl From<queue::Error> for Error {
    fn from(e: queue::Error) -> Self {
        Error::Queue(e)
    }
}

/// A request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// Read `count` sectors.
    Read {
        /// The first sector.
        sector: u64,
        /// How many (1 to [`MAX_SECTORS`]).
        count: u32,
    },
    /// Write whole sectors.
    Write {
        /// The first sector.
        sector: u64,
        /// The data: a whole number of sectors.
        data: &'a [u8],
    },
    /// Empty the device's write cache.
    Flush,
}

/// A request that has finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    /// A read: the data is in [`Block::data`].
    Read {
        /// How many bytes.
        bytes: usize,
    },
    /// A write.
    Written,
    /// A flush.
    Flushed,
}

#[derive(Clone, Copy)]
struct InFlight {
    head: u16,
    kind: u32,
    data_len: u32,
}

/// A virtio block device, started.
pub struct Block<R: Registers, M: Registers> {
    transport: Transport<R>,
    dma: M,
    dma_bus: u64,
    queue: Queue,
    notify_offset: u32,
    capacity: u64,
    read_only: bool,
    flush: bool,
    pending: Option<InFlight>,
}

impl<R: Registers, M: Registers> Block<R, M> {
    /// Resets the device and starts it: negotiates features, reads the capacity, sets up the
    /// request queue in `dma` (whose first byte is at bus address `dma_bus`), and tells the
    /// device the driver is ready. `vector` is the MSI-X vector queue interrupts go to.
    pub fn start(
        layout: TransportLayout,
        bars: [Option<R>; 6],
        dma: M,
        dma_bus: u64,
        vector: Option<u16>,
    ) -> Result<Self, Error> {
        if dma.len() < DMA_NEEDED
            || !dma_bus.is_multiple_of(4096)
            || dma_bus.checked_add(dma.len() as u64).is_none()
        {
            return Err(Error::DmaMemory);
        }
        let t = Transport::new(layout, bars)?;
        t.reset()?;
        let features = t.negotiate(F_RO | F_FLUSH)?;
        let capacity = t.read_config(|c| c.read64(0))?;
        // The capacity is in sectors: the byte count has to fit, and a disk of nothing is not
        // one to use.
        if capacity == 0 || capacity > u64::MAX / SECTOR as u64 / 2 {
            t.fail();
            return Err(Error::Capacity);
        }
        t.set_config_vector(None)?;
        let max = t.queue_max(0)?;
        let size = transport::round_down_pow2(max.min(QUEUE_ENTRIES));
        let queue = Queue::new(&dma, QUEUE_AT, dma_bus, size).ok_or(Error::DmaMemory)?;
        let notify_offset = t.enable_queue(0, size, queue.addresses(), vector)?;
        t.driver_ok()?;
        Ok(Block {
            transport: t,
            dma,
            dma_bus,
            queue,
            notify_offset,
            capacity,
            read_only: features & F_RO != 0,
            flush: features & F_FLUSH != 0,
            pending: None,
        })
    }

    /// The disk's size in sectors.
    pub fn sectors(&self) -> u64 {
        self.capacity
    }

    /// True if writes are refused.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// True if a request is in flight.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Reads the interrupt status (clearing it): bit 0 says a queue has used buffers.
    pub fn interrupt_status(&self) -> u8 {
        self.transport.isr()
    }

    /// Starts a request.
    pub fn submit(&mut self, r: Request<'_>) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let (kind, sector, data_len) = match r {
            Request::Read { sector, count } => {
                if count == 0 || count > MAX_SECTORS {
                    return Err(Error::BadArgument);
                }
                (T_IN, sector, count * SECTOR as u32)
            }
            Request::Write { sector, data } => {
                if data.is_empty()
                    || data.len() > SECTOR * MAX_SECTORS as usize
                    || !data.len().is_multiple_of(SECTOR)
                {
                    return Err(Error::BadArgument);
                }
                if self.read_only {
                    return Err(Error::ReadOnly);
                }
                (T_OUT, sector, data.len() as u32)
            }
            Request::Flush => {
                if !self.flush {
                    return Err(Error::BadArgument); // nothing to flush: the caller checks `has_flush`
                }
                (T_FLUSH, 0, 0)
            }
        };
        if kind != T_FLUSH {
            let end =
                sector.checked_add(u64::from(data_len) / SECTOR as u64).ok_or(Error::OutOfRange)?;
            if end > self.capacity {
                return Err(Error::OutOfRange);
            }
        }
        if let Request::Write { data, .. } = r {
            self.dma.write_bytes(DATA_AT, data);
        }
        self.dma.write32(HEADER_AT, kind);
        self.dma.write32(HEADER_AT + 4, 0);
        self.dma.write64(HEADER_AT + 8, sector);
        self.dma.write8(STATUS_AT, PENDING);
        let at = |off: usize| self.dma_bus + off as u64;
        let header = Buffer { bus: at(HEADER_AT), len: 16, device_writes: false };
        let status = Buffer { bus: at(STATUS_AT), len: 1, device_writes: true };
        let data = Buffer { bus: at(DATA_AT), len: data_len, device_writes: kind == T_IN };
        let head = if data_len == 0 {
            self.queue.push(&self.dma, &[header, status])?
        } else {
            self.queue.push(&self.dma, &[header, data, status])?
        };
        self.pending = Some(InFlight { head, kind, data_len });
        self.transport.notify(0, self.notify_offset);
        Ok(())
    }

    /// Checks whether the request in flight has finished. `Ok(None)` means not yet.
    pub fn poll(&mut self) -> Result<Option<Done>, Error> {
        let Some(p) = self.pending else { return Err(Error::Idle) };
        let done = match self.queue.pop(&self.dma) {
            Ok(Some(done)) => done,
            Ok(None) => return Ok(None),
            Err(e) => {
                self.pending = None; // the queue is broken; the request is lost with it
                return Err(e.into());
            }
        };
        self.pending = None;
        if done.head != p.head {
            return Err(Error::BadCompletion);
        }
        match self.dma.read8(STATUS_AT) {
            0 => {}
            1 | 2 => return Err(Error::Io),
            _ => return Err(Error::BadCompletion),
        }
        match p.kind {
            T_IN => {
                // The device wrote the data and the status byte.
                if done.written < p.data_len + 1 {
                    return Err(Error::BadCompletion);
                }
                Ok(Some(Done::Read { bytes: p.data_len as usize }))
            }
            T_OUT => Ok(Some(Done::Written)),
            _ => Ok(Some(Done::Flushed)),
        }
    }

    /// Copies out the data of the read that just finished.
    pub fn data(&self, out: &mut [u8]) {
        let n = out.len().min(SECTOR * MAX_SECTORS as usize);
        self.dma.read_bytes(DATA_AT, &mut out[..n]);
    }

    /// True if the device has a cache that flushes need to empty.
    pub fn has_flush(&self) -> bool {
        self.flush
    }

    /// Resets the device, abandoning anything in flight.
    pub fn stop(&mut self) {
        let _ = self.transport.reset();
        self.pending = None;
    }
}
