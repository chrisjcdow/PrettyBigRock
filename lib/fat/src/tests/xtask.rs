// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading what the build tool's own FAT32 writer makes: the EFI system partition and the FAT32
//! test disks (`xtask/src/image.rs`). It is a second, independent writer, with habits of its own:
//! one sector to a cluster, no long names, `..` of a top-level folder 0, a volume label in the
//! boot sector and in the root folder, a folder with no free entry when it is exactly full.
//!
//! `xtask` is a program, not a library, so its writer cannot be called from here. Instead the
//! part of `xtask/src/image.rs` that writes FAT32 is **copied below, unchanged** (as of commit
//! 2b78ec5, "Build tool: put the FAT32 volume label in the root folder too"), apart from the
//! two `pub`s removed and the small helpers it needs. If that file changes, copy it again.

use super::tools::{Image, need_tools};
use super::*;
use pbr_fs::SECTOR;
use std::string::ToString;

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---- begin copy of xtask/src/image.rs ---------------------------------------------------

const CLUSTER: usize = SECTOR; // one sector per cluster: the smallest valid FAT32 volume is 32 MiB
const RESERVED: u32 = 32;
const FAT_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1; // a fixed date, for repeatable builds

enum Node<'a> {
    File(&'a [u8]),
    Dir(std::vec::Vec<(std::string::String, Node<'a>)>),
}

fn insert<'a>(
    dir: &mut std::vec::Vec<(std::string::String, Node<'a>)>,
    path: &[&str],
    data: &'a [u8],
) {
    let name = path[0].to_ascii_uppercase();
    if path.len() == 1 {
        dir.push((name, Node::File(data)));
        return;
    }
    if !dir.iter().any(|(n, _)| *n == name) {
        dir.push((name.clone(), Node::Dir(std::vec![])));
    }
    if let Some((_, Node::Dir(sub))) = dir.iter_mut().find(|(n, _)| *n == name) {
        insert(sub, &path[1..], data);
    }
}

fn short_name(name: &str) -> Result<[u8; 11], std::string::String> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(std::format!("`{name}` doesn't fit an 8.3 name"));
    }
    let mut n = [b' '; 11];
    n[..base.len()].copy_from_slice(base.as_bytes());
    n[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(n)
}

struct Fat {
    table: std::vec::Vec<u32>,
    data: std::vec::Vec<u8>,
}

impl Fat {
    /// Allocates a chain of clusters for `bytes` (at least one) and writes them. Returns the first.
    fn store(&mut self, bytes: &[u8]) -> Result<u32, std::string::String> {
        let n = bytes.len().div_ceil(CLUSTER).max(1);
        let first = self.table.len() as u32;
        let capacity = (self.data.len() / CLUSTER) as u32 + 2;
        if first + n as u32 > capacity {
            return Err("the EFI system partition is full".into());
        }
        for i in 0..n as u32 {
            self.table.push(if i + 1 == n as u32 { 0x0FFF_FFFF } else { first + i + 1 });
        }
        let at = (first as usize - 2) * CLUSTER;
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(first)
    }
}

fn dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    put16(&mut e, 14, 0);
    put16(&mut e, 16, FAT_DATE);
    put16(&mut e, 18, FAT_DATE);
    put16(&mut e, 20, (cluster >> 16) as u16);
    put16(&mut e, 22, 0);
    put16(&mut e, 24, FAT_DATE);
    put16(&mut e, 26, cluster as u16);
    put32(&mut e, 28, size);
    e
}

/// Writes a directory and everything in it; returns its first cluster. `parent` is the parent's
/// first cluster, or `None` for the root (which has no "." and ".." entries). The root's first
/// entry is the volume label, which FAT keeps there as well as in the boot sector (Linux's
/// `fsck.fat` complains if the two don't match).
fn write_dir(
    fat: &mut Fat,
    entries: &[(std::string::String, Node<'_>)],
    parent: Option<u32>,
    label: Option<&[u8; 11]>,
) -> Result<u32, std::string::String> {
    // Allocate this directory's clusters first, so its children can point back at it, then
    // fill them in once the children's clusters are known. `dots` counts the entries before
    // the children: "." and "..", or the label.
    let dots = if parent.is_some() { 2 } else { usize::from(label.is_some()) };
    let mut bytes = std::vec![0u8; (entries.len() + dots) * 32];
    let own = fat.store(&bytes)?;
    if let Some(p) = parent {
        bytes[..32].copy_from_slice(&dir_entry(b".          ", 0x10, own, 0));
        let up = if p == 2 { 0 } else { p }; // ".." of a top-level folder is 0, by convention
        bytes[32..64].copy_from_slice(&dir_entry(b"..         ", 0x10, up, 0));
    } else if let Some(l) = label {
        bytes[..32].copy_from_slice(&dir_entry(l, 0x08, 0, 0)); // 0x08: the volume label
    }
    for (i, (name, node)) in entries.iter().enumerate() {
        let sn = short_name(name)?;
        let e = match node {
            Node::File(data) => {
                let c = if data.is_empty() { 0 } else { fat.store(data)? };
                dir_entry(&sn, 0x20, c, data.len() as u32)
            }
            Node::Dir(sub) => dir_entry(&sn, 0x10, write_dir(fat, sub, Some(own), None)?, 0),
        };
        bytes[(i + dots) * 32..(i + dots + 1) * 32].copy_from_slice(&e);
    }
    // Write the entries over the clusters allocated above (which are consecutive).
    let at = (own as usize - 2) * CLUSTER;
    fat.data[at..at + bytes.len()].copy_from_slice(&bytes);
    Ok(own)
}

/// Builds a FAT32 volume `sectors` long holding `files`. `hidden` is the volume's first sector
/// on the disk.
#[allow(dead_code)]
fn fat32(
    files: &[(&str, &[u8])],
    hidden: u64,
    sectors: u64,
) -> Result<std::vec::Vec<u8>, std::string::String> {
    fat32_labelled(files, hidden, sectors, b"PBR BOOT   ")
}

/// [`fat32`], with the volume label `label` (11 bytes, padded with spaces).
fn fat32_labelled(
    files: &[(&str, &[u8])],
    hidden: u64,
    sectors: u64,
    label: &[u8; 11],
) -> Result<std::vec::Vec<u8>, std::string::String> {
    let total = sectors as u32;
    // FAT size: each cluster needs 4 bytes; solve for the size that covers what's left.
    let mut fat_sectors = 1u32;
    loop {
        let clusters = total - RESERVED - 2 * fat_sectors;
        let need = ((clusters + 2) * 4).div_ceil(SECTOR as u32);
        if need <= fat_sectors {
            break;
        }
        fat_sectors = need;
    }
    let data_sectors = total - RESERVED - 2 * fat_sectors;
    let mut fat = Fat {
        table: std::vec![0x0FFF_FFF8, 0x0FFF_FFFF],
        data: std::vec![0u8; data_sectors as usize * SECTOR],
    };

    let mut root = std::vec::Vec::new();
    for (path, data) in files {
        let parts: std::vec::Vec<&str> = path.split('/').collect();
        insert(&mut root, &parts, data);
    }
    let root_cluster = write_dir(&mut fat, &root, None, Some(label))?;
    if root_cluster != 2 {
        return Err("internal error: the root directory isn't at cluster 2".into());
    }

    let mut v = std::vec![0u8; total as usize * SECTOR];
    let bs = &mut v[..SECTOR];
    bs[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"PBRBOOT ");
    put16(bs, 11, SECTOR as u16);
    bs[13] = (CLUSTER / SECTOR) as u8;
    put16(bs, 14, RESERVED as u16);
    bs[16] = 2;
    bs[21] = 0xF8;
    put16(bs, 24, 63);
    put16(bs, 26, 255);
    put32(bs, 28, hidden as u32);
    put32(bs, 32, total);
    put32(bs, 36, fat_sectors);
    put32(bs, 44, 2); // root cluster
    put16(bs, 48, 1); // FSInfo sector
    put16(bs, 50, 6); // backup boot sector
    bs[64] = 0x80;
    bs[66] = 0x29;
    put32(bs, 67, 0x5042_5221);
    bs[71..82].copy_from_slice(label);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xAA;
    let free = data_sectors + 2 - fat.table.len() as u32;
    {
        let fi = &mut v[SECTOR..2 * SECTOR];
        put32(fi, 0, 0x4161_5252);
        put32(fi, 484, 0x6141_7272);
        put32(fi, 488, free);
        put32(fi, 492, fat.table.len() as u32);
        put32(fi, 508, 0xAA55_0000);
    }
    let (boot, info) = (v[..SECTOR].to_vec(), v[SECTOR..2 * SECTOR].to_vec());
    v[6 * SECTOR..7 * SECTOR].copy_from_slice(&boot);
    v[7 * SECTOR..8 * SECTOR].copy_from_slice(&info);

    for copy in 0..2 {
        let at = (RESERVED + copy * fat_sectors) as usize * SECTOR;
        for (i, e) in fat.table.iter().enumerate() {
            put32(&mut v, at + i * 4, *e);
        }
    }
    let at = (RESERVED + 2 * fat_sectors) as usize * SECTOR;
    v[at..at + fat.data.len()].copy_from_slice(&fat.data);
    Ok(v)
}

// ---- end of the copy --------------------------------------------------------------------

/// 64 MiB, the size of the EFI system partition.
const ESP_SECTORS: u64 = 64 * 2048;

fn files() -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::vec![
        ("EFI/BOOT/BOOTX64.EFI".to_string(), b"x86 loader".to_vec()),
        ("EFI/BOOT/BOOTAA64.EFI".to_string(), b"arm loader".to_vec()),
        ("PBR/x86_64/kernel.elf".to_string(), bytes(1, 300_000)),
        ("PBR/aarch64/kernel.elf".to_string(), bytes(2, 123_457)),
        ("PBR/cmdline.txt".to_string(), Vec::new()),
        ("PBR/ONE.BIN".to_string(), bytes(3, 512)),
        ("PBR/CLUSTER1.BIN".to_string(), bytes(4, 513)),
    ];
    // A folder with exactly 14 files, which with `.` and `..` fill its one cluster: it has no
    // end marker, and the next file needs a second cluster.
    for i in 0..14 {
        files.push((std::format!("FULL/F{i}.TXT"), std::vec![i as u8; 20 + i]));
    }
    files
}

fn tree_of_files(files: &[(String, Vec<u8>)]) -> Tree {
    let mut tree = Tree::new();
    for (path, data) in files {
        let upper = path.to_ascii_uppercase();
        let mut at = 0;
        while let Some(i) = upper[at..].find('/') {
            tree.insert(std::format!("/{}", &upper[..at + i]), None);
            at += i + 1;
        }
        tree.insert(std::format!("/{upper}"), Some(data.clone()));
    }
    tree
}

fn build(label: &[u8; 11]) -> (MemDisk, Tree) {
    let files = files();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    let image = fat32_labelled(&refs, 18_432, ESP_SECTORS, label).unwrap();
    (MemDisk::from(image), tree_of_files(&files))
}

#[test]
fn reads_what_the_build_tool_writes() {
    let (disk, want) = build(b"PBR BOOT   ");
    let mut fs = mount(disk);
    let info = fs.info();
    assert_eq!((info.label.as_str(), info.cluster_bytes, info.fat_copies), ("PBR BOOT", 512, 2));
    assert_eq!(info.volume_id, 0x5042_5221);
    assert!(info.clean_at_mount && info.used_fsinfo);
    assert_same_tree(&tree_of(&mut fs), &want, "the build tool's volume");
    // Names are all capitals, with no long names.
    let root = fs.root();
    assert_eq!(names(&mut fs, root), ["EFI", "PBR", "FULL"]);
    // The count in its FSInfo sector is right: a recount gives the same.
    let counted = fs.vol.table.count_free(&mut fs.vol.dev).unwrap();
    assert_eq!(counted, info.free_clusters);
    let space = fs.space().unwrap();
    assert_eq!(space.label, "PBR BOOT");
}

#[test]
fn a_different_label() {
    let (disk, _) = build(b"MY LABEL   ");
    assert_eq!(mount(disk).space().unwrap().label, "MY LABEL");
}

#[test]
fn writing_into_the_build_tools_volume() {
    let (disk, _) = build(b"PBR BOOT   ");
    let mut fs = mount(disk);
    // Into the full folder, which must grow; into a folder from the tool; a new long name.
    put(&mut fs, "FULL/F14.TXT", b"fifteen").unwrap();
    put(&mut fs, "FULL/a longer name than 8.3.txt", b"long").unwrap();
    put(&mut fs, "EFI/BOOT/extra.cfg", &bytes(9, 4000)).unwrap();
    put(&mut fs, "PBR/cmdline.txt", b"console=serial").unwrap();
    let kernel = node_at(&mut fs, "PBR/x86_64/kernel.elf").unwrap();
    fs.write(kernel, 1000, &bytes(10, 5000)).unwrap();
    let arm = node_at(&mut fs, "PBR/aarch64/kernel.elf").unwrap();
    fs.set_size(arm, 100).unwrap();
    fs.commit(arm).unwrap();
    fs.commit(kernel).unwrap();
    let efi = node_at(&mut fs, "EFI").unwrap();
    let root = fs.root();
    fs.rename(efi, "BOOT", root, "BOOT2", false).unwrap();
    let mut fs = remount(fs);
    let tree = tree_of(&mut fs);
    assert_eq!(tree["/FULL/F14.TXT"], Some(b"fifteen".to_vec()));
    assert_eq!(tree["/FULL/a longer name than 8.3.txt"], Some(b"long".to_vec()));
    assert_eq!(tree["/BOOT2/extra.cfg"], Some(bytes(9, 4000)));
    assert_eq!(tree["/PBR/CMDLINE.TXT"], Some(b"console=serial".to_vec()));
    assert_eq!(tree["/PBR/AARCH64/KERNEL.ELF"].as_ref().unwrap(), &bytes(2, 123_457)[..100]);
    let mut k = bytes(1, 300_000);
    k[1000..6000].copy_from_slice(&bytes(10, 5000));
    assert_eq!(tree["/PBR/X86_64/KERNEL.ELF"], Some(k));
    assert_eq!(tree["/BOOT2/BOOTX64.EFI"], Some(b"x86 loader".to_vec()));
}

#[test]
fn the_tools_agree_about_the_build_tools_volume() {
    need_tools!("fsck.fat", "mdir", "mcopy");
    // `fsck.fat` finds nothing wrong with the build tool's volume (this also tests the build
    // tool) and `mtools` reads it the way we do.
    let (disk, want) = build(b"PBR BOOT   ");
    let img = Image::mkfs(40_000, &[]); // only for its folder and tool plumbing
    img.store(&disk);
    img.assert_clean("the build tool's volume");
    img.assert_holds(&want, "the build tool's volume");
    // And after we have changed it.
    let mut fs = mount(disk);
    put(&mut fs, "FULL/F14.TXT", b"fifteen").unwrap();
    put(&mut fs, "NEW/Deeper Folder/file.txt", b"x").ok();
    mkdir(&mut fs, "NEW").unwrap();
    mkdir(&mut fs, "NEW/Deeper Folder").unwrap();
    put(&mut fs, "NEW/Deeper Folder/file.txt", b"x").unwrap();
    fs.unmount().unwrap();
    img.store(&fs.into_device());
    img.assert_clean("the build tool's volume, changed here");
}
