// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The root program as supervisor (docs/design/0018-supervision-shell-and-m5-tests.md,
//! sections 1 to 4): it starts the system services (the device manager, the storage service,
//! the input service, the console or the display server, the power service and the users
//! service), watches them, and starts one again when it ends, with the restart policy every
//! supervisor uses (`lib/supervise`).
//!
//! **Falling back to text** (docs/design/0024-the-session-desktop.md, section 8): if the
//! display server ends too often and is given up on, the console is started in its place, so
//! that the user still gets the shell. The display server isn't tried again until the next
//! start.
//!
//! It also serves `pbr.supervisor` to them: **services find each other through it**, not
//! through capabilities fixed when they start. A fixed table, [`rule`], says who may look up
//! what. After a service is restarted, calls through its clients' old capabilities fail with
//! `PEER_GONE`, and they look it up again. For that to happen, nobody else may hold a capability
//! that can *receive* on a service's endpoint (the kernel fails callers only once the last
//! receiving capability has gone): so each service's endpoint is moved to it, and this module
//! keeps only a copy that can send and mint badged copies for clients.
//!
//! The device manager also gives it copies of its DMA memory to keep (`keep`, section 4): if the
//! device manager dies, its devices might still be writing into that memory, so it must not be
//! freed (and reused) until the next device manager has switched every device off and says so
//! with `release_kept`.
//!
//! The work happens on a thread of its own, so that the root program's main thread can run the
//! self-test meanwhile; the two share the children's records behind a lock.

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use granite::Handle;
use granite::endpoint::{Endpoint, Event};
use granite::lock::Lock;
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::{self, Process, Spawn, Spawned, State, role};
use granite::sys::rights;
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};
use pbr_supervise::{Decision, History};

/// The services the root program supervises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Devmgr,
    Storage,
    Users,
    Console,
    /// The input service (note 0021).
    Input,
    /// The display server (note 0022).
    Display,
    /// The power service (note 0024, section 7).
    Power,
}

const KINDS: [Kind; 7] = [
    Kind::Devmgr,
    Kind::Storage,
    Kind::Users,
    Kind::Console,
    Kind::Input,
    Kind::Display,
    Kind::Power,
];

impl Kind {
    /// Its program's name in the start-up archive.
    pub fn program(self) -> &'static str {
        match self {
            Kind::Devmgr => "devmgr",
            Kind::Storage => "storage",
            Kind::Users => "users",
            Kind::Console => "console",
            Kind::Input => "input",
            Kind::Display => "display",
            Kind::Power => "power",
        }
    }

    fn index(self) -> usize {
        match self {
            Kind::Devmgr => 0,
            Kind::Storage => 1,
            Kind::Users => 2,
            Kind::Console => 3,
            Kind::Input => 4,
            Kind::Display => 5,
            Kind::Power => 6,
        }
    }

    /// Its budget. The device manager's covers every driver of the self-test's machines,
    /// which is 96 MiB without a display, and a display driver's buffers (four of 10 MiB for
    /// virtio-gpu, or the framebuffer driver's allowance of 40 MiB; note 0020) twice over,
    /// since a driver being restarted is given its budget before the old one's is back; the
    /// storage service's its 16 MiB cache, volumes in memory and 8 MiB for each filesystem
    /// server; the users service's three sessions of 48 MiB at once. The console's holds its
    /// text grid and its threads (the buffer it draws in is the display driver's memory). The
    /// input service's holds its layouts, a thread for each device and the consumer's ring. The
    /// display server's holds every window's buffers (96 MiB a session, note 0022, section 3),
    /// the clipboard (16 MiB) and its own; a screen's buffers are its driver's. The power
    /// service's is next to nothing.
    fn budget(self) -> u64 {
        const MIB: u64 = 1024 * 1024;
        match self {
            Kind::Devmgr => 192 * MIB,
            Kind::Storage => 128 * MIB,
            Kind::Users => 160 * MIB,
            Kind::Console => 8 * MIB,
            Kind::Input => 8 * MIB,
            Kind::Display => 160 * MIB,
            Kind::Power => MIB,
        }
    }
}

/// The role the device manager is given the kernel log with `CONTROL` at
/// (`services/devmgr` uses the same number).
const DEVMGR_LOG_CONTROL: u32 = 1001;

/// The badge of the storage service's `pbr.storage` for administrators (`services/storage`'s
/// `ADMIN`): what the users service gets when it looks up `storage`.
pub const STORAGE_ADMIN: u64 = 1;

/// The badge on the clients' capabilities for services that don't tell their clients apart
/// (the device manager and the console). It is never 0: a capability without a badge that can
/// be copied lets its holder mint copies with any badge it likes (note 0006, section 5), which
/// would let a client pass itself off as another the day such a service starts using badges.
pub const ANY_CLIENT: u64 = 1;

/// The badge of a consumer of the seat (`services/input`'s `CONSUMER`): what the console gets
/// when it looks up `input`, and the display server will (note 0022). It can take the seat,
/// which means hearing every key typed, so it is given to nobody else.
pub const SEAT_CONSUMER: u64 = 1;
/// The badge of the users service's capability for the input service (`services/input`'s
/// `LAYOUT_SETTER`): it can set the keyboard layout and nothing else (note 0021, section 5).
pub const SEAT_LAYOUT: u64 = 2;
/// The badge of the users service's capability for the display server (`services/display`'s
/// `SESSIONS`): `pbr.display.sessions`, which starts sessions and decides which one the screens
/// show (note 0022, section 1). The capabilities apps get are derived from what it hands out.
pub const DISPLAY_SESSIONS: u64 = 1;
/// The badge of the users service's capability for the power service (`services/power`'s
/// `USERS`): the only one there is (note 0024, section 7).
pub const POWER_USERS: u64 = 1;

/// Who may look up what (note 0018, section 2; the input service: note 0021, section 1; the
/// display server: note 0024, section 1): the service, and the badge the capability carries
/// (never 0). Nobody else may look up anything. The console's endpoint serves `pbr.terminal`:
/// the users service gives it to one session, whose shell uses it.
///
/// This table is where "who may take the seat" and "who may take the screens" are decided:
/// the console and the display server, one or the other in a boot. The input service only
/// checks the badge a call arrives with; which program gets which badge is here. (A display
/// driver lets whoever opens it take the screens; only the system services in this table can
/// open a device at all, and of those only these two do.)
pub(crate) fn rule(child: Kind, name: &str) -> Option<(Kind, u64)> {
    match (child, name) {
        (Kind::Storage, "devices") => Some((Kind::Devmgr, ANY_CLIENT)),
        (Kind::Console, "devices") => Some((Kind::Devmgr, ANY_CLIENT)),
        (Kind::Input, "devices") => Some((Kind::Devmgr, ANY_CLIENT)),
        (Kind::Console, "input") => Some((Kind::Input, SEAT_CONSUMER)),
        (Kind::Users, "input") => Some((Kind::Input, SEAT_LAYOUT)),
        (Kind::Users, "storage") => Some((Kind::Storage, STORAGE_ADMIN)),
        (Kind::Users, "console") => Some((Kind::Console, ANY_CLIENT)),
        (Kind::Display, "devices") => Some((Kind::Devmgr, ANY_CLIENT)),
        (Kind::Display, "input") => Some((Kind::Input, SEAT_CONSUMER)),
        (Kind::Users, "display") => Some((Kind::Display, DISPLAY_SESSIONS)),
        (Kind::Users, "power") => Some((Kind::Power, POWER_USERS)),
        _ => None,
    }
}

/// The most memory objects kept for the device manager (one per device with DMA memory, for
/// this instance and the one before).
const MAX_KEPT: usize = 128;
/// A capability that can be passed on, but not received on.
const CLIENT: u32 = rights::SEND | rights::TRANSFER | rights::DUPLICATE;

/// The badge of a child's `pbr.supervisor` capability: which child, and which time it was
/// started (its *generation*). A capability left over from an earlier instance is told apart.
fn badge(kind: Kind, generation: u32) -> u64 {
    ((kind.index() as u64 + 1) << 32) | u64::from(generation)
}

fn from_badge(b: u64) -> Option<(Kind, u32)> {
    let kind = *KINDS.get(((b >> 32) as usize).checked_sub(1)?)?;
    Some((kind, b as u32))
}

/// One supervised service.
struct Child {
    kind: Kind,
    /// Its arguments.
    args: String,
    /// How many times it has been started.
    generation: u32,
    /// The running instance.
    running: Option<Spawned>,
    /// An unbadged copy of the running instance's endpoint that can send and mint, not receive.
    endpoint: Option<Endpoint>,
    history: History,
    /// When a restart is due (milliseconds on the clock).
    due: Option<u64>,
    restarts: u32,
    /// Stopped on purpose (or given up on): not restarted.
    retired: bool,
    /// Memory kept for it, with the generation that asked.
    kept: Vec<(u32, Memory)>,
    /// The last `release_kept`: which generation asked, and when (nanoseconds).
    released: Option<(u32, u64)>,
}

struct Inner {
    archive: Archive<'static>,
    archive_mem: u32,
    /// The root program's own copies of the device capabilities, for each device manager.
    devices: Vec<(u32, Handle)>,
    /// What the power service is started with: (role, capability, rights), and its arguments
    /// (`power::caps`).
    power: Vec<(u32, Handle, u32)>,
    power_args: String,
    /// Whether the console is started when the display server is given up on (note 0024,
    /// section 8): in every boot but the self-test, which tests supervision on its own terms.
    fallback: bool,
    /// The console's arguments then.
    fallback_args: String,
    selftest: bool,
    /// The endpoint children's `pbr.supervisor` capabilities are minted from.
    endpoint: Endpoint,
    /// Signalled with a child's bit (its index) when it ends; bound to `endpoint`.
    ended: Notification,
    children: Vec<Child>,
}

/// The supervisor: cheap to clone, shared between the root program's threads.
#[derive(Clone)]
pub struct Supervisor(Arc<Lock<Inner>>);

fn now_ms() -> u64 {
    time::now() / 1_000_000
}

/// A copy of the handle numbered `raw`, which belongs to someone else (it isn't closed here).
fn copy(raw: u32) -> granite::Result<Handle> {
    // `ManuallyDrop` stops the wrapper closing the original when it goes out of scope.
    let original = ManuallyDrop::new(Handle::from_raw(raw));
    original.duplicate(rights::ALL)
}

impl Supervisor {
    /// A supervisor with nothing started yet. It copies the device capabilities in `start`
    /// now, for every device manager it will start, and starts its thread.
    pub fn new(
        archive: Archive<'static>,
        archive_mem: u32,
        start: &granite::Start,
        selftest: bool,
    ) -> Result<Supervisor, String> {
        let mut devices = Vec::new();
        for r in
            [role::DEVICE_MEMORY, role::INTERRUPTS, role::MSI_POOL, role::IO_PORTS, role::MACHINE]
        {
            if let Some(h) = start.peek(r) {
                devices.push((r, copy(h).map_err(|e| format!("copying device access: {e}"))?));
            }
        }
        let endpoint = Endpoint::create().map_err(|e| format!("an endpoint: {e}"))?;
        let ended = Notification::create().map_err(|e| format!("a notification: {e}"))?;
        endpoint.bind(&ended).map_err(|e| format!("binding: {e}"))?;
        let receiver =
            Endpoint(endpoint.0.duplicate(rights::ALL).map_err(|e| format!("copying: {e}"))?);
        let sup = Supervisor(Arc::new(Lock::new(Inner {
            archive,
            archive_mem,
            devices,
            power: Vec::new(),
            power_args: String::new(),
            fallback: !selftest,
            fallback_args: String::new(),
            selftest,
            endpoint,
            ended,
            children: Vec::new(),
        })));
        let s = sup.clone();
        granite::thread::spawn(move || s.run(&receiver))
            .map_err(|e| format!("starting the supervisor's thread: {e}"))?;
        Ok(sup)
    }

    /// Starts `kind` with `args`: for the first time, or again after it was retired.
    pub fn start(&self, kind: Kind, args: &str) -> Result<(), String> {
        self.0.lock().start_child(kind, args)
    }

    /// The console's arguments if it is started in the display server's place (the desktop
    /// boot's test has it mirror the screen to the serial port, as the self-test does).
    pub fn set_fallback_args(&self, args: &str) {
        self.0.lock().fallback_args = String::from(args);
    }

    /// What the power service is to be started with (from `power::caps`).
    pub fn set_power(&self, caps: Vec<(u32, Handle, u32)>, args: String) {
        let mut g = self.0.lock();
        g.power = caps;
        g.power_args = args;
    }

    /// The thread: serves `pbr.supervisor`, notices children ending, and restarts them.
    fn run(&self, ep: &Endpoint) {
        let mut buffer = vec![0u8; 1024];
        loop {
            let wake = {
                let g = self.0.lock();
                g.children
                    .iter()
                    .filter_map(|c| c.due)
                    .min()
                    .map_or(time::FOREVER, |t| time::after_ms(t.saturating_sub(now_ms())))
            };
            match ep.receive(&mut buffer, wake) {
                Ok(Event::Call(msg, reply)) => {
                    let mut calls = Calls(self);
                    if let Some((_, reply)) = supervisor::dispatch(&mut calls, msg, &buffer, reply)
                    {
                        granite::interface::unknown(reply);
                    }
                }
                Ok(Event::Signal(bits)) => self.0.lock().ended(bits),
                Ok(Event::Gone(_)) | Err(granite::Error::TimedOut) => {}
                Err(e) => {
                    granite::println!("supervisor: receive failed: {e}");
                    time::sleep_ms(10);
                }
            }
            self.0.lock().restart_due();
        }
    }

    // --------------------------------------------------------------------------------------
    // For the root program's own use (and the self-test).

    /// A client capability for `kind`'s running instance, carrying `badge` (0 for none),
    /// waiting up to `secs` seconds for one to be running.
    pub fn client(&self, kind: Kind, badge: u64, secs: u64) -> Result<Endpoint, String> {
        let end = time::after_ms(secs * 1000);
        loop {
            {
                let g = self.0.lock();
                if let Some(ep) = g.child(kind).and_then(|c| c.endpoint.as_ref()) {
                    let made = if badge == 0 {
                        ep.0.duplicate(CLIENT).map(Endpoint)
                    } else {
                        ep.mint(badge, CLIENT)
                    };
                    return made.map_err(|e| format!("a capability for {}: {e}", kind.program()));
                }
            }
            if time::now() > end {
                return Err(format!("{} isn't running", kind.program()));
            }
            time::sleep_ms(20);
        }
    }

    /// The running instance's process (to watch and ask about, and stop).
    pub fn process(&self, kind: Kind) -> Option<Process> {
        let g = self.0.lock();
        let p = &g.child(kind)?.running.as_ref()?.process;
        p.0.duplicate(rights::INSPECT | rights::WATCH | rights::STOP).ok().map(Process)
    }

    /// How many times `kind` has been started.
    pub fn generation(&self, kind: Kind) -> u32 {
        self.0.lock().child(kind).map_or(0, |c| c.generation)
    }

    /// Waits up to `secs` seconds for `kind` to have been started more than `generation`
    /// times, and to be running.
    pub fn wait_started(&self, kind: Kind, generation: u32, secs: u64) -> Result<u32, String> {
        let end = time::after_ms(secs * 1000);
        loop {
            {
                let g = self.0.lock();
                if let Some(c) = g.child(kind)
                    && c.generation > generation
                    && c.running.is_some()
                {
                    return Ok(c.generation);
                }
            }
            if time::now() > end {
                return Err(format!("{} wasn't started again in {secs} seconds", kind.program()));
            }
            time::sleep_ms(20);
        }
    }

    /// Stops `kind`'s running instance as a crash would, so that it is restarted (self-test).
    pub fn crash(&self, kind: Kind) -> Result<(), String> {
        let g = self.0.lock();
        let c = g.child(kind).ok_or("not started")?;
        let p = &c.running.as_ref().ok_or("not running")?.process;
        p.stop(9).map_err(|e| format!("stopping {}: {e}", kind.program()))
    }

    /// Stops `kind` for good (until it is started again), and waits for it to end.
    pub fn retire(&self, kind: Kind) -> Result<(), String> {
        let process = {
            let mut g = self.0.lock();
            let Some(c) = g.children.iter_mut().find(|c| c.kind == kind) else { return Ok(()) };
            c.retired = true;
            c.due = None;
            c.endpoint = None;
            c.running.take()
        };
        let Some(s) = process else { return Ok(()) };
        let n = Notification::create().map_err(|e| format!("{e}"))?;
        let started = time::now();
        let _ = s.process.watch(&n, 1);
        let _ = s.process.stop(0);
        let _ = n.wait(time::after_ms(30_000));
        let ms = time::now().saturating_sub(started) / 1_000_000;
        // A process ends in two steps (note 0013, section 3): its threads stop and what it held
        // is closed (it is "ending"), then, once everything tied to it has ended too, it has
        // ended. Which step it is stuck at says where to look.
        match s.process.info() {
            Ok(i) if i.state == State::Running => Err(format!(
                "{} didn't stop in 30 seconds: still running, with {} threads",
                kind.program(),
                i.threads
            )),
            Ok(i) if i.state == State::Ending && i.threads > 0 => Err(format!(
                "{} didn't stop in 30 seconds: ending, but {} of its threads haven't stopped",
                kind.program(),
                i.threads
            )),
            Ok(i) if i.state == State::Ending => Err(format!(
                "{} didn't stop in 30 seconds: its threads have stopped, but it hasn't been \
                 closed down, or something tied to it hasn't ended",
                kind.program()
            )),
            _ => {
                granite::println!("supervisor: {} stopped for good in {ms} ms.", kind.program());
                Ok(())
            }
        }
    }

    /// The generations whose memory is kept for `kind`.
    pub fn kept(&self, kind: Kind) -> Vec<u32> {
        self.0.lock().child(kind).map_or(Vec::new(), |c| c.kept.iter().map(|(g, _)| *g).collect())
    }

    /// The last `release_kept` from `kind`: which generation, and when (nanoseconds).
    pub fn released(&self, kind: Kind) -> Option<(u32, u64)> {
        self.0.lock().child(kind).and_then(|c| c.released)
    }
}

impl Inner {
    fn child(&self, kind: Kind) -> Option<&Child> {
        self.children.iter().find(|c| c.kind == kind)
    }

    /// Starts child `i` (another generation).
    fn spawn(&mut self, i: usize) -> Result<(), String> {
        let kind = self.children[i].kind;
        let generation = self.children[i].generation + 1;
        let program = self
            .archive
            .find(kind.program())
            .ok_or(format!("{} isn't in the archive", kind.program()))?;
        let fail = |e: granite::Error| format!("starting {}: {e}", kind.program());
        // Its endpoint is moved to it; this keeps a copy that can only send and mint.
        let own = Endpoint::create().map_err(fail)?;
        let minter = Endpoint(own.0.duplicate(CLIENT).map_err(fail)?);
        let parent = self
            .endpoint
            .mint(badge(kind, generation), rights::SEND | rights::TRANSFER)
            .map_err(fail)?;
        let (own_rights, archive_rights) = match kind {
            // The device manager only receives on its endpoint.
            Kind::Devmgr => (rights::RECEIVE, Some(rights::READ | rights::MAP)),
            Kind::Storage => (rights::ALL, Some(rights::READ | rights::MAP)),
            // The users service passes the archive on to its sandbox managers.
            Kind::Users => (rights::ALL, Some(rights::READ | rights::MAP | CLIENT)),
            // The console starts no programs, so it needs no archive.
            Kind::Console => (rights::ALL, None),
            // The input service reads its keyboard layouts from the archive.
            Kind::Input => (rights::ALL, Some(rights::READ | rights::MAP)),
            // The display server starts no programs and reads nothing from the archive.
            Kind::Display => (rights::ALL, None),
            // Nor does the power service.
            Kind::Power => (rights::ALL, None),
        };
        let mut caps = vec![
            (role::OWN, own.0.into_raw(), own_rights, true),
            (role::PARENT, parent.0.into_raw(), rights::SEND, true),
        ];
        if let Some(r) = archive_rights {
            caps.push((role::ARCHIVE, self.archive_mem, r, false));
        }
        if kind == Kind::Devmgr {
            for (r, h) in &self.devices {
                caps.push((*r, h.raw(), rights::ALL, false));
            }
            // The right to take the screen from the kernel's text console goes to the device
            // manager, for the display driver alone (note 0010, section 7). Every other
            // program's log is given without it (`Spawn::log`).
            if granite::log::handle() != 0 {
                // (With `DUPLICATE` and `TRANSFER`, so that it can give the driver a copy, which
                // has nothing but `CONTROL`.)
                let mask = rights::CONTROL | rights::DUPLICATE | rights::TRANSFER;
                caps.push((DEVMGR_LOG_CONTROL, granite::log::handle(), mask, false));
            }
        }
        if kind == Kind::Power {
            for (r, h, mask) in &self.power {
                caps.push((*r, h.raw(), *mask, false));
            }
        }
        let c = &mut self.children[i];
        let mut args = c.args.clone();
        if kind == Kind::Power && !self.power_args.is_empty() {
            args = if args.is_empty() {
                self.power_args.clone()
            } else {
                format!("{args} {}", self.power_args)
            };
        }
        if self.selftest && !args.split_whitespace().any(|w| w == "selftest") {
            args =
                if args.is_empty() { String::from("selftest") } else { format!("selftest {args}") };
        }
        let spawned = process::spawn(&Spawn {
            name: kind.program(),
            program: program.data,
            budget: kind.budget(),
            priority: 2,
            caps: &caps,
            log: true,
            args: &args,
            tied: false,
            file_tree: &[],
        })
        .map_err(|e| {
            let (used, limit) =
                process::info(process::self_handle()).map_or((0, 0), |i| (i.used, i.budget));
            format!(
                "starting {}: {e} (the root program's memory: {used} of {limit})",
                kind.program()
            )
        })?;
        spawned.process.watch(&self.ended, 1 << kind.index()).map_err(fail)?;
        c.history.started(now_ms());
        c.generation = generation;
        c.running = Some(spawned);
        c.endpoint = Some(minter);
        c.due = None;
        if generation > 1 {
            granite::println!(
                "supervisor: {} started again ({} restarts).",
                kind.program(),
                c.restarts
            );
        }
        Ok(())
    }

    /// Children whose bit is in `bits` may have ended.
    fn ended(&mut self, bits: u64) {
        let mut fall_back = false;
        for c in &mut self.children {
            if bits & (1 << c.kind.index()) == 0 {
                continue;
            }
            let Some(s) = &c.running else { continue };
            let state = s.process.info().map(|i| i.state);
            if matches!(state, Ok(State::Running | State::Ending)) {
                continue;
            }
            // Its clients' capabilities fail now: nothing can receive on its endpoint.
            c.running = None;
            c.endpoint = None;
            granite::println!("supervisor: {} ended: {state:?}.", c.kind.program());
            if c.retired {
                continue;
            }
            match c.history.crashed(now_ms()) {
                Decision::Restart { delay_ms } => {
                    c.restarts += 1;
                    c.due = Some(now_ms() + delay_ms);
                }
                Decision::GiveUp => {
                    c.retired = true;
                    granite::println!(
                        "supervisor: {} ended too often; it is left stopped.",
                        c.kind.program()
                    );
                    if c.kind == Kind::Display && self.fallback {
                        fall_back = true;
                    }
                }
            }
        }
        // Without the display server, the console, so that the user gets the shell (note
        // 0024, section 8). The users service hears that the display server has gone, finds
        // the console instead, and starts the session again on its terminal.
        if fall_back {
            granite::println!(
                "supervisor: the display server was given up on, so the console is started \
                 in its place: the shell instead of the desktop until the next start."
            );
            let args = self.fallback_args.clone();
            if let Err(e) = self.start_child(Kind::Console, &args) {
                granite::println!("supervisor: can't start the console: {e}");
            }
        }
    }

    /// Starts `kind`: for the first time, or again after it was retired.
    fn start_child(&mut self, kind: Kind, args: &str) -> Result<(), String> {
        if !self.children.iter().any(|c| c.kind == kind) {
            self.children.push(Child {
                kind,
                args: String::new(),
                generation: 0,
                running: None,
                endpoint: None,
                history: History::new(),
                due: None,
                restarts: 0,
                retired: false,
                kept: Vec::new(),
                released: None,
            });
        }
        let i = self.children.iter().position(|c| c.kind == kind).ok_or("no child")?;
        if self.children[i].running.is_some() {
            return Err(format!("{} is already running", kind.program()));
        }
        self.children[i].args = String::from(args);
        self.children[i].retired = false;
        self.children[i].history = History::new();
        self.spawn(i)
    }

    /// Starts the children whose restart is due.
    fn restart_due(&mut self) {
        let now = now_ms();
        for i in 0..self.children.len() {
            if self.children[i].due.is_some_and(|t| t <= now) {
                self.children[i].due = None;
                if let Err(e) = self.spawn(i) {
                    granite::println!("supervisor: {e}");
                    self.children[i].retired = true;
                }
            }
        }
    }
}

/// `pbr.supervisor`, answering one call; the caller is known by its badge.
struct Calls<'a>(&'a Supervisor);

/// The child calling, if its badge is for its running instance.
fn caller(g: &Inner, badge: u64) -> Option<usize> {
    let (kind, generation) = from_badge(badge)?;
    g.children
        .iter()
        .position(|c| c.kind == kind && c.generation == generation && c.running.is_some())
}

fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("supervisor: answer failed: {e}");
    }
}

impl supervisor::Server for Calls<'_> {
    fn lookup(&mut self, name: String, reply: supervisor::lookup::Reply) {
        let result = {
            let g = self.0.0.lock();
            match caller(&g, reply.badge()).and_then(|i| rule(g.children[i].kind, &name)) {
                None => Err(SupervisorError::NotAllowed),
                Some((target, badge)) => match g.child(target) {
                    // Never started in this boot, or given up on: asking again won't help.
                    None => Err(SupervisorError::Stopped),
                    Some(c) if c.retired && c.running.is_none() => Err(SupervisorError::Stopped),
                    Some(c) => match &c.endpoint {
                        None => Err(SupervisorError::NotReady),
                        Some(ep) => ep.mint(badge, CLIENT).map_err(|_| SupervisorError::NotReady),
                    },
                },
            }
        };
        answered(match result {
            Ok(ep) => reply.ok(ep.0),
            Err(e) => reply.err(e),
        });
    }

    fn keep(&mut self, memory: Memory, reply: supervisor::keep::Reply) {
        let result = {
            let mut g = self.0.0.lock();
            match caller(&g, reply.badge()) {
                Some(i) if g.children[i].kind == Kind::Devmgr => {
                    let c = &mut g.children[i];
                    if c.kept.len() >= MAX_KEPT {
                        Err(SupervisorError::TooMany)
                    } else {
                        c.kept.push((c.generation, memory));
                        Ok(())
                    }
                }
                _ => Err(SupervisorError::NotAllowed),
            }
        };
        answered(match result {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }

    fn release_kept(&mut self, reply: supervisor::release_kept::Reply) {
        let result = {
            let mut g = self.0.0.lock();
            match caller(&g, reply.badge()) {
                Some(i) => {
                    let c = &mut g.children[i];
                    let generation = c.generation;
                    let before = c.kept.len();
                    // Dropping a `Memory` closes the capability: once the device manager's own
                    // copies have gone with it, the memory is freed.
                    c.kept.retain(|(g, _)| *g >= generation);
                    c.released = Some((generation, time::now()));
                    if before > c.kept.len() {
                        granite::println!(
                            "supervisor: released {} memory objects kept for the previous {}.",
                            before - c.kept.len(),
                            c.kind.program()
                        );
                    }
                    Ok(())
                }
                None => Err(SupervisorError::NotAllowed),
            }
        };
        answered(match result {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        });
    }
}
