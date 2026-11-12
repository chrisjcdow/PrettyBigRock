// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The input service's self-test (docs/design/0021-input-and-keyboard-layouts.md, section 9):
//! the parts that need the real service, real drivers and keys pressed from outside.
//!
//! The rules themselves (the merge, repeat timing, layouts, a full ring) are tested on the
//! host, in `lib/seat` and `lib/keymap`. Here the root program plays the seat's consumer, the
//! part the console and later the display server play, and checks what arrives:
//!
//! 1. **Who may do what.** The root program's lookup table gives the seat only to the console
//!    (and, when it exists, the display server), and the users service a capability that can
//!    set the layout and nothing else; the input service refuses each kind of capability what
//!    it isn't for.
//! 2. **Taking the seat** starts with a reset, and a second taker stops the first one's
//!    doorbell.
//! 3. **A key held while its driver crashes** doesn't stay held: the consumer is sent a reset
//!    with nothing held after it, the repeats stop, and the keyboard works again once its
//!    driver has been restarted.
//! 4. **A tablet's positions** arrive scaled to 0 to 65535, from the right tablet.
//!
//! The test runner (`xtask/src/qemu.rs`) presses the keys and points, through QEMU's control
//! socket, each time this prints a line starting `Seat self-test:`. The shell test, which
//! comes next, covers the rest: the console getting its keys through the input service, lines
//! typed with the `gb` and `us-intl` layouts, and the input service crashing under the
//! console.

use crate::blocktest;
use crate::supervisor::{self, Kind, SEAT_CONSUMER, SEAT_LAYOUT, Supervisor};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::Error;
use granite::interface::CallError;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::ring::{Consumer, RingError};
use granite::sys::perm;
use granite::time;
use pbr_interfaces::pbr_input as input;
use pbr_interfaces::pbr_seat::{self as seat, DeviceKind, SeatError};

/// One event from the seat's ring, as its fields (`interfaces/pbr.seat.pbi` has the table).
/// The root program reads the bytes itself rather than through `lib/seat`, so that this test
/// is a second reading of the interface file, not the same code on both sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    kind: u32,
    device: u16,
    code: u32,
    value: i32,
    first: u32,
    second: u32,
    time: u64,
}

/// The seat, taken: the ring and its doorbell.
struct Taken {
    consumer: Consumer,
    doorbell: Notification,
    /// The ring's memory, kept so that it stays mapped while the consumer reads it.
    _map: Mapping,
    _ring: Memory,
}

/// `pbr.seat` through a capability with `badge`, as the supervisor would give one out.
fn client(sup: &Supervisor, badge: u64) -> Result<seat::Client, String> {
    let mut c = seat::Client::new(sup.client(Kind::Input, badge, 60)?);
    // Long enough for a service that has only just been started to get as far as answering,
    // on an emulated machine that is busy.
    c.set_timeout_ms(60_000);
    Ok(c)
}

fn take(c: &seat::Client) -> Result<Taken, String> {
    let t = c.take().map_err(|e| format!("taking the seat: {e}"))?;
    let map = t.ring.map(perm::READ_WRITE).map_err(|e| format!("mapping the seat's ring: {e}"))?;
    // SAFETY: the mapping is readable and writable for its whole length and starts on a page
    // (aligned to 64, as a ring needs); it is kept beside the consumer in `Taken`.
    let consumer = unsafe { Consumer::new(map.as_mut_ptr(), map.len() as usize) }
        .map_err(|e| format!("the seat's ring is no good: {e:?}"))?;
    Ok(Taken { consumer, doorbell: t.doorbell, _map: map, _ring: t.ring })
}

impl Taken {
    /// The next event, waiting until `deadline` for one. `None` when the time is up.
    fn next(&mut self, deadline: u64) -> Result<Option<Event>, String> {
        let mut e = [0u8; seat::EVENT_SIZE as usize];
        loop {
            match self.consumer.pop(&mut e) {
                Ok(_) => {
                    let u16_at = |i: usize| u16::from_le_bytes([e[i], e[i + 1]]);
                    let u32_at =
                        |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
                    let mut time = [0u8; 8];
                    time.copy_from_slice(&e[24..32]);
                    return Ok(Some(Event {
                        kind: u32::from(u16_at(0)),
                        device: u16_at(2),
                        code: u32::from(u16_at(4)),
                        value: u32_at(8) as i32,
                        first: u32_at(12),
                        second: u32_at(16),
                        time: u64::from_le_bytes(time),
                    }));
                }
                Err(RingError::WouldBlock) => match self.doorbell.wait(deadline) {
                    Ok(_) => {}
                    Err(Error::TimedOut) => return Ok(None),
                    Err(e) => return Err(format!("the seat's doorbell: {e}")),
                },
                Err(e) => return Err(format!("the seat's ring: {e:?}")),
            }
        }
    }

    /// Reads events for up to `ms` until one satisfies `wanted`, and returns it with the ones
    /// read before it.
    fn until(
        &mut self,
        ms: u64,
        what: &str,
        mut wanted: impl FnMut(&Event) -> bool,
    ) -> Result<(Event, Vec<Event>), String> {
        let end = time::after_ms(ms);
        let mut before = Vec::new();
        loop {
            match self.next(end)? {
                Some(e) if wanted(&e) => return Ok((e, before)),
                Some(e) => before.push(e),
                None => {
                    return Err(format!("{what}: not within {ms} ms; what came was {before:?}"));
                }
            }
        }
    }

    /// Everything that arrives in `ms`.
    fn all(&mut self, ms: u64) -> Result<Vec<Event>, String> {
        let end = time::after_ms(ms);
        let mut events = Vec::new();
        while let Some(e) = self.next(end)? {
            events.push(e);
        }
        Ok(events)
    }
}

fn is_key(e: &Event, code: u32, value: u32) -> bool {
    e.kind == seat::EVENT_KEY && e.code == code && e.value == value as i32
}

/// Check 1: the lookup table, and what the input service lets each badge do.
fn permissions(sup: &Supervisor) -> Result<(), String> {
    // The table: the console is a consumer, the users service sets layouts, and nobody else
    // can look the input service up at all.
    let all = [Kind::Devmgr, Kind::Storage, Kind::Users, Kind::Console, Kind::Input];
    for kind in all {
        let got = supervisor::rule(kind, "input");
        let want = match kind {
            Kind::Console => Some((Kind::Input, SEAT_CONSUMER)),
            Kind::Users => Some((Kind::Input, SEAT_LAYOUT)),
            _ => None,
        };
        if got != want {
            return Err(format!("{kind:?} looking up input gets {got:?}, not {want:?}"));
        }
    }
    let refused = |what: &str, r: Result<(), CallError<SeatError>>| match r {
        Err(CallError::Service(SeatError::NotAllowed)) => Ok(()),
        other => Err(format!("{what}: {other:?} instead of being refused")),
    };
    // The users service's capability: layouts, and nothing that hears keys.
    let setter = client(sup, SEAT_LAYOUT)?;
    refused("the users service's badge taking the seat", setter.take().map(|_| ()))?;
    refused("the users service's badge listing devices", setter.devices().map(|_| ()))?;
    let layouts = setter.layouts().map_err(|e| format!("asking for the layouts: {e}"))?;
    let names: Vec<&str> = layouts.available.iter().map(|l| l.name.as_str()).collect();
    if names != ["gb", "us", "us-intl"] || layouts.current != "us" {
        return Err(format!("the layouts are {names:?}, with {:?} in use", layouts.current));
    }
    if layouts.available.iter().any(|l| l.title.is_empty()) {
        return Err(format!("a layout has no title: {:?}", layouts.available));
    }
    match setter.set_layout("klingon") {
        Err(CallError::Service(SeatError::NoSuchLayout)) => {}
        other => return Err(format!("setting a layout there isn't: {other:?}")),
    }
    for name in ["gb", "us"] {
        setter.set_layout(name).map_err(|e| format!("setting the layout {name}: {e}"))?;
        let now = setter.layouts().map_err(|e| format!("asking for the layouts: {e}"))?.current;
        if now != name {
            return Err(format!("after setting {name} the layout is {now}"));
        }
    }
    // A consumer's capability: everything but changing the layout.
    let consumer = client(sup, SEAT_CONSUMER)?;
    refused("a consumer setting the layout", consumer.set_layout("gb"))?;
    consumer.layouts().map_err(|e| format!("a consumer asking for the layouts: {e}"))?;
    // Any other badge, and none: nothing at all.
    for badge in [0, 3, u64::MAX] {
        let other = client(sup, badge)?;
        refused("an unknown badge taking the seat", other.take().map(|_| ()))?;
        refused("an unknown badge listing devices", other.devices().map(|_| ()))?;
        refused("an unknown badge asking for layouts", other.layouts().map(|_| ()))?;
        refused("an unknown badge setting the layout", other.set_layout("gb"))?;
    }
    Ok(())
}

/// Waits for the input service to have found its devices: at least a keyboard, and the list
/// unchanged for two seconds. (Drivers start one after another, and the input service looks
/// for new ones once a second.)
fn settled(c: &seat::Client) -> Result<Vec<seat::Device>, String> {
    let end = time::after_ms(90_000);
    let mut last: Vec<seat::Device> = Vec::new();
    let mut since = time::now();
    loop {
        let list = c.devices().map_err(|e| format!("listing the seat's devices: {e}"))?;
        if list != last {
            last = list;
            since = time::now();
        }
        let keyboard = last.iter().any(|d| d.kind == DeviceKind::Keyboard);
        if keyboard && time::now() - since > 2_000_000_000 {
            return Ok(last);
        }
        if time::now() > end {
            return Err(format!("the seat's devices didn't settle; they are {last:?}"));
        }
        time::sleep_ms(200);
    }
}

/// Check 3: a key held while its keyboard's driver crashes.
fn held_through_a_crash(
    sup: &Supervisor,
    c: &seat::Client,
    seat: &mut Taken,
) -> Result<(), String> {
    granite::println!("Seat self-test: hold b");
    let (down, _) =
        seat.until(30_000, "b pressed", |e| is_key(e, input::KEY_B, input::VALUE_PRESSED))?;
    if (down.first, down.second) != (u32::from('b'), 0) {
        return Err(format!("b's press carries {:#x} and {:#x}, not `b`", down.first, down.second));
    }
    let devices = c.devices().map_err(|e| format!("listing the seat's devices: {e}"))?;
    let keyboard = devices
        .iter()
        .find(|d| d.id == down.device && d.kind == DeviceKind::Keyboard)
        .ok_or_else(|| {
            format!("b came from device {}, which is no keyboard: {devices:?}", down.device)
        })?
        .clone();
    // The input service makes the repeats: the first after about half a second, then 30 a
    // second. (Bounds are loose: the emulated machine is slow and busy.)
    let (first, _) =
        seat.until(10_000, "b repeating", |e| is_key(e, input::KEY_B, input::VALUE_REPEAT))?;
    let waited = first.time.saturating_sub(down.time);
    if !(450_000_000..5_000_000_000).contains(&waited) {
        return Err(format!("b's first repeat came {} ms after its press", waited / 1_000_000));
    }
    if first.first != u32::from('b') || first.device != down.device {
        return Err(format!("b's repeat is {first:?}"));
    }
    seat.until(5_000, "b repeating again", |e| is_key(e, input::KEY_B, input::VALUE_REPEAT))?;

    // The keyboard's driver crashes with the key still down.
    let devmgr = blocktest::connect(sup)?;
    devmgr
        .debug
        .stop(keyboard.device)
        .map_err(|e| format!("stopping the keyboard's driver: {e}"))?;
    let (_, _) = seat.until(30_000, "a reset after the keyboard's driver stopped", |e| {
        e.kind == seat::EVENT_RESET
    })?;
    // After the reset: nothing held on that keyboard, no more repeats, and word that the
    // devices changed (it went; then, restarted, it comes back under a new id).
    let mut after = Vec::new();
    let back = {
        let end = time::after_ms(90_000);
        loop {
            after.extend(seat.all(500)?);
            let now = c.devices().map_err(|e| format!("listing the seat's devices: {e}"))?;
            let found = now
                .iter()
                .find(|d| d.kind == DeviceKind::Keyboard && d.device == keyboard.device)
                .cloned();
            match found {
                Some(d) if d.id != keyboard.id => break d,
                _ if time::now() > end => {
                    return Err(format!("the keyboard didn't come back; the devices are {now:?}"));
                }
                _ => {}
            }
        }
    };
    after.extend(seat.all(500)?);
    if let Some(e) = after.iter().find(|e| e.kind == seat::EVENT_KEY) {
        return Err(format!("after the keyboard's driver crashed, a key event: {e:?}"));
    }
    if !after.iter().any(|e| e.kind == seat::EVENT_DEVICES) {
        return Err(format!("no word that the devices changed; what came was {after:?}"));
    }
    devmgr.debug.forgive(keyboard.device).map_err(|e| format!("forgiving: {e}"))?;

    // The key is let go while nothing thinks it is down: nothing is said.
    granite::println!("Seat self-test: release b");
    let quiet = seat.all(1_500)?;
    if let Some(e) = quiet.iter().find(|e| e.kind == seat::EVENT_KEY) {
        return Err(format!("b's release, of a key that wasn't down, gave {e:?}"));
    }
    // And the keyboard works, through its new driver, as a new device.
    granite::println!("Seat self-test: tap a");
    let (a, _) =
        seat.until(30_000, "a pressed", |e| is_key(e, input::KEY_A, input::VALUE_PRESSED))?;
    if a.device != back.id || a.first != u32::from('a') {
        return Err(format!("a's press is {a:?}; the keyboard is now device {}", back.id));
    }
    seat.until(10_000, "a released", |e| is_key(e, input::KEY_A, input::VALUE_RELEASED))?;
    Ok(())
}

/// Where the runner points with the first tablet, of the 32767 QEMU's tablets measure each
/// way; tablet `n` gets `n + 1` times this (the driver test and `xtask/src/qemu.rs` have the
/// same numbers).
const TABLET_AT: (u32, u32) = (4000, 3000);

/// Check 4: each tablet's position arrives scaled, from that tablet. Returns how many.
fn tablets(c: &seat::Client, seat: &mut Taken) -> Result<usize, String> {
    let devices = c.devices().map_err(|e| format!("listing the seat's devices: {e}"))?;
    // In the order the device manager found them, which is the order of QEMU's command line.
    let mut tablets: Vec<&seat::Device> =
        devices.iter().filter(|d| d.kind == DeviceKind::Tablet).collect();
    tablets.sort_by_key(|d| d.device);
    for (n, tablet) in tablets.iter().enumerate() {
        seat.all(200)?;
        granite::println!("Seat self-test: point tablet {n}");
        // The driver scales a place on the tablet to 0 to 65535, rounding down; the seat
        // gives x in the value and y in the field after it.
        let scaled = |v: u32| v * (n as u32 + 1) * 65535 / 32767;
        let want = (scaled(TABLET_AT.0) as i32, scaled(TABLET_AT.1));
        let who = format!("tablet {n}");
        let (place, _) = seat.until(30_000, &who, |e| e.kind == seat::EVENT_POSITION)?;
        if (place.value, place.first) != want || place.device != tablet.id {
            return Err(format!(
                "{who} (device {}) pointed at {want:?}; what came was {place:?}",
                tablet.id
            ));
        }
        for value in [input::VALUE_PRESSED, input::VALUE_RELEASED] {
            let (button, _) = seat.until(10_000, &who, |e| e.kind == seat::EVENT_BUTTON)?;
            if button.device != tablet.id
                || button.code != input::BUTTON_LEFT
                || button.value != value as i32
            {
                return Err(format!("{who}'s click gave {button:?}"));
            }
        }
    }
    Ok(tablets.len())
}

/// The input service's self-test. Starts the input service, which stays running for the
/// tests that follow. Returns a short summary.
pub fn run(sup: &Supervisor) -> Result<String, String> {
    sup.start(Kind::Input, "")?;
    permissions(sup)?;

    // Check 2: taking the seat. The first thing in a new ring is a reset.
    let c = client(sup, SEAT_CONSUMER)?;
    let mut first = take(&c)?;
    match first.next(time::after_ms(5_000))? {
        Some(e) if e.kind == seat::EVENT_RESET => {}
        other => return Err(format!("a new consumer's first event is {other:?}, not a reset")),
    }
    // A second taker (as the display server will be when it takes over from the console):
    // the first one's doorbell stops working. (Its ring's memory is taken back too; reading
    // that would be a fault, so the doorbell is what is checked.)
    let other = client(sup, SEAT_CONSUMER)?;
    let mut seat = take(&other)?;
    match first.doorbell.wait(time::NOW) {
        Err(Error::TimedOut) | Ok(_) => {
            return Err(
                "the first consumer's doorbell still works after a second took the seat".into()
            );
        }
        Err(_) => {}
    }
    // (Dropping it unmaps the ring, which is all that is left to do with it.)
    drop(first);
    match seat.next(time::after_ms(5_000))? {
        Some(e) if e.kind == seat::EVENT_RESET => {}
        other => return Err(format!("the second consumer's first event is {other:?}")),
    }
    let devices = settled(&other)?;
    let count = |kind: DeviceKind| devices.iter().filter(|d| d.kind == kind).count();
    let (keyboards, mice) = (count(DeviceKind::Keyboard), count(DeviceKind::Mouse));
    if mice == 0 {
        return Err(format!("the seat has no mouse: {devices:?}"));
    }
    let mut ids: Vec<u16> = devices.iter().map(|d| d.id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != devices.len() || ids.contains(&0) || devices.iter().any(|d| d.name.is_empty()) {
        return Err(format!("the seat's devices are {devices:?}"));
    }
    // Whatever arrived while the devices were found (word of each one coming).
    seat.all(200)?;

    held_through_a_crash(sup, &other, &mut seat)?;
    let tablets = tablets(&other, &mut seat)?;
    Ok(String::from(match (keyboards, mice, tablets) {
        (_, _, 0) => "who may take the seat, a key held through its driver's crash",
        _ => "who may take the seat, a key held through its driver's crash, tablets",
    }))
}
