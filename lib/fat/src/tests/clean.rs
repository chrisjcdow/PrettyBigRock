// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The "cleanly unmounted" flag, and what a crash leaves behind.
//!
//! Bit 27 of the FAT's second entry says whether the last user shut the volume down properly.
//! Mounting for writing clears it on the disk; unmounting sets it again, last of all. A volume
//! found with the flag clear was not shut down: the counts in its FSInfo sector cannot be
//! trusted (they are only written at unmount), so they are recounted from the FAT.

use super::tools::{Image, need_tools};
use super::*;
use pbr_fs::SECTOR;

/// Where the FAT starts in volumes made by `blank` (32 reserved sectors).
const FAT_AT: usize = 32 * SECTOR;

/// The FAT's second entry, read straight from the disk image.
fn entry1(disk: &MemDisk) -> u32 {
    u32::from_le_bytes(disk.data[FAT_AT + 4..FAT_AT + 8].try_into().unwrap())
}

const CLEAN: u32 = 0x0800_0000;

/// The free count and search hint in the FSInfo sector (sector 1).
fn fsinfo(disk: &MemDisk) -> (u32, u32) {
    let at = SECTOR;
    let get = |o: usize| u32::from_le_bytes(disk.data[at + o..at + o + 4].try_into().unwrap());
    (get(488), get(492))
}

#[test]
fn mounting_clears_the_flag_and_unmounting_sets_it() {
    let disk = blank();
    assert_ne!(entry1(&disk) & CLEAN, 0, "a new volume is clean");
    let mut fs = mount(disk);
    assert!(fs.info().clean_at_mount);
    assert_eq!(entry1(fs.device()) & CLEAN, 0, "cleared on the disk the moment it is mounted");
    put(&mut fs, "f", &bytes(1, 3000)).unwrap();
    assert_eq!(entry1(fs.device()) & CLEAN, 0, "still clear while in use");
    fs.unmount().unwrap();
    assert_ne!(entry1(fs.device()) & CLEAN, 0, "set again by unmount");
    // The rest of the entry is untouched: only bit 27 moved.
    assert_eq!(entry1(fs.device()), 0x0FFF_FFFF);
}

#[test]
fn the_flag_is_set_last() {
    // Everything unmount does reaches the disk before the flag does. Cut the power at each
    // write in turn: until the flag's own writes (one for each copy of the FAT) the volume
    // must still say "not clean".
    let mut base = fresh();
    put(&mut base, "f", &bytes(2, 5000)).unwrap();
    let image = base.device().data.clone();
    let mut probe = mount(MemDisk::from(image.clone()));
    let before = probe.device().writes;
    probe.unmount().unwrap();
    let total = probe.device().writes - before;
    assert!(total >= 3, "unmount writes the FSInfo sector and the flag in both FAT copies");
    for cut in 0..total - 2 {
        let mut fs = mount(MemDisk::from(image.clone()));
        let at = fs.device().writes + cut;
        fs.device_mut().fail_after_writes = Some(at);
        assert_eq!(fs.unmount(), Err(FsError::Disk));
        assert_eq!(entry1(fs.device()) & CLEAN, 0, "cut after {cut} writes");
    }
}

#[test]
fn a_crash_leaves_the_flag_clear_and_the_hints_unused() {
    let mut fs = fresh();
    put(&mut fs, "a", &bytes(3, 10_000)).unwrap();
    fs.unmount().unwrap();
    let mut fs = mount(fs.into_device());
    assert!(fs.info().used_fsinfo, "after a clean unmount the hints are believed");
    put(&mut fs, "b", &bytes(4, 20_000)).unwrap();
    // The power goes: the disk is taken as it is, with no unmount. The FSInfo sector still holds
    // the old count, which is now wrong.
    let crashed = MemDisk::from(fs.device().data.clone());
    let (old_free, _) = fsinfo(&crashed);
    let mut fs = mount(crashed);
    let info = fs.info();
    assert!(!info.clean_at_mount && !info.used_fsinfo);
    assert!(info.free_clusters < old_free, "recounted, so b's clusters are not free");
    // And the data written before the crash is all there.
    let b = node_at(&mut fs, "b").unwrap();
    assert_eq!(read_all(&mut fs, b).unwrap(), bytes(4, 20_000));
    let counted = fs.vol.table.count_free(&mut fs.vol.dev).unwrap();
    assert_eq!(counted, info.free_clusters);
    // A clean unmount afterwards makes it clean again.
    let fs = remount(fs);
    assert!(fs.info().clean_at_mount && fs.info().used_fsinfo);
}

#[test]
fn the_fsinfo_sector_is_only_a_hint() {
    // A clean volume whose FSInfo count is impossible (more free clusters than there are) is
    // recounted, not believed.
    let mut disk = blank();
    let clusters = mount(MemDisk::from(disk.data.clone())).info().cluster_count;
    disk.data[SECTOR + 488..SECTOR + 492].copy_from_slice(&(clusters + 1000).to_le_bytes());
    let fs = mount(disk);
    assert!(!fs.info().used_fsinfo);
    assert_eq!(fs.info().free_clusters, clusters - 1, "all but the root folder's cluster");

    // A count that is wrong but possible is believed on a clean volume (that is what a hint
    // is for), and one that is "unknown" (all ones) is recounted.
    let mut disk = blank();
    disk.data[SECTOR + 488..SECTOR + 492].copy_from_slice(&5u32.to_le_bytes());
    let fs = mount(disk);
    assert!(fs.info().used_fsinfo);
    assert_eq!(fs.info().free_clusters, 5);
    let mut disk = blank();
    disk.data[SECTOR + 488..SECTOR + 492].copy_from_slice(&u32::MAX.to_le_bytes());
    let fs = mount(disk);
    assert!(!fs.info().used_fsinfo);
    assert_eq!(fs.info().free_clusters, clusters - 1);

    // A hint that points outside the volume is not used as a place to start looking.
    let mut disk = blank();
    disk.data[SECTOR + 492..SECTOR + 496].copy_from_slice(&0x0FFF_0000u32.to_le_bytes());
    let mut fs = mount(disk);
    put(&mut fs, "f", b"works").unwrap();

    // The volume without the signatures in its FSInfo sector is mounted and left alone.
    let mut disk = blank();
    disk.data[SECTOR..SECTOR + 4].fill(0);
    let mut fs = mount(disk);
    put(&mut fs, "f", b"works").unwrap();
    fs.unmount().unwrap();
    assert_eq!(&fs.device().data[SECTOR..SECTOR + 4], &[0, 0, 0, 0]);
}

#[test]
fn unmounting_updates_the_hints() {
    let mut fs = fresh();
    put(&mut fs, "a", &bytes(5, 7000)).unwrap();
    let free = fs.info().free_clusters;
    fs.unmount().unwrap();
    let (on_disk, _) = fsinfo(fs.device());
    assert_eq!(on_disk, free);
}

#[test]
fn a_read_only_mount_leaves_the_flag_alone() {
    let mut fs = fresh();
    fs.unmount().unwrap();
    let before = fs.device().data.clone();
    let mut fs = Fat::mount(fs.into_device(), clock, true).unwrap();
    assert_ne!(entry1(fs.device()) & CLEAN, 0);
    fs.unmount().unwrap();
    assert_eq!(fs.device().data, before);
    // Also on a volume that was not clean: read-only does not clean it up.
    let mut dirty = MemDisk::from(before);
    let v = entry1(&dirty) & !CLEAN;
    dirty.data[FAT_AT + 4..FAT_AT + 8].copy_from_slice(&v.to_le_bytes());
    let copy = dirty.data.clone();
    let mut fs = Fat::mount(dirty, clock, true).unwrap();
    assert!(!fs.info().clean_at_mount);
    fs.unmount().unwrap();
    assert_eq!(fs.device().data, copy, "still not clean, and not one byte changed");
}

#[test]
fn fsck_sees_the_flag() {
    need_tools!("mkfs.fat", "fsck.fat");
    let img = Image::mkfs(40_000, &[]);
    let mut fs = mount(img.disk());
    put(&mut fs, "f", &bytes(6, 9000)).unwrap();
    // In use: the tool reports the volume as not shut down.
    img.store(&MemDisk::from(fs.device().data.clone()));
    let out = img.fsck();
    assert!(out.text.contains("Dirty bit is set"), "in use is dirty: {}", out.text);
    // Unmounted: nothing to report.
    fs.unmount().unwrap();
    img.store(fs.device());
    img.assert_clean("after unmount");
}

#[test]
fn the_check_for_a_clean_volume_does_catch_damage() {
    need_tools!("mkfs.fat", "fsck.fat");
    // `assert_clean` is only worth something if it fails when it should. Mark a cluster as
    // used that no file owns (a lost cluster) and it must panic.
    let img = Image::mkfs(40_000, &[]);
    let mut fs = mount(img.disk());
    put(&mut fs, "f", &bytes(7, 2000)).unwrap();
    fs.unmount().unwrap();
    let mut disk = fs.into_device();
    img.store(&disk);
    img.assert_clean("before the damage");
    let fat_start = u16::from_le_bytes([disk.data[14], disk.data[15]]) as usize * SECTOR;
    let at = fat_start + 4 * 5000;
    disk.data[at..at + 4].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    img.store(&disk);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        img.assert_clean("the damaged volume");
    }));
    assert!(caught.is_err(), "a lost cluster must be noticed");
}
