// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio input device (virtio 1.x specification, section 5.8): keyboards, mice and
//! tablets that send Linux input events.
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
/// The range of one absolute axis (the `subsel` byte says which axis).
const CFG_ABS_INFO: u8 = 0x12;
/// Event types.
const EV_KEY: u8 = 1;
const EV_REL: u8 = 2;
const EV_ABS: u8 = 3;
/// The absolute axes a tablet has, which are also the bit numbers in its `EV_ABS` bitmap.
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
/// Where the configuration's answer starts: the first 8 bytes are the selector, the selector's
/// second byte, the answer's size, and padding.
const CFG_DATA: u32 = 8;
/// Bytes of an `ABS_INFO` answer the driver needs: `min` and `max`, each a little-endian
/// 32-bit number (`fuzz`, `flat` and `res` follow and are not used).
const ABS_INFO_NEEDED: u8 = 8;
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
    /// The device is neither a keyboard, a mouse nor a tablet.
    Unsupported,
    /// A tablet says an axis' range is empty or backwards (its largest value is not above its
    /// smallest), or answers the question about it with too few bytes to hold a range.
    BadRange,
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
    /// Says where it is: has absolute `X` and `Y` axes (a tablet or a touch screen).
    Tablet,
}

/// Checks the device's answer about one axis: `size` bytes came back, the first four being the
/// axis' smallest value and the next four its largest (both are signed numbers sent as raw
/// 32 bits). Returns them if the range is possible: at least 8 bytes, and the largest above the
/// smallest. The check is here, apart from the hardware, so that it can be tested on the host.
pub fn check_range(size: u8, min: u32, max: u32) -> Option<(i32, i32)> {
    // `as i32` reinterprets the same 32 bits as a signed number (it never fails or panics).
    let (min, max) = (min as i32, max as i32);
    (size >= ABS_INFO_NEEDED && max > min).then_some((min, max))
}

/// Scales `value`, on an axis that runs from `min` to `max`, to 0 to 65535 across the whole
/// axis: 0 for `min` and anything below it, 65535 for `max` and anything above it, and in
/// between the proportion, rounded down. Nothing overflows, whatever the inputs. (A `max` that
/// is not above `min` can't be scaled; the answer is 0. The driver never lets such a range
/// through, see [`check_range`].)
pub fn scale(value: i32, min: i32, max: i32) -> u16 {
    if max <= min || value <= min {
        return 0;
    }
    if value >= max {
        return u16::MAX;
    }
    // Do the sum in 64 bits: the span can be nearly 2^32 and times 65535 it would not fit in
    // 32. Here `min < value < max`, so the answer is between 0 and 65534.
    let above = i64::from(value) - i64::from(min);
    let span = i64::from(max) - i64::from(min);
    (above * i64::from(u16::MAX) / span) as u16
}

/// Follows a tablet's reports. A tablet sends `EV_ABS` events for X and for Y (not necessarily
/// both), and then an `EV_SYN` event to say "that is the whole report". This remembers the
/// latest value of each axis and hands out one position per report.
#[derive(Clone, Copy, Debug)]
pub struct Tablet {
    ranges: [(i32, i32); 2],
    /// The latest value of each axis, once the device has sent one.
    latest: [Option<i32>; 2],
    /// An axis has changed since the last position was handed out.
    changed: bool,
}

impl Tablet {
    /// Starts following a tablet whose axes have these (smallest, largest) values, as
    /// [`Input::abs_ranges`] gives them.
    pub fn new(ranges: [(i32, i32); 2]) -> Self {
        Tablet { ranges, latest: [None; 2], changed: false }
    }

    /// An `EV_ABS` event: remembers it if `code` is the X or Y axis, ignores any other axis.
    pub fn abs(&mut self, code: u16, value: i32) {
        let axis = match code {
            ABS_X => 0,
            ABS_Y => 1,
            _ => return,
        };
        self.latest[axis] = Some(value);
        self.changed = true;
    }

    /// The pointer's position, (x, y) each scaled to 0 to 65535, if an axis has changed since
    /// the last time this gave one. It waits until both axes have been seen at least once,
    /// rather than make up a value for the one it hasn't, which would throw the pointer to an
    /// edge of the screen.
    pub fn take(&mut self) -> Option<(u16, u16)> {
        let (Some(x), Some(y)) = (self.latest[0], self.latest[1]) else { return None };
        if !self.changed {
            return None;
        }
        self.changed = false;
        let [(x_min, x_max), (y_min, y_max)] = self.ranges;
        Some((scale(x, x_min, x_max), scale(y, y_min, y_max)))
    }
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
    /// A tablet's (smallest, largest) value on its X and Y axes.
    abs_ranges: Option<[(i32, i32); 2]>,
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
        // A tablet has absolute X and Y axes. This is checked first: QEMU's tablet also has
        // buttons and a wheel, which would otherwise make it look like a mouse.
        let has_xy = t.read_config(|c| {
            c.write8(0, CFG_EV_BITS)?;
            c.write8(1, EV_ABS)?;
            let size = usize::from(c.read8(2)?);
            // ABS_X and ABS_Y are bits 0 and 1 of the first byte.
            Ok(size >= 1 && c.read8(CFG_DATA)? & 0b11 == 0b11)
        })?;
        let mut abs_ranges = None;
        let kind = if has_xy {
            // The scaling divides by the range, so a range that makes no sense is the device
            // lying, not something to carry on with.
            let mut ranges = [(0, 0); 2];
            for (range, axis) in ranges.iter_mut().zip([ABS_X, ABS_Y]) {
                let (size, min, max) = t.read_config(|c| {
                    c.write8(0, CFG_ABS_INFO)?;
                    c.write8(1, axis as u8)?;
                    let size = c.read8(2)?;
                    if size < ABS_INFO_NEEDED {
                        return Ok((size, 0, 0));
                    }
                    Ok((size, c.read32(CFG_DATA)?, c.read32(CFG_DATA + 4)?))
                })?;
                match check_range(size, min, max) {
                    Some(r) => *range = r,
                    None => {
                        t.fail();
                        return Err(Error::BadRange);
                    }
                }
            }
            abs_ranges = Some(ranges);
            Kind::Tablet
        } else {
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
            if has_a {
                Kind::Keyboard
            } else if has_rel {
                Kind::Mouse
            } else {
                t.fail();
                return Err(Error::Unsupported);
            }
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
        Ok(Input {
            transport: t,
            dma,
            dma_bus,
            queue,
            notify_offset,
            slot_of_head,
            kind,
            name,
            abs_ranges,
        })
    }

    /// Keyboard, mouse or tablet.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// For a tablet, the (smallest, largest) value its X axis and its Y axis can report, as
    /// the device said when it started. `None` for anything else.
    pub fn abs_ranges(&self) -> Option<[(i32, i32); 2]> {
        self.abs_ranges
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
