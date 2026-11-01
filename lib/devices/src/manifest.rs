// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Driver manifests (note 0011, section 5): the small text file beside each driver that says
//! what it drives and what it needs, and the rule for choosing a driver for a device.
//!
//! A manifest is one `key value…` per line, ASCII, at most [`MAX_SIZE`] bytes, `#` starting a
//! comment. Anything unexpected is an error and the driver is skipped: unknown keys, repeated
//! keys (except `match`), bad or oversized numbers, over-long lines.

use alloc::string::String;
use alloc::vec::Vec;

/// The largest manifest.
pub const MAX_SIZE: usize = 1024;
/// The longest line.
pub const MAX_LINE: usize = 200;
/// The most `match` lines.
pub const MAX_MATCHES: usize = 16;
/// The longest `args` value.
pub const MAX_ARGS: usize = 128;
/// The most interrupts a driver can ask for.
pub const MAX_IRQS: u32 = 8;
/// The most DMA memory a driver can ask for (what the kernel allows, note 0010).
pub const MAX_DMA: u64 = 4 << 20;
/// The smallest and largest memory budgets.
pub const MIN_BUDGET: u64 = 64 << 10;
/// See [`MIN_BUDGET`].
pub const MAX_BUDGET: u64 = 256 << 20;

/// The class of interface a driver offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// `pbr.block`
    Block,
    /// `pbr.input`
    Input,
    /// `pbr.serial`
    Serial,
    /// `pbr.display`
    Display,
}

/// The devices that aren't on PCI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Legacy {
    /// The second serial port.
    Com2,
    /// The 8042 keyboard and mouse controller.
    Ps2,
    /// The firmware's framebuffer.
    Framebuffer,
}

/// One `match` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// `match pci VENDOR:DEVICE`
    Pci {
        /// Vendor ID.
        vendor: u16,
        /// Device ID.
        device: u16,
    },
    /// `match pci class CLASS:SUBCLASS[:PROGIF]`
    PciClass {
        /// Class code.
        class: u8,
        /// Sub-class.
        subclass: u8,
        /// Programming interface, if it matters.
        prog_if: Option<u8>,
    },
    /// `match legacy NAME`
    Legacy(Legacy),
}

/// A parsed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// What it offers.
    pub class: Class,
    /// What it drives.
    pub matches: Vec<Match>,
    /// Its memory budget in bytes.
    pub budget: u64,
    /// Its DMA memory in bytes (0: none).
    pub dma: u64,
    /// How many message-signalled interrupts it wants.
    pub irqs: u32,
    /// Its start-up arguments.
    pub args: String,
}

/// What was wrong with a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Longer than [`MAX_SIZE`].
    TooLong,
    /// A line longer than [`MAX_LINE`].
    LineTooLong,
    /// Not ASCII text.
    NotText,
    /// A key that isn't one of the known keys.
    UnknownKey,
    /// A key given twice.
    Repeated,
    /// A key with nothing after it, or something that isn't a number, class or name.
    BadValue,
    /// A number outside what is allowed.
    OutOfRange,
    /// More than [`MAX_MATCHES`] `match` lines.
    TooManyMatches,
    /// No `class`, `budget` or `match`.
    Missing,
}

fn hex(s: &str, max: u32) -> Result<u32, Error> {
    if s.is_empty() || s.len() > 8 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::BadValue);
    }
    let v = u32::from_str_radix(s, 16).map_err(|_| Error::BadValue)?;
    if v > max { Err(Error::OutOfRange) } else { Ok(v) }
}

/// A decimal number with an optional `K` or `M` suffix, as bytes.
fn size(s: &str) -> Result<u64, Error> {
    let (digits, shift) = match s.as_bytes().last() {
        Some(b'K') => (&s[..s.len() - 1], 10),
        Some(b'M') => (&s[..s.len() - 1], 20),
        _ => (s, 0),
    };
    if digits.is_empty() || digits.len() > 12 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::BadValue);
    }
    let n: u64 = digits.parse().map_err(|_| Error::BadValue)?;
    n.checked_mul(1 << shift).ok_or(Error::OutOfRange)
}

fn parse_match(rest: &str) -> Result<Match, Error> {
    let mut w = rest.split_ascii_whitespace();
    let (kind, a) = (w.next().ok_or(Error::BadValue)?, w.next().ok_or(Error::BadValue)?);
    let m = match kind {
        "pci" if a == "class" => {
            let spec = w.next().ok_or(Error::BadValue)?;
            let mut p = spec.split(':');
            let class = hex(p.next().ok_or(Error::BadValue)?, 0xFF)? as u8;
            let subclass = hex(p.next().ok_or(Error::BadValue)?, 0xFF)? as u8;
            let prog_if = p.next().map(|x| hex(x, 0xFF)).transpose()?.map(|x| x as u8);
            if p.next().is_some() {
                return Err(Error::BadValue);
            }
            Match::PciClass { class, subclass, prog_if }
        }
        "pci" => {
            let (v, d) = a.split_once(':').ok_or(Error::BadValue)?;
            Match::Pci { vendor: hex(v, 0xFFFF)? as u16, device: hex(d, 0xFFFF)? as u16 }
        }
        "legacy" => Match::Legacy(match a {
            "com2" => Legacy::Com2,
            "ps2" => Legacy::Ps2,
            "framebuffer" => Legacy::Framebuffer,
            _ => return Err(Error::BadValue),
        }),
        _ => return Err(Error::BadValue),
    };
    if w.next().is_some() {
        return Err(Error::BadValue);
    }
    Ok(m)
}

impl Manifest {
    /// Parses a manifest.
    pub fn parse(text: &[u8]) -> Result<Manifest, Error> {
        if text.len() > MAX_SIZE {
            return Err(Error::TooLong);
        }
        if !text.iter().all(|&b| b == b'\n' || b == b'\t' || (0x20..0x7F).contains(&b)) {
            return Err(Error::NotText);
        }
        let text = core::str::from_utf8(text).map_err(|_| Error::NotText)?;
        let (mut class, mut budget, mut dma, mut irqs, mut args) = (None, None, None, None, None);
        let mut matches = Vec::new();
        for line in text.split('\n') {
            if line.len() > MAX_LINE {
                return Err(Error::LineTooLong);
            }
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = match line.split_once(|c: char| c.is_ascii_whitespace()) {
                Some((k, v)) => (k, v.trim()),
                None => (line, ""),
            };
            if value.is_empty() {
                // Every key needs a value (`dma 0` and `irqs 0` say "none").
                return if matches!(key, "class" | "match" | "budget" | "dma" | "irqs" | "args") {
                    Err(Error::BadValue)
                } else {
                    Err(Error::UnknownKey)
                };
            }
            fn once<T>(slot: &mut Option<T>, v: T) -> Result<(), Error> {
                if slot.is_some() {
                    return Err(Error::Repeated);
                }
                *slot = Some(v);
                Ok(())
            }
            match key {
                "class" => once(
                    &mut class,
                    match value {
                        "block" => Class::Block,
                        "input" => Class::Input,
                        "serial" => Class::Serial,
                        "display" => Class::Display,
                        _ => return Err(Error::BadValue),
                    },
                )?,
                "match" => {
                    if matches.len() >= MAX_MATCHES {
                        return Err(Error::TooManyMatches);
                    }
                    matches.push(parse_match(value)?);
                }
                "budget" => {
                    let v = size(value)?;
                    if !(MIN_BUDGET..=MAX_BUDGET).contains(&v) {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut budget, v)?;
                }
                "dma" => {
                    let v = size(value)?;
                    if v > MAX_DMA || !v.is_multiple_of(4096) {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut dma, v)?;
                }
                "irqs" => {
                    let v = size(value)?;
                    if v > u64::from(MAX_IRQS) || value.ends_with(['K', 'M']) {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut irqs, v as u32)?;
                }
                "args" => {
                    if value.len() > MAX_ARGS {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut args, String::from(value))?;
                }
                _ => return Err(Error::UnknownKey),
            }
        }
        match (class, budget) {
            (Some(class), Some(budget)) if !matches.is_empty() => Ok(Manifest {
                class,
                matches,
                budget,
                dma: dma.unwrap_or(0),
                irqs: irqs.unwrap_or(0),
                args: args.unwrap_or_default(),
            }),
            _ => Err(Error::Missing),
        }
    }
}

/// A device to find a driver for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A PCI function.
    Pci {
        /// Vendor ID.
        vendor: u16,
        /// Device ID.
        device: u16,
        /// Class code.
        class: u8,
        /// Sub-class.
        subclass: u8,
        /// Programming interface.
        prog_if: u8,
    },
    /// A built-in legacy device.
    Legacy(Legacy),
}

/// How well a match fits: an exact vendor and device beats a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    Class,
    Exact,
}

fn fit(m: &Match, t: &Target) -> Option<Fit> {
    match (m, t) {
        (Match::Pci { vendor, device }, Target::Pci { vendor: v, device: d, .. }) => {
            (vendor == v && device == d).then_some(Fit::Exact)
        }
        (
            Match::PciClass { class, subclass, prog_if },
            Target::Pci { class: c, subclass: s, prog_if: p, .. },
        ) => (class == c && subclass == s && prog_if.is_none_or(|x| x == *p)).then_some(Fit::Class),
        (Match::Legacy(l), Target::Legacy(k)) => (l == k).then_some(Fit::Exact),
        _ => None,
    }
}

/// The driver for `target`: the first whose `match` lines fit it exactly, otherwise the first
/// that fits it by class. Returns an index into `manifests`.
pub fn choose(manifests: &[Manifest], target: &Target) -> Option<usize> {
    let mut best: Option<(Fit, usize)> = None;
    for (i, m) in manifests.iter().enumerate() {
        let f = m.matches.iter().filter_map(|x| fit(x, target)).max();
        if let Some(f) = f
            && best.is_none_or(|(b, _)| f > b)
        {
            best = Some((f, i));
        }
    }
    best.map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;
    use alloc::vec;

    const BLK: &str = "# drivers/virtio-blk.drv\nclass block\nmatch pci 1af4:1001\nmatch pci 1AF4:1042 # modern\nbudget 8M\ndma 1M\nirqs 1\nargs --quiet\n";

    #[test]
    fn reads_a_manifest() {
        let m = Manifest::parse(BLK.as_bytes()).unwrap();
        assert_eq!(m.class, Class::Block);
        assert_eq!(
            m.matches,
            [
                Match::Pci { vendor: 0x1af4, device: 0x1001 },
                Match::Pci { vendor: 0x1af4, device: 0x1042 }
            ]
        );
        assert_eq!((m.budget, m.dma, m.irqs), (8 << 20, 1 << 20, 1));
        assert_eq!(m.args, "--quiet");
        let k =
            Manifest::parse(b"class input\nmatch legacy ps2\nmatch pci class 3:0\nbudget 64K\n")
                .unwrap();
        assert_eq!(k.matches[1], Match::PciClass { class: 3, subclass: 0, prog_if: None });
        assert_eq!((k.dma, k.irqs), (0, 0));
        let c = Manifest::parse(b"class input\nmatch pci class c:3:30\nbudget 100K").unwrap();
        assert_eq!(c.matches[0], Match::PciClass { class: 0xc, subclass: 3, prog_if: Some(0x30) });
    }

    #[test]
    fn refuses_the_wrong() {
        let ok = |s: &str| Manifest::parse(s.as_bytes());
        let base = "class block\nmatch pci 1:2\nbudget 1M\n";
        assert!(ok(base).is_ok());
        assert_eq!(ok("match pci 1:2\nbudget 1M\n"), Err(Error::Missing));
        assert_eq!(ok("class block\nbudget 1M\n"), Err(Error::Missing));
        assert_eq!(ok("class block\nmatch pci 1:2\n"), Err(Error::Missing));
        assert_eq!(ok(&format!("{base}colour red\n")), Err(Error::UnknownKey));
        assert_eq!(ok(&format!("{base}class input\n")), Err(Error::Repeated));
        assert_eq!(ok(&format!("{base}budget 2M\n")), Err(Error::Repeated));
        assert_eq!(ok(&format!("{base}dma\n")), Err(Error::BadValue));
        assert_eq!(ok(&format!("{base}dma 5000\n")), Err(Error::OutOfRange));
        assert_eq!(ok(&format!("{base}dma 5M\n")), Err(Error::OutOfRange));
        assert_eq!(ok(&format!("{base}irqs 9\n")), Err(Error::OutOfRange));
        assert_eq!(ok(&format!("{base}irqs 1M\n")), Err(Error::OutOfRange));
        assert_eq!(ok("class block\nmatch pci 1:2\nbudget 1G\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch pci 1:2\nbudget 4K\n"), Err(Error::OutOfRange));
        assert_eq!(
            ok("class block\nmatch pci 1:2\nbudget 999999999999M\n"),
            Err(Error::OutOfRange)
        );
        assert_eq!(ok("class block\nmatch pci 1:2\nbudget 1_0M\n"), Err(Error::BadValue));
        assert_eq!(
            ok("class block\nmatch pci 1:2\nbudget 99999999999999M\n"),
            Err(Error::BadValue)
        );
        assert_eq!(ok("class block\nmatch pci 10000:2\nbudget 1M\n"), Err(Error::OutOfRange));
        assert_eq!(ok("class block\nmatch pci 1\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch pci 1:2 3\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch pci class 1\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch pci class 1:2:3:4\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch legacy fax\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class flying\nmatch pci 1:2\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok("class block\nmatch usb 1:2\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(ok(&format!("{base}args {}\n", "x".repeat(129))), Err(Error::OutOfRange));
        let many = "match pci 1:2\n".repeat(17);
        assert_eq!(ok(&format!("class block\nbudget 1M\n{many}")), Err(Error::TooManyMatches));
        assert_eq!(ok(&format!("{base}{}", "#".repeat(1100))), Err(Error::TooLong));
        assert_eq!(ok(&format!("{base}#{}\n", "x".repeat(200))), Err(Error::LineTooLong));
        assert_eq!(Manifest::parse(b"class block\x00\n"), Err(Error::NotText));
        assert_eq!(Manifest::parse("class bl\u{f6}ck\n".as_bytes()), Err(Error::NotText));
        assert_eq!(Manifest::parse(b""), Err(Error::Missing));
    }

    #[test]
    fn chooses_the_best_fit() {
        let m = |s: &str| Manifest::parse(s.as_bytes()).unwrap();
        let drivers = vec![
            m("class block\nmatch pci class 1:0\nbudget 1M\n"), // any SCSI-class device
            m("class block\nmatch pci 1af4:1042\nbudget 1M\n"),
            m("class input\nmatch legacy ps2\nbudget 1M\n"),
            m("class block\nmatch pci class 1:0\nbudget 2M\n"), // a second class match: loses to the first
        ];
        let virtio =
            Target::Pci { vendor: 0x1af4, device: 0x1042, class: 1, subclass: 0, prog_if: 0 };
        assert_eq!(choose(&drivers, &virtio), Some(1), "exact beats class even if later");
        let other = Target::Pci { vendor: 0x8086, device: 1, class: 1, subclass: 0, prog_if: 0 };
        assert_eq!(choose(&drivers, &other), Some(0), "the first class match wins");
        let none = Target::Pci { vendor: 0x8086, device: 1, class: 2, subclass: 0, prog_if: 0 };
        assert_eq!(choose(&drivers, &none), None);
        assert_eq!(choose(&drivers, &Target::Legacy(Legacy::Ps2)), Some(2));
        assert_eq!(choose(&drivers, &Target::Legacy(Legacy::Com2)), None);
        // A programming interface in a class match has to agree.
        let usb = vec![m("class input\nmatch pci class c:3:30\nbudget 1M\n")];
        let t = |p| Target::Pci { vendor: 1, device: 1, class: 0xc, subclass: 3, prog_if: p };
        assert_eq!(choose(&usb, &t(0x30)), Some(0));
        assert_eq!(choose(&usb, &t(0x20)), None);
    }

    #[test]
    fn random_text_never_panics() {
        let mut rng = Rng(99);
        let base = BLK.as_bytes();
        for _ in 0..5000 {
            let mut b = base.to_vec();
            for _ in 0..1 + rng.below(6) {
                let i = rng.below(b.len() as u64) as usize;
                match rng.below(3) {
                    0 => b[i] = rng.next() as u8,
                    1 => {
                        b.remove(i);
                    }
                    _ => b.insert(i, b"0123456789abcdef:# \nKM"[rng.below(22) as usize]),
                }
            }
            if let Ok(m) = Manifest::parse(&b) {
                assert!(m.dma <= MAX_DMA && m.irqs <= MAX_IRQS && m.matches.len() <= MAX_MATCHES);
                assert!((MIN_BUDGET..=MAX_BUDGET).contains(&m.budget));
            }
        }
    }
}
