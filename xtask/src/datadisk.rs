// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `prettybigrock-data.img`: the disk the release's launcher attaches, already a system volume
//! (docs/design/0026-apps-m6-tests-and-stages.md, Changes, 22 November 2026), so that files are
//! kept from the first start.
//!
//! It is 256 MiB with an MBR and one RockFS partition labelled `Files`, holding what the users
//! service's `setup` would have made for one administrator, `owner` (note 0017, sections 2 and
//! 3): `System/users.txt`, their rules file, and their home with its places. It is written
//! with `lib/rockfs` itself, the same code the filesystem server runs, through a disk held in
//! memory; the file is then written with only the parts that aren't zeros, so it takes little
//! room on a filesystem that can leave holes.
//!
//! The system never sets a disk up without being asked (a blank disk on a real computer may be
//! one someone cares about): this disk comes ready-made instead.

use pbr_fs::{BlockDevice, DeviceError, Filesystem, Kind, NodeId};
use pbr_manifest::Place;
use pbr_manifest::users::{self, User};
use pbr_rockfs::RockFs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SECTOR: usize = 512;
/// The whole disk.
pub const SIZE: u64 = 256 << 20;
/// Where the partition starts: 1 MiB in, as partitioning tools put it.
const START: u64 = 2048;
/// RockFS's MBR partition type (ROCKFS.md, "Where a volume lives").
const MBR_TYPE: u8 = 0x7F;
/// The volume's label, which the Welcome window names.
pub const LABEL: &str = "Files";
/// The one user.
const OWNER: &str = "owner";

/// A disk held in memory, a whole number of sectors.
struct MemDisk(Vec<u8>);

impl BlockDevice for MemDisk {
    fn sectors(&self) -> u64 {
        (self.0.len() / SECTOR) as u64
    }
    fn read_only(&self) -> bool {
        false
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        let at = usize::try_from(sector).map_err(|_| DeviceError)? * SECTOR;
        let part = self.0.get(at..at + buf.len()).ok_or(DeviceError)?;
        buf.copy_from_slice(part);
        Ok(())
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
        let at = usize::try_from(sector).map_err(|_| DeviceError)? * SECTOR;
        let part = self.0.get_mut(at..at + data.len()).ok_or(DeviceError)?;
        part.copy_from_slice(data);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A volume ID: it only has to tell volumes apart, so it comes from the time it was made.
fn new_uuid() -> [u8; 16] {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut x = (nanos as u64) ^ (u64::from(std::process::id()) << 40);
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        // splitmix64: a few multiplications and shifts that spread the bits around.
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out
}

/// A file or folder to make: its path, and a file's contents (`None` for a folder).
type Entry = (String, Option<Vec<u8>>);

/// What goes on the volume, parents first.
fn contents() -> Result<Vec<Entry>, String> {
    let owner = User { name: OWNER.into(), kind: users::Kind::Admin, display: "Owner".into() };
    let users = users::format(&[owner]).ok_or("the users file")?;
    let mut out: Vec<Entry> = vec![
        ("System".into(), None),
        ("System/rules".into(), None),
        // As the users service writes it for a new user (services/users, `make_home`).
        (
            format!("System/rules/{OWNER}.txt"),
            Some(format!("# Rules for {OWNER} (docs/design/0017, section 5)\n").into_bytes()),
        ),
        ("Users".into(), None),
        (format!("Users/{OWNER}"), None),
    ];
    for p in Place::ALL {
        out.push((format!("Users/{OWNER}/{}", p.folder()), None));
    }
    // The users go last, as `setup` writes them: until they are there, this isn't a system
    // volume.
    out.push(("System/users.txt".into(), Some(users.into_bytes())));
    Ok(out)
}

/// Finds the folder at `path` on `fs`.
fn folder_at(fs: &mut RockFs<MemDisk>, path: &str) -> Result<NodeId, String> {
    let mut node = fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        node = fs.lookup(node, part).map_err(|e| format!("{path}: {e:?}"))?;
    }
    Ok(node)
}

/// The volume, `bytes` long: the data disk's partition, or the stick image's third partition
/// (docs/design/0031-stick-tests-and-stages.md, section 1).
pub fn volume(bytes: u64) -> Result<Vec<u8>, String> {
    let mut disk = MemDisk(vec![0; bytes as usize]);
    pbr_rockfs::format(&mut disk, LABEL, new_uuid(), now()).map_err(|e| format!("{e:?}"))?;
    let mut fs = RockFs::mount(disk, now, false).map_err(|e| format!("mounting: {e:?}"))?;
    for (path, data) in contents()? {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
        let at = folder_at(&mut fs, parent)?;
        let fail = |e: pbr_fs::FsError| format!("{path}: {e:?}");
        match data {
            None => {
                fs.create(at, name, Kind::Folder).map_err(fail)?;
            }
            Some(bytes) => {
                let node = fs.create(at, name, Kind::File).map_err(fail)?;
                // (Each file here is far smaller than one write may carry, 64 KiB.)
                if fs.write(node, 0, &bytes).map_err(fail)? != bytes.len() {
                    return Err(format!("{path}: written short"));
                }
                fs.commit(node).map_err(fail)?;
            }
        }
    }
    fs.unmount().map_err(|e| format!("unmounting: {e:?}"))?;
    Ok(fs.into_device().0)
}

/// The MBR: one RockFS partition from [`START`] to the end.
fn mbr() -> [u8; SECTOR] {
    let mut mbr = [0u8; SECTOR];
    let sectors = (SIZE / SECTOR as u64 - START) as u32;
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[4] = MBR_TYPE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&(START as u32).to_le_bytes());
    entry[12..16].copy_from_slice(&sectors.to_le_bytes());
    mbr[440..444].copy_from_slice(&new_uuid()[..4]);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

/// Writes the disk to `path`, leaving holes where it is all zeros.
pub fn write(path: &Path) -> Result<(), String> {
    let start = START as usize * SECTOR;
    let mut disk = vec![0u8; SIZE as usize];
    disk[..SECTOR].copy_from_slice(&mbr());
    let volume = volume(SIZE - start as u64)?;
    disk[start..].copy_from_slice(&volume);
    crate::image::write_sparse(path, &disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The disk as the system will find it: an MBR with one RockFS partition, mounted clean,
    /// with `owner` the one administrator and their home in place.
    #[test]
    fn the_data_disk_is_a_system_volume() {
        let dir = std::env::temp_dir().join(format!("pbr-datadisk-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.img");
        write(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, SIZE);
        // How the storage service will find it.
        struct Whole<'a>(&'a [u8]);
        impl pbr_partition::Disk for Whole<'_> {
            fn sectors(&self) -> u64 {
                (self.0.len() / SECTOR) as u64
            }
            fn read(&mut self, at: u64, buf: &mut [u8]) -> Result<(), pbr_partition::ReadError> {
                let at = at as usize * SECTOR;
                buf.copy_from_slice(&self.0[at..at + buf.len()]);
                Ok(())
            }
        }
        let scan = pbr_partition::scan(&mut Whole(&bytes)).unwrap();
        assert_eq!(scan.table, pbr_partition::Table::Mbr);
        let v: Vec<_> = scan
            .volumes
            .iter()
            .map(|v| (v.partition, v.start, v.sectors, v.filesystem, v.label.as_str()))
            .collect();
        let rockfs = pbr_partition::Filesystem::RockFs;
        assert_eq!(v, [(1, START, SIZE / SECTOR as u64 - START, rockfs, LABEL)]);
        let volume = MemDisk(bytes[(START as usize * SECTOR)..].to_vec());
        let mut fs = RockFs::mount(volume, now, true).unwrap();
        assert!(fs.mount_report().is_empty(), "{:?}", fs.mount_report());
        assert_eq!(fs.info().label, LABEL);
        let users_txt = folder_at(&mut fs, "System").unwrap();
        let node = fs.lookup(users_txt, "users.txt").unwrap();
        let mut buf = vec![0u8; 4096];
        let n = fs.read(node, 0, &mut buf).unwrap();
        let parsed = users::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!((parsed[0].name.as_str(), parsed[0].kind), (OWNER, users::Kind::Admin));
        for p in Place::ALL {
            folder_at(&mut fs, &format!("Users/{OWNER}/{}", p.folder())).unwrap();
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
