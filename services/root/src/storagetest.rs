// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the storage service, and its self-test (docs/design/0015-storage-and-the-block-
//! cache.md; note 0018, section 7, test 2). The build tool attaches three disks for it
//! (`xtask/src/testdisks.rs`): an MBR disk with a RockFS volume `Data` and a FAT32 volume
//! `USB`, a GPT disk with a RockFS volume `Home` and a FAT32 volume `PICTURES`, and an empty
//! disk. The test checks that the service finds exactly those volumes, that a volume's
//! capability reaches that volume and nothing else, that writes go through the cache and reach
//! the disk when flushed, that volumes in memory work, and that a disk driver crashing
//! underneath changes nothing a volume's user can see.

use crate::blocktest::Devmgr;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::process::{self, Spawn, Spawned, role as start_role};
use granite::sys::rights;
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_block::{self as block, BlockError};
use pbr_interfaces::pbr_storage::{self as storage, Filesystem, StorageError, VolumeInfo};
use pbr_interfaces::pbr_storage_debug as storage_debug;

/// The storage service's start-up role for the device manager's endpoint (its own endpoint is
/// `role::OWN`).
const DEVICES: u32 = 1001;
/// Badges of the capabilities the root program mints from the storage service's endpoint
/// (`services/storage`'s `ADMIN` and `DEBUG`).
const ADMIN: u64 = 1;
const DEBUG: u64 = 2;
/// What the storage service may use: its 16 MiB cache, and room for volumes in memory.
const BUDGET: u64 = 64 * 1024 * 1024;
const SECTOR: usize = 512;
const MIB: u64 = 1024 * 1024;

/// The storage service, and ways to talk to it.
pub struct Storage {
    pub process: Spawned,
    pub admin: storage::Client,
    pub debug: storage_debug::Client,
}

/// Starts the storage service, giving it the device manager's `pbr.devices`.
pub fn start(archive: &Archive<'_>, devmgr: &Devmgr, selftest: bool) -> Result<Storage, String> {
    let file = archive.find("storage").ok_or("storage isn't in the archive")?;
    let ep = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;
    let devices = devmgr
        .client
        .endpoint()
        .0
        .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE)
        .map_err(|e| format!("copying the device manager's endpoint: {e}"))?;
    let caps = [
        (start_role::OWN, ep.0.raw(), rights::ALL, false),
        (DEVICES, devices.into_raw(), rights::ALL, true),
    ];
    let process = process::spawn(&Spawn {
        name: "storage",
        program: file.data,
        budget: BUDGET,
        priority: 2,
        caps: &caps,
        log: true,
        args: if selftest { "selftest" } else { "" },
        tied: false,
    })
    .map_err(|e| format!("starting storage: {e}"))?;
    let client = rights::SEND | rights::TRANSFER | rights::DUPLICATE;
    let mint = |badge| ep.mint(badge, client).map_err(|e| format!("minting: {e}"));
    let mut admin = storage::Client::new(mint(ADMIN)?);
    let mut debug = storage_debug::Client::new(mint(DEBUG)?);
    // The service finds its disks before it answers, which can take a while.
    admin.set_deadline(time::after_ms(60_000));
    debug.set_deadline(time::after_ms(60_000));
    Ok(Storage { process, admin, debug })
}

/// The one volume matching all of these, or an error saying which wasn't found.
fn find<'a>(
    list: &'a [VolumeInfo],
    filesystem: Filesystem,
    label: &str,
    partition: u32,
    size: u64,
) -> Result<&'a VolumeInfo, String> {
    let mut it = list.iter().filter(|v| {
        v.filesystem == filesystem && v.label == label && v.partition == partition && v.size == size
    });
    match (it.next(), it.next()) {
        (Some(v), None) => Ok(v),
        (None, _) => {
            Err(format!("no {filesystem:?} volume \"{label}\" ({size} bytes) in {list:?}"))
        }
        (Some(_), Some(_)) => Err(format!("two {filesystem:?} volumes \"{label}\" in {list:?}")),
    }
}

/// A volume's `pbr.block`, as its filesystem server would get it.
fn volume(s: &Storage, id: u32) -> Result<block::Client, String> {
    let ep = s.debug.volume(id).map_err(|e| format!("asking for volume {id}: {e}"))?;
    let mut c = block::Client::new(ep);
    c.set_deadline(time::after_ms(60_000));
    Ok(c)
}

fn refused<T>(
    what: &str,
    r: Result<T, CallError<BlockError>>,
    want: BlockError,
) -> Result<(), String> {
    match r {
        Err(CallError::Service(e)) if e == want => Ok(()),
        Err(e) => Err(format!("{what}: expected {want:?}, got {e}")),
        Ok(_) => Err(format!("{what}: expected {want:?}, but it worked")),
    }
}

fn read(c: &block::Client, sector: u64, count: u32) -> Result<Vec<u8>, String> {
    c.read(sector, count).map_err(|e| format!("reading {count} sectors at {sector}: {e}"))
}

/// Some recognisable data: `count` sectors, made from `seed`.
fn data(seed: u8, count: usize) -> Vec<u8> {
    (0..count * SECTOR).map(|i| seed.wrapping_add((i / 7) as u8)).collect()
}

/// Waits until the cache has `dirty` pages waiting (at most) or the deadline passes.
fn dirty(s: &Storage) -> Result<u32, String> {
    s.debug.cache().map(|r| r.dirty).map_err(|e| format!("asking about the cache: {e}"))
}

/// The storage self-test. Returns a short summary.
pub fn run(s: &Storage, devmgr: &Devmgr) -> Result<String, String> {
    let list = s.admin.list().map_err(|e| format!("listing volumes: {e}"))?;

    // Exactly the test disks' volumes (other disks, such as ARM's boot disk, may be there too).
    let data_v = find(&list, Filesystem::RockFs, "Data", 1, 8 * MIB)?;
    let usb = find(&list, Filesystem::Fat32, "USB", 2, 34 * MIB)?;
    let home = find(&list, Filesystem::RockFs, "Home", 1, 4 * MIB)?;
    find(&list, Filesystem::Fat32, "PICTURES", 2, 34 * MIB)?;
    let empty = find(&list, Filesystem::Empty, "", 0, 2 * MIB)?;
    if data_v.disk != usb.disk || data_v.disk == home.disk || data_v.in_memory {
        return Err(format!("the volumes are on the wrong disks: {list:?}"));
    }

    // A volume's capability: its sector 0 is the volume's first sector, and nothing past its
    // end can be reached.
    let d = volume(s, data_v.id)?;
    let info = d.info().map_err(|e| format!("volume info: {e}"))?;
    if info.sector_count != 8 * MIB / SECTOR as u64 || info.sector_size != SECTOR as u32 {
        return Err(format!("the Data volume says it is {info:?}"));
    }
    if pbr_partition::rockfs_superblock(&read(&d, 0, 2)?).as_deref() != Some("Data") {
        return Err("the Data volume's first sectors aren't its superblock".into());
    }
    let h = volume(s, home.id)?;
    if pbr_partition::rockfs_superblock(&read(&h, 0, 2)?).as_deref() != Some("Home") {
        return Err("the Home volume (in a GPT) doesn't start at its superblock".into());
    }
    let u = volume(s, usb.id)?;
    if pbr_partition::fat32_boot_sector(&read(&u, 0, 1)?, usb.size / SECTOR as u64).as_deref()
        != Some("USB")
    {
        return Err("the USB volume doesn't start at its boot sector".into());
    }
    let end = info.sector_count;
    refused("reading the sector after the volume", d.read(end, 1), BlockError::OutOfRange)?;
    refused("reading across the volume's end", d.read(end - 1, 2), BlockError::OutOfRange)?;
    refused("reading at a huge sector", d.read(u64::MAX, 1), BlockError::OutOfRange)?;
    refused("reading too much", d.read(0, 129), BlockError::BadArgument)?;
    refused("writing part of a sector", d.write(0, &[1; 100]), BlockError::BadArgument)?;

    // Writes go through the cache: they read back at once, wait in the cache, and a flush
    // writes them to the disk. The sectors around them don't change.
    let before = read(&d, 99, 5)?;
    let new = data(0x40, 3);
    d.write(100, &new).map_err(|e| format!("writing: {e}"))?;
    let after = read(&d, 99, 5)?;
    if after[SECTOR..4 * SECTOR] != new[..]
        || after[..SECTOR] != before[..SECTOR]
        || after[4 * SECTOR..] != before[4 * SECTOR..]
    {
        return Err("a write read back wrongly, or changed the sectors beside it".into());
    }
    let big = data(0x80, 128);
    d.write(1000, &big).map_err(|e| format!("writing 64 KiB: {e}"))?;
    if read(&d, 1000, 128)? != big {
        return Err("a 64 KiB write read back wrongly".into());
    }
    if dirty(s)? == 0 {
        return Err("writes went straight to the disk instead of waiting in the cache".into());
    }
    d.flush().map_err(|e| format!("flushing: {e}"))?;
    if dirty(s)? != 0 {
        return Err("a flush left pages waiting".into());
    }

    // A volume in memory.
    let tiny = s.admin.create_ram_volume(100 * 1024, "Tiny");
    if !matches!(tiny, Err(CallError::Service(StorageError::BadArgument))) {
        return Err(format!("a 100 KiB volume in memory wasn't refused: {tiny:?}"));
    }
    let ram_id = s
        .admin
        .create_ram_volume(4 * MIB, "Scratch")
        .map_err(|e| format!("making a volume in memory: {e}"))?;
    let list = s.admin.list().map_err(|e| format!("listing volumes: {e}"))?;
    let ram = find(&list, Filesystem::Empty, "Scratch", 0, 4 * MIB)?;
    if ram.id != ram_id || !ram.in_memory {
        return Err(format!("the volume in memory is listed as {ram:?}"));
    }
    let r = volume(s, ram_id)?;
    r.write(8, &big).map_err(|e| format!("writing to memory: {e}"))?;
    if read(&r, 8, 128)? != big || read(&r, 0, 8)? != vec![0u8; 8 * SECTOR] {
        return Err("the volume in memory read back wrongly".into());
    }
    refused("reading past the volume in memory", r.read(8192, 1), BlockError::OutOfRange)?;

    // Not until the filesystem servers exist: opening, formatting. Formatting something with
    // data on it is refused.
    let f = s.admin.format(data_v.id, "X");
    if !matches!(f, Err(CallError::Service(StorageError::NotEmpty))) {
        return Err(format!("formatting a RockFS volume wasn't refused: {f:?}"));
    }
    let _ = empty;

    // A disk driver crashing underneath: the volume's user sees nothing. Writing and flushing
    // reach the new driver, and reads of sectors never read before come from it.
    let restarts = |id| -> Result<u32, String> {
        let l = devmgr.client.list().map_err(|e| format!("listing devices: {e}"))?;
        l.iter().find(|x| x.id == id).map(|x| x.restarts).ok_or_else(|| format!("no device {id}"))
    };
    let was = restarts(data_v.disk)?;
    devmgr.debug.stop(data_v.disk).map_err(|e| format!("stopping the disk's driver: {e}"))?;
    let newer = data(0xC0, 4);
    d.write(200, &newer).map_err(|e| format!("writing after the driver crashed: {e}"))?;
    d.flush().map_err(|e| format!("flushing after the driver crashed: {e}"))?;
    if read(&u, 60_000, 128)? != vec![0u8; 128 * SECTOR] {
        return Err("sectors read from the restarted driver were wrong".into());
    }
    if read(&d, 200, 4)? != newer || read(&d, 100, 3)? != new {
        return Err("data written before and after the driver crashed read back wrongly".into());
    }
    if restarts(data_v.disk)? != was + 1 {
        return Err("the disk's driver wasn't restarted exactly once".into());
    }
    Ok(format!("{} volumes, volumes in memory, a driver crash", list.len()))
}
