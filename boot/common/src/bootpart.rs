// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Which partition the loader started from (docs/design/0027-kernel-1.4.md, section 4).
//!
//! A disk with a **GPT** (GUID partition table) gives each partition a unique 16-byte number, a
//! GUID. The loader passes the GUID of the partition it read the kernel from to the kernel, so
//! the storage service can choose the system volume on that disk rather than on another one
//! that happens to be plugged in (note 0030). Each loader finds it its own way:
//! - **UEFI** says which partition it loaded us from in a *device path*: a list of nodes
//!   describing the way from the machine to the file, one of which is the partition;
//! - **BIOS** knows only the first sector of its own partition, so it reads the GPT itself and
//!   finds the partition that starts there.
//!
//! Everything read from the disk is checked before it's believed: a damaged table gives "not
//! known" (all zero), never a wrong answer or a crash.

/// The UEFI device path node for a partition of a hard drive: type 4 ("media"), subtype 1, 42
/// bytes long, holding the partition's number, start, size and signature.
const MEDIA: u8 = 4;
const HARD_DRIVE: u8 = 1;
const HARD_DRIVE_LEN: usize = 42;
/// The node that ends a device path.
const END: u8 = 0x7F;

/// The unique GUID of the GPT partition named in a UEFI device path, or `None` if the path has
/// no hard-drive node for a GPT partition. `path` is the device path's bytes; it is read only
/// as far as its end node, and never past the slice.
pub fn from_device_path(path: &[u8]) -> Option<[u8; 16]> {
    let mut at = 0usize;
    let mut found = None;
    // Each node starts with its type, subtype and its own length (16 bits, including these 4).
    while let Some(head) = path.get(at..at + 4) {
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        if head[0] == END || len < 4 {
            break;
        }
        let node = path.get(at..at + len)?;
        // The signature type (byte 41) is 2 for a GUID, which only GPT partitions have.
        if node[0] == MEDIA && node[1] == HARD_DRIVE && len >= HARD_DRIVE_LEN && node[41] == 2 {
            let mut g = [0u8; 16];
            g.copy_from_slice(&node[24..40]);
            found = Some(g);
        }
        at += len;
    }
    found.filter(|g| *g != [0; 16])
}

/// The largest partition list this reads: 128 entries of 128 bytes, the usual size, which the
/// BIOS loader's second stage reads whole (32 sectors).
pub const MAX_ENTRIES_BYTES: usize = 128 * 128;

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// The GPT's partition list must be read from this sector, according to its header. `None` if
/// `header` (the disk's sector 1) isn't a GPT header that passes its checks, or the list is
/// larger than [`MAX_ENTRIES_BYTES`].
pub fn gpt_entries_at(header: &[u8; 512]) -> Option<u64> {
    if &header[0..8] != b"EFI PART" {
        return None;
    }
    let size = u32_at(header, 12) as usize;
    if !(92..=512).contains(&size) {
        return None;
    }
    // The header's checksum is worked out with its own field as zero.
    let crc = pbr_crc::update(pbr_crc::crc32(&header[..16]), &[0; 4]);
    if pbr_crc::update(crc, &header[20..size]) != u32_at(header, 16) {
        return None;
    }
    // This is the main header, at sector 1, and the list follows it somewhere after.
    let (me, at) = (u64_at(header, 24), u64_at(header, 72));
    let (count, entry) = (u32_at(header, 80) as usize, u32_at(header, 84) as usize);
    let bytes = count.checked_mul(entry)?;
    let entry_ok = (128..=1024).contains(&entry) && entry.is_multiple_of(8);
    (me == 1 && at >= 2 && entry_ok && count > 0 && bytes <= MAX_ENTRIES_BYTES).then_some(at)
}

/// The unique GUID of the partition whose first sector is `first`, from a GPT's `header` and
/// its partition list (`entries`, read from where [`gpt_entries_at`] said), after checking
/// both. `None` if anything doesn't match.
pub fn from_gpt(header: &[u8; 512], entries: &[u8], first: u64) -> Option<[u8; 16]> {
    gpt_entries_at(header)?;
    let (count, entry) = (u32_at(header, 80) as usize, u32_at(header, 84) as usize);
    let list = entries.get(..count * entry)?;
    if pbr_crc::crc32(list) != u32_at(header, 88) {
        return None;
    }
    // Each entry: type GUID (0..16, all zero if the entry is unused), unique GUID (16..32),
    // first sector (32..40), last sector (40..48), attributes, name.
    let mut hits = list
        .chunks_exact(entry)
        .filter(|e| e[0..16] != [0; 16] && u64_at(e, 32) == first)
        .map(|e| {
            let mut g = [0u8; 16];
            g.copy_from_slice(&e[16..32]);
            g
        });
    let g = hits.next()?;
    // Two partitions can't start at one sector in a sound table.
    (hits.next().is_none() && g != [0; 16]).then_some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESP: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];

    fn node(kind: u8, sub: u8, body: &[u8]) -> Vec<u8> {
        let mut n = vec![kind, sub];
        n.extend_from_slice(&((body.len() + 4) as u16).to_le_bytes());
        n.extend_from_slice(body);
        n
    }

    fn hard_drive(guid: [u8; 16], signature_type: u8) -> Vec<u8> {
        let mut b = vec![0u8; 38];
        b[0..4].copy_from_slice(&2u32.to_le_bytes()); // partition 2
        b[4..12].copy_from_slice(&(65u64 * 2048).to_le_bytes());
        b[20..36].copy_from_slice(&guid);
        b[36] = 2; // GPT
        b[37] = signature_type;
        node(MEDIA, HARD_DRIVE, &b)
    }

    #[test]
    fn device_paths() {
        let g = [9u8; 16];
        // PCI root, PCI device, USB, then the partition, then the end.
        let mut p = node(2, 1, &[0; 8]);
        p.extend(node(1, 1, &[0, 0x1D]));
        p.extend(node(3, 5, &[1, 0]));
        p.extend(hard_drive(g, 2));
        p.extend(node(END, 0xFF, &[]));
        assert_eq!(from_device_path(&p), Some(g));
        // Cut short anywhere, nothing is read past the end, and the partition is missed.
        for n in 0..p.len() - 4 {
            assert!(from_device_path(&p[..n]).is_none_or(|x| x == g));
        }
        // An MBR partition (signature type 1) has no GUID.
        let mut m = node(2, 1, &[0; 8]);
        m.extend(hard_drive(g, 1));
        assert_eq!(from_device_path(&m), None);
        // A length of zero stops the walk instead of looping for ever.
        assert_eq!(from_device_path(&[2, 1, 0, 0, 4, 1, 42, 0]), None);
        // Nodes after the end node are ignored.
        let mut e = node(END, 0xFF, &[]);
        e.extend(hard_drive(g, 2));
        assert_eq!(from_device_path(&e), None);
    }

    /// A GPT header and its list of `count` entries, with the given partitions.
    fn gpt(count: u32, parts: &[([u8; 16], u64)]) -> ([u8; 512], Vec<u8>) {
        let mut list = vec![0u8; count as usize * 128];
        for (i, (g, first)) in parts.iter().enumerate() {
            let e = &mut list[i * 128..];
            e[0..16].copy_from_slice(&ESP);
            e[16..32].copy_from_slice(g);
            e[32..40].copy_from_slice(&first.to_le_bytes());
        }
        let mut h = [0u8; 512];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x1_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[80..84].copy_from_slice(&count.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&pbr_crc::crc32(&list).to_le_bytes());
        seal(&mut h);
        (h, list)
    }

    fn seal(h: &mut [u8; 512]) {
        h[16..20].fill(0);
        let c = pbr_crc::crc32(&h[..92]);
        h[16..20].copy_from_slice(&c.to_le_bytes());
    }

    #[test]
    fn gpts() {
        let (a, b) = ([1u8; 16], [2u8; 16]);
        let (h, list) = gpt(128, &[(a, 2048), (b, 65 * 2048)]);
        assert_eq!(gpt_entries_at(&h), Some(2));
        assert_eq!(from_gpt(&h, &list, 2048), Some(a));
        assert_eq!(from_gpt(&h, &list, 65 * 2048), Some(b));
        assert_eq!(from_gpt(&h, &list, 4096), None);
        // A list shorter than the header says.
        assert_eq!(from_gpt(&h, &list[..list.len() - 1], 2048), None);
        // Fewer entries is fine.
        let (h4, l4) = gpt(4, &[(a, 2048)]);
        assert_eq!(from_gpt(&h4, &l4, 2048), Some(a));
        // A list larger than the loader reads.
        let (big, _) = gpt(256, &[]);
        assert_eq!(gpt_entries_at(&big), None);
        // Two partitions starting at one sector: not a sound table.
        let (h2, l2) = gpt(128, &[(a, 2048), (b, 2048)]);
        assert_eq!(from_gpt(&h2, &l2, 2048), None);
    }

    #[test]
    fn damaged_gpts_give_nothing() {
        let (h, list) = gpt(128, &[([1; 16], 2048)]);
        // Every byte of the header that is checked.
        for i in 0..92 {
            let mut d = h;
            d[i] ^= 0x10;
            assert_eq!(from_gpt(&d, &list, 2048), None, "header byte {i}");
        }
        // Every byte of the used entry.
        for i in 0..128 {
            let mut l = list.clone();
            l[i] ^= 0x10;
            assert_eq!(from_gpt(&h, &l, 2048), None, "entry byte {i}");
        }
        // A header that checks out but says nonsense.
        for (o, v) in [(24, 2u64), (72, 1)] {
            let mut d = h;
            d[o..o + 8].copy_from_slice(&v.to_le_bytes());
            seal(&mut d);
            assert_eq!(gpt_entries_at(&d), None);
        }
        for (o, v) in [(12, 91u32), (12, 513), (84, 120), (84, 132), (80, 0), (80, u32::MAX)] {
            let mut d = h;
            d[o..o + 4].copy_from_slice(&v.to_le_bytes());
            seal(&mut d);
            assert_eq!(gpt_entries_at(&d), None, "offset {o} value {v}");
        }
    }
}
