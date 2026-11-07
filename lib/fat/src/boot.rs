// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The boot sector, and the FSInfo sector beside it.
//!
//! A FAT32 volume starts with a few *reserved sectors*. The first, the **boot sector**,
//! describes the volume's layout: how big a cluster is (a *cluster* is the unit in which files
//! get space: a power-of-two number of sectors), where the FAT is (the *File Allocation Table*:
//! one 32-bit entry per cluster saying which cluster comes next in the file, or that the file
//! ends there), where the data area starts, and where the root folder is. The **FSInfo
//! sector** holds two hints, the number of free clusters and where to start looking for one.
//!
//! None of it is trusted: this volume may come from a hostile USB stick. `Geometry::parse`
//! checks every number against the others and against the size of the disk, so the rest of the
//! library can do arithmetic on them without overflowing or reaching outside the volume.

use crate::error::MountError;
use pbr_fs::SECTOR;

/// Bytes in a sector, as a `u32` for arithmetic.
pub(crate) const SECTOR32: u32 = SECTOR as u32;
/// The fewest clusters a FAT32 volume has. Microsoft's specification decides which kind of FAT a
/// volume is *only* by its cluster count: fewer than 4085 is FAT12, fewer than 65525 FAT16.
pub(crate) const MIN_CLUSTERS: u32 = 65_525;
/// The highest cluster number FAT32 can use: the FAT's entries are 28 bits, and the top values
/// mean "bad cluster" and "end of file".
pub(crate) const MAX_CLUSTER_NUMBER: u32 = 0x0FFF_FFF5;

/// What the boot sector says about the volume, checked.
#[derive(Clone, Debug)]
pub(crate) struct Geometry {
    /// Sectors in a cluster: 1, 2, 4, ... 128.
    pub sectors_per_cluster: u32,
    /// Bytes in a cluster.
    pub cluster_bytes: u32,
    /// Sectors before the first FAT.
    pub reserved: u32,
    /// Copies of the FAT (1 or 2).
    pub fats: u32,
    /// Sectors in one copy of the FAT.
    pub fat_sectors: u32,
    /// The first sector of the data area (where cluster 2 is), counted from the volume's start.
    pub data_start: u64,
    /// How many clusters there are. They are numbered from 2, so the last is `clusters + 1`.
    pub clusters: u32,
    /// The root folder's first cluster.
    pub root: u32,
    /// The FSInfo sector, if there is one.
    pub fsinfo: Option<u32>,
    /// Whether every copy of the FAT is kept the same. If not, only the active copy is used
    /// (and, since we only support copy 0, only copy 0 is written).
    pub mirror: bool,
    /// The serial number (0 if the boot sector has none).
    pub volume_id: u32,
    /// The label in the boot sector, if it has one.
    pub label: Option<[u8; 11]>,
}

fn le16(b: &[u8; SECTOR], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn le32(b: &[u8; SECTOR], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Geometry {
    /// Checks the boot sector `b` of a volume of `volume_sectors` sectors.
    ///
    /// Which error to give: a sector without the 0x55AA signature, or with a sector size or
    /// cluster size no FAT has, is not FAT at all ([`MountError::NotFat`]) unless it says
    /// "FAT32" in the place FAT32 boot sectors do, in which case it is a damaged FAT32 volume.
    pub fn parse(b: &[u8; SECTOR], volume_sectors: u64) -> Result<Geometry, MountError> {
        if b[510] != 0x55 || b[511] != 0xAA {
            return Err(MountError::NotFat);
        }
        if &b[3..11] == b"EXFAT   " {
            return Err(MountError::Unsupported("exFAT is not supported yet"));
        }
        let claims_fat32 = &b[82..90] == b"FAT32   ";
        let bad = |why: &'static str| {
            if claims_fat32 { MountError::Damaged(why) } else { MountError::NotFat }
        };
        // The jump instruction at the start (a relative jump over the data that follows, then
        // a no-op) is how a PC's firmware would run a bootable volume. Windows does not insist
        // on it, and nothing here depends on it, so it is not checked.
        match le16(b, 11) {
            512 => {}
            1024 | 2048 | 4096 => {
                return Err(MountError::Unsupported(
                    "sectors other than 512 bytes are not supported",
                ));
            }
            _ => return Err(bad("the sector size is not one FAT can have")),
        }
        let sectors_per_cluster = u32::from(b[13]);
        if !sectors_per_cluster.is_power_of_two() || sectors_per_cluster > 128 {
            return Err(bad("the cluster size is not a power of two from 1 to 128 sectors"));
        }
        // FAT12 and FAT16 keep their root folder in a fixed area and their FAT size in the
        // older 16-bit field; FAT32 sets both to zero.
        if le16(b, 17) != 0 || le16(b, 22) != 0 {
            return Err(MountError::Unsupported("FAT12 and FAT16 are not supported yet"));
        }
        let reserved = le16(b, 14);
        let fats = u32::from(b[16]);
        let fat_sectors = le32(b, 36);
        // The 32-bit sector count is the one FAT32 uses. Some writers fill only the older
        // 16-bit one for a very small volume; there it would be FAT16 anyway, but we accept it
        // here and let the cluster count below decide.
        let total = match le32(b, 32) {
            0 => u64::from(le16(b, 19)),
            n => u64::from(n),
        };
        if total == 0 || reserved == 0 || fat_sectors == 0 {
            return Err(MountError::Damaged("the boot sector has a zero where a size must be"));
        }
        if fats != 1 && fats != 2 {
            return Err(MountError::Damaged("the number of FAT copies is not 1 or 2"));
        }
        if total > volume_sectors {
            return Err(MountError::Damaged("the volume is bigger than the disk it is on"));
        }
        // All of these are at most 32 bits, so in `u64` they cannot overflow.
        let data_start = u64::from(reserved) + u64::from(fats) * u64::from(fat_sectors);
        if data_start >= total {
            return Err(MountError::Damaged("the reserved sectors and FATs fill the volume"));
        }
        let clusters = (total - data_start) / u64::from(sectors_per_cluster);
        if clusters < u64::from(MIN_CLUSTERS) {
            // Microsoft's rule: a volume this small is FAT16 whatever its boot sector says, and
            // Windows would read it as such. Linux mounts it anyway; we don't.
            return Err(MountError::Unsupported(
                "too few clusters for FAT32 (the specification's rule for telling FAT types)",
            ));
        }
        if clusters > u64::from(MAX_CLUSTER_NUMBER) - 1 {
            return Err(MountError::Damaged("more clusters than a FAT32 volume can have"));
        }
        let clusters = clusters as u32; // checked just above
        // Entries 0 and 1 of the FAT are reserved, so it needs `clusters + 2` entries of 4 bytes.
        if (u64::from(clusters) + 2) * 4 > u64::from(fat_sectors) * u64::from(SECTOR32) {
            return Err(MountError::Damaged("the FAT is too small for the number of clusters"));
        }
        let root = le32(b, 44);
        if root < 2 || root > clusters + 1 {
            return Err(MountError::Damaged("the root folder's cluster is outside the volume"));
        }
        if le16(b, 42) != 0 {
            return Err(MountError::Unsupported("a version of FAT32 newer than 0.0"));
        }
        // Extended flags: bit 7 means the FAT is not mirrored and only copy `n` (bits 0-3) is
        // in use. Using any copy but the first is more than we do.
        let flags = le16(b, 40);
        let mirror = flags & 0x80 == 0;
        if !mirror && flags & 0x0F != 0 {
            return Err(MountError::Unsupported("the active FAT is not the first copy"));
        }
        let fsinfo = match le16(b, 48) {
            0 | 0xFFFF => None,
            n if n < reserved => Some(n),
            _ => None,
        };
        // The serial number and label are only there if the "extended boot signature" says so.
        let extended = b[66] == 0x29;
        Ok(Geometry {
            sectors_per_cluster,
            cluster_bytes: sectors_per_cluster * SECTOR32,
            reserved,
            fats,
            fat_sectors,
            data_start,
            clusters,
            root,
            fsinfo,
            mirror,
            volume_id: if extended { le32(b, 67) } else { 0 },
            label: extended.then(|| {
                let mut label = [0u8; 11];
                label.copy_from_slice(&b[71..82]);
                label
            }),
        })
    }

    /// The highest valid cluster number.
    pub fn last_cluster(&self) -> u32 {
        self.clusters + 1
    }

    /// Whether `cluster` is a data cluster of this volume.
    pub fn is_data_cluster(&self, cluster: u32) -> bool {
        (2..=self.last_cluster()).contains(&cluster)
    }

    /// The first byte of `cluster`, counted from the start of the volume, or `None` if it is
    /// not a data cluster.
    pub fn cluster_offset(&self, cluster: u32) -> Option<u64> {
        self.is_data_cluster(cluster).then(|| {
            (self.data_start + u64::from(cluster - 2) * u64::from(self.sectors_per_cluster))
                * SECTOR as u64
        })
    }
}

/// The hints in the FSInfo sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FsInfo {
    /// Free clusters, if the sector believably says.
    pub free: Option<u32>,
    /// Where to start looking for a free cluster, if it believably says.
    pub next_free: Option<u32>,
}

/// Where the free-cluster count sits in the FSInfo sector.
const FSINFO_FREE: usize = 488;
/// Where the "start looking here" hint sits.
const FSINFO_NEXT: usize = 492;

impl FsInfo {
    /// Reads the hints from `sector`. `None` if it does not carry the three signatures that mark
    /// an FSInfo sector. A count or hint that is impossible for this volume is dropped and the
    /// other is kept, since each is a hint on its own.
    pub fn parse(sector: &[u8; SECTOR], geo: &Geometry) -> Option<FsInfo> {
        if !FsInfo::has_signatures(sector) {
            return None;
        }
        let free = le32(sector, FSINFO_FREE);
        let next = le32(sector, FSINFO_NEXT);
        Some(FsInfo {
            free: (free <= geo.clusters).then_some(free),
            next_free: geo.is_data_cluster(next).then_some(next),
        })
    }

    /// Whether `sector` carries the signatures of an FSInfo sector.
    pub fn has_signatures(sector: &[u8; SECTOR]) -> bool {
        le32(sector, 0) == 0x4161_5252
            && le32(sector, 484) == 0x6141_7272
            && le32(sector, 508) == 0xAA55_0000
    }

    /// Puts a free count and a hint into an FSInfo `sector` that [`FsInfo::has_signatures`].
    pub fn store(sector: &mut [u8; SECTOR], free: u32, next_free: u32) {
        sector[FSINFO_FREE..FSINFO_FREE + 4].copy_from_slice(&free.to_le_bytes());
        sector[FSINFO_NEXT..FSINFO_NEXT + 4].copy_from_slice(&next_free.to_le_bytes());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn put16(b: &mut [u8], at: usize, v: u16) {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// A good boot sector for a volume of `TOTAL` sectors: 1 sector per cluster, 32 reserved
    /// sectors, two FATs of 1024 sectors (room for 131072 entries).
    pub const TOTAL: u32 = 32 + 2 * 1024 + 70_000;

    pub fn good() -> [u8; SECTOR] {
        let mut b = [0u8; SECTOR];
        b[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"TESTFAT ");
        put16(&mut b, 11, 512);
        b[13] = 1;
        put16(&mut b, 14, 32);
        b[16] = 2;
        b[21] = 0xF8;
        put32(&mut b, 32, TOTAL);
        put32(&mut b, 36, 1024);
        put32(&mut b, 44, 2);
        put16(&mut b, 48, 1);
        put16(&mut b, 50, 6);
        b[66] = 0x29;
        put32(&mut b, 67, 0x1234_5678);
        b[71..82].copy_from_slice(b"TEST LABEL ");
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    }

    fn parse(b: &[u8; SECTOR]) -> Result<Geometry, MountError> {
        Geometry::parse(b, u64::from(TOTAL))
    }

    #[test]
    fn a_good_boot_sector() {
        let g = parse(&good()).unwrap();
        assert_eq!(g.sectors_per_cluster, 1);
        assert_eq!(g.cluster_bytes, 512);
        assert_eq!(g.data_start, 32 + 2048);
        assert_eq!(g.clusters, 70_000);
        assert_eq!(g.last_cluster(), 70_001);
        assert_eq!(g.root, 2);
        assert_eq!(g.fsinfo, Some(1));
        assert!(g.mirror);
        assert_eq!(g.volume_id, 0x1234_5678);
        assert_eq!(g.label, Some(*b"TEST LABEL "));
        assert_eq!(g.cluster_offset(2), Some((32 + 2048) * 512));
        assert_eq!(g.cluster_offset(3), Some((32 + 2048 + 1) * 512));
        assert_eq!(g.cluster_offset(1), None);
        assert_eq!(g.cluster_offset(70_002), None);
        assert!(g.is_data_cluster(70_001));
    }

    fn change(f: impl FnOnce(&mut [u8; SECTOR])) -> Result<Geometry, MountError> {
        let mut b = good();
        f(&mut b);
        parse(&b)
    }

    #[test]
    fn not_fat_at_all() {
        assert_eq!(change(|b| b[510] = 0).unwrap_err(), MountError::NotFat);
        assert_eq!(change(|b| b[511] = 0).unwrap_err(), MountError::NotFat);
        assert_eq!(parse(&[0u8; SECTOR]).unwrap_err(), MountError::NotFat);
        // Nonsense sector and cluster sizes, in a sector that does not say FAT32.
        let plain = |f: fn(&mut [u8; SECTOR])| {
            change(|b| {
                b[82..90].fill(0);
                f(b)
            })
        };
        assert_eq!(plain(|b| put16(b, 11, 0)).unwrap_err(), MountError::NotFat);
        assert_eq!(plain(|b| put16(b, 11, 513)).unwrap_err(), MountError::NotFat);
        assert_eq!(plain(|b| b[13] = 0).unwrap_err(), MountError::NotFat);
        assert_eq!(plain(|b| b[13] = 3).unwrap_err(), MountError::NotFat);
    }

    #[test]
    fn damaged_fat32() {
        // The same nonsense, in a sector that says it is FAT32.
        for f in [
            (|b: &mut [u8; SECTOR]| put16(b, 11, 0)) as fn(&mut [u8; SECTOR]),
            |b| put16(b, 11, 513),
            |b| b[13] = 0,
            |b| b[13] = 3,
            |b| b[13] = 255,
            |b| put16(b, 14, 0),
            |b| b[16] = 0,
            |b| b[16] = 3,
            |b| put32(b, 36, 0),
            |b| put32(b, 32, 0),
            // Bigger than the disk.
            |b| put32(b, 32, TOTAL + 1),
            // The reserved area and FATs fill the volume.
            |b| put32(b, 36, TOTAL),
            |b| put32(b, 36, u32::MAX),
            // FAT too small for the clusters (a 1-sector FAT holds 128 entries).
            |b| put32(b, 36, 100),
            // Root cluster outside the volume.
            |b| put32(b, 44, 0),
            |b| put32(b, 44, 1),
            |b| put32(b, 44, 70_002),
            |b| put32(b, 44, u32::MAX),
        ] {
            assert!(matches!(change(f), Err(MountError::Damaged(_))), "{:?}", change(f));
        }
    }

    #[test]
    fn unsupported_kinds() {
        let un = |f: fn(&mut [u8; SECTOR])| matches!(change(f), Err(MountError::Unsupported(_)));
        assert!(un(|b| put16(b, 11, 4096)));
        assert!(un(|b| put16(b, 11, 1024)));
        // FAT16: root entries and the 16-bit FAT size are set.
        assert!(un(|b| put16(b, 17, 512)));
        assert!(un(|b| put16(b, 22, 100)));
        assert!(un(|b| b[3..11].copy_from_slice(b"EXFAT   ")));
        assert!(un(|b| put16(b, 42, 1)));
        // Only the second FAT in use.
        assert!(un(|b| put16(b, 40, 0x81)));
        // Too few clusters for FAT32: 65524 clusters of 1 sector.
        assert!(un(|b| put32(b, 32, 32 + 2048 + 65_524)));
    }

    #[test]
    fn the_smallest_fat32_volume() {
        let mut b = good();
        put32(&mut b, 32, 32 + 2048 + 65_525);
        assert_eq!(Geometry::parse(&b, u64::from(TOTAL)).unwrap().clusters, 65_525);
        // Sectors at the end that do not make a whole cluster do not count.
        b[13] = 2;
        put32(&mut b, 32, 32 + 2048 + 2 * 65_525 + 1);
        assert_eq!(Geometry::parse(&b, u64::from(TOTAL) * 2).unwrap().clusters, 65_525);
        put32(&mut b, 32, 32 + 2048 + 2 * 65_525 - 1);
        assert!(matches!(
            Geometry::parse(&b, u64::from(TOTAL) * 2),
            Err(MountError::Unsupported(_))
        ));
    }

    #[test]
    fn options() {
        // Mirroring switched off with the first copy active: allowed, one copy is written.
        let g = change(|b| put16(b, 40, 0x80)).unwrap();
        assert!(!g.mirror);
        // No FSInfo, or one outside the reserved sectors.
        assert_eq!(change(|b| put16(b, 48, 0)).unwrap().fsinfo, None);
        assert_eq!(change(|b| put16(b, 48, 0xFFFF)).unwrap().fsinfo, None);
        assert_eq!(change(|b| put16(b, 48, 32)).unwrap().fsinfo, None);
        assert_eq!(change(|b| put16(b, 48, 31)).unwrap().fsinfo, Some(31));
        // No extended signature: no label or serial number.
        let g = change(|b| b[66] = 0).unwrap();
        assert_eq!((g.label, g.volume_id), (None, 0));
        // 128 sectors per cluster is the largest size; this small volume then has too few.
        assert!(matches!(change(|b| b[13] = 128), Err(MountError::Unsupported(_))));
    }

    #[test]
    fn fsinfo_hints() {
        let g = parse(&good()).unwrap();
        let mut s = [0u8; SECTOR];
        assert_eq!(FsInfo::parse(&s, &g), None);
        put32(&mut s, 0, 0x4161_5252);
        put32(&mut s, 484, 0x6141_7272);
        put32(&mut s, 508, 0xAA55_0000);
        put32(&mut s, FSINFO_FREE, 1234);
        put32(&mut s, FSINFO_NEXT, 77);
        assert_eq!(FsInfo::parse(&s, &g), Some(FsInfo { free: Some(1234), next_free: Some(77) }));
        // Unknown (0xFFFFFFFF), too many, and out-of-range hints are dropped separately.
        put32(&mut s, FSINFO_FREE, u32::MAX);
        put32(&mut s, FSINFO_NEXT, 1);
        assert_eq!(FsInfo::parse(&s, &g), Some(FsInfo { free: None, next_free: None }));
        put32(&mut s, FSINFO_FREE, 70_001);
        put32(&mut s, FSINFO_NEXT, 70_001);
        assert_eq!(FsInfo::parse(&s, &g), Some(FsInfo { free: None, next_free: Some(70_001) }));
        put32(&mut s, FSINFO_FREE, 70_000);
        assert_eq!(FsInfo::parse(&s, &g).unwrap().free, Some(70_000));
        FsInfo::store(&mut s, 5, 9);
        assert_eq!(FsInfo::parse(&s, &g), Some(FsInfo { free: Some(5), next_free: Some(9) }));
        put32(&mut s, 508, 0);
        assert_eq!(FsInfo::parse(&s, &g), None);
    }
}
