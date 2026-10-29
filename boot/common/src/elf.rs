// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading the kernel's ELF file: its entry point and the segments to load.

use crate::{PAGE, align_down, align_up};

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

const PT_LOAD: u32 = 1;

/// Why a kernel file was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// Not a 64-bit little-endian ELF executable.
    NotElf,
    /// Built for a different processor.
    WrongMachine(u16),
    /// A header or segment points outside the file, or overflows.
    Truncated,
    /// No loadable segments, or segments that overlap or aren't page-aligned.
    BadSegments,
    /// A segment is both writable and executable.
    WritableAndExecutable,
}

/// One loadable segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Virtual address it's linked at (page-aligned).
    pub vaddr: u64,
    /// Bytes in memory (the rest after `file_len` is zeroed).
    pub mem_len: u64,
    /// Where its bytes start in the file.
    pub file_offset: u64,
    /// Bytes taken from the file.
    pub file_len: u64,
    /// `PF_*` flags.
    pub flags: u32,
}

impl Segment {
    /// True if writable.
    pub fn writable(&self) -> bool {
        self.flags & PF_W != 0
    }
    /// True if executable.
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }
}

/// A checked kernel file.
pub struct Kernel<'a> {
    file: &'a [u8],
    /// The entry point's virtual address.
    pub entry: u64,
    phoff: u64,
    phentsize: u64,
    phnum: u64,
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

impl<'a> Kernel<'a> {
    /// Checks the file is a kernel for `machine` and that its segments make sense.
    pub fn parse(file: &'a [u8], machine: u16) -> Result<Self, ElfError> {
        if file.len() < 64 || &file[..4] != b"\x7fELF" || file[4] != 2 || file[5] != 1 {
            return Err(ElfError::NotElf);
        }
        if u16_at(file, 16) != Some(2) {
            return Err(ElfError::NotElf);
        }
        let m = u16_at(file, 18).ok_or(ElfError::Truncated)?;
        if m != machine {
            return Err(ElfError::WrongMachine(m));
        }
        let k = Kernel {
            file,
            entry: u64_at(file, 24).ok_or(ElfError::Truncated)?,
            phoff: u64_at(file, 32).ok_or(ElfError::Truncated)?,
            phentsize: u64::from(u16_at(file, 54).ok_or(ElfError::Truncated)?),
            phnum: u64::from(u16_at(file, 56).ok_or(ElfError::Truncated)?),
        };
        if k.phentsize < 56 {
            return Err(ElfError::Truncated);
        }
        let mut count = 0;
        let mut prev_end = 0u64;
        for i in 0..k.phnum {
            let Some(s) = k.segment(i)? else { continue };
            count += 1;
            if s.vaddr % PAGE != 0 || s.file_len > s.mem_len || s.vaddr < prev_end {
                return Err(ElfError::BadSegments);
            }
            if s.writable() && s.executable() {
                return Err(ElfError::WritableAndExecutable);
            }
            let fend = s.file_offset.checked_add(s.file_len).ok_or(ElfError::Truncated)?;
            if fend > file.len() as u64 {
                return Err(ElfError::Truncated);
            }
            prev_end = s.vaddr.checked_add(s.mem_len).ok_or(ElfError::BadSegments)?;
        }
        if count == 0 {
            return Err(ElfError::BadSegments);
        }
        Ok(k)
    }

    fn segment(&self, i: u64) -> Result<Option<Segment>, ElfError> {
        let o = self.phoff.checked_add(i * self.phentsize).ok_or(ElfError::Truncated)?;
        let t = u32_at(self.file, o).ok_or(ElfError::Truncated)?;
        if t != PT_LOAD {
            return Ok(None);
        }
        let get = |off| u64_at(self.file, o + off).ok_or(ElfError::Truncated);
        Ok(Some(Segment {
            flags: u32_at(self.file, o + 4).ok_or(ElfError::Truncated)?,
            file_offset: get(8)?,
            vaddr: get(16)?,
            file_len: get(32)?,
            mem_len: get(40)?,
        }))
    }

    /// The loadable segments, in address order.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        (0..self.phnum).filter_map(|i| self.segment(i).ok().flatten())
    }

    /// The virtual range the kernel occupies: from its first segment's page to the end of
    /// its last, page-aligned.
    pub fn span(&self) -> (u64, u64) {
        let mut lo = u64::MAX;
        let mut hi = 0;
        for s in self.segments() {
            lo = lo.min(align_down(s.vaddr, PAGE));
            hi = hi.max(align_up(s.vaddr + s.mem_len, PAGE));
        }
        (lo, hi)
    }

    /// Copies the kernel into `dest`, which must be `span` bytes long and is laid out exactly as
    /// the kernel's virtual range: `dest[0]` is the first byte of the span. Zeroes the rest.
    pub fn load_into(&self, dest: &mut [u8]) -> Result<(), ElfError> {
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a small kernel file with the given segments `(vaddr, data, mem_len, flags)`.
    pub fn make(machine: u16, entry: u64, segs: &[(u64, &[u8], u64, u32)]) -> Vec<u8> {
        let mut f = vec![0u8; 64 + 56 * segs.len()];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[16..18].copy_from_slice(&2u16.to_le_bytes());
        f[18..20].copy_from_slice(&machine.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        for (i, (vaddr, data, mem, flags)) in segs.iter().enumerate() {
            let off = f.len() as u64;
            f.extend_from_slice(data);
            let p = 64 + 56 * i;
            f[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
            f[p + 4..p + 8].copy_from_slice(&flags.to_le_bytes());
            f[p + 8..p + 16].copy_from_slice(&off.to_le_bytes());
            f[p + 16..p + 24].copy_from_slice(&vaddr.to_le_bytes());
            f[p + 32..p + 40].copy_from_slice(&(data.len() as u64).to_le_bytes());
            f[p + 40..p + 48].copy_from_slice(&mem.to_le_bytes());
        }
        f
    }

    const B: u64 = crate::KERNEL_BASE;

    #[test]
    fn loads_segments() {
        let f = make(
            EM_X86_64,
            B,
            &[(B, b"code", 4, PF_R | PF_X), (B + 0x2000, b"data", 0x1800, PF_R | PF_W)],
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
        assert_eq!(Kernel::parse(b"nope", EM_X86_64).err(), Some(ElfError::NotElf));
        let f = make(EM_AARCH64, B, &[(B, b"x", 1, PF_R | PF_X)]);
        assert_eq!(Kernel::parse(&f, EM_X86_64).err(), Some(ElfError::WrongMachine(EM_AARCH64)));
        let f = make(EM_X86_64, B, &[(B, b"x", 1, PF_R | PF_W | PF_X)]);
        assert_eq!(Kernel::parse(&f, EM_X86_64).err(), Some(ElfError::WritableAndExecutable));
        let f = make(EM_X86_64, B, &[(B + 1, b"x", 1, PF_R)]);
        assert_eq!(Kernel::parse(&f, EM_X86_64).err(), Some(ElfError::BadSegments));
        let f = make(EM_X86_64, B, &[(B + 0x1000, b"x", 1, PF_R), (B, b"y", 1, PF_R)]);
        assert_eq!(Kernel::parse(&f, EM_X86_64).err(), Some(ElfError::BadSegments));
        let mut f = make(EM_X86_64, B, &[(B, b"xyz", 3, PF_R)]);
        let n = f.len();
        f.truncate(n - 1);
        assert_eq!(Kernel::parse(&f, EM_X86_64).err(), Some(ElfError::Truncated));
    }
}
