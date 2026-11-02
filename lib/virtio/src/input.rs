// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio input device (virtio 1.x specification, section 5.8): keyboards and mice that
//! send Linux input events.
//!
//! The driver keeps the event queue full of empty 8-byte buffers. The device fills one for
//! each event and returns it; the driver reads the event and gives the buffer back. Nothing
//! the device writes is trusted: a completion for a buffer the driver didn't post, or one that
//! isn't exactly an event long, is an error, and the queue is then broken until reset.

use crate::queue::{self, Buffer, Queue};
use crate::transport::{self, Layout as TransportLayout, Transport};
use alloc::string::String;
use alloc::vec::Vec;
use pbr_devices::regs::Registers;

/// Bytes in an event: type, code, value.
const EVENT: usize = 8;
const QUEUE_AT: usize = 0;
const EVENTS_AT: usize = 4096;
/// The queue size used (or less, if the device allows less).
const QUEUE_ENTRIES: u16 = 64;
/// The DMA memory an input device needs.
pub const DMA_NEEDED: usize = EVENTS_AT + EVENT * QUEUE_ENTRIES as usize;

/// Configuration selectors (the `select` byte of the device configuration).
const CFG_ID_NAME: u8 = 0x01;
const CFG_EV_BITS: u8 = 0x11;
/// Event types.
const EV_KEY: u8 = 1;
const EV_REL: u8 = 2;
/// The code of the key A: if the device has it, it is a keyboard.
const KEY_A: usize = 30;

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The transport failed.
    Transport(transport::Error),
    /// The queue failed (the device lied).
    Queue(queue::Error),
    /// The DMA memory is too small or unaligned.
    DmaMemory,
    /// The device is neither a keyboard nor a mouse.
    Unsupported,
    /// A completion that is impossible: the wrong size, or for a buffer that isn't ours.
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

/// What kind of device it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Has keys, and the letter A among them.
    Keyboard,
    /// Moves relative to where it was.
    Mouse,
}

/// An input event, as the device sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// `EV_KEY` (1), `EV_REL` (2) and so on.
    pub kind: u16,
    /// Which key, button or axis.
    pub code: u16,
    /// The value: 1 down, 0 up, 2 repeat for keys; the amount for movement.
    pub value: i32,
}

/// A virtio input device, started.
pub struct Input<R: Registers, M: Registers> {
    transport: Transport<R>,
    dma: M,
    dma_bus: u64,
    queue: Queue,
    notify_offset: u32,
    /// Which of the event buffers each descriptor chain head holds.
    slot_of_head: Vec<Option<usize>>,
    kind: Kind,
    name: String,
}

impl<R: Registers, M: Registers> Input<R, M> {
    /// Resets the device, learns what it is, and starts it with the event queue full.
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
        t.negotiate(0)?;
        let name = t.read_config(|c| {
            c.write8(0, CFG_ID_NAME)?;
            c.write8(1, 0)?;
            let n = usize::from(c.read8(2)?).min(128);
            let mut bytes = Vec::with_capacity(n);
            for i in 0..n {
                bytes.push(c.read8(8 + i as u32)?);
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })?;
        // A keyboard has the letter A; a mouse moves.
        let has_a = t.read_config(|c| {
            c.write8(0, CFG_EV_BITS)?;
            c.write8(1, EV_KEY)?;
            let size = usize::from(c.read8(2)?);
            Ok(size > KEY_A / 8 && c.read8(8 + (KEY_A / 8) as u32)? & (1 << (KEY_A % 8)) != 0)
        })?;
        let has_rel = t.read_config(|c| {
            c.write8(0, CFG_EV_BITS)?;
            c.write8(1, EV_REL)?;
            Ok(c.read8(2)? != 0)
        })?;
        let kind = if has_a {
            Kind::Keyboard
        } else if has_rel {
            Kind::Mouse
        } else {
            t.fail();
            return Err(Error::Unsupported);
        };

        t.set_config_vector(None)?;
        let max = t.queue_max(0)?;
        let size = transport::round_down_pow2(max.min(QUEUE_ENTRIES));
        let mut queue = Queue::new(&dma, QUEUE_AT, dma_bus, size).ok_or(Error::DmaMemory)?;
        let notify_offset = t.enable_queue(0, size, queue.addresses(), vector)?;
        let mut slot_of_head = alloc::vec![None; usize::from(size)];
        for slot in 0..usize::from(size) {
            let bus = dma_bus + (EVENTS_AT + slot * EVENT) as u64;
            let head =
                queue.push(&dma, &[Buffer { bus, len: EVENT as u32, device_writes: true }])?;
            slot_of_head[usize::from(head)] = Some(slot);
        }
        t.driver_ok()?;
        t.notify(0, notify_offset);
        Ok(Input { transport: t, dma, dma_bus, queue, notify_offset, slot_of_head, kind, name })
    }

    /// Keyboard or mouse.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The device's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reads the interrupt status (clearing it).
    pub fn interrupt_status(&self) -> u8 {
        self.transport.isr()
    }

    /// The next event the device has sent, if any. Gives its buffer back to the device.
    pub fn poll(&mut self) -> Result<Option<Event>, Error> {
        let Some(done) = self.queue.pop(&self.dma)? else { return Ok(None) };
        let slot = self
            .slot_of_head
            .get_mut(usize::from(done.head))
            .and_then(Option::take)
            .ok_or(Error::BadCompletion)?;
        if done.written != EVENT as u32 {
            return Err(Error::BadCompletion);
        }
        let at = EVENTS_AT + slot * EVENT;
        let event = Event {
            kind: self.dma.read16(at),
            code: self.dma.read16(at + 2),
            value: self.dma.read32(at + 4) as i32,
        };
        // Give the buffer back.
        let bus = self.dma_bus + at as u64;
        let head = self
            .queue
            .push(&self.dma, &[Buffer { bus, len: EVENT as u32, device_writes: true }])?;
        self.slot_of_head[usize::from(head)] = Some(slot);
        self.transport.notify(0, self.notify_offset);
        Ok(Some(event))
    }

    /// Resets the device.
    pub fn stop(&mut self) {
        let _ = self.transport.reset();
    }
}
