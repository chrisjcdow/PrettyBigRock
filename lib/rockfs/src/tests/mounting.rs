// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Mounting: what is accepted, what is refused, the backup superblock, features, and the
//! check that runs when a volume was not shut down cleanly.

use super::*;
use crate::MountError;
use crate::superblock::Superblock;

/// Rewrites both copies of the superblock after `edit` changes them (with fresh checksums).
fn edit_superblocks(disk: &mut MemDisk, edit: impl Fn(&mut Superblock)) {
    let last = disk.data.len() - 4096;
    for at in [0, last] {
        let mut sb = Superblock::parse(&disk.data[at..at + 1024]).unwrap();
        edit(&mut sb);
        disk.data[at..at + 1024].copy_from_slice(&sb.to_bytes());
    }
}

fn mount_err(disk: MemDisk) -> MountError {
    match RockFs::mount(disk, clock, false) {
        Ok(_) => panic!("mounted"),
        Err(e) => e,
    }
}

#[test]
fn a_blank_or_tiny_disk_is_not_rockfs() {
    assert_eq!(mount_err(MemDisk::new(8 * 100)), MountError::NotRockFs);
    assert_eq!(mount_err(MemDisk::new(8)), MountError::NotRockFs);
    assert_eq!(mount_err(MemDisk::new(2)), MountError::NotRockFs);
}

#[test]
fn a_clean_volume_mounts_and_counts_mounts() {
    let fs = fresh(100);
    let info = fs.info();
    assert!(!info.read_only);
    assert_eq!(info.mount_count, 1);
    assert_eq!(info.label, "Test");
    assert_eq!(info.uuid, UUID);
    assert_eq!(info.block_count, 100);
    assert!(fs.mount_report().is_empty());
    let fs = remount(fs);
    assert_eq!(fs.info().mount_count, 2);
    assert!(fs.info().last_mounted > 1_700_000_000);
}

#[test]
fn the_backup_superblock_is_used_when_the_main_one_is_damaged() {
    let mut fs = fresh(100);
    put(&mut fs, "f", b"data").unwrap();
    fs.unmount().unwrap();
    let mut disk = fs.into_device();
    disk.data[20] ^= 0xFF;
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(fs.info().used_backup);
    assert!(fs.mount_report().iter().any(|p| p.text.contains("backup")));
    let f = node_at(&mut fs, "f").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"data");
    // The repair at mount put the main copy right again.
    fs.unmount().unwrap();
    let disk = fs.into_device();
    assert!(Superblock::parse(&disk.data[..1024]).is_some());
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(!fs.info().used_backup);
    assert_clean(&mut fs);
}

#[test]
fn the_main_superblock_is_used_when_the_backup_is_damaged() {
    let fs = remount(fresh(100));
    let mut disk = {
        let mut f = fs;
        f.unmount().unwrap();
        f.into_device()
    };
    let last = disk.data.len() - 4096;
    disk.data[last + 100] ^= 1;
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(!fs.info().used_backup);
    // The next change to the superblock restores the backup too.
    fs.unmount().unwrap();
    let disk = fs.into_device();
    let last = disk.data.len() - 4096;
    assert!(Superblock::parse(&disk.data[last..last + 1024]).is_some());
}

#[test]
fn two_damaged_superblocks_are_refused() {
    let mut fs = fresh(100);
    fs.unmount().unwrap();
    let mut disk = fs.into_device();
    let last = disk.data.len() - 4096;
    disk.data[30] ^= 1;
    disk.data[last + 30] ^= 1;
    assert!(matches!(mount_err(disk), MountError::Damaged(_)));
}

#[test]
fn versions_block_sizes_and_incompatible_features_are_refused() {
    for edit in [
        (|sb: &mut Superblock| sb.version = 2) as fn(&mut Superblock),
        |sb| sb.block_shift = 13,
        |sb| sb.features_incompat = 1,
        |sb| sb.features_incompat = 0x8000_0000,
    ] {
        let mut fs = fresh(100);
        fs.unmount().unwrap();
        let mut disk = fs.into_device();
        edit_superblocks(&mut disk, edit);
        assert!(matches!(mount_err(disk), MountError::Unsupported(_)));
    }
}

#[test]
fn unknown_read_only_features_and_name_rules_mount_read_only() {
    for edit in [
        (|sb: &mut Superblock| sb.features_ro_compat = 4) as fn(&mut Superblock),
        |sb| sb.name_rule = 2,
        |sb| sb.name_rule = 0,
    ] {
        let mut fs = fresh(100);
        put(&mut fs, "f", b"x").unwrap();
        fs.unmount().unwrap();
        let mut disk = fs.into_device();
        edit_superblocks(&mut disk, edit);
        let before = disk.data.clone();
        let mut fs = RockFs::mount(disk, clock, false).unwrap();
        assert!(fs.read_only());
        assert!(fs.info().read_only);
        let root = fs.root();
        assert_eq!(fs.create(root, "g", Kind::File), Err(FsError::ReadOnly));
        let f = node_at(&mut fs, "f").unwrap();
        assert_eq!(read_all(&mut fs, f).unwrap(), b"x");
        fs.unmount().unwrap();
        assert!(fs.into_device().data == before, "a read-only mount wrote to the disk");
    }
}

#[test]
fn unknown_compatible_features_are_ignored() {
    let mut fs = fresh(100);
    fs.unmount().unwrap();
    let mut disk = fs.into_device();
    edit_superblocks(&mut disk, |sb| sb.features_compat = 0xFFFF);
    let fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(!fs.read_only());
}

#[test]
fn a_superblock_that_does_not_fit_the_disk_is_refused() {
    for edit in [
        (|sb: &mut Superblock| sb.block_count = 1 << 40) as fn(&mut Superblock),
        |sb| sb.block_count = 50,
        |sb| sb.data_start += 1,
        |sb| sb.bitmap_start = 5,
        |sb| sb.bitmap_blocks = 0,
        |sb| sb.root_inode = 0,
        |sb| sb.root_inode = 9999,
        |sb| sb.inode_count = 1,
    ] {
        let mut fs = fresh(100);
        fs.unmount().unwrap();
        let mut disk = fs.into_device();
        edit_superblocks(&mut disk, edit);
        assert!(matches!(mount_err(disk), MountError::Damaged(_) | MountError::NotRockFs));
    }
}

#[test]
fn an_unclean_volume_is_checked_and_repaired_at_mount() {
    let mut fs = fresh(200);
    put(&mut fs, "kept", &bytes(1, 30_000)).unwrap();
    let lost = fs.create(fs.root(), "lost", Kind::File).unwrap();
    fs.write(lost, 0, &bytes(2, 20_000)).unwrap();
    // Not committed, not unmounted: as if the power failed. Its blocks were allocated in
    // memory only, so the disk's bitmap does not know about them; nothing to find on disk.
    let mut disk = fs.into_device();
    let mut fs = RockFs::mount(
        {
            disk.data.truncate(disk.data.len());
            disk
        },
        clock,
        false,
    )
    .unwrap();
    assert_eq!(fs.info().mount_count, 2);
    assert!(fs.mount_report().iter().all(|p| p.harmless || p.fixed), "{:?}", fs.mount_report());
    // The uncommitted file's name was written (create is durable at once) but it has no data.
    let lost = node_at(&mut fs, "lost").unwrap();
    assert_eq!(fs.stat(lost).unwrap().size, 0);
    let kept = node_at(&mut fs, "kept").unwrap();
    assert_eq!(read_all(&mut fs, kept).unwrap(), bytes(1, 30_000));
    assert_clean(&mut fs);
    assert!(!fs.info().clean);
    fs.unmount().unwrap();
    let fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
    assert!(fs.info().clean);
}

#[test]
fn a_read_only_mount_of_an_unclean_volume_changes_nothing() {
    let mut fs = fresh(100);
    put(&mut fs, "f", b"x").unwrap();
    let disk = fs.into_device();
    let before = disk.data.clone();
    let mut fs = RockFs::mount(disk, clock, true).unwrap();
    assert!(fs.read_only());
    assert!(!fs.info().clean);
    assert_eq!(fs.check(true), Err(FsError::ReadOnly));
    // A read-only check works, and says what it sees.
    let _ = fs.check(false).unwrap();
    fs.unmount().unwrap();
    assert!(fs.into_device().data == before);
}

#[test]
fn a_read_only_device_mounts_read_only() {
    let mut fs = fresh(100);
    fs.unmount().unwrap();
    let mut disk = fs.into_device();
    disk.read_only = true;
    let fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(fs.read_only());
}

#[test]
fn a_failing_disk_stops_changes_and_keeps_the_volume_unclean() {
    let mut fs = fresh(100);
    put(&mut fs, "f", b"x").unwrap();
    let writes = fs.device().writes;
    fs.device_mut().fail_after_writes = Some(writes + 2);
    let root = fs.root();
    let mut failed = false;
    for i in 0..10 {
        if fs.create(root, &std::format!("n{i}"), Kind::File).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    // Once a write has failed, later changes are refused rather than risk a muddle...
    assert!(fs.create(root, "after", Kind::File).is_err());
    // ...and unmounting must not mark the volume clean.
    let _ = fs.unmount();
    let mut disk = fs.into_device();
    disk.fail_after_writes = None;
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(!fs.mount_report().iter().any(|p| !p.harmless && !p.fixed), "{:?}", fs.mount_report());
    assert_clean(&mut fs);
}
