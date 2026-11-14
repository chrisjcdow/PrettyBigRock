// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The storage service (docs/design/0015-storage-and-the-block-cache.md): the only program
//! that talks to disk drivers. It finds the volumes on every disk, keeps the one block cache
//! all filesystems share, and gives each filesystem server a capability for its own volume
//! alone.
//!
//! It serves three interfaces on one endpoint, told apart by the badge of the capability a
//! request comes through (a badge is a number the service stamps on each capability it hands
//! out; note 0008):
//! - [`ADMIN`]: `pbr.storage`, for the root program (and, from stage 6, the users service);
//! - [`DEBUG`]: `pbr.storage.debug`, only in self-test runs;
//! - [`VOLUME_BASE`] + a volume's number: `pbr.block` for that volume, with sector numbers
//!   counted from the volume's start, so nothing outside it can be named;
//! - [`HOST_BASE`] + a volume's number: `pbr.fs.host`, for the filesystem server it started
//!   for that volume.
//!
//! It starts a filesystem server for each RockFS and FAT32 volume (note 0016, section 1), one per volume,
//! each reaching only its own volume. The server says when the volume is ready, and from then
//! `open` hands out copies of its root folder's capability.
//!
//! It handles one request at a time (note 0015, section 7). That shapes shutting down: a
//! server saves its volume by writing through this service, so this service must never wait
//! for a server (the server may at that moment be waiting for it). It asks each server to
//! unmount by signalling a notification, which never waits, carries on serving their writes,
//! and answers `shutdown` once every server has exited.

#![no_std]
#![no_main]

extern crate alloc;

mod disks;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use disks::{Disks, Raw, SECTOR};
use granite::endpoint::{Endpoint, Event};
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::{self, Spawn, Spawned, State, role};
use granite::sys::{perm, rights};
use granite::time;
use pbr_archive::Archive;
use pbr_blockcache::{Cache, Error as CacheError};
use pbr_interfaces::pbr_block::{self as block, BlockError, BlockInfo};
use pbr_interfaces::pbr_fs_host::{self as host, FsFailure, HostError};
use pbr_interfaces::pbr_storage::{
    self as storage, Filesystem, StorageError, VolumeInfo, VolumeState,
};
use pbr_interfaces::pbr_storage_debug::{self as debug, DebugError};
use pbr_supervise::{Decision, History};

granite::main!(main);

/// Start-up roles: this service's endpoint (with every right, so it can mint capabilities from
/// it), the start-up archive (for the filesystem servers' programs) and its supervisor's
/// `pbr.supervisor` (role 5), through which it finds the device manager (note 0018, section 2).
const OWN: u32 = role::OWN;
/// The roles a filesystem server gets its volume's `pbr.block` and its stop notification at
/// (`services/rockfs`, `services/fat`).
const SERVER_BLOCK: u32 = 1003;
const SERVER_STOP: u32 = 1004;
/// The bit that asks a server to unmount (`pbr_fsserve::serve::STOP`).
const STOP_BIT: u64 = 1;

/// The badge of the capability for `pbr.storage`, which the root program mints from this
/// service's endpoint.
pub const ADMIN: u64 = 1;
/// The badge for `pbr.storage.debug` (self-test runs only).
pub const DEBUG: u64 = 2;
/// A volume's `pbr.block` capability carries this plus the volume's number.
pub const VOLUME_BASE: u64 = 0x1000_0000;
/// The `pbr.fs.host` capability of the server for a volume carries this plus its number.
pub const HOST_BASE: u64 = 0x2000_0000;
/// Volume numbers stay below this, so the badge ranges don't overlap.
const MAX_VOLUME: u64 = 0x1000_0000;

/// The volume number in a badge from the range starting at `base`.
fn in_range(badge: u64, base: u64) -> Option<u32> {
    let id = badge.checked_sub(base)?;
    (id > 0 && id < MAX_VOLUME).then_some(id as u32)
}

/// Each filesystem server's budget.
const SERVER_BUDGET: u64 = 8 * 1024 * 1024;
/// How long servers have to save their volumes when shutting down.
const SHUTDOWN_MS: u64 = 30_000;

/// The cache's size: 16 MiB in M5, while the QEMU tests run with 512 MiB (note 0015).
const CACHE_PAGES: usize = 4096;
/// Pages dirty for longer than this are written back even if nobody flushes.
const WRITE_BACK_MS: u64 = 30_000;
/// How often to look for them.
const TICK_MS: u64 = 5_000;
/// The largest volume in memory.
const MAX_RAM_VOLUME: u64 = 256 * 1024 * 1024;

/// One volume.
struct Volume {
    id: u32,
    disk: u32,
    partition: u32,
    start: u64,
    sectors: u64,
    filesystem: Filesystem,
    state: VolumeState,
    label: String,
    read_only: bool,
    in_memory: bool,
    /// Its filesystem server, while one runs.
    server: Option<Server>,
    /// Its server's crashes, for the restart policy (`lib/supervise`).
    history: History,
    /// When its server is due to be started again (milliseconds), after a crash.
    due: Option<u64>,
    /// A `check` under way (note 0026, section 2).
    check: Option<Check>,
}

/// A `check` of a volume: first its server is asked to unmount, then a server is started to
/// check it, which reports and ends; then the volume is served again and the caller answered.
struct Check {
    repair: bool,
    reply: storage::check::Reply,
    /// What the checking server reported.
    report: Option<(String, u32)>,
}

/// A filesystem server this service started.
struct Server {
    spawned: Spawned,
    /// Signalled to have it unmount.
    stop: Notification,
    /// The volume's root folder, once the server has said it is ready.
    root: Option<Endpoint>,
    /// Its `pbr.fs.control`. This service doesn't call it (it must never wait for a server),
    /// but keeps it: it is the server's, and tests may ask for its records through it later.
    _control: Option<Endpoint>,
    /// Asked to unmount.
    unmounting: bool,
    /// It said it couldn't serve the volume (damaged, or not a filesystem it knows): ending
    /// then is no crash, and it isn't restarted.
    gave_up: bool,
    /// It was started to check the volume, not to serve it.
    checker: bool,
}

/// A `shutdown` waiting for servers to finish saving.
struct ShuttingDown {
    reply: storage::shutdown::Reply,
    deadline: u64,
    /// A server didn't save its volume.
    failed: bool,
}

/// Everything the service keeps.
struct Storage {
    disks: Disks,
    cache: Cache,
    volumes: Vec<Volume>,
    endpoint: Endpoint,
    shut_down: bool,
    next_id: u32,
    /// The filesystem servers' programs, from the start-up archive.
    rockfs: Option<Vec<u8>>,
    fat: Option<Vec<u8>>,
    shutting_down: Option<ShuttingDown>,
}

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("storage: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let selftest = start.args().split_whitespace().any(|w| w == "selftest");
    let endpoint = Endpoint(start.take(OWN).ok_or("started without an endpoint to serve")?);
    let mut parent = pbr_interfaces::pbr_supervisor::Client::new(Endpoint(
        start.take(role::PARENT).ok_or("started without a supervisor")?,
    ));
    parent.set_timeout_ms(10_000);
    let [rockfs, fat] = match start.take(role::ARCHIVE) {
        Some(h) => programs(Memory(h), ["rockfs", "fat"]),
        None => [None, None],
    };
    let mut s = Storage {
        disks: Disks::new(parent),
        cache: Cache::new(CACHE_PAGES),
        volumes: Vec::new(),
        endpoint,
        shut_down: false,
        next_id: 1,
        rockfs,
        fat,
        shutting_down: None,
    };
    for (name, p) in [("RockFS", &s.rockfs), ("FAT32", &s.fat)] {
        if p.is_none() {
            granite::println!("storage: no {name} server in the start-up archive.");
        }
    }
    s.disks.find();
    let ids: Vec<u32> = s.disks.real.keys().copied().collect();
    for id in ids {
        s.scan(id);
    }
    let ids: Vec<u32> = s.volumes.iter().map(|v| v.id).collect();
    for id in ids {
        s.start_server(id, None);
    }
    serve(&mut s, selftest);
    Ok(())
}

/// How a server ended: its exit status, or `None` if it was stopped or crashed. Its `pbr.fs.host`
/// capability goes as its handles are closed, which can be just before it has finished ending,
/// so this waits (briefly) for that.
fn exit_status(s: &Spawned) -> Option<u64> {
    let state = || s.process.info().map(|i| i.state).ok();
    if matches!(state(), Some(State::Running | State::Ending)) {
        // `watch` signals the notification when the process has ended.
        let n = Notification::create().ok()?;
        s.process.watch(&n, 1).ok()?;
        let _ = n.wait(time::after_ms(2_000));
    }
    match state() {
        Some(State::Exited(n)) => Some(n),
        _ => None,
    }
}

/// Copies of the programs called `names` in the start-up archive. (They are copied so the
/// archive can be unmapped.) `N` is a *const generic*: the function works for an array of any
/// length, known when it is compiled.
fn programs<const N: usize>(archive: Memory, names: [&str; N]) -> [Option<Vec<u8>>; N] {
    let Ok(mapping) = archive.map(perm::READ) else { return [const { None }; N] };
    // SAFETY: the mapping is readable for its whole length while `mapping` lives, which is
    // longer than `bytes` is used, and nothing writes to it.
    let bytes = unsafe { core::slice::from_raw_parts(mapping.as_ptr(), mapping.len() as usize) };
    let Ok(archive) = Archive::parse(bytes) else { return [const { None }; N] };
    names.map(|name| archive.find(name).map(|f| f.data.to_vec()))
}

impl Storage {
    /// Reads disk `disk`'s partition table, and adds its volumes.
    fn scan(&mut self, disk: u32) {
        let sectors = self.disks.sectors(disk);
        let in_memory = self.disks.ram.contains_key(&disk);
        let read_only = self.disks.real.get(&disk).is_some_and(|d| d.read_only);
        let name = self.disks.real.get(&disk).map_or(String::from("memory"), |d| d.name.clone());
        if !in_memory {
            self.cache.add_disk(disk, sectors);
        }
        let found = match pbr_partition::scan(&mut Raw { disks: &mut self.disks, id: disk }) {
            Ok(f) => f,
            Err(_) => {
                granite::println!("storage: disk {disk} ({name}) can't be read.");
                return;
            }
        };
        granite::println!(
            "storage: disk {disk} ({name}, {} MiB, {:?}): {} volumes.",
            sectors * SECTOR as u64 / (1024 * 1024),
            found.table,
            found.volumes.len()
        );
        for p in &found.problems {
            granite::println!("storage: disk {disk}: {p:?}");
        }
        for v in found.volumes {
            let filesystem = match v.filesystem {
                pbr_partition::Filesystem::RockFs => Filesystem::RockFs,
                pbr_partition::Filesystem::Fat32 => Filesystem::Fat32,
                pbr_partition::Filesystem::Empty => Filesystem::Empty,
                pbr_partition::Filesystem::Unknown => Filesystem::Unknown,
            };
            self.add_volume(disk, v.partition, v.start, v.sectors, filesystem, v.label, read_only);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_volume(
        &mut self,
        disk: u32,
        partition: u32,
        start: u64,
        sectors: u64,
        filesystem: Filesystem,
        label: String,
        read_only: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let in_memory = self.disks.ram.contains_key(&disk);
        if !in_memory && self.cache.add_volume(id, disk, start, sectors).is_err() {
            // `lib/partition` only reports volumes on the disk, so this can't happen.
            granite::println!("storage: volume {id} doesn't fit on disk {disk}");
        }
        // Until a server starts for it (`start_server`), a volume isn't served: RockFS and
        // FAT32 volumes get one at start-up, empty ones when formatted.
        let state = VolumeState::Unsupported;
        granite::println!(
            "storage: volume {id}: disk {disk} partition {partition}, {} KiB, {filesystem:?} \"{label}\".",
            sectors * SECTOR as u64 / 1024
        );
        self.volumes.push(Volume {
            id,
            disk,
            partition,
            start,
            sectors,
            filesystem,
            state,
            label,
            read_only,
            in_memory,
            server: None,
            history: History::new(),
            due: None,
            check: None,
        });
        id
    }

    fn volume_mut(&mut self, id: u32) -> Option<&mut Volume> {
        self.volumes.iter_mut().find(|v| v.id == id)
    }

    /// Starts a filesystem server for volume `id` if it holds RockFS or FAT32, or, with
    /// `format`, puts a new RockFS volume labelled `format` on it first. With `check`, a RockFS
    /// server is started to check the volume (and repair it, with `Some(true)`) and report. The server gets three capabilities: an
    /// endpoint of its own; the volume's `pbr.block`; and `pbr.fs.host` here, under a *watched*
    /// badge (note 0013), so that when the server ends for any reason this service hears of it.
    fn start_server(&mut self, id: u32, format: Option<&str>) {
        self.start_server_for(id, format, None);
    }

    fn start_server_for(&mut self, id: u32, format: Option<&str>, check: Option<bool>) {
        let Some(v) = self.volume(id) else { return };
        if v.server.is_some() {
            return;
        }
        let (name, program) = match (format, v.filesystem) {
            (Some(_), _) | (None, Filesystem::RockFs) => ("rockfs", self.rockfs.as_deref()),
            (None, Filesystem::Fat32) => ("fat", self.fat.as_deref()),
            _ => return,
        };
        let Some(program) = program else { return };
        let read_only = v.read_only;
        let args = match (format, check) {
            (_, Some(true)) => alloc::format!("volume {id} check repair"),
            (_, Some(false)) => alloc::format!("volume {id} check"),
            (Some(label), None) => alloc::format!("volume {id} format {label}"),
            (None, None) if read_only => alloc::format!("volume {id} readonly"),
            (None, None) => alloc::format!("volume {id}"),
        };
        let stop = match Notification::create() {
            Ok(n) => n,
            Err(e) => {
                granite::println!("storage: volume {id}: can't start its server: {e}");
                return;
            }
        };
        let caps = (|| {
            let own = Endpoint::create()?;
            // (A capability can only be given to another process if it carries the right to be
            // transferred; the server's copy then has only SEND.)
            let host = self
                .endpoint
                .mint_watched(HOST_BASE + u64::from(id), rights::SEND | rights::TRANSFER)?;
            let block = self.volume_cap(id)?;
            Ok::<_, granite::Error>([
                (role::OWN, own.0.into_raw(), rights::ALL, true),
                (role::PARENT, host.0.into_raw(), rights::SEND, true),
                (SERVER_BLOCK, block.0.into_raw(), rights::SEND | rights::TRANSFER, true),
                (SERVER_STOP, stop.0.raw(), rights::ALL, false),
            ])
        })();
        // A *tied* process (note 0013) is stopped if this service ends: no server outlives
        // the service whose cache its volume goes through.
        let started = caps.map_err(|e| alloc::format!("{e}")).and_then(|caps| {
            process::spawn(&Spawn {
                name,
                program,
                budget: SERVER_BUDGET,
                priority: 2,
                caps: &caps,
                log: true,
                args: &args,
                tied: true,
                file_tree: &[],
            })
            .map_err(|e| alloc::format!("{e}"))
        });
        let Some(v) = self.volume_mut(id) else { return };
        match started {
            Ok(spawned) => {
                v.state = VolumeState::Starting;
                v.history.started(now_ms());
                v.server = Some(Server {
                    spawned,
                    stop,
                    root: None,
                    _control: None,
                    unmounting: false,
                    gave_up: false,
                    checker: check.is_some(),
                });
            }
            Err(e) => {
                granite::println!("storage: volume {id}: can't start its server: {e}");
                v.state = VolumeState::Failed;
            }
        }
    }

    /// Volume `id`'s server has ended (its `pbr.fs.host` capability went). One that crashed is
    /// started again on the same volume, with the restart policy every supervisor uses (note
    /// 0018, section 3). Every capability for the volume failed with it (nothing can receive on
    /// its server's endpoint any more), so whoever held the volume's root asks for it again; a
    /// RockFS volume left unclean is checked when the new server mounts it.
    fn server_gone(&mut self, id: u32) {
        let stopping = self.shut_down || self.shutting_down.is_some();
        let Some(v) = self.volume_mut(id) else { return };
        let Some(server) = v.server.take() else { return };
        if server.checker {
            return self.checked(id);
        }
        let status = exit_status(&server.spawned);
        // A server that never mounted its volume (it said it couldn't) had nothing to save.
        let served = server.root.is_some();
        let saved = !served || (server.unmounting && status == Some(0));
        if server.unmounting && served {
            if saved {
                granite::println!("storage: volume {id} saved and unmounted.");
            } else {
                let state = server.spawned.process.info().map(|i| i.state);
                granite::println!("storage: volume {id} wasn't saved cleanly ({state:?}).");
            }
        } else if !server.gave_up && !stopping {
            match v.history.crashed(now_ms()) {
                Decision::Restart { delay_ms } => {
                    granite::println!(
                        "storage: volume {id}'s server stopped ({status:?}); starting it again."
                    );
                    v.state = VolumeState::Starting;
                    v.due = Some(now_ms() + delay_ms);
                }
                Decision::GiveUp => {
                    granite::println!(
                        "storage: volume {id}'s server stopped ({status:?}) too often; the volume fails."
                    );
                    v.state = VolumeState::Failed;
                }
            }
        } else if v.state != VolumeState::Damaged && v.state != VolumeState::Unsupported {
            granite::println!("storage: volume {id}'s server stopped ({status:?}).");
            v.state = VolumeState::Failed;
        }
        if !saved && let Some(sd) = &mut self.shutting_down {
            sd.failed = true;
        }
        // A check was waiting for this server to go: now the volume can be checked.
        if let Some(repair) = self.volume(id).and_then(|v| v.check.as_ref()).map(|c| c.repair) {
            if let Some(v) = self.volume_mut(id) {
                v.due = None;
                v.state = VolumeState::Starting;
            }
            self.start_server_for(id, None, Some(repair));
            if self.volume(id).is_some_and(|v| v.server.is_none()) {
                self.checked(id);
            }
        }
    }

    /// Starts a `check` of volume `id` (note 0026, section 2): its server is asked to unmount;
    /// once it has gone (`server_gone`), the checking server is started. A volume with no
    /// server running (damaged, or failed too often) is checked at once.
    fn check(&mut self, id: u32, repair: bool, reply: storage::check::Reply) {
        let stopping = self.shut_down || self.shutting_down.is_some();
        let early = match self.volume(id) {
            None => Err(StorageError::NoSuchVolume),
            Some(_) if stopping => Err(StorageError::ShutDown),
            Some(v) if v.check.is_some() => Err(StorageError::NotReady),
            Some(v) if v.filesystem != Filesystem::RockFs || (repair && v.read_only) => {
                Err(StorageError::Unsupported)
            }
            Some(v) if self.disks.real.get(&v.disk).is_some_and(|d| d.failed) => {
                Err(StorageError::DiskError)
            }
            Some(_) => Ok(()),
        };
        if let Err(e) = early {
            return answered(reply.err(e));
        }
        let Some(v) = self.volume_mut(id) else { return };
        granite::println!(
            "storage: volume {id}: {} it.",
            if repair { "checking and repairing" } else { "checking" }
        );
        v.check = Some(Check { repair, reply, report: None });
        v.state = VolumeState::Starting;
        match &mut v.server {
            Some(server) => {
                if !server.unmounting {
                    if let Err(e) = server.stop.signal(STOP_BIT) {
                        granite::println!("storage: volume {id}: can't ask it to unmount: {e}");
                    }
                    server.unmounting = true;
                }
            }
            None => {
                v.due = None;
                self.start_server_for(id, None, Some(repair));
                if self.volume(id).is_some_and(|v| v.server.is_none()) {
                    self.checked(id);
                }
            }
        }
    }

    /// The checking server for volume `id` has ended (or couldn't start): answers the `check`
    /// with its report, and serves the volume again. Its restart history starts afresh: it is a
    /// new start, not a crash.
    fn checked(&mut self, id: u32) {
        let Some(v) = self.volume_mut(id) else { return };
        let Some(c) = v.check.take() else { return };
        v.history = History::new();
        v.state = VolumeState::Starting;
        answered(match c.report {
            Some((report, problems)) => c.reply.ok(&report, problems),
            None => c.reply.err(StorageError::DiskError),
        });
        self.start_server(id, None);
    }

    /// Asks every server to unmount, without waiting: each saves its volume, marks it clean
    /// and exits, which this service hears of as its `pbr.fs.host` capability going. (A server
    /// still starting sees the signal once it serves, and unmounts then.)
    fn unmount_all(&mut self) {
        for v in &mut self.volumes {
            let Some(server) = &mut v.server else { continue };
            if !server.unmounting {
                if let Err(e) = server.stop.signal(STOP_BIT) {
                    granite::println!("storage: volume {}: can't ask it to unmount: {e}", v.id);
                }
                server.unmounting = true;
            }
        }
    }

    /// Finishes a `shutdown` once every server has gone, or its time is up.
    fn finish_shutdown(&mut self) {
        let Some(sd) = &self.shutting_down else { return };
        let waiting = self.volumes.iter().filter(|v| v.server.is_some()).count();
        if waiting > 0 && time::now() < sd.deadline {
            return;
        }
        let Some(sd) = self.shutting_down.take() else { return };
        let flushed = self.cache.flush_all(&mut self.disks);
        self.shut_down = true;
        let ok = waiting == 0 && !sd.failed && flushed.is_ok();
        granite::println!(
            "storage: shut down{}.",
            if ok { "" } else { ", but not every volume was saved" }
        );
        answered(if ok { sd.reply.ok() } else { sd.reply.err(StorageError::DiskError) })
    }

    fn volume(&self, id: u32) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.id == id)
    }

    /// Starts the servers whose restart is due.
    fn restart_due(&mut self) {
        let now = now_ms();
        let due: Vec<u32> =
            self.volumes.iter().filter(|v| v.due.is_some_and(|t| t <= now)).map(|v| v.id).collect();
        for id in due {
            if let Some(v) = self.volume_mut(id) {
                v.due = None;
            }
            self.start_server(id, None);
        }
    }

    /// When the next server restart is due (nanoseconds on the kernel's clock), if any.
    fn next_due(&self) -> Option<u64> {
        self.volumes.iter().filter_map(|v| v.due).min().map(|ms| ms * 1_000_000)
    }

    /// A capability for volume `id`'s `pbr.block`. It can be sent (and moved, which is how it
    /// travels in a message), but not copied: it reaches only that one volume anyway.
    fn volume_cap(&self, id: u32) -> granite::Result<Endpoint> {
        self.endpoint.mint(VOLUME_BASE + u64::from(id), rights::SEND | rights::TRANSFER)
    }
}

fn now_ms() -> u64 {
    time::now() / 1_000_000
}

/// Answers requests for ever, writing old dirty pages back between them.
fn serve(s: &mut Storage, selftest: bool) {
    let ep = Endpoint(match s.endpoint.0.duplicate(rights::ALL) {
        Ok(h) => h,
        Err(e) => {
            granite::println!("storage: can't copy its endpoint: {e}");
            return;
        }
    });
    let mut buffer = vec![0u8; 65536];
    let mut next_tick = time::after_ms(TICK_MS);
    granite::println!("storage: serving {} volumes.", s.volumes.len());
    loop {
        let mut wake = s.shutting_down.as_ref().map_or(next_tick, |sd| next_tick.min(sd.deadline));
        if let Some(t) = s.next_due() {
            wake = wake.min(t);
        }
        match ep.receive(&mut buffer, wake) {
            Ok(Event::Call(msg, reply)) => {
                let badge = msg.badge;
                let unknown = if badge == ADMIN {
                    storage::dispatch(&mut Admin(s), msg, &buffer, reply)
                } else if badge == DEBUG && selftest {
                    debug::dispatch(&mut Debug(s), msg, &buffer, reply)
                } else if let Some(id) = in_range(badge, VOLUME_BASE) {
                    block::dispatch(&mut Block { s, id }, msg, &buffer, reply)
                } else if let Some(id) = in_range(badge, HOST_BASE) {
                    host::dispatch(&mut Host { s, id }, msg, &buffer, reply)
                } else {
                    Some((msg, reply))
                };
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Gone(badge)) => {
                if let Some(id) = in_range(badge, HOST_BASE) {
                    s.server_gone(id);
                }
            }
            Ok(Event::Signal(_)) => {}
            Err(granite::Error::TimedOut) => {}
            Err(e) => {
                granite::println!("storage: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
        s.finish_shutdown();
        s.restart_due();
        if time::now() >= next_tick {
            if !s.shut_down {
                s.cache.write_back_older(&mut s.disks, now_ms().saturating_sub(WRITE_BACK_MS));
            }
            next_tick = time::after_ms(TICK_MS);
        }
    }
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("storage: answer failed: {e}");
    }
}

// ------------------------------------------------------------------------------------------
// `pbr.block` for one volume

/// A volume's `pbr.block`. (`'a` is a *lifetime*: it says this borrows the `Storage` for as long
/// as it exists, so the compiler can check nothing else uses it meanwhile.)
struct Block<'a> {
    s: &'a mut Storage,
    id: u32,
}

fn block_error(e: CacheError) -> BlockError {
    match e {
        CacheError::OutOfRange => BlockError::OutOfRange,
        CacheError::BadArgument => BlockError::BadArgument,
        CacheError::NoSuchVolume | CacheError::Disk => BlockError::DeviceError,
    }
}

impl Block<'_> {
    /// The volume, if it exists and the service hasn't shut down.
    fn get(&self) -> Result<(u32, u64, u64, bool, bool), BlockError> {
        if self.s.shut_down {
            return Err(BlockError::DeviceError);
        }
        let v = self.s.volume(self.id).ok_or(BlockError::DeviceError)?;
        Ok((v.disk, v.start, v.sectors, v.read_only, v.in_memory))
    }

    /// The byte range of `len` bytes at the volume's `sector` in its disk in memory.
    fn ram_range(
        &self,
        disk: u32,
        start: u64,
        sectors: u64,
        sector: u64,
        len: usize,
    ) -> Result<core::ops::Range<usize>, BlockError> {
        if len == 0 || !len.is_multiple_of(SECTOR) || len > 65536 {
            return Err(BlockError::BadArgument);
        }
        let count = (len / SECTOR) as u64;
        match sector.checked_add(count) {
            Some(end) if end <= sectors => {}
            _ => return Err(BlockError::OutOfRange),
        }
        let at = ((start + sector) as usize) * SECTOR;
        let size = self.s.disks.ram.get(&disk).map_or(0, |v| v.len());
        if at + len > size {
            return Err(BlockError::OutOfRange);
        }
        Ok(at..at + len)
    }
}

impl block::Server for Block<'_> {
    fn info(&mut self, reply: block::info::Reply) {
        answered(match self.get() {
            Ok((_, _, sectors, read_only, _)) => reply.ok(&BlockInfo {
                sector_size: SECTOR as u32,
                sector_count: sectors,
                read_only,
            }),
            Err(e) => reply.err(e),
        })
    }

    fn read(&mut self, sector: u64, count: u32, reply: block::read::Reply) {
        let r = (|| {
            let (disk, start, sectors, _, in_memory) = self.get()?;
            let len = (count as usize).checked_mul(SECTOR).ok_or(BlockError::BadArgument)?;
            if in_memory {
                let range = self.ram_range(disk, start, sectors, sector, len)?;
                return Ok(self.s.disks.ram[&disk][range].to_vec());
            }
            if len == 0 || len > 65536 {
                return Err(BlockError::BadArgument);
            }
            let mut buf = vec![0u8; len];
            self.s.cache.read(&mut self.s.disks, self.id, sector, &mut buf).map_err(block_error)?;
            Ok(buf)
        })();
        answered(match r {
            Ok(data) => reply.ok(&data),
            Err(e) => reply.err(e),
        })
    }

    fn write(&mut self, sector: u64, data: alloc::vec::Vec<u8>, reply: block::write::Reply) {
        let r = (|| {
            let (disk, start, sectors, read_only, in_memory) = self.get()?;
            if read_only {
                return Err(BlockError::ReadOnly);
            }
            if in_memory {
                let range = self.ram_range(disk, start, sectors, sector, data.len())?;
                if let Some(v) = self.s.disks.ram.get_mut(&disk) {
                    v[range].copy_from_slice(&data);
                }
                return Ok(());
            }
            self.s
                .cache
                .write(&mut self.s.disks, self.id, sector, &data, now_ms())
                .map_err(block_error)
        })();
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        })
    }

    fn flush(&mut self, reply: block::flush::Reply) {
        let r = (|| {
            let (_, _, _, _, in_memory) = self.get()?;
            if in_memory {
                return Ok(());
            }
            self.s.cache.flush(&mut self.s.disks, self.id).map_err(block_error)
        })();
        answered(match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        })
    }
}

// ------------------------------------------------------------------------------------------
// `pbr.storage`

struct Admin<'a>(&'a mut Storage);

impl storage::Server for Admin<'_> {
    fn list(&mut self, reply: storage::list::Reply) {
        let list: Vec<VolumeInfo> = self
            .0
            .volumes
            .iter()
            .take(32)
            .map(|v| {
                let failed = self.0.disks.real.get(&v.disk).is_some_and(|d| d.failed);
                VolumeInfo {
                    id: v.id,
                    disk: v.disk,
                    partition: v.partition,
                    filesystem: v.filesystem,
                    state: if failed { VolumeState::Failed } else { v.state },
                    label: v.label.clone(),
                    size: v.sectors * SECTOR as u64,
                    read_only: v.read_only,
                    in_memory: v.in_memory,
                }
            })
            .collect();
        answered(reply.ok(&list))
    }

    /// A copy of the volume's root folder capability. The copy has the rights the server gave
    /// the original, which are every right a folder capability has.
    fn open(&mut self, id: u32, reply: storage::open::Reply) {
        let s = &*self.0;
        let r = match s.volume(id) {
            None => Err(StorageError::NoSuchVolume),
            Some(_) if s.shut_down || s.shutting_down.is_some() => Err(StorageError::ShutDown),
            Some(v) => match (&v.server, v.state) {
                (Some(Server { root: Some(root), .. }), VolumeState::Ready) => root
                    .0
                    .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE | rights::REVOKE)
                    .map(Endpoint)
                    .map_err(|_| StorageError::NoMemory),
                (_, VolumeState::Starting) => Err(StorageError::NotReady),
                (_, VolumeState::Failed) => Err(StorageError::DiskError),
                _ => Err(StorageError::Unsupported),
            },
        };
        answered(match r {
            Ok(root) => reply.ok(root.0),
            Err(e) => reply.err(e),
        })
    }

    fn create_ram_volume(
        &mut self,
        size: u64,
        label: String,
        reply: storage::create_ram_volume::Reply,
    ) {
        let s = &mut *self.0;
        let size = size / 4096 * 4096;
        let r = if s.shut_down {
            Err(StorageError::ShutDown)
        } else if !(256 * 1024..=MAX_RAM_VOLUME).contains(&size) || label.len() > 63 {
            Err(StorageError::BadArgument)
        } else {
            match s.disks.add_ram(size) {
                Some(disk) => {
                    // Its server formats it as RockFS, then serves it: `open` works once
                    // it is ready.
                    let sectors = size / SECTOR as u64;
                    let id =
                        s.add_volume(disk, 0, 0, sectors, Filesystem::Empty, label.clone(), false);
                    s.start_server(id, Some(&label));
                    Ok(id)
                }
                None => Err(StorageError::NoMemory),
            }
        };
        answered(match r {
            Ok(id) => reply.ok(id),
            Err(e) => reply.err(e),
        })
    }

    /// Formats an empty volume as RockFS. An empty whole disk first gets an MBR with one
    /// partition from 1 MiB to the end, as 1.3 does, so the two systems lay disks out alike;
    /// the volume's number then changes (the partition is a new volume). The server formats
    /// and then serves it: `list` says when it is ready.
    fn format(&mut self, id: u32, label: String, reply: storage::format::Reply) {
        answered(match format(self.0, id, &label) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(e),
        })
    }

    fn check(&mut self, id: u32, repair: bool, reply: storage::check::Reply) {
        self.0.check(id, repair, reply);
    }

    fn shutdown(&mut self, reply: storage::shutdown::Reply) {
        let s = &mut *self.0;
        if s.shut_down {
            return answered(reply.ok());
        }
        if s.shutting_down.is_some() {
            return answered(reply.err(StorageError::NotReady));
        }
        // Answered later, by `finish_shutdown`, once every server has saved its volume.
        s.shutting_down =
            Some(ShuttingDown { reply, deadline: time::after_ms(SHUTDOWN_MS), failed: false });
        s.unmount_all();
    }
}

/// `format`'s work (see [`Admin::format`]).
fn format(s: &mut Storage, id: u32, label: &str) -> Result<(), StorageError> {
    if s.shut_down || s.shutting_down.is_some() {
        return Err(StorageError::ShutDown);
    }
    if label.is_empty() || label.len() > 63 {
        return Err(StorageError::BadArgument);
    }
    let v = s.volume(id).ok_or(StorageError::NoSuchVolume)?;
    if v.filesystem != Filesystem::Empty || v.server.is_some() {
        return Err(StorageError::NotEmpty);
    }
    if v.read_only {
        return Err(StorageError::Unsupported);
    }
    if v.partition != 0 || v.in_memory {
        // A partition (or a disk in memory): formatted where it is.
        s.start_server(id, Some(label));
        return Ok(());
    }
    // A whole disk: the MBR, written through the cache as the whole-disk volume, then the
    // disk scanned again, which finds the new partition.
    let (disk, sectors) = (v.disk, v.sectors);
    if sectors < 2048 + 512 {
        return Err(StorageError::BadArgument);
    }
    let count = (sectors - 2048).min(u64::from(u32::MAX)) as u32;
    let mut mbr = vec![0u8; SECTOR];
    let p = &mut mbr[446..462];
    p[1..4].fill(0xFF); // a CHS address meaning "use the LBA fields", as 1.3 writes it
    p[4] = 0x7F; // RockFS's partition type
    p[5..8].fill(0xFF);
    p[8..12].copy_from_slice(&2048u32.to_le_bytes());
    p[12..16].copy_from_slice(&count.to_le_bytes());
    mbr[440..444].copy_from_slice(&(time::now() as u32).to_le_bytes()); // the disk signature
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    s.cache.write(&mut s.disks, id, 0, &mbr, now_ms()).map_err(|_| StorageError::DiskError)?;
    s.cache.flush(&mut s.disks, id).map_err(|_| StorageError::DiskError)?;
    s.volumes.retain(|v| v.disk != disk);
    s.scan(disk);
    let new = s
        .volumes
        .iter()
        .find(|v| v.disk == disk && v.partition == 1 && v.filesystem == Filesystem::Empty)
        .map(|v| v.id)
        .ok_or(StorageError::DiskError)?;
    s.start_server(new, Some(label));
    Ok(())
}

// ------------------------------------------------------------------------------------------
// `pbr.fs.host`

/// What volume `id`'s filesystem server says.
struct Host<'a> {
    s: &'a mut Storage,
    id: u32,
}

impl host::Server for Host<'_> {
    fn ready(
        &mut self,
        root: Endpoint,
        control: Endpoint,
        label: String,
        reply: host::ready::Reply,
    ) {
        let Some(v) = self.s.volume_mut(self.id) else {
            return answered(reply.err(HostError::BadState));
        };
        let Some(server) = v.server.as_mut().filter(|s| s.root.is_none()) else {
            return answered(reply.err(HostError::BadState));
        };
        server.root = Some(root);
        server._control = Some(control);
        v.state = VolumeState::Ready;
        // A volume just formatted held nothing before; now it holds RockFS.
        if v.filesystem == Filesystem::Empty {
            v.filesystem = Filesystem::RockFs;
        }
        v.label = label;
        granite::println!("storage: volume {} is ready: {:?} \"{}\".", v.id, v.filesystem, v.label);
        answered(reply.ok());
    }

    fn checked(&mut self, report: String, problems: u32, reply: host::checked::Reply) {
        let Some(v) = self.s.volume_mut(self.id) else {
            return answered(reply.err(HostError::BadState));
        };
        // Only a server started to check may report, and once.
        match (&v.server, &mut v.check) {
            (Some(Server { checker: true, .. }), Some(c)) if c.report.is_none() => {
                c.report = Some((report, problems));
                answered(reply.ok())
            }
            _ => answered(reply.err(HostError::BadState)),
        }
    }

    fn failed(&mut self, reason: FsFailure, reply: host::failed::Reply) {
        let Some(v) = self.s.volume_mut(self.id) else {
            return answered(reply.err(HostError::BadState));
        };
        v.state = match reason {
            FsFailure::Damaged => VolumeState::Damaged,
            FsFailure::Unsupported => VolumeState::Unsupported,
            _ => VolumeState::Failed,
        };
        if let Some(server) = &mut v.server {
            server.gave_up = true;
        }
        granite::println!("storage: volume {} can't be served: {reason:?}.", v.id);
        answered(reply.ok())
    }
}

// ------------------------------------------------------------------------------------------
// `pbr.storage.debug`

struct Debug<'a>(&'a mut Storage);

impl debug::Server for Debug<'_> {
    fn volume(&mut self, id: u32, reply: debug::volume::Reply) {
        answered(match self.0.volume(id).map(|v| v.id) {
            None => reply.err(DebugError::NoSuchVolume),
            Some(id) => match self.0.volume_cap(id) {
                Ok(cap) => reply.ok(cap),
                Err(_) => reply.fail(),
            },
        })
    }

    fn cache(&mut self, reply: debug::cache::Reply) {
        let s = &*self.0;
        let dirty: usize = s.volumes.iter().map(|v| s.cache.dirty_pages(v.id)).sum();
        answered(reply.ok(s.cache.pages() as u32, dirty as u32))
    }

    fn stop_server(&mut self, id: u32, reply: debug::stop_server::Reply) {
        let stopped = self
            .0
            .volume(id)
            .and_then(|v| v.server.as_ref())
            .is_some_and(|server| server.spawned.process.stop(9).is_ok());
        answered(if stopped { reply.ok() } else { reply.err(DebugError::NoSuchVolume) })
    }

    fn flush(&mut self, reply: debug::flush::Reply) {
        let s = &mut *self.0;
        answered(match s.cache.flush_all(&mut s.disks) {
            Ok(()) => reply.ok(),
            Err(_) => reply.fail(),
        })
    }
}
