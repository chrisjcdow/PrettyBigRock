// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The inside of a folder block (ROCKFS.md, "Folders").
//!
//! A folder's contents are stored like a file's bytes, but they are a list of entries, each
//! a 16-byte header followed by the name. Entries never cross a block boundary; the last one in
//! a block stretches to the block's end, and a removed name just becomes an entry with inode 0
//! ("unused space") that a later name can be carved out of.
//!
//! This module works on one block held in memory and knows nothing about the disk. Everything
//! read from a block is checked before it is trusted, because the block may be damaged or
//! deliberately malicious: an entry that is too short, runs past the block's end, or has a name
//! longer than its entry is reported as [`FsError::Damaged`], never indexed blindly.

use crate::layout::*;
use alloc::boxed::Box;
use pbr_fs::FsError;

/// Bytes of an entry before the name.
pub const HEADER: usize = 16;

/// One entry's header, checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where the entry starts in the block.
    pub pos: usize,
    /// The inode it names; 0 means unused space.
    pub inode: u32,
    /// Bytes from the start of this entry to the start of the next.
    pub len: usize,
    /// Bytes of name.
    pub name_len: usize,
    /// A copy of the inode's type (1 file, 2 folder).
    pub kind: u8,
}

impl Entry {
    /// The name's bytes within `block`.
    pub fn name<'a>(&self, block: &'a [u8]) -> &'a [u8] {
        &block[self.pos + HEADER..self.pos + HEADER + self.name_len]
    }

    /// Bytes the entry needs for itself: header and name, rounded up to 8. The rest of `len`
    /// is spare room a new name can be carved from.
    pub fn used(&self) -> usize {
        if self.inode == 0 { 0 } else { align8(HEADER + self.name_len) }
    }
}

/// Reads and checks the entry header at `pos`.
fn entry_at(block: &[u8], pos: usize) -> Result<Entry, FsError> {
    if pos + HEADER > BLOCK {
        return Err(FsError::Damaged);
    }
    let len = usize::from(u16_at(block, pos + 4));
    let name_len = usize::from(u16_at(block, pos + 6));
    let inode = u32_at(block, pos);
    // `len` is at least the header, a multiple of 8, and stays inside the block; a name must
    // fit inside its own entry. Together these make the walk from one entry to the next always
    // move forward by a sensible amount, so a loop over a block always ends.
    if len < HEADER || len % 8 != 0 || pos + len > BLOCK {
        return Err(FsError::Damaged);
    }
    if inode != 0 && HEADER + name_len > len {
        return Err(FsError::Damaged);
    }
    Ok(Entry { pos, inode, len, name_len, kind: block[pos + 8] })
}

/// Whether a block is entirely zeros: a folder block that was never filled in, which the C and
/// Python implementations both treat as one empty entry.
fn all_zero(block: &[u8]) -> bool {
    block.iter().all(|&b| b == 0)
}

/// Walks the entries of a block, including unused ones, stopping after the first problem.
pub struct Entries<'a> {
    block: &'a [u8],
    pos: usize,
}

impl<'a> Entries<'a> {
    /// Starts at the first entry. `block` is [`BLOCK`] bytes.
    pub fn new(block: &'a [u8]) -> Entries<'a> {
        let blank = u16_at(block, 4) == 0 && all_zero(block);
        Entries { block, pos: if blank { BLOCK } else { 0 } }
    }
}

impl Iterator for Entries<'_> {
    type Item = Result<Entry, FsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= BLOCK {
            return None;
        }
        match entry_at(self.block, self.pos) {
            Ok(e) => {
                self.pos += e.len;
                Some(Ok(e))
            }
            Err(err) => {
                self.pos = BLOCK;
                Some(Err(err))
            }
        }
    }
}

/// A block holding one entry, for a folder's first block or one added when the others are full.
pub fn new_block(name: &[u8], inode: u32, kind: u8) -> Box<[u8; BLOCK]> {
    let mut block = Box::new([0u8; BLOCK]);
    write_entry(&mut block[..], 0, BLOCK, name, inode, kind);
    block
}

/// Writes an entry of `len` bytes at `pos`, zeroing the room it covers.
fn write_entry(block: &mut [u8], pos: usize, len: usize, name: &[u8], inode: u32, kind: u8) {
    block[pos..pos + len].fill(0);
    put_u32(block, pos, inode);
    put_u16(block, pos + 4, len as u16);
    put_u16(block, pos + 6, name.len() as u16);
    block[pos + 8] = kind;
    block[pos + HEADER..pos + HEADER + name.len()].copy_from_slice(name);
}

/// Adds a name to the block if there is room. Returns `Ok(false)`, changing nothing, if there
/// is none. `name` is at most 1020 bytes, so a new entry always fits in an empty block.
///
/// The name goes into the first entry with enough spare room after its own name (or an
/// unused entry), splitting it: the old entry shrinks to what it needs and the new one takes
/// the rest.
pub fn try_insert(block: &mut [u8], name: &[u8], inode: u32, kind: u8) -> Result<bool, FsError> {
    if u16_at(block, 4) == 0 && all_zero(block) {
        put_u16(block, 4, BLOCK as u16); // a never-filled block is one unused entry
    }
    let need = align8(HEADER + name.len());
    let mut pos = 0;
    while pos < BLOCK {
        let e = entry_at(block, pos)?;
        let used = e.used();
        if e.len - used >= need {
            let (at, len) = if e.inode != 0 {
                put_u16(block, pos + 4, used as u16);
                (pos + used, e.len - used)
            } else {
                (pos, e.len)
            };
            write_entry(block, at, len, name, inode, kind);
            return Ok(true);
        }
        pos += e.len;
    }
    Ok(false)
}

/// Removes the entry at `pos` by marking it unused, and joins its room to the entry before it
/// in the block if there is one (ROCKFS.md allows this, and it keeps blocks from filling up
/// with tiny gaps).
pub fn remove_at(block: &mut [u8], pos: usize) -> Result<(), FsError> {
    let mut at = 0;
    let mut prev: Option<Entry> = None;
    while at < BLOCK {
        let e = entry_at(block, at)?;
        if at == pos {
            block[pos..pos + e.len].fill(0);
            match prev {
                Some(p) => put_u16(block, p.pos + 4, (p.len + e.len) as u16),
                None => put_u16(block, pos + 4, e.len as u16),
            }
            return Ok(());
        }
        prev = Some(e);
        at += e.len;
    }
    Err(FsError::Damaged)
}

/// Replaces the entry at `pos` by one with a different name, inode and type, in this block
/// image, if the new entry fits. Returns `Ok(false)`, changing nothing, if it does not.
///
/// The new entry takes the old one's room when that is enough; otherwise the old entry is
/// removed and the name is added anywhere else in the block, so the block's new image is
/// complete either way and one block write makes the replacement.
pub fn replace_at(
    block: &mut [u8],
    pos: usize,
    name: &[u8],
    inode: u32,
    kind: u8,
) -> Result<bool, FsError> {
    let old = entry_at(block, pos)?;
    if align8(HEADER + name.len()) <= old.len {
        write_entry(block, pos, old.len, name, inode, kind);
        return Ok(true);
    }
    let mut trial = [0u8; BLOCK];
    trial.copy_from_slice(block);
    remove_at(&mut trial, pos)?;
    if try_insert(&mut trial, name, inode, kind)? {
        block.copy_from_slice(&trial);
        return Ok(true);
    }
    Ok(false)
}

/// Makes the entry at `pos` name `inode` (of type `kind`) instead, keeping its name. This is
/// how a rename replaces an existing file in one step.
pub fn repoint(block: &mut [u8], pos: usize, inode: u32, kind: u8) {
    put_u32(block, pos, inode);
    block[pos + 8] = kind;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn names(block: &[u8]) -> Vec<(Vec<u8>, u32)> {
        Entries::new(block)
            .map(|e| e.unwrap())
            .filter(|e| e.inode != 0)
            .map(|e| (e.name(block).to_vec(), e.inode))
            .collect()
    }

    #[test]
    fn insert_fill_and_remove() {
        let mut block = new_block(b"first", 5, 1);
        assert_eq!(names(&block[..]), [(b"first".to_vec(), 5)]);
        let mut n = 0;
        // Fill the block until a name no longer fits.
        loop {
            let name = alloc::format!("file number {n:04}");
            if !try_insert(&mut block[..], name.as_bytes(), 100 + n, 1).unwrap() {
                break;
            }
            n += 1;
        }
        assert!(n > 50);
        assert_eq!(names(&block[..]).len() as u32, n + 1);
        // Every entry still tiles the block exactly.
        let total: usize = Entries::new(&block[..]).map(|e| e.unwrap().len).sum();
        assert_eq!(total, BLOCK);
        // Removing a name makes room again, and the others survive.
        let e = Entries::new(&block[..]).map(|e| e.unwrap()).nth(3).unwrap();
        remove_at(&mut block[..], e.pos).unwrap();
        assert_eq!(names(&block[..]).len() as u32, n);
        assert!(try_insert(&mut block[..], b"replacement", 999, 2).unwrap());
        assert!(names(&block[..]).contains(&(b"replacement".to_vec(), 999)));
        let total: usize = Entries::new(&block[..]).map(|e| e.unwrap().len).sum();
        assert_eq!(total, BLOCK);
    }

    #[test]
    fn removing_the_first_entry_leaves_an_unused_one() {
        let mut block = new_block(b"a", 2, 1);
        remove_at(&mut block[..], 0).unwrap();
        assert!(names(&block[..]).is_empty());
        assert!(try_insert(&mut block[..], b"b", 3, 1).unwrap());
        assert_eq!(names(&block[..]), [(b"b".to_vec(), 3)]);
    }

    #[test]
    fn repointing_keeps_the_name() {
        let mut block = new_block(b"keep", 2, 1);
        repoint(&mut block[..], 0, 9, 2);
        let e = Entries::new(&block[..]).next().unwrap().unwrap();
        assert_eq!((e.inode, e.kind, e.name(&block[..])), (9, 2, &b"keep"[..]));
    }

    #[test]
    fn replacing_an_entry_changes_its_spelling() {
        let mut block = new_block(b"other", 2, 1);
        assert!(try_insert(&mut block[..], b"zzz", 3, 1).unwrap());
        assert!(replace_at(&mut block[..], 0, b"OTHER", 9, 1).unwrap());
        assert_eq!(names(&block[..]), [(b"OTHER".to_vec(), 9), (b"zzz".to_vec(), 3)]);
        // A longer name than the entry's room goes elsewhere in the block.
        let long = [b'x'; 200];
        assert!(replace_at(&mut block[..], 0, &long, 10, 1).unwrap());
        assert_eq!(names(&block[..]).len(), 2);
        assert!(names(&block[..]).contains(&(long.to_vec(), 10)));
        // A block too full to take it is left alone.
        let mut full = new_block(b"a", 2, 1);
        let mut n = 0;
        while try_insert(&mut full[..], &[b'q'; 100], 50 + n, 1).unwrap() {
            n += 1;
        }
        let copy = full.clone();
        assert!(!replace_at(&mut full[..], 0, &[b'z'; 300], 1, 1).unwrap());
        assert_eq!(full, copy);
    }

    #[test]
    fn a_blank_block_counts_as_empty_and_takes_names() {
        let mut block = [0u8; BLOCK];
        assert_eq!(Entries::new(&block).count(), 0);
        assert!(try_insert(&mut block, b"x", 4, 1).unwrap());
        assert_eq!(names(&block), [(b"x".to_vec(), 4)]);
    }

    #[test]
    fn damaged_blocks_are_errors_not_panics() {
        let good = new_block(b"name", 2, 1);
        let mut seed = 7u64;
        let mut rnd = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for _ in 0..3000 {
            let mut block = good.clone();
            for _ in 0..1 + rnd(4) {
                // Damage the header area, where the lengths are.
                block[rnd(24)] = rnd(256) as u8;
            }
            for e in Entries::new(&block[..]).flatten() {
                assert!(e.pos + e.len <= BLOCK);
            }
            let _ = try_insert(&mut block[..], b"another", 8, 1);
            let _ = remove_at(&mut block[..], 0);
        }
        // Specific shapes: a length that runs off the end, a length of zero, a name too long.
        for (len, nlen) in [(4096u16 + 8, 4u16), (0, 0), (8, 0), (16, 200), (20, 1)] {
            let mut block = [0u8; BLOCK];
            put_u32(&mut block, 0, 3);
            put_u16(&mut block, 4, len);
            put_u16(&mut block, 6, nlen);
            assert_eq!(Entries::new(&block).next().unwrap(), Err(FsError::Damaged), "{len}");
        }
    }
}
