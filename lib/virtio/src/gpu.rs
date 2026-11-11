// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The virtio GPU device, 2D only (virtio 1.2 specification, section 5.7;
//! docs/design/0020-screens-and-virtio-gpu.md, section 4).
//!
//! A GPU (graphics processor) keeps pictures in its own memory, as numbered *resources*, and
//! shows them on its *scanouts*: a scanout is one output, which drives one screen. Virtio's GPU
//! is the one QEMU provides; the pictures live in QEMU, and the driver talks to it with
//! *commands*. The driver draws in memory of its own (the resource's *backing*), then tells the
//! device to copy the parts that changed into the resource (`TRANSFER_TO_HOST_2D`) and to put the
//! resource on the screen (`RESOURCE_FLUSH`). This file knows the commands and nothing about
//! what the driver does with them.
//!
//! Commands travel on the *control queue*, a virtqueue (see [`crate::queue`]). Each is a pair of
//! buffers in the driver's DMA memory (memory the device can read and write too): the request
//! the device reads, and the response it writes. There is a fixed pool of [`SLOTS`] such pairs,
//! so at most that many commands are in flight. [`Gpu::submit`] sends a command under a
//! number the caller chooses (a *token*) and [`Gpu::poll`] says which have finished.
//!
//! The device is not trusted. Every response's length and type is checked, and so is every
//! count and size in it; a completion must belong to a command that was really sent. After
//! anything impossible, [`Gpu::poll`] returns an error and the device is *broken*: it refuses
//! everything until it is reset by a new driver (the supervisor starts one).

use crate::queue::{self, Buffer, Queue};
use crate::transport::{self, Layout as TransportLayout, Transport};
use alloc::vec;
use alloc::vec::Vec;
use pbr_devices::regs::Registers;
use pbr_devices::screens::MAX_SIDE;

/// The feature bit for `GET_EDID`: a screen's description of itself (see `pbr_devices::edid`).
pub const F_EDID: u64 = 1 << 1;

/// The bit of the device's events that says the screens changed (one was plugged in or out, or
/// resized).
pub const EVENT_DISPLAY: u32 = 1;

/// The most scanouts a virtio GPU has.
pub const MAX_SCANOUTS: u32 = 16;

/// How many commands can be in flight at once.
pub const SLOTS: usize = 16;

/// The pixel format of the resources made here: blue in the lowest byte, then green, red and a
/// byte not used. It is `pbr.display`'s buffer format.
pub const FORMAT_B8G8R8X8_UNORM: u32 = 2;

/// The commands. (Numbers from the specification.)
mod cmd {
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const GET_EDID: u32 = 0x010A;
}

/// The types of response.
mod resp {
    /// The command worked and there is nothing more to say.
    pub const OK_NODATA: u32 = 0x1100;
    pub const OK_DISPLAY_INFO: u32 = 0x1101;
    pub const OK_EDID: u32 = 0x1104;
    /// Responses from here to `ERR_LAST` say the command failed.
    pub const ERR_FIRST: u32 = 0x1200;
    pub const ERR_LAST: u32 = 0x12FF;
}

/// The error codes the specification defines, for the callers that care which it was.
pub mod error_code {
    /// Something went wrong, unspecified.
    pub const UNSPEC: u32 = 0x1200;
    /// The device has no memory for it.
    pub const OUT_OF_MEMORY: u32 = 0x1201;
    /// There is no such scanout.
    pub const INVALID_SCANOUT_ID: u32 = 0x1202;
    /// There is no such resource.
    pub const INVALID_RESOURCE_ID: u32 = 0x1203;
    /// There is no such context (3D only).
    pub const INVALID_CONTEXT_ID: u32 = 0x1204;
    /// Some number in the command is no good.
    pub const INVALID_PARAMETER: u32 = 0x1205;
}

/// The flag that asks the device to answer a command with the same fence.
const FLAG_FENCE: u32 = 1;

/// The device configuration: what the device says about itself outside the queues.
mod config {
    /// Bits for events the device has to tell the driver about.
    pub const EVENTS_READ: u32 = 0;
    /// The driver writes the bits it has dealt with here.
    pub const EVENTS_CLEAR: u32 = 4;
    /// How many scanouts there are.
    pub const NUM_SCANOUTS: u32 = 8;
}

/// Bytes in the header every request and response starts with: the command or response type,
/// flags, the fence, the context and padding.
const HEADER: usize = 24;
/// Bytes in one scanout's entry in the display information.
const INFO_ENTRY: usize = 24;
/// A response to `GET_DISPLAY_INFO`: the header and an entry for each of 16 scanouts.
const INFO_RESPONSE: usize = HEADER + MAX_SCANOUTS as usize * INFO_ENTRY;
/// The most bytes of EDID a response holds.
const EDID_MAX: usize = 1024;
/// A response to `GET_EDID`: the header, the size, padding and the EDID.
const EDID_RESPONSE: usize = HEADER + 8 + EDID_MAX;
/// The most bytes in a request.
const REQUEST_MAX: usize = 64;

/// Where things are in the DMA memory: the queue first, then the pool of command slots, each
/// with room for a request and then a response.
const QUEUE_AT: usize = 0;
const SLOTS_AT: usize = 4096;
const SLOT_SIZE: usize = 2048;
const RESPONSE_AT: usize = REQUEST_MAX;
/// The DMA memory a GPU device needs.
pub const DMA_NEEDED: usize = SLOTS_AT + SLOTS * SLOT_SIZE;
/// The queue size used (or less, if the device allows less).
const QUEUE_ENTRIES: u16 = 64;
/// The commands a driver may need in flight at once (a present of up to four rectangles sends
/// seven). The queue must have room for this many pairs of buffers, or a device with a tiny
/// queue could never be sent a whole present, so a smaller queue is refused at the start.
pub const MIN_COMMANDS: usize = 8;

/// A response is zeroed before the command is sent, so that nothing a previous command left
/// behind can be mistaken for the device's answer.
static ZEROS: [u8; EDID_RESPONSE] = [0; EDID_RESPONSE];

/// What went wrong. (`derive` asks the compiler to write the obvious code to copy, print and
/// compare a type, so that tests can say `assert_eq!(result, Err(Error::Broken))`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The transport failed.
    Transport(transport::Error),
    /// The queue failed (the device lied).
    Queue(queue::Error),
    /// The DMA memory is too small or unaligned.
    DmaMemory,
    /// The device has no scanouts, or more than there can be.
    Scanouts,
    /// The device's queue is too small to be of use.
    QueueTooSmall,
    /// The command has a number in it that can't be right (a resource 0, an empty rectangle, a
    /// scanout that doesn't exist). Nothing was sent.
    BadArgument,
    /// The command needs a feature the device didn't offer.
    Unsupported,
    /// All the command slots are in use. Poll for completions and send again.
    PoolFull,
    /// The device's answer is impossible: the wrong type or size, a bad count, or a fence that
    /// doesn't match. The device is broken.
    BadCompletion,
    /// The device lied earlier; reset it.
    Broken,
}

impl From<transport::Error> for Error {
    fn from(e: transport::Error) -> Self {
        Error::Transport(e)
    }
}

/// A rectangle of pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

impl Rect {
    /// Not empty, and not running off the end of the numbers.
    fn is_sound(&self) -> bool {
        self.width != 0
            && self.height != 0
            && self.x.checked_add(self.width).is_some()
            && self.y.checked_add(self.height).is_some()
    }
}

/// One scanout, as the device describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scanout {
    /// The picture's place and size. For an enabled scanout, its size is the mode the device
    /// prefers.
    pub rect: Rect,
    /// Something is connected to it.
    pub enabled: bool,
}

/// A command. Resource numbers are the driver's to choose, but not 0, which the device keeps to
/// mean "no resource".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Ask for every scanout's size and whether it is enabled.
    GetDisplayInfo,
    /// Ask for a scanout's EDID. Needs [`F_EDID`].
    GetEdid {
        /// Which scanout.
        scanout: u32,
    },
    /// Make a resource of `width` by `height` pixels in [`FORMAT_B8G8R8X8_UNORM`].
    CreateResource {
        /// Its number.
        resource: u32,
        /// Pixels across.
        width: u32,
        /// Pixels down.
        height: u32,
    },
    /// Free a resource.
    Unref {
        /// Which.
        resource: u32,
    },
    /// Show `rect` of a resource on a scanout. Resource 0 turns the scanout off.
    SetScanout {
        /// Which scanout.
        scanout: u32,
        /// Which resource (0 for none).
        resource: u32,
        /// The part of the resource to show: its whole size, for a screen of the same size.
        /// Ignored (it is sent as zeros) when `resource` is 0.
        rect: Rect,
    },
    /// Put what the device holds of `rect` of a resource on the screens that show it. With
    /// `fenced`, the device answers only once everything sent before has been done, and says
    /// so with the fence.
    Flush {
        /// Which resource.
        resource: u32,
        /// Which part.
        rect: Rect,
        /// Ask for a fence.
        fenced: bool,
    },
    /// Copy `rect` of the resource's backing memory into the resource. `offset` is where in
    /// the backing the first pixel of the rectangle is.
    Transfer {
        /// Which resource.
        resource: u32,
        /// Which part.
        rect: Rect,
        /// Byte offset of the rectangle's first pixel in the backing.
        offset: u64,
    },
    /// Give a resource its backing: `len` bytes at bus address `address` (one piece).
    AttachBacking {
        /// Which resource.
        resource: u32,
        /// The device's address of the memory.
        address: u64,
        /// How long it is.
        len: u32,
    },
    /// Take a resource's backing away.
    DetachBacking {
        /// Which resource.
        resource: u32,
    },
}

/// What a finished command answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// It worked.
    Done,
    /// The scanouts, as many as the device has.
    DisplayInfo(Vec<Scanout>),
    /// A scanout's EDID, as the device gave it (not checked: see `pbr_devices::edid`).
    Edid(Vec<u8>),
}

/// The device's error code (see [`error_code`]) for a command it refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceError(pub u32);

/// A command that has finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The token it was sent under.
    pub token: u64,
    /// What the device said.
    pub result: Result<Response, DeviceError>,
}

/// What kind of response a command gets, which says how long it can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    NoData,
    DisplayInfo,
    Edid,
}

impl Expect {
    /// The size of the response buffer.
    fn size(self) -> usize {
        match self {
            Expect::NoData => HEADER,
            Expect::DisplayInfo => INFO_RESPONSE,
            Expect::Edid => EDID_RESPONSE,
        }
    }
}

/// A command in flight.
#[derive(Clone, Copy, Debug)]
struct Pending {
    token: u64,
    expect: Expect,
    /// The fence the command asked for, which the response must repeat.
    fence: Option<u64>,
}

/// Reads a little-endian number (the least significant byte first, as virtio sends them) from
/// `data` at `at`, if there is room. `Option` is Rust's "maybe": `Some(value)` or `None`. A
/// function that returns one can use `?` on another `Option`, which returns `None` from it at
/// once if that one was `None`. That is how [`decode`] gives up on the first thing that is
/// wrong, without indexing past the end of anything.
fn le32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

/// Works out what a response means. `data` is the bytes the device says it wrote. `None` if
/// they are impossible for the command: too short, the wrong type, a bad count or size, or
/// the wrong fence. `scanouts` is how many the device said it has in its configuration.
fn decode(
    expect: Expect,
    fence: Option<u64>,
    scanouts: u32,
    data: &[u8],
) -> Option<Result<Response, DeviceError>> {
    // Every response starts with the whole header.
    if data.len() < HEADER {
        return None;
    }
    let kind = le32(data, 0)?;
    // A response to a command with a fence repeats the fence, errors included: it is how the
    // driver knows the device did everything before it.
    if let Some(fence) = fence
        && (le32(data, 4)? & FLAG_FENCE == 0 || le64(data, 8)? != fence)
    {
        return None;
    }
    if (resp::ERR_FIRST..=resp::ERR_LAST).contains(&kind) {
        return Some(Err(DeviceError(kind)));
    }
    match (expect, kind) {
        (Expect::NoData, resp::OK_NODATA) => Some(Ok(Response::Done)),
        (Expect::DisplayInfo, resp::OK_DISPLAY_INFO) => {
            // The device always sends all 16 entries, but only the ones for scanouts it has
            // mean anything, and those must all be there.
            if scanouts > MAX_SCANOUTS {
                return None;
            }
            let count = usize::try_from(scanouts).ok()?;
            let mut info = Vec::with_capacity(count);
            for i in 0..count {
                let at = HEADER.checked_add(i.checked_mul(INFO_ENTRY)?)?;
                // The whole entry has to be there, including the flags that aren't used.
                le32(data, at + 20)?;
                info.push(Scanout {
                    rect: Rect {
                        x: le32(data, at)?,
                        y: le32(data, at + 4)?,
                        width: le32(data, at + 8)?,
                        height: le32(data, at + 12)?,
                    },
                    enabled: le32(data, at + 16)? != 0,
                });
            }
            Some(Ok(Response::DisplayInfo(info)))
        }
        (Expect::Edid, resp::OK_EDID) => {
            let size = usize::try_from(le32(data, HEADER)?).ok()?;
            let end = (HEADER + 8).checked_add(size)?;
            if size > EDID_MAX {
                return None;
            }
            Some(Ok(Response::Edid(data.get(HEADER + 8..end)?.to_vec())))
        }
        _ => None,
    }
}

/// The bytes of a request being built.
struct Encoder(Vec<u8>);

impl Encoder {
    /// A request of type `kind`, with the header: flags (a fence, if there is one), the fence's
    /// number, and zeros for the context and padding.
    fn new(kind: u32, fence: Option<u64>) -> Encoder {
        let mut e = Encoder(Vec::with_capacity(REQUEST_MAX));
        e.u32(kind);
        e.u32(if fence.is_some() { FLAG_FENCE } else { 0 });
        e.u64(fence.unwrap_or(0));
        e.u32(0);
        e.u32(0);
        e
    }

    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn rect(&mut self, r: Rect) {
        for v in [r.x, r.y, r.width, r.height] {
            self.u32(v);
        }
    }
}

/// A virtio GPU device, started. It is *generic*: it works with any types `R` and `M` that
/// implement [`Registers`], which the real driver fills in with mapped device memory and the
/// tests with ordinary memory and a software GPU. `R` is the device's registers, `M` the DMA
/// memory shared with it.
pub struct Gpu<R: Registers, M: Registers> {
    transport: Transport<R>,
    dma: M,
    dma_bus: u64,
    queue: Queue,
    notify_offset: u32,
    scanouts: u32,
    edid: bool,
    /// How many of the slots can be used (the queue needs two descriptors for each).
    capacity: usize,
    /// The commands in flight, by slot.
    slots: [Option<Pending>; SLOTS],
    /// Which slot each descriptor chain head belongs to.
    slot_of_head: Vec<Option<usize>>,
    next_fence: u64,
    broken: bool,
}

impl<R: Registers, M: Registers> Gpu<R, M> {
    /// Resets the device and starts it: negotiates features (`VERSION_1`, and [`F_EDID`] if
    /// offered, never 3D or blobs), reads the number of scanouts, sets up the control queue in
    /// `dma` (whose first byte is at bus address `dma_bus`), and tells the device the driver is
    /// ready. `vector` is the MSI-X vector that interrupts go to, for the queue and for events.
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
        let features = t.negotiate(F_EDID)?;
        let scanouts = t.read_config(|c| c.read32(config::NUM_SCANOUTS))?;
        if scanouts == 0 || scanouts > MAX_SCANOUTS {
            t.fail();
            return Err(Error::Scanouts);
        }
        t.set_config_vector(vector)?;
        let max = t.queue_max(0)?;
        let size = transport::round_down_pow2(max.min(QUEUE_ENTRIES));
        let capacity = (usize::from(size) / 2).min(SLOTS);
        if capacity < MIN_COMMANDS {
            t.fail();
            return Err(Error::QueueTooSmall);
        }
        let queue = Queue::new(&dma, QUEUE_AT, dma_bus, size).ok_or(Error::DmaMemory)?;
        let notify_offset = t.enable_queue(0, size, queue.addresses(), vector)?;
        t.driver_ok()?;
        Ok(Gpu {
            transport: t,
            dma,
            dma_bus,
            queue,
            notify_offset,
            scanouts,
            edid: features & F_EDID != 0,
            capacity,
            slots: [None; SLOTS],
            slot_of_head: vec![None; usize::from(size)],
            next_fence: 1,
            broken: false,
        })
    }

    /// How many scanouts the device has (1 to [`MAX_SCANOUTS`]).
    pub fn scanouts(&self) -> u32 {
        self.scanouts
    }

    /// Whether `GET_EDID` can be used.
    pub fn has_edid(&self) -> bool {
        self.edid
    }

    /// How many commands can be in flight at once.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many more commands can be sent now.
    pub fn free_slots(&self) -> usize {
        self.slots.iter().take(self.capacity).filter(|s| s.is_none()).count()
    }

    /// How many commands are in flight.
    pub fn in_flight(&self) -> usize {
        self.capacity - self.free_slots()
    }

    /// True once the device has lied.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Reads the interrupt status (clearing it): bit 0 says a queue has used buffers, bit 1 that
    /// the configuration changed.
    pub fn interrupt_status(&self) -> u8 {
        self.transport.isr()
    }

    /// Reads the events the device has raised (see [`EVENT_DISPLAY`]) and tells it they are
    /// dealt with. Returns the bits that were set.
    pub fn take_events(&self) -> Result<u32, Error> {
        let bits = self.transport.read_config(|c| {
            let bits = c.read32(config::EVENTS_READ)?;
            if bits != 0 {
                c.write32(config::EVENTS_CLEAR, bits)?;
            }
            Ok(bits)
        })?;
        Ok(bits)
    }

    /// Sends a command, to be answered by a [`Completion`] carrying `token`.
    pub fn submit(&mut self, token: u64, command: Command) -> Result<(), Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        let fence = if matches!(command, Command::Flush { fenced: true, .. }) {
            let f = self.next_fence;
            // Fence 0 means "none" in the header, so skip it if the count ever goes round.
            self.next_fence = self.next_fence.wrapping_add(1).max(1);
            Some(f)
        } else {
            None
        };
        let (request, expect) = self.encode(&command, fence)?;
        let slot = self
            .slots
            .iter()
            .take(self.capacity)
            .position(Option::is_none)
            .ok_or(Error::PoolFull)?;
        let base = SLOTS_AT + slot * SLOT_SIZE;
        self.dma.write_bytes(base, &request.0);
        self.dma.write_bytes(base + RESPONSE_AT, &ZEROS[..expect.size()]);
        let at = |offset: usize| self.dma_bus + offset as u64;
        let bufs = [
            Buffer { bus: at(base), len: request.0.len() as u32, device_writes: false },
            Buffer { bus: at(base + RESPONSE_AT), len: expect.size() as u32, device_writes: true },
        ];
        let head = self.queue.push(&self.dma, &bufs).map_err(|e| match e {
            queue::Error::Full => Error::PoolFull,
            e => Error::Queue(e),
        })?;
        // `push` gave a head inside the queue, and `slot` is below `SLOTS`, so these always
        // find their places; `get_mut` makes that a check and not a hope.
        let (Some(by_head), Some(entry)) =
            (self.slot_of_head.get_mut(usize::from(head)), self.slots.get_mut(slot))
        else {
            self.broken = true;
            return Err(Error::BadCompletion);
        };
        *by_head = Some(slot);
        *entry = Some(Pending { token, expect, fence });
        self.transport.notify(0, self.notify_offset);
        Ok(())
    }

    /// Builds a command's request and checks its numbers. Nothing the caller passes is trusted
    /// to be right either: a bad number is an error here and not a message the device has to
    /// make sense of.
    fn encode(&self, command: &Command, fence: Option<u64>) -> Result<(Encoder, Expect), Error> {
        let scanout_ok = |s: u32| s < self.scanouts;
        let resource_ok = |r: u32| r != 0;
        let side_ok = |v: u32| (1..=MAX_SIDE).contains(&v);
        let bad = Err(Error::BadArgument);
        Ok(match *command {
            Command::GetDisplayInfo => {
                (Encoder::new(cmd::GET_DISPLAY_INFO, fence), Expect::DisplayInfo)
            }
            Command::GetEdid { scanout } => {
                if !self.edid {
                    return Err(Error::Unsupported);
                }
                if !scanout_ok(scanout) {
                    return bad;
                }
                let mut e = Encoder::new(cmd::GET_EDID, fence);
                e.u32(scanout);
                e.u32(0);
                (e, Expect::Edid)
            }
            Command::CreateResource { resource, width, height } => {
                if !resource_ok(resource) || !side_ok(width) || !side_ok(height) {
                    return bad;
                }
                let mut e = Encoder::new(cmd::RESOURCE_CREATE_2D, fence);
                for v in [resource, FORMAT_B8G8R8X8_UNORM, width, height] {
                    e.u32(v);
                }
                (e, Expect::NoData)
            }
            Command::Unref { resource } => {
                if !resource_ok(resource) {
                    return bad;
                }
                let mut e = Encoder::new(cmd::RESOURCE_UNREF, fence);
                e.u32(resource);
                e.u32(0);
                (e, Expect::NoData)
            }
            Command::SetScanout { scanout, resource, rect } => {
                // Turning a scanout off names no resource and no rectangle.
                let rect = if resource == 0 { Rect::default() } else { rect };
                if !scanout_ok(scanout) || (resource != 0 && !rect.is_sound()) {
                    return bad;
                }
                let mut e = Encoder::new(cmd::SET_SCANOUT, fence);
                e.rect(rect);
                e.u32(scanout);
                e.u32(resource);
                (e, Expect::NoData)
            }
            Command::Flush { resource, rect, .. } => {
                if !resource_ok(resource) || !rect.is_sound() {
                    return bad;
                }
                let mut e = Encoder::new(cmd::RESOURCE_FLUSH, fence);
                e.rect(rect);
                e.u32(resource);
                e.u32(0);
                (e, Expect::NoData)
            }
            Command::Transfer { resource, rect, offset } => {
                if !resource_ok(resource) || !rect.is_sound() {
                    return bad;
                }
                let mut e = Encoder::new(cmd::TRANSFER_TO_HOST_2D, fence);
                e.rect(rect);
                e.u64(offset);
                e.u32(resource);
                e.u32(0);
                (e, Expect::NoData)
            }
            Command::AttachBacking { resource, address, len } => {
                if !resource_ok(resource)
                    || len == 0
                    || address.checked_add(u64::from(len)).is_none()
                {
                    return bad;
                }
                let mut e = Encoder::new(cmd::RESOURCE_ATTACH_BACKING, fence);
                e.u32(resource);
                e.u32(1); // one entry follows: a piece of memory
                e.u64(address);
                e.u32(len);
                e.u32(0);
                (e, Expect::NoData)
            }
            Command::DetachBacking { resource } => {
                if !resource_ok(resource) {
                    return bad;
                }
                let mut e = Encoder::new(cmd::RESOURCE_DETACH_BACKING, fence);
                e.u32(resource);
                e.u32(0);
                (e, Expect::NoData)
            }
        })
    }

    /// Takes the next command the device has finished with, if any. An `Err` means the device
    /// said something impossible: it is broken from then on.
    pub fn poll(&mut self) -> Result<Option<Completion>, Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        let done = match self.queue.pop(&self.dma) {
            Ok(Some(done)) => done,
            Ok(None) => return Ok(None),
            Err(e) => {
                self.broken = true;
                return Err(Error::Queue(e));
            }
        };
        // The queue has checked that this chain was in flight; this checks that it is one of
        // ours with a command that was really sent.
        let slot = self.slot_of_head.get_mut(usize::from(done.head)).and_then(Option::take);
        let pending = slot.and_then(|s| self.slots.get_mut(s)).and_then(Option::take);
        let (Some(slot), Some(pending)) = (slot, pending) else {
            self.broken = true;
            return Err(Error::BadCompletion);
        };
        // `written` is at most the response buffer (the queue checked), so this is small. The
        // response is copied out once and then read from the copy: the device could change the
        // memory under the driver between two reads.
        let mut data = vec![0u8; done.written as usize];
        self.dma.read_bytes(SLOTS_AT + slot * SLOT_SIZE + RESPONSE_AT, &mut data);
        match decode(pending.expect, pending.fence, self.scanouts, &data) {
            Some(result) => Ok(Some(Completion { token: pending.token, result })),
            None => {
                self.broken = true;
                Err(Error::BadCompletion)
            }
        }
    }

    /// Tells the device the driver has given up on it, and resets it, abandoning everything in
    /// flight.
    pub fn stop(&mut self) {
        self.transport.fail();
        let _ = self.transport.reset();
        self.broken = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response of `kind` with room for `len` bytes in all.
    fn response(kind: u32, fence: Option<u64>, len: usize) -> Vec<u8> {
        let mut d = vec![0u8; len];
        d[..4].copy_from_slice(&kind.to_le_bytes());
        if let Some(f) = fence {
            d[4..8].copy_from_slice(&FLAG_FENCE.to_le_bytes());
            d[8..16].copy_from_slice(&f.to_le_bytes());
        }
        d
    }

    #[test]
    fn decode_checks_length_type_fence_and_counts() {
        use Expect::*;
        let ok = Some(Ok(Response::Done));
        assert_eq!(decode(NoData, None, 1, &response(resp::OK_NODATA, None, 24)), ok);
        // Longer than a header is fine; shorter is not.
        assert_eq!(decode(NoData, None, 1, &response(resp::OK_NODATA, None, 40)), ok);
        for n in 0..4 {
            assert_eq!(decode(NoData, None, 1, &response(resp::OK_NODATA, None, 24)[..n]), None);
        }
        assert_eq!(decode(NoData, None, 1, &[]), None);
        // The wrong type for the command.
        assert_eq!(decode(NoData, None, 1, &response(resp::OK_DISPLAY_INFO, None, 24)), None);
        assert_eq!(decode(NoData, None, 1, &response(0, None, 24)), None);
        assert_eq!(decode(NoData, None, 1, &response(0x1300, None, 24)), None);
        assert_eq!(decode(NoData, None, 1, &response(0x10FF, None, 24)), None);
        assert_eq!(decode(DisplayInfo, None, 1, &response(resp::OK_NODATA, None, 408)), None);
        assert_eq!(decode(Edid, None, 1, &response(resp::OK_NODATA, None, 1056)), None);
        // Errors are answers to anything, with their code.
        for kind in [resp::ERR_FIRST, error_code::INVALID_PARAMETER, resp::ERR_LAST] {
            let want = Some(Err(DeviceError(kind)));
            for e in [NoData, DisplayInfo, Edid] {
                assert_eq!(decode(e, None, 1, &response(kind, None, 24)), want);
            }
        }
        // A fence has to be repeated, even by an error.
        assert_eq!(decode(NoData, Some(7), 1, &response(resp::OK_NODATA, Some(7), 24)), ok);
        assert_eq!(decode(NoData, Some(7), 1, &response(resp::OK_NODATA, Some(8), 24)), None);
        assert_eq!(decode(NoData, Some(7), 1, &response(resp::OK_NODATA, None, 24)), None);
        assert_eq!(
            decode(NoData, Some(7), 1, &response(resp::ERR_FIRST, Some(7), 24)),
            Some(Err(DeviceError(resp::ERR_FIRST)))
        );
        assert_eq!(decode(NoData, Some(7), 1, &response(resp::ERR_FIRST, None, 24)), None);
        // Display information has to hold an entry for every scanout the device has.
        let info = response(resp::OK_DISPLAY_INFO, None, 24 + 24 * 2);
        assert!(
            matches!(decode(DisplayInfo, None, 2, &info), Some(Ok(Response::DisplayInfo(v))) if v.len() == 2)
        );
        assert_eq!(decode(DisplayInfo, None, 3, &info), None);
        assert_eq!(decode(DisplayInfo, None, 2, &info[..info.len() - 1]), None);
        assert_eq!(decode(DisplayInfo, None, u32::MAX, &info), None);
        // An EDID's size has to fit what was written, and the buffer.
        let mut edid = response(resp::OK_EDID, None, 32 + 128);
        edid[24..28].copy_from_slice(&128u32.to_le_bytes());
        assert!(
            matches!(decode(Edid, None, 1, &edid), Some(Ok(Response::Edid(v))) if v.len() == 128)
        );
        edid[24..28].copy_from_slice(&129u32.to_le_bytes());
        assert_eq!(decode(Edid, None, 1, &edid), None);
        edid[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(Edid, None, 1, &edid), None);
        let mut big = response(resp::OK_EDID, None, EDID_RESPONSE);
        big[24..28].copy_from_slice(&1025u32.to_le_bytes());
        assert_eq!(decode(Edid, None, 1, &big), None);
        big[24..28].copy_from_slice(&1024u32.to_le_bytes());
        assert!(decode(Edid, None, 1, &big).is_some());
        assert_eq!(decode(Edid, None, 1, &response(resp::OK_EDID, None, 31)), None);
    }

    #[test]
    fn the_layout_fits_the_dma_memory() {
        const { assert!(HEADER + 8 + EDID_MAX <= SLOT_SIZE - RESPONSE_AT) };
        const { assert!(REQUEST_MAX <= RESPONSE_AT) };
        // Two descriptors for each of 64 entries' worth of commands fit in the first page.
        let l = queue::Layout::new(QUEUE_ENTRIES).unwrap();
        assert!(l.total <= SLOTS_AT);
        assert_eq!(DMA_NEEDED, 4096 + 16 * 2048);
    }
}
