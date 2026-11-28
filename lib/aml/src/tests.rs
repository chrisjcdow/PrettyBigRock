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

/// Every real machine's tables, corrupted at random (bytes changed, cut short, stretches repeated), with
/// its checksum put right so it is read: loading, starting up, listing devices and routing
/// must each end, within the limits, without a panic (note 0028, section 10).
#[test]
fn corrupted_tables_fail_cleanly() {
    small_stack(|| {
        let machines = ["roxy-h61mx", "qemu-q35", "qemu-virt"].map(tables);
        let mut rng = Rng(0x5EED_AC91);
        for round in 0..3000 {
            let mut set = machines[round % 3].clone();
            let which = rng.below(set.len());
            let t = &mut set[which];
            match (round / 3) % 3 {
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

/// A machine's namespace started as the ACPI service starts it.
fn started(name: &str) -> Aml<Fake> {
    let mut aml = machine(name, Fake::default());
    if aml.find(None, "\\_PIC").is_some() {
        aml.evaluate("\\_PIC", &[Value::Integer(1)]).unwrap();
    }
    let report = aml.initialise();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    aml
}

/// A device's resources, by its path.
fn resources_of(aml: &mut Aml<Fake>, path: &str) -> Vec<Resource> {
    let devices = aml.devices();
    devices.into_iter().find(|d| d.path == path).unwrap_or_else(|| panic!("no {path}")).resources
}

fn edge(number: u32) -> Resource {
    Resource::Interrupt(Line { number, level: false, active_low: false, shared: false })
}

#[test]
fn q35_legacy_devices() {
    // QEMU's q35, as the boot tests have it: two serial ports, the keyboard controller and
    // its mouse port, each on its ISA line.
    let mut aml = started("qemu-q35");
    let io = |first, count| Resource::Io { first, count };
    assert_eq!(resources_of(&mut aml, "\\_SB.PCI0.SF8.COM1"), [io(0x3F8, 8), edge(4)]);
    assert_eq!(resources_of(&mut aml, "\\_SB.PCI0.SF8.COM2"), [io(0x2F8, 8), edge(3)]);
    assert_eq!(resources_of(&mut aml, "\\_SB.PCI0.SF8.KBD"), [io(0x60, 1), io(0x64, 1), edge(1)]);
    assert_eq!(resources_of(&mut aml, "\\_SB.PCI0.SF8.MOU"), [edge(12)]);
    let ids: Vec<String> = aml.devices().into_iter().map(|d| d.hid).collect();
    for id in ["PNP0501", "PNP0303", "PNP0F13", "PNP0A08"] {
        assert!(ids.iter().any(|h| h == id), "no {id}");
    }
}

#[test]
fn q35_routes_through_links() {
    // q35's `_PRT` names link devices (`GSIA` to `GSIH`) whose `_CRS` says level-triggered
    // and active high (note 0028, section 8): device 1's INTA is GSIF, line 21; device 31's
    // (the chipset's) GSIA, line 16.
    let mut aml = started("qemu-q35");
    let line = |number| Line { number, level: true, active_low: false, shared: true };
    assert_eq!(aml.route(0, 0, 1, 1), Ok(line(21)));
    assert_eq!(aml.route(0, 0, 4, 1), Ok(line(20)));
    assert_eq!(aml.route(0, 0, 31, 1), Ok(line(16)));
    // No bus 9, and no pin 5.
    assert_eq!(aml.route(0, 9, 1, 1), Err(crate::Route::NotFound));
    assert_eq!(aml.route(0, 0, 1, 5), Err(crate::Route::NotFound));
    assert_eq!(aml.sleep_type(5).unwrap(), crate::SleepType { a: 0, b: 0 });
}

#[test]
fn virt_devices_and_routes() {
    // ARM64's virt: the PL011 serial port at 0x09000000 on GIC interrupt 33, the PCI links on
    // 35 to 38, no `\_S5` (it is turned off through the firmware, PSCI).
    let mut aml = started("qemu-virt");
    let level = Line { number: 33, level: true, active_low: false, shared: false };
    assert_eq!(
        resources_of(&mut aml, "\\_SB.COM0"),
        [Resource::Memory { base: 0x0900_0000, length: 0x1000 }, Resource::Interrupt(level)]
    );
    let line = aml.route(0, 0, 1, 1).unwrap();
    assert!((35..=38).contains(&line.number) && line.level && !line.active_low, "{line:?}");
    assert!(aml.sleep_type(5).is_err());
}

/// The cross-check against ACPICA (note 0028, section 10): each machine's tables are loaded by
/// ACPICA's `acpiexec` and by this interpreter, and the same objects evaluated in the same
/// order by both, on the same blank machine (memory and registers reading back what was
/// written, zero otherwise): every device's `_STA`, `_HID`, `_CID`, `_UID`, `_CRS` and `_PRT`,
/// and `\_S5`. The answers must agree. Skipped, saying so, without `acpiexec` (Ubuntu's
/// `acpica-tools`); a stage isn't finished until it has run, and CI installs it.
mod acpica {
    use super::*;
    use std::process::Command;
    use std::string::ToString;

    /// A value as `acpiexec` prints one.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Seen {
        Integer(u64),
        String(String),
        Buffer(Vec<u8>),
        Package(Vec<Seen>),
        /// A reference to a named object: its last name segment.
        Reference(String),
        /// Anything else, as printed.
        Other(String),
    }

    /// What our interpreter gives, in the same form.
    pub fn seen(v: &Value, aml: &Aml<Fake>) -> Seen {
        use crate::value::{Ref, Reference};
        match v {
            Value::Integer(i) => Seen::Integer(*i),
            Value::String(s) => Seen::String(s.clone()),
            Value::Buffer(b) => Seen::Buffer(b.borrow().clone()),
            Value::Package(p) => Seen::Package(p.borrow().0.iter().map(|e| seen(e, aml)).collect()),
            Value::Reference(Reference(Ref::Node(n))) => Seen::Reference(
                aml.path(*n).rsplit('.').next().unwrap_or("").trim_start_matches('\\').to_string(),
            ),
            Value::Reference(Reference(Ref::Name(_, name))) => {
                Seen::Reference(name.to_string().rsplit('.').next().unwrap_or("").to_string())
            }
            other => Seen::Other(std::format!("{other:?}")),
        }
    }

    /// Reads one printed value starting at `lines[*i]`.
    fn value(lines: &[&str], i: &mut usize) -> Option<Seen> {
        let line = lines.get(*i)?.trim();
        *i += 1;
        if let Some(hex) = line.strip_prefix("[Integer] = ") {
            return u64::from_str_radix(hex.trim(), 16).ok().map(Seen::Integer);
        }
        if let Some(rest) = line.strip_prefix("[String] Length ") {
            let s = rest.split_once(" = \"")?.1.strip_suffix('"')?;
            return Some(Seen::String(s.to_string()));
        }
        if let Some(rest) = line.strip_prefix("[Buffer] Length ") {
            let (len, rest) = rest.split_once(" =")?;
            let len = usize::from_str_radix(len.trim(), 16).ok()?;
            let mut bytes = Vec::new();
            let take = |text: &str, bytes: &mut Vec<u8>| {
                let hex = text.split("//").next().unwrap_or("");
                let Some((_, hex)) = hex.split_once(": ") else { return };
                for b in hex.split_whitespace() {
                    if bytes.len() < len {
                        bytes.extend(u8::from_str_radix(b, 16).ok());
                    }
                }
            };
            take(rest, &mut bytes);
            while bytes.len() < len && *i < lines.len() {
                take(lines[*i], &mut bytes);
                *i += 1;
            }
            return Some(Seen::Buffer(bytes));
        }
        if let Some(rest) = line.strip_prefix("[Package] Contains ") {
            let n: usize = rest.split_whitespace().next()?.parse().ok()?;
            let mut out = Vec::new();
            for _ in 0..n {
                out.push(value(lines, i)?);
            }
            return Some(Seen::Package(out));
        }
        if line.starts_with("[Object Reference]") {
            let name = line.split(" Name ").nth(1)?.split_whitespace().next()?;
            return Some(Seen::Reference(name.trim_end_matches('_').to_string()));
        }
        Some(Seen::Other(line.to_string()))
    }

    /// Runs `acpiexec` on a machine's tables with these evaluations; each one's answer, or its
    /// failure status. `None` if `acpiexec` can't be run.
    pub fn run(machine: &str, paths: &[String]) -> Option<BTreeMap<String, Result<Seen, String>>> {
        let dir = super::data(machine);
        let mut tables = alloc::vec![dir.join("DSDT")];
        for i in 1.. {
            let p = dir.join(std::format!("SSDT{i}"));
            if !p.exists() {
                break;
            }
            tables.push(p);
        }
        run_tables(&tables, paths)
    }

    /// The same for one table in a file.
    pub fn run_file(
        table: &std::path::Path,
        paths: &[String],
    ) -> Option<BTreeMap<String, Result<Seen, String>>> {
        run_tables(&[table.to_path_buf()], paths)
    }

    fn run_tables(
        tables: &[PathBuf],
        paths: &[String],
    ) -> Option<BTreeMap<String, Result<Seen, String>>> {
        // The commands go in on standard input: `-b` ignores a batch longer than about 1000
        // characters.
        use std::io::Write;
        let mut script: String = paths.iter().map(|p| std::format!("evaluate {p}\n")).collect();
        script.push_str("quit\n");
        let mut child = Command::new("acpiexec")
            .args(tables)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(script.as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = text.lines().collect();
        let mut answers = BTreeMap::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            i += 1;
            let Some(rest) = line.strip_prefix("Evaluation of ") else { continue };
            let Some((path, how)) = rest.split_once(' ') else { continue };
            let answer = if how.starts_with("returned object") {
                match value(&lines, &mut i) {
                    Some(v) => Ok(v),
                    None => Err("unreadable".to_string()),
                }
            } else if let Some(status) = how.strip_prefix("failed with status ") {
                Err(status.trim().to_string())
            } else {
                Err(how.to_string())
            };
            answers.insert(path.to_string(), answer);
        }
        Some(answers)
    }
}

/// See [`acpica`].
#[test]
fn agrees_with_acpica() {
    if std::process::Command::new("acpiexec").arg("-v").output().is_err() {
        std::println!("agrees_with_acpica: skipped, acpiexec isn't installed (acpica-tools)");
        return;
    }
    for name in ["roxy-h61mx", "qemu-q35", "qemu-virt"] {
        let mut aml = machine(name, Fake::default());
        let report = aml.initialise();
        assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
        // Every device's methods that exist, and \_S5, in namespace order.
        let mut paths = Vec::new();
        for (path, kind) in aml.listing() {
            if kind == 6 {
                for m in ["_STA", "_HID", "_CID", "_UID", "_CRS", "_PRT"] {
                    let p = std::format!("{path}.{m}");
                    if aml.find(None, &p).is_some() {
                        paths.push(p);
                    }
                }
            }
        }
        if aml.find(None, "\\_S5").is_some() {
            paths.push(String::from("\\_S5"));
        }
        let theirs = acpica::run(name, &paths).expect("acpiexec ran");
        let mut compared = 0;
        let mut differ = Vec::new();
        for p in &paths {
            let ours = aml.evaluate(p, &[]).map(|v| acpica::seen(&v, &aml));
            match (theirs.get(p), &ours) {
                // ACPICA's debugger can't print an answer bigger than its buffer.
                (Some(Err(status)), _) if status.contains("BUFFER_OVERFLOW") => continue,
                (Some(Ok(t)), Ok(o)) if t == o => compared += 1,
                (Some(Err(_)), Err(_)) => compared += 1,
                (t, o) => differ.push(std::format!("{p}: ACPICA {t:?}, ours {o:?}")),
            }
        }
        std::println!("{name}: {compared} of {} evaluations agree with ACPICA", paths.len());
        assert!(differ.is_empty(), "{name}: {} differ:\n{}", differ.len(), differ.join("\n"));
        assert!(compared > 50, "{name}: only {compared} compared");
    }
}

/// The small programs in `lib/aml/tests/asl/` (note 0028, section 10): ASL sources, each with
/// its AML beside it, compiled by `iasl -of` (constant folding off, so the arithmetic is left
/// for the interpreter). Every method named `T` and three digits checks one part of the
/// language and returns 0, or the number of the check that failed; each must return 0 here,
/// and give what ACPICA gives when `acpiexec` is installed.
#[test]
fn small_programs() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/asl");
    let mut names: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "aml"))
        .collect();
    names.sort();
    assert!(names.len() >= 4, "the programs: {names:?}");
    let acpiexec = std::process::Command::new("acpiexec").arg("-v").output().is_ok();
    let mut checked = 0;
    for path in &names {
        let t = fs::read(path).unwrap();
        let mut aml = Aml::new(Fake::default());
        let report = aml.load(&[&t]);
        assert!(report.skipped.is_empty(), "{path:?}: {:?}", report.skipped);
        let tests: Vec<String> = aml
            .listing()
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| {
                p.len() == 5 && p.starts_with("\\T") && p[2..].bytes().all(|b| b.is_ascii_digit())
            })
            .collect();
        assert!(!tests.is_empty(), "{path:?} has no tests");
        let mut ours = Vec::new();
        for t in &tests {
            let r = aml.evaluate(t, &[]);
            assert_eq!(r.as_ref().ok().and_then(Value::as_integer), Some(0), "{path:?} {t}: {r:?}");
            ours.push(r.map(|v| acpica::seen(&v, &aml)));
            checked += 1;
        }
        if acpiexec {
            let answers = acpica::run_file(path, &tests).expect("acpiexec ran");
            for (t, o) in tests.iter().zip(&ours) {
                let theirs = answers.get(t).cloned();
                assert_eq!(
                    theirs,
                    Some(o.clone().map_err(|e| std::format!("{e}"))),
                    "{path:?} {t}: ACPICA disagrees"
                );
            }
        }
    }
    std::println!(
        "small programs: {checked} tests in {} files{}",
        names.len(),
        if acpiexec { ", as ACPICA" } else { "" }
    );
}
