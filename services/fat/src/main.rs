// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The FAT32 server (docs/design/0016-filesystems.md, section 1): one per FAT32 volume, started
//! by the storage service. It is the same program as the RockFS server (`services/rockfs`)
//! with `lib/fat` in place of `lib/rockfs`: `lib/fat` knows the disk format, `lib/fsserve`
//! knows capabilities and `pbr.file`, and this joins them to the volume's `pbr.block`.
//!
//! One server for each volume means a damaged or hostile disk that trips a bug in the FAT code
//! can take down only the server for that disk. FAT volumes come from other systems (the boot
//! partition, a USB stick), so this is the server most likely to meet a volume built to cause
//! harm.
//!
//! Start-up roles, as for RockFS: its own endpoint ([`role::OWN`]), the storage service's
//! `pbr.fs.host` ([`role::PARENT`]), the volume's `pbr.block` ([`BLOCK`]) and a notification
//! the storage service signals to have it unmount ([`STOP`]). Arguments: `volume ID`, then
//! `readonly` to mount it read-only. `format` is refused: PrettyBigRock does not make FAT
//! volumes (note 0016).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::notification::Notification;
use granite::process::role;
use granite::time;
use pbr_fat::{Fat, MountError};
use pbr_fs::{BlockDevice, DeviceError};
use pbr_interfaces::pbr_block as block;
use pbr_interfaces::pbr_fs_host::{self as host, FsFailure};

granite::main!(main);

/// The start-up role of the volume's `pbr.block` capability.
pub const BLOCK: u32 = 1003;
/// The start-up role of the stop notification.
pub const STOP: u32 = 1004;

/// The bytes in a sector.
const SECTOR: usize = 512;
/// The most sectors in one `pbr.block` request (64 KiB).
const MAX_SECTORS: usize = 128;

/// The volume, as `lib/fat` sees it: `pbr.block` calls to the storage service, cut into
/// requests it accepts. (`impl BlockDevice for Volume` below is how a type gets a *trait*'s
/// methods, which is all `lib/fat` needs to know about it.)
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

fn main(mut start: granite::Start) -> i32 {
    let (Some(own), Some(parent), Some(volume)) =
        (start.take(role::OWN), start.take(role::PARENT), start.take(BLOCK))
    else {
        granite::println!("fat: started without its endpoints");
        return 1;
    };
    let args: Vec<&str> = start.args().split_whitespace().collect();
    let id: u32 = match args.as_slice() {
        ["volume", n, ..] => n.parse().unwrap_or(0),
        _ => 0,
    };
    let read_only = args.get(2) == Some(&"readonly");
    let format = args.get(2) == Some(&"format");

    let mut host = host::Client::new(Endpoint(parent));
    host.set_timeout_ms(30_000);
    // Says why the volume can't be served, and stops.
    let fail = |host: &host::Client, why: FsFailure, what: &dyn core::fmt::Display| -> i32 {
        granite::println!("fat: volume {id}: {what}");
        let _ = host.failed(why);
        1
    };
    if format {
        return fail(&host, FsFailure::Unsupported, &"FAT32 volumes can't be made here");
    }

    let mut client = block::Client::new(Endpoint(volume));
    client.set_timeout_ms(60_000);
    let info = match client.info() {
        Ok(i) if i.sector_size as usize == SECTOR => i,
        Ok(i) => return fail(&host, FsFailure::Unsupported, &i.sector_size),
        Err(e) => return fail(&host, FsFailure::Disk, &e),
    };
    let dev = Volume { client, sectors: info.sector_count, read_only: info.read_only };
    let fs = match Fat::mount(dev, time::wall_seconds, read_only) {
        Ok(fs) => fs,
        Err(e) => {
            let why = match e {
                MountError::NotFat | MountError::Damaged(_) => FsFailure::Damaged,
                MountError::Unsupported(_) => FsFailure::Unsupported,
                MountError::Disk => FsFailure::Disk,
            };
            return fail(&host, why, &e);
        }
    };
    let volume_info = fs.info();
    if !volume_info.clean_at_mount {
        // We cannot repair a FAT volume, so it is used as it is.
        granite::println!("fat: volume {id} wasn't shut down cleanly; used as it is.");
    }
    let label = volume_info.label;
    // From here `lib/fsserve` does everything: tells the storage service the volume is
    // ready, then answers `pbr.file` until it is unmounted.
    let stop = start.take(STOP).map(Notification);
    pbr_fsserve::serve::run(fs, Endpoint(own), host.into_endpoint(), &label, stop)
}
