// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! **A stopgap**, to be removed whole when the ACPI interpreter comes (M7): finding the sleep
//! type values for turning an x86 PC off (docs/design/0024-the-session-desktop.md, section 7)
//! by scanning the DSDT's and SSDTs' bytes, as 1.3 did (`acpi_poweroff` in
//! `classic/kernel/hw.c`).
//!
//! ACPI describes a machine in a small bytecode language, AML. Among its definitions is a name,
//! `\_S5`, for a package of numbers: the first two are what to write into the PM1a and PM1b
//! control registers (with the "sleep enable" bit) to put the machine in sleep state 5, which
//! is off. Reading it properly needs an interpreter; but the name, written `_S5_` in AML, is
//! almost always defined as a plain package of constants, so scanning for those bytes finds it.

/// AML's opcodes met on the way.
const NAME_OP: u8 = 0x08;
const ROOT_CHAR: u8 = b'\\';
const PACKAGE_OP: u8 = 0x12;
const BYTE_PREFIX: u8 = 0x0A;

/// The sleep type values for PM1a and PM1b (each 3 bits) from the first `Name (_S5_,
/// Package (...) {a, b, ...})` in `aml` (a table's contents, header included), if there is one
/// written as constants.
pub fn sleep_types(aml: &[u8]) -> Option<(u16, u16)> {
    let mut at = 0;
    while let Some(found) = aml.get(at..)?.windows(4).position(|w| w == b"_S5_") {
        let s = at + found;
        at = s + 1;
        // Defined by `Name` (the opcode just before it, or before a `\` that says the name
        // starts from the root), and a package after it.
        let named = s >= 1 && aml[s - 1] == NAME_OP
            || s >= 2 && aml[s - 2] == NAME_OP && aml[s - 1] == ROOT_CHAR;
        if !named || aml.get(s + 4) != Some(&PACKAGE_OP) {
            continue;
        }
        // The package's length is 1 to 4 bytes (its first byte's top two bits say how many
        // follow), then one byte with the number of elements.
        let mut p = s + 5;
        p += usize::from(aml.get(p)? >> 6) + 2;
        let mut value = || {
            if aml.get(p) == Some(&BYTE_PREFIX) {
                p += 1;
            }
            let v = *aml.get(p)?;
            p += 1;
            Some(u16::from(v) & 7)
        };
        let a = value()?;
        let b = value()?;
        return Some((a, b));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sleep_types() {
        // QEMU's: `Name (_S5, Package (0x04) { Zero, Zero, Zero, Zero })`.
        let qemu = [0x10, 0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0, 0, 0, 0, 0x5B];
        assert_eq!(sleep_types(&qemu), Some((0, 0)));
        // With byte prefixes and a root `\`: `Name (\_S5, Package (2) { 0x05, 0x07 })`.
        let real = [0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x07, 0x02, 0x0A, 5, 0x0A, 7];
        assert_eq!(sleep_types(&real), Some((5, 7)));
        // A two-byte package length.
        let long = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x46, 0x00, 0x04, 0x0A, 6, 1];
        assert_eq!(sleep_types(&long), Some((6, 1)));
    }

    #[test]
    fn ignores_what_is_not_a_definition() {
        // A reference to `_S5_` (no `Name` before it), then the real one.
        let aml =
            [0x70, b'_', b'S', b'5', b'_', 0x08, b'_', b'S', b'5', b'_', 0x12, 4, 2, 0x0A, 3, 1];
        assert_eq!(sleep_types(&aml), Some((3, 1)));
        assert_eq!(sleep_types(b"no such name here"), None);
        assert_eq!(sleep_types(&[]), None);
        // Cut off part-way.
        assert_eq!(sleep_types(&[0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x0A]), None);
        assert_eq!(sleep_types(&[0x08, b'_', b'S', b'5', b'_']), None);
        // Any bytes at all: never a panic.
        let mut x = 12345u32;
        for len in 0..200 {
            let mut v = vec![0u8; len];
            for b in v.iter_mut() {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                *b = [0x08, 0x12, 0x0A, b'_', b'S', b'5', b'\\', (x >> 16) as u8]
                    [(x >> 8) as usize % 8];
            }
            let _ = sleep_types(&v);
        }
    }
}
