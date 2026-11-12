// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding the keyboards and pointers, and keeping them found
//! (docs/design/0021-input-and-keyboard-layouts.md, sections 1 and 8).
//!
//! The input service owns no hardware. The device manager starts a driver program for each
//! device (note 0011); this module asks it, through `pbr.devices`, for every device of class
//! input, opens each with `pbr.input` (which takes the device over from whoever had it) and
//! reads the driver's ring of events. It finds the device manager through its supervisor, the
//! root program (`pbr.supervisor.lookup`, note 0018, section 2).
//!
//! A driver is an ordinary program and can crash and be started again, and so can the device
//! manager. Every capability held for it then stops working: calls fail with `PeerGone` (the
//! program is gone), `Revoked` or `BadHandle`. That is not a reason to give up: the device is
//! let go of (the main thread tells the consumer, so that no key stays held), looked for again
//! once a second, and opened again when its new driver is running.
//!
//! The threads, all of which only *tell the main thread* what they find, through the
//! [`Shared`] queue and a notification bound to the service's endpoint (a bound notification
//! wakes the main thread's `receive` as a call would, so it never has to poll):
//! - a **watcher** ([`Devices::watch`]) that looks at the device list at start-up and then
//!   once a second;
//! - a **reader** for each open device ([`Reader`]), which sleeps until the device's doorbell
//!   rings and passes its events on.
//!
//! This is the console's M5 arrangement (note 0018, section 5), which read keyboards itself
//! before there was an input service; the console now gets its keys from here.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::lock::Lock;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::ring::{Consumer, RingError};
use granite::sys::perm;
use granite::thread::{self, JoinHandle};
use granite::{Error, time};
use pbr_interfaces::pbr_devices::{
    self as devices, DeviceClass, DeviceInfo, DeviceState, DevicesError,
};
use pbr_interfaces::pbr_input::{self as input, InputKind};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_seat::Source;
use pbr_seat::seat::RAW_SIZE;

/// How long one call to the device manager or a driver may take. A driver that is not
/// answering must not stop the input service for ever.
const CALL_MS: u64 = 5_000;
/// How often the watcher looks for what is new or has come back, and how often an idle reader
/// checks that its device is still there.
const CHECK_MS: u64 = 1_000;
/// How many times opening one device may fail before it is left alone.
const MAX_TRIES: u32 = 5;
/// How many messages may wait for the main thread. If it is stuck for long, readers drop
/// events and say so (as a driver does when its own ring fills).
const MAX_QUEUED: usize = 4096;
/// The signal bit that means "look at the queue".
pub const WAKE: u64 = 1;

/// What the other threads tell the main thread. `key` is this module's own number for an open
/// device, new each time one is opened.
pub enum Message {
    /// A device was opened.
    Added {
        key: u32,
        source: Source,
        /// Its driver's name for it.
        name: String,
        /// The device manager's number for it.
        device: u32,
    },
    /// An event from a device's ring, as the driver wrote it (16 bytes, `pbr.input`).
    Raw { key: u32, event: [u8; RAW_SIZE] },
    /// A device has gone: its driver ended, or the device manager did.
    Gone { key: u32 },
}

/// What the threads share: the queue of messages for the main thread.
///
/// Rust only lets a value be shared between threads in ways that cannot go wrong: an `Arc<T>`
/// is a pointer to a `T` that several threads own together, freed when the last owner lets
/// go; a [`Lock`] lets one thread at a time use the value inside it; and an `AtomicBool` is a
/// flag that threads can read and change without a lock, one indivisible step at a time.
pub struct Shared {
    queue: Lock<VecDeque<Message>>,
    /// Signalled when the queue has something in it. The main thread binds it to its endpoint.
    wake: Notification,
}

impl Shared {
    /// An empty queue and a notification to wake the main thread with.
    pub fn new() -> Result<Arc<Shared>, String> {
        let wake = Notification::create().map_err(|e| format!("making a notification: {e}"))?;
        Ok(Arc::new(Shared { queue: Lock::new(VecDeque::new()), wake }))
    }

    /// The notification that wakes the main thread.
    pub fn wake(&self) -> &Notification {
        &self.wake
    }

    /// Adds a message for the main thread and wakes it. Returns `false`, and adds nothing, if
    /// the message is an event and the queue is full. (That a device came or went always goes
    /// in: there are few of those, and losing one would leave the seat wrong for good.)
    fn push(&self, message: Message) -> bool {
        {
            let mut queue = self.queue.lock();
            if matches!(message, Message::Raw { .. }) && queue.len() >= MAX_QUEUED {
                return false;
            }
            queue.push_back(message);
        }
        // If this fails the main thread is already gone, and nobody needs waking.
        let _ = self.wake.signal(WAKE);
        true
    }

    /// Takes everything queued so far, in order.
    pub fn take(&self) -> VecDeque<Message> {
        core::mem::take(&mut *self.queue.lock())
    }
}

/// Whether a call failed because what it called has gone, so that it may be found again.
fn lost<E>(e: &CallError<E>) -> bool {
    matches!(e, CallError::Call(Error::PeerGone | Error::Revoked | Error::BadHandle))
}

/// What a failed call to a driver means for opening its device.
fn trouble<E: core::fmt::Debug>(what: &str, e: CallError<E>) -> Trouble {
    if lost(&e) { Trouble::Later } else { Trouble::Failed(format!("{what}: {e}")) }
}

/// What went wrong with opening a device.
enum Trouble {
    /// It is not ready, or has gone: try again at the next look.
    Later,
    /// It cannot be used, for this reason.
    Failed(String),
}

/// Flags shared with a reader thread.
#[derive(Default)]
struct Flags {
    /// Asked of the thread by the watcher: end when you next wake.
    stop: AtomicBool,
    /// Set by the thread as it ends, for any reason.
    done: AtomicBool,
}

/// A device that is open.
struct Open {
    /// The device manager's number for it.
    id: u32,
    /// This module's number for it.
    key: u32,
    flags: Arc<Flags>,
    thread: JoinHandle,
}

/// The device manager and everything found through it.
pub struct Devices {
    supervisor: supervisor::Client,
    devmgr: Option<devices::Client>,
    open: Vec<Open>,
    /// Devices of a device manager that has gone, whose readers are ending.
    retiring: Vec<Open>,
    /// Devices to leave alone until the device manager changes: ones that failed to open too
    /// often.
    ignored: Vec<u32>,
    /// How many times each device has failed to open.
    tries: Vec<(u32, u32)>,
    next_key: u32,
    shared: Arc<Shared>,
    /// The last thing reported as wrong, so that the same thing is not reported every second.
    problem: Option<String>,
}

impl Devices {
    /// Nothing found yet. `supervisor` is where the device manager is looked up.
    pub fn new(supervisor: supervisor::Client, shared: Arc<Shared>) -> Devices {
        Devices {
            supervisor,
            devmgr: None,
            open: Vec::new(),
            retiring: Vec::new(),
            ignored: Vec::new(),
            tries: Vec::new(),
            next_key: 1,
            shared,
            problem: None,
        }
    }

    /// The watcher thread's work, for ever: notice what has gone, and open what is running
    /// and not open yet. The first look happens at once; a device whose driver is still
    /// starting is found by a later one, and the consumer hears of it then (a "devices
    /// changed" event), so nothing waits for anything at start-up.
    ///
    /// It all happens on this thread because a thread's handle cannot be passed to another
    /// thread: the readers started here are joined here.
    pub fn watch(mut self) {
        loop {
            self.reap();
            self.look();
            time::sleep_ms(CHECK_MS);
        }
    }

    /// Prints `problem` unless it was the last thing printed.
    fn report(&mut self, problem: String) {
        if self.problem.as_ref() != Some(&problem) {
            granite::println!("input: {problem}");
            self.problem = Some(problem);
        }
    }

    /// Looks the device manager up through the supervisor. While it is starting or being
    /// started again the answer is "not ready", and the next look asks again.
    fn connect(&mut self) -> bool {
        match self.supervisor.lookup("devices") {
            Ok(h) => {
                let mut client = devices::Client::new(Endpoint(h));
                client.set_timeout_ms(CALL_MS);
                self.devmgr = Some(client);
                true
            }
            Err(CallError::Service(SupervisorError::NotReady)) => false,
            Err(e) => {
                self.report(format!("can't find the device manager: {e}"));
                false
            }
        }
    }

    /// The device manager has gone: its devices are gone with it, and the numbers it gave out
    /// mean something else to the next one. The main thread is told at once, without waiting
    /// for the readers to notice.
    fn devmgr_gone(&mut self) {
        granite::println!("input: the device manager has gone; looking for it again.");
        self.devmgr = None;
        self.ignored.clear();
        self.tries.clear();
        for open in &self.open {
            open.flags.stop.store(true, Ordering::Release);
            self.shared.push(Message::Gone { key: open.key });
        }
        self.retiring.append(&mut self.open);
    }

    /// Joins the readers that have ended and forgets their devices.
    fn reap(&mut self) {
        for list in [&mut self.open, &mut self.retiring] {
            let mut i = 0;
            while i < list.len() {
                if list[i].flags.done.load(Ordering::Acquire) {
                    let open = list.swap_remove(i);
                    // (The thread has finished, so this does not wait.)
                    let _ = open.thread.join();
                } else {
                    i += 1;
                }
            }
        }
    }

    /// Looks at the device list once and opens every input device that is running and not
    /// open.
    fn look(&mut self) {
        if self.devmgr.is_none() && !self.connect() {
            return;
        }
        let listed = match &self.devmgr {
            Some(client) => client.list(),
            None => return,
        };
        let list = match listed {
            Ok(list) => list,
            Err(e) => {
                if lost(&e) {
                    self.devmgr_gone();
                } else {
                    self.report(format!("can't list devices: {e}"));
                }
                return;
            }
        };
        self.problem = None;
        let inputs: Vec<&DeviceInfo> = list
            .iter()
            .filter(|d| d.kind == DeviceClass::Input && !self.ignored.contains(&d.id))
            .collect();
        for d in inputs {
            if self.open.iter().any(|o| o.id == d.id) || d.state != DeviceState::Running {
                continue;
            }
            match self.open_device(d) {
                Ok(()) | Err(Trouble::Later) => {}
                Err(Trouble::Failed(why)) => self.give_up(d.id, &d.name, &why),
            }
        }
    }

    /// A device failed to open for `why`; after a few failures it is left alone.
    fn give_up(&mut self, id: u32, name: &str, why: &str) {
        let tries = match self.tries.iter_mut().find(|t| t.0 == id) {
            Some(t) => {
                t.1 += 1;
                t.1
            }
            None => {
                self.tries.push((id, 1));
                1
            }
        };
        if tries >= MAX_TRIES {
            granite::println!("input: leaving {name} alone: {why}");
            self.ignored.push(id);
        } else {
            granite::println!("input: can't use {name}: {why}");
        }
    }

    /// An endpoint for device `id`'s driver, from the device manager.
    fn endpoint(&mut self, id: u32) -> Result<Endpoint, Trouble> {
        let opened = match &self.devmgr {
            Some(client) => client.open(id),
            None => return Err(Trouble::Later),
        };
        match opened {
            Ok(h) => Ok(Endpoint(h)),
            Err(CallError::Service(DevicesError::NotReady)) => Err(Trouble::Later),
            Err(e) if lost(&e) => {
                self.devmgr_gone();
                Err(Trouble::Later)
            }
            Err(e) => Err(Trouble::Failed(format!("the device manager: {e}"))),
        }
    }

    /// Opens input device `d` and starts a thread to read it.
    fn open_device(&mut self, d: &DeviceInfo) -> Result<(), Trouble> {
        let mut client = input::Client::new(self.endpoint(d.id)?);
        client.set_timeout_ms(CALL_MS);
        let info = client.info().map_err(|e| trouble("asking what it is", e))?;
        let source = match info.kind {
            InputKind::Keyboard => Source::Keyboard,
            InputKind::Mouse => Source::Mouse,
            InputKind::KeyboardAndMouse => Source::KeyboardAndMouse,
            InputKind::Tablet => Source::Tablet,
        };
        let opened = client.open().map_err(|e| trouble("opening it", e))?;
        let map = opened
            .ring
            .map(perm::READ_WRITE)
            .map_err(|e| Trouble::Failed(format!("mapping its ring: {e}")))?;
        // SAFETY: the mapping is readable and writable for its whole length and starts on a
        // page, which is aligned to 64 as a ring needs. It is kept in the same struct as the
        // consumer, so it lives as long as the consumer does.
        let consumer = unsafe { Consumer::new(map.as_mut_ptr(), map.len() as usize) }
            .map_err(|e| Trouble::Failed(format!("its ring is no good: {e:?}")))?;

        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1).max(1);
        let flags = Arc::new(Flags::default());
        let reader = Reader {
            key,
            name: String::from(info.name.trim()),
            client,
            consumer,
            doorbell: opened.doorbell,
            _map: map,
            _ring: opened.ring,
            flags: flags.clone(),
            shared: self.shared.clone(),
            dropped: false,
        };
        // The main thread hears of the device before any of its events: this message is
        // queued before the reader exists.
        // (Some drivers pass on a device's own name for itself, padding and all.)
        let name = String::from(info.name.trim());
        self.shared.push(Message::Added { key, source, name, device: d.id });
        // `move ||` makes a closure (a function with no name) that takes everything it uses
        // with it, which is how a new thread is given its device.
        match thread::spawn(move || reader.run()) {
            Ok(handle) => {
                self.open.push(Open { id: d.id, key, flags, thread: handle });
                Ok(())
            }
            Err(e) => {
                self.shared.push(Message::Gone { key });
                Err(Trouble::Failed(format!("starting a thread: {e}")))
            }
        }
    }
}

/// Reads one device's ring for as long as the device is there.
struct Reader {
    key: u32,
    name: String,
    /// Used only to ask, now and then, whether the driver is still there.
    client: input::Client,
    consumer: Consumer,
    /// Rung by the driver when the ring goes from empty to not empty.
    doorbell: Notification,
    /// The ring's memory, which the consumer reads: kept so that it stays mapped.
    _map: Mapping,
    _ring: Memory,
    flags: Arc<Flags>,
    shared: Arc<Shared>,
    /// Whether events were dropped because the main thread's queue was full.
    dropped: bool,
}

impl Reader {
    /// Runs until the device goes or the watcher asks it to stop.
    fn run(mut self) {
        let mut event = [0u8; RAW_SIZE];
        loop {
            match self.consumer.pop(&mut event) {
                Ok(_) => self.pass_on(event),
                // Nothing to read. The driver rings the doorbell when it adds something (the
                // `pop` that found the ring empty asked it to); until then sleep, but wake now
                // and then to check that the device is still there.
                Err(RingError::WouldBlock) => match self.doorbell.wait(time::after_ms(CHECK_MS)) {
                    Ok(_) => {}
                    Err(Error::TimedOut) => {
                        if self.flags.stop.load(Ordering::Acquire) || self.driver_gone() {
                            break;
                        }
                    }
                    // The doorbell itself stopped working: the driver is gone.
                    Err(_) => break,
                },
                // The ring is broken (the driver wrote nonsense, or the memory is wrong).
                Err(_) => break,
            }
        }
        if !self.flags.stop.load(Ordering::Acquire) {
            granite::println!("input: {} has gone.", self.name);
        }
        // (If the watcher already said so, the main thread ignores the second telling.)
        self.shared.push(Message::Gone { key: self.key });
        self.flags.done.store(true, Ordering::Release);
    }

    /// Queues an event for the main thread. If the queue was full and events were dropped,
    /// the next one that fits is preceded by a "lost" event, exactly as a driver reports its
    /// own losses, so the seat lets go of whatever was held on this device.
    fn pass_on(&mut self, event: [u8; RAW_SIZE]) {
        if self.dropped {
            // All zeros is `pbr.input`'s lost event (kind 0); nothing reads its count.
            if !self.shared.push(Message::Raw { key: self.key, event: [0; RAW_SIZE] }) {
                return;
            }
            self.dropped = false;
        }
        if !self.shared.push(Message::Raw { key: self.key, event }) {
            self.dropped = true;
        }
    }

    /// Whether the driver has gone, by asking it something.
    fn driver_gone(&self) -> bool {
        matches!(self.client.info(), Err(ref e) if lost(e))
    }
}
