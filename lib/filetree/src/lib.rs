// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Each program's file tree (docs/design/0014-files-and-file-trees.md, section 8): the paths
//! a program sees, and which capability serves each.
//!
//! A program is given **entries**: a path such as `/Home/Documents` and a capability for the
//! folder (or file) there. Everything it can reach is inside an entry; the folders above the
//! entries (`/`, `/Home`) exist only in the program, so that it can list them. This library is
//! the plain logic: the start-up format, making paths absolute and tidy, and finding which entry
//! a path is in. It never decides what a program may reach: capabilities do that. A program that
//! rewrote its own tree would only rename what it already holds.
//!
//! `..` is resolved here, by removing the name before it, and is never sent to a filesystem
//! server, so it can't climb out of an entry: at most it reaches the folders above the entries.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(test)]
mod tests;

/// The start-up block role of the file tree's description (a read-only memory object).
pub const ROLE: u32 = 12;
/// The role of entry 0's capability; entry `i`'s is `FIRST_ROLE + i`.
pub const FIRST_ROLE: u32 = 100;
/// The most entries.
pub const MAX_ENTRIES: usize = 256;
/// The largest description, in bytes.
pub const MAX_BYTES: usize = 65536;
/// The longest path, in bytes.
pub const MAX_PATH: usize = 4096;

/// File or folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A file.
    File = 1,
    /// A folder.
    Folder = 2,
}

/// What's wrong with a path or a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A name breaks the rules (control characters, too long), or the path is too long.
    BadPath,
    /// The description isn't in the format.
    Format,
    /// Two entries at the same path.
    Duplicate,
    /// An entry inside another entry.
    Nested,
    /// More than [`MAX_ENTRIES`], or bigger than [`MAX_BYTES`].
    TooBig,
}

/// One entry: a path and what's there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// An absolute, tidy path (as [`normalize`] makes them).
    pub path: String,
    /// File or folder.
    pub kind: Kind,
}

/// Checks one name: 1 to 255 characters, no `/`, no control characters, not `.` or `..`.
fn check_name(name: &str) -> Result<(), Error> {
    let n = name.chars().count();
    if n == 0 || n > 255 || name == "." || name == ".." {
        return Err(Error::BadPath);
    }
    if name.chars().any(|c| c == '/' || (c as u32) < 32 || c as u32 == 127) {
        return Err(Error::BadPath);
    }
    Ok(())
}

/// Makes `path` absolute (from `current`, an absolute tidy path, if it doesn't start with `/`)
/// and tidy: no empty names, no `.`, and `..` resolved by removing the name before it (`..` at
/// `/` stays at `/`). The result is `/` or `/name/name...`.
pub fn normalize(current: &str, path: &str) -> Result<String, Error> {
    if path.len() > MAX_PATH || current.len() > MAX_PATH {
        return Err(Error::BadPath);
    }
    let mut names: Vec<&str> = Vec::new();
    let start = if path.starts_with('/') { "" } else { current };
    for name in start.split('/').chain(path.split('/')) {
        match name {
            "" | "." => {}
            ".." => {
                names.pop();
            }
            _ => {
                check_name(name)?;
                names.push(name);
            }
        }
    }
    let mut out = String::new();
    for n in &names {
        out.push('/');
        out.push_str(n);
    }
    if out.is_empty() {
        out.push('/');
    }
    if out.len() > MAX_PATH {
        return Err(Error::BadPath);
    }
    Ok(out)
}

/// Whether `inner` is inside the folder at `outer` (both tidy absolute paths).
fn inside(outer: &str, inner: &str) -> bool {
    if outer == "/" {
        return inner != "/";
    }
    inner.len() > outer.len() && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'/'
}

/// Checks a set of entries against note 0014's rules: tidy absolute paths, none the same, none
/// inside another, at most [`MAX_ENTRIES`].
pub fn check(mounts: &[Mount]) -> Result<(), Error> {
    if mounts.len() > MAX_ENTRIES {
        return Err(Error::TooBig);
    }
    for (i, m) in mounts.iter().enumerate() {
        if m.path == "/" || normalize("/", &m.path)? != m.path {
            return Err(Error::BadPath);
        }
        for other in &mounts[..i] {
            if other.path == m.path {
                return Err(Error::Duplicate);
            }
            if inside(&other.path, &m.path) || inside(&m.path, &other.path) {
                return Err(Error::Nested);
            }
        }
    }
    Ok(())
}

/// The description a launcher gives a new program (note 0014, section 8): a 16-byte header
/// (`"PBRT"`, version 1, the entry count, 8 reserved bytes), then each entry: `u32` role,
/// `u16` path length, `u8` kind, a reserved 0, the path, zeros to a multiple of 8. Entry `i`'s
/// role is `FIRST_ROLE + i`.
pub fn encode(mounts: &[Mount]) -> Result<Vec<u8>, Error> {
    check(mounts)?;
    let mut b = Vec::new();
    b.extend_from_slice(b"PBRT");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(mounts.len() as u16).to_le_bytes());
    b.extend_from_slice(&[0; 8]);
    for (i, m) in mounts.iter().enumerate() {
        b.extend_from_slice(&(FIRST_ROLE + i as u32).to_le_bytes());
        b.extend_from_slice(&(m.path.len() as u16).to_le_bytes());
        b.push(m.kind as u8);
        b.push(0);
        b.extend_from_slice(m.path.as_bytes());
        while b.len() % 8 != 0 {
            b.push(0);
        }
    }
    if b.len() > MAX_BYTES {
        return Err(Error::TooBig);
    }
    Ok(b)
}

/// Reads a description, checking everything: it came from another program. Entries come back
/// in order, so entry `i`'s capability is at role `FIRST_ROLE + i`. The description arrives in
/// a memory object, which is whole pages, so zeros may follow the last entry; anything else
/// there is refused.
pub fn decode(b: &[u8]) -> Result<Vec<Mount>, Error> {
    if b.len() < 16 || b.len() > MAX_BYTES || &b[0..4] != b"PBRT" || b[4..6] != [1, 0] {
        return Err(Error::Format);
    }
    let count = usize::from(u16::from_le_bytes([b[6], b[7]]));
    if count > MAX_ENTRIES || b[8..16].iter().any(|&x| x != 0) {
        return Err(Error::Format);
    }
    let mut at = 16;
    let mut out = Vec::new();
    for i in 0..count {
        let head = b.get(at..at + 8).ok_or(Error::Format)?;
        let role = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let len = usize::from(u16::from_le_bytes([head[4], head[5]]));
        let kind = match head[6] {
            1 => Kind::File,
            2 => Kind::Folder,
            _ => return Err(Error::Format),
        };
        if role != FIRST_ROLE + i as u32 || head[7] != 0 {
            return Err(Error::Format);
        }
        let path = b.get(at + 8..at + 8 + len).ok_or(Error::Format)?;
        let path = core::str::from_utf8(path).map_err(|_| Error::Format)?;
        let end = (at + 8 + len).next_multiple_of(8);
        if b.get(at + 8 + len..end).is_none_or(|pad| pad.iter().any(|&x| x != 0)) {
            return Err(Error::Format);
        }
        at = end;
        out.push(Mount { path: String::from(path), kind });
    }
    if b[at..].iter().any(|&x| x != 0) {
        return Err(Error::Format);
    }
    check(&out)?;
    Ok(out)
}

/// Where a path is, in a tree.
#[derive(Debug, PartialEq, Eq)]
pub enum Found<'a> {
    /// In entry `index`; `rest` is the path below it, relative (empty for the entry itself).
    Entry {
        /// The entry's position.
        index: usize,
        /// The rest of the path, without a leading `/`.
        rest: &'a str,
    },
    /// A folder above some entries: the names directly inside it, and whether each is a folder.
    Above(Vec<(String, Kind)>),
    /// Nothing the program holds.
    Nothing,
}

/// A program's file tree.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    mounts: Vec<Mount>,
}

impl Tree {
    /// A tree of `mounts`, which must pass [`check`].
    pub fn new(mounts: Vec<Mount>) -> Result<Tree, Error> {
        check(&mounts)?;
        Ok(Tree { mounts })
    }

    /// The entries, in order.
    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// Adds an entry at the end (its index is the old count).
    pub fn add(&mut self, m: Mount) -> Result<usize, Error> {
        let mut all = self.mounts.clone();
        all.push(m);
        check(&all)?;
        self.mounts = all;
        Ok(self.mounts.len() - 1)
    }

    /// Where the tidy absolute path `path` is.
    pub fn find<'a>(&self, path: &'a str) -> Found<'a> {
        for (index, m) in self.mounts.iter().enumerate() {
            if path == m.path {
                return Found::Entry { index, rest: "" };
            }
            if inside(&m.path, path) {
                return Found::Entry { index, rest: &path[m.path.len() + 1..] };
            }
        }
        let mut children: Vec<(String, Kind)> = Vec::new();
        for m in &self.mounts {
            if inside(path, &m.path) {
                let below = if path == "/" { &m.path[1..] } else { &m.path[path.len() + 1..] };
                let (name, kind) = match below.split_once('/') {
                    Some((n, _)) => (n, Kind::Folder),
                    None => (below, m.kind),
                };
                if !children.iter().any(|(c, _)| c == name) {
                    children.push((String::from(name), kind));
                }
            }
        }
        if children.is_empty() && path != "/" {
            return Found::Nothing;
        }
        children.sort();
        Found::Above(children)
    }
}
