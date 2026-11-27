// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading Intel's microcode update files (docs/design/0027-kernel-1.4.md, section 1e).
//!
//! A processor's **microcode** is the firmware inside it that carries out its more complicated
//! instructions. Intel fixes flaws by publishing new microcode, which the firmware or the
//! operating system loads into the processor at every start (it is forgotten when the power
//! goes). Intel publishes one file per processor family, model and stepping, named like
//! `06-2a-07`; each file is a list of updates, each for one or more processor signatures.
//!
//! Each update is a 48-byte header followed by its data and, optionally, a table of further
//! signatures it also fits:
//!
//! | Offset | Field |
//! |-------:|-------|
//! | 0 | header version (1) |
//! | 4 | revision |
//! | 8 | date |
//! | 12 | processor signature (CPUID leaf 1's `EAX`) |
//! | 16 | checksum: the update's 32-bit words sum to zero |
//! | 20 | loader version (1) |
//! | 24 | processor flags: which platforms (`IA32_PLATFORM_ID`) it fits, one bit each |
//! | 28 | data size (0 means 2000) |
//! | 32 | total size (0 means 2048) |
//!
//! The extended table, when the total size leaves room for one after the data: a count, a
//! checksum (the table's words sum to zero) and 12 reserved bytes, then `count` entries of
//! (signature, processor flags, checksum).
//!
//! **The processor checks Intel's signature on the update itself** and refuses anything else,
//! so a damaged or hostile file can't put foreign code into the processor. These checks only
//! keep the kernel from passing nonsense: every length is checked against what is left, and
//! anything that doesn't parse is skipped.

#![cfg_attr(not(test), no_std)]

/// The size of an update's header.
pub const HEADER: usize = 48;
/// The size of the extended signature table's header, and of each of its entries.
const EXT_HEADER: usize = 20;
const EXT_ENTRY: usize = 12;

/// One update from a file, its checksums checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update<'a> {
    /// The microcode revision it brings.
    pub revision: u32,
    /// When Intel made it, as `0xMMDDYYYY` in hexadecimal digits.
    pub date: u32,
    /// The whole update, header included.
    pub bytes: &'a [u8],
    /// The part the processor is given: the data after the header.
    pub data: &'a [u8],
}

impl Update<'_> {
    /// True if the update fits a processor with this CPUID signature and platform (0 to 7,
    /// from `IA32_PLATFORM_ID`), by its header or its extended table.
    pub fn fits(&self, signature: u32, platform: u32) -> bool {
        let want = 1u32.checked_shl(platform).unwrap_or(0);
        let matches = |sig: u32, flags: u32| sig == signature && (flags & want != 0 || flags == 0);
        if matches(u32_at(self.bytes, 12), u32_at(self.bytes, 24)) {
            return true;
        }
        let used = HEADER + self.data.len();
        let ext = &self.bytes[used..];
        if ext.len() < EXT_HEADER {
            return false;
        }
        let count = u32_at(ext, 0) as usize;
        ext[EXT_HEADER..]
            .chunks_exact(EXT_ENTRY)
            .take(count)
            .any(|e| matches(u32_at(e, 0), u32_at(e, 4)))
    }
}

/// Why an update in a file was skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bad {
    /// The file ends inside the header, or the update's sizes run past the end.
    Truncated,
    /// A header or loader version other than 1, or sizes that don't make sense.
    Header,
    /// The update's words don't sum to zero.
    Checksum,
    /// The extended signature table's words don't sum to zero, or its count runs past it.
    ExtendedTable,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The sum of `b`'s 32-bit little-endian words, wrapping. `b`'s length is a multiple of 4.
fn sum(b: &[u8]) -> u32 {
    b.chunks_exact(4).fold(0u32, |s, w| s.wrapping_add(u32_at(w, 0)))
}

/// Reads the update at the start of `b`. Returns it (or why it's bad) and its total size, or
/// `None` when nothing more can be read: sizes that run past the end leave no way to find the
/// next update.
fn one(b: &[u8]) -> Option<(Result<Update<'_>, Bad>, usize)> {
    if b.len() < HEADER {
        return (!b.is_empty()).then_some((Err(Bad::Truncated), b.len()));
    }
    let data_size = match u32_at(b, 28) {
        0 => 2000,
        n => n as usize,
    };
    let total = match u32_at(b, 32) {
        0 => 2048,
        n => n as usize,
    };
    let sane = data_size.is_multiple_of(4)
        && total.is_multiple_of(4)
        && data_size.checked_add(HEADER).is_some_and(|d| d <= total);
    if !sane {
        return Some((Err(Bad::Header), b.len()));
    }
    let Some(bytes) = b.get(..total) else { return Some((Err(Bad::Truncated), b.len())) };
    if u32_at(b, 0) != 1 || u32_at(b, 20) != 1 {
        return Some((Err(Bad::Header), total));
    }
    if sum(bytes) != 0 {
        return Some((Err(Bad::Checksum), total));
    }
    let ext = &bytes[HEADER + data_size..];
    if !ext.is_empty() {
        let count = if ext.len() >= EXT_HEADER { u32_at(ext, 0) as usize } else { usize::MAX };
        let fits = count.checked_mul(EXT_ENTRY).and_then(|n| n.checked_add(EXT_HEADER));
        match fits {
            Some(n) if n <= ext.len() && sum(&ext[..n]) == 0 => {}
            _ => return Some((Err(Bad::ExtendedTable), total)),
        }
    }
    let update = Update {
        revision: u32_at(b, 4),
        date: u32_at(b, 8),
        bytes,
        data: &bytes[HEADER..HEADER + data_size],
    };
    Some((Ok(update), total))
}

/// Every update in a file, in order, each either checked or with why it was skipped.
pub fn updates(file: &[u8]) -> impl Iterator<Item = Result<Update<'_>, Bad>> {
    let mut rest = file;
    core::iter::from_fn(move || {
        let (u, used) = one(rest)?;
        rest = &rest[used..];
        Some(u)
    })
}

/// The newest update in `file` that fits a processor with this CPUID signature and platform,
/// if any is newer than `running` (the revision the processor has now). Never an older or the
/// same revision.
pub fn newest<'a>(
    file: &'a [u8],
    signature: u32,
    platform: u32,
    running: u32,
) -> Option<Update<'a>> {
    updates(file)
        .filter_map(Result::ok)
        .filter(|u| u.fits(signature, platform) && u.revision > running)
        .max_by_key(|u| u.revision)
}

/// The name Intel gives the file for a processor with this CPUID signature (leaf 1's `EAX`):
/// family, model and stepping in two-digit lower-case hexadecimal, such as `06-2a-07`.
pub fn file_name(signature: u32) -> [u8; 8] {
    let stepping = signature & 0xF;
    let mut model = (signature >> 4) & 0xF;
    let mut family = (signature >> 8) & 0xF;
    // Families 6 and 15 put more of the model in the "extended model" bits; family 15 adds
    // the "extended family" too.
    if family == 6 || family == 15 {
        model |= ((signature >> 16) & 0xF) << 4;
    }
    if family == 15 {
        family += (signature >> 20) & 0xFF;
    }
    let hex = |v: u32| {
        let d = |n: u32| b"0123456789abcdef"[(n & 0xF) as usize];
        [d(v >> 4), d(v)]
    };
    let (f, m, s) = (hex(family), hex(model), hex(stepping));
    [f[0], f[1], b'-', m[0], m[1], b'-', s[0], s[1]]
}

/// The other way round from [`file_name`]: the CPUID signature a file named `name` is for,
/// with the processor type and reserved bits zero, or `None` if `name` isn't such a name. The
/// BIOS loader, which can't read files by name, matches on this (masked with `0x0FFF_0FFF`).
pub fn signature_of_name(name: &str) -> Option<u32> {
    let mut parts = name.split('-');
    let mut next = || {
        let p = parts.next()?;
        (p.len() == 2).then(|| u32::from_str_radix(p, 16).ok()).flatten()
    };
    let (family, model, stepping) = (next()?, next()?, next()?);
    if parts.next().is_some() || stepping > 0xF {
        return None;
    }
    let (base, ext_family) = if family >= 0xF { (0xF, family - 0xF) } else { (family, 0) };
    if (base != 6 && base != 0xF && model > 0xF) || ext_family > 0xFF {
        return None;
    }
    Some(ext_family << 20 | (model >> 4) << 16 | base << 8 | (model & 0xF) << 4 | stepping)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An update with the given header fields, `data_words` words of data and the extended
    /// signatures `ext`, with every checksum right.
    fn update(
        signature: u32,
        flags: u32,
        revision: u32,
        data_words: usize,
        ext: &[(u32, u32)],
    ) -> Vec<u8> {
        let data = data_words * 4;
        let ext_len = if ext.is_empty() { 0 } else { EXT_HEADER + ext.len() * EXT_ENTRY };
        let total = HEADER + data + ext_len;
        let mut b = vec![0u8; total];
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, 1);
        put(&mut b, 4, revision);
        put(&mut b, 8, 0x0217_2019);
        put(&mut b, 12, signature);
        put(&mut b, 20, 1);
        put(&mut b, 24, flags);
        put(&mut b, 28, data as u32);
        put(&mut b, 32, total as u32);
        for i in 0..data_words {
            put(&mut b, HEADER + i * 4, (i as u32).wrapping_mul(0x9E37_79B9));
        }
        if !ext.is_empty() {
            let e = HEADER + data;
            put(&mut b, e, ext.len() as u32);
            for (i, &(s, f)) in ext.iter().enumerate() {
                put(&mut b, e + EXT_HEADER + i * EXT_ENTRY, s);
                put(&mut b, e + EXT_HEADER + i * EXT_ENTRY + 4, f);
            }
            let c = sum(&b[e..]);
            put(&mut b, e + 4, 0u32.wrapping_sub(c));
        }
        let c = sum(&b);
        put(&mut b, 16, 0u32.wrapping_sub(c));
        b
    }

    const ROXY: u32 = 0x206A7;

    #[test]
    fn finds_the_newest_that_fits() {
        let mut file = update(ROXY, 0x12, 0x28, 10, &[]);
        file.extend(update(ROXY, 0x12, 0x2F, 12, &[]));
        file.extend(update(0x306A9, 0x12, 0x40, 8, &[]));
        file.extend(update(ROXY, 0x01, 0x30, 8, &[])); // another platform
        assert_eq!(updates(&file).filter(Result::is_ok).count(), 4);
        // Platform 1 (bit 0x02) or 4 (bit 0x10) fits the first two.
        for platform in [1, 4] {
            let u = newest(&file, ROXY, platform, 0x28).unwrap();
            assert_eq!(u.revision, 0x2F);
            assert_eq!(u.data.len(), 48);
            assert_eq!(u.bytes.len(), HEADER + 48);
        }
        assert_eq!(newest(&file, ROXY, 0, 0x28).unwrap().revision, 0x30);
        // Never older or the same.
        assert_eq!(newest(&file, ROXY, 1, 0x2F), None);
        assert_eq!(newest(&file, ROXY, 1, 0x50), None);
        assert_eq!(newest(&file, 0x206A6, 1, 0), None);
        assert_eq!(newest(&file, ROXY, 9, 0), None);
    }

    #[test]
    fn extended_signatures() {
        let file = update(0x306A9, 0x02, 0x21, 4, &[(ROXY, 0x10), (0x206A6, 0x02)]);
        assert_eq!(newest(&file, ROXY, 4, 0).unwrap().revision, 0x21);
        assert_eq!(newest(&file, ROXY, 1, 0), None);
        assert_eq!(newest(&file, 0x206A6, 1, 0).unwrap().revision, 0x21);
        assert_eq!(newest(&file, 0x306A9, 1, 0).unwrap().data.len(), 16);
    }

    #[test]
    fn damage_is_skipped() {
        let good = update(ROXY, 0x12, 0x2F, 10, &[]);
        // A wrong checksum: skipped, and the next one still read.
        let mut bad = update(ROXY, 0x12, 0x30, 10, &[]);
        bad[HEADER] ^= 1;
        let mut file = bad.clone();
        file.extend(&good);
        let all: Vec<_> = updates(&file).collect();
        assert_eq!(all[0], Err(Bad::Checksum));
        assert_eq!(all[1].unwrap().revision, 0x2F);
        assert_eq!(newest(&file, ROXY, 1, 0).unwrap().revision, 0x2F);
        // A bad extended table.
        let mut e = update(0x306A9, 0x02, 0x21, 4, &[(ROXY, 0x10)]);
        let at = HEADER + 16;
        e[at..at + 4].copy_from_slice(&5u32.to_le_bytes()); // a count past the end
        let c = sum(&e);
        let fix = u32_at(&e, 16).wrapping_sub(c);
        e[16..20].copy_from_slice(&fix.to_le_bytes());
        assert_eq!(updates(&e).next(), Some(Err(Bad::ExtendedTable)));
        // Sizes running past the end: nothing more can be read.
        let mut t = good.clone();
        t.truncate(good.len() - 4);
        assert_eq!(updates(&t).collect::<Vec<_>>(), vec![Err(Bad::Truncated)]);
        assert_eq!(updates(&good[..20]).collect::<Vec<_>>(), vec![Err(Bad::Truncated)]);
        assert_eq!(updates(&[]).count(), 0);
        // Nonsense sizes.
        let mut s = good.clone();
        s[28..32].copy_from_slice(&(good.len() as u32).to_le_bytes()); // data bigger than total
        assert_eq!(updates(&s).next(), Some(Err(Bad::Header)));
        let mut v = good.clone();
        v[0] = 2;
        assert_eq!(updates(&v).next(), Some(Err(Bad::Header)));
    }

    #[test]
    fn random_damage_never_panics() {
        let mut file = update(ROXY, 0x12, 0x28, 30, &[(0x206A6, 0x02)]);
        file.extend(update(ROXY, 0x12, 0x2F, 40, &[]));
        let mut x: u64 = 1;
        for _ in 0..20_000 {
            let mut f = file.clone();
            for _ in 0..1 + x % 4 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let i = (x >> 33) as usize % f.len();
                f[i] = (x >> 20) as u8;
            }
            let cut = (x >> 40) as usize % (f.len() + 1);
            for u in updates(&f[..cut]).flatten() {
                assert!(u.data.len() + HEADER <= u.bytes.len());
                let _ = u.fits(ROXY, 1);
            }
            let _ = newest(&f, ROXY, 1, 0);
        }
    }

    #[test]
    fn file_names() {
        assert_eq!(&file_name(ROXY), b"06-2a-07");
        assert_eq!(&file_name(0x906E9), b"06-9e-09"); // Impala's i7-7700K
        assert_eq!(&file_name(0xF29), b"0f-02-09");
        assert_eq!(&file_name(0x0010_0F00), b"10-00-00"); // family 15 plus 1
    }

    #[test]
    fn names_and_signatures() {
        assert_eq!(signature_of_name("06-2a-07"), Some(ROXY));
        assert_eq!(signature_of_name("06-9e-09"), Some(0x906E9));
        for name in ["06-2a-07", "06-9e-09", "0f-02-09", "10-00-00", "05-08-0c"] {
            let s = signature_of_name(name).unwrap();
            assert_eq!(&file_name(s), name.as_bytes(), "{name}");
        }
        for bad in ["06-2a", "06-2a-07-01", "6-2a-07", "06-2a-1f", "05-20-00", "zz-00-00", ""] {
            assert_eq!(signature_of_name(bad), None, "{bad}");
        }
    }

    /// The real files, when this machine has them (Ubuntu's `intel-microcode` package).
    #[test]
    fn intel_files() {
        let dir = std::path::Path::new("/lib/firmware/intel-ucode");
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("skipped: no {}", dir.display());
            return;
        };
        let mut count = 0;
        for e in entries.flatten() {
            let name = e.file_name().into_string().unwrap();
            // Ubuntu adds a few files of its own (`06-8e-0a.initramfs`), which the build tool
            // leaves out the same way.
            let Some(sig) = signature_of_name(&name) else { continue };
            assert_eq!(file_name(sig), name.as_bytes());
            let file = std::fs::read(e.path()).unwrap();
            for u in updates(&file) {
                assert!(u.is_ok(), "{}: {u:?}", e.path().display());
                count += 1;
            }
        }
        assert!(count > 100, "only {count} updates");
        if let Ok(roxy) = std::fs::read(dir.join("06-2a-07")) {
            let u = newest(&roxy, ROXY, 1, 0x28).unwrap();
            assert_eq!(u.revision, 0x2F);
            assert_eq!(u.date, 0x0217_2019);
        }
    }
}
