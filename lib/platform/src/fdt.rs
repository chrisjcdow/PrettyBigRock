// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Just enough of the flattened device tree format to find a device's registers. The kernel
//! will have a full reader; the loader only needs the serial port.

const MAGIC: u32 = 0xD00D_FEED;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const NOP: u32 = 4;
const END: u32 = 9;
const MAX_DEPTH: usize = 16;

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

/// The size of the device tree whose header is in `header` (at least 8 bytes), or `None` if it
/// isn't one.
pub fn total_size(header: &[u8]) -> Option<u32> {
    (be32(header, 0)? == MAGIC).then(|| be32(header, 4)).flatten()
}

fn cstr(b: &[u8], o: usize) -> Option<&[u8]> {
    let rest = b.get(o..)?;
    let n = rest.iter().position(|&c| c == 0)?;
    Some(&rest[..n])
}

#[derive(Clone, Copy)]
struct Node {
    addr_cells: u32,
    size_cells: u32,
    compatible: bool,
    disabled: bool,
    reg: Option<u64>,
}

const DEFAULT: Node =
    Node { addr_cells: 2, size_cells: 1, compatible: false, disabled: false, reg: None };

/// Finds the first enabled node whose `compatible` list includes `compat`, and returns the
/// first address in its `reg` property.
pub fn find_compatible(fdt: &[u8], compat: &str) -> Option<u64> {
    total_size(fdt)?;
    let structs = be32(fdt, 8)? as usize;
    let strings = be32(fdt, 12)? as usize;
    let mut stack = [DEFAULT; MAX_DEPTH];
    let mut depth = 0usize; // number of open nodes
    let mut o = structs;
    loop {
        let token = be32(fdt, o)?;
        o += 4;
        match token {
            BEGIN_NODE => {
                let name = cstr(fdt, o)?;
                o += (name.len() + 4) & !3;
                if depth >= MAX_DEPTH {
                    return None;
                }
                stack[depth] = DEFAULT;
                depth += 1;
            }
            END_NODE => {
                let n = stack[depth.checked_sub(1)?];
                if n.compatible && !n.disabled && n.reg.is_some() {
                    return n.reg;
                }
                depth -= 1;
            }
            PROP => {
                let len = be32(fdt, o)? as usize;
                let name = cstr(fdt, strings + be32(fdt, o + 4)? as usize)?;
                let value = fdt.get(o + 8..o + 8 + len)?;
                o += 8 + ((len + 3) & !3);
                let cur = depth.checked_sub(1)?;
                let parent = if cur > 0 { stack[cur - 1] } else { DEFAULT };
                let node = &mut stack[cur];
                match name {
                    b"#address-cells" => node.addr_cells = be32(value, 0)?,
                    b"#size-cells" => node.size_cells = be32(value, 0)?,
                    b"compatible" => {
                        node.compatible = value.split(|&c| c == 0).any(|s| s == compat.as_bytes())
                    }
                    b"status" => {
                        node.disabled = !value.starts_with(b"okay") && !value.starts_with(b"ok\0")
                    }
                    b"reg" => {
                        let mut a = 0u64;
                        for i in 0..parent.addr_cells as usize {
                            a = (a << 32) | u64::from(be32(value, i * 4)?);
                        }
                        node.reg = Some(a);
                    }
                    _ => {}
                }
            }
            NOP => {}
            END => return None,
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny device tree builder for the tests.
    struct Builder {
        structs: Vec<u8>,
        strings: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self { structs: vec![], strings: vec![] }
        }
        fn begin(&mut self, name: &str) {
            self.structs.extend_from_slice(&BEGIN_NODE.to_be_bytes());
            self.structs.extend_from_slice(name.as_bytes());
            self.structs.push(0);
            while !self.structs.len().is_multiple_of(4) {
                self.structs.push(0);
            }
        }
        fn end(&mut self) {
            self.structs.extend_from_slice(&END_NODE.to_be_bytes());
        }
        fn prop(&mut self, name: &str, value: &[u8]) {
            let off = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.structs.extend_from_slice(&PROP.to_be_bytes());
            self.structs.extend_from_slice(&(value.len() as u32).to_be_bytes());
            self.structs.extend_from_slice(&off.to_be_bytes());
            self.structs.extend_from_slice(value);
            while !self.structs.len().is_multiple_of(4) {
                self.structs.push(0);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            self.structs.extend_from_slice(&END.to_be_bytes());
            let mut f = vec![0u8; 40];
            let soff = 40 + self.structs.len();
            f.extend_from_slice(&self.structs);
            f.extend_from_slice(&self.strings);
            let total = f.len() as u32;
            f[0..4].copy_from_slice(&MAGIC.to_be_bytes());
            f[4..8].copy_from_slice(&total.to_be_bytes());
            f[8..12].copy_from_slice(&40u32.to_be_bytes());
            f[12..16].copy_from_slice(&(soff as u32).to_be_bytes());
            f
        }
    }

    fn cells(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    #[test]
    fn finds_the_uart() {
        let mut b = Builder::new();
        b.begin("");
        b.prop("#address-cells", &cells(&[2]));
        b.prop("#size-cells", &cells(&[2]));
        b.begin("pl011@8000000");
        b.prop("compatible", b"arm,pl011\0arm,primecell\0");
        b.prop("status", b"disabled\0");
        b.prop("reg", &cells(&[0, 0x0800_0000, 0, 0x1000]));
        b.end();
        b.begin("soc");
        b.prop("#address-cells", &cells(&[1]));
        b.begin("pl011@9000000");
        b.prop("compatible", b"arm,pl011\0arm,primecell\0");
        b.prop("reg", &cells(&[0x0900_0000, 0x1000]));
        b.end();
        b.end();
        b.end();
        let fdt = b.finish();
        assert_eq!(total_size(&fdt), Some(fdt.len() as u32));
        assert_eq!(find_compatible(&fdt, "arm,pl011"), Some(0x0900_0000));
        assert_eq!(find_compatible(&fdt, "ns16550a"), None);
        assert_eq!(total_size(b"\0\0\0\0\0\0\0\0"), None);
    }
}
