// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Endpoints, calls, replies and message delivery (docs/design/0008-messages-and-interfaces.md,
//! sections 1 to 5).
//!
//! Nothing is queued in the kernel except the threads themselves: a call is handed straight to
//! a waiting receiver, or the caller waits on the endpoint until one comes. Whichever side
//! arrives second does the delivery, copying from the sender's message page (and attached
//! buffer) into the receiver's.
//!
//! Each program thread's part in a message exchange is its [`Phase`], guarded by its own lock.
//! Lock order: an endpoint's lock, then a thread's phase, then a reply's. A [`Reply`] that still
//! has its caller must never be dropped while that caller's phase is locked (its drop tells the
//! caller): take the caller out of it first.
//!
//! Interface version 1.2 (docs/design/0013-kernel-1.2.md) adds two things here:
//! - **watched badges.** A badge is a number a service stamps on the capabilities it hands out,
//!   so it can tell its clients apart (note 0008). A [`Watch`] counts the capabilities that
//!   carry one badge. When the count reaches zero the record joins the endpoint's *gone queue*,
//!   and a receive returns it ([`Received::Gone`]), so the service can free what it kept for
//!   that badge (an open file, say);
//! - **derived replies.** A reply may ask for a capability derived from the one the caller
//!   called through ([`DERIVED`]): a child of it in the derivation tree, so that revoking the
//!   caller's capability revokes what it was used to reach.

use super::budget::Charge;
use super::caps;
use super::notification::Notification;
use super::table::Entry;
use super::{Error, Object, charge, require, rights};
use crate::mem::space::{Space, perm};
use crate::sched::{self, Thread, Woken};
use crate::sync::SpinLock;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use pbr_captree::NodeId;

/// A reply's capability entry with this value asks for a capability derived from the one the
/// caller used (note 0013, section 2). Bit 31 is set, which no handle has.
pub const DERIVED: u32 = 0xFFFF_FFFF;

/// Bytes of data a message can carry in its page.
pub const DATA_MAX: usize = 240;
/// The most capabilities in one message.
pub const CAPS_MAX: usize = 4;
/// The largest attached buffer.
pub const BUFFER_MAX: u32 = 65536;
/// Bytes of the message page the kernel reads and writes.
pub const PAGE_USED: usize = 64 + DATA_MAX;

/// What a successful receive got.
#[derive(Clone, Copy, Debug)]
pub enum Received {
    /// A call: the message is in the message page; this is the reply handle.
    Message(u32),
    /// The bound notification's bits.
    Bits(u64),
    /// Every capability carrying this watched badge has gone (note 0013, section 1).
    Gone(u64),
}

/// Where a program thread is in a message exchange.
pub enum Phase {
    /// Not in one.
    Idle,
    /// Waiting on an endpoint for a receiver.
    Calling,
    /// A receiver has taken the call and is copying it.
    Delivering,
    /// The call was delivered; waiting for the reply.
    AwaitingReply(Weak<Reply>),
    /// The call is over.
    Replied(Result<(), Error>),
    /// Waiting on an endpoint for a call.
    Receiving,
    /// A caller has taken this receiver and is copying a call to it.
    Taken,
    /// The receive is over.
    Received(Result<Received, Error>),
}

struct Caller {
    thread: Arc<Thread>,
    badge: u64,
    reply: Arc<Reply>,
}

/// The capability a caller called through: where a derived reply's new capability goes in
/// the derivation tree.
pub struct CallerCap {
    /// The calling process.
    pub process: Weak<super::process::Process>,
    /// The handle it called through.
    pub handle: u32,
    /// That capability's node, so a closed and reused handle is never mistaken for it.
    pub node: NodeId,
}

/// A watched badge's record (note 0013, section 1): how many capabilities carry the badge.
/// Every capability carrying it holds an `Arc` to this; the count is kept separately, by the
/// table hooks in `caps` (an `Arc` is also held briefly by lookups, which aren't capabilities).
pub struct Watch {
    endpoint: Weak<Endpoint>,
    badge: u64,
    count: AtomicU64,
    /// What the record costs whoever minted it, given back when its notice is received.
    charge: SpinLock<Option<Charge>>,
}

impl Watch {
    /// A record for `badge` on `endpoint`, with no capabilities counted yet.
    pub fn new(endpoint: &Arc<Endpoint>, badge: u64, charge: Charge) -> Arc<Watch> {
        Arc::new(Watch {
            endpoint: Arc::downgrade(endpoint),
            badge,
            count: AtomicU64::new(0),
            charge: SpinLock::new(Some(charge)),
        })
    }

    /// A capability carrying the badge was put in a table.
    pub fn cap_added(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// A capability carrying the badge left a table. The last one queues the gone notice.
    /// (The count can't rise again from zero: copies are only made from capabilities that
    /// exist, while the derivation tree's lock is held.)
    pub fn cap_removed(self: &Arc<Self>) {
        if self.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            match self.endpoint.upgrade() {
                Some(ep) => ep.badge_gone(self.clone()),
                None => drop(self.charge.lock().take()),
            }
        }
    }
}

struct Inner {
    callers: VecDeque<Caller>,
    receivers: VecDeque<Arc<Thread>>,
    bound: Option<Arc<Notification>>,
    senders: u64,
    receiver_caps: u64,
    watch: Option<(Arc<Notification>, u64)>,
    /// Watched badges whose last capability has gone, waiting to be received.
    gone: VecDeque<Arc<Watch>>,
}

/// A service's receiving point for messages.
pub struct Endpoint {
    inner: SpinLock<Inner>,
    _charge: Option<Charge>,
}

/// A one-time capability to answer one call.
pub struct Reply {
    caller: SpinLock<Option<Arc<Thread>>>,
    /// The capability the caller used, for a derived reply.
    through: CallerCap,
}

fn phase_of(t: &Thread) -> &SpinLock<Phase> {
    &t.user.as_ref().expect("a program thread").ipc
}

/// Where a program thread is in a call or receive, for reports. Not for interrupt handlers:
/// it takes the thread's phase lock.
pub fn phase_name(t: &Thread) -> &'static str {
    let Some(u) = t.user.as_ref() else { return "a kernel thread" };
    match &*u.ipc.lock() {
        Phase::Idle => "not in a call",
        Phase::Calling => "calling, not yet received",
        Phase::Delivering => "calling, being delivered",
        Phase::AwaitingReply(_) => "calling, awaiting the reply",
        Phase::Replied(_) => "calling, replied to",
        Phase::Receiving => "receiving",
        Phase::Taken => "receiving, a call being copied in",
        Phase::Received(_) => "receiving, received",
    }
}

fn set_phase(t: &Thread, p: Phase) {
    *phase_of(t).lock() = p;
}

impl Reply {
    fn new(caller: Arc<Thread>, through: CallerCap) -> Arc<Reply> {
        Arc::new(Reply { caller: SpinLock::new(Some(caller)), through })
    }

    /// Answers the call with the replying thread's message page.
    pub fn answer(&self, from: &Arc<Thread>) -> Result<(), Error> {
        let caller = self.caller.lock().take().ok_or(Error::PeerGone)?;
        let r = deliver(from, &caller, 0, None, Some(&self.through)).map(|_| ());
        set_phase(&caller, Phase::Replied(r.map_err(|_| Error::PeerGone)));
        sched::wake(&caller);
        r
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        // Closed, or its holder ended, without answering: the caller is told.
        if let Some(c) = self.caller.get_mut().take() {
            set_phase(&c, Phase::Replied(Err(Error::PeerGone)));
            sched::wake(&c);
        }
    }
}

impl Endpoint {
    /// A new endpoint.
    pub fn new(charge: Option<Charge>) -> Arc<Self> {
        Arc::new(Endpoint {
            inner: SpinLock::new(Inner {
                callers: VecDeque::new(),
                receivers: VecDeque::new(),
                bound: None,
                senders: 0,
                receiver_caps: 0,
                watch: None,
                gone: VecDeque::new(),
            }),
            _charge: charge,
        })
    }

    /// A capability with `rights` to this endpoint was created somewhere.
    pub fn cap_added(&self, r: u32) {
        let mut i = self.inner.lock();
        if r & rights::SEND != 0 {
            i.senders += 1;
        }
        if r & rights::RECEIVE != 0 {
            i.receiver_caps += 1;
        }
    }

    /// A capability with `rights` to this endpoint was destroyed or closed.
    pub fn cap_removed(&self, r: u32) {
        let mut signal = None;
        let mut gone = Vec::new();
        let mut notices = VecDeque::new();
        let waiting: Vec<Arc<Thread>>;
        {
            let mut i = self.inner.lock();
            if r & rights::SEND != 0 {
                i.senders -= 1;
                if i.senders == 0 {
                    signal = i.watch.clone();
                }
            }
            if r & rights::RECEIVE != 0 {
                i.receiver_caps -= 1;
                if i.receiver_caps == 0 {
                    // Nobody can ever receive: every waiting caller fails, and queued gone
                    // notices are dropped (their charges given back, below).
                    notices = core::mem::take(&mut i.gone);
                    gone = i.callers.drain(..).collect();
                    for c in &gone {
                        set_phase(&c.thread, Phase::Replied(Err(Error::PeerGone)));
                        c.reply.caller.lock().take();
                    }
                }
            }
            // Everyone waiting looks again, in case it was their capability that went.
            waiting = i
                .callers
                .iter()
                .map(|c| c.thread.clone())
                .chain(i.receivers.iter().cloned())
                .collect();
        }
        for c in &gone {
            sched::wake(&c.thread);
        }
        for t in &waiting {
            sched::wake(t);
        }
        if let Some((n, bits)) = signal {
            n.signal(bits);
        }
        for w in notices {
            drop(w.charge.lock().take());
        }
    }

    /// The last capability carrying a watched badge has gone: queue the notice for a receiver.
    fn badge_gone(&self, w: Arc<Watch>) {
        let waiting: Vec<Arc<Thread>> = {
            let mut i = self.inner.lock();
            if i.receiver_caps == 0 {
                drop(i);
                drop(w.charge.lock().take());
                return;
            }
            i.gone.push_back(w);
            i.receivers.iter().cloned().collect()
        };
        for t in &waiting {
            sched::wake(t);
        }
    }

    /// Whether a gone notice is waiting.
    fn gone_pending(&self) -> bool {
        !self.inner.lock().gone.is_empty()
    }

    /// Asks for `bits` on `n` whenever the last capability with the send right goes.
    pub fn watch(&self, n: Arc<Notification>, bits: u64) {
        let fire = {
            let mut i = self.inner.lock();
            i.watch = Some((n.clone(), bits));
            i.senders == 0
        };
        if fire {
            n.signal(bits);
        }
    }

    /// Binds `n`, so receiving here also returns its bits.
    pub fn bind(self: &Arc<Self>, n: Arc<Notification>) -> Result<(), Error> {
        let mut i = self.inner.lock();
        if i.bound.is_some() {
            return Err(Error::AlreadyExists);
        }
        n.bind(self)?;
        i.bound = Some(n);
        Ok(())
    }

    /// Wakes every thread receiving here (the bound notification was signalled).
    pub fn wake_receivers(&self) {
        let r: Vec<Arc<Thread>> = self.inner.lock().receivers.iter().cloned().collect();
        for t in &r {
            sched::wake(t);
        }
    }

    /// Calls: delivers the calling thread's message to a receiver and waits for the reply.
    /// `still_held` says whether the caller's capability still exists.
    pub fn call(
        &self,
        me: &Arc<Thread>,
        badge: u64,
        deadline: u64,
        through: CallerCap,
        still_held: impl Fn() -> bool,
    ) -> Result<(), Error> {
        read_header(me)?; // reject a malformed message before anyone sees it
        let reply = Reply::new(me.clone(), through);
        let weak = Arc::downgrade(&reply);
        {
            let mut i = self.inner.lock();
            if i.receiver_caps == 0 {
                return Err(Error::PeerGone);
            }
            if let Some(r) = i.receivers.pop_front() {
                set_phase(&r, Phase::Taken);
                drop(i);
                match deliver(me, &r, badge, Some(reply.clone()), None) {
                    Ok(h) => {
                        set_phase(me, Phase::AwaitingReply(weak.clone()));
                        drop(reply); // the receiver's table holds it now
                        set_phase(&r, Phase::Received(Ok(Received::Message(h))));
                        sched::wake(&r);
                    }
                    Err(e) => {
                        // The receiver carries on waiting; the caller gets the error.
                        reply.caller.lock().take();
                        let mut i = self.inner.lock();
                        set_phase(&r, Phase::Receiving);
                        i.receivers.push_front(r);
                        return Err(e);
                    }
                }
            } else {
                if !still_held() {
                    return Err(Error::Revoked);
                }
                set_phase(me, Phase::Calling);
                let at = i
                    .callers
                    .iter()
                    .position(|c| c.thread.priority < me.priority)
                    .unwrap_or(i.callers.len());
                i.callers.insert(at, Caller { thread: me.clone(), badge, reply });
                let waiting: Vec<Arc<Thread>> = i.receivers.iter().cloned().collect();
                drop(i);
                for t in &waiting {
                    sched::wake(t);
                }
            }
        }
        // Wait for the reply. While the other side is in the middle of copying (our call to
        // a receiver, or its reply to us), the deadline is set aside: that finishes promptly.
        let mut answering = false;
        loop {
            let busy = answering || matches!(*phase_of(me).lock(), Phase::Delivering);
            let dl = if busy { u64::MAX } else { deadline };
            let woken = sched::wait_event(dl, || {
                matches!(*phase_of(me).lock(), Phase::Replied(_)) || !still_held()
            });
            let mut i = self.inner.lock();
            let mut p = phase_of(me).lock();
            match &*p {
                Phase::Replied(r) => {
                    let r = *r;
                    *p = Phase::Idle;
                    return r;
                }
                Phase::Calling => {
                    // Still queued: give up, for whichever reason woke us.
                    let Some(why) = giving_up(woken, still_held()) else { continue };
                    let mut mine = Vec::new();
                    i.callers.retain(|c| {
                        let me_too = Arc::ptr_eq(&c.thread, me);
                        if me_too {
                            mine.push(c.reply.clone());
                        }
                        !me_too
                    });
                    for r in &mine {
                        r.caller.lock().take();
                    }
                    *p = Phase::Idle;
                    drop(p);
                    drop(i);
                    return Err(why);
                }
                Phase::AwaitingReply(w) if !answering => {
                    let Some(why) = giving_up(woken, still_held()) else { continue };
                    let abandoned = match w.upgrade() {
                        Some(rep) => rep.caller.lock().take().is_some(),
                        None => false,
                    };
                    if abandoned {
                        *p = Phase::Idle;
                        return Err(why);
                    }
                    answering = true; // the service is answering right now
                }
                _ => {}
            }
            drop(p);
            drop(i);
            if woken == Woken::Stopped {
                sched::yield_now(); // let the other side finish
            }
        }
    }

    /// Receives: waits for a call (or the bound notification's bits).
    pub fn receive(
        &self,
        me: &Arc<Thread>,
        deadline: u64,
        still_held: impl Fn() -> bool,
    ) -> Result<Received, Error> {
        let bound = self.inner.lock().bound.clone();
        loop {
            if let Some(n) = &bound {
                let bits = n.take();
                if bits != 0 {
                    return Ok(Received::Bits(bits));
                }
            }
            let mut i = self.inner.lock();
            if let Some(w) = i.gone.pop_front() {
                drop(i);
                drop(w.charge.lock().take()); // the record's cost goes back to its minter
                return Ok(Received::Gone(w.badge));
            }
            if let Some(c) = i.callers.pop_front() {
                set_phase(&c.thread, Phase::Delivering);
                drop(i);
                let weak = Arc::downgrade(&c.reply);
                match deliver(&c.thread, me, c.badge, Some(c.reply.clone()), None) {
                    Ok(h) => {
                        set_phase(&c.thread, Phase::AwaitingReply(weak));
                        drop(c.reply); // our table holds it now
                        sched::wake(&c.thread);
                        return Ok(Received::Message(h));
                    }
                    Err(e) => {
                        // The caller gets the error; this receiver carries on.
                        c.reply.caller.lock().take();
                        set_phase(&c.thread, Phase::Replied(Err(e)));
                        sched::wake(&c.thread);
                        continue;
                    }
                }
            }
            if !still_held() {
                return Err(Error::Revoked);
            }
            set_phase(me, Phase::Receiving);
            i.receivers.push_back(me.clone());
            drop(i);
            loop {
                let busy = matches!(*phase_of(me).lock(), Phase::Taken);
                let dl = if busy { u64::MAX } else { deadline };
                let woken = sched::wait_event(dl, || {
                    matches!(*phase_of(me).lock(), Phase::Received(_))
                        || bound.as_ref().is_some_and(|n| n.pending())
                        || self.gone_pending()
                        || !still_held()
                });
                let mut i = self.inner.lock();
                let mut p = phase_of(me).lock();
                match &*p {
                    Phase::Received(r) => {
                        let r = *r;
                        *p = Phase::Idle;
                        return r;
                    }
                    Phase::Receiving => {
                        let bits =
                            bound.as_ref().is_some_and(|n| n.pending()) || !i.gone.is_empty();
                        let why = if bits { None } else { giving_up(woken, still_held()) };
                        if why.is_none() && !bits {
                            continue;
                        }
                        i.receivers.retain(|t| !Arc::ptr_eq(t, me));
                        *p = Phase::Idle;
                        match why {
                            Some(e) => return Err(e),
                            None => break, // take the bits or the notice at the top
                        }
                    }
                    _ => {} // a caller is copying to us: wait for it
                }
                drop(p);
                drop(i);
                if woken == Woken::Stopped {
                    sched::yield_now();
                }
            }
        }
    }
}

/// Whether a waiting thread should give up, and with what error.
fn giving_up(woken: Woken, held: bool) -> Option<Error> {
    if !held {
        Some(Error::Revoked)
    } else {
        match woken {
            Woken::TimedOut => Some(Error::TimedOut),
            Woken::Stopped => Some(Error::BadState),
            Woken::Ready => None,
        }
    }
}

// ------------------------------------------------------------------------------------------
// The message page (note 0008, section 3) and delivery (section 4).

struct Header {
    label: u32,
    data_len: usize,
    cap_count: usize,
    cap_move: u8,
    caps: [u32; 4],
    masks: [u32; 4],
    buf_addr: u64,
    buf_len: u32,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

/// Reads and checks a thread's outgoing message header.
fn read_header(t: &Thread) -> Result<(Header, [u8; PAGE_USED]), Error> {
    let u = t.user.as_ref().ok_or(Error::BadState)?;
    let mut page = [0u8; PAGE_USED];
    u.msg.read(0, &mut page)?;
    let h = Header {
        label: u32_at(&page, 0),
        data_len: u16::from_le_bytes([page[4], page[5]]) as usize,
        cap_count: page[6] as usize,
        cap_move: page[7],
        caps: core::array::from_fn(|i| u32_at(&page, 8 + 4 * i)),
        masks: core::array::from_fn(|i| u32_at(&page, 24 + 4 * i)),
        buf_addr: u64_at(&page, 48),
        buf_len: u32_at(&page, 56),
    };
    if h.data_len > DATA_MAX || h.cap_count > CAPS_MAX || h.cap_move >> h.cap_count != 0 {
        return Err(Error::InvalidArgument);
    }
    if h.buf_len > BUFFER_MAX {
        return Err(Error::TooBig);
    }
    Ok((h, page))
}

/// Delivers `src`'s message to `dst`: all of it, or nothing. With `reply`, the receiver also
/// gets a reply capability; its handle is returned (0 without one). `through` is given only
/// for a reply: the capability the caller (`dst`) called through, from which a [`DERIVED`]
/// entry makes the new capability.
pub fn deliver(
    src: &Arc<Thread>,
    dst: &Arc<Thread>,
    badge: u64,
    reply: Option<Arc<Reply>>,
    through: Option<&CallerCap>,
) -> Result<u32, Error> {
    let su = src.user.as_ref().ok_or(Error::BadState)?;
    let du = dst.user.as_ref().ok_or(Error::BadState)?;
    let (h, page) = read_header(src)?;
    let mut dhead = [0u8; 64];
    du.msg.read(0, &mut dhead)?;
    let dst_buf = u64_at(&dhead, 48);
    let capacity = u32_at(&dhead, 60).min(BUFFER_MAX);
    if h.buf_len > capacity {
        return Err(Error::TooBig);
    }
    let (sp, dp) = (&su.process, &du.process);
    let mut tree = caps::lock();

    // Check the capabilities to send, and that there's room for them.
    let mut entries: Vec<Entry> = Vec::new();
    let mut derived: Option<(usize, Derived)> = None;
    for i in 0..h.cap_count {
        if h.caps[..i].contains(&h.caps[i]) {
            return Err(Error::InvalidArgument); // also refuses two derived entries
        }
        if h.caps[i] == DERIVED {
            // Only a reply can derive, and only from a caller that still holds its capability.
            let t = through.ok_or(Error::InvalidArgument)?;
            derived = Some((i, Derived::check(t, dp, u64_at(&page, 40), h.masks[i])?));
            continue;
        }
        let e = caps::lookup(sp, h.caps[i])?;
        require(e.rights, rights::TRANSFER)?;
        if h.cap_move & (1 << i) == 0 {
            require(e.rights, rights::DUPLICATE)?;
        }
        entries.push(e);
    }
    dp.table.lock().reserve(h.cap_count + usize::from(reply.is_some()))?;

    // The attached buffer, straight from one program's memory to the other's.
    let mut buf_len = 0;
    if h.buf_len > 0 {
        let len = u64::from(h.buf_len);
        let copy = |a: &Space, b: &Space| -> Result<(), Error> {
            a.check(h.buf_addr, len, perm::READ)?;
            b.check(dst_buf, len, perm::WRITE)?;
            Space::copy_between(a, h.buf_addr, b, dst_buf, len)
        };
        if Arc::ptr_eq(sp, dp) {
            let s = sp.space.lock();
            copy(&s, &s)?;
        } else if Arc::as_ptr(sp) < Arc::as_ptr(dp) {
            let a = sp.space.lock();
            let b = dp.space.lock();
            copy(&a, &b)?;
        } else {
            let b = dp.space.lock();
            let a = sp.space.lock();
            copy(&a, &b)?;
        }
        buf_len = h.buf_len;
    }

    // The capabilities.
    let mut out_caps = [0u32; 4];
    let mut out_rights = [0u32; 4];
    for i in 0..h.cap_count {
        let (nh, r) = match &derived {
            Some((at, d)) if *at == i => {
                // The record is charged to whoever replies: the service that chose to derive.
                let watch =
                    Watch::new(&d.endpoint, d.badge, sp.budget.charge(charge::NOTIFICATION)?);
                let object = Object::Endpoint(d.endpoint.clone());
                let nh = caps::insert_entry(
                    &mut tree,
                    dp,
                    object,
                    d.rights,
                    d.badge,
                    Some(watch),
                    Some(d.node),
                )?;
                (nh, d.rights)
            }
            _ => {
                let moving = h.cap_move & (1 << i) != 0;
                caps::give(&mut tree, sp, h.caps[i], dp, h.masks[i], moving)?
            }
        };
        out_caps[i] = nh;
        out_rights[i] = r;
    }
    let reply_handle = match reply {
        Some(r) => caps::insert(&mut tree, dp, Object::Reply(r), 0, 0, None)?,
        None => 0,
    };
    drop(tree);
    let _ = entries;

    // The header and data.
    let mut out = [0u8; PAGE_USED];
    out[0..4].copy_from_slice(&h.label.to_le_bytes());
    out[4..6].copy_from_slice(&(h.data_len as u16).to_le_bytes());
    out[6] = h.cap_count as u8;
    out[7] = h.cap_move;
    for i in 0..4 {
        out[8 + 4 * i..12 + 4 * i].copy_from_slice(&out_caps[i].to_le_bytes());
        out[24 + 4 * i..28 + 4 * i].copy_from_slice(&out_rights[i].to_le_bytes());
    }
    out[40..48].copy_from_slice(&badge.to_le_bytes());
    out[48..56].copy_from_slice(&dst_buf.to_le_bytes());
    out[56..60].copy_from_slice(&buf_len.to_le_bytes());
    out[60..64].copy_from_slice(&capacity.to_le_bytes());
    out[64..64 + h.data_len].copy_from_slice(&page[64..64 + h.data_len]);
    du.msg.write(0, &out)?;
    Ok(reply_handle)
}

/// A derived capability about to be made (note 0013, section 2).
struct Derived {
    endpoint: Arc<Endpoint>,
    node: NodeId,
    rights: u32,
    badge: u64,
}

impl Derived {
    /// Checks a derived entry for a reply to `caller`, who called through `t`: the caller still
    /// holds that capability (the tree's lock is held, so it can't go before it's used), and the
    /// badge isn't 0 ("unbadged", which a client could mint from).
    fn check(
        t: &CallerCap,
        caller: &Arc<super::process::Process>,
        badge: u64,
        mask: u32,
    ) -> Result<Derived, Error> {
        if badge == 0 {
            return Err(Error::InvalidArgument);
        }
        let same = t.process.upgrade().is_some_and(|p| Arc::ptr_eq(&p, caller));
        let e = if same { caps::lookup(caller, t.handle).ok() } else { None };
        // A capability that has gone (closed, revoked, or its handle reused) can't be derived
        // from: the caller's call has failed or is failing, so the reply goes nowhere.
        let Some(e) = e.filter(|e| e.node == t.node) else { return Err(Error::PeerGone) };
        let Object::Endpoint(ep) = &e.object else { return Err(Error::PeerGone) };
        Ok(Derived { endpoint: ep.clone(), node: e.node, rights: e.rights & mask, badge })
    }
}
