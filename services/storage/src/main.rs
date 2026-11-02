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
//!   counted from the volume's start, so nothing outside it can be named.
//!
//! It handles one request at a time (note 0015, section 7).

#![no_std]
#![no_main]

extern crate alloc;

mod disks;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use disks::{Disks, Raw, SECTOR};
use granite::endpoint::{Endpoint, Event};
use granite::process::role;
use granite::sys::rights;
use granite::time;
use pbr_blockcache::{Cache, Error as CacheError};
use pbr_interfaces::pbr_block::{self as block, BlockError, BlockInfo};
use pbr_interfaces::pbr_devices as devices;
use pbr_interfaces::pbr_storage::{
    self as storage, Filesystem, StorageError, VolumeInfo, VolumeState,
};
use pbr_interfaces::pbr_storage_debug::{self as debug, DebugError};

granite::main!(main);

/// Start-up roles: this service's endpoint (with every right, so it can mint capabilities from
/// it) and the device manager's `pbr.devices`.
const OWN: u32 = role::OWN;
const DEVICES: u32 = 1001;

/// The badge of the capability for `pbr.storage`, which the root program mints from this
/// service's endpoint.
pub const ADMIN: u64 = 1;
/// The badge for `pbr.storage.debug` (self-test runs only).
pub const DEBUG: u64 = 2;
/// A volume's `pbr.block` capability carries this plus the volume's number.
pub const VOLUME_BASE: u64 = 0x1000_0000;

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
}

/// Everything the service keeps.
struct Storage {
    disks: Disks,
    cache: Cache,
    volumes: Vec<Volume>,
    endpoint: Endpoint,
    shut_down: bool,
    next_id: u32,
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
    let devices = Endpoint(start.take(DEVICES).ok_or("started without the device manager")?);
    let mut s = Storage {
        disks: Disks::new(devices::Client::new(devices)),
        cache: Cache::new(CACHE_PAGES),
        volumes: Vec::new(),
        endpoint,
        shut_down: false,
        next_id: 1,
    };
    s.disks.find();
    let ids: Vec<u32> = s.disks.real.keys().copied().collect();
    for id in ids {
        s.scan(id);
    }
    serve(&mut s, selftest);
    Ok(())
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
        // Filesystem servers arrive in stages 3 to 5: until then nothing is served.
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
        });
        id
    }

    fn volume(&self, id: u32) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.id == id)
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
        match ep.receive(&mut buffer, next_tick) {
            Ok(Event::Call(msg, reply)) => {
                let badge = msg.badge;
                let unknown = match badge {
                    ADMIN => storage::dispatch(&mut Admin(s), msg, &buffer, reply),
                    DEBUG if selftest => debug::dispatch(&mut Debug(s), msg, &buffer, reply),
                    b if b > VOLUME_BASE && b - VOLUME_BASE <= u64::from(u32::MAX) => {
                        let id = (b - VOLUME_BASE) as u32;
                        block::dispatch(&mut Block { s, id }, msg, &buffer, reply)
                    }
                    _ => Some((msg, reply)),
                };
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(_) | Event::Gone(_)) => {}
            Err(granite::Error::TimedOut) => {}
            Err(e) => {
                granite::println!("storage: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
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

    fn open(&mut self, id: u32, reply: storage::open::Reply) {
        // Volumes are opened through their filesystem servers, from stage 3.
        answered(match self.0.volume(id) {
            None => reply.err(StorageError::NoSuchVolume),
            Some(_) if self.0.shut_down => reply.err(StorageError::ShutDown),
            Some(_) => reply.err(StorageError::Unsupported),
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
                    // Formatted as RockFS once the RockFS server exists (stage 4).
                    let sectors = size / SECTOR as u64;
                    Ok(s.add_volume(disk, 0, 0, sectors, Filesystem::Empty, label, false))
                }
                None => Err(StorageError::NoMemory),
            }
        };
        answered(match r {
            Ok(id) => reply.ok(id),
            Err(e) => reply.err(e),
        })
    }

    fn format(&mut self, id: u32, _label: String, reply: storage::format::Reply) {
        // Needs RockFS's formatting: stage 4.
        answered(match self.0.volume(id) {
            None => reply.err(StorageError::NoSuchVolume),
            Some(v) if v.filesystem != Filesystem::Empty => reply.err(StorageError::NotEmpty),
            Some(_) => reply.err(StorageError::Unsupported),
        })
    }

    fn shutdown(&mut self, reply: storage::shutdown::Reply) {
        let s = &mut *self.0;
        // From stage 3, each filesystem server is unmounted first.
        let r = s.cache.flush_all(&mut s.disks);
        s.shut_down = true;
        granite::println!("storage: shut down{}.", if r.is_ok() { "" } else { ", with errors" });
        answered(match r {
            Ok(()) => reply.ok(),
            Err(_) => reply.err(StorageError::DiskError),
        })
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
}
