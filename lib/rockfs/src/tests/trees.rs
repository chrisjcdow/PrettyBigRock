// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Files so fragmented that their extent lists need a tree of blocks, one and two levels deep.

use super::*;

/// The depth of the extent tree of a file (0 = the extents sit in the inode).
fn depth(fs: &mut RockFs<MemDisk>, node: NodeId) -> u16 {
    fs.read_inode((node & 0xFFFF_FFFF) as u32, true).unwrap().depth
}

/// Grows two files in step, one block at a time and committing each time, so that their blocks
/// alternate on the disk and neither can be stored in fewer extents than it has blocks.
/// Returns the two files' contents so far.
fn interleave(fs: &mut RockFs<MemDisk>, blocks: usize) -> (NodeId, NodeId, Vec<u8>, Vec<u8>) {
    let a = fs.create(fs.root(), "a", Kind::File).unwrap();
    let b = fs.create(fs.root(), "b", Kind::File).unwrap();
    let (mut da, mut db) = (Vec::new(), Vec::new());
    for i in 0..blocks {
        let x = bytes(i as u64 * 2, 4096);
        let y = bytes(i as u64 * 2 + 1, 4096);
        fs.write(a, da.len() as u64, &x).unwrap();
        fs.commit(a).unwrap();
        fs.write(b, db.len() as u64, &y).unwrap();
        fs.commit(b).unwrap();
        da.extend_from_slice(&x);
        db.extend_from_slice(&y);
    }
    (a, b, da, db)
}

#[test]
fn a_fragmented_file_gets_a_one_level_tree() {
    let mut fs = fresh(512);
    let (a, b, da, db) = interleave(&mut fs, 40);
    assert_eq!(depth(&mut fs, a), 1);
    assert_eq!(read_all(&mut fs, a).unwrap(), da);
    assert_eq!(read_all(&mut fs, b).unwrap(), db);
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    assert_eq!(read_all(&mut fs, a).unwrap(), da);
    assert_eq!(read_all(&mut fs, b).unwrap(), db);
    assert_clean(&mut fs);
}

#[test]
fn a_very_fragmented_file_gets_a_two_level_tree() {
    // More than 7 * 170 extents are needed for a second level.
    let mut fs = fresh(4096);
    let (a, b, da, db) = interleave(&mut fs, 1300);
    assert!(depth(&mut fs, a) >= 2, "depth {}", depth(&mut fs, a));
    assert_eq!(read_all(&mut fs, a).unwrap(), da);
    assert_eq!(read_all(&mut fs, b).unwrap(), db);
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    // Random reads across the tree.
    let mut rng = Rng(5);
    for _ in 0..200 {
        let off = rng.below(da.len() as u64 - 1) as usize;
        let len = (rng.below(9000) as usize + 1).min(da.len() - off);
        let mut buf = std::vec![0u8; len];
        assert_eq!(fs.read(a, off as u64, &mut buf).unwrap(), len);
        assert_eq!(buf, da[off..off + len]);
    }
    // Change the middle, shrink, and remove: every tree block must come back.
    let before = fs.space().unwrap().free;
    fs.write(a, 1000 * 4096 + 17, b"hello").unwrap();
    fs.commit(a).unwrap();
    assert_clean(&mut fs);
    fs.set_size(a, 3 * 4096 + 5).unwrap();
    fs.commit(a).unwrap();
    assert_eq!(depth(&mut fs, a), 1.min(depth(&mut fs, a)));
    assert_clean(&mut fs);
    fs.remove(fs.root(), "a").unwrap();
    fs.remove(fs.root(), "b").unwrap();
    assert_clean(&mut fs);
    assert!(fs.space().unwrap().free > before);
    let mut fs = remount(fs);
    assert_clean(&mut fs);
}

#[test]
fn shrinking_a_tree_back_to_the_inode_frees_the_tree_blocks() {
    let mut fs = fresh(512);
    let (a, _b, da, _) = interleave(&mut fs, 30);
    assert!(depth(&mut fs, a) >= 1);
    fs.set_size(a, 4 * 4096).unwrap();
    fs.commit(a).unwrap();
    assert_eq!(depth(&mut fs, a), 0);
    assert_eq!(read_all(&mut fs, a).unwrap(), da[..4 * 4096]);
    assert_clean(&mut fs);
}

#[test]
fn rewriting_blocks_of_a_fragmented_file_keeps_it_whole() {
    let mut fs = fresh(1024);
    let (a, _b, mut da, _) = interleave(&mut fs, 60);
    let mut rng = Rng(77);
    for round in 0..40 {
        let off = rng.below(da.len() as u64) as usize;
        let len = (rng.below(12000) as usize + 1).min(da.len() - off);
        let data = bytes(1000 + round, len);
        fs.write(a, off as u64, &data).unwrap();
        da[off..off + len].copy_from_slice(&data);
        if round % 3 == 0 {
            fs.commit(a).unwrap();
        }
    }
    fs.commit(a).unwrap();
    assert_eq!(read_all(&mut fs, a).unwrap(), da);
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    assert_eq!(read_all(&mut fs, a).unwrap(), da);
    assert_clean(&mut fs);
}
