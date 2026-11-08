// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The disks behind the cache: connections to disk drivers, and disks in memory
//! (docs/design/0015-storage-and-the-block-cache.md, sections 3 and 6).
//!
//! A disk driver is a separate program that may crash and be restarted by the device manager
//! (note 0012). When that happens, calls to it fail with `PEER_GONE`; [`Disks`] asks the device
//! manager for the new driver and repeats the call. Block reads, writes and flushes do the same
//! thing if done twice, so repeating one is safe, and filesystem servers never notice.
//!
//! The device manager itself may end and be restarted by the root program (note 0018), taking
//! every driver with it. Calls to it then fail with `PEER_GONE` too, and [`Disks`] looks it up
//! again through the supervisor (`pbr.supervisor.lookup("devices")`) and carries on the same way.
//! A disk is found again under the same number (the new device manager finds the devices in the
//! same order) and must still have the same size, or its volumes fail rather than reach a
//! different disk.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::{Error, time};
use pbr_blockcache::{Backend, IoError};
use pbr_interfaces::pbr_block::{self as block, BlockError};
use pbr_interfaces::pbr_devices::{self as devices, DeviceClass, DeviceState, DevicesError};
use pbr_interfaces::pbr_supervisor::{self as supervisor, SupervisorError};

/// Bytes in a sector: the only size supported in M5.
pub const SECTOR: usize = 512;
/// Disks in memory are numbered from here; real disks have the device manager's ids.
pub const RAM_DISK_BASE: u32 = 1000;
/// How long a single call to a driver may take before the disk counts as failed.
const CALL_MS: u64 = 30_000;
/// How long to keep asking for a restarted driver (or device manager, which restarts its
/// drivers too).
const RECONNECT_MS: u64 = 30_000;

/// One disk.
pub struct Disk {
    /// The device manager's name for it ("virtio-blk"), for the log.
    pub name: String,
    pub sectors: u64,
    pub read_only: bool,
    /// The connection to its driver; `None` while it is being reconnected.
    client: Option<block::Client>,
    /// Gone for good: the device manager has disabled it.
    pub failed: bool,
}

/// Every disk: real ones through their drivers, and ones in memory.
pub struct Disks {
    /// The supervisor, to find the device manager through.
    supervisor: supervisor::Client,
    /// The device manager's `pbr.devices`, once found (and until it goes).
    devices: Option<devices::Client>,
    pub real: BTreeMap<u32, Disk>,
    /// Disks in memory, by number: their bytes.
    pub ram: BTreeMap<u32, Vec<u8>>,
}

/// A call to a driver failed in a way that a new connection might fix.
fn connection_lost<E>(e: &CallError<E>) -> bool {
    matches!(e, CallError::Call(Error::PeerGone | Error::Revoked | Error::BadHandle))
}

impl Disks {
    /// No disks yet; the device manager is found through `supervisor`.
    pub fn new(supervisor: supervisor::Client) -> Disks {
        Disks { supervisor, devices: None, real: BTreeMap::new(), ram: BTreeMap::new() }
    }

    /// Makes `call` to the device manager, finding it through the supervisor first if need be,
    /// and again if it went away (until `RECONNECT_MS` has passed).
    fn with_devices<T>(
        &mut self,
        call: impl Fn(&devices::Client) -> Result<T, CallError<DevicesError>>,
    ) -> Result<T, CallError<DevicesError>> {
        let end = time::after_ms(RECONNECT_MS);
        loop {
            if self.devices.is_none() {
                match self.supervisor.lookup("devices") {
                    Ok(h) => {
                        let mut c = devices::Client::new(Endpoint(h));
                        c.set_timeout_ms(CALL_MS);
                        self.devices = Some(c);
                    }
                    Err(e) if time::now() < end => {
                        if !matches!(e, CallError::Service(SupervisorError::NotReady)) {
                            granite::println!("storage: can't find the device manager: {e}");
                        }
                        time::sleep_ms(50);
                        continue;
                    }
                    Err(_) => return Err(CallError::Call(Error::PeerGone)),
                }
            }
            let Some(d) = &self.devices else { continue };
            match call(d) {
                Err(e) if connection_lost(&e) && time::now() < end => {
                    granite::println!("storage: the device manager has gone; finding it again.");
                    self.devices = None;
                }
                r => return r,
            }
        }
    }

    /// Finds every disk the device manager serves, waiting up to 30 seconds for drivers that
    /// are still starting. Disks with sectors other than 512 bytes are left out (note 0015).
    pub fn find(&mut self) {
        let end = time::after_ms(30_000);
        loop {
            let list = match self.with_devices(|d| d.list()) {
                Ok(l) => l,
                Err(e) => {
                    granite::println!("storage: can't list devices: {e}");
                    return;
                }
            };
            let mut waiting = false;
            for d in list.iter().filter(|d| d.kind == DeviceClass::Block) {
                if self.real.contains_key(&d.id) {
                    continue;
                }
                match d.state {
                    DeviceState::Running => {}
                    DeviceState::Starting | DeviceState::Restarting => {
                        waiting = true;
                        continue;
                    }
                    DeviceState::Disabled => continue,
                }
                match self.connect(d.id) {
                    Ok(client) => self.add(d.id, &d.name, client),
                    Err(true) => waiting = true,
                    Err(false) => {}
                }
            }
            if !waiting || time::now() > end {
                return;
            }
            time::sleep_ms(50);
        }
    }

    /// Opens device `id`'s driver. `Err(true)` means "not ready yet, ask again".
    fn connect(&mut self, id: u32) -> Result<block::Client, bool> {
        match self.with_devices(|d| d.open(id)) {
            Ok(h) => {
                let mut c = block::Client::new(Endpoint(h));
                c.set_deadline(time::after_ms(CALL_MS));
                Ok(c)
            }
            // Not ready, or not there yet (a restarted device manager lists its devices as it
            // finds them).
            Err(CallError::Service(DevicesError::NotReady | DevicesError::NoSuchDevice)) => {
                Err(true)
            }
            Err(_) => Err(false),
        }
    }

    fn add(&mut self, id: u32, name: &str, client: block::Client) {
        let info = match client.info() {
            Ok(i) => i,
            Err(e) => {
                granite::println!("storage: disk {id} ({name}) won't say its size: {e}");
                return;
            }
        };
        if info.sector_size != SECTOR as u32 {
            granite::println!(
                "storage: disk {id} ({name}) has {}-byte sectors, which aren't supported yet.",
                info.sector_size
            );
            return;
        }
        let disk = Disk {
            name: String::from(name),
            sectors: info.sector_count,
            read_only: info.read_only,
            client: Some(client),
            failed: false,
        };
        self.real.insert(id, disk);
    }

    /// Asks the device manager for disk `id`'s driver again, after its connection was lost.
    /// Returns whether a new connection was made.
    fn reconnect(&mut self, id: u32) -> bool {
        if let Some(d) = self.real.get_mut(&id) {
            d.client = None;
        }
        let end = time::after_ms(RECONNECT_MS);
        loop {
            match self.connect(id) {
                Ok(c) => {
                    // It must be the same disk: the same size (and read-only or not).
                    let same = c.info().is_ok_and(|i| {
                        self.real.get(&id).is_some_and(|d| {
                            i.sector_count == d.sectors && i.read_only == d.read_only
                        })
                    });
                    if !same {
                        granite::println!(
                            "storage: disk {id} isn't the disk it was; its volumes fail."
                        );
                        if let Some(d) = self.real.get_mut(&id) {
                            d.failed = true;
                        }
                        return false;
                    }
                    granite::println!("storage: reconnected to disk {id}'s driver.");
                    if let Some(d) = self.real.get_mut(&id) {
                        d.client = Some(c);
                    }
                    return true;
                }
                Err(true) if time::now() < end => time::sleep_ms(50),
                Err(_) => {
                    granite::println!("storage: disk {id}'s driver is gone; its volumes fail.");
                    if let Some(d) = self.real.get_mut(&id) {
                        d.failed = true;
                    }
                    return false;
                }
            }
        }
    }

    /// Makes `call` to disk `id`'s driver, once more after reconnecting if the driver went away
    /// in between. Every call gets a fresh deadline.
    fn with_driver<T>(
        &mut self,
        id: u32,
        call: impl Fn(&block::Client) -> Result<T, CallError<BlockError>>,
    ) -> Result<T, IoError> {
        for attempt in 0..2 {
            let Some(d) = self.real.get_mut(&id) else { return Err(IoError) };
            if d.failed {
                return Err(IoError);
            }
            let Some(c) = d.client.as_mut() else { return Err(IoError) };
            c.set_deadline(time::after_ms(CALL_MS));
            match call(c) {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && connection_lost(&e) => {
                    if !self.reconnect(id) {
                        return Err(IoError);
                    }
                }
                Err(_) => return Err(IoError),
            }
        }
        Err(IoError)
    }

    /// Adds a disk in memory of `bytes` bytes (whole sectors). Returns its number, or `None` if
    /// there isn't the memory for it.
    pub fn add_ram(&mut self, bytes: u64) -> Option<u32> {
        let mut v: Vec<u8> = Vec::new();
        // `try_reserve_exact` asks for the memory without stopping the program if there isn't
        // any (an ordinary `Vec` allocation would).
        v.try_reserve_exact(usize::try_from(bytes).ok()?).ok()?;
        v.resize(bytes as usize, 0);
        let id = RAM_DISK_BASE + self.ram.len() as u32;
        self.ram.insert(id, v);
        Some(id)
    }

    /// How many sectors disk `id` has.
    pub fn sectors(&self, id: u32) -> u64 {
        match self.ram.get(&id) {
            Some(v) => (v.len() / SECTOR) as u64,
            None => self.real.get(&id).map_or(0, |d| d.sectors),
        }
    }
}

/// The cache reads and writes real disks through here. (Disks in memory don't go through the
/// cache: they already are memory.)
impl Backend for Disks {
    fn read(&mut self, disk: u32, sector: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let count = (buf.len() / SECTOR) as u32;
        let data = self.with_driver(disk, |c| c.read(sector, count))?;
        // The driver is another program: never trust it to have sent the right amount.
        if data.len() != buf.len() {
            return Err(IoError);
        }
        buf.copy_from_slice(&data);
        Ok(())
    }

    fn write(&mut self, disk: u32, sector: u64, data: &[u8]) -> Result<(), IoError> {
        self.with_driver(disk, |c| c.write(sector, data))
    }

    fn flush(&mut self, disk: u32) -> Result<(), IoError> {
        self.with_driver(disk, |c| c.flush())
    }
}

/// Reading a disk directly, without the cache, for its partition table: `lib/partition`
/// reads through this.
pub struct Raw<'a> {
    pub disks: &'a mut Disks,
    pub id: u32,
}

impl pbr_partition::Disk for Raw<'_> {
    fn sectors(&self) -> u64 {
        self.disks.sectors(self.id)
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), pbr_partition::ReadError> {
        if let Some(v) = self.disks.ram.get(&self.id) {
            let at = usize::try_from(sector).map_err(|_| pbr_partition::ReadError)? * SECTOR;
            let src = v.get(at..at + buf.len()).ok_or(pbr_partition::ReadError)?;
            buf.copy_from_slice(src);
            return Ok(());
        }
        // A partition table read is small; split it at the driver's 64 KiB limit anyway.
        for (i, chunk) in buf.chunks_mut(65536).enumerate() {
            let at = sector + (i * 65536 / SECTOR) as u64;
            Backend::read(self.disks, self.id, at, chunk).map_err(|_| pbr_partition::ReadError)?;
        }
        Ok(())
    }
}
