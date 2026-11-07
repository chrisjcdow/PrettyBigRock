// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Folder entries.
//!
//! A FAT folder is a file whose contents are a list of 32-byte *entries*. A file or folder has
//! one **short entry** (its 8.3 name, attributes, times, first cluster and size) and, if its name
//! needs it, **long-name entries** just before that (see `names.rs`). Other entries mean other
//! things: the first byte 0xE5 marks a *deleted* entry whose slot is free for reuse, and 0x00
//! marks the *end* of the list: that slot and every one after it are free. The volume label is
//! an entry too, in the root folder. A folder other than the root begins with `.` (itself) and
//! `..` (its parent).
//!
//! [`Scanner`] turns a run of raw entries into [`Item`]s the way Windows does: a long name is
//! used only if its parts are all there, in order, and their checksum matches the short name
//! (otherwise an older program must have renamed the file, and the long name is stale); if not,
//! the short name is used.

use crate::names::{
    MAX_LFN_ENTRIES, UNITS_PER_ENTRY, checksum, decode_long, lfn_units, same_name, short_display,
};
use crate::time::{from_fat, to_fat};
use alloc::string::String;

/// Bytes in a folder entry.
pub(crate) const ENTRY: usize = 32;

/// Attribute bits of a short entry.
pub(crate) const ATTR_HIDDEN: u8 = 0x02;
pub(crate) const ATTR_VOLUME: u8 = 0x08;
pub(crate) const ATTR_DIR: u8 = 0x10;
pub(crate) const ATTR_ARCHIVE: u8 = 0x20;

/// A file or folder found in a folder.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    /// Its name: the long name if it has a good one, else the short name written out. `None` if
    /// neither makes a name we can hand out (the entry is then kept out of listings).
    pub name: Option<String>,
    /// The short name as stored (11 bytes: 8 of name, 3 of extension).
    pub short: [u8; 11],
    /// The slot of its first entry (a long-name entry, or the short one if it has none).
    pub first_slot: u32,
    /// The slot of its short entry.
    pub slot: u32,
    /// Its attribute bits.
    pub attr: u8,
    /// Its first cluster (0 for an empty file).
    pub cluster: u32,
    /// Its size in bytes (meaningless for a folder).
    pub size: u32,
    /// When it was created and last changed, in seconds since 1970 (0 if not recorded).
    pub created: u64,
    /// See `created`.
    pub modified: u64,
    /// Whether this is a `.` or `..` entry.
    pub dots: bool,
    /// The "NT" byte of the short entry, which says if the short name is meant in lower case.
    nt: u8,
}

impl Item {
    /// Whether it is a folder.
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIR != 0
    }

    /// Whether it is marked hidden.
    pub fn hidden(&self) -> bool {
        self.attr & ATTR_HIDDEN != 0
    }

    /// The short name as text (`PROGRA~1`), the way old programs and Windows' command line
    /// still know the file.
    pub fn alias(&self) -> Option<String> {
        short_display(&self.short, self.nt)
    }

    /// Whether `name` names this item: by its long name, or by its short name. Windows lets a
    /// file be opened by either, so a new name must not clash with either. Case differences in
    /// A-Z are ignored. `.` and `..` match nothing.
    pub fn matches(&self, name: &str) -> bool {
        !self.dots
            && (self.name.as_deref().is_some_and(|n| same_name(n, name))
                || self.alias().is_some_and(|a| same_name(&a, name)))
    }
}

/// What one entry turned out to be.
#[derive(Debug)]
pub(crate) enum Scan {
    /// The end marker: no more entries follow.
    End,
    /// Nothing to report: a deleted entry, a part of a long name, or something unusable.
    Skip,
    /// The volume label.
    Label([u8; 11]),
    /// A file or folder.
    Item(Item),
}

/// The long-name parts seen so far, waiting for their short entry.
struct Pending {
    /// The slot of the first part seen (which is the last part of the name).
    start: u32,
    /// The number of the part expected next; 0 when all parts have been seen.
    next: u8,
    /// How many parts the name has.
    count: u8,
    /// The checksum every part must carry.
    checksum: u8,
    /// The name's UTF-16 units, filled in part by part.
    units: [u16; MAX_LFN_ENTRIES * UNITS_PER_ENTRY],
}

/// Reads folder entries one at a time, in order, and joins long names to their short entries.
pub(crate) struct Scanner {
    pending: Option<Pending>,
}

fn le16(e: &[u8; ENTRY], at: usize) -> u16 {
    u16::from_le_bytes([e[at], e[at + 1]])
}

impl Scanner {
    /// A scanner at the start of a folder.
    pub fn new() -> Scanner {
        Scanner { pending: None }
    }

    /// Looks at the entry in `slot`.
    pub fn feed(&mut self, slot: u32, e: &[u8; ENTRY]) -> Scan {
        match e[0] {
            0x00 => {
                self.pending = None;
                return Scan::End;
            }
            0xE5 => {
                self.pending = None;
                return Scan::Skip;
            }
            _ => {}
        }
        let attr = e[11];
        // A long-name part has all four of these bits set (and so cannot be a real file's).
        if attr & 0x3F == 0x0F {
            self.long_part(slot, e);
            return Scan::Skip;
        }
        let pending = self.pending.take();
        let mut short = [0u8; 11];
        short.copy_from_slice(&e[..11]);
        if attr & ATTR_VOLUME != 0 {
            return if attr & ATTR_DIR == 0 { Scan::Label(short) } else { Scan::Skip };
        }
        let dots = short == *b".          " || short == *b"..         ";
        // The long name counts only if complete and matching this short name.
        let long = pending.filter(|p| p.next == 0 && p.checksum == checksum(&short));
        let first_slot = long.as_ref().map_or(slot, |p| p.start);
        let long_name = long
            .as_ref()
            .and_then(|p| decode_long(&p.units[..usize::from(p.count) * UNITS_PER_ENTRY]));
        let cluster = (u32::from(le16(e, 20)) << 16 | u32::from(le16(e, 26))) & 0x0FFF_FFFF;
        let item = Item {
            name: if dots { None } else { long_name.or_else(|| short_display(&short, e[12])) },
            short,
            first_slot,
            slot,
            attr,
            cluster,
            size: u32::from_le_bytes([e[28], e[29], e[30], e[31]]),
            created: from_fat(le16(e, 16), le16(e, 14), e[13]),
            modified: from_fat(le16(e, 24), le16(e, 22), 0),
            dots,
            nt: e[12],
        };
        Scan::Item(item)
    }

    /// Takes in a long-name part. A part that does not fit the sequence so far throws the
    /// sequence away: the short entry that follows then keeps its short name.
    fn long_part(&mut self, slot: u32, e: &[u8; ENTRY]) {
        let number = e[0] & 0x3F;
        // The first-cluster field of a long-name entry must be zero.
        if e[26] != 0 || e[27] != 0 || number == 0 || usize::from(number) > MAX_LFN_ENTRIES {
            self.pending = None;
            return;
        }
        let at = (usize::from(number) - 1) * UNITS_PER_ENTRY;
        if e[0] & 0x40 != 0 {
            // The first part on the disk is the last part of the name.
            let mut p = Pending {
                start: slot,
                next: number - 1,
                count: number,
                checksum: e[13],
                units: [0xFFFF; MAX_LFN_ENTRIES * UNITS_PER_ENTRY],
            };
            p.units[at..at + UNITS_PER_ENTRY].copy_from_slice(&lfn_units(e));
            self.pending = Some(p);
        } else {
            match self.pending.as_mut() {
                Some(p) if p.next == number && p.checksum == e[13] => {
                    p.units[at..at + UNITS_PER_ENTRY].copy_from_slice(&lfn_units(e));
                    p.next = number - 1;
                }
                _ => self.pending = None,
            }
        }
    }
}

/// A short entry for a new file or folder: first cluster `cluster`, size `size`, and all three
/// times `now` (seconds since 1970).
pub(crate) fn short_entry(
    short: &[u8; 11],
    attr: u8,
    cluster: u32,
    size: u32,
    now: u64,
) -> [u8; ENTRY] {
    let mut e = [0u8; ENTRY];
    e[..11].copy_from_slice(short);
    e[11] = attr;
    let (date, time, tenths) = to_fat(now);
    e[13] = tenths;
    e[14..16].copy_from_slice(&time.to_le_bytes());
    e[16..18].copy_from_slice(&date.to_le_bytes());
    e[18..20].copy_from_slice(&date.to_le_bytes());
    e[22..24].copy_from_slice(&time.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    set_cluster(&mut e, cluster);
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// The `.` and `..` entries that start every folder but the root. `parent` is the parent's
/// first cluster, or 0 if the parent is the root (a convention every FAT system follows).
pub(crate) fn dot_entries(cluster: u32, parent: u32, now: u64) -> [[u8; ENTRY]; 2] {
    [
        short_entry(b".          ", ATTR_DIR, cluster, 0, now),
        short_entry(b"..         ", ATTR_DIR, parent, 0, now),
    ]
}

/// Sets the first cluster of a short entry (split over two 16-bit fields).
pub(crate) fn set_cluster(e: &mut [u8; ENTRY], cluster: u32) {
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
}

/// Records a change at time `now` in a short entry: the modification time, and the access date
/// (FAT has only a date for that). Windows also sets the "archive" bit on a changed file.
pub(crate) fn touch(e: &mut [u8; ENTRY], now: u64, file: bool) {
    let (date, time, _) = to_fat(now);
    e[18..20].copy_from_slice(&date.to_le_bytes());
    e[22..24].copy_from_slice(&time.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    if file {
        e[11] |= ATTR_ARCHIVE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::lfn_entries;
    use alloc::vec::Vec;

    /// Feeds `entries` in order and collects the items.
    fn scan(entries: &[[u8; ENTRY]]) -> Vec<Item> {
        let mut s = Scanner::new();
        let mut items = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            match s.feed(i as u32, e) {
                Scan::Item(item) => items.push(item),
                Scan::End => break,
                _ => {}
            }
        }
        items
    }

    fn file(short: &[u8; 11], cluster: u32, size: u32) -> [u8; ENTRY] {
        short_entry(short, ATTR_ARCHIVE, cluster, size, 1_700_000_000)
    }

    #[test]
    fn a_short_entry() {
        let items = scan(&[file(b"HELLO   TXT", 0x0001_0203, 77)]);
        assert_eq!(items.len(), 1);
        let i = &items[0];
        assert_eq!(i.name.as_deref(), Some("HELLO.TXT"));
        assert_eq!((i.cluster, i.size, i.slot, i.first_slot), (0x0001_0203, 77, 0, 0));
        assert!(!i.is_dir() && !i.hidden());
        assert_eq!(i.created, 1_700_000_000);
        assert_eq!(i.modified, 1_700_000_000 & !1);
        assert!(i.matches("hello.txt") && !i.matches("hello.tx"));
    }

    #[test]
    fn a_long_name() {
        let short = *b"LONGNA~1TXT";
        let mut entries = lfn_entries("A long name.txt", &short);
        entries.push(file(&short, 5, 1));
        let items = scan(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name.as_deref(), Some("A long name.txt"));
        assert_eq!((items[0].first_slot, items[0].slot), (0, 2));
        // Either name finds it.
        assert!(items[0].matches("a LONG name.TXT"));
        assert!(items[0].matches("longna~1.txt"));
        assert!(!items[0].matches("longna~2.txt"));
    }

    #[test]
    fn broken_long_names_fall_back_to_the_short_name() {
        let short = *b"LONGNA~1TXT";
        let good = lfn_entries("A long name that needs two parts.txt", &short);
        assert_eq!(good.len(), 3);
        let short_entry = file(&short, 5, 1);
        let name = |entries: &[[u8; ENTRY]]| {
            let mut all = entries.to_vec();
            all.push(short_entry);
            let items = scan(&all);
            assert_eq!(items.len(), 1);
            items[0].name.clone().unwrap()
        };
        assert_eq!(name(&good), "A long name that needs two parts.txt");
        let fallback = "LONGNA~1.TXT";
        // A part missing from the middle, from the front, or from the end of the sequence.
        assert_eq!(name(&[good[0], good[2]]), fallback);
        assert_eq!(name(&good[1..]), fallback);
        assert_eq!(name(&good[..2]), fallback);
        // Parts in the wrong order.
        assert_eq!(name(&[good[1], good[0], good[2]]), fallback);
        // A checksum that does not match the short name (the file was renamed by an old program).
        let mut bad = good.clone();
        bad[1][13] ^= 1;
        assert_eq!(name(&bad), fallback);
        let mut bad = good.clone();
        bad[0][13] ^= 1;
        assert_eq!(name(&bad), fallback);
        // A number out of range, zero, or a non-zero cluster field.
        let mut bad = good.clone();
        bad[0][0] = 0x40 | 21;
        assert_eq!(name(&bad), fallback);
        let mut bad = good.clone();
        bad[2][0] = 0x40;
        assert_eq!(name(&bad), fallback);
        let mut bad = good.clone();
        bad[1][26] = 1;
        assert_eq!(name(&bad), fallback);
        // A deleted part.
        let mut bad = good.clone();
        bad[1][0] = 0xE5;
        assert_eq!(name(&bad), fallback);
        // A forbidden character in the long name, or a lone surrogate.
        let mut bad = good.clone();
        bad[2][1] = b'/';
        assert_eq!(name(&bad), fallback);
        let mut bad = good.clone();
        bad[2][1] = 0x00;
        bad[2][2] = 0xD8;
        assert_eq!(name(&bad), fallback);
        // A long name left over from a different file does not attach to the next one.
        let mut all = good.clone();
        all.push(file(b"OTHER   TXT", 6, 1));
        assert_eq!(scan(&all)[0].name.as_deref(), Some("OTHER.TXT"));
    }

    #[test]
    fn special_entries() {
        let mut label = file(b"MY DISK    ", 0, 0);
        label[11] = ATTR_VOLUME;
        let mut folder = file(b"SUB        ", 9, 0);
        folder[11] = ATTR_DIR;
        let mut deleted = file(b"GONE    TXT", 8, 3);
        deleted[0] = 0xE5;
        let dots = dot_entries(9, 0, 0);
        let mut s = Scanner::new();
        assert!(matches!(s.feed(0, &label), Scan::Label(l) if &l == b"MY DISK    "));
        assert!(matches!(s.feed(1, &deleted), Scan::Skip));
        let Scan::Item(sub) = s.feed(2, &folder) else { panic!() };
        assert!(sub.is_dir());
        let Scan::Item(dot) = s.feed(3, &dots[0]) else { panic!() };
        assert!(dot.dots && dot.name.is_none() && !dot.matches(".") && !dot.matches(".."));
        let Scan::Item(dotdot) = s.feed(4, &dots[1]) else { panic!() };
        assert!(dotdot.dots);
        // A volume label that is also marked as a folder is rubbish.
        let mut both = label;
        both[11] = ATTR_VOLUME | ATTR_DIR;
        assert!(matches!(s.feed(5, &both), Scan::Skip));
        assert!(matches!(s.feed(6, &[0u8; ENTRY]), Scan::End));
    }

    #[test]
    fn entries_with_unusable_names_are_kept_but_nameless() {
        let items = scan(&[file(b"A\x01B     TXT", 4, 4), file(b"        TXT", 5, 4)]);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.name.is_none() && !i.matches("a")));
    }

    #[test]
    fn building_entries() {
        let mut e = short_entry(b"A       TXT", ATTR_ARCHIVE, 0x0012_3456, 99, 1_800_000_001);
        assert_eq!(&e[20..22], &[0x12, 0x00]);
        assert_eq!(&e[26..28], &[0x56, 0x34]);
        let items = scan(&[e]);
        assert_eq!(items[0].cluster, 0x0012_3456);
        assert_eq!(items[0].created, 1_800_000_001);
        set_cluster(&mut e, 7);
        touch(&mut e, 1_800_000_100, true);
        let items = scan(&[e]);
        assert_eq!((items[0].cluster, items[0].modified), (7, 1_800_000_100));
        // The top four bits of a cluster number are not part of it.
        e[21] = 0xF0;
        assert_eq!(scan(&[e])[0].cluster, 7);
    }
}
