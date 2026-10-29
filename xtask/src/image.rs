// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Writes the boot disk image (docs/design/0004-loaders-and-boot-disk.md): a GPT disk with a
//! BIOS boot partition and a FAT32 EFI system partition. Everything here is our own code, so
//! building the image needs nothing but Rust.

const SECTOR: usize = 512;
const BIOS_START: u64 = 2048;
const BIOS_SECTORS: u64 = 8 * 2048; // 8 MiB
const ESP_START: u64 = BIOS_START + BIOS_SECTORS;
const ESP_SECTORS: u64 = 64 * 2048; // 64 MiB

/// The image's contents.
pub struct Contents<'a> {
    /// The MBR's first 440 bytes: the BIOS first stage, with the BIOS loader's location at 432.
    pub mbr_code: &'a [u8],
    /// What goes at the start of the BIOS boot partition: the BIOS loader, then the payload.
    pub bios_partition: &'a [u8],
    /// Files for the EFI system partition, as (path with `/`, contents).
    pub esp_files: &'a [(&'a str, &'a [u8])],
}

/// Where the BIOS boot partition starts, in sectors (the first stage needs it).
pub const BIOS_PARTITION_LBA: u64 = BIOS_START;

/// Builds the whole disk image.
pub fn build(c: &Contents<'_>) -> Result<Vec<u8>, String> {
    let total = ESP_START + ESP_SECTORS + 2048; // room for the backup GPT, rounded to a MiB
    let mut disk = vec![0u8; total as usize * SECTOR];

    if c.mbr_code.len() > 440 {
        return Err("the MBR boot code is bigger than 440 bytes".into());
    }
    disk[..c.mbr_code.len()].copy_from_slice(c.mbr_code);
    protective_mbr(&mut disk[..SECTOR], total);

    if c.bios_partition.len() as u64 > BIOS_SECTORS * SECTOR as u64 {
        return Err("the BIOS loader and payload don't fit in the BIOS boot partition".into());
    }
    let b = BIOS_START as usize * SECTOR;
    disk[b..b + c.bios_partition.len()].copy_from_slice(c.bios_partition);

    let fat = fat32(c.esp_files, ESP_START, ESP_SECTORS)?;
    let e = ESP_START as usize * SECTOR;
    disk[e..e + fat.len()].copy_from_slice(&fat);

    gpt(&mut disk, total);
    Ok(disk)
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn protective_mbr(s: &mut [u8], total: u64) {
    let p = 446;
    s[p] = 0x00; // not "active" (see the open question in note 0004)
    s[p + 1..p + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    s[p + 4] = 0xEE;
    s[p + 5..p + 8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    put32(s, p + 8, 1);
    put32(s, p + 12, (total - 1).min(u64::from(u32::MAX)) as u32);
    s[510] = 0x55;
    s[511] = 0xAA;
}

/// CRC-32 (IEEE), as GPT uses.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// A GUID written the way GPT stores it (the first three groups little-endian).
fn guid(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s
        .split('-')
        .flat_map(|g| {
            (0..g.len()).step_by(2).map(move |i| u8::from_str_radix(&g[i..i + 2], 16).unwrap())
        })
        .collect();
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(&[hex[3], hex[2], hex[1], hex[0]]);
    g[4..6].copy_from_slice(&[hex[5], hex[4]]);
    g[6..8].copy_from_slice(&[hex[7], hex[6]]);
    g[8..].copy_from_slice(&hex[8..16]);
    g
}

fn gpt(disk: &mut [u8], total: u64) {
    let last = total - 1;
    let mut entries = vec![0u8; 128 * 128];
    let parts = [
        (
            "21686148-6449-6E6F-744E-656564454649",
            "7B2E5C01-0000-4000-8000-505242494F53",
            BIOS_START,
            BIOS_SECTORS,
            "BIOS boot",
        ),
        (
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
            "7B2E5C02-0000-4000-8000-505245535030",
            ESP_START,
            ESP_SECTORS,
            "EFI system",
        ),
    ];
    for (i, (ty, id, start, len, name)) in parts.iter().enumerate() {
        let e = &mut entries[i * 128..(i + 1) * 128];
        e[..16].copy_from_slice(&guid(ty));
        e[16..32].copy_from_slice(&guid(id)); // fixed, so builds are repeatable
        put64(e, 32, *start);
        put64(e, 40, start + len - 1);
        for (j, u) in name.encode_utf16().enumerate() {
            put16(e, 56 + 2 * j, u);
        }
    }
    let ecrc = crc32(&entries);
    let header = |my: u64, alt: u64, entries_lba: u64| {
        let mut h = vec![0u8; SECTOR];
        h[..8].copy_from_slice(b"EFI PART");
        put32(&mut h, 8, 0x0001_0000);
        put32(&mut h, 12, 92);
        put64(&mut h, 24, my);
        put64(&mut h, 32, alt);
        put64(&mut h, 40, 34);
        put64(&mut h, 48, last - 33);
        h[56..72].copy_from_slice(&guid("7B2E5C00-0000-4000-8000-505242444953"));
        put64(&mut h, 72, entries_lba);
        put32(&mut h, 80, 128);
        put32(&mut h, 84, 128);
        put32(&mut h, 88, ecrc);
        let c = crc32(&h[..92]);
        put32(&mut h, 16, c);
        h
    };
    let s = SECTOR;
    disk[s..2 * s].copy_from_slice(&header(1, last, 2));
    disk[2 * s..2 * s + entries.len()].copy_from_slice(&entries);
    let be = (last - 32) as usize * s;
    disk[be..be + entries.len()].copy_from_slice(&entries);
    disk[last as usize * s..].copy_from_slice(&header(last, 1, last - 32));
}

// ------------------------------------------------------------------------------------------
// FAT32

const CLUSTER: usize = SECTOR; // one sector per cluster: the smallest valid FAT32 volume is 32 MiB
const RESERVED: u32 = 32;
const FAT_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1; // a fixed date, for repeatable builds

enum Node<'a> {
    File(&'a [u8]),
    Dir(Vec<(String, Node<'a>)>),
}

fn insert<'a>(dir: &mut Vec<(String, Node<'a>)>, path: &[&str], data: &'a [u8]) {
    let name = path[0].to_ascii_uppercase();
    if path.len() == 1 {
        dir.push((name, Node::File(data)));
        return;
    }
    if !dir.iter().any(|(n, _)| *n == name) {
        dir.push((name.clone(), Node::Dir(vec![])));
    }
    if let Some((_, Node::Dir(sub))) = dir.iter_mut().find(|(n, _)| *n == name) {
        insert(sub, &path[1..], data);
    }
}

fn short_name(name: &str) -> Result<[u8; 11], String> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(format!("`{name}` doesn't fit an 8.3 name"));
    }
    let mut n = [b' '; 11];
    n[..base.len()].copy_from_slice(base.as_bytes());
    n[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(n)
}

struct Fat {
    table: Vec<u32>,
    data: Vec<u8>,
}

impl Fat {
    /// Allocates a chain of clusters for `bytes` (at least one) and writes them. Returns the first.
    fn store(&mut self, bytes: &[u8]) -> Result<u32, String> {
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
/// first cluster, or `None` for the root (which has no "." and ".." entries).
fn write_dir(
    fat: &mut Fat,
    entries: &[(String, Node<'_>)],
    parent: Option<u32>,
) -> Result<u32, String> {
    // Allocate this directory's clusters first, so its children can point back at it, then
    // fill them in once the children's clusters are known.
    let dots = if parent.is_some() { 2 } else { 0 };
    let mut bytes = vec![0u8; (entries.len() + dots) * 32];
    let own = fat.store(&bytes)?;
    if let Some(p) = parent {
        bytes[..32].copy_from_slice(&dir_entry(b".          ", 0x10, own, 0));
        let up = if p == 2 { 0 } else { p }; // ".." of a top-level folder is 0, by convention
        bytes[32..64].copy_from_slice(&dir_entry(b"..         ", 0x10, up, 0));
    }
    for (i, (name, node)) in entries.iter().enumerate() {
        let sn = short_name(name)?;
        let e = match node {
            Node::File(data) => {
                let c = if data.is_empty() { 0 } else { fat.store(data)? };
                dir_entry(&sn, 0x20, c, data.len() as u32)
            }
            Node::Dir(sub) => dir_entry(&sn, 0x10, write_dir(fat, sub, Some(own))?, 0),
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
pub fn fat32(files: &[(&str, &[u8])], hidden: u64, sectors: u64) -> Result<Vec<u8>, String> {
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
        table: vec![0x0FFF_FFF8, 0x0FFF_FFFF],
        data: vec![0u8; data_sectors as usize * SECTOR],
    };

    let mut root = Vec::new();
    for (path, data) in files {
        let parts: Vec<&str> = path.split('/').collect();
        insert(&mut root, &parts, data);
    }
    let root_cluster = write_dir(&mut fat, &root, None)?;
    if root_cluster != 2 {
        return Err("internal error: the root directory isn't at cluster 2".into());
    }

    let mut v = vec![0u8; total as usize * SECTOR];
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
    bs[71..82].copy_from_slice(b"PBR BOOT   ");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn guid_layout() {
        let g = guid("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(&g[..8], &[0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11]);
        assert_eq!(&g[8..], &[0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
    }

    #[test]
    fn short_names() {
        assert_eq!(&short_name("BOOTX64.EFI").unwrap(), b"BOOTX64 EFI");
        assert_eq!(&short_name("AARCH64").unwrap(), b"AARCH64    ");
        assert!(short_name("TOOLONGNAME.TXT").is_err());
    }

    /// Reads a file back out of a FAT32 volume, following the directory tree and FAT chains,
    /// as an independent check on the writer.
    fn read_back(v: &[u8], path: &str) -> Option<Vec<u8>> {
        let u16_at = |o: usize| u16::from_le_bytes([v[o], v[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(v[o..o + 4].try_into().unwrap()) as usize;
        let reserved = u16_at(14);
        let fat_sz = u32_at(36);
        let data = (reserved + 2 * fat_sz) * SECTOR;
        let chain = |mut c: usize| {
            let mut out = Vec::new();
            while (2..0x0FFF_FFF8).contains(&c) {
                out.extend_from_slice(&v[data + (c - 2) * CLUSTER..data + (c - 1) * CLUSTER]);
                c = u32_at(reserved * SECTOR + c * 4) & 0x0FFF_FFFF;
            }
            out
        };
        let mut dir = chain(u32_at(44));
        let parts: Vec<&str> = path.split('/').collect();
        for (i, p) in parts.iter().enumerate() {
            let want = short_name(&p.to_ascii_uppercase()).ok()?;
            let e = dir.chunks(32).find(|e| e[0] != 0 && e[..11] == want)?;
            let c = (u16::from_le_bytes([e[20], e[21]]) as usize) << 16
                | u16::from_le_bytes([e[26], e[27]]) as usize;
            let size = u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize;
            let content = chain(c);
            if i + 1 == parts.len() {
                return Some(content[..size].to_vec());
            }
            dir = content;
        }
        None
    }

    #[test]
    fn fat32_round_trip() {
        let big: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        let files: [(&str, &[u8]); 4] = [
            ("EFI/BOOT/BOOTX64.EFI", b"x86 loader"),
            ("EFI/BOOT/BOOTAA64.EFI", b"arm loader"),
            ("PBR/x86_64/kernel.elf", &big),
            ("PBR/cmdline.txt", b""),
        ];
        let v = fat32(&files, 18432, 64 * 2048).unwrap();
        assert_eq!(read_back(&v, "EFI/BOOT/BOOTX64.EFI").unwrap(), b"x86 loader");
        assert_eq!(read_back(&v, "EFI/BOOT/BOOTAA64.EFI").unwrap(), b"arm loader");
        assert_eq!(read_back(&v, "pbr/x86_64/kernel.elf").unwrap(), big);
        assert_eq!(read_back(&v, "PBR/cmdline.txt").unwrap(), b"");
        assert!(read_back(&v, "PBR/nothing.txt").is_none());
        // The cluster count makes it FAT32, not FAT16.
        let clusters =
            (64 * 2048 - 32 - 2 * u32::from_le_bytes(v[36..40].try_into().unwrap())) as usize;
        assert!(clusters >= 65525);
    }

    #[test]
    fn gpt_headers_check_out() {
        let disk =
            build(&Contents { mbr_code: &[0x90; 10], bios_partition: b"bios", esp_files: &[] })
                .unwrap();
        assert_eq!(&disk[512..520], b"EFI PART");
        let mut h = disk[512..512 + 92].to_vec();
        let stored = u32::from_le_bytes(h[16..20].try_into().unwrap());
        h[16..20].fill(0);
        assert_eq!(crc32(&h), stored);
        let last = disk.len() / 512 - 1;
        assert_eq!(&disk[last * 512..last * 512 + 8], b"EFI PART");
        assert_eq!(disk[450], 0xEE);
        assert_eq!(&disk[510..512], &[0x55, 0xAA]);
        assert_eq!(disk[0], 0x90);
        assert_eq!(&disk[2048 * 512..2048 * 512 + 4], b"bios");
    }
}
