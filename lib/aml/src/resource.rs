// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Resource templates: the buffers `_CRS` ("current resource settings") returns, listing the
//! ports, memory and interrupts a device uses (ACPI 6.5, section 6.4).
//!
//! A template is a run of descriptors, each with a tag saying its kind and length, ended by
//! an end tag. Small descriptors have a one-byte tag (kind in bits 3 to 6, length in bits 0 to
//! 2); large ones have bit 7 set, the kind in bits 0 to 6, and a 16-bit length after. Each is
//! checked against its length before it is read; a kind this library doesn't know is stepped
//! over; a template that runs out before its end tag is an error.

use alloc::vec::Vec;

/// An interrupt line, as the firmware describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    /// On x86-64 the global system interrupt (an ISA IRQ for the old descriptor); on ARM64
    /// the GIC's interrupt ID.
    pub number: u32,
    /// Level-triggered (otherwise edge).
    pub level: bool,
    /// Active low (otherwise high).
    pub active_low: bool,
    /// May be shared with other devices.
    pub shared: bool,
}

/// One thing a device uses, or (for a bridge) passes on to the devices behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// I/O ports, `count` of them from `first`.
    Io {
        /// The first port.
        first: u16,
        /// How many.
        count: u16,
    },
    /// Physical memory.
    Memory {
        /// Where it starts.
        base: u64,
        /// How long it is.
        length: u64,
    },
    /// A range of bus numbers (a PCI host bridge's).
    Bus {
        /// The first bus.
        first: u16,
        /// How many.
        count: u16,
    },
    /// An interrupt line.
    Interrupt(Line),
}

/// Why a template couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    /// It ran out before its end tag, or a descriptor ran past its end.
    CutShort,
    /// A descriptor is shorter than its kind needs.
    TooShort,
}

/// A little-endian integer of `bytes.len()` bytes (up to 8).
fn le(bytes: &[u8]) -> u64 {
    bytes.iter().rev().fold(0, |acc, &b| (acc << 8) | u64::from(b))
}

/// Reads a template.
pub fn decode(t: &[u8]) -> Result<Vec<Resource>, TemplateError> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let &tag = t.get(i).ok_or(TemplateError::CutShort)?;
        if tag & 0x80 == 0 {
            let kind = (tag >> 3) & 0x0F;
            let len = usize::from(tag & 7);
            let body = t.get(i + 1..i + 1 + len).ok_or(TemplateError::CutShort)?;
            match kind {
                0x0F => return Ok(out), // the end tag (and its checksum)
                0x04 => irq(body, &mut out)?,
                0x08 => {
                    // IO: information, minimum, maximum, alignment, length. The current
                    // settings have the minimum and maximum the same.
                    let b = body.get(..7).ok_or(TemplateError::TooShort)?;
                    let count = u16::from(b[6]);
                    if count > 0 {
                        out.push(Resource::Io { first: le(&b[1..3]) as u16, count });
                    }
                }
                0x09 => {
                    // FixedIO: a 10-bit base and a length.
                    let b = body.get(..3).ok_or(TemplateError::TooShort)?;
                    let count = u16::from(b[2]);
                    if count > 0 {
                        out.push(Resource::Io { first: (le(&b[..2]) & 0x3FF) as u16, count });
                    }
                }
                _ => {} // DMA, start and end of dependent functions, vendor: not needed
            }
            i += 1 + len;
        } else {
            let l = t.get(i + 1..i + 3).ok_or(TemplateError::CutShort)?;
            let len = usize::from(u16::from_le_bytes([l[0], l[1]]));
            let body = t.get(i + 3..i + 3 + len).ok_or(TemplateError::CutShort)?;
            match tag & 0x7F {
                0x01 => {
                    // Memory24: in units of 256 bytes.
                    let b = body.get(..9).ok_or(TemplateError::TooShort)?;
                    let length = le(&b[7..9]) << 8;
                    if length > 0 {
                        out.push(Resource::Memory { base: le(&b[1..3]) << 8, length });
                    }
                }
                0x05 => {
                    // Memory32: information, minimum, maximum, alignment, length.
                    let b = body.get(..17).ok_or(TemplateError::TooShort)?;
                    let length = le(&b[13..17]);
                    if length > 0 {
                        out.push(Resource::Memory { base: le(&b[1..5]), length });
                    }
                }
                0x06 => {
                    // Memory32Fixed: information, base, length.
                    let b = body.get(..9).ok_or(TemplateError::TooShort)?;
                    let length = le(&b[5..9]);
                    if length > 0 {
                        out.push(Resource::Memory { base: le(&b[1..5]), length });
                    }
                }
                0x07 => address(body, 4, &mut out)?, // DWord address space
                0x08 => address(body, 2, &mut out)?, // Word
                0x0A => address(body, 8, &mut out)?, // QWord
                0x0B => {
                    // Extended address space: type, flags, type flags, revision, reserved,
                    // then granularity, minimum, maximum, translation and length, 8 bytes each.
                    let b = body.get(..45).ok_or(TemplateError::TooShort)?;
                    push_address(b[0], le(&b[13..21]), le(&b[37..45]), &mut out);
                }
                0x09 => {
                    // Extended interrupt: flags, a count, then that many 32-bit numbers.
                    let b = body.get(..2).ok_or(TemplateError::TooShort)?;
                    let flags = b[0];
                    let count = usize::from(b[1]);
                    let numbers = body.get(2..2 + 4 * count).ok_or(TemplateError::TooShort)?;
                    for n in numbers.chunks_exact(4) {
                        out.push(Resource::Interrupt(Line {
                            number: le(n) as u32,
                            level: flags & 2 == 0,
                            active_low: flags & 4 != 0,
                            shared: flags & 8 != 0,
                        }));
                    }
                }
                _ => {} // generic registers, vendor, GPIO, serial buses and the rest
            }
            i += 3 + len;
        }
    }
}

/// The old IRQ descriptor: a mask of ISA lines, and optionally their trigger and polarity
/// (without it, edge and active high).
fn irq(body: &[u8], out: &mut Vec<Resource>) -> Result<(), TemplateError> {
    let b = body.get(..2).ok_or(TemplateError::TooShort)?;
    let mask = u16::from_le_bytes([b[0], b[1]]);
    let info = body.get(2).copied().unwrap_or(1);
    for number in 0..16 {
        if mask & (1 << number) != 0 {
            out.push(Resource::Interrupt(Line {
                number,
                level: info & 1 == 0,
                active_low: info & 8 != 0,
                shared: info & 0x10 != 0,
            }));
        }
    }
    Ok(())
}

/// The Word, DWord and QWord address-space descriptors: type, flags, type flags, then
/// granularity, minimum, maximum, translation and length, each `w` bytes.
fn address(body: &[u8], w: usize, out: &mut Vec<Resource>) -> Result<(), TemplateError> {
    let b = body.get(..3 + 5 * w).ok_or(TemplateError::TooShort)?;
    let min = le(&b[3 + w..3 + 2 * w]);
    let length = le(&b[3 + 4 * w..3 + 5 * w]);
    push_address(b[0], min, length, out);
    Ok(())
}

fn push_address(kind: u8, min: u64, length: u64, out: &mut Vec<Resource>) {
    if length == 0 {
        return;
    }
    match kind {
        0 => out.push(Resource::Memory { base: min, length }),
        1 => out.push(Resource::Io { first: min as u16, count: length.min(0xFFFF) as u16 }),
        2 => out.push(Resource::Bus { first: min as u16, count: length.min(0xFFFF) as u16 }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_serial_port() {
        // IO (Decode16, 0x2F8, 0x2F8, 0x01, 0x08), IRQNoFlags () {3}, end tag.
        let t = [0x47, 0x01, 0xF8, 0x02, 0xF8, 0x02, 0x01, 0x08, 0x22, 0x08, 0x00, 0x79, 0x00];
        let r = decode(&t).unwrap();
        assert_eq!(r[0], Resource::Io { first: 0x2F8, count: 8 });
        let line = Line { number: 3, level: false, active_low: false, shared: false };
        assert_eq!(r[1], Resource::Interrupt(line));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn an_extended_interrupt() {
        // Interrupt (ResourceConsumer, Level, ActiveHigh, Shared) {0x10}: flags 0b1001.
        let t = [0x89, 0x06, 0x00, 0x09, 0x01, 0x10, 0x00, 0x00, 0x00, 0x79, 0x00];
        let line = Line { number: 16, level: true, active_low: false, shared: true };
        assert_eq!(decode(&t).unwrap(), [Resource::Interrupt(line)]);
    }

    #[test]
    fn cut_short_and_unknown() {
        assert_eq!(decode(&[0x47, 0x01, 0xF8]), Err(TemplateError::CutShort));
        assert_eq!(decode(&[]), Err(TemplateError::CutShort));
        // A small descriptor too short for its kind.
        assert_eq!(decode(&[0x41, 0x00, 0x79, 0x00]), Err(TemplateError::TooShort));
        // An unknown large descriptor is stepped over; no end tag is an error.
        assert_eq!(decode(&[0xFE, 0x01, 0x00, 0xAA, 0x79, 0x00]), Ok(Vec::new()));
        assert_eq!(decode(&[0xFE, 0x01, 0x00, 0xAA]), Err(TemplateError::CutShort));
        // A huge length on a short buffer.
        assert_eq!(decode(&[0x86, 0xFF, 0xFF, 0x00]), Err(TemplateError::CutShort));
    }
}
