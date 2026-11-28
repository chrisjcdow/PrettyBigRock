// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading the bytecode: bytes and integers, package lengths, and names (ACPI 6.5, section
//! 20.2).
//!
//! A table's bytes are kept whole, shared by every method in it: `Rc` is a pointer that
//! counts its owners and frees the bytes when the last one goes, so a method can keep its
//! body without copying it. A [`Code`] is a position in those bytes and an end it must not
//! pass; every read checks it, so bytecode that is cut short is an error, never a read past
//! the end.

use crate::{Error, Result};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A stretch of bytecode being read.
#[derive(Clone)]
pub(crate) struct Code {
    pub bytes: Rc<[u8]>,
    pub pos: usize,
    pub end: usize,
}

impl Code {
    /// All of `bytes`, from `pos` to `end` (which the caller has checked).
    pub fn new(bytes: Rc<[u8]>, pos: usize, end: usize) -> Code {
        let end = end.min(bytes.len());
        Code { bytes, pos: pos.min(end), end }
    }

    /// The same bytes, from here to `end`.
    pub fn until(&self, end: usize) -> Code {
        Code::new(self.bytes.clone(), self.pos, end)
    }

    /// True when nothing is left.
    pub fn done(&self) -> bool {
        self.pos >= self.end
    }

    /// The next byte, without moving past it.
    pub fn peek(&self) -> Result<u8> {
        self.at(0)
    }

    /// The byte `n` ahead.
    pub fn at(&self, n: usize) -> Result<u8> {
        let i = self
            .pos
            .checked_add(n)
            .filter(|&i| i < self.end)
            .ok_or(Error::Malformed("cut short"))?;
        Ok(self.bytes[i])
    }

    /// The next byte.
    pub fn byte(&mut self) -> Result<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    /// The next `n` bytes (up to 8) as a little-endian integer, as all of ACPI's are.
    pub fn int(&mut self, n: usize) -> Result<u64> {
        let mut v = 0u64;
        for i in 0..n {
            v |= u64::from(self.at(i)?) << (8 * i);
        }
        self.pos += n;
        Ok(v)
    }

    /// Moves past `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        let to = self.pos.checked_add(n).filter(|&to| to <= self.end);
        self.pos = to.ok_or(Error::Malformed("cut short"))?;
        Ok(())
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&[u8]> {
        let from = self.pos;
        self.skip(n)?;
        Ok(&self.bytes[from..self.pos])
    }

    /// A package length (section 20.2.4): where the object it starts ends. The length counts
    /// itself, so the end is measured from where it starts. One to four bytes: the first's top
    /// two bits say how many follow; alone, its low six bits are the length, otherwise its low
    /// four bits are the bottom of it and each following byte adds eight more.
    pub fn pkg_length(&mut self) -> Result<usize> {
        let start = self.pos;
        let lead = self.byte()?;
        let follow = usize::from(lead >> 6);
        let length = if follow == 0 {
            usize::from(lead & 0x3F)
        } else {
            let mut length = usize::from(lead & 0x0F);
            for i in 0..follow {
                length |= usize::from(self.byte()?) << (4 + 8 * i);
            }
            length
        };
        let end = start.checked_add(length).ok_or(Error::Malformed("package length"))?;
        if end > self.end || end < self.pos {
            return Err(Error::Malformed("package length runs past its end"));
        }
        Ok(end)
    }

    /// A null-terminated string of ASCII characters (`StringPrefix`'s, and `Name`'s strings).
    pub fn ascii(&mut self) -> Result<String> {
        let mut s = String::new();
        loop {
            match self.byte()? {
                0 => return Ok(s),
                b @ 0x01..=0x7F => s.push(char::from(b)),
                _ => return Err(Error::Malformed("a string's character isn't ASCII")),
            }
        }
    }

    /// A name (section 20.2.2): an optional `\` (from the root) or `^`s (up a level each),
    /// then no segments (`NullName`), one, two (`DualNamePrefix`) or many (`MultiNamePrefix`
    /// and a count).
    pub fn name(&mut self) -> Result<AmlName> {
        let mut name = AmlName { absolute: false, parents: 0, segs: Vec::new() };
        if self.peek()? == b'\\' {
            self.pos += 1;
            name.absolute = true;
        } else {
            while self.peek()? == b'^' {
                self.pos += 1;
                name.parents = name.parents.checked_add(1).ok_or(Error::Malformed("name"))?;
            }
        }
        let count = match self.peek()? {
            0x00 => {
                self.pos += 1;
                0
            }
            0x2E => {
                self.pos += 1;
                2
            }
            0x2F => {
                self.pos += 1;
                usize::from(self.byte()?)
            }
            _ => 1,
        };
        for _ in 0..count {
            let s = self.take(4)?;
            let seg = [s[0], s[1], s[2], s[3]];
            if !is_lead(seg[0]) || !seg[1..].iter().all(|&b| is_lead(b) || b.is_ascii_digit()) {
                return Err(Error::Malformed("a name segment's characters"));
            }
            name.segs.push(seg);
        }
        Ok(name)
    }
}

/// True for the characters a name segment may start with.
pub(crate) fn is_lead(b: u8) -> bool {
    b.is_ascii_uppercase() || b == b'_'
}

/// True if a name starts here: a lead character or one of the prefixes.
pub(crate) fn starts_name(b: u8) -> bool {
    is_lead(b) || matches!(b, b'\\' | b'^' | 0x2E | 0x2F)
}

/// A name as the bytecode writes it, before it is looked up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmlName {
    /// It starts at the root (`\`).
    pub absolute: bool,
    /// How many `^`s: levels up from the current scope.
    pub parents: u8,
    /// Its four-character segments.
    pub segs: Vec<[u8; 4]>,
}

impl AmlName {
    /// Reads a name written as text, such as `\_SB.PCI0` or `^^FOO`; short segments are padded
    /// with `_`, as ASL does. `None` if it isn't one.
    pub fn parse(text: &str) -> Option<AmlName> {
        let mut rest = text.as_bytes();
        let mut name = AmlName { absolute: false, parents: 0, segs: Vec::new() };
        if let [b'\\', r @ ..] = rest {
            name.absolute = true;
            rest = r;
        }
        while let [b'^', r @ ..] = rest {
            name.parents = name.parents.checked_add(1)?;
            rest = r;
        }
        if rest.is_empty() {
            return Some(name);
        }
        for part in rest.split(|&b| b == b'.') {
            if part.is_empty() || part.len() > 4 || !is_lead(part[0]) {
                return None;
            }
            let mut seg = [b'_'; 4];
            for (i, &b) in part.iter().enumerate() {
                if !(is_lead(b) || b.is_ascii_digit()) {
                    return None;
                }
                seg[i] = b;
            }
            name.segs.push(seg);
        }
        Some(name)
    }

    /// True for a single segment with no prefix: the kind the search rules apply to (a name
    /// not found in the current scope is looked for in each scope above it).
    pub fn searched(&self) -> bool {
        !self.absolute && self.parents == 0 && self.segs.len() == 1
    }
}

impl fmt::Display for AmlName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.absolute {
            f.write_str("\\")?;
        }
        for _ in 0..self.parents {
            f.write_str("^")?;
        }
        for (i, seg) in self.segs.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write_seg(f, seg)?;
        }
        Ok(())
    }
}

/// Writes one segment, as ASL does: trailing `_`s dropped (`_SB_` is `_SB`).
pub(crate) fn write_seg(f: &mut impl fmt::Write, seg: &[u8; 4]) -> fmt::Result {
    let len = 4 - seg.iter().rev().take(3).take_while(|&&b| b == b'_').count();
    for &b in &seg[..len] {
        f.write_char(char::from(b))?;
    }
    Ok(())
}
