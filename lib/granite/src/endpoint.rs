// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Endpoints, calls and replies (docs/design/0008-messages-and-interfaces.md).
//!
//! A service creates an [`Endpoint`], gives clients badged copies with [`Endpoint::mint`], and
//! answers with [`Endpoint::receive`] and [`ReplyCap::reply`]. A client calls with
//! [`Endpoint::call`].

use crate::handle::Handle;
use crate::message::{self, Incoming, Outgoing};
use crate::notification::Notification;
use crate::sys::{self, Error, Result, num};

/// An endpoint capability.
#[derive(Debug)]
pub struct Endpoint(pub Handle);

/// What a receive got.
#[derive(Debug)]
pub enum Event {
    /// A call, and the capability to answer it.
    Call(Incoming, ReplyCap),
    /// The bound notification's bits.
    Signal(u64),
}

/// A one-time capability to answer a call. Dropping it without replying tells the caller the
/// service has gone (`PeerGone`).
#[derive(Debug)]
pub struct ReplyCap(Handle);

fn page() -> *mut u8 {
    crate::thread::message_page()
}

impl Endpoint {
    /// A new endpoint, with every right.
    pub fn create() -> Result<Endpoint> {
        sys::call(num::ENDPOINT_CREATE, [0; 6]).map(|(h, _)| Endpoint(Handle::from_raw(h as u32)))
    }

    /// A copy carrying `badge`, with only the rights in `mask` (only from an unbadged one).
    pub fn mint(&self, badge: u64, mask: u32) -> Result<Endpoint> {
        sys::call(num::ENDPOINT_MINT, [self.0.raw().into(), badge, mask.into(), 0, 0, 0])
            .map(|(h, _)| Endpoint(Handle::from_raw(h as u32)))
    }

    /// Calls the endpoint and waits for the reply. The first `send` bytes of `buffer` go with
    /// the request; the reply may fill `buffer` (up to its length) with its own.
    pub fn call(
        &self,
        m: &Outgoing<'_>,
        buffer: &mut [u8],
        send: usize,
        deadline: u64,
    ) -> Result<Incoming> {
        if send > buffer.len() {
            return Err(Error::InvalidArgument);
        }
        let p = page();
        message::write(p, m, buffer.as_ptr(), send, buffer.len())?;
        sys::call(num::ENDPOINT_CALL, [self.0.raw().into(), deadline, 0, 0, 0, 0])?;
        Ok(message::read(p))
    }

    /// Waits for a call (or the bound notification's bits). An attached buffer arrives in
    /// `buffer`; a call with a bigger one never arrives here.
    pub fn receive(&self, buffer: &mut [u8], deadline: u64) -> Result<Event> {
        let p = page();
        message::set_receive_buffer(p, buffer.as_mut_ptr(), buffer.len());
        let (kind, value) =
            sys::call(num::ENDPOINT_RECEIVE, [self.0.raw().into(), deadline, 0, 0, 0, 0])?;
        Ok(event(p, kind, value))
    }

    /// Ties `n` to this endpoint, so receiving here also returns its bits.
    pub fn bind(&self, n: &Notification) -> Result<()> {
        sys::call(num::ENDPOINT_BIND, [self.0.raw().into(), n.0.raw().into(), 0, 0, 0, 0])
            .map(|_| ())
    }

    /// Asks for `bits` on `n` whenever the last capability with the send right goes.
    pub fn watch(&self, n: &Notification, bits: u64) -> Result<()> {
        sys::call(num::ENDPOINT_WATCH, [self.0.raw().into(), n.0.raw().into(), bits, 0, 0, 0])
            .map(|_| ())
    }
}

fn event(p: *mut u8, kind: u64, value: u64) -> Event {
    if kind & 0xFFFF_FFFF == 2 {
        Event::Signal(value)
    } else {
        Event::Call(message::read(p), ReplyCap(Handle::from_raw(value as u32)))
    }
}

impl ReplyCap {
    /// Answers the call with `m`, and the bytes of `buffer` as its attached buffer.
    pub fn reply(self, m: &Outgoing<'_>, buffer: &[u8]) -> Result<()> {
        let p = page();
        message::write(p, m, buffer.as_ptr(), buffer.len(), 0)?;
        // The kernel uses up the reply capability whatever happens.
        let h = self.0.into_raw();
        sys::call(num::ENDPOINT_REPLY, [h.into(), 0, 0, 0, 0, 0]).map(|_| ())
    }

    /// Answers with `m`, then waits for the next call on `ep` in the same system call: a
    /// service's main loop. A failed reply doesn't stop the receive; its error is returned
    /// alongside.
    pub fn reply_receive(
        self,
        m: &Outgoing<'_>,
        out: &[u8],
        ep: &Endpoint,
        buffer: &mut [u8],
        deadline: u64,
    ) -> (Result<()>, Result<Event>) {
        let p = page();
        if let Err(e) = message::write(p, m, out.as_ptr(), out.len(), 0) {
            drop(self);
            return (Err(e), ep.receive(buffer, deadline));
        }
        let h = self.0.into_raw();
        // The outgoing buffer fields are read for the reply before the receive reuses them,
        // so the receive buffer can only be set afterwards: use a separate receive when one
        // is wanted.
        if !buffer.is_empty() {
            let r = sys::call(num::ENDPOINT_REPLY, [h.into(), 0, 0, 0, 0, 0]).map(|_| ());
            return (r, ep.receive(buffer, deadline));
        }
        message::set_receive_buffer(p, buffer.as_mut_ptr(), 0);
        let (s, kind, value) =
            sys::raw(num::ENDPOINT_REPLY_RECEIVE, [h.into(), ep.0.raw().into(), deadline, 0, 0, 0]);
        let replied = match kind >> 32 {
            0 => Ok(()),
            e => Err(Error::from_code(e)),
        };
        if s != 0 {
            return (replied, Err(Error::from_code(s)));
        }
        (replied, Ok(event(p, kind, value)))
    }
}
