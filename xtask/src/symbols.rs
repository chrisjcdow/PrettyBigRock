// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Naming the kernel functions that addresses belong to, for the report of a boot that hung or
//! crashed.
//!
//! When a boot test times out, the runner asks QEMU for every core's registers and the top of
//! its stack (see `qemu::Qmp::registers`). Those are bare numbers. To learn where each core is
//! stuck they have to be matched against the kernel that was running, and that kernel may be
//! gone by the time anyone reads the report: a failure on GitHub leaves only its log. So the
//! runner does the matching itself, there and then, against the kernel file it just booted:
//! every number in the dump that lies inside a kernel function gets that function's name and
//! the distance into it written after its line.
//!
//! The names come from the ELF file's symbol table (ELF is the format the kernel is built in;
//! the symbol table lists each function's name, address and size). Rust "mangles" names into a
//! compact code (`_RNvNtCs…`); if a demangling tool is installed it is used to turn them back
//! into readable paths, and if not they are left as they are, to be decoded later with
//! `llvm-cxxfilt` or `rustfilt`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// The functions of one kernel, sorted by address.
pub struct Symbols {
    /// (start, size, name).
    functions: Vec<(u64, u64, String)>,
}

/// Reads a little-endian number of `N` bytes at `at`, or `None` if the file is too short.
/// (`?` returns `None` early; `try_into` checks the slice is exactly `N` bytes.)
fn bytes<const N: usize>(file: &[u8], at: u64) -> Option<[u8; N]> {
    let at = usize::try_from(at).ok()?;
    file.get(at..at.checked_add(N)?)?.try_into().ok()
}
fn u16_at(file: &[u8], at: u64) -> Option<u64> {
    bytes(file, at).map(|b| u64::from(u16::from_le_bytes(b)))
}
fn u32_at(file: &[u8], at: u64) -> Option<u64> {
    bytes(file, at).map(|b| u64::from(u32::from_le_bytes(b)))
}
fn u64_at(file: &[u8], at: u64) -> Option<u64> {
    bytes(file, at).map(u64::from_le_bytes)
}

impl Symbols {
    /// The function symbols of a 64-bit little-endian ELF file; `None` if it isn't one or has
    /// no symbol table. Nothing here trusts the file: every offset is checked.
    pub fn read(file: &[u8]) -> Option<Symbols> {
        if file.get(..6)? != b"\x7fELF\x02\x01" {
            return None;
        }
        // The section header table: where it is, how big each entry is, how many there are.
        let (table, entry, count) = (u64_at(file, 0x28)?, u16_at(file, 0x3A)?, u16_at(file, 0x3C)?);
        let section = |i: u64| -> Option<u64> { table.checked_add(i.checked_mul(entry)?) };
        let mut functions = Vec::new();
        for i in 0..count {
            let s = section(i)?;
            // Type 2 is a symbol table; its `link` field names the section with the names.
            if u32_at(file, s.saturating_add(4))? != 2 {
                continue;
            }
            let (offset, size) =
                (u64_at(file, s.saturating_add(0x18))?, u64_at(file, s.saturating_add(0x20))?);
            let strings = section(u32_at(file, s.saturating_add(0x28))?)?;
            let (names, names_size) = (
                u64_at(file, strings.saturating_add(0x18))?,
                u64_at(file, strings.saturating_add(0x20))?,
            );
            // Each symbol is 24 bytes: name (an offset into the names), info (whose low four
            // bits are its type: 2 is a function), other, section, value (address), size.
            for n in 0..size / 24 {
                let sym = offset.checked_add(n.checked_mul(24)?)?;
                let (name, info) =
                    (u32_at(file, sym)?, bytes::<1>(file, sym.saturating_add(4))?[0]);
                let (value, len) =
                    (u64_at(file, sym.saturating_add(8))?, u64_at(file, sym.saturating_add(16))?);
                if info & 0xF != 2 || len == 0 || name >= names_size {
                    continue;
                }
                let start = usize::try_from(names.checked_add(name)?).ok()?;
                let rest = file.get(start..)?;
                let end = rest.iter().position(|&b| b == 0)?;
                let name = String::from_utf8_lossy(&rest[..end]).into_owned();
                functions.push((value, len, name));
            }
        }
        if functions.is_empty() {
            return None;
        }
        functions.sort();
        Some(Symbols { functions })
    }

    /// Reads the kernel at `path`, and demangles its names if a tool for that is installed.
    pub fn of_kernel(path: &Path) -> Option<Symbols> {
        let mut s = Symbols::read(&std::fs::read(path).ok()?)?;
        s.demangle();
        Some(s)
    }

    /// The function `address` is in, and how far into it.
    pub fn find(&self, address: u64) -> Option<(&str, u64)> {
        // The last function starting at or before the address.
        let at =
            self.functions.partition_point(|(start, _, _)| *start <= address).checked_sub(1)?;
        let (start, size, name) = &self.functions[at];
        (address - start < *size).then_some((name.as_str(), address - start))
    }

    /// `text` with, after each line, the kernel functions its numbers point into. A number is
    /// any run of hexadecimal digits (with or without `0x`, or after `NAME=` as QEMU writes
    /// registers) that is an address inside a function.
    pub fn annotate(&self, text: &str) -> String {
        let mut out = String::new();
        for line in text.lines() {
            out.push_str(line);
            out.push('\n');
            for word in line.split(|c: char| !c.is_ascii_alphanumeric()) {
                let digits = word.strip_prefix("0x").unwrap_or(word);
                // Kernel addresses are 16 digits; anything shorter is a small number.
                if digits.len() != 16 {
                    continue;
                }
                let Ok(address) = u64::from_str_radix(digits, 16) else { continue };
                if let Some((name, offset)) = self.find(address) {
                    out.push_str(&format!("        {digits} is in {name} (+{offset:#x})\n"));
                }
            }
        }
        out
    }

    /// Turns the mangled names back into readable ones with whichever tool is installed
    /// (one name a line in, one a line out). Without one, the names stay as they are.
    fn demangle(&mut self) {
        const TOOLS: [&str; 7] = [
            "rustfilt",
            "llvm-cxxfilt",
            "llvm-cxxfilt-20",
            "llvm-cxxfilt-19",
            "llvm-cxxfilt-18",
            "llvm-cxxfilt-17",
            "llvm-cxxfilt-16",
        ];
        let input: String = self.functions.iter().map(|(_, _, n)| format!("{n}\n")).collect();
        for tool in TOOLS {
            let child = Command::new(tool)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else { continue };
            // Written from another thread, so that a tool which answers as it reads can't
            // fill its output pipe while this is still blocked writing its input.
            let Some(mut stdin) = child.stdin.take() else { continue };
            let text = input.clone();
            let writer = std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            });
            let output = child.wait_with_output();
            let _ = writer.join();
            let Ok(output) = output else { continue };
            let names: Vec<&str> =
                std::str::from_utf8(&output.stdout).unwrap_or_default().lines().collect();
            if output.status.success() && names.len() == self.functions.len() {
                for (f, name) in self.functions.iter_mut().zip(names) {
                    f.2 = name.to_string();
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny ELF file with a symbol table: a 64-byte header, then the names, the symbols, and
    /// three section headers (an empty one, the symbol table, the names).
    fn elf(symbols: &[(&str, u8, u64, u64)]) -> Vec<u8> {
        let mut names = vec![0u8];
        let mut table = Vec::new();
        for (name, info, value, size) in symbols {
            table.extend((names.len() as u32).to_le_bytes());
            table.extend([*info, 0, 1, 0]);
            table.extend(value.to_le_bytes());
            table.extend(size.to_le_bytes());
            names.extend(name.as_bytes());
            names.push(0);
        }
        let mut file = vec![0u8; 64];
        file[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let names_at = file.len() as u64;
        file.extend(&names);
        let table_at = file.len() as u64;
        file.extend(&table);
        let headers_at = file.len() as u64;
        let header = |kind: u32, offset: u64, size: u64, link: u32| {
            let mut h = vec![0u8; 64];
            h[4..8].copy_from_slice(&kind.to_le_bytes());
            h[0x18..0x20].copy_from_slice(&offset.to_le_bytes());
            h[0x20..0x28].copy_from_slice(&size.to_le_bytes());
            h[0x28..0x2C].copy_from_slice(&link.to_le_bytes());
            h
        };
        file.extend(header(0, 0, 0, 0));
        file.extend(header(2, table_at, table.len() as u64, 2));
        file.extend(header(3, names_at, names.len() as u64, 0));
        file[0x28..0x30].copy_from_slice(&headers_at.to_le_bytes());
        file[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        file[0x3C..0x3E].copy_from_slice(&3u16.to_le_bytes());
        file
    }

    const BASE: u64 = 0xFFFF_FFFF_8000_0000;

    fn sample() -> Symbols {
        Symbols::read(&elf(&[
            ("idle", 2, BASE + 0x100, 0x20),
            ("spin", 0x12, BASE + 0x200, 0x40), // a global function (binding 1, type 2)
            ("a_variable", 1, BASE + 0x300, 8), // an object, not a function
            ("empty", 2, BASE + 0x400, 0),
        ]))
        .expect("a symbol table")
    }

    #[test]
    fn finds_the_function_an_address_is_in() {
        let s = sample();
        assert_eq!(s.find(BASE + 0x100), Some(("idle", 0)));
        assert_eq!(s.find(BASE + 0x11F), Some(("idle", 0x1F)));
        assert_eq!(s.find(BASE + 0x120), None); // just past its end
        assert_eq!(s.find(BASE + 0x23C), Some(("spin", 0x3C)));
        assert_eq!(s.find(BASE + 0x300), None); // data
        assert_eq!(s.find(BASE + 0x400), None); // no size
        assert_eq!(s.find(0), None);
        assert_eq!(s.find(u64::MAX), None);
    }

    #[test]
    fn annotates_registers_and_stacks_as_qemu_prints_them() {
        let s = sample();
        let text = " PC=ffffffff80000104 X00=ffffc0000003fbe8\n\
                    ffffc0000003fd80: 0xffffffff8000023c 0x0000000000000004\n\
                    RIP=ffffffff80000100 RFL=00000046";
        let out = s.annotate(text);
        assert!(out.contains("ffffffff80000104 is in idle (+0x4)"));
        assert!(out.contains("ffffffff8000023c is in spin (+0x3c)"));
        assert!(out.contains("ffffffff80000100 is in idle (+0x0)"));
        // Every line of the dump is still there, in order, and nothing else was named.
        assert_eq!(out.lines().filter(|l| !l.contains(" is in ")).count(), 3);
        assert_eq!(out.lines().filter(|l| l.contains(" is in ")).count(), 3);
    }

    #[test]
    fn refuses_files_that_are_not_what_it_reads() {
        assert!(Symbols::read(b"").is_none());
        assert!(Symbols::read(b"not an ELF file at all, just some text").is_none());
        // No symbols, and one whose tables point outside the file.
        assert!(Symbols::read(&elf(&[])).is_none());
        let mut bad = elf(&[("idle", 2, BASE, 16)]);
        bad[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Symbols::read(&bad).is_none());
        let good = elf(&[("idle", 2, BASE, 16)]);
        for cut in 0..good.len() {
            let _ = Symbols::read(&good[..cut]); // must not panic, whatever is missing
        }
    }
}
