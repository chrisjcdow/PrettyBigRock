// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding the screen and the seat, and keeping them found.
//!
//! The console does not own any hardware. The device manager does (note 0011): it starts a
//! driver program for each device, and this service asks it, through `pbr.devices`, for the
//! display, then talks to its driver through `pbr.display`. Keys come from the input service
//! (note 0021): the console takes the *seat* (`pbr.seat`: every keyboard merged, with the
//! user's layout applied) and reads key events from the ring that gives it. It finds both
//! services through its supervisor, the root program (`pbr.supervisor.lookup`, note 0018,
//! section 2), which is also what decides that the console may take the seat at all.
//!
//! A driver is an ordinary program, and can crash and be restarted (note 0012); so can the
//! device manager and the input service. When that happens every capability this service
//! holds for it stops working: calls fail with `PeerGone` (the program is gone) or `Revoked`
//! or `BadHandle`. That is not a reason to give up: the display is asked for again, at most
//! once a second ([`CHECK_MS`]), and the seat is taken again from the new input service as
//! soon as it can be looked up.
//!
//! Several threads are involved, and all of them only *tell the main thread* what they
//! find, through a [`Shared`] queue and a notification bound to the service's endpoint (a
//! bound notification wakes the main thread's `receive`, so it never has to poll):
//! - the main thread, which draws and serves `pbr.terminal` (`main.rs`);
//! - a **watcher** ([`Devices::watch`]) that finds the display and the seat at start-up, then
//!   looks for what is missing and what has gone;
//! - a **seat thread** ([`SeatThread`]), which sleeps until the seat's doorbell rings, then
//!   turns its events into keys.

use crate::screen::{Screen, Taken};
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
use granite::thread::{self, JoinHandle};
use granite::{Error, time};
use pbr_interfaces::pbr_devices::{
    self as devices, DeviceClass, DeviceInfo, DeviceState, DevicesError,
};
use pbr_interfaces::pbr_display as display;
use pbr_interfaces::pbr_seat::{self as seat, SeatError};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_terminal::Key;
use pbr_terminal::input::{EVENT_SIZE, keys};
use pbr_terminal::render::{Layout, PixelOrder};

/// How long to keep asking the supervisor for the device manager when it is not ready, at
/// start-up.
const LOOKUP_MS: u64 = 60_000;
/// How long, at start-up, to wait for a display that is still starting, or for the seat.
const SETTLE_MS: u64 = 30_000;
/// How long one call to the device manager or a driver may take. A driver that is not
/// answering must not stop the console for ever.
const CALL_MS: u64 = 5_000;
/// How often the watcher looks for what has gone or is missing, and how often an idle seat
/// thread checks whether it has been asked to stop.
const CHECK_MS: u64 = 1_000;
/// How many times opening one device may fail before it is left alone.
const MAX_TRIES: u32 = 5;
/// How many keys may wait for the main thread. Keys that come faster than that for long (or
/// a main thread that is stuck) lose the newest.
const MAX_QUEUED: usize = 1024;
/// The signal bit that means "look at the queue".
const WAKE: u64 = 1;

/// What the other threads tell the main thread.
pub enum Event {
    /// A key was typed.
    Key(Key),
    /// A screen was found, or found again. (Boxed because it is much bigger than a key.)
    Screen(Box<Screen>),
}

/// What the threads share: the queue of events for the main thread, and whether there is a
/// seat to get keys from.
///
/// Rust only lets a value be shared between threads in ways that cannot go wrong: an `Arc<T>`
/// (see below) is a pointer to a `T` that several threads own together, freed when the last
/// owner lets go; a [`Lock`] lets one thread at a time use the value inside it; and an
/// `AtomicBool` is a flag that threads can read and change without a lock, one indivisible
/// step at a time.
pub struct Shared {
    queue: Lock<VecDeque<Event>>,
    /// Signalled when the queue has something in it. The main thread binds it to its
    /// endpoint, so that its `receive` wakes for it as it would for a call.
    wake: Notification,
    /// Signalled once, when the watcher has finished looking for the devices at start-up.
    settled: Notification,
    /// Whether the seat is taken: keys can arrive.
    seat: AtomicBool,
}

impl Shared {
    /// An empty queue and a notification to wake the main thread with.
    pub fn new() -> Result<Arc<Shared>, String> {
        let make = || Notification::create().map_err(|e| format!("making a notification: {e}"));
        Ok(Arc::new(Shared {
            queue: Lock::new(VecDeque::new()),
            wake: make()?,
            settled: make()?,
            seat: AtomicBool::new(false),
        }))
    }

    /// Waits until the watcher has done its first look for devices (which takes at most about
    /// a minute and a half). Returns whether it has.
    pub fn wait_settled(&self) -> bool {
        self.settled.wait(time::after_ms(LOOKUP_MS + SETTLE_MS + 10_000)).is_ok()
    }

    /// The notification that wakes the main thread.
    pub fn wake(&self) -> &Notification {
        &self.wake
    }

    /// Whether the console has the seat right now, so that typing can reach it.
    pub fn has_seat(&self) -> bool {
        self.seat.load(Ordering::Acquire)
    }

    /// Adds an event for the main thread and wakes it.
    fn push(&self, event: Event) {
        {
            let mut queue = self.queue.lock();
            if matches!(event, Event::Key(_)) && queue.len() >= MAX_QUEUED {
                return;
            }
            queue.push_back(event);
        }
        // If this fails the main thread is already gone, and nobody needs waking.
        let _ = self.wake.signal(WAKE);
    }

    /// Takes everything queued so far, in order.
    pub fn take(&self) -> VecDeque<Event> {
        core::mem::take(&mut *self.queue.lock())
    }
}

/// Whether a call failed because what it called has gone, so that it may be found again.
fn lost<E>(e: &CallError<E>) -> bool {
    matches!(e, CallError::Call(Error::PeerGone | Error::Revoked | Error::BadHandle))
}

/// What a failed call to a driver means for opening its device: it has gone (try again), or
/// it did not do what it was asked (`what`).
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

/// Flags shared with the seat thread.
#[derive(Default)]
struct Flags {
    /// Asked of the thread by the watcher: end when you next wake.
    stop: AtomicBool,
    /// Set by the thread as it ends, for any reason.
    done: AtomicBool,
}

/// The seat, while the console has it.
struct Seat {
    /// The input service, through the capability the seat was taken with. The watch for the
    /// input service going is on this capability, so it is kept for as long as the seat is.
    _client: seat::Client,
    flags: Arc<Flags>,
    thread: JoinHandle,
}

/// The device manager, the input service, and everything found through them.
pub struct Devices {
    supervisor: supervisor::Client,
    devmgr: Option<devices::Client>,
    /// The display driver, while it is the one whose screen is being drawn on.
    display: Option<display::Client>,
    seat: Option<Seat>,
    /// Seat threads of an input service that has gone, ending.
    retiring: Vec<Seat>,
    /// Signalled (bit 0) by the kernel when the input service whose seat this is can no longer
    /// receive calls: it has ended (a *receiver watch*, note 0019, section 2). The watcher
    /// sleeps on it, so it wakes at once instead of at its next look.
    seat_gone: Notification,
    /// Displays to leave alone until the device manager changes: ones that failed to open
    /// too often.
    ignored: Vec<u32>,
    /// How many times each device has failed to open.
    tries: Vec<(u32, u32)>,
    shared: Arc<Shared>,
    /// The last thing reported as wrong, so that the same thing is not reported every second.
    problem: Option<String>,
}

impl Devices {
    /// Nothing found yet. `supervisor` is where the device manager and the input service are
    /// looked up; `seat_gone` is a notification of the watcher's own, for hearing that the
    /// input service has gone.
    pub fn new(
        supervisor: supervisor::Client,
        shared: Arc<Shared>,
        seat_gone: Notification,
    ) -> Devices {
        Devices {
            supervisor,
            devmgr: None,
            display: None,
            seat: None,
            retiring: Vec::new(),
            seat_gone,
            ignored: Vec::new(),
            tries: Vec::new(),
            shared,
            problem: None,
        }
    }

    /// Start-up: finds the device manager (waiting for it for up to a minute), the first
    /// display that runs and the seat, waiting up to 30 seconds for a display that is still
    /// starting or an input service that is. Carries on with whatever was found: the watcher
    /// keeps looking for the rest.
    fn settle(&mut self) {
        let end = time::after_ms(SETTLE_MS);
        let mut patience = LOOKUP_MS;
        loop {
            let display_pending = self.look(patience);
            patience = 0;
            self.find_seat();
            let waiting = (display_pending && self.display.is_none()) || self.seat.is_none();
            if !waiting || time::now() >= end {
                return;
            }
            time::sleep_ms(50);
        }
    }

    /// The watcher thread's work. It finds the display and the seat at start-up and tells the
    /// main thread when it is done (the main thread does not serve until then, so that a
    /// `read_line` is not refused for want of a seat that is about to be taken). Then, for
    /// ever, once a second, or at once when the input service goes, it notices what has gone
    /// and looks for what is missing.
    ///
    /// It all happens on this thread, from the first look on, because a thread's handle cannot
    /// be passed to another thread: the seat threads started here are joined here.
    pub fn watch(mut self) {
        self.settle();
        let _ = self.shared.settled.signal(1);
        loop {
            // Sleeps for a second, or until the kernel says the input service has gone.
            if self.seat_gone.wait(time::after_ms(CHECK_MS)).is_ok() {
                self.seat_lost();
            }
            self.reap();
            self.check_display();
            self.look(0);
            self.find_seat();
        }
    }

    /// Prints `problem` unless it was the last thing printed.
    fn report(&mut self, problem: String) {
        if self.problem.as_ref() != Some(&problem) {
            granite::println!("console: {problem}");
            self.problem = Some(problem);
        }
    }

    /// Looks the device manager up through the supervisor, retrying while it is not ready for
    /// up to `patience_ms`.
    fn connect(&mut self, patience_ms: u64) -> bool {
        let end = time::after_ms(patience_ms);
        loop {
            match self.supervisor.lookup("devices") {
                Ok(h) => {
                    let mut client = devices::Client::new(Endpoint(h));
                    client.set_timeout_ms(CALL_MS);
                    self.devmgr = Some(client);
                    return true;
                }
                Err(CallError::Service(SupervisorError::NotReady)) => {}
                Err(e) => {
                    self.report(format!("can't find the device manager: {e}"));
                    return false;
                }
            }
            if time::now() >= end {
                return false;
            }
            time::sleep_ms(50);
        }
    }

    /// The device manager has gone: its devices are gone with it, and the ids it gave out mean
    /// something else to the next one. (The seat is the input service's, which finds its
    /// devices again by itself: the console keeps the seat.)
    fn devmgr_gone(&mut self) {
        granite::println!("console: the device manager has gone; looking for it again.");
        self.devmgr = None;
        self.ignored.clear();
        self.tries.clear();
        self.display = None;
    }

    /// The input service has gone: the seat it gave is dead. Its thread is asked to end, and
    /// the next look takes the seat from the new input service.
    fn seat_lost(&mut self) {
        if let Some(seat) = self.seat.take() {
            granite::println!("console: the input service has gone; looking for it again.");
            self.shared.seat.store(false, Ordering::Release);
            seat.flags.stop.store(true, Ordering::Release);
            self.retiring.push(seat);
        }
    }

    /// Joins the seat threads that have ended. One that ended by itself (its ring broke) also
    /// means the seat is to be taken again.
    fn reap(&mut self) {
        if self.seat.as_ref().is_some_and(|s| s.flags.done.load(Ordering::Acquire)) {
            self.seat_lost();
        }
        let mut i = 0;
        while i < self.retiring.len() {
            if self.retiring[i].flags.done.load(Ordering::Acquire) {
                let seat = self.retiring.swap_remove(i);
                // (The thread has finished, so this does not wait.)
                let _ = seat.thread.join();
            } else {
                i += 1;
            }
        }
    }

    /// Takes the seat, if the console doesn't have it: looks the input service up, asks it to
    /// tell us when it goes, takes the seat (which stops whoever had it before from getting
    /// keys) and starts a thread to read the ring. If the input service isn't running yet, or
    /// is being started again, the next look tries again.
    fn find_seat(&mut self) {
        if self.seat.is_some() {
            return;
        }
        let endpoint = match self.supervisor.lookup("input") {
            Ok(h) => Endpoint(h),
            Err(CallError::Service(SupervisorError::NotReady)) => return,
            Err(e) => {
                self.report(format!("can't find the input service: {e}"));
                return;
            }
        };
        // Forget a signal left from the input service before this one, then watch this one.
        // (The watch belongs to this capability, so the capability is kept with the seat.)
        let _ = self.seat_gone.wait(time::NOW);
        if let Err(e) = endpoint.watch_receiver(&self.seat_gone, 1) {
            self.report(format!("can't watch the input service: {e}"));
            return;
        }
        let mut client = seat::Client::new(endpoint);
        client.set_timeout_ms(CALL_MS);
        let taken = match client.take() {
            Ok(t) => t,
            // It went between the lookup and the call: the watch has fired, or will.
            Err(ref e) if lost(e) => return,
            Err(CallError::Service(SeatError::NotAllowed)) => {
                self.report(String::from("the input service won't give the console the seat"));
                return;
            }
            Err(e) => {
                self.report(format!("can't take the seat: {e}"));
                return;
            }
        };
        let map = match taken.ring.map(perm::READ_WRITE) {
            Ok(m) => m,
            Err(e) => {
                self.report(format!("mapping the seat's ring: {e}"));
                return;
            }
        };
        // SAFETY: the mapping is readable and writable for its whole length and starts on a
        // page, which is aligned to 64 as a ring needs. It is kept in the same struct as the
        // consumer, so it lives as long as the consumer does.
        let consumer = match unsafe { Consumer::new(map.as_mut_ptr(), map.len() as usize) } {
            Ok(c) => c,
            Err(e) => {
                self.report(format!("the seat's ring is no good: {e:?}"));
                return;
            }
        };
        let flags = Arc::new(Flags::default());
        let thread = SeatThread {
            consumer,
            doorbell: taken.doorbell,
            _map: map,
            _ring: taken.ring,
            flags: flags.clone(),
            shared: self.shared.clone(),
        };
        // Set before the thread starts, so that a `read_line` that comes at once finds it.
        self.shared.seat.store(true, Ordering::Release);
        // `move ||` makes a closure (a function with no name) that takes everything it uses
        // with it, which is how a new thread is given its ring.
        match thread::spawn(move || thread.run()) {
            Ok(handle) => {
                granite::println!("console: keys come from the input service.");
                self.problem = None;
                self.seat = Some(Seat { _client: client, flags, thread: handle });
            }
            Err(e) => {
                self.shared.seat.store(false, Ordering::Release);
                self.report(format!("starting a thread: {e}"));
            }
        }
    }

    /// Asks the display driver something, to see whether it is still there.
    fn check_display(&mut self) {
        let gone = self
            .display
            .as_ref()
            .is_some_and(|client| matches!(client.screens(), Err(ref e) if lost(e)));
        if gone {
            granite::println!("console: the screen has gone; looking for it again.");
            self.display = None;
        }
    }

    /// Looks at the device list once and opens the first display that is running, if there is
    /// none now. Returns whether a display is still starting.
    fn look(&mut self, patience_ms: u64) -> bool {
        let mut pending = false;
        if self.devmgr.is_none() && !self.connect(patience_ms) {
            return pending;
        }
        let listed = match &self.devmgr {
            Some(client) => client.list(),
            None => return pending,
        };
        let list = match listed {
            Ok(list) => list,
            Err(e) => {
                if lost(&e) {
                    self.devmgr_gone();
                } else {
                    self.report(format!("can't list devices: {e}"));
                }
                return pending;
            }
        };
        if self.display.is_some() {
            return pending;
        }
        let screens: Vec<&DeviceInfo> =
            list.iter().filter(|d| d.kind == DeviceClass::Display && self.wanted(d.id)).collect();
        for d in screens {
            match d.state {
                DeviceState::Running => {
                    let (id, name) = (d.id, d.name.clone());
                    match self.open_display(id) {
                        Ok(()) => {}
                        Err(Trouble::Later) => pending = true,
                        Err(Trouble::Failed(why)) => self.give_up(id, &name, &why),
                    }
                    break;
                }
                DeviceState::Starting | DeviceState::Restarting => pending = true,
                DeviceState::Disabled => {}
            }
        }
        pending
    }

    /// Whether device `id` is one to try (not one given up on).
    fn wanted(&self, id: u32) -> bool {
        !self.ignored.contains(&id)
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
            granite::println!("console: leaving {name} alone: {why}");
            self.ignored.push(id);
        } else {
            granite::println!("console: can't use {name}: {why}");
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

    /// Opens display `id`: takes it (which also takes the screen from the kernel), gets a
    /// buffer for its first connected screen, maps it and tells the main thread.
    fn open_display(&mut self, id: u32) -> Result<(), Trouble> {
        let mut client = display::Client::new(self.endpoint(id)?);
        client.set_timeout_ms(CALL_MS);
        let listed = client.screens().map_err(|e| trouble("its screens", e))?;
        // A device can have several screens; the console writes on the first that has
        // something plugged in. (The desktop, note 0022, uses them all.)
        let Some(info) = listed.screens.iter().find(|s| s.connected && s.screen < 16) else {
            return Err(Trouble::Failed(String::from("nothing is plugged in to it")));
        };
        let taken = client.take().map_err(|e| trouble("taking it", e))?;
        let (mut taker, done) = (taken.taker, taken.done);
        taker.set_timeout_ms(CALL_MS);
        // A screen that is plugged in but switched off shows the mode it likes best.
        let mut mode = info.current.clone();
        if mode.width == 0 {
            let Some(first) = info.modes.first() else {
                return Err(Trouble::Failed(String::from("the screen is off and offers no mode")));
            };
            taker.set_mode(info.screen, first).map_err(|e| trouble("switching it on", e))?;
            mode = first.clone();
        }
        let made = taker.create_buffer(info.screen).map_err(|e| trouble("a buffer", e))?;
        // Buffers are always blue, green, red, one unused byte, whatever the screen itself is.
        let layout = Layout::new(mode.width, mode.height, made.stride, PixelOrder::Bgrx)
            .ok_or_else(|| {
                Trouble::Failed(format!(
                    "a screen of {}x{} pixels ({} bytes a row) is of no use",
                    mode.width, mode.height, made.stride
                ))
            })?;
        // The driver says how big the buffer is, but the mapping is what is really there
        // (`Screen::new` compares them).
        let map = made
            .memory
            .map(perm::READ_WRITE)
            .map_err(|e| Trouble::Failed(format!("mapping the buffer: {e}")))?;
        let taken =
            Taken { taker, done, screen: info.screen, buffer: made.buffer, _memory: made.memory };
        let screen = Screen::new(layout, map, taken).ok_or_else(|| {
            Trouble::Failed(String::from("the buffer is smaller than the screen"))
        })?;
        self.display = Some(client);
        self.problem = None;
        self.shared.push(Event::Screen(Box::new(screen)));
        Ok(())
    }
}

/// Reads the seat's ring for as long as the seat is this console's.
struct SeatThread {
    consumer: Consumer,
    /// Rung by the input service when it adds events.
    doorbell: Notification,
    /// The ring's memory, which the consumer reads: kept so that it stays mapped.
    _map: Mapping,
    _ring: Memory,
    flags: Arc<Flags>,
    shared: Arc<Shared>,
}

impl SeatThread {
    /// Runs until the watcher asks it to stop or the ring breaks.
    ///
    /// The input service going doesn't make the doorbell fail (this thread holds the
    /// notification, so it goes on existing; it just never rings again): the watcher hears of
    /// that from the kernel and sets the stop flag, which is looked at once a second.
    fn run(mut self) {
        let mut event = [0u8; EVENT_SIZE];
        loop {
            match self.consumer.pop(&mut event) {
                Ok(_) => {
                    // An event is at most two keys (a dead key and a letter it doesn't
                    // combine with type two characters).
                    for key in keys(&event).into_iter().flatten() {
                        self.shared.push(Event::Key(key));
                    }
                }
                // Nothing to read. The input service rings the doorbell when it adds
                // something; until then sleep, but wake now and then to see whether to stop.
                Err(RingError::WouldBlock) => match self.doorbell.wait(time::after_ms(CHECK_MS)) {
                    Ok(_) => {}
                    Err(Error::TimedOut) => {
                        if self.flags.stop.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    // The doorbell stopped working: the seat was taken by someone else (the
                    // input service revokes the old consumer's ring and doorbell).
                    Err(_) => break,
                },
                // The ring is broken (the memory was taken back, or holds nonsense).
                Err(_) => break,
            }
        }
        self.flags.done.store(true, Ordering::Release);
        // Wake the main thread: the queue may be waiting to be read.
        let _ = self.shared.wake.signal(WAKE);
    }
}
