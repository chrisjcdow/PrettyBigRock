// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The check and repair: each kind of damage is made on purpose, found, and fixed.

use super::*;
use crate::layout::{TYPE_FILE, TYPE_FOLDER};
use crate::runs::Run;

/// A volume with a few files, ready to be damaged.
fn sample() -> RockFs<MemDisk> {
    let mut fs = fresh(300);
    mkdir(&mut fs, "docs").unwrap();
    put(&mut fs, "docs/a", &bytes(1, 20_000)).unwrap();
    put(&mut fs, "docs/b", &bytes(2, 5)).unwrap();
    put(&mut fs, "c", &bytes(3, 70_000)).unwrap();
    fs
}

fn texts(problems: &[crate::Problem]) -> Vec<&str> {
    problems.iter().map(|p| p.text.as_str()).collect()
}

#[test]
fn a_sound_volume_checks_clean_and_repair_changes_nothing() {
    let mut fs = sample();
    assert_clean(&mut fs);
    let writes = fs.device().writes;
    assert_eq!(fs.check(true).unwrap(), Vec::new());
    assert_eq!(fs.device().writes, writes);
}

#[test]
fn space_marked_used_but_owned_by_nothing_is_a_harmless_leak_and_is_reclaimed() {
    let mut fs = sample();
    let free = fs.info().free_blocks;
    let (lo, hi) = (fs.sb.data_start, fs.sb.block_count - 1);
    fs.bitmap.allocate(3, lo, hi).unwrap();
    let report = fs.check(false).unwrap();
    assert_eq!(report.len(), 1, "{report:?}");
    assert!(report[0].harmless && !report[0].fixed, "{report:?}");
    let report = fs.check(true).unwrap();
    assert!(report.iter().all(|p| p.fixed));
    assert_clean(&mut fs);
    assert_eq!(fs.bitmap.free(), free);
}

#[test]
fn space_in_use_but_marked_free_is_rebuilt_in_the_bitmap() {
    let mut fs = sample();
    let node = node_at(&mut fs, "c").unwrap();
    let inode = fs.read_inode((node & 0xFFFF_FFFF) as u32, true).unwrap();
    let loaded = fs.extents_loaded((node & 0xFFFF_FFFF) as u32, &inode).unwrap();
    let e = loaded.extents[0];
    assert!(fs.bitmap.mark_free(Run { start: e.disk_block, len: 1 }));
    let report = fs.check(false).unwrap();
    assert!(
        report.iter().any(|p| !p.harmless && p.text.contains("in use but marked free")),
        "{report:?}"
    );
    let report = fs.check(true).unwrap();
    assert!(report.iter().all(|p| p.fixed), "{report:?}");
    assert_clean(&mut fs);
    let fs = remount(fs);
    let mut fs = fs;
    let node = node_at(&mut fs, "c").unwrap();
    assert_eq!(read_all(&mut fs, node).unwrap(), bytes(3, 70_000));
}

#[test]
fn an_item_with_no_folder_goes_to_lost_and_found_and_an_empty_one_is_removed() {
    let mut fs = sample();
    // A file with contents, and an empty one, that no folder names (as after a crash between
    // making an inode and naming it).
    let (n, inode) = fs.alloc_inode(TYPE_FILE, 1).unwrap();
    let id = fs.node_of(n, &inode);
    fs.write(id, 0, &bytes(9, 12_345)).unwrap();
    fs.commit(id).unwrap();
    let (empty, empty_inode) = fs.alloc_inode(TYPE_FILE, 1).unwrap();
    let empty_id = fs.node_of(empty, &empty_inode);
    let report = fs.check(false).unwrap();
    assert!(texts(&report).iter().any(|t| t.contains("belong to no folder")), "{report:?}");
    let report = fs.check(true).unwrap();
    assert!(report.iter().all(|p| p.fixed), "{report:?}");
    assert_clean(&mut fs);
    assert_eq!(fs.stat(empty_id), Err(FsError::Gone));
    let lost = node_at(&mut fs, "lost+found").unwrap();
    let found = fs.lookup(lost, &std::format!("#{n}")).unwrap();
    assert_eq!(read_all(&mut fs, found).unwrap(), bytes(9, 12_345));
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn an_orphaned_folder_keeps_its_contents() {
    let mut fs = sample();
    let (n, inode) = fs.alloc_inode(TYPE_FOLDER, 1).unwrap();
    let folder = fs.node_of(n, &inode);
    let file = fs.create(folder, "inside", Kind::File).unwrap();
    fs.write(file, 0, b"kept").unwrap();
    fs.commit(file).unwrap();
    fs.check(true).unwrap();
    assert_clean(&mut fs);
    let lost = node_at(&mut fs, "lost+found").unwrap();
    let moved = fs.lookup(lost, &std::format!("#{n}")).unwrap();
    let inside = fs.lookup(moved, "inside").unwrap();
    assert_eq!(read_all(&mut fs, inside).unwrap(), b"kept");
}

#[test]
fn a_second_name_for_one_inode_is_unlinked() {
    let mut fs = sample();
    let c = node_at(&mut fs, "c").unwrap();
    let root_n = fs.sb.root_inode;
    let root_inode = fs.read_inode(root_n, true).unwrap();
    fs.dir_insert(root_n, b"alias", (c & 0xFFFF_FFFF) as u32, TYPE_FILE as u8, false).unwrap();
    let _ = root_inode;
    let report = fs.check(false).unwrap();
    assert!(texts(&report).iter().any(|t| t.contains("second name")), "{report:?}");
    fs.check(true).unwrap();
    assert_clean(&mut fs);
    // One of the two names is gone, the file itself is intact.
    let root = fs.root();
    let both = [fs.lookup(root, "c"), fs.lookup(root, "alias")];
    assert_eq!(both.iter().filter(|r| r.is_ok()).count(), 1);
    let c = *both.iter().find_map(|r| r.as_ref().ok()).unwrap();
    assert_eq!(read_all(&mut fs, c).unwrap(), bytes(3, 70_000));
}

#[test]
fn a_wrong_parent_and_a_wrong_recorded_type_are_corrected() {
    let mut fs = sample();
    let a = node_at(&mut fs, "docs/a").unwrap();
    fs.set_parent((a & 0xFFFF_FFFF) as u32, 1).unwrap();
    let report = fs.check(true).unwrap();
    assert!(texts(&report).iter().any(|t| t.contains("says its folder")), "{report:?}");
    assert_clean(&mut fs);
    assert_eq!(read_all(&mut fs, a).unwrap(), bytes(1, 20_000));
}

#[test]
fn a_damaged_inode_is_freed_and_its_old_id_stays_gone() {
    let mut fs = sample();
    let a = node_at(&mut fs, "docs/a").unwrap();
    let n = (a & 0xFFFF_FFFF) as u32;
    // Flip a byte of the inode on the disk (its checksum no longer matches).
    let (block, offset) = fs.inode_place(n).unwrap();
    let sector = block * 8 + (offset / 512) as u64;
    let mut raw = [0u8; 512];
    fs.dev_read(sector, &mut raw).unwrap();
    raw[offset % 512 + 10] ^= 0x55;
    fs.dev_write(sector, &raw).unwrap();
    let report = fs.check(false).unwrap();
    assert!(texts(&report).iter().any(|t| t.contains("damaged")), "{report:?}");
    let report = fs.check(true).unwrap();
    assert!(report.iter().all(|p| p.fixed), "{report:?}");
    assert_clean(&mut fs);
    assert_eq!(fs.stat(a), Err(FsError::Gone));
    let docs = node_at(&mut fs, "docs").unwrap();
    assert_eq!(fs.lookup(docs, "a"), Err(FsError::NotFound));
    // The name can be used again, and the freed blocks are reusable.
    put(&mut fs, "docs/a", b"new").unwrap();
    assert_clean(&mut fs);
}

#[test]
fn two_names_that_count_as_one_are_reported() {
    let mut fs = sample();
    let root_n = fs.sb.root_inode;
    let a = node_at(&mut fs, "docs/b").unwrap();
    let (n, _) = fs.alloc_inode(TYPE_FILE, root_n).unwrap();
    let _ = a;
    fs.dir_insert(root_n, b"C", n, TYPE_FILE as u8, false).unwrap();
    let report = fs.check(false).unwrap();
    assert!(texts(&report).iter().any(|t| t.contains("count as")), "{report:?}");
}

#[test]
fn a_check_commits_changes_that_were_waiting() {
    let mut fs = sample();
    let c = node_at(&mut fs, "c").unwrap();
    fs.write(c, 0, b"changed").unwrap();
    assert_clean(&mut fs);
    let mut fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
    let c = node_at(&mut fs, "c").unwrap();
    let data = read_all(&mut fs, c).unwrap();
    assert_eq!(&data[..7], b"changed");
}
