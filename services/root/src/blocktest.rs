// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the device manager, and the block device test (docs/design/0011-drivers.md): the
//! root program asks the device manager for the disks, finds the test disk the build attaches
//! (`xtask` writes it, with the pattern in [`pattern`]), and reads, writes and re-reads it.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::Handle;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::process::{self, Spawn, Spawned, role as start_role};
use granite::sys::rights;
use granite::time;
use pbr_archive::Archive;
use pbr_interfaces::pbr_block::{self as block, BlockError};
use pbr_interfaces::pbr_devices::{self as devices, DeviceClass, DeviceState};
use pbr_interfaces::pbr_devices_debug as devices_debug;

/// The role of the device manager's public endpoint.
const PUBLIC: u32 = start_role::OWN;
/// What the device manager may use.
const BUDGET: u64 = 64 * 1024 * 1024;
const SECTOR: usize = 512;
/// The size of the test disk, in sectors.
const TEST_SECTORS: u64 = 2048;

/// What byte `i` of sector `s` of the test disk is. (`xtask` writes the same.)
pub fn pattern(s: u64, i: usize) -> u8 {
    (s.wrapping_mul(31) as u8).wrapping_add((i as u8).wrapping_mul(7)).wrapping_add(3)
}

fn sectors(first: u64, count: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(count as usize * SECTOR);
    for s in first..first + u64::from(count) {
        v.extend((0..SECTOR).map(|i| pattern(s, i)));
    }
    v
}

/// The device manager, and a way to talk to it.
pub struct Devmgr {
    pub process: Spawned,
    pub client: devices::Client,
    /// `pbr.devices.debug`, served in self-test runs.
    pub debug: devices_debug::Client,
}

/// Starts the device manager. With `keep` the root program keeps its device capabilities (the
/// device tests use them) and the device manager is started in self-test mode; without, the
/// capabilities move to the device manager.
pub fn start(
    archive: &Archive<'_>,
    archive_mem: u32,
    start: &granite::Start,
    keep: bool,
) -> Result<Devmgr, String> {
    let file = archive.find("devmgr").ok_or("devmgr isn't in the archive")?;
    let public = Endpoint::create().map_err(|e| format!("creating an endpoint: {e}"))?;
    let all = rights::ALL;
    let mut caps = vec![
        (start_role::ARCHIVE, archive_mem, all, false),
        (PUBLIC, public.0.raw(), rights::RECEIVE, false),
    ];
    for r in
        [start_role::DEVICE_MEMORY, start_role::MSI_POOL, start_role::IO_PORTS, start_role::MACHINE]
    {
        if let Some(h) = start.peek(r) {
            caps.push((r, h, all, !keep));
        }
    }
    let process = process::spawn(&Spawn {
        name: "devmgr",
        program: file.data,
        budget: BUDGET,
        priority: 2,
        caps: &caps,
        log: true,
        args: if keep { "selftest" } else { "" },
    })
    .map_err(|e| format!("starting devmgr: {e}"))?;
    let client = devices::Client::new(Endpoint(
        public
            .0
            .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE)
            .map_err(|e| format!("{e}"))?,
    ));
    let debug = devices_debug::Client::new(Endpoint(
        public
            .0
            .duplicate(rights::SEND | rights::TRANSFER | rights::DUPLICATE)
            .map_err(|e| format!("{e}"))?,
    ));
    Ok(Devmgr { process, client, debug })
}

fn call<T>(what: &str, r: Result<T, CallError<BlockError>>) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
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

/// Waits for the test disk to be served, and opens it.
pub(crate) fn find_disk(d: &Devmgr) -> Result<(u32, block::Client), String> {
    let end = time::after_ms(30_000);
    loop {
        let list = d.client.list().map_err(|e| format!("listing devices: {e}"))?;
        let mut waiting = false;
        for dev in list.iter().filter(|x| x.kind == DeviceClass::Block) {
            match dev.state {
                DeviceState::Running => {}
                DeviceState::Starting | DeviceState::Restarting => {
                    waiting = true;
                    continue;
                }
                DeviceState::Disabled => continue,
            }
            let h: Handle = match d.client.open(dev.id) {
                Ok(h) => h,
                Err(CallError::Service(devices::DevicesError::NotReady)) => {
                    waiting = true;
                    continue;
                }
                Err(e) => return Err(format!("opening {}: {e}", dev.name)),
            };
            let c = block::Client::new(Endpoint(h));
            let info = call("info", c.info())?;
            if info.sector_count == TEST_SECTORS {
                return Ok((dev.id, c));
            }
        }
        if !waiting || time::now() > end {
            return Err(format!("the test disk wasn't found among {} devices", list.len()));
        }
        time::sleep_ms(50);
    }
}

/// The block device test. Returns a short summary.
pub fn run(d: &Devmgr) -> Result<String, String> {
    let (_, mut c) = find_disk(d)?;
    c.set_deadline(time::after_ms(60_000));
    let info = call("info", c.info())?;
    if info.sector_size != SECTOR as u32 || info.read_only {
        return Err(format!(
            "odd disk: {} byte sectors, read only {}",
            info.sector_size, info.read_only
        ));
    }

    for (first, count) in [(0u64, 1u32), (0, 8), (100, 128), (TEST_SECTORS - 1, 1), (777, 3)] {
        let got = call("read", c.read(first, count))?;
        if got != sectors(first, count) {
            return Err(format!("reading {count} sectors at {first} gave the wrong data"));
        }
    }

    refused("reading past the end", c.read(TEST_SECTORS - 1, 2), BlockError::OutOfRange)?;
    refused("reading far past the end", c.read(u64::MAX - 1, 1), BlockError::OutOfRange)?;
    refused("reading nothing", c.read(0, 0), BlockError::BadArgument)?;
    refused("reading too much", c.read(0, 129), BlockError::BadArgument)?;

    // Write three sectors, read them back and check their neighbours didn't change.
    let new: Vec<u8> =
        (0..3 * SECTOR).map(|i| !pattern(10 + (i / SECTOR) as u64, i % SECTOR)).collect();
    call("write", c.write(10, &new))?;
    call("flush", c.flush())?;
    if call("read back", c.read(10, 3))? != new {
        return Err("the sectors written read back differently".into());
    }
    if call("read before", c.read(9, 1))? != sectors(9, 1)
        || call("read after", c.read(13, 1))? != sectors(13, 1)
    {
        return Err("a write changed the sectors next to it".into());
    }
    // The largest request, and the last sector.
    let big: Vec<u8> = (0..128 * SECTOR).map(|i| (i % 251) as u8).collect();
    call("big write", c.write(200, &big))?;
    if call("big read", c.read(200, 128))? != big {
        return Err("the biggest write read back differently".into());
    }
    let last = vec![0xA5u8; SECTOR];
    call("last write", c.write(TEST_SECTORS - 1, &last))?;
    if call("last read", c.read(TEST_SECTORS - 1, 1))? != last {
        return Err("the last sector read back differently".into());
    }

    refused(
        "writing past the end",
        c.write(TEST_SECTORS - 1, &vec![0; 2 * SECTOR]),
        BlockError::OutOfRange,
    )?;
    refused("writing part of a sector", c.write(0, &[0; 100]), BlockError::BadArgument)?;
    refused("writing nothing", c.write(0, &[]), BlockError::BadArgument)?;
    // A failed request left the disk as it was.
    if call("read after refusals", c.read(0, 1))? != sectors(0, 1) {
        return Err("a refused write changed the disk".into());
    }
    Ok(format!("{} sectors", info.sector_count))
}
