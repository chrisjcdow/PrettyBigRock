// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Delivering device interrupts to programs (docs/design/0010-device-access.md, section 4).
//!
//! A program binds an interrupt to one of its notifications. When the interrupt fires, the
//! architecture's handler masks it at the controller, acknowledges it and calls [`fired`],
//! which only records the interrupt number and wakes the `irq` kernel thread, because
//! signalling a notification takes ordinary spin locks that aren't safe in an interrupt
//! handler. The `irq` thread then signals the notification. The driver handles its device and
//! calls `interrupt_acknowledge` to unmask the interrupt again.

use crate::obj::notification::Notification;
use crate::obj::{Error, device};
use crate::sched::{self, Thread};
use crate::sync::{Once, SpinLock};
use crate::{arch, kprintln};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use pbr_captree::NodeId;

const WORDS: usize = device::MSI_END as usize / 64;

/// Interrupts that have fired and haven't been signalled yet, one bit per interrupt number.
static FIRED: [AtomicU64; WORDS] = [const { AtomicU64::new(0) }; WORDS];
/// Interrupts already reported as firing with nobody bound (reported once each).
static REPORTED: [AtomicU64; WORDS] = [const { AtomicU64::new(0) }; WORDS];
static THREAD: Once<Arc<Thread>> = Once::new();
static SELFTEST: AtomicBool = AtomicBool::new(false);
/// Interrupts delivered so far, for tests.
pub static DELIVERED: AtomicU64 = AtomicU64::new(0);

/// A bound interrupt.
pub struct Binding {
    /// The process that made it (it's released when that process ends).
    pub process: u64,
    /// What to signal.
    pub notification: Arc<Notification>,
    /// With which bits.
    pub bits: u64,
    /// The hidden node in the derivation tree that revocation destroys.
    pub node: NodeId,
    /// Keeps a message-signalled vector from being reused while it's bound.
    pub _object: Arc<device::Interrupts>,
}

static BINDINGS: SpinLock<BTreeMap<u32, Binding>> = SpinLock::new(BTreeMap::new());

/// Starts the delivery thread. `selftest` allows [`trigger`].
pub fn start(selftest: bool) {
    SELFTEST.store(selftest, Ordering::Relaxed);
    THREAD.set(sched::spawn("irq", (sched::PRIORITIES - 1) as u8, run));
}

/// Called by an architecture's interrupt handler, after it has masked the interrupt (if it's a
/// level-triggered line) and told the controller it's been taken. Takes no locks.
pub fn fired(id: u32) {
    if id >= device::MSI_END {
        return;
    }
    FIRED[id as usize / 64].fetch_or(1 << (id % 64), Ordering::SeqCst);
    if let Some(t) = THREAD.get() {
        sched::wake(t);
    }
}

fn any_fired() -> bool {
    FIRED.iter().any(|w| w.load(Ordering::SeqCst) != 0)
}

fn run() {
    loop {
        sched::wait_event(u64::MAX, any_fired);
        for (i, word) in FIRED.iter().enumerate() {
            let mut bits = word.swap(0, Ordering::SeqCst);
            while bits != 0 {
                let b = bits.trailing_zeros();
                bits &= bits - 1;
                deliver(i as u32 * 64 + b);
            }
        }
    }
}

fn deliver(id: u32) {
    let target = BINDINGS.lock().get(&id).map(|b| (b.notification.clone(), b.bits));
    match target {
        Some((n, bits)) => {
            DELIVERED.fetch_add(1, Ordering::Relaxed);
            n.signal(bits);
        }
        None => {
            let (w, bit) = (id as usize / 64, 1u64 << (id % 64));
            if REPORTED[w].fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                kprintln!("Interrupt {id} fired with nobody bound; it stays masked.");
            }
        }
    }
}

/// Binds interrupt `id` as `binding` says, and unmasks it.
pub fn bind(id: u32, binding: Binding) -> Result<(), Error> {
    let mut table = BINDINGS.lock();
    if table.contains_key(&id) {
        return Err(Error::AlreadyExists);
    }
    arch::irqchip::attach(id)?;
    table.insert(id, binding);
    arch::irqchip::unmask(id);
    Ok(())
}

/// Releases the binding of `id`, if `process` made it. Masks a line interrupt, so that a
/// program that ends mid-interrupt can't leave a storm behind. Returns the binding's node.
pub fn unbind(id: u32, process: Option<u64>) -> Result<NodeId, Error> {
    let mut table = BINDINGS.lock();
    let b = table.get(&id).ok_or(Error::BadState)?;
    if process.is_some_and(|p| p != b.process) {
        return Err(Error::NoRight);
    }
    let b = table.remove(&id).expect("found just now");
    arch::irqchip::detach(id);
    // The binding (and the vector it keeps) goes after the lock is released.
    drop(table);
    let node = b.node;
    drop(b);
    Ok(node)
}

/// Releases the binding of `id` if its node is `node` (the node was revoked).
pub fn release_node(id: u32, node: NodeId) {
    let mut table = BINDINGS.lock();
    if table.get(&id).is_some_and(|b| b.node == node)
        && let Some(b) = table.remove(&id)
    {
        arch::irqchip::detach(id);
        drop(table);
        drop(b);
    }
}

/// Releases every binding `process` made, and returns their nodes (for the caller to remove
/// from the derivation tree).
pub fn release_process(process: u64) -> alloc::vec::Vec<NodeId> {
    let mut table = BINDINGS.lock();
    let ids: alloc::vec::Vec<u32> =
        table.iter().filter(|(_, b)| b.process == process).map(|(&i, _)| i).collect();
    let mut gone = alloc::vec::Vec::new();
    for id in ids {
        if let Some(b) = table.remove(&id) {
            arch::irqchip::detach(id);
            gone.push(b);
        }
    }
    drop(table);
    gone.into_iter().map(|b| b.node).collect()
}

/// Unmasks `id` after the driver has handled it. Does nothing if it isn't bound.
pub fn acknowledge(id: u32) {
    if BINDINGS.lock().contains_key(&id) {
        arch::irqchip::unmask(id);
    }
}

/// Makes `id` pending by software (self-test runs only).
pub fn trigger(id: u32) -> Result<(), Error> {
    if !SELFTEST.load(Ordering::Relaxed) {
        return Err(Error::NotSupported);
    }
    if !BINDINGS.lock().contains_key(&id) {
        return Err(Error::BadState);
    }
    arch::irqchip::trigger(id)
}

/// True if `id` is bound (tests).
#[allow(dead_code)]
pub fn is_bound(id: u32) -> bool {
    BINDINGS.lock().contains_key(&id)
}
