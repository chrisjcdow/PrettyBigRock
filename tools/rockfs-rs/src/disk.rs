// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A disk image file as a block device, and finding the RockFS volume inside it.
//!
//! A *block device* is anything that stores data in fixed-size sectors and is read and written
//! a whole sector at a time; here it is an ordinary file on the host. A *trait* is Rust's
//! name for an interface: `BlockDevice` is the interface `lib/rockfs` wants, and `FileDisk`
//! provides it.

use pbr_fs::{BlockDevice, DeviceError};
use pbr_partition::{Disk, Filesystem, ReadError, Table};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

/// Bytes in a sector.
pub const SECTOR: usize = 512;

/// A MBR partition type that RockFS uses (ROCKFS.md, "Where a volume lives").
pub const MBR_TYPE: u8 = 0x7F;

/// Where `mkfs` puts the partition: sector 2048, which is 1 MiB in, like most partitioning tools.
pub const PART_START: u64 = 2048;

/// An image file, seen as the sectors from `start` for `sectors` sectors.
pub struct FileDisk {
    file: File,
    start: u64,
    sectors: u64,
    read_only: bool,
}

impl FileDisk {
    /// Opens the whole file as one run of sectors. `writable` opens it for changes too.
    pub fn open(path: &str, writable: bool) -> Result<FileDisk, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(writable)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        let bytes = file.metadata().map_err(|e| format!("{path}: {e}"))?.len();
        Ok(FileDisk { file, start: 0, sectors: bytes / SECTOR as u64, read_only: !writable })
    }

    /// Narrows the disk to the volume that starts `start` sectors in and is `sectors` long.
    pub fn narrow(mut self, start: u64, sectors: u64) -> FileDisk {
        self.start += start;
        self.sectors = sectors;
        self
    }

    /// Makes sure everything written has reached the storage, not just the operating system's
    /// cache. (`flush` below does not do this on every call: a host tool has no power cuts to
    /// survive, and the cost would make the tests slow.)
    pub fn sync(&self) {
        if !self.read_only {
            let _ = self.file.sync_all();
        }
    }

    fn seek(&mut self, sector: u64) -> Result<(), DeviceError> {
        let at = self.start.checked_add(sector).and_then(|s| s.checked_mul(SECTOR as u64));
        let at = at.ok_or(DeviceError)?;
        self.file.seek(SeekFrom::Start(at)).map_err(|_| DeviceError)?;
        Ok(())
    }
}

impl BlockDevice for FileDisk {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        let end = sector.checked_add((buf.len() / SECTOR) as u64).ok_or(DeviceError)?;
        if end > self.sectors {
            return Err(DeviceError);
        }
        self.seek(sector)?;
        self.file.read_exact(buf).map_err(|_| DeviceError)
    }

    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
        let end = sector.checked_add((data.len() / SECTOR) as u64).ok_or(DeviceError)?;
        if self.read_only || end > self.sectors {
            return Err(DeviceError);
        }
        self.seek(sector)?;
        self.file.write_all(data).map_err(|_| DeviceError)
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        // The operating system's cache is shared by every program that reads the file, so
        // another tool reading the image sees these writes in order without an fsync.
        self.file.flush().map_err(|_| DeviceError)
    }
}

/// The partition scanner reads the whole image through this.
struct ScanDisk<'a>(&'a mut File, u64);

impl Disk for ScanDisk<'_> {
    fn sectors(&self) -> u64 {
        self.1
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        self.0.seek(SeekFrom::Start(sector * SECTOR as u64)).map_err(|_| ReadError)?;
        self.0.read_exact(buf).map_err(|_| ReadError)
    }
}

/// Opens `path` and narrows it to the RockFS volume inside: the whole file if it starts with a
/// superblock, otherwise the first RockFS partition of its MBR or GPT.
pub fn open_volume(path: &str, writable: bool) -> Result<FileDisk, String> {
    let mut disk = FileDisk::open(path, false)?;
    let sectors = disk.sectors;
    let scan = pbr_partition::scan(&mut ScanDisk(&mut disk.file, sectors))
        .map_err(|_| format!("{path}: the disk could not be read"))?;
    let chosen = scan
        .volumes
        .iter()
        .find(|v| v.filesystem == Filesystem::RockFs)
        // A volume whose superblocks are damaged is not recognised, but if it is the only
        // thing on the disk it can only be what the user means.
        .or(match (scan.table, scan.volumes.as_slice()) {
            (Table::None, [only]) => Some(only),
            _ => None,
        });
    let Some(volume) = chosen else {
        return Err(if scan.problems.is_empty() {
            "no RockFS volume found".into()
        } else {
            "no RockFS volume found (the partition table is damaged)".into()
        });
    };
    let (start, length) = (volume.start, volume.sectors);
    drop(disk);
    Ok(FileDisk::open(path, writable)?.narrow(start, length))
}
