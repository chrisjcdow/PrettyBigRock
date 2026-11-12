// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Putting events in the consumer's ring, and what happens when it is full.
//!
//! The consumer (the display server, or the console) reads events from a ring: a fixed piece
//! of shared memory the input service writes to and the consumer reads from. A consumer that
//! is slow, stuck or hostile must never hold up the input service, so a full ring is not
//! waited for: events that don't fit are dropped and counted. When there is room again the
//! consumer is told how many it missed (a "lost" event) and then where things stand now (a
//! reset, and what is still held: [`Seat::held`]), because a key's release may be among what
//! was dropped, and a key the consumer thinks is down for ever is worse than a missed letter.
//!
//! The ring itself is `granite::ring`, which only programs can use; the rule is here, behind
//! the [`Sink`] trait, so that it is tested on the host with a pretend ring. (A *trait* is a
//! set of functions a type promises to have; code written against the trait works with any
//! type that has them.)

use crate::event::{EVENT_SIZE, Event, kind};
use crate::seat::Seat;
use alloc::vec::Vec;

/// Where events go: the consumer's ring.
pub trait Sink {
    /// How many more events fit right now.
    fn free(&self) -> usize;
    /// Adds an event. `false` means the ring is broken (the consumer wrote nonsense into its
    /// bookkeeping), not that it is full: [`Sink::free`] is asked first.
    fn push(&mut self, event: &[u8; EVENT_SIZE]) -> bool;
}

/// The ring is broken and can't be written to any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Broken;

/// What has been dropped for want of room, to be owned up to.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outbox {
    lost: u32,
}

impl Outbox {
    /// Nothing dropped.
    pub fn new() -> Outbox {
        Outbox::default()
    }

    /// How many events have been dropped and not yet reported.
    pub fn lost(&self) -> u32 {
        self.lost
    }

    /// For a consumer that has just taken the seat, with a new, empty ring: forgets what the
    /// last consumer missed, and starts the new one off with a reset and what is held.
    pub fn start(&mut self, sink: &mut impl Sink, seat: &Seat, now: u64) -> Result<(), Broken> {
        self.lost = 0;
        let mut first = Vec::new();
        seat.held(now, &mut first);
        self.deliver(sink, seat, &first, now).map(|_| ())
    }

    /// Puts `events` in the ring, in order. Returns whether anything was written (the
    /// consumer's doorbell is rung only then).
    ///
    /// - If everything fits, it goes in.
    /// - If not, what fits goes in and the rest is dropped and counted.
    /// - While a count is owed, new events are dropped and counted too, until there is room
    ///   for the whole catching-up at once: the lost event, the reset, and what is held.
    ///   (`seat` is where things stand after `events`, so the catching-up covers them.)
    pub fn deliver(
        &mut self,
        sink: &mut impl Sink,
        seat: &Seat,
        events: &[Event],
        now: u64,
    ) -> Result<bool, Broken> {
        if self.lost == 0 {
            let fit = events.len().min(sink.free());
            for e in &events[..fit] {
                if !sink.push(&e.encode()) {
                    return Err(Broken);
                }
            }
            self.lost = (events.len() - fit).min(u32::MAX as usize) as u32;
            return Ok(fit > 0);
        }
        self.lost = self.lost.saturating_add(events.len().min(u32::MAX as usize) as u32);
        let mut catching_up = Vec::new();
        catching_up.push(Event {
            kind: kind::LOST,
            value: self.lost.min(i32::MAX as u32) as i32,
            time: now,
            ..Event::default()
        });
        seat.held(now, &mut catching_up);
        if sink.free() < catching_up.len() {
            return Ok(false);
        }
        for e in &catching_up {
            if !sink.push(&e.encode()) {
                return Err(Broken);
            }
        }
        self.lost = 0;
        Ok(true)
    }

    /// Tries the catching-up again when nothing new has happened (the consumer may have made
    /// room meanwhile). Does nothing when nothing is owed.
    pub fn retry(&mut self, sink: &mut impl Sink, seat: &Seat, now: u64) -> Result<bool, Broken> {
        if self.lost == 0 { Ok(false) } else { self.deliver(sink, seat, &[], now) }
    }
}
