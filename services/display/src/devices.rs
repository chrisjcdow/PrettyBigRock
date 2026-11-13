// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding the screens and the seat, and keeping them found.
//!
//! The display server owns no hardware: the device manager starts a driver for each display
//! (note 0011), and this program *takes* every display it finds through `pbr.display`, which
//! makes it the one client whose pictures are shown (note 0020). Keys and the pointer come from
//! the input service: the display server *takes the seat* (`pbr.seat`, note 0021), as the
//! console does in text mode. It finds both services through its supervisor, the root
//! program, whose lookup table is what decides that it may take the seat at all.
//!
//! Drivers and the input service can crash and be restarted. When that happens every
//! capability held for them stops working; they are simply found again.
//!
//! Several threads are involved, and all of them only *tell the main thread* what they find,
//! through [`Shared`]'s queue and a notification bound to the service's endpoint (a bound
//! notification wakes the main thread's `receive` as a call would, so it never polls):
//! - a **watcher** ([`Watcher::watch`]) that looks for displays and the seat once a second, or
//!   at once when the input service goes;
//! - a **seat thread** ([`SeatThread`]) that reads the seat's ring and passes its events on;
//! - a **done thread** for each display ([`DoneThread`]), waiting on the notification its driver
//!   signals when a picture is on a screen, and passing that on.
//!
//! The main thread owns everything it is given: the displays, once taken, are its alone, so
//! nothing that draws or presents needs a lock.

use alloc::boxed::Box;
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
use granite::{Error, thread, time};
use pbr_compositor::Tablet;
use pbr_interfaces::pbr_devices::{self as devices, DeviceClass, DeviceState, DevicesError};
use pbr_interfaces::pbr_display as display;
use pbr_interfaces::pbr_seat::{self as seat, DeviceKind, SeatError};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_seat::Event as SeatEvent;
use pbr_seat::event::{EVENT_SIZE, kind as seat_kind};

/// How long one call to the device manager, a driver or the input service may take.
pub const CALL_MS: u64 = 5_000;
/// How often the watcher looks, and how often an idle thread checks whether to stop.
const CHECK_MS: u64 = 1_000;
/// How many times opening a display may fail before it is left alone.
const MAX_TRIES: u32 = 5;
/// The most seat events waiting for the main thread; past that the newest are dropped (the
/// seat's own ring is where a slow consumer is meant to lose events, and say so).
const MAX_QUEUED: usize = 4096;
/// The signal bit that means "look at the queue".
pub const WAKE: u64 = 1;

/// A display, taken: what the main thread needs to use it.
pub struct Taken {
    /// The device manager's number for it.
    pub device: u32,
    /// The driver, through the capability that may only ask about the screens.
    pub client: display::Client,
    /// The driver, through the taker's capability: only requests through it are obeyed.
    pub taker: display::Client,
    /// Set by the main thread when it lets go of the display: its done thread ends.
    pub stop: Arc<AtomicBool>,
}

/// What the other threads tell the main thread.
pub enum Message {
    /// A display was taken. (Boxed: it is much bigger than the rest.)
    Display(Box<Taken>),
    /// Events from the seat, in order.
    Seat(Vec<SeatEvent>),
    /// The seat's tablets, in its order (after it was taken, and whenever its devices change).
    Tablets(Vec<Tablet>),
    /// A display's driver signalled its done notification: bit n for screen n's picture on
    /// the screen, bit 63 for its screens having changed.
    Done { device: u32, bits: u64 },
}

/// What the threads share.
///
/// `Arc<T>` is a pointer to a `T` that several threads own together; a [`Lock`] lets one
/// thread at a time use what is inside it.
pub struct Shared {
    queue: Lock<VecDeque<Message>>,
    wake: Notification,
    /// The displays the main thread holds, so the watcher doesn't take one twice.
    held: Lock<Vec<u32>>,
}

impl Shared {
    /// An empty queue, and a notification to wake the main thread with.
    pub fn new() -> Result<Arc<Shared>, String> {
        let wake = Notification::create().map_err(|e| format!("a notification: {e}"))?;
        Ok(Arc::new(Shared {
            queue: Lock::new(VecDeque::new()),
            wake,
            held: Lock::new(Vec::new()),
        }))
    }

    /// The notification that wakes the main thread.
    pub fn wake(&self) -> &Notification {
        &self.wake
    }

    fn push(&self, m: Message) {
        {
            let mut q = self.queue.lock();
            if matches!(m, Message::Seat(_)) && q.len() >= MAX_QUEUED {
                return;
            }
            q.push_back(m);
        }
        let _ = self.wake.signal(WAKE);
    }

    /// Everything queued so far, in order.
    pub fn take(&self) -> VecDeque<Message> {
        core::mem::take(&mut *self.queue.lock())
    }

    /// The main thread now holds display `device`.
    pub fn hold(&self, device: u32) {
        let mut h = self.held.lock();
        if !h.contains(&device) {
            h.push(device);
        }
    }

    /// The main thread let go of display `device` (its driver went): look for it again.
    pub fn release(&self, device: u32) {
        self.held.lock().retain(|d| *d != device);
    }

    fn holds(&self, device: u32) -> bool {
        self.held.lock().contains(&device)
    }
}

/// Whether a call failed because what it called has gone, so that it may be found again.
pub fn lost<E>(e: &CallError<E>) -> bool {
    matches!(e, CallError::Call(Error::PeerGone | Error::Revoked | Error::BadHandle))
}

/// The seat, while this program has it.
struct Seat {
    /// Through the capability the seat was taken with; the watch for the input service going
    /// is on it, so it is kept for as long as the seat is.
    _client: seat::Client,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

/// The watcher thread's state.
pub struct Watcher {
    supervisor: supervisor::Client,
    devmgr: Option<devices::Client>,
    seat: Option<Seat>,
    /// Signalled (bit 0) by the kernel when the input service whose seat this is can no
    /// longer receive calls (a receiver watch, note 0019).
    seat_gone: Notification,
    tries: Vec<(u32, u32)>,
    shared: Arc<Shared>,
    problem: Option<String>,
}

impl Watcher {
    /// Nothing found yet.
    pub fn new(supervisor: supervisor::Client, shared: Arc<Shared>) -> Result<Watcher, String> {
        Ok(Watcher {
            supervisor,
            devmgr: None,
            seat: None,
            seat_gone: Notification::create().map_err(|e| format!("a notification: {e}"))?,
            tries: Vec::new(),
            shared,
            problem: None,
        })
    }

    fn report(&mut self, problem: String) {
        if self.problem.as_ref() != Some(&problem) {
            granite::println!("display: {problem}");
            self.problem = Some(problem);
        }
    }

    /// The watcher's work, for ever: once a second, or at once when the input service goes,
    /// it looks for displays not yet taken and for the seat.
    pub fn watch(mut self) {
        loop {
            self.look();
            self.find_seat();
            if self.seat_gone.wait(time::after_ms(CHECK_MS)).is_ok() {
                self.seat_lost();
            }
            if self.seat.as_ref().is_some_and(|s| s.done.load(Ordering::Acquire)) {
                self.seat_lost();
            }
        }
    }

    fn connect(&mut self) -> bool {
        if self.devmgr.is_some() {
            return true;
        }
        match self.supervisor.lookup("devices") {
            Ok(h) => {
                let mut c = devices::Client::new(Endpoint(h));
                c.set_timeout_ms(CALL_MS);
                self.devmgr = Some(c);
                true
            }
            Err(CallError::Service(SupervisorError::NotReady)) => false,
            Err(e) => {
                self.report(format!("can't find the device manager: {e}"));
                false
            }
        }
    }

    /// Looks at the device list once, and takes every running display not yet held.
    fn look(&mut self) {
        if !self.connect() {
            return;
        }
        let listed = match &self.devmgr {
            Some(c) => c.list(),
            None => return,
        };
        let list = match listed {
            Ok(l) => l,
            Err(e) => {
                if lost(&e) {
                    self.devmgr = None;
                    self.tries.clear();
                } else {
                    self.report(format!("can't list devices: {e}"));
                }
                return;
            }
        };
        for d in list.iter().filter(|d| d.kind == DeviceClass::Display) {
            if d.state != DeviceState::Running || self.shared.holds(d.id) || self.given_up(d.id) {
                continue;
            }
            // An empty reason is "not ready yet": a driver still starting (a framebuffer's can
            // take many seconds, emulated) isn't a failure, and is simply tried again.
            if let Err(why) = self.take_display(d.id)
                && !why.is_empty()
            {
                if self.tried(d.id) >= MAX_TRIES {
                    granite::println!("display: leaving {} alone: {why}", d.name);
                } else {
                    granite::println!("display: can't use {} yet: {why}", d.name);
                }
            }
        }
    }

    fn given_up(&self, id: u32) -> bool {
        self.tries.iter().any(|t| t.0 == id && t.1 >= MAX_TRIES)
    }

    fn tried(&mut self, id: u32) -> u32 {
        match self.tries.iter_mut().find(|t| t.0 == id) {
            Some(t) => {
                t.1 += 1;
                t.1
            }
            None => {
                self.tries.push((id, 1));
                1
            }
        }
    }

    /// Takes display `id` and hands it to the main thread, with a thread to hear its done
    /// notification. An empty error means "not ready yet, try later" without a complaint.
    fn take_display(&mut self, id: u32) -> Result<(), String> {
        let opened = match &self.devmgr {
            Some(c) => c.open(id),
            None => return Err(String::new()),
        };
        let endpoint = match opened {
            Ok(h) => Endpoint(h),
            Err(CallError::Service(DevicesError::NotReady)) => return Err(String::new()),
            Err(e) if lost(&e) => {
                self.devmgr = None;
                return Err(String::new());
            }
            Err(e) => return Err(format!("the device manager: {e}")),
        };
        let mut client = display::Client::new(endpoint);
        client.set_timeout_ms(CALL_MS);
        let taken = client.take().map_err(|e| format!("taking it: {e}"))?;
        let mut taker = taken.taker;
        taker.set_timeout_ms(CALL_MS);
        let stop = Arc::new(AtomicBool::new(false));
        let waiter = DoneThread {
            device: id,
            done: taken.done,
            stop: stop.clone(),
            shared: self.shared.clone(),
        };
        // `move ||` makes a closure (a function without a name) that takes what it uses with
        // it: here the done notification goes to the new thread. It ends by itself when the
        // main thread sets `stop`, so it is never joined.
        thread::spawn(move || waiter.run()).map_err(|e| format!("starting a thread: {e}"))?;
        self.shared.hold(id);
        self.shared.push(Message::Display(Box::new(Taken { device: id, client, taker, stop })));
        self.problem = None;
        Ok(())
    }

    fn seat_lost(&mut self) {
        if let Some(s) = self.seat.take() {
            granite::println!("display: the input service has gone; looking for it again.");
            s.stop.store(true, Ordering::Release);
        }
    }

    /// Takes the seat if this program doesn't have it: looks the input service up, asks the
    /// kernel to say when it goes, takes the seat and starts a thread to read its ring.
    fn find_seat(&mut self) {
        if self.seat.is_some() {
            return;
        }
        let endpoint = match self.supervisor.lookup("input") {
            Ok(h) => Endpoint(h),
            Err(CallError::Service(SupervisorError::NotReady)) => return,
            Err(e) => return self.report(format!("can't find the input service: {e}")),
        };
        // A copy of the capability for the seat thread, which asks for the devices.
        let Ok(copy) = endpoint.0.duplicate(granite::sys::rights::SEND) else {
            return self.report(String::from("can't copy the input service's capability"));
        };
        let _ = self.seat_gone.wait(time::NOW);
        if let Err(e) = endpoint.watch_receiver(&self.seat_gone, 1) {
            return self.report(format!("can't watch the input service: {e}"));
        }
        let mut client = seat::Client::new(endpoint);
        client.set_timeout_ms(CALL_MS);
        let taken = match client.take() {
            Ok(t) => t,
            Err(ref e) if lost(e) => return,
            Err(CallError::Service(SeatError::NotAllowed)) => {
                return self.report(String::from("the input service won't give us the seat"));
            }
            Err(e) => return self.report(format!("can't take the seat: {e}")),
        };
        let map = match taken.ring.map(perm::READ_WRITE) {
            Ok(m) => m,
            Err(e) => return self.report(format!("mapping the seat's ring: {e}")),
        };
        // SAFETY: the mapping is readable and writable for its whole length and starts on a
        // page, aligned as a ring needs. It goes to the seat thread with the consumer, so it
        // lives as long as the consumer does.
        let consumer = match unsafe { Consumer::new(map.as_mut_ptr(), map.len() as usize) } {
            Ok(c) => c,
            Err(e) => return self.report(format!("the seat's ring is no good: {e:?}")),
        };
        let mut asker = seat::Client::new(Endpoint(copy));
        asker.set_timeout_ms(CALL_MS);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let t = SeatThread {
            consumer,
            doorbell: taken.doorbell,
            _map: map,
            _ring: taken.ring,
            asker,
            stop: stop.clone(),
            done: done.clone(),
            shared: self.shared.clone(),
        };
        match thread::spawn(move || t.run()) {
            Ok(_) => {
                granite::println!("display: the seat is ours.");
                self.problem = None;
                self.seat = Some(Seat { _client: client, stop, done });
            }
            Err(e) => self.report(format!("starting a thread: {e}")),
        }
    }
}

/// Reads the seat's ring for as long as the seat is this program's.
struct SeatThread {
    consumer: Consumer,
    doorbell: Notification,
    _map: Mapping,
    _ring: Memory,
    /// The input service, to ask for its devices.
    asker: seat::Client,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl SeatThread {
    fn run(mut self) {
        self.tablets();
        let mut event = [0u8; EVENT_SIZE];
        loop {
            let mut batch = Vec::new();
            let mut broken = false;
            // Everything there is now, as one message.
            loop {
                match self.consumer.pop(&mut event) {
                    Ok(_) => {
                        let e = SeatEvent::decode(&event);
                        let devices = e.kind == seat_kind::DEVICES;
                        batch.push(e);
                        if devices {
                            // The tablets go before what follows, so a position from a new
                            // tablet finds it known.
                            self.shared.push(Message::Seat(core::mem::take(&mut batch)));
                            self.tablets();
                        }
                        if batch.len() >= 256 {
                            break;
                        }
                    }
                    Err(RingError::WouldBlock) => break,
                    Err(_) => {
                        broken = true;
                        break;
                    }
                }
            }
            if !batch.is_empty() {
                self.shared.push(Message::Seat(batch));
            }
            if broken {
                break;
            }
            match self.doorbell.wait(time::after_ms(CHECK_MS)) {
                Ok(_) => {}
                Err(Error::TimedOut) => {
                    if self.stop.load(Ordering::Acquire) {
                        break;
                    }
                }
                // Revoked: someone else took the seat.
                Err(_) => break,
            }
        }
        self.done.store(true, Ordering::Release);
    }

    /// Asks the input service for its devices and passes the tablets on.
    fn tablets(&mut self) {
        match self.asker.devices() {
            Ok(list) => {
                let tablets = list
                    .iter()
                    .filter(|d| d.kind == DeviceKind::Tablet)
                    .map(|d| Tablet { id: d.id, name: d.name.clone() })
                    .collect();
                self.shared.push(Message::Tablets(tablets));
            }
            Err(e) => granite::println!("display: can't ask the seat for its devices: {e}"),
        }
    }
}

/// Waits on one display's done notification and passes its bits on.
struct DoneThread {
    device: u32,
    done: Notification,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl DoneThread {
    fn run(self) {
        loop {
            match self.done.wait(time::after_ms(CHECK_MS)) {
                Ok(bits) => self.shared.push(Message::Done { device: self.device, bits }),
                Err(Error::TimedOut) => {}
                Err(_) => break,
            }
            if self.stop.load(Ordering::Acquire) {
                break;
            }
        }
    }
}
