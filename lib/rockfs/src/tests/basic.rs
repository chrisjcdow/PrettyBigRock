// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Round trips and the rules of the `Filesystem` trait.

use super::*;
use pbr_fs::SECTOR;

#[test]
fn format_mount_and_a_first_file() {
    let mut fs = fresh(256);
    let info = fs.info();
    assert_eq!(info.label, "Test");
    assert_eq!(info.uuid, UUID);
    assert_eq!(info.block_count, 256);
    assert_eq!(info.mount_count, 1);
    assert!(info.clean);
    assert_eq!(info.reserve_blocks, 16);
    let sp = fs.space().unwrap();
    assert_eq!(sp.filesystem, "RockFS");
    assert_eq!(sp.total, 256 * 4096);
    assert_eq!(sp.label, "Test");
    // 256 blocks: superblock, bitmap, inode table, backup are used; 16 are reserved.
    assert_eq!(sp.free, (256 - 4 - 16) * 4096);

    let root = fs.root();
    assert_eq!(fs.stat(root).unwrap().kind, Kind::Folder);
    let data = bytes(1, 10_000);
    let file = put(&mut fs, "hello.txt", &data).unwrap();
    assert_eq!(read_all(&mut fs, file).unwrap(), data);
    assert_eq!(fs.lookup(root, "HELLO.TXT").unwrap(), file, "A-Z match a-z");
    assert_eq!(names(&mut fs, root), ["hello.txt"]);
    assert_clean(&mut fs);

    // Survives a clean unmount and a mount.
    let mut fs = remount(fs);
    assert_eq!(fs.info().mount_count, 2);
    let file = node_at(&mut fs, "hello.txt").unwrap();
    assert_eq!(read_all(&mut fs, file).unwrap(), data);
    assert_clean(&mut fs);
}

#[test]
fn stat_reports_size_dates_and_kind() {
    let mut fs = fresh(256);
    let t0 = clock();
    let f = put(&mut fs, "a", b"12345").unwrap();
    let st = fs.stat(f).unwrap();
    assert_eq!(st.kind, Kind::File);
    assert_eq!(st.size, 5);
    assert!(st.created > t0 && st.modified >= st.created);
    assert!(!st.hidden);
    let d = mkdir(&mut fs, "dir").unwrap();
    let st = fs.stat(d).unwrap();
    assert_eq!((st.kind, st.size), (Kind::Folder, 0));
}

#[test]
fn folders_nest_and_list_with_cursors() {
    let mut fs = fresh(512);
    mkdir(&mut fs, "a").unwrap();
    mkdir(&mut fs, "a/b").unwrap();
    mkdir(&mut fs, "a/b/c").unwrap();
    put(&mut fs, "a/b/c/deep.txt", b"deep").unwrap();
    let f = node_at(&mut fs, "a/b/c/deep.txt").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"deep");
    // Many names, so the folder needs several blocks and the listing several calls.
    let dir = node_at(&mut fs, "a").unwrap();
    for i in 0..300 {
        fs.create(dir, &std::format!("file with a fairly long name number {i:04}.txt"), Kind::File)
            .unwrap();
    }
    let listed = names(&mut fs, dir);
    assert_eq!(listed.len(), 301);
    let unique: std::collections::BTreeSet<_> = listed.iter().collect();
    assert_eq!(unique.len(), 301, "no entry repeated");
    // The cursor 0 means the end only when returned.
    let mut count = 0;
    let end = fs
        .list(dir, 0, &mut |_| {
            count += 1;
            true
        })
        .unwrap();
    assert_eq!((count, end), (301, 0));
    assert_clean(&mut fs);
}

#[test]
fn listing_shows_pending_sizes() {
    let mut fs = fresh(256);
    let root = fs.root();
    let f = fs.create(root, "x", Kind::File).unwrap();
    fs.write(f, 0, &[1; 5000]).unwrap();
    let mut sizes = Vec::new();
    fs.list(root, 0, &mut |e| {
        sizes.push(e.size);
        true
    })
    .unwrap();
    assert_eq!(sizes, [5000]);
    assert_eq!(fs.stat(f).unwrap().size, 5000);
}

#[test]
fn names_are_checked() {
    let mut fs = fresh(256);
    let root = fs.root();
    for bad in
        ["a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\\b", "trail.", "trail ", ".", ".."]
    {
        assert_eq!(fs.create(root, bad, Kind::File), Err(FsError::BadName), "{bad:?}");
    }
    let long = "x".repeat(256);
    assert_eq!(fs.create(root, &long, Kind::File), Err(FsError::BadName));
    fs.create(root, &"x".repeat(255), Kind::File).unwrap();
    fs.create(root, "Notes.txt", Kind::File).unwrap();
    assert_eq!(fs.create(root, "NOTES.TXT", Kind::File), Err(FsError::AlreadyExists));
    assert_eq!(fs.create(root, "notes.txt", Kind::Folder), Err(FsError::AlreadyExists));
    // Only A-Z fold.
    fs.create(root, "École", Kind::File).unwrap();
    fs.create(root, "école", Kind::File).unwrap();
    assert_eq!(fs.lookup(root, "ÉCOLE").unwrap(), fs.lookup(root, "École").unwrap());
    assert_eq!(fs.lookup(root, "ÉCOLÉ"), Err(FsError::NotFound));
    // Stored as given.
    assert!(names(&mut fs, root).contains(&String::from("Notes.txt")));
    assert_eq!(fs.lookup(root, "no such"), Err(FsError::NotFound));
    assert_eq!(fs.lookup(root, "a/b"), Err(FsError::NotFound));
    let mut fs = remount(fs);
    let root = fs.root();
    assert!(fs.lookup(root, "école").is_ok());
}

#[test]
fn kinds_are_enforced() {
    let mut fs = fresh(256);
    let root = fs.root();
    let f = put(&mut fs, "f", b"x").unwrap();
    let d = mkdir(&mut fs, "d").unwrap();
    assert_eq!(fs.lookup(f, "x"), Err(FsError::NotAFolder));
    assert_eq!(fs.create(f, "x", Kind::File), Err(FsError::NotAFolder));
    assert_eq!(fs.list(f, 0, &mut |_| true), Err(FsError::NotAFolder));
    assert_eq!(fs.read(d, 0, &mut [0; 4]), Err(FsError::NotAFile));
    assert_eq!(fs.write(d, 0, b"x"), Err(FsError::NotAFile));
    assert_eq!(fs.set_size(d, 5), Err(FsError::NotAFile));
    assert_eq!(fs.remove(f, "x"), Err(FsError::NotAFolder));
    assert_eq!(fs.remove(root, "nothing"), Err(FsError::NotFound));
}

#[test]
fn reads_and_writes_at_odd_offsets_and_holes() {
    let mut fs = fresh(512);
    let root = fs.root();
    let f = fs.create(root, "sparse", Kind::File).unwrap();
    let mut model = std::vec![0u8; 0];
    let mut rng = Rng(77);
    for round in 0..60 {
        let off = rng.below(40_000) as usize;
        let len = 1 + rng.below(9000) as usize;
        let data = bytes(round, len);
        fs.write(f, off as u64, &data).unwrap();
        if model.len() < off + len {
            model.resize(off + len, 0);
        }
        model[off..off + len].copy_from_slice(&data);
        assert_eq!(read_all(&mut fs, f).unwrap(), model, "round {round}");
        if round % 7 == 0 {
            fs.commit(f).unwrap();
        }
    }
    // Reads past the end, and short reads.
    let mut buf = [9u8; 100];
    assert_eq!(fs.read(f, model.len() as u64, &mut buf).unwrap(), 0);
    assert_eq!(fs.read(f, model.len() as u64 + 1000, &mut buf).unwrap(), 0);
    assert_eq!(fs.read(f, model.len() as u64 - 10, &mut buf).unwrap(), 10);
    assert_eq!(&buf[..10], &model[model.len() - 10..]);
    // A big gap is a hole that reads as zeros and takes no space.
    let free = fs.space().unwrap().free;
    fs.write(f, 100_000_000, b"far").unwrap();
    assert!(free - fs.space().unwrap().free <= 4096);
    let mut hole = [7u8; 4096];
    assert_eq!(fs.read(f, 50_000_000, &mut hole).unwrap(), 4096);
    assert!(hole.iter().all(|&b| b == 0));
    fs.commit(f).unwrap();
    assert_clean(&mut fs);
}

#[test]
fn set_size_cuts_and_extends_with_zeros() {
    let mut fs = fresh(256);
    let f = put(&mut fs, "f", &[0xAA; 10_000]).unwrap();
    fs.set_size(f, 5000).unwrap();
    assert_eq!(fs.stat(f).unwrap().size, 5000);
    // Growing again must show zeros, not the 0xAA that used to be there.
    fs.set_size(f, 9000).unwrap();
    let data = read_all(&mut fs, f).unwrap();
    assert!(data[..5000].iter().all(|&b| b == 0xAA));
    assert!(data[5000..].iter().all(|&b| b == 0));
    fs.commit(f).unwrap();
    let data = read_all(&mut fs, f).unwrap();
    assert!(data[5000..].iter().all(|&b| b == 0));
    fs.set_size(f, 0).unwrap();
    assert_eq!(fs.read(f, 0, &mut [0; 8]).unwrap(), 0);
    fs.commit(f).unwrap();
    assert_clean(&mut fs);
    // A truncation frees the space.
    let before = fs.space().unwrap().free;
    let g = put(&mut fs, "g", &bytes(5, 100_000)).unwrap();
    assert!(fs.space().unwrap().free < before);
    fs.set_size(g, 0).unwrap();
    fs.commit(g).unwrap();
    assert_eq!(fs.space().unwrap().free, before);
}

#[test]
fn uncommitted_changes_are_seen_but_committed_ones_survive_a_crash() {
    let mut fs = fresh(256);
    let f = put(&mut fs, "f", b"old contents").unwrap();
    fs.write(f, 0, b"NEW").unwrap();
    // Visible at once ...
    assert_eq!(read_all(&mut fs, f).unwrap(), b"NEW contents");
    // ... but a crash now (no commit, no unmount) leaves the old version.
    let disk = fs.into_device();
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(!fs.info().clean, "the volume was in use");
    let f = node_at(&mut fs, "f").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"old contents");
    // The check at mount reclaimed the blocks the lost write used.
    assert_clean(&mut fs);
}

#[test]
fn commit_makes_changes_durable() {
    let mut fs = fresh(256);
    let f = put(&mut fs, "f", b"first").unwrap();
    fs.write(f, 0, b"SECOND").unwrap();
    fs.commit(f).unwrap();
    // Crash right after the commit: the new version is there.
    let mut fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
    let f = node_at(&mut fs, "f").unwrap();
    assert_eq!(read_all(&mut fs, f).unwrap(), b"SECOND");
    // Committing a file with nothing to commit does nothing, not even a flush.
    let flushes = fs.device().flushes;
    fs.commit(f).unwrap();
    assert_eq!(fs.device().flushes, flushes);
}

#[test]
fn overwriting_uncommitted_blocks_frees_them_at_once() {
    let mut fs = fresh(256);
    let root = fs.root();
    let f = fs.create(root, "f", Kind::File).unwrap();
    let before = fs.space().unwrap().free;
    for i in 0..50 {
        fs.write(f, 0, &bytes(i, 40_000)).unwrap();
    }
    // Fifty rewrites of the same 40 KB hold only one copy of it (10 blocks).
    assert_eq!(before - fs.space().unwrap().free, 10 * 4096);
    fs.commit(f).unwrap();
    assert_eq!(before - fs.space().unwrap().free, 10 * 4096);
}

#[test]
fn remove_and_gone() {
    let mut fs = fresh(256);
    let root = fs.root();
    let free0 = fs.space().unwrap().free;
    let f = put(&mut fs, "f", &bytes(3, 30_000)).unwrap();
    let d = mkdir(&mut fs, "d").unwrap();
    put(&mut fs, "d/inner", b"x").unwrap();
    assert_eq!(fs.remove(root, "d"), Err(FsError::FolderNotEmpty));
    fs.remove(root, "f").unwrap();
    assert_eq!(fs.stat(f), Err(FsError::Gone));
    assert_eq!(fs.read(f, 0, &mut [0; 4]), Err(FsError::Gone));
    assert_eq!(fs.write(f, 0, b"x"), Err(FsError::Gone));
    assert_eq!(fs.commit(f), Err(FsError::Gone));
    assert_eq!(fs.lookup(root, "f"), Err(FsError::NotFound));
    fs.remove(d, "inner").unwrap();
    fs.remove(root, "d").unwrap();
    assert_eq!(fs.stat(d), Err(FsError::Gone));
    assert_eq!(fs.list(d, 0, &mut |_| true), Err(FsError::Gone));
    // A new file reuses the inode but is a different node: the old id stays Gone.
    let g = fs.create(root, "g", Kind::File).unwrap();
    assert_ne!(g, f);
    assert_eq!(fs.stat(f), Err(FsError::Gone));
    assert_eq!(fs.stat(g).unwrap().size, 0);
    fs.remove(root, "g").unwrap();
    assert_eq!(fs.stat(g), Err(FsError::Gone));
    // The blocks came back (a folder keeps one block of entries, as it never shrinks).
    assert!(free0 - fs.space().unwrap().free <= 4096);
    assert_clean(&mut fs);
    // Ids with a wrong generation or an impossible inode number are Gone, not errors of
    // another kind.
    assert_eq!(fs.stat(0), Err(FsError::Gone));
    assert_eq!(fs.stat(0xFFFF_FFFF), Err(FsError::Gone));
    assert_eq!(fs.stat(u64::MAX), Err(FsError::Gone));
}

#[test]
fn removing_a_file_with_pending_changes() {
    let mut fs = fresh(256);
    let root = fs.root();
    put(&mut fs, "anchor", b"x").unwrap();
    let free0 = fs.space().unwrap().free;
    let f = put(&mut fs, "f", &bytes(1, 20_000)).unwrap();
    fs.write(f, 0, &bytes(2, 20_000)).unwrap();
    fs.remove(root, "f").unwrap();
    assert_eq!(fs.commit(f), Err(FsError::Gone));
    assert_eq!(free0, fs.space().unwrap().free);
    assert_clean(&mut fs);
}

#[test]
fn rename_moves_replaces_and_keeps_the_node() {
    let mut fs = fresh(512);
    let root = fs.root();
    let a = mkdir(&mut fs, "a").unwrap();
    let b = mkdir(&mut fs, "b").unwrap();
    let f = put(&mut fs, "a/f", b"file f").unwrap();
    // Plain move to another folder, new name.
    fs.rename(a, "f", b, "moved", false).unwrap();
    assert_eq!(fs.lookup(a, "f"), Err(FsError::NotFound));
    assert_eq!(fs.lookup(b, "MOVED").unwrap(), f, "the node id is unchanged");
    assert_eq!(read_all(&mut fs, f).unwrap(), b"file f");
    // Only the capitals change.
    fs.rename(b, "moved", b, "Moved", false).unwrap();
    assert_eq!(names(&mut fs, b), ["Moved"]);
    fs.rename(b, "Moved", b, "Moved", false).unwrap();
    // Onto an existing name.
    let g = put(&mut fs, "b/other", b"file g").unwrap();
    assert_eq!(fs.rename(b, "Moved", b, "OTHER", false), Err(FsError::AlreadyExists));
    assert_eq!(read_all(&mut fs, g).unwrap(), b"file g");
    // Replacing a file.
    fs.rename(b, "Moved", b, "OTHER", true).unwrap();
    assert_eq!(fs.stat(g), Err(FsError::Gone), "the replaced file is gone");
    assert_eq!(fs.lookup(b, "other").unwrap(), f);
    assert_eq!(names(&mut fs, b), ["OTHER"]);
    // Replace across folders.
    put(&mut fs, "a/x", b"x file").unwrap();
    let xa = node_at(&mut fs, "a/x").unwrap();
    fs.rename(a, "x", b, "other", true).unwrap();
    assert_eq!(fs.lookup(b, "other").unwrap(), xa);
    assert_eq!(fs.stat(f), Err(FsError::Gone));
    assert!(names(&mut fs, a).is_empty());
    // Never a folder.
    let c = mkdir(&mut fs, "a/c").unwrap();
    mkdir(&mut fs, "b/c").unwrap();
    assert_eq!(fs.rename(a, "c", b, "c", true), Err(FsError::AlreadyExists));
    assert!(fs.stat(c).is_ok());
    // A folder can't replace nor be replaced.
    assert_eq!(fs.rename(root, "a", root, "b", true), Err(FsError::AlreadyExists));
    // Missing source, bad name.
    assert_eq!(fs.rename(a, "nope", b, "z", false), Err(FsError::NotFound));
    assert_eq!(fs.rename(b, "other", b, "bad:name", false), Err(FsError::BadName));
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn a_folder_cannot_move_into_itself() {
    let mut fs = fresh(256);
    let root = fs.root();
    let a = mkdir(&mut fs, "a").unwrap();
    let b = mkdir(&mut fs, "a/b").unwrap();
    let c = mkdir(&mut fs, "a/b/c").unwrap();
    assert_eq!(fs.rename(root, "a", a, "x", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(root, "a", b, "x", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(root, "a", c, "x", false), Err(FsError::BadArgument));
    assert_eq!(fs.rename(a, "b", c, "x", false), Err(FsError::BadArgument));
    // Moving it upwards is fine, and so is moving a sibling in.
    fs.rename(a, "b", root, "b", false).unwrap();
    fs.rename(root, "a", b, "a", false).unwrap();
    assert!(node_at(&mut fs, "b/a").is_ok());
    assert_clean(&mut fs);
}

#[test]
fn moving_a_folder_updates_its_parent_link() {
    let mut fs = fresh(256);
    let root = fs.root();
    let a = mkdir(&mut fs, "a").unwrap();
    mkdir(&mut fs, "b").unwrap();
    mkdir(&mut fs, "a/sub").unwrap();
    let b = node_at(&mut fs, "b").unwrap();
    fs.rename(a, "sub", b, "sub", false).unwrap();
    // The check compares each inode's `parent` with the folder holding it.
    assert_clean(&mut fs);
    // Moving a to inside b's sub afterwards must be refused (a is not in b, so it's allowed),
    // but b into a's former child is a loop only if b is above: check both directions.
    let sub = node_at(&mut fs, "b/sub").unwrap();
    assert_eq!(fs.rename(root, "b", sub, "b", false), Err(FsError::BadArgument));
}

#[test]
fn read_only_mounts_refuse_changes() {
    let mut fs = fresh(256);
    let f = put(&mut fs, "f", b"data").unwrap();
    fs.unmount().unwrap();
    let disk = fs.into_device();
    let writes = disk.writes;
    let mut fs = RockFs::mount(disk, clock, true).unwrap();
    let root = fs.root();
    assert!(fs.read_only());
    assert!(fs.info().read_only);
    assert_eq!(fs.info().mount_count, 1, "a read-only mount changes nothing");
    assert_eq!(fs.write(f, 0, b"x"), Err(FsError::ReadOnly));
    assert_eq!(fs.set_size(f, 1), Err(FsError::ReadOnly));
    assert_eq!(fs.create(root, "n", Kind::File), Err(FsError::ReadOnly));
    assert_eq!(fs.remove(root, "f"), Err(FsError::ReadOnly));
    assert_eq!(fs.rename(root, "f", root, "g", false), Err(FsError::ReadOnly));
    assert_eq!(fs.check(true), Err(FsError::ReadOnly));
    assert_eq!(read_all(&mut fs, f).unwrap(), b"data");
    fs.commit(f).unwrap();
    fs.unmount().unwrap();
    // Nothing was written at all.
    let disk = fs.into_device();
    assert_eq!(disk.writes, writes);
    // A read-only device mounts read-only too.
    let mut disk = disk;
    disk.read_only = true;
    let fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(fs.read_only());
}

#[test]
fn many_small_files_grow_the_inode_table() {
    let mut fs = fresh(1024);
    let root = fs.root();
    for i in 0..100 {
        let f = fs.create(root, &std::format!("f{i}"), Kind::File).unwrap();
        fs.write(f, 0, &bytes(i, (i * 37) as usize)).unwrap();
        fs.commit(f).unwrap();
    }
    assert!(fs.info().inode_count >= 112, "the table grew in blocks of 16");
    let mut fs = remount(fs);
    let root = fs.root();
    for i in 0..100 {
        let f = fs.lookup(root, &std::format!("f{i}")).unwrap();
        assert_eq!(read_all(&mut fs, f).unwrap(), bytes(i, (i * 37) as usize));
    }
    assert_clean(&mut fs);
    // The superblock's backup was rewritten with the table, so it still describes it: damage
    // the main copy and everything is still found.
    let mut disk = fs.into_device();
    disk.data[0] ^= 0xFF;
    let mut fs = RockFs::mount(disk, clock, false).unwrap();
    assert!(fs.info().used_backup);
    let root = fs.root();
    assert_eq!(names(&mut fs, root).len(), 100);
    assert_clean(&mut fs);
}

#[test]
fn flushes_come_before_the_writes_that_depend_on_them() {
    // A device that records the order of writes and flushes lets the rule be checked: an
    // inode (which makes a change take effect) is only written after a flush that follows the
    // data it points at.
    use pbr_fs::DeviceError;
    struct Log {
        inner: MemDisk,
        events: Vec<(char, u64)>,
    }
    impl BlockDevice for Log {
        fn sectors(&self) -> u64 {
            self.inner.sectors()
        }
        fn read_only(&self) -> bool {
            false
        }
        fn read(&mut self, s: u64, b: &mut [u8]) -> Result<(), DeviceError> {
            self.inner.read(s, b)
        }
        fn write(&mut self, s: u64, d: &[u8]) -> Result<(), DeviceError> {
            self.events.push(('w', s));
            self.inner.write(s, d)
        }
        fn flush(&mut self) -> Result<(), DeviceError> {
            self.events.push(('f', 0));
            self.inner.flush()
        }
    }
    let mut fs = fresh_on(Log { inner: formatted(256), events: Vec::new() });
    let root = fs.root();
    let f = fs.create(root, "f", Kind::File).unwrap();
    fs.write(f, 0, &[5; 20_000]).unwrap();
    let before = fs.device().events.len();
    fs.commit(f).unwrap();
    let events: Vec<_> = fs.device().events[before..].to_vec();
    // The commit writes exactly one inode sector (the only write of 1 sector, to the inode
    // table's block) after at least one flush, and ends with a flush.
    assert_eq!(events.first().map(|e| e.0), Some('f'), "{events:?}");
    assert_eq!(events.last().map(|e| e.0), Some('f'), "{events:?}");
    assert_eq!(events.iter().filter(|e| e.0 == 'w').count(), 1, "{events:?}");
    let _ = SECTOR;
}
