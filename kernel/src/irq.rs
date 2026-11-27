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
//!
//! **Shared lines** (note 0027, section 2). A PCI device without message-signalled interrupts
//! raises its interrupt by holding a wire low until its driver has dealt with it
//! ("level-triggered"), and several devices' wires may be joined into one line. The kernel
//! can't tell which device it was, so every driver bound to the line is signalled, each
//! binding is marked as **owed** an acknowledgement, and the line is unmasked only when none
//! is owed: unmasking earlier would let it fire again at once. Edge-triggered lines and
//! message-signalled interrupts are never shared.

use crate::obj::notification::Notification;
use crate::obj::{Error, device};
use crate::sched::{self, Thread};
use crate::sync::{Level, Once, SpinLock};
use crate::{arch, kprintln};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
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
    /// Signalled of an interrupt it hasn't acknowledged yet.
    pub owed: bool,
}

/// Every bound interrupt's bindings: one, or more on a shared line, never none.
static BINDINGS: SpinLock<BTreeMap<u32, Vec<Binding>>> =
    SpinLock::new(Level::InterruptBindings, "the interrupt bindings", BTreeMap::new());

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
    // Who to signal, gathered under the lock and signalled after it: signalling takes the
    // notifications' own locks.
    let mut targets = Vec::new();
    if let Some(list) = BINDINGS.lock().get_mut(&id) {
        for b in list {
            b.owed = true;
            targets.push((b.notification.clone(), b.bits));
        }
    }
    match targets.is_empty() {
        false => {
            DELIVERED.fetch_add(1, Ordering::Relaxed);
            for (n, bits) in targets {
                n.signal(bits);
            }
        }
        true => {
            let (w, bit) = (id as usize / 64, 1u64 << (id % 64));
            if REPORTED[w].fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                kprintln!("Interrupt {id} fired with nobody bound; it stays masked.");
            }
        }
    }
}

/// Binds interrupt `id` as `binding` says, and unmasks it. A level-triggered line already
/// bound by another process is shared; a second binding from the same process never is.
pub fn bind(id: u32, binding: Binding) -> Result<(), Error> {
    let mut table = BINDINGS.lock();
    if let Some(list) = table.get_mut(&id) {
        if list.iter().any(|b| b.process == binding.process) || !arch::irqchip::shareable(id) {
            return Err(Error::AlreadyExists);
        }
        // The line is already set up, and unmasked unless it's waiting for the others'
        // acknowledgements, in which case this binding hears of the next interrupt.
        list.push(Binding { owed: false, ..binding });
        return Ok(());
    }
    arch::irqchip::attach(id)?;
    table.insert(id, alloc::vec![Binding { owed: false, ..binding }]);
    arch::irqchip::unmask(id);
    Ok(())
}

/// Removes the bindings `gone` picks from `id`'s list, and returns them. If none remain, the
/// line is masked and its vector given back, so a program that ends mid-interrupt can't leave
/// a storm behind; if others remain and none is owed, it is unmasked, so they aren't held up
/// by one that ended without acknowledging.
fn remove(
    table: &mut BTreeMap<u32, Vec<Binding>>,
    id: u32,
    mut gone: impl FnMut(&Binding) -> bool,
) -> Vec<Binding> {
    let Some(list) = table.get_mut(&id) else { return Vec::new() };
    let (out, keep): (Vec<Binding>, Vec<Binding>) = list.drain(..).partition(|b| gone(b));
    *list = keep;
    if out.is_empty() {
        return out;
    }
    if list.is_empty() {
        table.remove(&id);
        arch::irqchip::detach(id);
    } else if list.iter().all(|b| !b.owed) {
        arch::irqchip::unmask(id);
    }
    out
}

/// Releases `process`'s binding of `id`. Returns the binding's node.
pub fn unbind(id: u32, process: u64) -> Result<NodeId, Error> {
    let mut table = BINDINGS.lock();
    let gone = remove(&mut table, id, |b| b.process == process);
    // The bindings (and the vector they keep) go after the lock is released.
    drop(table);
    gone.first().map(|b| b.node).ok_or(Error::BadState)
}

/// Releases the binding of `id` whose node is `node` (the node was revoked).
pub fn release_node(id: u32, node: NodeId) {
    let mut table = BINDINGS.lock();
    let gone = remove(&mut table, id, |b| b.node == node);
    drop(table);
    drop(gone);
}

/// Releases every binding `process` made, and returns their nodes (for the caller to remove
/// from the derivation tree).
pub fn release_process(process: u64) -> Vec<NodeId> {
    let mut table = BINDINGS.lock();
    let ids: Vec<u32> = table
        .iter()
        .filter(|(_, list)| list.iter().any(|b| b.process == process))
        .map(|(&i, _)| i)
        .collect();
    let mut gone = Vec::new();
    for id in ids {
        gone.extend(remove(&mut table, id, |b| b.process == process));
    }
    drop(table);
    gone.into_iter().map(|b| b.node).collect()
}

/// `process` has handled interrupt `id`: its binding is no longer owed, and once no binding
/// on the line is, the line is unmasked. `BAD_STATE` if `process` hasn't bound `id`; when
/// nothing is owed, it does nothing harmful.
pub fn acknowledge(id: u32, process: u64) -> Result<(), Error> {
    let mut table = BINDINGS.lock();
    let list = table.get_mut(&id).ok_or(Error::BadState)?;
    let mine = list.iter_mut().find(|b| b.process == process).ok_or(Error::BadState)?;
    mine.owed = false;
    if list.iter().all(|b| !b.owed) {
        arch::irqchip::unmask(id);
    }
    Ok(())
}

/// Sets line `id`'s trigger and polarity (`interrupt_configure`, note 0028, section 8): `mode`
/// bit 0 level-triggered, bit 1 active low. Only while nobody has the line bound, so everyone
/// sharing it agrees.
pub fn configure(id: u32, mode: u64) -> Result<(), Error> {
    if mode & !3 != 0 {
        return Err(Error::InvalidArgument);
    }
    // The bindings' lock is held throughout, so nobody can bind the line half-way.
    let table = BINDINGS.lock();
    if table.contains_key(&id) {
        return Err(Error::BadState);
    }
    arch::irqchip::configure(id, mode & 1 != 0, mode & 2 != 0)
}

/// Makes `id` pending by software (self-test runs only). Returns whether the line was masked
/// at the controller, waiting for acknowledgements: then nothing is triggered, as a real
/// device's interrupt wouldn't get through either until it is unmasked.
pub fn trigger(id: u32) -> Result<bool, Error> {
    if !SELFTEST.load(Ordering::Relaxed) {
        return Err(Error::NotSupported);
    }
    let table = BINDINGS.lock();
    let list = table.get(&id).ok_or(Error::BadState)?;
    if list.iter().any(|b| b.owed) {
        return Ok(true);
    }
    arch::irqchip::trigger(id)?;
    Ok(false)
}

/// True if `id` is bound (tests).
#[allow(dead_code)]
pub fn is_bound(id: u32) -> bool {
    BINDINGS.lock().contains_key(&id)
}
