// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading the kernel's ELF file: its entry point and the segments to load. The reader itself
//! is `pbr-elf`, shared with the kernel and `granite`, so there's one ELF reader.

pub use pbr_elf::{EM_AARCH64, EM_X86_64, ElfError, Kernel, PF_R, PF_W, PF_X, Segment};

#[cfg(test)]
pub(crate) mod tests {
    /// Builds a small kernel file with the given segments `(vaddr, data, mem_len, flags)`.
    pub fn make(machine: u16, entry: u64, segs: &[(u64, &[u8], u64, u32)]) -> Vec<u8> {
        let phs: Vec<pbr_elf::testing::Ph<'_>> = segs
            .iter()
            .map(|&(vaddr, data, mem, flags)| (pbr_elf::testing::LOAD, flags, vaddr, data, mem))
            .collect();
        pbr_elf::testing::make(2, machine, entry, &phs)
    }
}
