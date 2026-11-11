// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Finding the screen and the keyboards, and keeping them found.
//!
//! The console does not own any hardware. The device manager does (note 0011): it starts a
//! driver program for each device, and this service asks it, through `pbr.devices`, for the
//! display and every input device, then talks to their drivers through `pbr.display` and
//! `pbr.input`. It finds the device manager through its supervisor, the root program
//! (`pbr.supervisor.lookup`, note 0018, section 2).
//!
//! A driver is an ordinary program, and can crash and be restarted (note 0012); so can the
//! device manager. When that happens every capability this service holds for it stops working:
//! calls fail with `PeerGone` (the program is gone) or `Revoked` or `BadHandle`. A call that
//! fails like that is not a reason to give up: the device is asked for again, at most once a
//! second ([`CHECK_MS`]), and used again once it is found.
//!
//! Several threads are involved, and all of them only *tell the main thread* what they
//! find, through a [`Shared`] queue and a notification bound to the service's endpoint (a
//! bound notification wakes the main thread's `receive`, so it never has to poll):
//! - the main thread, which draws and serves `pbr.terminal` (`main.rs`);
//! - a **watcher** ([`Devices::watch`]) that finds the devices at start-up, then looks for
//!   what is missing and what has gone;
//! - a **keyboard thread** for each keyboard ([`KeyboardThread`]), which sleeps until the
//!   keyboard's doorbell rings, then turns its events into keys.

use crate::screen::{Screen, Taken};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use pbr_interfaces::pbr_input::{self as input, InputKind};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_terminal::Key;
use pbr_terminal::input::{EVENT_SIZE, KeyDecoder};
use pbr_terminal::render::{Layout, PixelOrder};

/// How long to keep asking the supervisor for the device manager when it is not ready, at
/// start-up.
const LOOKUP_MS: u64 = 60_000;
/// How long, at start-up, to wait for a display or a keyboard that is still starting.
const SETTLE_MS: u64 = 30_000;
/// How long one call to the device manager or a driver may take. A driver that is not
/// answering must not stop the console for ever.
const CALL_MS: u64 = 5_000;
/// How often the watcher looks for what has gone or is missing, and how often an idle
/// keyboard thread checks that its keyboard is still there.
const CHECK_MS: u64 = 1_000;
/// How many times opening one device may fail before it is left alone.
const MAX_TRIES: u32 = 5;
/// How many keys may wait for the main thread. A keyboard that sends keys faster than that
/// for long (or a main thread that is stuck) loses the newest.
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

/// What the threads share: the queue of events for the main thread, and how many keyboards
/// there are.
///
/// Rust only lets a value be shared between threads in ways that cannot go wrong: an `Arc<T>`
/// (see below) is a pointer to a `T` that several threads own together, freed when the last
/// owner lets go; a [`Lock`] lets one thread at a time use the value inside it; and an
/// `AtomicUsize` (or `AtomicBool`) is a number that threads can change without a lock, one
/// indivisible step at a time.
pub struct Shared {
    queue: Lock<VecDeque<Event>>,
    /// Signalled when the queue has something in it. The main thread binds it to its
    /// endpoint, so that its `receive` wakes for it as it would for a call.
    wake: Notification,
    /// Signalled once, when the watcher has finished looking for the devices at start-up.
    settled: Notification,
    keyboards: AtomicUsize,
}

impl Shared {
    /// An empty queue and a notification to wake the main thread with.
    pub fn new() -> Result<Arc<Shared>, String> {
        let make = || Notification::create().map_err(|e| format!("making a notification: {e}"));
        Ok(Arc::new(Shared {
            queue: Lock::new(VecDeque::new()),
            wake: make()?,
            settled: make()?,
            keyboards: AtomicUsize::new(0),
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

    /// How many keyboards are open right now.
    pub fn keyboards(&self) -> usize {
        self.keyboards.load(Ordering::Acquire)
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

/// What a look at the device list found still starting, which is worth waiting for at start-up.
#[derive(Default)]
struct Pending {
    display: bool,
    keyboard: bool,
}

/// Flags shared with a keyboard thread.
#[derive(Default)]
struct Flags {
    /// Asked of the thread by the watcher: end when you next wake.
    stop: AtomicBool,
    /// Set by the thread as it ends, for any reason.
    done: AtomicBool,
}

/// A keyboard that is open.
struct Keyboard {
    id: u32,
    flags: Arc<Flags>,
    thread: JoinHandle,
}

/// The device manager and everything found through it.
pub struct Devices {
    supervisor: supervisor::Client,
    devmgr: Option<devices::Client>,
    /// The display driver, while it is the one whose screen is being drawn on.
    display: Option<display::Client>,
    keyboards: Vec<Keyboard>,
    /// Keyboards of a device manager that has gone, ending.
    retiring: Vec<Keyboard>,
    /// Devices to leave alone until the device manager changes: mice, and devices that failed
    /// to open too often.
    ignored: Vec<u32>,
    /// How many times each device has failed to open.
    tries: Vec<(u32, u32)>,
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
            display: None,
            keyboards: Vec::new(),
            retiring: Vec::new(),
            ignored: Vec::new(),
            tries: Vec::new(),
            shared,
            problem: None,
        }
    }

    /// Start-up: finds the device manager (waiting for it for up to a minute), the first
    /// display that runs and the keyboards, waiting up to 30 seconds for a display or a
    /// keyboard that is still starting. Carries on with whatever was found: the watcher keeps
    /// looking for the rest.
    fn settle(&mut self) {
        let end = time::after_ms(SETTLE_MS);
        let mut patience = LOOKUP_MS;
        loop {
            let pending = self.look(patience);
            patience = 0;
            let waiting = (pending.display && self.display.is_none())
                || (pending.keyboard && self.keyboards.is_empty());
            if !waiting || time::now() >= end {
                return;
            }
            time::sleep_ms(50);
        }
    }

    /// The watcher thread's work. It finds the devices at start-up and tells the main thread
    /// when it is done (the main thread does not serve until then, so that a `read_line` is not
    /// refused for want of a keyboard that is about to be found). Then, for ever, once a
    /// second, it notices what has gone and looks for what is missing.
    ///
    /// It all happens on this thread, from the first look on, because a thread's handle cannot
    /// be passed to another thread: the keyboard threads started here are joined here.
    pub fn watch(mut self) {
        self.settle();
        let _ = self.shared.settled.signal(1);
        loop {
            time::sleep_ms(CHECK_MS);
            self.reap();
            self.check_display();
            self.look(0);
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
    /// something else to the next one.
    fn devmgr_gone(&mut self) {
        granite::println!("console: the device manager has gone; looking for it again.");
        self.devmgr = None;
        self.ignored.clear();
        self.tries.clear();
        self.display = None;
        for keyboard in &self.keyboards {
            keyboard.flags.stop.store(true, Ordering::Release);
        }
        self.retiring.append(&mut self.keyboards);
    }

    /// Joins the keyboard threads that have ended and forgets their keyboards.
    fn reap(&mut self) {
        for list in [&mut self.keyboards, &mut self.retiring] {
            let mut i = 0;
            while i < list.len() {
                if list[i].flags.done.load(Ordering::Acquire) {
                    let keyboard = list.swap_remove(i);
                    // (The thread has finished, so this does not wait.)
                    let _ = keyboard.thread.join();
                } else {
                    i += 1;
                }
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

    /// Looks at the device list once and opens what is running and not open yet: the first
    /// display, if there is none now, and every keyboard. Returns what is still starting.
    fn look(&mut self, patience_ms: u64) -> Pending {
        let mut pending = Pending::default();
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
        self.problem = None;

        if self.display.is_none() {
            let screens: Vec<&DeviceInfo> = list
                .iter()
                .filter(|d| d.kind == DeviceClass::Display && self.wanted(d.id))
                .collect();
            for d in screens {
                match d.state {
                    DeviceState::Running => {
                        let (id, name) = (d.id, d.name.clone());
                        match self.open_display(id) {
                            Ok(()) => {}
                            Err(Trouble::Later) => pending.display = true,
                            Err(Trouble::Failed(why)) => self.give_up(id, &name, &why),
                        }
                        break;
                    }
                    DeviceState::Starting | DeviceState::Restarting => pending.display = true,
                    DeviceState::Disabled => {}
                }
            }
        }

        let inputs: Vec<&DeviceInfo> =
            list.iter().filter(|d| d.kind == DeviceClass::Input && self.wanted(d.id)).collect();
        for d in inputs {
            if self.keyboards.iter().any(|k| k.id == d.id) {
                continue;
            }
            match d.state {
                DeviceState::Running => match self.open_keyboard(d) {
                    Ok(()) => {}
                    Err(Trouble::Later) => pending.keyboard = true,
                    Err(Trouble::Failed(why)) => self.give_up(d.id, &d.name, &why),
                },
                DeviceState::Starting | DeviceState::Restarting => pending.keyboard = true,
                DeviceState::Disabled => {}
            }
        }
        pending
    }

    /// Whether device `id` is one to try (not a mouse, and not one given up on).
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

    /// Opens input device `d`, and if it is a keyboard (or one that is a mouse as well), starts
    /// a thread to read it. A mouse is put on the list to leave alone: nothing is wrong, the
    /// console has no use for it.
    fn open_keyboard(&mut self, d: &DeviceInfo) -> Result<(), Trouble> {
        let mut client = input::Client::new(self.endpoint(d.id)?);
        client.set_timeout_ms(CALL_MS);
        let info = client.info().map_err(|e| trouble("asking what it is", e))?;
        if matches!(info.kind, InputKind::Mouse) {
            self.ignored.push(d.id);
            return Ok(());
        }
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

        let flags = Arc::new(Flags::default());
        let thread = KeyboardThread {
            name: d.name.clone(),
            client,
            consumer,
            doorbell: opened.doorbell,
            _map: map,
            _ring: opened.ring,
            flags: flags.clone(),
            shared: self.shared.clone(),
        };
        // Counted before the thread starts, so that a `read_line` that comes at once finds it.
        self.shared.keyboards.fetch_add(1, Ordering::AcqRel);
        // `move ||` makes a closure (a function with no name) that takes everything it uses
        // with it, which is how a new thread is given its keyboard.
        match thread::spawn(move || thread.run()) {
            Ok(handle) => {
                granite::println!("console: using the keyboard {}.", d.name);
                self.keyboards.push(Keyboard { id: d.id, flags, thread: handle });
                Ok(())
            }
            Err(e) => {
                self.shared.keyboards.fetch_sub(1, Ordering::AcqRel);
                Err(Trouble::Failed(format!("starting a thread: {e}")))
            }
        }
    }
}

/// Reads one keyboard's ring for as long as the keyboard is there.
struct KeyboardThread {
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
}

impl KeyboardThread {
    /// Runs until the keyboard goes or the watcher asks it to stop.
    fn run(mut self) {
        let mut decoder = KeyDecoder::new();
        let mut event = [0u8; EVENT_SIZE];
        loop {
            match self.consumer.pop(&mut event) {
                Ok(_) => {
                    if let Some(key) = decoder.feed(&event) {
                        self.shared.push(Event::Key(key));
                    }
                }
                // Nothing to read. The driver rings the doorbell when it adds something (the
                // `pop` that found the ring empty asked it to); until then sleep, but wake now
                // and then to check that the keyboard is still there.
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
        granite::println!("console: the keyboard {} has gone.", self.name);
        self.shared.keyboards.fetch_sub(1, Ordering::AcqRel);
        self.flags.done.store(true, Ordering::Release);
        // Wake the main thread: a `read_line` may be waiting on this keyboard (nothing else
        // is needed from it, but it is cheap and the queue may be waiting to be read).
        let _ = self.shared.wake.signal(WAKE);
    }

    /// Whether the driver has gone, by asking it something.
    fn driver_gone(&self) -> bool {
        matches!(self.client.info(), Err(ref e) if lost(e))
    }
}
