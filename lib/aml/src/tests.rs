// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests over real machines' tables (`tests/data/acpi/`) and small programs.

use crate::resource::{Line, Resource};
use crate::{Aml, Host, PciAddress, Refused, Value};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use std::fs;
use std::path::PathBuf;

/// A machine that remembers what was written to it: memory, ports and configuration space
/// read back what was last written there, and 0 (or all ones for an absent PCI function's
/// identity) otherwise.
#[derive(Default)]
pub(crate) struct Fake {
    pub memory: BTreeMap<u64, u8>,
    pub io: BTreeMap<u16, u8>,
    pub pci: BTreeMap<(PciAddress, u16), u8>,
    pub log: Vec<String>,
    pub clock: u64,
    /// Ports that refuse access.
    pub refused: Vec<u16>,
    /// Every port access, when tracing: (port, width, written value or `None` for a read).
    pub trace: Option<Vec<(u16, u8, Option<u64>)>>,
    /// A board I/O chip at ports `0x2E` and `0x2F`, if there is one.
    pub superio: Option<SuperIo>,
}

/// A board I/O chip as ITE's work: write a register's number to the index port (`0x2E`),
/// then read or write it at the data port (`0x2F`). Register 7 chooses the logical device
/// (each of the chip's serial ports, its keyboard controller and so on); registers from
/// `0x30` up are that device's own.
#[derive(Default)]
pub(crate) struct SuperIo {
    index: u8,
    global: BTreeMap<u8, u8>,
    devices: BTreeMap<(u8, u8), u8>,
}

impl SuperIo {
    /// Roxy's chip as Linux found it: the second serial port (logical device 2) on, at
    /// `0x2F8`, interrupt 3.
    pub fn roxy() -> SuperIo {
        let mut s = SuperIo::default();
        for (reg, v) in [(0x30, 1), (0x60, 0x02), (0x61, 0xF8), (0x70, 3), (0x74, 4)] {
            s.devices.insert((2, reg), v);
        }
        s
    }

    fn read(&mut self, port: u16) -> u8 {
        if port == 0x2E {
            return self.index;
        }
        let ldn = *self.global.get(&7).unwrap_or(&0);
        match self.index {
            i if i < 0x30 => *self.global.get(&i).unwrap_or(&0),
            i => *self.devices.get(&(ldn, i)).unwrap_or(&0),
        }
    }

    fn write(&mut self, port: u16, v: u8) {
        if port == 0x2E {
            self.index = v;
            return;
        }
        let ldn = *self.global.get(&7).unwrap_or(&0);
        match self.index {
            i if i < 0x30 => {
                self.global.insert(i, v);
            }
            i => {
                self.devices.insert((ldn, i), v);
            }
        }
    }
}

impl Fake {
    fn get<K: Ord + Copy>(map: &BTreeMap<K, u8>, at: impl Fn(u64) -> K, width: u8) -> u64 {
        (0..u64::from(width / 8))
            .map(|i| u64::from(*map.get(&at(i)).unwrap_or(&0)) << (8 * i))
            .sum()
    }
    fn put<K: Ord + Copy>(map: &mut BTreeMap<K, u8>, at: impl Fn(u64) -> K, width: u8, v: u64) {
        for i in 0..u64::from(width / 8) {
            map.insert(at(i), (v >> (8 * i)) as u8);
        }
    }
}

impl Host for Fake {
    fn read_memory(&mut self, address: u64, width: u8) -> Result<u64, Refused> {
        Ok(Fake::get(&self.memory, |i| address.wrapping_add(i), width))
    }
    fn write_memory(&mut self, address: u64, width: u8, value: u64) -> Result<(), Refused> {
        Fake::put(&mut self.memory, |i| address.wrapping_add(i), width, value);
        Ok(())
    }
    fn read_io(&mut self, port: u16, width: u8) -> Result<u64, Refused> {
        if self.refused.contains(&port) {
            return Err(Refused);
        }
        if let Some(t) = &mut self.trace {
            t.push((port, width, None));
        }
        if let (Some(sio), 0x2E | 0x2F, 8) = (&mut self.superio, port, width) {
            return Ok(u64::from(sio.read(port)));
        }
        Ok(Fake::get(&self.io, |i| port.wrapping_add(i as u16), width))
    }
    fn write_io(&mut self, port: u16, width: u8, value: u64) -> Result<(), Refused> {
        if self.refused.contains(&port) {
            return Err(Refused);
        }
        if let Some(t) = &mut self.trace {
            t.push((port, width, Some(value)));
        }
        if let (Some(sio), 0x2E | 0x2F, 8) = (&mut self.superio, port, width) {
            sio.write(port, value as u8);
            return Ok(());
        }
        Fake::put(&mut self.io, |i| port.wrapping_add(i as u16), width, value);
        Ok(())
    }
    fn read_pci(&mut self, at: PciAddress, offset: u16, width: u8) -> Result<u64, Refused> {
        Ok(Fake::get(&self.pci, |i| (at, offset.wrapping_add(i as u16)), width))
    }
    fn write_pci(
        &mut self,
        at: PciAddress,
        offset: u16,
        width: u8,
        value: u64,
    ) -> Result<(), Refused> {
        Fake::put(&mut self.pci, |i| (at, offset.wrapping_add(i as u16)), width, value);
        Ok(())
    }
    fn now(&mut self) -> u64 {
        self.clock += 1000;
        self.clock
    }
    fn sleep(&mut self, ms: u64) {
        self.clock += ms * 1_000_000;
    }
    fn stall(&mut self, us: u64) {
        self.clock += us * 1000;
    }
    fn log(&mut self, _verbose: bool, message: core::fmt::Arguments) {
        self.log.push(std::format!("{message}"));
    }
}

/// A folder of `tests/data/acpi/`.
fn data(machine: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/acpi").join(machine)
}

/// A machine's DSDT and SSDTs, in the order its firmware lists them.
pub(crate) fn tables(machine: &str) -> Vec<Vec<u8>> {
    let dir = data(machine);
    let mut out = alloc::vec![fs::read(dir.join("DSDT")).unwrap()];
    for i in 1.. {
        match fs::read(dir.join(std::format!("SSDT{i}"))) {
            Ok(t) => out.push(t),
            Err(_) => break,
        }
    }
    out
}

/// Loads and starts a machine's namespace.
pub(crate) fn machine(name: &str, host: Fake) -> Aml<Fake> {
    let t = tables(name);
    let refs: Vec<&[u8]> = t.iter().map(|t| t.as_slice()).collect();
    let mut aml = Aml::new(host);
    let report = aml.load(&refs);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    aml
}

/// Roxy started as the ACPI service starts it: `\_PIC(1)`, then the devices.
fn roxy() -> Aml<Fake> {
    let host = Fake { superio: Some(SuperIo::roxy()), ..Fake::default() };
    let mut aml = machine("roxy-h61mx", host);
    aml.evaluate("\\_PIC", &[Value::Integer(1)]).unwrap();
    let report = aml.initialise();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    aml
}

#[test]
fn roxy_serial_port() {
    let mut aml = roxy();
    let devices = aml.devices();
    let com2 = devices.iter().find(|d| d.path == "\\_SB.PCI0.LPCB.UAR2").unwrap();
    assert_eq!(com2.hid, "PNP0501");
    let line = Line { number: 3, level: false, active_low: false, shared: false };
    assert_eq!(
        com2.resources,
        [Resource::Io { first: 0x2F8, count: 8 }, Resource::Interrupt(line)]
    );
}

#[test]
fn roxy_usb_lines() {
    // Linux's `/proc/interrupts`: the EHCI controllers at 00:1a.0 and 00:1d.0 on lines 16 and
    // 23, level-triggered (and active low, as lines named directly are).
    let mut aml = roxy();
    for (device, number) in [(0x1A, 16), (0x1D, 23)] {
        let line = aml.route(0, 0, device, 1).unwrap();
        assert_eq!(line, Line { number, level: true, active_low: true, shared: true });
    }
}

#[test]
fn roxy_off() {
    let mut aml = roxy();
    let s5 = aml.sleep_type(5).unwrap();
    std::println!("{s5:?}");
}

/// AML put together by hand, for programs too small to need `iasl`, or too broken for it to
/// write.
mod asm {
    use alloc::vec::Vec;

    /// `body` with a package length in front (which counts itself).
    pub fn pkg(body: &[u8]) -> Vec<u8> {
        let l = body.len();
        let mut out = if l + 1 < 0x40 {
            alloc::vec![(l + 1) as u8]
        } else if l + 2 < 1 << 12 {
            let t = l + 2;
            alloc::vec![0x40 | (t & 0xF) as u8, (t >> 4) as u8]
        } else {
            let t = l + 3;
            assert!(t < 1 << 20);
            alloc::vec![0x80 | (t & 0xF) as u8, (t >> 4) as u8, (t >> 12) as u8]
        };
        out.extend_from_slice(body);
        out
    }

    /// An opcode with a package length around the rest.
    pub fn op(opcode: &[u8], rest: &[u8]) -> Vec<u8> {
        let mut out = opcode.to_vec();
        out.extend(pkg(rest));
        out
    }

    /// `Method (NAME, args) { body }`.
    pub fn method(name: &[u8; 4], args: u8, body: &[u8]) -> Vec<u8> {
        let mut rest = name.to_vec();
        rest.push(args);
        rest.extend_from_slice(body);
        op(&[0x14], &rest)
    }

    /// A DSDT of revision 2 around `body`, its checksum set.
    pub fn table(body: &[u8]) -> Vec<u8> {
        let mut t = b"DSDT".to_vec();
        t.extend(((36 + body.len()) as u32).to_le_bytes());
        t.push(2);
        t.push(0);
        t.extend_from_slice(b"PBR   TESTTEST");
        t.extend(1u32.to_le_bytes());
        t.extend_from_slice(b"PBR ");
        t.extend(1u32.to_le_bytes());
        t.extend_from_slice(body);
        fix_checksum(&mut t);
        t
    }

    /// Sets byte 9 so the table's bytes sum to zero.
    pub fn fix_checksum(t: &mut [u8]) {
        if t.len() < 10 {
            return;
        }
        t[9] = 0;
        let sum = t.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        t[9] = 0u8.wrapping_sub(sum);
    }

    pub const ONE: u8 = 0x01;
    pub const ZERO: u8 = 0x00;
    pub const LOCAL0: u8 = 0x60;
    pub const STORE: u8 = 0x70;
    pub const ADD: u8 = 0x72;
    pub const CONCAT: u8 = 0x73;
    pub const RETURN: u8 = 0xA4;
    pub const WHILE: u8 = 0xA2;
}

/// Loads one hand-made table into an empty machine.
fn program(body: &[u8]) -> Aml<Fake> {
    let t = asm::table(body);
    let mut aml = Aml::new(Fake::default());
    let report = aml.load(&[&t]);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    aml
}

/// Runs `f` on a thread with a 2 MiB stack, half the ACPI service's, so the tests show the
/// limits stop the interpreter's recursion well before the service's stack would run out.
/// (Built optimised, as the workspace builds this library, one level of an expression takes
/// about 1.1 KiB, so the 1024 levels of [`crate::limits::NESTING`] need about 1.2 MiB.)
fn small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(2 << 20).spawn(f).unwrap().join().unwrap();
}

#[test]
fn an_endless_loop_stops() {
    use asm::*;
    // Method (LOOP) { While (One) { } }
    let mut aml = program(&method(b"LOOP", 0, &op(&[WHILE], &[ONE])));
    aml.operations = 100_000;
    assert_eq!(aml.evaluate("\\LOOP", &[]).err(), Some(crate::Error::Limit("operations")));
}

#[test]
fn endless_recursion_stops() {
    small_stack(|| {
        // Method (RECU) { RECU () }
        let mut aml = program(&asm::method(b"RECU", 0, b"RECU"));
        let r = aml.evaluate("\\RECU", &[]);
        assert!(matches!(r, Err(crate::Error::Limit(_))), "{r:?}");
    });
}

#[test]
fn deep_expressions_stop() {
    small_stack(|| {
        use asm::*;
        // Method (DEEP) { Return (Add (Add (Add (… One …, One), One), One)) }, 5000 deep.
        let mut body = alloc::vec![RETURN];
        body.extend(core::iter::repeat_n(ADD, 5000));
        body.push(ONE);
        for _ in 0..5000 {
            body.extend([ONE, ZERO]);
        }
        let mut aml = program(&method(b"DEEP", 0, &body));
        assert_eq!(aml.evaluate("\\DEEP", &[]).err(), Some(crate::Error::Limit("nesting")));
        // And a deep one within the limit works.
        let mut body = alloc::vec![RETURN];
        body.extend(core::iter::repeat_n(ADD, 500));
        body.push(ONE);
        for _ in 0..500 {
            body.extend([ONE, ZERO]);
        }
        let mut aml = program(&method(b"DEEP", 0, &body));
        assert_eq!(aml.evaluate("\\DEEP", &[]).unwrap().as_integer(), Some(501));
    });
}

#[test]
fn huge_objects_are_refused() {
    use asm::*;
    // Method (BIGB) { Return (Buffer (0xFFFFFFFF) { }) }
    let mut buffer = alloc::vec![0x0C];
    buffer.extend(0xFFFF_FFFFu32.to_le_bytes());
    let mut body = alloc::vec![RETURN];
    body.extend(op(&[0x11], &buffer));
    let mut aml = program(&method(b"BIGB", 0, &body));
    assert_eq!(aml.evaluate("\\BIGB", &[]).err(), Some(crate::Error::Limit("a buffer's length")));

    // Method (GROW) { Local0 = "x"; While (One) { Local0 = Concatenate (Local0, Local0) } }
    let mut body = alloc::vec![STORE, 0x0D, b'x', 0, LOCAL0];
    body.extend(op(&[WHILE], &[ONE, CONCAT, LOCAL0, LOCAL0, LOCAL0]));
    let mut aml = program(&method(b"GROW", 0, &body));
    let r = aml.evaluate("\\GROW", &[]);
    assert!(matches!(r, Err(crate::Error::Limit(_))), "{r:?}");
}

#[test]
fn nested_packages_are_limited_and_freed() {
    small_stack(|| {
        use asm::*;
        // Method (NEST) {
        //     Local0 = Package (1) { }
        //     While (One) { Local1 = Package (1) { }; Local1 [0] = Local0; Local0 = Local1 }
        // }
        // Each store into the element copies the package, so the copy's depth limit ends it.
        let mut body = alloc::vec![STORE];
        body.extend(op(&[0x12], &[1]));
        body.push(LOCAL0);
        let mut inner = alloc::vec![ONE, STORE];
        inner.extend(op(&[0x12], &[1]));
        inner.push(0x61);
        inner.extend([STORE, LOCAL0, 0x88, 0x61, ZERO, ZERO]);
        inner.extend([STORE, 0x61, LOCAL0]);
        body.extend(op(&[WHILE], &inner));
        let mut aml = program(&method(b"NEST", 0, &body));
        let r = aml.evaluate("\\NEST", &[]);
        assert_eq!(r.err(), Some(crate::Error::Limit("packages nested too deeply")));
    });
}

#[test]
fn a_method_s_names_go_with_it() {
    use asm::*;
    // Method (MKNM) { Name (TEMP, 5); Return (TEMP) }, called twice.
    let mut body = alloc::vec![0x08];
    body.extend(b"TEMP");
    body.extend([0x0A, 5, RETURN]);
    body.extend(b"TEMP");
    let mut aml = program(&method(b"MKNM", 0, &body));
    for _ in 0..2 {
        assert_eq!(aml.evaluate("\\MKNM", &[]).unwrap().as_integer(), Some(5));
    }
    assert!(aml.find(None, "\\MKNM.TEMP").is_none());
}

#[test]
fn fields_write_only_their_bits() {
    use asm::*;
    // OperationRegion (PORT, SystemIO, 0x80, 2)
    // Field (PORT, ByteAcc, NoLock, Preserve) { LOW, 4, HIGH, 4, WORD, 8 }
    // Method (SETH) { HIGH = 0xA }
    let mut body = alloc::vec![0x5B, 0x80];
    body.extend(b"PORT");
    body.extend([1, 0x0A, 0x80, 0x0A, 2]);
    let mut list = b"PORT".to_vec();
    list.push(0x01); // byte access, no lock, preserve
    list.extend(b"LOW_");
    list.push(4);
    list.extend(b"HIGH");
    list.push(4);
    list.extend(b"WORD");
    list.push(8);
    body.extend(op(&[0x5B, 0x81], &list));
    let mut seth = alloc::vec![STORE, 0x0A, 0x0A];
    seth.extend(b"HIGH");
    body.extend(method(b"SETH", 0, &seth));
    let mut aml = program(&body);
    aml.host().io.insert(0x80, 0x05);
    aml.evaluate("\\SETH", &[]).unwrap();
    assert_eq!(aml.host().io.get(&0x80), Some(&0xA5));
    assert_eq!(aml.evaluate("\\LOW", &[]).unwrap().as_integer(), Some(5));
    assert_eq!(aml.evaluate("\\HIGH", &[]).unwrap().as_integer(), Some(0xA));
    // A refused port reads as zero and ignores writes, and is logged once.
    aml.host().refused.push(0x81);
    assert_eq!(aml.evaluate("\\WORD", &[]).unwrap().as_integer(), Some(0));
    assert_eq!(aml.evaluate("\\WORD", &[]).unwrap().as_integer(), Some(0));
    let refusals = aml.host().log.iter().filter(|l| l.contains("refused")).count();
    assert_eq!(refusals, 1);
}

/// A small random number generator (xorshift), so the corruption tests are the same on every
/// run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Every real table, corrupted at random (bytes changed, cut short, stretches repeated), with
/// its checksum put right so it is read: loading, starting up, listing devices and routing
/// must each end, within the limits, without a panic (note 0028, section 10).
#[test]
fn corrupted_tables_fail_cleanly() {
    small_stack(|| {
        let originals = tables("roxy-h61mx");
        let mut rng = Rng(0x5EED_AC91);
        for round in 0..3000 {
            let mut set = originals.clone();
            let which = rng.below(set.len());
            let t = &mut set[which];
            match round % 3 {
                0 => {
                    for _ in 0..1 + rng.below(20) {
                        let i = 36 + rng.below(t.len() - 36);
                        t[i] = rng.next() as u8;
                    }
                }
                1 => {
                    let keep = 36 + rng.below(t.len() - 36);
                    t.truncate(keep);
                    let l = (t.len() as u32).to_le_bytes();
                    t[4..8].copy_from_slice(&l);
                }
                _ => {
                    let from = 36 + rng.below(t.len() - 36);
                    let len = 1 + rng.below(64).min(t.len() - from - 1);
                    let stretch = t[from..from + len].to_vec();
                    let at = 36 + rng.below(t.len() - 36);
                    t.splice(at..at, stretch);
                    let l = (t.len() as u32).to_le_bytes();
                    t[4..8].copy_from_slice(&l);
                }
            }
            asm::fix_checksum(t);
            let refs: Vec<&[u8]> = set.iter().map(|t| t.as_slice()).collect();
            let host = Fake { superio: Some(SuperIo::roxy()), ..Fake::default() };
            let mut aml = Aml::new(host);
            aml.operations = 20_000;
            let _ = aml.load(&refs);
            let _ = aml.evaluate("\\_PIC", &[Value::Integer(1)]);
            let _ = aml.initialise();
            let _ = aml.devices();
            let _ = aml.route(0, 0, 0x1A, 1);
            let _ = aml.sleep_type(5);
        }
    });
}
