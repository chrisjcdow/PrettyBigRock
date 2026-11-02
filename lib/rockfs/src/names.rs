// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! File and folder names (ROCKFS.md, "Names").
//!
//! Names are UTF-8, 1 to 255 characters, and avoid every character Windows forbids, so any
//! RockFS file can be copied to Windows, Linux or a Mac unchanged. Two names count as the same
//! under `name_rule` 1 when they differ only in the case of the letters A-Z.

use crate::layout::{MAX_NAME_BYTES, MAX_NAME_CHARS};

/// Whether `name` may be given to a file or folder.
///
/// A `&str` in Rust is always valid UTF-8, so only the other rules are checked here. Names on
/// the disk are `&[u8]` and checked with `valid_stored_name`.
pub fn valid_name(name: &str) -> bool {
    valid_stored_name(name.as_bytes())
}

/// Whether `bytes` is a name the rules allow: valid UTF-8 and everything [`valid_name`] says.
/// A name read from a disk is checked with this before it is handed to anyone, since a damaged
/// or hostile disk may hold a name such as `..` or `a/b`.
pub fn valid_stored_name(bytes: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(bytes) else { return false };
    if text.is_empty() || bytes.len() > MAX_NAME_BYTES || text.chars().count() > MAX_NAME_CHARS {
        return false;
    }
    if text == "." || text == ".." {
        return false;
    }
    if text.ends_with(' ') || text.ends_with('.') {
        return false;
    }
    !text.chars().any(|c| {
        // Control characters, then `/` and the ones Windows forbids.
        c < ' '
            || c == '\u{7f}'
            || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    })
}

/// Whether two stored names count as the same under `name_rule` 1: equal, ignoring the case of
/// the letters A-Z. Other characters, such as `É` and `é`, must match exactly.
pub fn same_name(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
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
        let long_ok = "x".repeat(255);
        assert!(valid_name(&long_ok));
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
        // 255 characters of 4 bytes is 1020 bytes: fine; 256 characters is not.
        assert!(valid_name(&"😀".repeat(255)));
        assert!(!valid_name(&"😀".repeat(256)));
    }

    #[test]
    fn stored_names_must_be_utf8() {
        assert!(valid_stored_name(b"ok"));
        assert!(!valid_stored_name(&[0xff, 0xfe]));
        assert!(!valid_stored_name(&[b'a', 0xc3]));
    }

    #[test]
    fn only_a_to_z_fold() {
        assert!(same_name(b"Notes.TXT", b"notes.txt"));
        assert!(!same_name("École".as_bytes(), "école".as_bytes()));
        assert!(!same_name(b"a", b"ab"));
        assert!(same_name("É".as_bytes(), "É".as_bytes()));
    }
}
