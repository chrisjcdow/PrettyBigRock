// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading ELF files: the kernel, for the loaders (note 0004), and programs, for the kernel
//! and `granite`'s program loader (docs/design/0009-programs-and-granite.md, section 2).
//!
//! Everything is checked before anything is used: a file is attacker-controlled data, so
//! every offset and size is bounds-checked with overflow-checked arithmetic, and a file that
//! breaks any rule is refused with the reason.

#![cfg_attr(not(test), no_std)]

#[cfg(any(feature = "alloc", test))]
extern crate alloc;

#[cfg(any(feature = "alloc", test))]
use alloc::vec::Vec;
use core::fmt;

/// The page size every PrettyBigRock architecture uses.
pub const PAGE: u64 = 4096;

/// ELF machine number for x86-64.
pub const EM_X86_64: u16 = 62;
/// ELF machine number for AArch64.
pub const EM_AARCH64: u16 = 183;

/// The segment may be executed.
pub const PF_X: u32 = 1;
/// The segment may be written.
pub const PF_W: u32 = 2;
/// The segment may be read.
pub const PF_R: u32 = 4;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_TLS: u32 = 7;
const PT_GNU_STACK: u32 = 0x6474_E551;

const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_PLTRELSZ: u64 = 2;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_JMPREL: u64 = 23;
const DT_RELR: u64 = 36;

const R_X86_64_RELATIVE: u32 = 8;
const R_AARCH64_RELATIVE: u32 = 1027;

/// The owner name of PrettyBigRock's ELF note.
pub const NOTE_OWNER: &[u8] = b"PrettyBigRock";
/// Note type: the interface version a program needs (`u16` major, `u16` minor).
pub const NOTE_VERSION: u32 = 1;
/// Note type: the main thread's stack size in bytes (`u64`).
pub const NOTE_STACK: u32 = 2;

/// The default stack size for a program's main thread.
pub const DEFAULT_STACK: u64 = 256 * 1024;
/// The largest span a program's segments may cover.
pub const MAX_SPAN: u64 = 1 << 30;

/// Why a file was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// Not a 64-bit little-endian ELF file of the expected type.
    NotElf,
    /// Built for another processor.
    WrongMachine(u16),
    /// Something points past the end of the file.
    Truncated,
    /// The segments aren't page-aligned, overlap, are out of order, or there are none.
    BadSegments,
    /// A segment is both writable and executable.
    WritableAndExecutable,
    /// The program needs a dynamic linker or shared libraries.
    Dynamic,
    /// It uses thread-local storage, which isn't supported yet.
    ThreadLocalStorage,
    /// Its stack is marked executable, or not marked at all.
    ExecutableStack,
    /// A relocation of a kind PrettyBigRock doesn't do, or one aimed outside the program.
    BadRelocation,
    /// The PrettyBigRock note is missing or malformed.
    NoNote,
    /// The entry point isn't in an executable segment.
    BadEntry,
    /// The segments span more than 1 GiB.
    TooBig,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ElfError::NotElf => "not a program file of the right kind",
            ElfError::WrongMachine(_) => "built for a different processor",
            ElfError::Truncated => "the file is cut short",
            ElfError::BadSegments => "its segments are laid out wrongly",
            ElfError::WritableAndExecutable => "a segment is both writable and executable",
            ElfError::Dynamic => "it needs shared libraries, which aren't supported yet",
            ElfError::ThreadLocalStorage => "it uses thread-local storage, not supported yet",
            ElfError::ExecutableStack => "its stack isn't marked non-executable",
            ElfError::BadRelocation => "it has a relocation PrettyBigRock can't apply",
            ElfError::NoNote => "it has no valid PrettyBigRock note",
            ElfError::BadEntry => "its entry point isn't in its code",
            ElfError::TooBig => "it's bigger than 1 GiB",
        })
    }
}

fn u16_at(b: &[u8], o: u64) -> Option<u16> {
    let o = usize::try_from(o).ok()?;
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: u64) -> Option<u32> {
    let o = usize::try_from(o).ok()?;
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: u64) -> Option<u64> {
    let o = usize::try_from(o).ok()?;
    Some(u64::from_le_bytes(b.get(o..o.checked_add(8)?)?.try_into().ok()?))
}

/// A loadable segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Where it goes (for a program, relative to where the program is loaded).
    pub vaddr: u64,
    /// How much memory it takes.
    pub mem_len: u64,
    /// Where its contents start in the file.
    pub file_offset: u64,
    /// How many bytes come from the file (the rest is zero).
    pub file_len: u64,
    /// `PF_R`, `PF_W` and `PF_X`.
    pub flags: u32,
}

impl Segment {
    /// True if it may be written.
    pub fn writable(&self) -> bool {
        self.flags & PF_W != 0
    }
    /// True if it may be executed.
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }
    /// Its pages: (first byte, one past the last), page-aligned.
    pub fn pages(&self) -> (u64, u64) {
        (self.vaddr & !(PAGE - 1), (self.vaddr + self.mem_len).next_multiple_of(PAGE))
    }
}

/// A program header.
#[derive(Clone, Copy)]
struct Header {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    file_len: u64,
    mem_len: u64,
}

/// The parts of an ELF file common to kernels and programs.
#[derive(Clone, Copy)]
struct Elf<'a> {
    file: &'a [u8],
    entry: u64,
    phoff: u64,
    phentsize: u64,
    phnum: u64,
}

impl<'a> Elf<'a> {
    fn parse(file: &'a [u8], machine: u16, kind: u16) -> Result<Self, ElfError> {
        if file.len() < 64 || &file[..4] != b"\x7fELF" || file[4] != 2 || file[5] != 1 {
            return Err(ElfError::NotElf);
        }
        if u16_at(file, 16) != Some(kind) {
            return Err(ElfError::NotElf);
        }
        let m = u16_at(file, 18).ok_or(ElfError::Truncated)?;
        if m != machine {
            return Err(ElfError::WrongMachine(m));
        }
        let e = Elf {
            file,
            entry: u64_at(file, 24).ok_or(ElfError::Truncated)?,
            phoff: u64_at(file, 32).ok_or(ElfError::Truncated)?,
            phentsize: u64::from(u16_at(file, 54).ok_or(ElfError::Truncated)?),
            phnum: u64::from(u16_at(file, 56).ok_or(ElfError::Truncated)?),
        };
        if e.phentsize < 56 {
            return Err(ElfError::Truncated);
        }
        // Every program header must be inside the file.
        let table = e.phentsize.checked_mul(e.phnum).and_then(|t| t.checked_add(e.phoff));
        if table.is_none_or(|end| end > file.len() as u64) {
            return Err(ElfError::Truncated);
        }
        Ok(e)
    }

    fn header(&self, i: u64) -> Header {
        let o = self.phoff + i * self.phentsize;
        // Bounds were checked in `parse`.
        let f = self.file;
        Header {
            kind: u32_at(f, o).unwrap_or(0),
            flags: u32_at(f, o + 4).unwrap_or(0),
            offset: u64_at(f, o + 8).unwrap_or(0),
            vaddr: u64_at(f, o + 16).unwrap_or(0),
            file_len: u64_at(f, o + 32).unwrap_or(0),
            mem_len: u64_at(f, o + 40).unwrap_or(0),
        }
    }

    fn headers(&self) -> impl Iterator<Item = Header> + '_ {
        (0..self.phnum).map(|i| self.header(i))
    }

    /// Checks the loadable segments: ordered, not overlapping, not W+X, inside the file.
    /// `whole_pages` also demands page-aligned file offsets (programs are mapped page by page).
    fn check_segments(&self, whole_pages: bool) -> Result<usize, ElfError> {
        let mut count = 0;
        let mut prev_end = 0u64;
        for h in self.headers().filter(|h| h.kind == PT_LOAD) {
            count += 1;
            if !h.vaddr.is_multiple_of(PAGE)
                || (whole_pages && !h.offset.is_multiple_of(PAGE))
                || h.file_len > h.mem_len
                || h.vaddr < prev_end
            {
                return Err(ElfError::BadSegments);
            }
            if h.flags & PF_W != 0 && h.flags & PF_X != 0 {
                return Err(ElfError::WritableAndExecutable);
            }
            let fend = h.offset.checked_add(h.file_len).ok_or(ElfError::Truncated)?;
            if fend > self.file.len() as u64 {
                return Err(ElfError::Truncated);
            }
            prev_end = h.vaddr.checked_add(h.mem_len).ok_or(ElfError::BadSegments)?;
        }
        if count == 0 {
            return Err(ElfError::BadSegments);
        }
        Ok(count)
    }

    fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.headers().filter(|h| h.kind == PT_LOAD).map(|h| Segment {
            vaddr: h.vaddr,
            mem_len: h.mem_len,
            file_offset: h.offset,
            file_len: h.file_len,
            flags: h.flags,
        })
    }

    fn span(&self) -> (u64, u64) {
        let mut lo = u64::MAX;
        let mut hi = 0;
        for s in self.segments() {
            let (a, b) = s.pages();
            lo = lo.min(a);
            hi = hi.max(b);
        }
        (lo, hi)
    }

    /// The segments' contents laid out over their span, zero-filled.
    fn load_into(&self, dest: &mut [u8]) -> Result<(), ElfError> {
        let (lo, hi) = self.span();
        if dest.len() as u64 != hi - lo {
            return Err(ElfError::BadSegments);
        }
        dest.fill(0);
        for s in self.segments() {
            let at = (s.vaddr - lo) as usize;
            let src = &self.file[s.file_offset as usize..(s.file_offset + s.file_len) as usize];
            dest[at..at + src.len()].copy_from_slice(src);
        }
        Ok(())
    }
}

/// A kernel file (a fixed-address executable), checked.
pub struct Kernel<'a> {
    elf: Elf<'a>,
    /// The entry point's virtual address.
    pub entry: u64,
}

impl<'a> Kernel<'a> {
    /// Checks the kernel file and finds its segments.
    pub fn parse(file: &'a [u8], machine: u16) -> Result<Self, ElfError> {
        let elf = Elf::parse(file, machine, ET_EXEC)?;
        elf.check_segments(false)?;
        Ok(Kernel { elf, entry: elf.entry })
    }

    /// The loadable segments.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.elf.segments()
    }

    /// The page-aligned range of virtual addresses the segments cover.
    pub fn span(&self) -> (u64, u64) {
        self.elf.span()
    }

    /// Copies the segments into `dest`, which covers [`Kernel::span`] exactly, zero-filling
    /// the rest.
    pub fn load_into(&self, dest: &mut [u8]) -> Result<(), ElfError> {
        self.elf.load_into(dest)
    }
}

/// A program file, checked against every rule of note 0009, section 2.
pub struct Program<'a> {
    elf: Elf<'a>,
    /// The entry point, relative to where the program is loaded.
    pub entry: u64,
    /// The interface version it needs: (major, minor).
    pub needs_version: (u16, u16),
    /// Its main thread's stack size, in bytes (a whole number of pages).
    pub stack_size: u64,
    /// Where its relocations are in the file, and how many there are.
    relocs: (u64, u64),
    reloc_type: u32,
}

impl<'a> Program<'a> {
    /// Checks a program file for `machine`.
    pub fn parse(file: &'a [u8], machine: u16) -> Result<Self, ElfError> {
        let elf = Elf::parse(file, machine, ET_DYN)?;
        elf.check_segments(true)?;
        let (lo, hi) = elf.span();
        if hi - lo > MAX_SPAN {
            return Err(ElfError::TooBig);
        }
        let mut stack_marked = false;
        let mut dynamic = None;
        let mut note = None;
        for h in elf.headers() {
            match h.kind {
                PT_INTERP => return Err(ElfError::Dynamic),
                PT_TLS => return Err(ElfError::ThreadLocalStorage),
                PT_GNU_STACK => {
                    if h.flags & PF_X != 0 {
                        return Err(ElfError::ExecutableStack);
                    }
                    stack_marked = true;
                }
                PT_DYNAMIC => dynamic = Some(h),
                PT_NOTE => {
                    if let Some(n) = find_note(file, h)? {
                        note = Some(n);
                    }
                }
                _ => {}
            }
        }
        if !stack_marked {
            return Err(ElfError::ExecutableStack);
        }
        let (needs_version, stack_size) = note.ok_or(ElfError::NoNote)?;
        let entry = elf.entry;
        if !elf
            .segments()
            .any(|s| s.executable() && entry >= s.vaddr && entry < s.vaddr + s.mem_len)
        {
            return Err(ElfError::BadEntry);
        }
        let reloc_type = if machine == EM_X86_64 { R_X86_64_RELATIVE } else { R_AARCH64_RELATIVE };
        let mut p = Program { elf, entry, needs_version, stack_size, relocs: (0, 0), reloc_type };
        if let Some(d) = dynamic {
            p.relocs = p.read_dynamic(d)?;
        }
        // Every relocation must be the one kind we apply, and land inside the program.
        for i in 0..p.relocs.1 {
            p.relocation(i)?;
        }
        Ok(p)
    }

    /// Converts a virtual address in the file's terms to a file offset, if `len` bytes there
    /// come from the file.
    fn file_offset(&self, vaddr: u64, len: u64) -> Option<u64> {
        let end = vaddr.checked_add(len)?;
        self.elf
            .segments()
            .find(|s| vaddr >= s.vaddr && end <= s.vaddr + s.file_len)
            .map(|s| s.file_offset + (vaddr - s.vaddr))
    }

    fn read_dynamic(&self, d: Header) -> Result<(u64, u64), ElfError> {
        let f = self.elf.file;
        let end = d.offset.checked_add(d.file_len).ok_or(ElfError::Truncated)?;
        if end > f.len() as u64 {
            return Err(ElfError::Truncated);
        }
        let (mut rela, mut relasz, mut relaent) = (None, 0u64, 24u64);
        let mut o = d.offset;
        while o + 16 <= end {
            let tag = u64_at(f, o).ok_or(ElfError::Truncated)?;
            let val = u64_at(f, o + 8).ok_or(ElfError::Truncated)?;
            match tag {
                DT_NULL => break,
                DT_NEEDED => return Err(ElfError::Dynamic),
                DT_RELA => rela = Some(val),
                DT_RELASZ => relasz = val,
                DT_RELAENT => relaent = val,
                DT_REL | DT_RELSZ | DT_RELR if val != 0 => return Err(ElfError::BadRelocation),
                DT_JMPREL | DT_PLTRELSZ if val != 0 => return Err(ElfError::Dynamic),
                _ => {}
            }
            o += 16;
        }
        let Some(rela) = rela else {
            return if relasz == 0 { Ok((0, 0)) } else { Err(ElfError::BadRelocation) };
        };
        if relaent != 24 || !relasz.is_multiple_of(24) {
            return Err(ElfError::BadRelocation);
        }
        let at = self.file_offset(rela, relasz).ok_or(ElfError::BadRelocation)?;
        Ok((at, relasz / 24))
    }

    /// Relocation `i`: (where, relative to the load address; the value to add the load
    /// address to).
    fn relocation(&self, i: u64) -> Result<(u64, u64), ElfError> {
        let f = self.elf.file;
        let o = self.relocs.0 + i * 24;
        let offset = u64_at(f, o).ok_or(ElfError::Truncated)?;
        let info = u64_at(f, o + 8).ok_or(ElfError::Truncated)?;
        let addend = u64_at(f, o + 16).ok_or(ElfError::Truncated)?;
        if info as u32 != self.reloc_type || info >> 32 != 0 || !offset.is_multiple_of(8) {
            return Err(ElfError::BadRelocation);
        }
        let inside = self.elf.segments().any(|s| {
            offset >= s.vaddr && offset.checked_add(8).is_some_and(|e| e <= s.vaddr + s.mem_len)
        });
        if !inside {
            return Err(ElfError::BadRelocation);
        }
        Ok((offset, addend))
    }

    /// The loadable segments, with addresses relative to where the program is loaded.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.elf.segments()
    }

    /// The page-aligned range the segments cover, relative to the load address.
    pub fn span(&self) -> (u64, u64) {
        self.elf.span()
    }

    /// The number of relocations.
    pub fn relocation_count(&self) -> u64 {
        self.relocs.1
    }

    #[cfg(any(feature = "alloc", test))]
    /// The bytes to put at the start of segment `seg` when the program is loaded at `base`:
    /// its contents from the file, with every relocation inside it applied, extended with
    /// zeros where a relocation lands past the file's part. The rest of the segment's memory
    /// is zero.
    pub fn segment_data(&self, seg: &Segment, base: u64) -> Result<Vec<u8>, ElfError> {
        let f = self.elf.file;
        let mut len = seg.file_len;
        let in_seg = |off: u64| off >= seg.vaddr && off + 8 <= seg.vaddr + seg.mem_len;
        for i in 0..self.relocs.1 {
            let (at, _) = self.relocation(i)?;
            if in_seg(at) {
                len = len.max(at + 8 - seg.vaddr);
            }
        }
        let mut out = Vec::with_capacity(len as usize);
        out.extend_from_slice(
            f.get(seg.file_offset as usize..(seg.file_offset + seg.file_len) as usize)
                .ok_or(ElfError::Truncated)?,
        );
        out.resize(len as usize, 0);
        for i in 0..self.relocs.1 {
            let (at, addend) = self.relocation(i)?;
            if in_seg(at) {
                let p = (at - seg.vaddr) as usize;
                out[p..p + 8].copy_from_slice(&base.wrapping_add(addend).to_le_bytes());
            }
        }
        Ok(out)
    }

    #[cfg(any(feature = "alloc", test))]
    /// The program's whole memory image for loading at `base`: the bytes of
    /// [`Program::span`], with every relocation applied. (For small programs and tests; a
    /// loader uses [`Program::segment_data`].)
    pub fn image(&self, base: u64) -> Result<Vec<u8>, ElfError> {
        let (lo, hi) = self.span();
        let mut img = alloc::vec![0u8; (hi - lo) as usize];
        self.elf.load_into(&mut img)?;
        for i in 0..self.relocs.1 {
            let (at, addend) = self.relocation(i)?;
            let p = (at - lo) as usize;
            img[p..p + 8].copy_from_slice(&base.wrapping_add(addend).to_le_bytes());
        }
        Ok(img)
    }
}

/// Reads the PrettyBigRock note from a `PT_NOTE` segment, if it has one:
/// (version needed, stack size).
/// What a PrettyBigRock note says: (interface version needed, stack size).
type NoteInfo = ((u16, u16), u64);

fn find_note(f: &[u8], h: Header) -> Result<Option<NoteInfo>, ElfError> {
    let end = h.offset.checked_add(h.file_len).ok_or(ElfError::Truncated)?;
    if end > f.len() as u64 {
        return Err(ElfError::Truncated);
    }
    let mut version = None;
    let mut stack = DEFAULT_STACK;
    let mut o = h.offset;
    let pad = |n: u64| n.next_multiple_of(4);
    while o + 12 <= end {
        let namesz = u64::from(u32_at(f, o).ok_or(ElfError::Truncated)?);
        let descsz = u64::from(u32_at(f, o + 4).ok_or(ElfError::Truncated)?);
        let kind = u32_at(f, o + 8).ok_or(ElfError::Truncated)?;
        let name_at = o + 12;
        let desc_at = name_at.checked_add(pad(namesz)).ok_or(ElfError::NoNote)?;
        let next = desc_at.checked_add(pad(descsz)).ok_or(ElfError::NoNote)?;
        if next > end {
            return Err(ElfError::NoNote);
        }
        let name = &f[name_at as usize..(name_at + namesz) as usize];
        let ours = name.strip_suffix(b"\0").unwrap_or(name) == NOTE_OWNER;
        if ours {
            match (kind, descsz) {
                (NOTE_VERSION, 4) => {
                    let major = u16_at(f, desc_at).ok_or(ElfError::Truncated)?;
                    let minor = u16_at(f, desc_at + 2).ok_or(ElfError::Truncated)?;
                    version = Some((major, minor));
                }
                (NOTE_STACK, 8) => {
                    stack = u64_at(f, desc_at).ok_or(ElfError::Truncated)?;
                    if stack == 0 || stack > MAX_SPAN || !stack.is_multiple_of(PAGE) {
                        return Err(ElfError::NoNote);
                    }
                }
                (NOTE_VERSION | NOTE_STACK, _) => return Err(ElfError::NoNote),
                _ => {} // later note types are ignored
            }
        }
        o = next;
    }
    Ok(version.map(|v| (v, stack)))
}

/// Builds small ELF files for tests, here and in the crates that use this one.
#[cfg(any(feature = "alloc", test))]
pub mod testing {
    use super::*;

    /// A program header for [`make`]: (type, flags, vaddr, contents, mem_len).
    pub type Ph<'a> = (u32, u32, u64, &'a [u8], u64);

    /// Builds an ELF file of type `kind` (2 executable, 3 position-independent) with these
    /// program headers. Each header's contents start on a page boundary in the file.
    pub fn make(kind: u16, machine: u16, entry: u64, phs: &[Ph<'_>]) -> Vec<u8> {
        let mut f = alloc::vec![0u8; 64 + 56 * phs.len()];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[6] = 1;
        f[16..18].copy_from_slice(&kind.to_le_bytes());
        f[18..20].copy_from_slice(&machine.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
        for (i, (t, flags, vaddr, data, mem)) in phs.iter().enumerate() {
            f.resize(f.len().next_multiple_of(PAGE as usize), 0);
            let off = f.len() as u64;
            f.extend_from_slice(data);
            let p = 64 + 56 * i;
            f[p..p + 4].copy_from_slice(&t.to_le_bytes());
            f[p + 4..p + 8].copy_from_slice(&flags.to_le_bytes());
            f[p + 8..p + 16].copy_from_slice(&off.to_le_bytes());
            f[p + 16..p + 24].copy_from_slice(&vaddr.to_le_bytes());
            f[p + 32..p + 40].copy_from_slice(&(data.len() as u64).to_le_bytes());
            f[p + 40..p + 48].copy_from_slice(&mem.to_le_bytes());
        }
        f
    }

    /// A PrettyBigRock note asking for interface `version`, and optionally a stack size.
    pub fn note(version: (u16, u16), stack: Option<u64>) -> Vec<u8> {
        let mut n = Vec::new();
        let mut one = |kind: u32, desc: &[u8]| {
            n.extend_from_slice(&14u32.to_le_bytes());
            n.extend_from_slice(&(desc.len() as u32).to_le_bytes());
            n.extend_from_slice(&kind.to_le_bytes());
            n.extend_from_slice(b"PrettyBigRock\0\0\0");
            n.extend_from_slice(desc);
        };
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&version.0.to_le_bytes());
        v[2..].copy_from_slice(&version.1.to_le_bytes());
        one(NOTE_VERSION, &v);
        if let Some(s) = stack {
            one(NOTE_STACK, &s.to_le_bytes());
        }
        n
    }

    /// `PT_DYNAMIC` contents pointing at relocations at `rela` (`count` entries).
    pub fn dynamic(rela: u64, count: u64) -> Vec<u8> {
        let mut d = Vec::new();
        for (t, v) in [(DT_RELA, rela), (DT_RELASZ, count * 24), (DT_RELAENT, 24), (DT_NULL, 0)] {
            d.extend_from_slice(&t.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes());
        }
        d
    }

    /// Relocation entries of kind `kind`: (offset, addend) each.
    pub fn relas(kind: u32, list: &[(u64, u64)]) -> Vec<u8> {
        let mut r = Vec::new();
        for (o, a) in list {
            r.extend_from_slice(&o.to_le_bytes());
            r.extend_from_slice(&u64::from(kind).to_le_bytes());
            r.extend_from_slice(&a.to_le_bytes());
        }
        r
    }

    /// The x86-64 relative relocation number.
    pub const RELATIVE_X86_64: u32 = R_X86_64_RELATIVE;
    /// The AArch64 relative relocation number.
    pub const RELATIVE_AARCH64: u32 = R_AARCH64_RELATIVE;
    /// `PT_LOAD`.
    pub const LOAD: u32 = PT_LOAD;
    /// `PT_DYNAMIC`.
    pub const DYNAMIC: u32 = PT_DYNAMIC;
    /// `PT_NOTE`.
    pub const NOTE: u32 = PT_NOTE;
    /// `PT_GNU_STACK`.
    pub const GNU_STACK: u32 = PT_GNU_STACK;
    /// `PT_TLS`.
    pub const TLS: u32 = PT_TLS;
    /// `PT_INTERP`.
    pub const INTERP: u32 = PT_INTERP;
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const B: u64 = 0xFFFF_FFFF_8000_0000;

    #[test]
    fn loads_kernel_segments() {
        let f = make(
            2,
            EM_X86_64,
            B,
            &[(LOAD, PF_R | PF_X, B, b"code", 4), (LOAD, PF_R | PF_W, B + 0x2000, b"data", 0x1800)],
        );
        let k = Kernel::parse(&f, EM_X86_64).unwrap();
        assert_eq!(k.entry, B);
        assert_eq!(k.span(), (B, B + 0x4000));
        let mut mem = vec![0xAAu8; 0x4000];
        k.load_into(&mut mem).unwrap();
        assert_eq!(&mem[..4], b"code");
        assert_eq!(mem[4], 0);
        assert_eq!(&mem[0x2000..0x2004], b"data");
        assert!(mem[0x2004..].iter().all(|&b| b == 0));
    }

    #[test]
    fn rejects_bad_kernels() {
        let k = |f: &[u8]| Kernel::parse(f, EM_X86_64).err();
        assert_eq!(k(b"nope"), Some(ElfError::NotElf));
        let f = make(2, EM_AARCH64, B, &[(LOAD, PF_R | PF_X, B, b"x", 1)]);
        assert_eq!(k(&f), Some(ElfError::WrongMachine(EM_AARCH64)));
        let f = make(2, EM_X86_64, B, &[(LOAD, PF_R | PF_W | PF_X, B, b"x", 1)]);
        assert_eq!(k(&f), Some(ElfError::WritableAndExecutable));
        let f = make(2, EM_X86_64, B, &[(LOAD, PF_R, B + 1, b"x", 1)]);
        assert_eq!(k(&f), Some(ElfError::BadSegments));
        let f =
            make(2, EM_X86_64, B, &[(LOAD, PF_R, B + 0x1000, b"x", 1), (LOAD, PF_R, B, b"y", 1)]);
        assert_eq!(k(&f), Some(ElfError::BadSegments));
        let mut f = make(2, EM_X86_64, B, &[(LOAD, PF_R, B, b"xyz", 3)]);
        let n = f.len();
        f.truncate(n - 1);
        assert_eq!(k(&f), Some(ElfError::Truncated));
        // A program header table running off the end.
        let mut f = make(2, EM_X86_64, B, &[(LOAD, PF_R, B, b"x", 1)]);
        f[56..58].copy_from_slice(&500u16.to_le_bytes());
        assert_eq!(k(&f), Some(ElfError::Truncated));
    }

    /// A small valid program: code at 0, data at 0x2000 holding one pointer to relocate,
    /// relocations in a read-only segment at 0x4000, and its note.
    fn program(machine: u16) -> Vec<u8> {
        let kind = if machine == EM_X86_64 { RELATIVE_X86_64 } else { RELATIVE_AARCH64 };
        let rel = relas(kind, &[(0x2008, 0x10)]);
        let dynm = dynamic(0x4000, 1);
        let n = note((1, 0), Some(64 * 1024));
        make(
            3,
            machine,
            0x10,
            &[
                (LOAD, PF_R | PF_X, 0, &[0x90; 32], 32),
                (LOAD, PF_R | PF_W, 0x2000, &[0; 16], 0x100),
                (LOAD, PF_R, 0x4000, &rel, rel.len() as u64),
                (LOAD, PF_R, 0x5000, &dynm, dynm.len() as u64),
                (DYNAMIC, PF_R, 0x5000, &dynm, dynm.len() as u64),
                (NOTE, PF_R, 0x6000, &n, n.len() as u64),
                (GNU_STACK, PF_R | PF_W, 0, &[], 0),
            ],
        )
    }

    #[test]
    fn loads_programs() {
        for m in [EM_X86_64, EM_AARCH64] {
            let f = program(m);
            let p = Program::parse(&f, m).unwrap();
            assert_eq!(p.entry, 0x10);
            assert_eq!(p.needs_version, (1, 0));
            assert_eq!(p.stack_size, 64 * 1024);
            assert_eq!(p.relocation_count(), 1);
            assert_eq!(p.span(), (0, 0x6000));
            let img = p.image(0x4000_0000).unwrap();
            assert_eq!(img.len(), 0x6000);
            assert_eq!(&img[..32], &[0x90; 32]);
            assert_eq!(u64::from_le_bytes(img[0x2008..0x2010].try_into().unwrap()), 0x4000_0010);
            assert!(img[0x2010..0x2100].iter().all(|&b| b == 0));
            // Segment by segment gives the same bytes.
            for s in p.segments() {
                let d = p.segment_data(&s, 0x4000_0000).unwrap();
                let at = s.vaddr as usize;
                assert_eq!(&img[at..at + d.len()], &d[..]);
            }
        }
    }

    #[test]
    fn rejects_bad_programs() {
        let good = program(EM_X86_64);
        let p = |f: &[u8]| Program::parse(f, EM_X86_64).err();
        assert!(p(&good).is_none());
        assert_eq!(Program::parse(&good, EM_AARCH64).err(), Some(ElfError::WrongMachine(62)));
        // A kernel-style fixed-address file isn't a program.
        let mut f = good.clone();
        f[16] = 2;
        assert_eq!(p(&f), Some(ElfError::NotElf));
        let n = note((1, 0), None);
        let stack = (GNU_STACK, PF_R | PF_W, 0, &[][..], 0);
        let code = (LOAD, PF_R | PF_X, 0, &[0x90u8; 16][..], 16);
        let notes = (NOTE, PF_R, 0x3000, &n[..], n.len() as u64);
        let build = |phs: &[Ph<'_>]| make(3, EM_X86_64, 0, phs);
        assert!(p(&build(&[code, notes, stack])).is_none());
        assert_eq!(p(&build(&[code, stack])), Some(ElfError::NoNote));
        assert_eq!(p(&build(&[code, notes])), Some(ElfError::ExecutableStack));
        let xstack = (GNU_STACK, PF_R | PF_W | PF_X, 0, &[][..], 0);
        assert_eq!(p(&build(&[code, notes, xstack])), Some(ElfError::ExecutableStack));
        let tls = (TLS, PF_R, 0x1000, &[][..], 8);
        assert_eq!(p(&build(&[code, notes, stack, tls])), Some(ElfError::ThreadLocalStorage));
        let interp = (INTERP, PF_R, 0x1000, &b"/lib/ld.so\0"[..], 11);
        assert_eq!(p(&build(&[code, notes, stack, interp])), Some(ElfError::Dynamic));
        let wx = (LOAD, PF_R | PF_W | PF_X, 0, &[0u8; 4][..], 4);
        assert_eq!(p(&build(&[wx, notes, stack])), Some(ElfError::WritableAndExecutable));
        let data = (LOAD, PF_R | PF_W, 0, &[0u8; 4][..], 4);
        assert_eq!(p(&build(&[data, notes, stack])), Some(ElfError::BadEntry));
        let huge = (LOAD, PF_R | PF_W, 0x1000, &[][..], 2 << 30);
        assert_eq!(p(&build(&[code, huge, notes, stack])), Some(ElfError::TooBig));
        let bad_stack = note((1, 0), Some(100));
        let bad_notes = (NOTE, PF_R, 0x3000, &bad_stack[..], bad_stack.len() as u64);
        assert_eq!(p(&build(&[code, bad_notes, stack])), Some(ElfError::NoNote));
        // A relocation of another kind, and one aimed outside the program.
        for rel in [relas(1, &[(0x1000, 0)]), relas(RELATIVE_X86_64, &[(0x9000, 0)])] {
            let dynm = dynamic(0x2000, 1);
            let f = build(&[
                code,
                (LOAD, PF_R | PF_W, 0x1000, &[0u8; 8][..], 8),
                (LOAD, PF_R, 0x2000, &rel[..], rel.len() as u64),
                (LOAD, PF_R, 0x4000, &dynm[..], dynm.len() as u64),
                (DYNAMIC, PF_R, 0x4000, &dynm[..], dynm.len() as u64),
                notes,
                stack,
            ]);
            assert_eq!(p(&f), Some(ElfError::BadRelocation));
        }
        // A program needing shared libraries.
        let mut dynm = dynamic(0, 0);
        dynm.splice(0..0, [1u64.to_le_bytes(), 5u64.to_le_bytes()].concat());
        let f = build(&[
            code,
            (LOAD, PF_R, 0x4000, &dynm[..], dynm.len() as u64),
            (DYNAMIC, PF_R, 0x4000, &dynm[..], dynm.len() as u64),
            notes,
            stack,
        ]);
        assert_eq!(p(&f), Some(ElfError::Dynamic));
    }

    /// Damaging any byte of a valid program's headers never panics, and never gives a program
    /// whose image can't be built.
    #[test]
    fn survives_damage() {
        let good = program(EM_X86_64);
        let mut seed = 12345u64;
        for i in 0..good.len().min(0x400) {
            for _ in 0..4 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let mut f = good.clone();
                f[i] = seed as u8;
                if let Ok(p) = Program::parse(&f, EM_X86_64) {
                    p.image(0x1000_0000).unwrap();
                }
            }
        }
    }
}
