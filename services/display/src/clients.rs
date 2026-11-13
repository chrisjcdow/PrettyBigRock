// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What this program holds for each client: its ring of events, and its windows' buffers
//! (docs/design/0022-display-server.md, sections 3 and 10).
//!
//! **Rings.** A client's events go through a ring (`granite::ring`) in memory this program
//! made, which the client maps to read. The client can do what it likes to that memory, but
//! the ring code here checks everything it reads back from it, so a client can only make
//! itself lose events. When a client's ring is full its events are dropped and counted, and a
//! "lost" event says how many as soon as there is room: the client then redraws and assumes
//! no key is held.
//!
//! **Buffers.** A window's two frames are one memory object this program makes, in its own
//! budget, and lends: the client gets a copy it can map to write, this program maps the
//! original to read. Because the client's copy was made from this program's, the client can't
//! take the memory back (revoking a capability takes only the copies made from it), so
//! composing can never fault on memory that vanished (note 0006, section 5).

use alloc::collections::BTreeMap;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::ring::{self, Producer};
use granite::sys::{perm, rights};
use pbr_compositor::draw::Frames;
use pbr_compositor::event::{EVENT_SIZE, Event, kind};
use pbr_compositor::{Buffers, ClientId, Shown, WindowId};
use pbr_draw::Picture;

/// Bytes in a client's ring: room for about five hundred events.
const RING_BYTES: u64 = 16384;

/// A client's ring, as this program holds it.
struct Ring {
    _memory: Memory,
    /// Kept mapped for the producer, which writes through it.
    _map: Mapping,
    producer: Producer,
    doorbell: Notification,
    /// Something was added since the doorbell last rang.
    rang: bool,
    /// The client wrecked the ring: nothing more is written to it.
    broken: bool,
}

/// What this program keeps for one client.
#[derive(Default)]
struct Record {
    ring: Option<Ring>,
    /// Events dropped (because the ring was full, or not asked for yet) and not yet reported.
    lost: u32,
}

/// One window's buffers, mapped to read.
struct Buf {
    memory: Memory,
    map: Mapping,
    buffers: Buffers,
}

/// Every client's ring and buffers.
#[derive(Default)]
pub struct Clients {
    records: BTreeMap<ClientId, Record>,
    bufs: BTreeMap<(ClientId, u32, u32), Buf>,
}

/// Why `events` failed.
pub enum RingTrouble {
    /// It was asked for already.
    Twice,
    /// The memory or the doorbell couldn't be made.
    NoMemory,
}

impl Clients {
    /// A new client.
    pub fn add(&mut self, client: ClientId) {
        self.records.insert(client, Record::default());
    }

    /// A client has gone: its ring and every buffer go.
    pub fn remove(&mut self, client: ClientId) {
        self.records.remove(&client);
        self.bufs.retain(|k, _| k.0 != client);
    }

    /// `events`: makes the client's ring, and gives a copy; `doorbell` is the client's own
    /// notification, signalled (bit 0) when events are added. Signalling a notification never
    /// waits, and fails cleanly if the client has taken it back.
    pub fn events(
        &mut self,
        client: ClientId,
        doorbell: Notification,
    ) -> Result<Memory, RingTrouble> {
        let record = self.records.entry(client).or_default();
        if record.ring.is_some() {
            return Err(RingTrouble::Twice);
        }
        let made = || -> granite::Result<(Ring, Memory)> {
            let memory = Memory::create(RING_BYTES)?;
            let map = memory.map(perm::READ_WRITE)?;
            // SAFETY: the mapping is RING_BYTES long, writable, page aligned, and holds only
            // the ring; it is kept beside the producer in `Ring`, so it outlives it.
            let producer = unsafe {
                ring::init(map.as_mut_ptr(), RING_BYTES as usize, ring::Kind::Entries, EVENT_SIZE)
                    .and_then(|_| Producer::new(map.as_mut_ptr(), RING_BYTES as usize))
            }
            .map_err(|_| granite::Error::InvalidArgument)?;
            // The client may read and write the ring's memory (reading moves its "read up to
            // here" mark, which is in the memory).
            let theirs = memory
                .0
                .duplicate(rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER)?;
            Ok((
                Ring { _memory: memory, _map: map, producer, doorbell, rang: false, broken: false },
                Memory(theirs),
            ))
        };
        let (ring, memory) = made().map_err(|_| RingTrouble::NoMemory)?;
        record.ring = Some(ring);
        Ok(memory)
    }

    /// Puts an event in a client's ring (or counts it lost).
    pub fn deliver(&mut self, client: ClientId, e: &Event) {
        let Some(record) = self.records.get_mut(&client) else { return };
        let Some(ring) = record.ring.as_mut().filter(|r| !r.broken) else {
            record.lost = record.lost.saturating_add(1);
            return;
        };
        let free = match ring.producer.free() {
            Ok(f) => f,
            Err(_) => {
                ring.broken = true;
                return;
            }
        };
        // A lost event goes first, so the client knows what it missed before what follows.
        let need = if record.lost > 0 { 2 } else { 1 };
        if free < need {
            record.lost = record.lost.saturating_add(1);
            return;
        }
        if record.lost > 0 {
            let l = Event::new(kind::LOST, 0, [record.lost, 0, 0], e.time);
            if ring.producer.push(&l.encode()).is_err() {
                ring.broken = true;
                return;
            }
            record.lost = 0;
        }
        if ring.producer.push(&e.encode()).is_err() {
            ring.broken = true;
            return;
        }
        ring.rang = true;
    }

    /// Rings the doorbell of every client whose ring got something, and reports losses to
    /// clients that have made room since.
    pub fn ring(&mut self, now: u64) {
        for record in self.records.values_mut() {
            let Some(ring) = record.ring.as_mut().filter(|r| !r.broken) else { continue };
            if record.lost > 0 && ring.producer.free().is_ok_and(|f| f >= 1) {
                let l = Event::new(kind::LOST, 0, [record.lost, 0, 0], now);
                if ring.producer.push(&l.encode()).is_ok() {
                    record.lost = 0;
                    ring.rang = true;
                }
            }
            if ring.rang {
                ring.rang = false;
                let _ = ring.doorbell.signal(1);
            }
        }
    }

    /// Whether some client has lost events not yet reported: try again soon.
    pub fn owe_losses(&self) -> bool {
        self.records.values().any(|r| r.lost > 0 && r.ring.as_ref().is_some_and(|g| !g.broken))
    }

    /// Makes a window's buffers, or copies the ones it has again (`reused`), and gives the
    /// client a copy it can map to write.
    pub fn buffers(&mut self, id: WindowId, b: Buffers, reused: bool) -> granite::Result<Memory> {
        let key = (id.client, id.number, b.generation);
        if !reused {
            let memory = Memory::create(b.bytes())?;
            let map = memory.map(perm::READ)?;
            self.bufs.insert(key, Buf { memory, map, buffers: b });
        }
        let buf = self.bufs.get(&key).ok_or(granite::Error::InvalidArgument)?;
        let copy = buf
            .memory
            .0
            .duplicate(rights::READ | rights::WRITE | rights::MAP | rights::TRANSFER)?;
        Ok(Memory(copy))
    }

    /// Lets go of a window's buffers of one generation.
    pub fn free(&mut self, id: WindowId, generation: u32) {
        self.bufs.remove(&(id.client, id.number, generation));
    }
}

impl Frames for Clients {
    fn picture(&self, id: WindowId, shown: &Shown) -> Option<Picture<'_>> {
        let buf = self.bufs.get(&(id.client, id.number, shown.buffers.generation))?;
        let b = &buf.buffers;
        let frame = b.frame_bytes();
        let start = frame.checked_mul(u64::from(shown.frame.min(1)))?;
        if start.checked_add(frame)? > buf.map.len() || b.stride % 4 != 0 {
            return None;
        }
        // SAFETY: the frame (`start` to `start + frame` bytes) is inside the mapping (checked
        // just above), which is page aligned and lives as long as `self`. The client may write
        // these pixels while they are read (a client that breaks the rules about frames it
        // gave away), which can only change the colours shown: every bit pattern is a pixel.
        let pixels = unsafe {
            core::slice::from_raw_parts(
                buf.map.as_ptr().add(start as usize).cast::<u32>(),
                (frame / 4) as usize,
            )
        };
        Picture::new(pixels, b.width, b.height, b.stride / 4)
    }
}
