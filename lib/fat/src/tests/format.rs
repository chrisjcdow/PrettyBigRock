// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A FAT32 formatter for tests that need no outside tool.
//!
//! It makes the layout `mkfs.fat` makes: 32 reserved sectors (boot sector, FSInfo, backups), two
//! FATs, and the root folder in cluster 2. Formatting FAT volumes is not part of the library
//! (note 0016), so it lives here, in the tests only.

use pbr_fs::SECTOR;
use pbr_fs::memdisk::MemDisk;

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A new volume of `total` sectors with `spc` sectors to a cluster, labelled `label` (a label
/// entry in the root folder and in the boot sector, as `mkfs.fat -n` makes).
pub fn blank_with(total: u32, spc: u8, label: &str) -> MemDisk {
    let reserved = 32u32;
    // The FAT must hold clusters + 2 entries; solve for the size that fits what is left.
    let mut fat_sectors = 1u32;
    loop {
        let clusters = (total - reserved - 2 * fat_sectors) / u32::from(spc);
        let need = ((clusters + 2) * 4).div_ceil(SECTOR as u32);
        if need <= fat_sectors {
            break;
        }
        fat_sectors = need;
    }
    let mut d = MemDisk::new(u64::from(total));
    let mut name = [b' '; 11];
    name[..label.len()].copy_from_slice(label.as_bytes());
    {
        let b = &mut d.data[..SECTOR];
        b[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"mkfs.fat");
        put16(b, 11, 512);
        b[13] = spc;
        put16(b, 14, reserved as u16);
        b[16] = 2;
        b[21] = 0xF8;
        put16(b, 24, 32);
        put16(b, 26, 64);
        put32(b, 32, total);
        put32(b, 36, fat_sectors);
        put32(b, 44, 2);
        put16(b, 48, 1);
        put16(b, 50, 6);
        b[64] = 0x80;
        b[66] = 0x29;
        put32(b, 67, 0xCAFE_F00D);
        b[71..82].copy_from_slice(&name);
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510] = 0x55;
        b[511] = 0xAA;
    }
    let clusters = (total - reserved - 2 * fat_sectors) / u32::from(spc);
    {
        let f = &mut d.data[SECTOR..2 * SECTOR];
        put32(f, 0, 0x4161_5252);
        put32(f, 484, 0x6141_7272);
        put32(f, 488, clusters - 1);
        put32(f, 492, 3);
        put32(f, 508, 0xAA55_0000);
    }
    let (boot, info) = (d.data[..SECTOR].to_vec(), d.data[SECTOR..2 * SECTOR].to_vec());
    d.data[6 * SECTOR..7 * SECTOR].copy_from_slice(&boot);
    d.data[7 * SECTOR..8 * SECTOR].copy_from_slice(&info);
    for copy in 0..2 {
        let at = (reserved + copy * fat_sectors) as usize * SECTOR;
        put32(&mut d.data, at, 0x0FFF_FFF8);
        put32(&mut d.data, at + 4, 0x0FFF_FFFF);
        put32(&mut d.data, at + 8, 0x0FFF_FFFF); // the root folder: one cluster
    }
    let root = (reserved + 2 * fat_sectors) as usize * SECTOR;
    if !label.is_empty() {
        d.data[root..root + 11].copy_from_slice(&name);
        d.data[root + 11] = 0x08;
    }
    d
}

/// A volume of 35 MiB with 512-byte clusters (the smallest FAT32 allows is 32 MiB), labelled
/// `TESTVOL`.
pub fn blank() -> MemDisk {
    blank_with(72_000, 1, "TESTVOL")
}
