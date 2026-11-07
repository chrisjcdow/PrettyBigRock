// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! File and folder names on FAT.
//!
//! A FAT folder entry has room for only an old DOS name: 8 characters, a dot and 3 more, all
//! capitals (the *short name*). VFAT ("long file names", from Windows 95) keeps the real name
//! in extra entries placed before it (the *long name*), in UTF-16 (the 16-bit encoding of
//! Unicode that Windows uses), 13 units to an entry. Every file with a long name also has a
//! short name, made up from it, so that old programs can still open the file. This module
//! checks names, makes short names, and converts both kinds to and from their stored forms.

use alloc::string::String;
use alloc::vec::Vec;

/// The most UTF-16 units a long name may have.
pub(crate) const MAX_UNITS: usize = 255;
/// UTF-16 units held by one long-name entry.
pub(crate) const UNITS_PER_ENTRY: usize = 13;
/// The most long-name entries a name can need: 255 units at 13 a piece.
pub(crate) const MAX_LFN_ENTRIES: usize = MAX_UNITS.div_ceil(UNITS_PER_ENTRY);

/// Characters FAT cannot have in a name: the ones Windows forbids, and control characters. (DEL
/// is allowed by the FAT specification but refused here, as RockFS refuses it, so that a name
/// made here can be copied anywhere.)
fn forbidden(c: char) -> bool {
    c < ' ' || c == '\u{7f}' || matches!(c, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|')
}

/// Whether `name` may be given to a file or folder: 1 to 255 UTF-16 units, none of the
/// characters FAT forbids (`" * / : < > ? \ |` and control characters), not ending in a space
/// or dot (Windows would silently remove them, so two names would become one), and not `.` or
/// `..`. A `&str` in Rust is always valid UTF-8, so nothing else needs checking.
pub fn valid_name(name: &str) -> bool {
    !(name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with(' ')
        || name.ends_with('.')
        || name.chars().any(forbidden)
        || name.encode_utf16().count() > MAX_UNITS)
}

/// Whether two names count as the same: equal, ignoring the case of the letters A-Z only.
/// (Windows also folds accented letters, which needs Unicode tables of our own: an open question
/// in note 0016.) Two names that differ only in, say, `é` and `É` are different names here.
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The checksum of a short name, which every long-name entry repeats so that a long name can be
/// told apart from one left behind by an older program that deleted or renamed the file without
/// knowing about long names. It rotates the running total right by one bit before adding each
/// of the 11 bytes (8 of name, 3 of extension).
pub(crate) fn checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

/// Where in a long-name entry its 13 UTF-16 units sit: 5, then 6, then 2.
const UNIT_OFFSETS: [usize; UNITS_PER_ENTRY] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

/// The 13 units held by a long-name entry.
pub(crate) fn lfn_units(entry: &[u8; 32]) -> [u16; UNITS_PER_ENTRY] {
    let mut units = [0u16; UNITS_PER_ENTRY];
    for (unit, &at) in units.iter_mut().zip(UNIT_OFFSETS.iter()) {
        *unit = u16::from_le_bytes([entry[at], entry[at + 1]]);
    }
    units
}

/// The long-name entries for `name`, in the order they go on the disk: the last part first,
/// flagged with 0x40 in its sequence number, down to part 1, which sits just before the short
/// entry. A name that does not fill its last entry is ended with a zero unit and padded with
/// 0xFFFF. `name` must be valid ([`valid_name`]).
pub(crate) fn lfn_entries(name: &str, short: &[u8; 11]) -> Vec<[u8; 32]> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let count = units.len().div_ceil(UNITS_PER_ENTRY);
    let sum = checksum(short);
    let mut entries = Vec::with_capacity(count);
    for part in 0..count {
        let mut e = [0u8; 32];
        e[0] = (part + 1) as u8 | if part + 1 == count { 0x40 } else { 0 };
        e[11] = 0x0F; // attributes: read-only + hidden + system + volume label = "long name"
        e[13] = sum;
        for (i, &at) in UNIT_OFFSETS.iter().enumerate() {
            let n = part * UNITS_PER_ENTRY + i;
            let unit = match n.cmp(&units.len()) {
                core::cmp::Ordering::Less => units[n],
                core::cmp::Ordering::Equal => 0x0000,
                core::cmp::Ordering::Greater => 0xFFFF,
            };
            e[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entries.push(e);
    }
    entries.reverse();
    entries
}

/// Decodes stored long-name units (up to the first zero unit) into a name. `None` if they are
/// not UTF-16 (a lone surrogate) or do not make a name [`valid_name`] accepts: such a long
/// name is ignored and the short name is used instead.
pub(crate) fn decode_long(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let name: String =
        char::decode_utf16(units[..end].iter().copied()).collect::<Result<_, _>>().ok()?;
    valid_name(&name).then_some(name)
}

/// The characters 0x80 to 0xFF stand for in DOS's code page 437, the one short names are
/// stored in unless told otherwise (other code pages exist; we have no way to know which one
/// the writer used, so we assume the most common).
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// One byte of a short name as a character.
fn oem_char(b: u8) -> char {
    if b < 0x80 {
        char::from(b)
    } else {
        CP437_HIGH.chars().nth(usize::from(b - 0x80)).unwrap_or('_')
    }
}

/// Bits of a short entry's "NT" byte (offset 12) that Windows NT and Linux use to remember
/// that the name part, or the extension, was typed in lower case (so that `readme.txt` needs
/// no long-name entry).
const LOWER_BASE: u8 = 0x08;
const LOWER_EXT: u8 = 0x10;

/// A short name as text: `NAME    EXT` becomes `NAME.EXT`. `None` if it cannot be a name (empty,
/// or with control characters).
pub(crate) fn short_display(short: &[u8; 11], nt_flags: u8) -> Option<String> {
    let part = |bytes: &[u8], lower: bool| -> String {
        let end = bytes.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
        bytes[..end]
            .iter()
            .map(|&b| {
                let c = oem_char(b);
                if lower { c.to_ascii_lowercase() } else { c }
            })
            .collect()
    };
    let mut base_bytes = [0u8; 8];
    base_bytes.copy_from_slice(&short[..8]);
    // A first byte of 0xE5 would mean "deleted", so a real 0xE5 is stored as 0x05.
    if base_bytes[0] == 0x05 {
        base_bytes[0] = 0xE5;
    }
    let mut name = part(&base_bytes, nt_flags & LOWER_BASE != 0);
    if name.is_empty() {
        return None;
    }
    let ext = part(&short[8..], nt_flags & LOWER_EXT != 0);
    if !ext.is_empty() {
        name.push('.');
        name.push_str(&ext);
    }
    valid_name(&name).then_some(name)
}

/// The characters (besides A-Z and 0-9) allowed in a short name.
fn short_char_ok(c: char) -> bool {
    c.is_ascii_uppercase()
        || c.is_ascii_digit()
        || matches!(c, '!' | '#' | '$' | '%' | '&' | '\'' | '(' | ')' | '-' | '@' | '^' | '_')
        || matches!(c, '`' | '{' | '}' | '~')
}

/// `name` as a short name, if it already is a valid one (capitals, at most 8 characters, a dot
/// and at most 3 more): then no long-name entry is needed.
pub(crate) fn exact_short(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = match name.split_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    if base.is_empty()
        || base.len() > 8
        || ext.len() > 3
        || name.matches('.').count() > 1
        || !base.chars().chain(ext.chars()).all(short_char_ok)
    {
        return None;
    }
    let mut short = [b' '; 11];
    short[..base.len()].copy_from_slice(base.as_bytes());
    short[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(short)
}

/// Makes a short name for a file whose long name is `name`, in the style Windows uses:
/// `LONGNA~1.TXT`. `taken` says which short names already exist in the folder. Returns `None`
/// only if a million names with the same start are taken.
///
/// The rules (Microsoft's FAT specification, with what Windows does where it is silent):
/// - capitalise; drop spaces and dots other than the last one; replace characters a short name
///   cannot have (including anything not ASCII, as we have no code page tables) with `_`;
/// - keep the first 8 characters of the name and 3 of the extension;
/// - if nothing was lost and the result is free, use it as it is (`Notes.txt` becomes
///   `NOTES.TXT`, with the long name beside it to keep the real capitals);
/// - otherwise add `~1` (or `~2`, ...) to the name, shortening it to fit, until the result is
///   free.
pub(crate) fn make_short(name: &str, taken: &dyn Fn(&[u8; 11]) -> bool) -> Option<[u8; 11]> {
    let mut lossy = false;
    // Leading dots and spaces are not kept.
    let trimmed = name.trim_start_matches(['.', ' ']);
    lossy |= trimmed.len() != name.len();
    let (base_text, ext_text) = match trimmed.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (trimmed, ""),
    };
    let convert = |text: &str, limit: usize, lossy: &mut bool| -> Vec<u8> {
        let mut out = Vec::new();
        for c in text.chars() {
            match c {
                ' ' | '.' => *lossy = true,
                _ => {
                    let up = c.to_ascii_uppercase();
                    if short_char_ok(up) {
                        out.push(up as u8);
                    } else {
                        out.push(b'_');
                        *lossy = true;
                    }
                }
            }
        }
        if out.len() > limit {
            out.truncate(limit);
            *lossy = true;
        }
        out
    };
    let mut base = convert(base_text, usize::MAX, &mut lossy);
    let ext = convert(ext_text, 3, &mut lossy);
    if base.is_empty() {
        base.push(b'_');
        lossy = true;
    }
    let build = |base: &[u8]| -> [u8; 11] {
        let mut short = [b' '; 11];
        short[..base.len()].copy_from_slice(base);
        short[8..8 + ext.len()].copy_from_slice(&ext);
        short
    };
    if !lossy && base.len() <= 8 && !taken(&build(&base)) {
        return Some(build(&base));
    }
    for n in 1..=999_999u32 {
        let tail = alloc::format!("~{n}");
        let keep = 8 - tail.len();
        let mut candidate: Vec<u8> = base.iter().copied().take(keep).collect();
        candidate.extend_from_slice(tail.as_bytes());
        let short = build(&candidate);
        if !taken(&short) {
            return Some(short);
        }
    }
    None
}

/// A volume label (11 bytes of code page 437, padded with spaces) as text.
pub(crate) fn label_text(label: &[u8; 11]) -> String {
    let end = label.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    label[..end].iter().map(|&b| oem_char(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_and_bad_names() {
        for good in ["a", "Notes.txt", "École", "name with spaces", "日本語", ".hidden", "a.b.c"]
        {
            assert!(valid_name(good), "{good}");
        }
        assert!(valid_name(&"x".repeat(255)));
        let bad = [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a:b",
            "a*b",
            "a?b",
            "a\"b",
            "a<b",
            "a>b",
            "a|b",
            "trailing ",
            "trailing.",
            "tab\there",
            "nul\0here",
            "del\u{7f}",
        ];
        for b in bad {
            assert!(!valid_name(b), "{b:?}");
        }
        assert!(!valid_name(&"x".repeat(256)));
        // The limit counts UTF-16 units, not characters: an emoji is two.
        assert!(valid_name(&"😀".repeat(127)));
        assert!(!valid_name(&"😀".repeat(128)));
    }

    #[test]
    fn only_a_to_z_fold() {
        assert!(same_name("Notes.TXT", "notes.txt"));
        assert!(!same_name("École", "école"));
        assert!(!same_name("a", "ab"));
    }

    #[test]
    fn checksum_matches_the_specification_example() {
        // Computed by hand from the algorithm in Microsoft's specification for "FOO     BAR".
        let mut sum = 0u8;
        for &b in b"FOO     BAR" {
            sum = (if sum & 1 != 0 { 0x80u8 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(b);
        }
        assert_eq!(checksum(b"FOO     BAR"), sum);
        assert_ne!(checksum(b"FOO     BAS"), sum);
    }

    #[test]
    fn long_names_round_trip() {
        let short = *b"LONGNA~1TXT";
        for name in [
            "a",
            "exactly13char",
            "exactly 26 characters long",
            "A long name with spaces and Ünïcödé.txt",
            "😀 emoji",
        ] {
            let entries = lfn_entries(name, &short);
            let n = name.encode_utf16().count().div_ceil(13);
            assert_eq!(entries.len(), n);
            // The first entry on disk is the last part, flagged.
            assert_eq!(entries[0][0], 0x40 | n as u8);
            assert_eq!(entries[n - 1][0] & 0x3F, 1);
            let mut units = Vec::new();
            for e in entries.iter().rev() {
                assert_eq!(e[11], 0x0F);
                assert_eq!(e[13], checksum(&short));
                assert_eq!(&e[26..28], &[0, 0]);
                units.extend_from_slice(&lfn_units(e));
            }
            assert_eq!(decode_long(&units).as_deref(), Some(name));
        }
    }

    #[test]
    fn decoding_refuses_nonsense() {
        // A lone surrogate, a forbidden character, an empty name.
        assert_eq!(decode_long(&[0xD800, 0]), None);
        assert_eq!(decode_long(&[b'a'.into(), b'/'.into(), 0]), None);
        assert_eq!(decode_long(&[0]), None);
        assert_eq!(decode_long(&[b'o'.into(), b'k'.into(), 0, 0xFFFF]).as_deref(), Some("ok"));
        // No terminator: all units count.
        assert_eq!(decode_long(&[b'o'.into(), b'k'.into()]).as_deref(), Some("ok"));
    }

    #[test]
    fn cp437_table_is_complete() {
        assert_eq!(CP437_HIGH.chars().count(), 128);
        assert_eq!(oem_char(0x80), 'Ç');
        assert_eq!(oem_char(0x82), 'é');
        assert_eq!(oem_char(0xE1), 'ß');
        assert_eq!(oem_char(0xFF), '\u{a0}');
    }

    #[test]
    fn short_names_are_shown() {
        assert_eq!(short_display(b"README  TXT", 0).as_deref(), Some("README.TXT"));
        assert_eq!(
            short_display(b"README  TXT", LOWER_BASE | LOWER_EXT).as_deref(),
            Some("readme.txt")
        );
        assert_eq!(short_display(b"README  TXT", LOWER_EXT).as_deref(), Some("README.txt"));
        assert_eq!(short_display(b"NOEXT      ", 0).as_deref(), Some("NOEXT"));
        assert_eq!(short_display(b"\x05AB     TXT", 0).as_deref(), Some("\u{3c3}AB.TXT"));
        assert_eq!(short_display(b".          ", 0), None);
        assert_eq!(short_display(b"..         ", 0), None);
        assert_eq!(short_display(b"           ", 0), None);
        assert_eq!(short_display(b"A\x01B     TXT", 0), None);
        assert_eq!(short_display(b"CAF\x82    TXT", 0).as_deref(), Some("CAFé.TXT"));
    }

    #[test]
    fn exact_short_names() {
        assert_eq!(exact_short("BOOTX64.EFI"), Some(*b"BOOTX64 EFI"));
        assert_eq!(exact_short("AARCH64"), Some(*b"AARCH64    "));
        assert_eq!(exact_short("A~1.TXT"), Some(*b"A~1     TXT"));
        for not in [
            "lower.txt",
            "TOOLONGNAME.TXT",
            "A.TOOL",
            "A.B.C",
            "WITH SPACE.TXT",
            "É.TXT",
            ".HIDDEN",
            "A+B.TXT",
        ] {
            assert_eq!(exact_short(not), None, "{not}");
        }
    }

    fn none_taken(_: &[u8; 11]) -> bool {
        false
    }

    #[test]
    fn made_up_short_names() {
        let s = |name: &str| {
            make_short(name, &none_taken).map(|n| String::from_utf8(n.to_vec()).unwrap())
        };
        // Nothing lost: no tail.
        assert_eq!(s("Notes.txt").as_deref(), Some("NOTES   TXT"));
        assert_eq!(s("noext").as_deref(), Some("NOEXT      "));
        // Too long, spaces, several dots, forbidden characters, leading dots, non-ASCII.
        assert_eq!(s("A long name.text").as_deref(), Some("ALONGN~1TEX"));
        assert_eq!(s("Program Files").as_deref(), Some("PROGRA~1   "));
        assert_eq!(s("a.b.c").as_deref(), Some("AB~1    C  "));
        assert_eq!(s("x+y;z.txt").as_deref(), Some("X_Y_Z~1 TXT"));
        assert_eq!(s(".hidden").as_deref(), Some("HIDDEN~1   "));
        assert_eq!(s("日本語.txt").as_deref(), Some("___~1   TXT"));
        assert_eq!(s("ab").as_deref(), Some("AB         "));
    }

    #[test]
    fn short_names_avoid_collisions() {
        let mut taken: Vec<[u8; 11]> = Vec::new();
        for i in 1..=12 {
            let short = make_short("A long name.text", &|n| taken.contains(n)).unwrap();
            let shown = String::from_utf8(short.to_vec()).unwrap();
            let expect = if i < 10 {
                alloc::format!("ALONGN~{i}TEX")
            } else {
                alloc::format!("ALONG~{i}TEX")
            };
            assert_eq!(shown, expect);
            taken.push(short);
        }
        // A name with no loss but already taken gets a tail too.
        let taken = [*b"NOTES   TXT"];
        assert_eq!(make_short("Notes.txt", &|n| taken.contains(n)), Some(*b"NOTES~1 TXT"));
        // Everything taken: gives up rather than looping.
        assert_eq!(make_short("whatever.txt", &|_| true), None);
    }

    #[test]
    fn labels() {
        assert_eq!(label_text(b"PBR BOOT   "), "PBR BOOT");
        assert_eq!(label_text(b"           "), "");
        assert_eq!(label_text(b"My Disk\x82   "), "My Diské");
    }
}
