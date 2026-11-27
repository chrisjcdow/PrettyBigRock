// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Choosing the early serial console (docs/design/0027-kernel-1.4.md, section 3).
//!
//! The kernel writes its log to whatever serial port the loader names in the boot information.
//! On x86-64 the loader chooses, in order: a `console=` word on the command line (the person's
//! own choice comes first, since firmware tables are sometimes wrong), then the firmware's SPCR
//! table, then COM1, the port nearly every PC had until about 2015.

use pbr_bootinfo::EarlyConsole;

/// COM1's I/O port: where the console goes when nothing says otherwise.
pub const COM1: u16 = 0x3F8;

/// What a `console=` word asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    /// `console=none`: no serial console at all.
    None,
    /// `console=0x2f8`: a 16550 UART at that I/O port.
    Port(u16),
}

/// The `console=` word on the command line, if there is one that makes sense. When there are
/// several, the last wins, so a word added at the end overrides one already there. A value
/// that isn't `none` or a hexadecimal port (such as Linux's `console=ttyS1`) is ignored.
pub fn word(cmdline: &[u8]) -> Option<Word> {
    let mut found = None;
    for w in cmdline.split(|b| b.is_ascii_whitespace()) {
        let Some(v) = w.strip_prefix(b"console=") else { continue };
        if v.eq_ignore_ascii_case(b"none") {
            found = Some(Word::None);
        } else if let Some(port) = hex_port(v) {
            found = Some(Word::Port(port));
        }
    }
    found
}

/// `0x` and one to four hexadecimal digits, not zero.
fn hex_port(v: &[u8]) -> Option<u16> {
    let digits = v.strip_prefix(b"0x").or_else(|| v.strip_prefix(b"0X"))?;
    if digits.is_empty() || digits.len() > 4 {
        return None;
    }
    let mut n: u16 = 0;
    for &d in digits {
        // `to_digit` turns '0' to '9' and 'a' to 'f' (either case) into their values.
        n = n * 16 + (d as char).to_digit(16)? as u16;
    }
    (n != 0).then_some(n)
}

/// A 16550 UART at an I/O port, as the boot information describes it.
pub const fn port(p: u16) -> EarlyConsole {
    EarlyConsole { kind: 1, register_width: 1, address: p as u64 }
}

/// The x86-64 loaders' choice: the command line's word, else the SPCR table's console (if the
/// firmware has one), else COM1. "None" is the boot information's kind 0.
pub fn choose_x86(cmdline: &[u8], spcr: Option<EarlyConsole>) -> EarlyConsole {
    match word(cmdline) {
        Some(Word::None) => EarlyConsole::default(),
        Some(Word::Port(p)) => port(p),
        None => spcr.unwrap_or(port(COM1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(word(b""), None);
        assert_eq!(word(b"selftest"), None);
        assert_eq!(word(b"console=0x2f8"), Some(Word::Port(0x2F8)));
        assert_eq!(word(b"selftest console=0X2F8 x"), Some(Word::Port(0x2F8)));
        assert_eq!(word(b"console=none"), Some(Word::None));
        assert_eq!(word(b"console=NONE"), Some(Word::None));
        // The last word that makes sense wins.
        assert_eq!(word(b"console=none console=0x3e8"), Some(Word::Port(0x3E8)));
        assert_eq!(word(b"console=0x3e8 console=ttyS1"), Some(Word::Port(0x3E8)));
        // Nonsense is ignored.
        for bad in [
            &b"console=ttyS1"[..],
            b"console=2f8",
            b"console=0x",
            b"console=0x0",
            b"console=0x12345",
            b"console=0xg",
            b"xconsole=0x2f8",
            b"console=",
        ] {
            assert_eq!(word(bad), None, "{:?}", core::str::from_utf8(bad));
        }
        assert_eq!(word(b"console=0xffff"), Some(Word::Port(0xFFFF)));
        assert_eq!(word(b"a\tconsole=0x2f8\nb"), Some(Word::Port(0x2F8)));
    }

    #[test]
    fn choosing() {
        let spcr = EarlyConsole { kind: 2, register_width: 4, address: 0xFE00_0000 };
        assert_eq!(choose_x86(b"", None), port(COM1));
        assert_eq!(choose_x86(b"", Some(spcr)), spcr);
        assert_eq!(choose_x86(b"console=0x2f8", Some(spcr)), port(0x2F8));
        assert_eq!(choose_x86(b"console=none", Some(spcr)).kind, 0);
        assert_eq!(choose_x86(b"console=bad", None), port(COM1));
    }
}
