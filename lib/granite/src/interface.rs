// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What code generated from interface files uses (docs/design/0008-messages-and-interfaces.md,
//! sections 7 to 9): encoding and checking messages, and the errors a call can end in.
//!
//! [`Encoder`] lays a message out: its fixed part (at most 240 bytes, in the message page) and
//! its variable part (strings, bytes and lists, in the attached buffer). [`Decoder`] reads one
//! back and checks everything as it goes; anything wrong is [`Malformed`], never a panic.

use crate::endpoint::{Endpoint, ReplyCap};
use crate::handle::Handle;
use crate::message::{CapSend, Incoming, Outgoing};
use crate::sys::Error;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::fmt;

/// A reply's status: success; the results follow.
pub const STATUS_OK: u32 = 0;
/// A reply's status: the operation's own error; its value (a `u32`) follows.
pub const STATUS_ERROR: u32 = 1;
/// A reply's status: no such version or operation.
pub const STATUS_UNKNOWN: u32 = 0xFFFF_0001;
/// A reply's status: the request was malformed.
pub const STATUS_MALFORMED: u32 = 0xFFFF_0002;
/// A reply's status: the service failed internally.
pub const STATUS_FAILED: u32 = 0xFFFF_0003;

/// The bytes of a fixed part.
pub const FIXED_MAX: usize = crate::message::DATA_MAX;
/// The bytes of a variable part.
pub const VARIABLE_MAX: usize = crate::message::BUFFER_MAX;

/// A message that broke the encoding's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

/// Why a call through a generated client failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError<E> {
    /// The operation's own error.
    Service(E),
    /// The call itself failed (see [`Error`]).
    Call(Error),
    /// The service doesn't know this version or operation.
    Unknown,
    /// The service said the request was malformed.
    Rejected,
    /// The service failed internally.
    Failed,
    /// The reply broke the encoding's rules.
    Malformed,
}

impl<E: fmt::Debug> fmt::Display for CallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Service(e) => write!(f, "{e:?}"),
            CallError::Call(e) => write!(f, "{e}"),
            CallError::Unknown => f.write_str("the service doesn't know this operation"),
            CallError::Rejected => f.write_str("the service rejected the request as malformed"),
            CallError::Failed => f.write_str("the service failed"),
            CallError::Malformed => f.write_str("the reply was malformed"),
        }
    }
}

impl<E> From<Error> for CallError<E> {
    fn from(e: Error) -> Self {
        CallError::Call(e)
    }
}

impl<E> From<Malformed> for CallError<E> {
    fn from(_: Malformed) -> Self {
        CallError::Malformed
    }
}

/// A laid-out message: its fixed part, its variable part, and the capabilities it moves.
pub type Encoded = (Vec<u8>, Vec<u8>, Vec<CapSend>);

/// Lays out one message.
pub struct Encoder {
    fixed: Vec<u8>,
    var: Vec<u8>,
    caps: Vec<CapSend>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    /// An empty message.
    pub fn new() -> Self {
        Encoder { fixed: Vec::new(), var: Vec::new(), caps: Vec::new() }
    }

    fn pad(&mut self, align: usize) {
        while !self.fixed.len().is_multiple_of(align) {
            self.fixed.push(0);
        }
    }

    /// Writes an unsigned integer of `size` bytes, aligned to its size.
    pub fn uint(&mut self, v: u64, size: usize) {
        self.pad(size);
        self.fixed.extend_from_slice(&v.to_le_bytes()[..size]);
    }

    /// Writes a signed integer of `size` bytes.
    pub fn int(&mut self, v: i64, size: usize) {
        self.uint(v as u64, size);
    }

    /// Writes a bool.
    pub fn bool(&mut self, v: bool) {
        self.fixed.push(u8::from(v));
    }

    /// Aligns the fixed part to `align` (a record's or an option's value).
    pub fn align(&mut self, align: usize) {
        self.pad(align);
    }

    /// Writes `n` zero bytes (an absent option's value).
    pub fn zeros(&mut self, n: usize) {
        self.fixed.extend(core::iter::repeat_n(0, n));
    }

    fn reference(&mut self, offset: usize, len: usize) {
        self.uint(offset as u64, 4);
        self.uint(len as u64, 4);
    }

    /// Writes bytes into the variable part, and a reference to them into the fixed part.
    pub fn bytes(&mut self, b: &[u8]) {
        while !self.var.len().is_multiple_of(8) {
            self.var.push(0);
        }
        let at = self.var.len();
        self.var.extend_from_slice(b);
        self.reference(at, b.len());
    }

    /// Writes a string (as its UTF-8 bytes).
    pub fn string(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    /// Writes a list: `elem_size` bytes per element, each laid out by `each` like a fixed
    /// part. The list's block comes first in the variable part; whatever its elements refer
    /// to follows it.
    pub fn list<T>(&mut self, items: &[T], elem_size: usize, each: impl Fn(&mut Encoder, &T)) {
        while !self.var.len().is_multiple_of(8) {
            self.var.push(0);
        }
        let at = self.var.len();
        let len = items.len() * elem_size;
        self.var.extend(core::iter::repeat_n(0, len));
        let mut sub =
            Encoder { fixed: Vec::new(), var: core::mem::take(&mut self.var), caps: Vec::new() };
        for it in items {
            let start = sub.fixed.len();
            each(&mut sub, it);
            sub.zeros(start + elem_size - sub.fixed.len());
        }
        self.var = sub.var;
        self.var[at..at + len].copy_from_slice(&sub.fixed);
        self.reference(at, len);
    }

    /// Moves a capability into the message, and writes its index.
    pub fn handle(&mut self, h: Handle) {
        let i = self.caps.len() as u64;
        self.caps.push(CapSend::moved(h, u32::MAX));
        self.uint(i, 4);
    }

    /// The fixed part, padded to `size` bytes.
    pub fn finish(mut self, size: usize) -> Encoded {
        self.zeros(size.saturating_sub(self.fixed.len()));
        (self.fixed, self.var, self.caps)
    }
}

/// Reads one message, checking every rule.
pub struct Decoder<'a> {
    fixed: &'a [u8],
    pos: usize,
    var: &'a [u8],
    caps: [Option<Handle>; 4],
    used: [bool; 4],
    count: usize,
}

impl<'a> Decoder<'a> {
    /// A decoder over a fixed part, a variable part, and the capabilities that came with them.
    pub fn new(fixed: &'a [u8], var: &'a [u8], caps: [Option<Handle>; 4]) -> Self {
        let count = caps.iter().filter(|c| c.is_some()).count();
        Decoder { fixed, pos: 0, var, caps, used: [false; 4], count }
    }

    /// A decoder over a received message's data and attached buffer, taking its capabilities.
    pub fn incoming(msg: &'a mut Incoming, buffer: &'a [u8]) -> (u32, Self) {
        let label = msg.label;
        let mut caps: [Option<Handle>; 4] = [None, None, None, None];
        for (i, c) in caps.iter_mut().enumerate() {
            *c = msg.take_cap(i);
        }
        let n = msg.buffer_len.min(buffer.len());
        let msg: &'a Incoming = msg;
        (label, Decoder::new(msg.data(), &buffer[..n], caps))
    }

    /// Checks the fixed part is exactly `size` bytes.
    pub fn expect(&self, size: usize) -> Result<(), Malformed> {
        if self.fixed.len() == size { Ok(()) } else { Err(Malformed) }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        let end = self.pos.checked_add(n).ok_or(Malformed)?;
        let s = self.fixed.get(self.pos..end).ok_or(Malformed)?;
        self.pos = end;
        Ok(s)
    }

    /// Skips to a multiple of `align`; the skipped bytes must be zero.
    pub fn align(&mut self, align: usize) -> Result<(), Malformed> {
        while !self.pos.is_multiple_of(align) {
            if self.take(1)?[0] != 0 {
                return Err(Malformed);
            }
        }
        Ok(())
    }

    /// Reads an unsigned integer of `size` bytes.
    pub fn uint(&mut self, size: usize) -> Result<u64, Malformed> {
        self.align(size)?;
        let b = self.take(size)?;
        let mut v = [0u8; 8];
        v[..size].copy_from_slice(b);
        Ok(u64::from_le_bytes(v))
    }

    /// Reads a signed integer of `size` bytes.
    pub fn int(&mut self, size: usize) -> Result<i64, Malformed> {
        let v = self.uint(size)?;
        let shift = 64 - 8 * size as u32;
        Ok(((v << shift) as i64) >> shift)
    }

    /// Reads a bool: 0 or 1 only.
    pub fn bool(&mut self) -> Result<bool, Malformed> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Malformed),
        }
    }

    /// Reads an option's tag: 0 or 1 only.
    pub fn tag(&mut self) -> Result<bool, Malformed> {
        self.bool()
    }

    /// Checks `n` bytes are zero (an absent option's value).
    pub fn zeros(&mut self, n: usize) -> Result<(), Malformed> {
        if self.take(n)?.iter().any(|&b| b != 0) { Err(Malformed) } else { Ok(()) }
    }

    fn reference(&mut self, max: usize) -> Result<&'a [u8], Malformed> {
        let offset = self.uint(4)? as usize;
        let len = self.uint(4)? as usize;
        if len > max {
            return Err(Malformed);
        }
        let end = offset.checked_add(len).ok_or(Malformed)?;
        self.var.get(offset..end).ok_or(Malformed)
    }

    /// Reads bytes of at most `max` length.
    pub fn bytes(&mut self, max: usize) -> Result<Vec<u8>, Malformed> {
        Ok(self.reference(max)?.to_vec())
    }

    /// Reads a string of at most `max` bytes, which must be valid UTF-8.
    pub fn string(&mut self, max: usize) -> Result<String, Malformed> {
        let b = self.reference(max)?;
        core::str::from_utf8(b).map(String::from).map_err(|_| Malformed)
    }

    /// Reads a list of at most `max` elements of `elem_size` bytes, each read by `each`.
    pub fn list<T>(
        &mut self,
        max: usize,
        elem_size: usize,
        each: impl Fn(&mut Decoder<'a>) -> Result<T, Malformed>,
    ) -> Result<Vec<T>, Malformed> {
        let block = self.reference(max.checked_mul(elem_size).ok_or(Malformed)?)?;
        if elem_size == 0 || !block.len().is_multiple_of(elem_size) {
            return Err(Malformed);
        }
        let mut out = Vec::with_capacity(block.len() / elem_size);
        for chunk in block.chunks(elem_size) {
            let mut sub = Decoder {
                fixed: chunk,
                pos: 0,
                var: self.var,
                caps: [None, None, None, None],
                used: [false; 4],
                count: 0,
            };
            out.push(each(&mut sub)?);
            sub.end()?;
        }
        Ok(out)
    }

    /// Reads a capability's index and takes the capability (each only once).
    pub fn handle(&mut self) -> Result<Handle, Malformed> {
        let i = self.uint(4)? as usize;
        if i >= 4 || self.used[i] {
            return Err(Malformed);
        }
        self.used[i] = true;
        self.caps[i].take().ok_or(Malformed)
    }

    /// Checks the whole fixed part was read (its trailing padding must be zero).
    pub fn end(&mut self) -> Result<(), Malformed> {
        let rest = self.fixed.len() - self.pos.min(self.fixed.len());
        self.zeros(rest)
    }

    /// Checks the whole message was read: every byte of the fixed part, and every capability.
    pub fn finish(mut self) -> Result<(), Malformed> {
        self.end()?;
        if self.used.iter().filter(|u| **u).count() != self.count {
            return Err(Malformed);
        }
        Ok(())
    }
}

/// Closes the capabilities a message would have moved, after it failed to go. (If it did
/// arrive after all, the handles no longer name anything: closing them does nothing.)
fn close_moved(caps: &[CapSend]) {
    for c in caps {
        if c.moving {
            drop(Handle::from_raw(c.handle));
        }
    }
}

/// Sends a request and waits for the reply, which arrives with its attached buffer (of up to
/// `reply_buffer` bytes). The capabilities in the request are moved; if it fails they're
/// closed.
pub fn call(
    ep: &Endpoint,
    label: u32,
    m: Encoded,
    reply_buffer: usize,
    deadline: u64,
) -> Result<(Incoming, Vec<u8>), Error> {
    let (fixed, var, caps) = m;
    let mut buffer = vec![0u8; var.len().max(reply_buffer)];
    buffer[..var.len()].copy_from_slice(&var);
    let out = Outgoing { label, data: &fixed, caps: &caps };
    match ep.call(&out, &mut buffer, var.len(), deadline) {
        Ok(r) => Ok((r, buffer)),
        Err(e) => {
            close_moved(&caps);
            Err(e)
        }
    }
}

/// Reads a reply: on success decodes its results with `decode`; for the operation's error,
/// reads its value with `error`. Anything else is the matching [`CallError`].
pub fn reply<T, E>(
    mut r: Incoming,
    buffer: &[u8],
    decode: impl FnOnce(Decoder<'_>) -> Result<T, Malformed>,
    error: impl FnOnce(u32) -> Option<E>,
) -> Result<T, CallError<E>> {
    match r.label {
        STATUS_OK => Ok(decode(Decoder::incoming(&mut r, buffer).1)?),
        STATUS_ERROR => {
            let (_, mut d) = Decoder::incoming(&mut r, buffer);
            d.expect(4)?;
            let v = d.uint(4)? as u32;
            d.finish()?;
            Err(CallError::Service(error(v).ok_or(Malformed)?))
        }
        STATUS_UNKNOWN => Err(CallError::Unknown),
        STATUS_MALFORMED => Err(CallError::Rejected),
        STATUS_FAILED => Err(CallError::Failed),
        _ => Err(CallError::Malformed),
    }
}

/// For an operation without errors: no value is one.
pub fn no_error(_: u32) -> Option<Infallible> {
    None
}

/// Answers a call with `status` and a laid-out message. If the answer can't be delivered, the
/// capabilities it would have moved are closed.
pub fn answer(reply: ReplyCap, status: u32, m: Encoded) -> Result<(), Error> {
    let (fixed, var, caps) = m;
    let out = Outgoing { label: status, data: &fixed, caps: &caps };
    reply.reply(&out, &var).inspect_err(|_| close_moved(&caps))
}

/// Answers a call with the operation's error `value`.
pub fn answer_error(reply: ReplyCap, value: u32) -> Result<(), Error> {
    reply.reply(&Outgoing::new(STATUS_ERROR, &value.to_le_bytes()), &[])
}

/// Answers a call with just a status.
pub fn answer_status(reply: ReplyCap, status: u32) -> Result<(), Error> {
    reply.reply(&Outgoing::new(status, &[]), &[])
}

/// Answers that a request was malformed. (If the caller has gone, there's no one to tell.)
pub fn reject(reply: ReplyCap) {
    let _ = answer_status(reply, STATUS_MALFORMED);
}

/// Answers that the version or operation is unknown.
pub fn unknown(reply: ReplyCap) {
    let _ = answer_status(reply, STATUS_UNKNOWN);
}

/// Checks a capability that arrived is of the right kind (and, for an endpoint, can be sent
/// to). Only on PrettyBigRock: elsewhere there's no kernel to ask.
pub fn check_kind(h: &Handle, kind: u32, endpoint: bool) -> Result<(), Malformed> {
    #[cfg(target_os = "prettybigrock")]
    {
        let (k, r) = h.info().map_err(|_| Malformed)?;
        if k != kind || (endpoint && r & crate::sys::rights::SEND == 0) {
            return Err(Malformed);
        }
    }
    let _ = (h, kind, endpoint);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_strings_and_lists_round_trip() {
        let mut e = Encoder::new();
        e.uint(7, 1);
        e.uint(0x1234_5678_9ABC, 8);
        e.int(-5, 2);
        e.bool(true);
        e.string("héllo");
        e.list(&["a", "bc"], 8, |e, s| e.string(s));
        let (fixed, var, caps) = e.finish(48);
        assert_eq!(fixed.len(), 48);
        assert!(caps.is_empty());
        let mut d = Decoder::new(&fixed, &var, [None, None, None, None]);
        assert_eq!(d.uint(1), Ok(7));
        assert_eq!(d.uint(8), Ok(0x1234_5678_9ABC));
        assert_eq!(d.int(2), Ok(-5));
        assert_eq!(d.bool(), Ok(true));
        assert_eq!(d.string(16).as_deref(), Ok("héllo"));
        assert_eq!(d.list(4, 8, |d| d.string(4)), Ok(alloc::vec!["a".into(), "bc".into()]));
        assert_eq!(d.finish(), Ok(()));
    }

    #[test]
    fn rejects_what_breaks_the_rules() {
        let mut e = Encoder::new();
        e.string("abc");
        let (fixed, var, _) = e.finish(8);
        // Too long for its declared maximum.
        assert_eq!(Decoder::new(&fixed, &var, Default::default()).string(2), Err(Malformed));
        // Pointing outside the buffer.
        assert_eq!(Decoder::new(&fixed, &var[..2], Default::default()).string(8), Err(Malformed));
        // Not UTF-8.
        assert_eq!(
            Decoder::new(&fixed, &[0xFF, 0xFE, 0xFD], Default::default()).string(8),
            Err(Malformed)
        );
        // A bool that isn't 0 or 1, non-zero padding, trailing junk.
        assert_eq!(Decoder::new(&[2], &[], Default::default()).bool(), Err(Malformed));
        let mut d = Decoder::new(&[1, 9, 0, 0, 5, 0, 0, 0], &[], Default::default());
        assert_eq!(d.bool(), Ok(true));
        assert_eq!(d.uint(4), Err(Malformed));
        let mut d = Decoder::new(&[1, 0, 0, 7], &[], Default::default());
        assert_eq!(d.bool(), Ok(true));
        assert_eq!(d.finish(), Err(Malformed));
        // A list whose length isn't a whole number of elements.
        let mut fixed = alloc::vec![0u8; 8];
        fixed[4] = 3;
        assert_eq!(
            Decoder::new(&fixed, &[0; 8], Default::default()).list(4, 2, |d| d.uint(2)),
            Err(Malformed)
        );
        // A handle index used twice, or never sent.
        let mut d = Decoder::new(&[0, 0, 0, 0, 0, 0, 0, 0], &[], Default::default());
        assert_eq!(d.handle().err(), Some(Malformed));
    }

    /// Random bytes never panic the decoder.
    #[test]
    fn random_messages_never_panic() {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            let mut fixed = [0u8; 24];
            let mut var = [0u8; 64];
            for b in fixed.iter_mut().chain(var.iter_mut()) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = (x >> 32) as u8 & if x & 1 == 0 { 0x0F } else { 0xFF };
            }
            let mut d = Decoder::new(&fixed, &var, Default::default());
            let _ = d.bool();
            let _ = d.string(16);
            let _ = d.list(8, 4, |d| d.int(4));
            let _ = d.finish();
        }
    }
}
