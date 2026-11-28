// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The questions the ACPI service asks (note 0028, sections 2, 3 and 6): loading the tables,
//! starting the namespace up, listing the devices it describes, where a PCI interrupt pin is
//! wired, and how to turn the machine off.

use crate::namespace::{NodeId, Object};
use crate::resource::{self, Line, Resource};
use crate::value::{Ref, Reference, Value};
use crate::{Aml, AmlName, Error, Host, PciAddress, Result};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// What loading the tables did.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Tables loaded.
    pub tables: usize,
    /// Tables skipped, and why.
    pub skipped: Vec<String>,
}

/// What starting the namespace up did.
#[derive(Debug, Default)]
pub struct InitReport {
    /// Devices (and processors and thermal zones) found.
    pub devices: usize,
    /// Of those, the ones present.
    pub present: usize,
    /// `_INI`, `_REG` and `_STA` methods that failed, and how.
    pub failed: Vec<String>,
}

/// A device the namespace describes (note 0028, section 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Its node.
    pub node: NodeId,
    /// Its full name, such as `\_SB.PCI0.LPCB.UAR2`.
    pub path: String,
    /// `_HID`, as text (an EISA number converted, as `PNP0501`).
    pub hid: String,
    /// `_CID`, its compatible IDs.
    pub compatible: Vec<String>,
    /// `_UID`, if it has one, as text.
    pub uid: Option<String>,
    /// What `_CRS` returned, decoded; empty without one.
    pub resources: Vec<Resource>,
    /// Why `_CRS` couldn't be had, if it couldn't.
    pub crs_error: Option<String>,
}

/// Why a pin has no route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// No bridge for that bus, no `_PRT` entry for the pin, or a link without a line set.
    NotFound,
    /// The firmware's method failed (the message says how).
    Failed(String),
}

/// The two values `\_S5` (or another sleep state) gives, for PM1a and PM1b control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepType {
    /// For PM1a control's `SLP_TYP`.
    pub a: u8,
    /// For PM1b control's.
    pub b: u8,
}

/// The IDs of PCI host bridges: conventional PCI and PCI Express.
const HOST_BRIDGES: [&str; 2] = ["PNP0A03", "PNP0A08"];

/// Turns an EISA ID, packed into an integer, into its text (ACPI 6.5, section 19.3.4): three
/// letters of five bits each and four hexadecimal digits, the bytes in the order written.
pub fn eisa_id(v: u64) -> String {
    let v = (v as u32).swap_bytes();
    let letter = |shift: u32| char::from(0x40 + ((v >> shift) & 0x1F) as u8);
    format!("{}{}{}{:04X}", letter(26), letter(21), letter(16), v & 0xFFFF)
}

/// Checks a table's header: its length fits, and its bytes sum to zero.
fn check_table(t: &[u8]) -> core::result::Result<usize, &'static str> {
    if t.len() < 36 {
        return Err("shorter than its header");
    }
    let length = u32::from_le_bytes([t[4], t[5], t[6], t[7]]) as usize;
    if length < 36 || length > t.len() {
        return Err("its length is wrong");
    }
    if t[..length].iter().fold(0u8, |a, &b| a.wrapping_add(b)) != 0 {
        return Err("its checksum is wrong");
    }
    Ok(length)
}

impl<H: Host> Aml<H> {
    /// Loads the DSDT and then each SSDT, in the order given (note 0028, section 2). A table
    /// whose header is wrong is skipped; one whose bytecode fails part way keeps what it built.
    pub fn load(&mut self, tables: &[&[u8]]) -> LoadReport {
        let mut report = LoadReport::default();
        for t in tables {
            let sig = String::from_utf8_lossy(t.get(..4).unwrap_or(b"????")).into_owned();
            let length = match check_table(t) {
                Ok(l) => l,
                Err(why) => {
                    report.skipped.push(format!("{sig}: {why}"));
                    continue;
                }
            };
            if sig != "DSDT" && sig != "SSDT" {
                report.skipped.push(format!("{sig}: not a table of AML"));
                continue;
            }
            if sig == "DSDT" {
                self.wide = t[8] >= 2;
            }
            let bytes: Rc<[u8]> = Rc::from(&t[..length]);
            self.begin();
            let r = self.load_aml(bytes);
            self.end();
            match r {
                Ok(()) => report.tables += 1,
                Err(e) => {
                    self.host.log(false, format_args!("{sig}: stopped loading: {e}"));
                    report.skipped.push(format!("{sig}: {e}"));
                }
            }
        }
        report
    }

    /// Starts the namespace up (note 0028, section 3, step 4): `\_INI` and `\_SB._INI` if
    /// they exist; each region's `_REG`, saying its space can now be used; then every device,
    /// depth first: its `_STA`, and if it is present its `_INI`. A device that isn't present is
    /// skipped with everything under it, unless `_STA` says it is functioning.
    pub fn initialise(&mut self) -> InitReport {
        let mut report = InitReport::default();
        for path in ["\\_INI", "\\_SB._INI"] {
            if self.find(None, path).is_some()
                && let Err(e) = self.evaluate(path, &[])
            {
                report.failed.push(format!("{path}: {e}"));
            }
        }
        self.run_reg(&mut report);
        let root = self.ns.root();
        let mut stack: Vec<NodeId> = self.ns.children(root).into_iter().rev().collect();
        while let Some(n) = stack.pop() {
            if !self.is_device_like(n) {
                stack.extend(self.ns.children(n).into_iter().rev());
                continue;
            }
            report.devices += 1;
            let status = match self.status(n) {
                Ok(s) => s,
                Err(e) => {
                    report.failed.push(format!("{}._STA: {e}", self.ns.path(n)));
                    0
                }
            };
            if status & 1 != 0 {
                report.present += 1;
                if let Some(ini) = self.ns.child(n, b"_INI")
                    && let Err(e) = self.evaluate_node(ini, &[])
                {
                    report.failed.push(format!("{}._INI: {e}", self.ns.path(n)));
                }
            }
            if status & 0b1001 != 0 {
                stack.extend(self.ns.children(n).into_iter().rev());
            }
        }
        report
    }

    /// Runs `_REG(space, 1)` for the spaces the service reaches (memory, I/O, PCI
    /// configuration), once for each device that has regions of that space and a `_REG`.
    fn run_reg(&mut self, report: &mut InitReport) {
        let root = self.ns.root();
        let regions: Vec<(NodeId, u8)> = self
            .ns
            .walk(root, |ns, n| !matches!(ns.object(n), Some(Object::Method(_))))
            .into_iter()
            .filter_map(|n| match self.ns.object(n) {
                Some(Object::Region(r)) => Some((n, r.borrow().space)),
                _ => None,
            })
            .collect();
        let mut done = BTreeSet::new();
        for (region, space) in regions {
            if space > 2 {
                continue;
            }
            let Some(parent) = self.ns.parent(region) else { continue };
            let Some(reg) = self.ns.child(parent, b"_REG") else { continue };
            if done.insert((parent, space))
                && let Err(e) =
                    self.evaluate_node(reg, &[Value::Integer(space.into()), Value::Integer(1)])
            {
                report.failed.push(format!("{}._REG: {e}", self.ns.path(parent)));
            }
        }
    }

    fn is_device_like(&self, n: NodeId) -> bool {
        matches!(self.ns.object(n), Some(Object::Device | Object::Processor | Object::ThermalZone))
    }

    /// A device's `_STA`: present, enabled, shown, functioning (bits 0 to 3); all four
    /// without one.
    pub fn status(&mut self, n: NodeId) -> Result<u64> {
        match self.ns.child(n, b"_STA") {
            None => Ok(0x0F),
            Some(sta) => {
                let v = self.evaluate_node(sta, &[])?;
                self.integer(&v)
            }
        }
    }

    /// The devices that are present and have a `_HID` (note 0028, section 6), in namespace
    /// order. PCI functions, which have only an `_ADR`, are found on the bus instead.
    pub fn devices(&mut self) -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        let root = self.ns.root();
        let mut stack: Vec<NodeId> = self.ns.children(root).into_iter().rev().collect();
        while let Some(n) = stack.pop() {
            if !self.is_device_like(n) {
                stack.extend(self.ns.children(n).into_iter().rev());
                continue;
            }
            let status = self.status(n).unwrap_or(0);
            if status & 0b1001 != 0 {
                stack.extend(self.ns.children(n).into_iter().rev());
            }
            if status & 1 == 0 || !matches!(self.ns.object(n), Some(Object::Device)) {
                continue;
            }
            let Some(hid) = self.id_text(n, b"_HID").into_iter().next() else { continue };
            let compatible = self.id_text(n, b"_CID");
            let uid = self.ns.child(n, b"_UID").and_then(|u| {
                let v = self.evaluate_node(u, &[]).ok()?;
                match v {
                    Value::Integer(i) => Some(format!("{i}")),
                    Value::String(s) => Some(s),
                    _ => None,
                }
            });
            let (resources, crs_error) = match self.resources(n) {
                Ok(r) => (r, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            let path = self.ns.path(n);
            out.push(DeviceInfo { node: n, path, hid, compatible, uid, resources, crs_error });
        }
        out
    }

    /// A device's `_CRS`, decoded; empty without one.
    pub fn resources(&mut self, n: NodeId) -> core::result::Result<Vec<Resource>, String> {
        let Some(crs) = self.ns.child(n, b"_CRS") else { return Ok(Vec::new()) };
        let v = self.evaluate_node(crs, &[]).map_err(|e| format!("{e}"))?;
        let Value::Buffer(b) = v else { return Err(format!("_CRS gave {}", v.kind_name())) };
        let bytes = b.borrow().clone();
        resource::decode(&bytes).map_err(|e| format!("_CRS: {e:?}"))
    }

    /// The IDs in a device's `_HID` or `_CID`: an EISA number, a string, or (`_CID`) a
    /// package of them. Ones that fail are left out.
    fn id_text(&mut self, n: NodeId, name: &[u8; 4]) -> Vec<String> {
        let Some(id) = self.ns.child(n, name) else { return Vec::new() };
        let Ok(v) = self.evaluate_node(id, &[]) else { return Vec::new() };
        let one = |v: &Value| match v {
            Value::Integer(i) => Some(eisa_id(*i)),
            Value::String(s) => Some(s.clone()),
            _ => None,
        };
        match &v {
            Value::Package(p) => p.borrow().0.iter().filter_map(one).collect(),
            v => one(v).into_iter().collect(),
        }
    }

    /// True if `n` is a PCI host bridge. Asked during an evaluation (a PCI region's first
    /// use), so it reads the IDs inside the evaluation's budget.
    pub(crate) fn is_host_bridge(&mut self, n: NodeId) -> bool {
        for name in [b"_HID", b"_CID"] {
            let Some(id) = self.ns.child(n, name) else { continue };
            let Ok(v) = self.call_or_read(id, &[]) else { continue };
            let ids: Vec<Value> = match &v {
                Value::Package(p) => p.borrow().0.clone(),
                v => alloc::vec![v.clone()],
            };
            for v in ids {
                let text = match v {
                    Value::Integer(i) => eisa_id(i),
                    Value::String(s) => s,
                    _ => continue,
                };
                if HOST_BRIDGES.contains(&text.as_str()) {
                    return true;
                }
            }
        }
        false
    }

    /// An integer child (`_ADR`, `_BBN`, `_SEG`), read inside the current evaluation; `None`
    /// if there is none.
    pub(crate) fn integer_of(&mut self, n: NodeId, name: &str) -> Result<Option<u64>> {
        let seg = AmlName::parse(name)
            .and_then(|a| a.segs.first().copied())
            .ok_or(Error::Malformed("name"))?;
        let Some(c) = self.ns.child(n, &seg) else { return Ok(None) };
        let v = self.call_or_read(c, &[])?;
        Ok(Some(self.integer(&v)?))
    }

    /// The same, as its own evaluation.
    fn integer_child(&mut self, n: NodeId, name: &str) -> Result<Option<u64>> {
        self.begin();
        let r = self.integer_of(n, name);
        self.end();
        r
    }

    /// Where a PCI function's interrupt pin is wired (note 0028, section 6). `pin` is 1 to 4
    /// (INTA to INTD), as configuration space says.
    pub fn route(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        pin: u8,
    ) -> core::result::Result<Line, Route> {
        if !(1..=4).contains(&pin) {
            return Err(Route::NotFound);
        }
        let (mut bus, mut device, mut pin) = (bus, device, pin);
        // Up through PCI-to-PCI bridges without a `_PRT`, as far as there are buses.
        for _ in 0..256 {
            let Some((bridge, parent_bus, bridge_device)) = self.bridge_for(segment, bus) else {
                return Err(Route::NotFound);
            };
            if let Some(prt) = self.ns.child(bridge, b"_PRT") {
                return self.prt_route(prt, device, pin);
            }
            let Some(parent_bus) = parent_bus else { return Err(Route::NotFound) };
            // The PCI-to-PCI bridge specification's wiring: pins rotate by the device number.
            pin = ((pin - 1 + device) % 4) + 1;
            device = bridge_device;
            bus = parent_bus;
        }
        Err(Route::NotFound)
    }

    /// The namespace device for `bus`: a host bridge whose `_BBN` (0 without one) is that bus,
    /// or a PCI-to-PCI bridge whose secondary bus is. For a PCI-to-PCI bridge, also the bus
    /// it is on and its device number there.
    fn bridge_for(&mut self, segment: u16, bus: u8) -> Option<(NodeId, Option<u8>, u8)> {
        let root = self.ns.root();
        let devices = self.ns.walk(root, |ns, n| !matches!(ns.object(n), Some(Object::Method(_))));
        let devices: Vec<NodeId> = devices
            .into_iter()
            .filter(|&n| matches!(self.ns.object(n), Some(Object::Device)))
            .collect();
        for &n in &devices {
            self.begin();
            let host = self.is_host_bridge(n);
            self.end();
            if host
                && self.integer_child(n, "_SEG").ok().flatten().unwrap_or(0) as u16 == segment
                && self.integer_child(n, "_BBN").ok().flatten().unwrap_or(0) as u8 == bus
            {
                return Some((n, None, 0));
            }
        }
        for &n in &devices {
            if self.ns.child(n, b"_ADR").is_none() {
                continue;
            }
            self.begin();
            let at = self.device_address(n);
            self.end();
            let Some(at) = at.ok().flatten() else { continue };
            if at.segment != segment {
                continue;
            }
            let header = self.host.read_pci(at, 0x0E, 8).unwrap_or(0) & 0x7F;
            if header == 1 && self.host.read_pci(at, 0x19, 8).unwrap_or(0) as u8 == bus {
                return Some((n, Some(at.bus), at.device));
            }
        }
        None
    }

    /// The PCI address of a device the namespace places under a host bridge (as a region's,
    /// in `field.rs`); `None` if it isn't under one.
    fn device_address(&mut self, n: NodeId) -> Result<Option<PciAddress>> {
        let mut chain = Vec::new();
        let mut at = Some(n);
        let mut bridge = None;
        while let Some(d) = at {
            if self.is_host_bridge(d) {
                bridge = Some(d);
                break;
            }
            chain.push(d);
            at = self.ns.parent(d);
        }
        let Some(b) = bridge else { return Ok(None) };
        let mut pci = PciAddress { segment: 0, bus: 0, device: 0, function: 0 };
        pci.segment = self.integer_of(b, "_SEG")?.unwrap_or(0) as u16;
        pci.bus = self.integer_of(b, "_BBN")?.unwrap_or(0) as u8;
        let count = chain.len();
        for (i, &d) in chain.iter().rev().enumerate() {
            let adr = self.integer_of(d, "_ADR")?.unwrap_or(0);
            pci.device = ((adr >> 16) & 0x1F) as u8;
            pci.function = (adr & 7) as u8;
            if i + 1 < count {
                let header = self.host.read_pci(pci, 0x0E, 8).unwrap_or(0) & 0x7F;
                if header == 1 || header == 2 {
                    pci.bus = self.host.read_pci(pci, 0x19, 8).unwrap_or(0) as u8;
                }
            }
        }
        Ok(Some(pci))
    }

    /// Looks the pin up in a `_PRT`: packages of (address, pin, source, source index), the
    /// address's top half the device and its bottom half `0xFFFF` (any function).
    fn prt_route(&mut self, prt: NodeId, device: u8, pin: u8) -> core::result::Result<Line, Route> {
        let failed = |e: Error| Route::Failed(format!("{e}"));
        let table = self.evaluate_node(prt, &[]).map_err(failed)?;
        let Value::Package(entries) = table else {
            return Err(Route::Failed(format!("_PRT gave {}", table.kind_name())));
        };
        let entries = entries.borrow().0.clone();
        for e in entries {
            let Some(fields) = e.as_package() else { continue };
            let [address, p, source, index, ..] = fields.as_slice() else { continue };
            let (Some(address), Some(p)) = (address.as_integer(), p.as_integer()) else { continue };
            if (address >> 16) & 0xFFFF != u64::from(device) || p != u64::from(pin - 1) {
                continue;
            }
            return self.prt_source(prt, source, index);
        }
        Err(Route::NotFound)
    }

    /// A `_PRT` entry's source: 0, meaning the index is the line itself (level-triggered and
    /// active low, as ACPI defines it); or an interrupt link device, whose `_CRS` gives the
    /// line with its own trigger and polarity.
    fn prt_source(
        &mut self,
        prt: NodeId,
        source: &Value,
        index: &Value,
    ) -> core::result::Result<Line, Route> {
        let link = match source {
            Value::Integer(_) => {
                let number = index.as_integer().ok_or(Route::NotFound)? as u32;
                return Ok(Line { number, level: true, active_low: true, shared: true });
            }
            Value::Reference(Reference(Ref::Node(n))) => *n,
            Value::Reference(Reference(Ref::Name(scope, name))) => {
                self.ns.lookup(*scope, name).ok_or(Route::NotFound)?
            }
            Value::String(s) => {
                let name = AmlName::parse(s).ok_or(Route::NotFound)?;
                let scope = self.ns.parent(prt).unwrap_or(self.ns.root());
                self.ns.lookup(scope, &name).ok_or(Route::NotFound)?
            }
            _ => return Err(Route::NotFound),
        };
        let resources = self.resources(self.ns.through_alias(link)).map_err(Route::Failed)?;
        resources
            .into_iter()
            .find_map(|r| match r {
                Resource::Interrupt(line) => Some(line),
                _ => None,
            })
            .ok_or(Route::NotFound)
    }

    /// The sleep type values for sleep state `state` (5 for off): `\_S5`'s first two
    /// elements, or, as some firmware packs them, both in the bytes of one.
    pub fn sleep_type(&mut self, state: u8) -> Result<SleepType> {
        let path = format!("\\_S{state}");
        let v = self.evaluate(&path, &[])?;
        let elements =
            v.as_package().ok_or(Error::WrongType("a sleep state that isn't a package"))?;
        let ints: Vec<u64> = elements.iter().filter_map(Value::as_integer).collect();
        match (elements.len(), ints.as_slice()) {
            (1, [packed]) => Ok(SleepType { a: *packed as u8, b: (*packed >> 8) as u8 }),
            (_, [a, b, ..]) => Ok(SleepType { a: *a as u8, b: *b as u8 }),
            _ => Err(Error::WrongType("a sleep state's values aren't integers")),
        }
    }

    /// For tests and the service's verbose log: every node's path and kind.
    pub fn listing(&self) -> Vec<(String, u64)> {
        let root = self.ns.root();
        self.ns
            .walk(root, |_, _| true)
            .into_iter()
            .map(|n| (self.ns.path(n), self.ns.object(n).map(Object::kind).unwrap_or(0)))
            .collect()
    }
}
