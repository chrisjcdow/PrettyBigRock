// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Hostile and damaged disks. Whatever is on the disk, mounting and using the volume must end
//! in a result: never a panic, an endless loop, an out-of-range access or a huge allocation.
//! (A panic fails the test; an endless loop would hang it.)
//!
//! A volume with every kind of structure is damaged at random in several ways, and then
//! mounted, walked, read, checked, repaired and written to.

use super::*;
use crate::extents::xblock_checksum;
use crate::inode::Top;
use crate::superblock::Superblock;

/// A volume with nested folders, a folder of several blocks, files with extent trees, and a
/// sparse file, unmounted cleanly.
fn base() -> MemDisk {
    let mut fs = fresh(300);
    mkdir(&mut fs, "docs").unwrap();
    mkdir(&mut fs, "docs/deep").unwrap();
    mkdir(&mut fs, "many").unwrap();
    for i in 0..90 {
        put(
            &mut fs,
            &std::format!("many/a-file-with-a-longish-name-{i}"),
            &bytes(i, 100 + i as usize * 50),
        )
        .unwrap();
    }
    put(&mut fs, "docs/deep/small", b"hello").unwrap();
    put(&mut fs, "docs/big", &bytes(7, 60_000)).unwrap();
    // Two files grown in step so each has a tree of extent blocks.
    let a = fs.create(fs.root(), "frag-a", Kind::File).unwrap();
    let b = fs.create(fs.root(), "frag-b", Kind::File).unwrap();
    for i in 0..24u64 {
        fs.write(a, i * 4096, &bytes(i, 4096)).unwrap();
        fs.commit(a).unwrap();
        fs.write(b, i * 4096, &bytes(i + 100, 4096)).unwrap();
        fs.commit(b).unwrap();
    }
    let s = fs.create(fs.root(), "sparse", Kind::File).unwrap();
    fs.write(s, 1 << 33, b"far").unwrap();
    fs.commit(s).unwrap();
    fs.unmount().unwrap();
    fs.into_device()
}

/// Gives every checksum on the disk its right value again, so that damage reaches the checks
/// that come after the checksums.
fn fix_checksums(data: &mut [u8], table_blocks: &[u64]) {
    let last = data.len() - 4096;
    for at in [0, last] {
        let sum = pbr_crc::crc32(&data[at..at + 1020]);
        data[at + 1020..at + 1024].copy_from_slice(&sum.to_le_bytes());
        // The table's inode inside the superblock.
        let sum = pbr_crc::crc32(&data[at + 512..at + 512 + 252]);
        data[at + 512 + 252..at + 512 + 256].copy_from_slice(&sum.to_le_bytes());
        let sum = pbr_crc::crc32(&data[at..at + 1020]);
        data[at + 1020..at + 1024].copy_from_slice(&sum.to_le_bytes());
    }
    for &b in table_blocks {
        let at = b as usize * 4096;
        for slot in 0..16 {
            let s = at + slot * 256;
            if data[s..s + 256].iter().any(|&x| x != 0) {
                let sum = pbr_crc::crc32(&data[s..s + 252]);
                data[s + 252..s + 256].copy_from_slice(&sum.to_le_bytes());
            }
        }
    }
    for block in data.chunks_exact_mut(4096) {
        if &block[..4] == b"RXTB" {
            let sum = xblock_checksum(block);
            block[12..16].copy_from_slice(&sum.to_le_bytes());
        }
    }
}

/// Uses a volume every way there is. Nothing here may panic; errors are fine.
fn exercise(disk: MemDisk, read_only: bool) {
    let Ok(mut fs) = RockFs::mount(disk, clock, read_only) else { return };
    let _ = fs.info();
    let mut stack = std::vec![(fs.root(), 0u32)];
    let mut visited = 0;
    let mut files = Vec::new();
    while let Some((folder, depth)) = stack.pop() {
        visited += 1;
        if visited > 2000 || depth > 40 {
            break;
        }
        let mut cursor = 0;
        for _ in 0..200 {
            let mut entries = Vec::new();
            let next = fs.list(folder, cursor, &mut |e| {
                entries.push(e);
                true
            });
            let Ok(next) = next else { break };
            for e in entries {
                if let Ok(id) = fs.lookup(folder, &e.name) {
                    let _ = fs.stat(id);
                    if e.kind == Kind::Folder {
                        stack.push((id, depth + 1));
                    } else {
                        files.push(id);
                    }
                }
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
    }
    for &f in files.iter().take(300) {
        let mut buf = std::vec![0u8; 70_000];
        let _ = fs.read(f, 0, &mut buf[..65536]);
        let _ = fs.read(f, 1 << 33, &mut buf[..16]);
        let _ = fs.read(f, u64::MAX - 1, &mut buf[..16]);
    }
    let _ = fs.space();
    let first = fs.check(false);
    if !read_only {
        let root = fs.root();
        if first.is_ok() {
            // Repair, then what is left must be only what was said not to be fixable.
            if let Ok(fixed) = fs.check(true)
                && fixed.iter().all(|p| p.fixed || p.harmless)
                && let Ok(after) = fs.check(false)
            {
                assert!(after.is_empty(), "left after repair: {after:?} (was {fixed:?})");
            }
        }
        if let Ok(n) = fs.create(root, "new-file", Kind::File) {
            let _ = fs.write(n, 0, &bytes(1, 9000));
            let _ = fs.commit(n);
        }
        let _ = fs.create(root, "new-folder", Kind::Folder);
        let _ = fs.rename(root, "docs", root, "docs2", false);
        for f in files.iter().take(40) {
            let _ = fs.write(*f, 100, b"patch");
            let _ = fs.set_size(*f, 10);
            let _ = fs.commit(*f);
        }
        let _ = fs.remove(root, "many");
        let _ = fs.remove(root, "frag-a");
        let _ = fs.remove(root, "new-file");
        let _ = fs.unmount();
    }
}

/// The blocks holding the inode table.
fn table_blocks(disk: &MemDisk) -> Vec<u64> {
    let sb = Superblock::parse(&disk.data[..1024]).unwrap();
    match &sb.table.top {
        Top::Extents(list) => {
            list.iter().flat_map(|e| e.disk_block..e.disk_block + u64::from(e.length)).collect()
        }
        Top::Index(_) => Vec::new(),
    }
}

/// The blocks that hold structure rather than file contents: the superblocks, the bitmap, the
/// inode table, extent blocks, and the blocks of folders. Damage is aimed at these.
fn used_blocks(disk: &MemDisk) -> Vec<u64> {
    let blocks = disk.data.len() as u64 / 4096;
    let mut out: Vec<u64> = std::vec![0, 1, blocks - 1];
    let tables = table_blocks(disk);
    out.extend(&tables);
    for b in 0..blocks {
        if &disk.data[b as usize * 4096..][..4] == b"RXTB" {
            out.push(b);
        }
    }
    for &t in &tables {
        for slot in 0..16 {
            let raw = &disk.data[t as usize * 4096 + slot * 256..][..256];
            if let Ok(inode) = crate::inode::Inode::parse(raw, true)
                && let (2, Top::Extents(list)) = (inode.kind, &inode.top)
            {
                out.extend(
                    list.iter().flat_map(|e| e.disk_block..e.disk_block + u64::from(e.length)),
                );
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[test]
fn the_base_volume_is_sound() {
    let mut fs = RockFs::mount(base(), clock, false).unwrap();
    assert_clean(&mut fs);
    let a = node_at(&mut fs, "frag-a").unwrap();
    assert!(fs.read_inode((a & 0xFFFF_FFFF) as u32, true).unwrap().depth >= 1);
}

#[test]
fn single_byte_damage_anywhere_is_survived() {
    let disk = base();
    let used = used_blocks(&disk);
    let tables = table_blocks(&disk);
    let mut rng = Rng(99);
    for round in 0..200 {
        let mut d = MemDisk::from(disk.data.clone());
        for _ in 0..1 + rng.below(3) {
            let block =
                if rng.below(4) == 0 { 0 } else { used[rng.below(used.len() as u64) as usize] };
            // Bias towards the start of the block, where headers are.
            let off = if rng.below(2) == 0 { rng.below(300) } else { rng.below(4096) };
            let at = block as usize * 4096 + off as usize;
            d.data[at] =
                if rng.below(2) == 0 { d.data[at] ^ (1 << rng.below(8)) } else { rng.next() as u8 };
        }
        // Half the time make the checksums right again so the damage is seen deeper in.
        if round % 2 == 0 {
            fix_checksums(&mut d.data, &tables);
        }
        exercise(MemDisk::from(d.data.clone()), false);
        exercise(d, true);
    }
}

#[test]
fn whole_blocks_of_junk_are_survived() {
    let disk = base();
    let used = used_blocks(&disk);
    let tables = table_blocks(&disk);
    let mut rng = Rng(1234);
    for round in 0..120 {
        let mut d = MemDisk::from(disk.data.clone());
        let block = used[rng.below(used.len() as u64) as usize] as usize;
        let range = block * 4096..block * 4096 + 4096;
        match rng.below(4) {
            0 => d.data[range].fill(0),
            1 => d.data[range].fill(0xFF),
            2 => d.data[range].iter_mut().for_each(|b| *b = rng.next() as u8),
            _ => {
                // Another block's contents copied here (two things now claim the same blocks).
                let other = used[rng.below(used.len() as u64) as usize] as usize * 4096;
                let copy = d.data[other..other + 4096].to_vec();
                d.data[range].copy_from_slice(&copy);
            }
        }
        if round % 2 == 0 {
            fix_checksums(&mut d.data, &tables);
        }
        exercise(MemDisk::from(d.data.clone()), false);
        exercise(d, true);
    }
}

#[test]
fn nonsense_numbers_in_inodes_and_folders_are_survived() {
    let disk = base();
    let tables = table_blocks(&disk);
    let mut rng = Rng(4321);
    let interesting: [u64; 8] = [0, 1, 0xFFFF_FFFF, u64::MAX, 1 << 63, 1 << 40, 4096, 299];
    for round in 0..150 {
        let mut d = MemDisk::from(disk.data.clone());
        // Overwrite an eight-byte field of a random inode (or the folder block of the first
        // few entries) with an extreme value.
        let t = tables[rng.below(tables.len() as u64) as usize] as usize;
        let slot = rng.below(16) as usize;
        let field = (rng.below(31) * 8) as usize;
        let value = interesting[rng.below(8) as usize];
        let at = t * 4096 + slot * 256 + field;
        d.data[at..at + 8].copy_from_slice(&value.to_le_bytes());
        fix_checksums(&mut d.data, &tables);
        exercise(MemDisk::from(d.data.clone()), false);
        exercise(d, round % 2 == 0);
    }
}

#[test]
fn damaged_extent_blocks_are_refused_or_repaired() {
    let disk = base();
    let tables = table_blocks(&disk);
    let tree_blocks: Vec<usize> = disk
        .data
        .chunks_exact(4096)
        .enumerate()
        .filter(|(_, b)| &b[..4] == b"RXTB")
        .map(|(i, _)| i)
        .collect();
    assert!(!tree_blocks.is_empty());
    let mut rng = Rng(8);
    for _ in 0..100 {
        let mut d = MemDisk::from(disk.data.clone());
        let b = tree_blocks[rng.below(tree_blocks.len() as u64) as usize];
        let off = 16 + rng.below(170) as usize * 24 + rng.below(24) as usize;
        let at = b * 4096 + off;
        d.data[at] = rng.next() as u8;
        if rng.below(3) == 0 {
            d.data[b * 4096 + 8 + rng.below(4) as usize] = rng.next() as u8;
        }
        fix_checksums(&mut d.data, &tables);
        exercise(MemDisk::from(d.data.clone()), false);
        exercise(d, true);
    }
}

#[test]
fn truncated_and_oversized_devices_are_survived() {
    let disk = base();
    for sectors in [0usize, 1, 7, 8, 9, 16, 64, 2000] {
        let mut data = disk.data.clone();
        data.truncate(sectors * 512);
        exercise(MemDisk::from(data), false);
    }
    // A volume on a bigger device than it says: the backup is not in the last block.
    let mut data = disk.data.clone();
    data.extend(std::vec![0u8; 4096 * 10]);
    exercise(MemDisk::from(data), false);
}
