// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The device self-test (docs/design/0012-driver-restart-and-m4-tests.md, section "The
//! tests"): the other drivers, through their interfaces, as a client would use them.
//!
//! - The scan: the device manager found exactly the devices the QEMU configuration has and
//!   every driver is running.
//! - The screens: see `displaytest.rs`.
//! - COM2 (x86-64): text goes out of the port (the test runner captures it), and with the chip
//!   in loopback a byte comes back with the readable notification.
//! - Keyboards and mice: the test runner presses keys and moves the mouse through QEMU when it
//!   sees a line this test prints, and the events come out of the ring, in order.

use crate::blocktest::Devmgr;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use granite::Handle;
use granite::device::IoPorts;
use granite::endpoint::Endpoint;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::process::role as start_role;
use granite::ring::{Consumer, RingError};
use granite::sys::{perm, rights};
use granite::time;
use pbr_devices::keys;
use pbr_interfaces::pbr_devices::{DeviceClass, DeviceInfo, DeviceState};
use pbr_interfaces::pbr_input::{self as input, InputKind};
use pbr_interfaces::pbr_serial as serial;

/// Copies of the root program's own capabilities that this test uses; `devtest` takes the
/// originals.
pub struct Inputs {
    machine: Memory,
    ports: Option<IoPorts>,
}

/// Makes the copies. Call before `devtest::run`.
pub fn copy_inputs(start: &granite::Start) -> Result<Inputs, String> {
    let copy = |r: u32| -> Result<Option<Handle>, String> {
        let Some(raw) = start.peek(r) else { return Ok(None) };
        // `raw` stays the start-up block's; only look at it.
        let h = ManuallyDrop::new(Handle::from_raw(raw));
        h.duplicate(rights::ALL).map(Some).map_err(|e| format!("copying a capability: {e}"))
    };
    let machine = copy(start_role::MACHINE)?.ok_or("no machine description")?;
    Ok(Inputs { machine: Memory(machine), ports: copy(start_role::IO_PORTS)?.map(IoPorts) })
}

const WAIT_MS: u64 = 30_000;
/// An event is 16 bytes.
const EVENT: usize = input::EVENT_SIZE as usize;

/// What the machine description says (note 0011, section 2).
struct Machine {
    ps2: bool,
    screen: Option<(u32, u32, u32)>,
}

fn machine(m: &Memory) -> Result<Machine, String> {
    let mut page = [0u8; 128];
    m.read(0, &mut page).map_err(|e| format!("reading the machine description: {e}"))?;
    let u32_at = |o: usize| u32::from_le_bytes([page[o], page[o + 1], page[o + 2], page[o + 3]]);
    let screen = (u32_at(44) > 0).then(|| (u32_at(64 + 16), u32_at(64 + 20), u32_at(64 + 24)));
    Ok(Machine { ps2: u32_at(12) & 1 != 0, screen })
}

/// Runs the device self-test. Returns a short summary.
pub fn run(d: &Devmgr, inputs: &Inputs) -> Result<String, String> {
    let m = machine(&inputs.machine)?;
    let list = scan(d, &m)?;
    let mut done: Vec<&str> = alloc::vec!["the scan"];

    // Open everything first, so that the runner's keys and the rings meet.
    let open = |info: &DeviceInfo| -> Result<Endpoint, String> {
        d.client.open(info.id).map(Endpoint).map_err(|e| format!("opening {}: {e}", info.name))
    };
    let of = |class: DeviceClass| list.iter().filter(move |x| x.kind == class);

    if cfg!(target_arch = "x86_64") {
        let info = of(DeviceClass::Serial).next().ok_or("no serial port")?;
        let ports = inputs.ports.as_ref().ok_or("no I/O ports")?;
        com2(open(info)?, ports)?;
        done.push("COM2 (output and loopback)");
    }
    let mut pads: Vec<Pad> = Vec::new();
    for info in of(DeviceClass::Input) {
        pads.push(Pad::open(open(info)?, &info.name)?);
    }
    let virtio_pads = |kind: InputKind| pads.iter().position(|p| p.kind == kind && p.virtio);
    let kb = virtio_pads(InputKind::Keyboard).ok_or("no virtio keyboard")?;
    let mouse = virtio_pads(InputKind::Mouse).ok_or("no virtio mouse")?;

    granite::println!("Device self-test: inject virtio");
    keyboard_events(&mut pads[kb], "the virtio keyboard")?;
    mouse_events(&mut pads[mouse], "the virtio mouse")?;
    done.push("a virtio keyboard and mouse");

    // The displays (`displaytest.rs`): the runner answers on the virtio keyboard when it has
    // looked at the screens, so this comes once that keyboard is known to work.
    let mut screens = 0;
    for info in of(DeviceClass::Display) {
        let firmware = info.name.starts_with("framebuffer");
        let size = m.screen.filter(|_| firmware).map(|(width, height, _)| (width, height));
        screens += crate::displaytest::run(d, info, size, &mut pads[kb])?;
    }
    if screens > 0 {
        done.push("the screens");
    }

    // The tablets (note 0021, section 7), in the order the device manager found them, which
    // is the order of the screens they are tied to where they are tied to one. The runner
    // points with each at a place of its own, so a tablet that got another's events, or a
    // position scaled wrongly, is caught.
    let tablets: Vec<usize> =
        (0..pads.len()).filter(|i| pads[*i].kind == InputKind::Tablet).collect();
    for (n, pad) in tablets.iter().enumerate() {
        granite::println!("Device self-test: inject tablet {n}");
        tablet_events(&mut pads[*pad], n as u32)?;
    }
    for pad in &tablets {
        pads[*pad].quiet("a tablet nobody pointed with")?;
    }
    if !tablets.is_empty() {
        done.push("tablets");
    }

    granite::println!("Device self-test: inject flood");
    let sent = pads[mouse].flood()?;
    granite::println!("Device self-test: inject once");
    pads[mouse].lost(sent)?;
    done.push("a full ring");

    if m.ps2 {
        let ps2 = pads.iter().position(|p| !p.virtio).ok_or("no PS/2 controller")?;
        granite::println!("Device self-test: inject ps2");
        keyboard_events(&mut pads[ps2], "the PS/2 keyboard")?;
        mouse_events(&mut pads[ps2], "the PS/2 mouse")?;
        done.push("a PS/2 keyboard and mouse");
    }
    Ok(done.join(", "))
}

/// Waits for every driver to be running, and checks the list against what the machine has.
fn scan(d: &Devmgr, m: &Machine) -> Result<Vec<DeviceInfo>, String> {
    let end = time::after_ms(WAIT_MS);
    let list = loop {
        let list = d.client.list().map_err(|e| format!("listing devices: {e}"))?;
        if list.iter().all(|x| matches!(x.state, DeviceState::Running | DeviceState::Disabled)) {
            break list;
        }
        if time::now() > end {
            return Err(format!("drivers still starting after {WAIT_MS} ms: {list:?}"));
        }
        time::sleep_ms(20);
    };
    if let Some(bad) = list.iter().find(|x| x.state != DeviceState::Running) {
        return Err(format!("{} is {:?}", bad.name, bad.state));
    }
    let count = |class: DeviceClass, prefix: &str| {
        list.iter().filter(|x| x.kind == class && x.name.starts_with(prefix)).count()
    };
    let x86 = cfg!(target_arch = "x86_64");
    // From M6 some configurations also have QEMU's tablets (note 0026, section 6), which are
    // virtio input devices too. The self-test image is the same for every configuration, so
    // how many there are is told from the kind each device says it is; they come on top of the
    // keyboard and mouse.
    let mut tablets = 0;
    for x in list.iter().filter(|x| x.kind == DeviceClass::Input && x.name.starts_with("virtio")) {
        let ep = d.client.open(x.id).map_err(|e| format!("opening {}: {e}", x.name))?;
        let info = input::Client::new(Endpoint(ep)).info();
        let info = info.map_err(|e| format!("{}: info: {e}", x.name))?;
        if info.kind == InputKind::Tablet {
            tablets += 1;
        }
    }
    if tablets > 2 {
        return Err(format!("{tablets} tablets, expected at most 2; the list is {list:?}"));
    }
    let want = [
        // The test disk and the storage tests' three (M5), and on ARM64 the boot disk too (on
        // x86-64 that is an AHCI disk).
        ("virtio disks", count(DeviceClass::Block, "virtio-blk"), if x86 { 5 } else { 6 }),
        ("virtio input devices", count(DeviceClass::Input, "virtio-input"), 2 + tablets),
        ("PS/2 controllers", count(DeviceClass::Input, "ps2"), usize::from(x86 && m.ps2)),
        ("serial ports", count(DeviceClass::Serial, "serial16550"), usize::from(x86)),
        ("screens", count(DeviceClass::Display, "framebuffer"), usize::from(m.screen.is_some())),
    ];
    for (what, have, wanted) in want {
        if have != wanted {
            return Err(format!("{have} {what}, expected {wanted}; the list is {list:?}"));
        }
    }
    if list.len() != want.iter().map(|w| w.2).sum::<usize>() {
        return Err(format!("devices nobody expected: {list:?}"));
    }
    Ok(list)
}

/// Where the runner points with the first tablet, of the 32767 QEMU's tablets measure each
/// way (`xtask/src/qemu.rs` has the same numbers); tablet `n` gets `n + 1` times this.
const TABLET_AT: (u32, u32) = (4000, 3000);

/// Expects from tablet `n`: one position, where the runner pointed, scaled to 0 to 65535 each
/// way; then the left button pressed and released there.
fn tablet_events(p: &mut Pad, n: u32) -> Result<(), String> {
    let who = format!("tablet {n}");
    // The driver scales a place on the tablet to 0 to 65535, rounding down.
    let scaled = |v: u32| v * (n + 1) * 65535 / 32767;
    let want = scaled(TABLET_AT.0) | (scaled(TABLET_AT.1) << 16);
    let events = p.take(3, &who)?;
    let got: Vec<(u32, u32, u32)> =
        events.iter().map(|e| (u32::from(e.kind), u32::from(e.code), e.value as u32)).collect();
    let expected = [
        (input::EVENT_POSITION, 0, want),
        (input::EVENT_BUTTON, input::BUTTON_LEFT, input::VALUE_PRESSED),
        (input::EVENT_BUTTON, input::BUTTON_LEFT, input::VALUE_RELEASED),
    ];
    if got != expected {
        return Err(format!("{who} sent {got:x?}, not {expected:x?}"));
    }
    Ok(())
}

/// Writes to COM2, then loops a byte back through the chip.
fn com2(ep: Endpoint, ports: &IoPorts) -> Result<(), String> {
    let c = serial::Client::new(ep);
    let text = b"PrettyBigRock COM2 test\n";
    let sent = c.write(text).map_err(|e| format!("writing to COM2: {e}"))?;
    if sent as usize != text.len() {
        return Err(format!("COM2 took {sent} of {} bytes", text.len()));
    }
    let readable = c.events().map_err(|e| format!("COM2 events: {e}"))?;
    if !c.read(16).map_err(|e| format!("reading COM2: {e}"))?.is_empty() {
        return Err("COM2 had bytes nobody sent".into());
    }
    if c.read(serial::MAX_TRANSFER + 1).is_ok() {
        return Err("COM2 read more than it may".into());
    }
    // The chip's loopback mode: what is sent is received (modem control register, bit 4).
    let chip = ports.slice(0x2F8, 8, rights::ALL).map_err(|e| format!("slicing COM2: {e}"))?;
    chip.write(4, 1, 0x1B).map_err(|e| format!("entering loopback: {e}"))?;
    let word = b"loop";
    let result = (|| {
        c.write(word).map_err(|e| format!("writing in loopback: {e}"))?;
        let mut got = Vec::new();
        let end = time::after_ms(5000);
        while got.len() < word.len() {
            let _ = readable.wait(time::after_ms(200));
            got.extend(c.read(16).map_err(|e| format!("reading in loopback: {e}"))?);
            if time::now() > end {
                return Err(format!("only {got:?} came back"));
            }
        }
        if got != word {
            return Err(format!("{got:?} came back instead of {word:?}"));
        }
        Ok(())
    })();
    chip.write(4, 1, 0x0B).map_err(|e| format!("leaving loopback: {e}"))?;
    result
}

/// An opened input device.
pub struct Pad {
    kind: InputKind,
    virtio: bool,
    ring: Consumer,
    doorbell: Notification,
    capacity: usize,
    _map: Mapping,
    _memory: Memory,
    /// Events read while looking for others: a PS/2 ring carries keys and mouse together.
    aside: VecDeque<Event>,
}

/// What came out of a ring.
#[derive(Clone, Copy, Debug)]
struct Event {
    kind: u16,
    code: u16,
    value: i32,
}

impl Pad {
    fn open(ep: Endpoint, name: &str) -> Result<Pad, String> {
        let c = input::Client::new(ep);
        let info = c.info().map_err(|e| format!("{name}: info: {e}"))?;
        let opened = c.open().map_err(|e| format!("{name}: open: {e}"))?;
        let map = opened.ring.map(perm::READ_WRITE).map_err(|e| format!("{name}: mapping: {e}"))?;
        // SAFETY: the mapping is readable and writable for its whole length, 64-aligned (a
        // page), and lives as long as the consumer (both are in the struct).
        let ring = unsafe { Consumer::new(map.as_mut_ptr(), map.len() as usize) }
            .map_err(|e| format!("{name}: the ring is no good: {e:?}"))?;
        // The ring holds the largest power of two entries that fit.
        let capacity = 1usize << ((map.len() as usize - granite::ring::ENTRIES_AT) / EVENT).ilog2();
        Ok(Pad {
            kind: info.kind,
            virtio: name.starts_with("virtio-input"),
            ring,
            doorbell: opened.doorbell,
            capacity,
            _map: map,
            _memory: opened.ring,
            aside: VecDeque::new(),
        })
    }

    /// The next event, waiting for it for at most `ms`.
    fn next(&mut self, ms: u64) -> Result<Option<Event>, String> {
        let end = time::after_ms(ms);
        loop {
            let mut e = [0u8; EVENT];
            match self.ring.pop(&mut e) {
                Ok(_) => {
                    return Ok(Some(Event {
                        kind: u16::from_le_bytes([e[0], e[1]]),
                        code: u16::from_le_bytes([e[2], e[3]]),
                        value: i32::from_le_bytes([e[4], e[5], e[6], e[7]]),
                    }));
                }
                Err(RingError::WouldBlock) => {
                    if time::now() >= end {
                        return Ok(None);
                    }
                    let _ = self.doorbell.wait(end.min(time::after_ms(100)));
                }
                Err(e) => return Err(format!("the ring broke: {e:?}")),
            }
        }
    }

    /// The next `n` events, in order.
    fn take(&mut self, n: usize, who: &str) -> Result<Vec<Event>, String> {
        let mut v = Vec::new();
        while v.len() < n {
            match self.next(WAIT_MS)? {
                Some(e) => v.push(e),
                None => return Err(format!("{who}: only {v:?} arrived, expected {n} events")),
            }
        }
        Ok(v)
    }

    /// Checks that nothing else arrives soon.
    fn quiet(&mut self, who: &str) -> Result<(), String> {
        match self.next(300)? {
            None => Ok(()),
            Some(e) => Err(format!("{who}: an extra event {e:?}")),
        }
    }

    /// Reads a ring that was allowed to fill: it must hold exactly as many events as fit, and
    /// all of them the movements the runner sent. Returns how many were sent in all.
    fn flood(&mut self) -> Result<u32, String> {
        // Wait for the ring to fill (or for the movements to stop arriving).
        let end = time::after_ms(WAIT_MS);
        while self.ring.available().unwrap_or(0) < self.capacity as u32 && time::now() < end {
            time::sleep_ms(50);
        }
        time::sleep_ms(500); // anything more that is on its way
        let mut n = 0;
        while let Some(e) = self.next(300)? {
            if (e.kind, e.code, e.value) != (input::EVENT_MOVE as u16, input::AXIS_X as u16, 1) {
                return Err(format!("a flooded ring holds {e:?}"));
            }
            n += 1;
        }
        if n != self.capacity {
            return Err(format!(
                "the full ring held {n} events, it has room for {}",
                self.capacity
            ));
        }
        Ok(FLOOD)
    }

    /// After one more event, the first one is the count of those that were lost.
    fn lost(&mut self, sent: u32) -> Result<(), String> {
        let got = self.take(2, "after the flood")?;
        let lost = sent as usize - self.capacity;
        if (got[0].kind, got[0].value) != (input::EVENT_LOST as u16, lost as i32) {
            return Err(format!("expected a loss of {lost} events first, got {got:?}"));
        }
        if (got[1].kind, got[1].code) != (input::EVENT_MOVE as u16, input::AXIS_X as u16) {
            return Err(format!("expected the new movement second, got {got:?}"));
        }
        Ok(())
    }
}

/// How many one-pixel movements the runner sends to fill the ring (more than it holds).
const FLOOD: u32 = 700;

/// The runner pressed and released A.
pub fn keyboard_events(p: &mut Pad, who: &str) -> Result<(), String> {
    if !matches!(p.kind, InputKind::Keyboard | InputKind::KeyboardAndMouse) {
        return Err(format!("{who} says it is a {:?}", p.kind));
    }
    let want = [
        (input::EVENT_KEY as u16, keys::key::A, input::VALUE_PRESSED as i32),
        (input::EVENT_KEY as u16, keys::key::A, input::VALUE_RELEASED as i32),
    ];
    // A shared ring (PS/2) holds the mouse as well; take only the keys.
    let mut got = Vec::new();
    while got.len() < want.len() {
        let e = p.take(1, who)?[0];
        if e.kind == input::EVENT_KEY as u16 {
            got.push((e.kind, e.code, e.value));
        } else {
            p.aside.push_back(e);
        }
    }
    if got != want {
        return Err(format!("{who}: the keys were {got:?}, expected {want:?}"));
    }
    if p.kind == InputKind::Keyboard { p.quiet(who) } else { Ok(()) }
}

/// The runner moved the mouse 10 right and 5 down while pressing the left button, then
/// released it.
fn mouse_events(p: &mut Pad, who: &str) -> Result<(), String> {
    if !matches!(p.kind, InputKind::Mouse | InputKind::KeyboardAndMouse) {
        return Err(format!("{who} says it is a {:?}", p.kind));
    }
    let (mut dx, mut dy) = (0, 0);
    let mut buttons = Vec::new();
    while buttons.len() < 2 || (dx, dy) != (10, 5) {
        let e = match p.aside.pop_front() {
            Some(e) => e,
            None => p.take(1, who)?[0],
        };
        match (e.kind as u32, e.code as u32) {
            (input::EVENT_MOVE, input::AXIS_X) => dx += e.value,
            (input::EVENT_MOVE, input::AXIS_Y) => dy += e.value,
            (input::EVENT_BUTTON, input::BUTTON_LEFT) => buttons.push(e.value),
            _ => return Err(format!("{who}: unexpected {e:?}")),
        }
        if buttons.len() > 2 || dx.abs() > 10 || dy.abs() > 5 {
            return Err(format!("{who}: too much: moved ({dx}, {dy}), buttons {buttons:?}"));
        }
    }
    if buttons != [input::VALUE_PRESSED as i32, input::VALUE_RELEASED as i32] {
        return Err(format!("{who}: the button went {buttons:?}"));
    }
    p.quiet(who)
}
