// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The RockFS server (docs/design/0016-filesystems.md, section 1): one per RockFS volume,
//! started by the storage service. It is thin: `lib/rockfs` knows the disk format, `lib/fsserve`
//! knows capabilities and `pbr.file`, and this joins them to the volume's `pbr.block`.
//!
//! A server reaches its own volume and nothing else. That's what makes one per volume worth it:
//! a damaged or hostile disk that trips a bug in the RockFS code can take down only the server
//! for that disk, not the system disk's (note 0016, "Alternatives considered").
//!
//! Start-up roles: its own endpoint ([`role::OWN`], every right, to serve and mint from), the
//! storage service's `pbr.fs.host` ([`role::PARENT`]), the volume's `pbr.block` ([`BLOCK`]) and
//! a notification the storage service signals to have it unmount ([`STOP`]). Arguments: `volume ID`, then `readonly` to mount it read-only, or
//! `format LABEL...` to put a new, empty RockFS volume on it first.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::notification::Notification;
use granite::process::role;
use granite::time;
use pbr_fs::{BlockDevice, DeviceError};
use pbr_interfaces::pbr_block as block;
use pbr_interfaces::pbr_fs_host::{self as host, FsFailure};
use pbr_rockfs::{MountError, RockFs};

granite::main!(main);

/// The start-up role of the volume's `pbr.block` capability.
pub const BLOCK: u32 = 1003;
/// The start-up role of the stop notification.
pub const STOP: u32 = 1004;

/// The bytes in a sector.
const SECTOR: usize = 512;
/// The most sectors in one `pbr.block` request (64 KiB).
const MAX_SECTORS: usize = 128;

/// The volume, as `lib/rockfs` sees it: `pbr.block` calls to the storage service, cut into
/// requests it accepts. (`impl BlockDevice for Volume` below is how a type gets a *trait*'s
/// methods, which is all `lib/rockfs` needs to know about it.)
struct Volume {
    client: block::Client,
    sectors: u64,
    read_only: bool,
}

impl BlockDevice for Volume {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        for (i, chunk) in buf.chunks_mut(MAX_SECTORS * SECTOR).enumerate() {
            let at = sector + (i * MAX_SECTORS) as u64;
            let data =
                self.client.read(at, (chunk.len() / SECTOR) as u32).map_err(|_| DeviceError)?;
            if data.len() != chunk.len() {
                return Err(DeviceError);
            }
            chunk.copy_from_slice(&data);
        }
        Ok(())
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
        for (i, chunk) in data.chunks(MAX_SECTORS * SECTOR).enumerate() {
            let at = sector + (i * MAX_SECTORS) as u64;
            self.client.write(at, chunk).map_err(|_| DeviceError)?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DeviceError> {
        self.client.flush().map_err(|_| DeviceError)
    }
}

/// A new volume's ID: it only has to tell volumes apart, so the time (to the nanosecond since
/// start-up, and the wall clock) and the volume's number are mixed into 16 bytes.
fn new_uuid(volume: u32) -> [u8; 16] {
    // SplitMix64, a small, well-known mixing function: every input bit affects every output
    // bit, so close times give unrelated IDs.
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let a = mix(time::now() ^ (u64::from(volume) << 48));
    let b = mix(a ^ time::wall_now().unwrap_or(0));
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&a.to_le_bytes());
    id[8..].copy_from_slice(&b.to_le_bytes());
    // Marked as a random (version 4) UUID, as 1.3 and `rockfs.py` mark theirs.
    id[6] = (id[6] & 0x0F) | 0x40;
    id[8] = (id[8] & 0x3F) | 0x80;
    id
}

fn main(mut start: granite::Start) -> i32 {
    let (Some(own), Some(parent), Some(volume)) =
        (start.take(role::OWN), start.take(role::PARENT), start.take(BLOCK))
    else {
        granite::println!("rockfs: started without its endpoints");
        return 1;
    };
    let args: Vec<&str> = start.args().split_whitespace().collect();
    let id: u32 = match args.as_slice() {
        ["volume", n, ..] => n.parse().unwrap_or(0),
        _ => 0,
    };
    let read_only = args.get(2) == Some(&"readonly");
    let format: Option<String> = (args.get(2) == Some(&"format")).then(|| args[3..].join(" "));

    let mut host = host::Client::new(Endpoint(parent));
    host.set_timeout_ms(30_000);
    // Says why the volume can't be served, and stops.
    let fail = |host: &host::Client, why: FsFailure, what: &dyn core::fmt::Display| -> i32 {
        granite::println!("rockfs: volume {id}: {what}");
        let _ = host.failed(why);
        1
    };

    let mut client = block::Client::new(Endpoint(volume));
    client.set_timeout_ms(60_000);
    let info = match client.info() {
        Ok(i) if i.sector_size as usize == SECTOR => i,
        Ok(i) => return fail(&host, FsFailure::Unsupported, &i.sector_size),
        Err(e) => return fail(&host, FsFailure::Disk, &e),
    };
    let mut dev = Volume { client, sectors: info.sector_count, read_only: info.read_only };
    if let Some(label) = &format
        && let Err(e) = pbr_rockfs::format(&mut dev, label, new_uuid(id), time::wall_seconds())
    {
        return fail(&host, FsFailure::Disk, &alloc::format!("formatting: {e:?}"));
    }
    let fs = match RockFs::mount(dev, time::wall_seconds, read_only) {
        Ok(fs) => fs,
        Err(e) => {
            let why = match e {
                MountError::NotRockFs | MountError::Damaged(_) => FsFailure::Damaged,
                MountError::Unsupported(_) => FsFailure::Unsupported,
                MountError::Disk => FsFailure::Disk,
            };
            return fail(&host, why, &e);
        }
    };
    let problems = fs.mount_report().len();
    if problems > 0 {
        granite::println!("rockfs: volume {id} wasn't shut down cleanly; {problems} repairs.");
    }
    let label = fs.info().label;
    // From here `lib/fsserve` does everything: tells the storage service the volume is
    // ready, then answers `pbr.file` until it is unmounted.
    let stop = start.take(STOP).map(Notification);
    pbr_fsserve::serve::run(fs, Endpoint(own), host.into_endpoint(), &label, stop)
}
