// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Paths and names: joining, splitting and checking them.
//!
//! A path is text like `/Home/Documents/letter.txt`: parts separated by `/`, always starting
//! at the top (`/`) of the program's own tree. Most functions here take `&str`, a borrowed
//! view of text somebody else owns (Rust's way of reading text without copying it); the ones
//! that make something new give back a `String`, which owns its text.

use crate::tree::BrowseError;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest name, in characters (the same limit as the file servers').
const MAX_NAME: usize = 255;

/// Joins a name onto a folder's path: `join("/", "a")` is `/a`, `join("/a", "b")` is `/a/b`.
/// The name is used as given (check it with [`check_name`] first if it came from a person).
pub fn join(folder: &str, name: &str) -> String {
    let mut out = String::from(folder.trim_end_matches('/'));
    out.push('/');
    out.push_str(name);
    out
}

/// The folder a path is in: `/a/b` gives `/a`, `/a` gives `/`, and `/` (or text with no `/` at
/// all) has none. `Option` is Rust's "maybe a value": `Some(x)` or `None`.
pub fn parent(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let cut = trimmed.rfind('/')?;
    if cut == 0 { Some("/") } else { trimmed.get(..cut) }
}

/// The last part of a path (`/a/b` gives `b`); `/` is named `/`.
pub fn name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    match trimmed.rfind('/') {
        Some(cut) => trimmed.get(cut + 1..).unwrap_or(trimmed),
        None => trimmed,
    }
}

/// The parts of a path for a path bar, each with the path that leads to it:
/// `/Home/Documents` gives `[("/", "/"), ("Home", "/Home"), ("Documents", "/Home/Documents")]`.
/// Empty parts (from `//`) are skipped.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    out.push((String::from("/"), String::from("/")));
    let mut so_far = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        so_far.push('/');
        so_far.push_str(part);
        out.push((String::from(part), so_far.clone()));
    }
    out
}

/// Whether `inner` is `outer` or somewhere below it: `/a` is inside `/a` and `/`, but `/ab` is
/// not inside `/a` (a whole part must match, not just the first letters).
pub fn inside(outer: &str, inner: &str) -> bool {
    let outer = outer.trim_end_matches('/');
    if outer.is_empty() {
        return inner.starts_with('/');
    }
    // `starts_with` having succeeded, `outer.len()` is a character boundary of `inner`, and
    // looking at the byte there (with `get`, which answers `None` instead of failing past the
    // end) tells whether `outer` ended exactly at the end of a part.
    inner.starts_with(outer) && matches!(inner.as_bytes().get(outer.len()), None | Some(b'/'))
}

/// The extension of a file name, the text after its last `.`: `a.TXT` gives `TXT` (as written;
/// compare without regard to case). A name that starts with its only `.` (`.profile`), ends
/// with it (`a.`), or has none gives `None`.
pub fn extension(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    name.get(dot + 1..)
}

/// The rules for a name that a person types for a file or folder: 1 to 255 characters, no `/`,
/// no control characters (below 32, and 127), not `.` or `..`, and no space at the start or the
/// end (such names are too easy to mistake for another). Anything else is
/// [`BrowseError::BadName`]. `Result` is Rust's "this worked, or here is why not".
pub fn check_name(name: &str) -> Result<(), BrowseError> {
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_NAME || name == "." || name == ".." {
        return Err(BrowseError::BadName);
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Err(BrowseError::BadName);
    }
    if name.chars().any(|c| c == '/' || (c as u32) < 32 || c as u32 == 127) {
        return Err(BrowseError::BadName);
    }
    Ok(())
}

/// Checks that a path is tidy: absolute, with no empty, `.` or `..` parts and no `/` at the end
/// (except `/` itself). It gives back the same path if it is, and `None` if not; this library
/// never rewrites a path, because `..` could lead out of where the program is allowed.
pub fn tidy(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    if rest.is_empty() {
        return Some(String::from("/"));
    }
    if rest.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return None;
    }
    Some(String::from(path))
}
