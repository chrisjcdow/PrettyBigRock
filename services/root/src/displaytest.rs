// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The display part of the device self-test (docs/design/0020-screens-and-virtio-gpu.md,
//! section 7): every display driver, through `pbr.display`, as the display server will use it.
//!
//! A program can't see its own screen, so the looking is done from outside: this test puts a
//! known picture on every screen and prints a line saying so; the test runner
//! (`xtask/src/qemu.rs`) then asks QEMU for a screenshot of each screen, compares it with the
//! same picture worked out for itself (`pbr_devices::display::test`), and presses a key on the
//! virtio keyboard to say it has looked. If the screenshot is wrong the runner fails the boot.
//!
//! Three pictures are looked at on every screen:
//! 1. a whole pattern, with the kernel printing afterwards (it must no longer draw its text
//!    on a screen a client has taken);
//! 2. the buffer redrawn all over, but presented with only one rectangle named: only that
//!    rectangle may change on the screen;
//! 3. another pattern from a new client, after the driver was crashed and restarted.
//!
//! In between come the rules a client must not be able to break: nothing but `screens` works
//! without taking the display; a second taker ends the first one's buffers; buffers are
//! refused once the driver's memory allowance is used; rectangles outside a buffer are
//! refused.

use crate::blocktest::Devmgr;
use crate::crashtest;
use crate::drivertest::{Pad, keyboard_events};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::write_volatile;
use granite::Error;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::sys::perm;
use granite::time;
use pbr_devices::display::test;
use pbr_interfaces::pbr_devices::DeviceInfo;
use pbr_interfaces::pbr_display::{self as display, DisplayError, Mode, Rect, ScreenInfo};
use pbr_interfaces::pbr_driver_debug::CrashKind;

/// How long a driver may take to say a picture is on the screen.
const DONE_MS: u64 = 10_000;
/// The bit of the done notification that says the screens changed.
const SCREENS_CHANGED: u64 = 1 << 63;

/// A buffer for one screen, mapped.
struct Canvas {
    screen: u32,
    id: u32,
    width: u32,
    height: u32,
    stride: u32,
    memory: Memory,
    map: Mapping,
}

impl Canvas {
    /// Asks `taker` for a buffer for the screen `info` describes, and checks that what comes
    /// back can hold the screen's picture.
    fn make(taker: &display::Client, info: &ScreenInfo, who: &str) -> Result<Canvas, String> {
        let (width, height) = (info.current.width, info.current.height);
        let made = taker
            .create_buffer(info.screen)
            .map_err(|e| format!("{who}: a buffer for screen {}: {e}", info.screen))?;
        let size = made.memory.size().map_err(|e| format!("{who}: the buffer's size: {e}"))?;
        if u64::from(made.stride) < u64::from(width) * 4
            || !made.stride.is_multiple_of(4)
            || size < u64::from(made.stride) * u64::from(height)
        {
            return Err(format!(
                "{who}: a buffer of {size} bytes, {} a row, for a screen of {width}x{height}",
                made.stride
            ));
        }
        let map = made
            .memory
            .map(perm::READ_WRITE)
            .map_err(|e| format!("{who}: mapping a buffer: {e}"))?;
        Ok(Canvas {
            screen: info.screen,
            id: made.buffer,
            width,
            height,
            stride: made.stride,
            memory: made.memory,
            map,
        })
    }

    /// Fills the whole buffer: the pixel at (x, y) is `pixel(x, y)`.
    fn draw(&self, pixel: impl Fn(u32, u32) -> u32) {
        for y in 0..self.height {
            for x in 0..self.width {
                let at = y as usize * self.stride as usize + x as usize * 4;
                // SAFETY: `make` checked that `stride * height` bytes fit in the memory, which
                // is mapped whole and writable, and that a row holds `width` pixels, so this
                // pixel is inside the mapping; `at` is a multiple of 4 (so is the stride) from
                // the start of a page, so it is aligned. Volatile, because the driver reads
                // this memory from another program.
                unsafe { write_volatile(self.map.as_mut_ptr().add(at).cast::<u32>(), pixel(x, y)) };
            }
        }
    }
}

/// A client that has taken a display.
struct Taken {
    taker: display::Client,
    done: Notification,
    canvases: Vec<Canvas>,
}

impl Taken {
    /// Takes the display through `client` and makes a buffer for every connected screen.
    fn new(client: &display::Client, screens: &[ScreenInfo], who: &str) -> Result<Taken, String> {
        let t = client.take().map_err(|e| format!("{who}: take: {e}"))?;
        let mut taker = t.taker;
        taker.set_timeout_ms(DONE_MS);
        let mut canvases = Vec::new();
        for info in screens.iter().filter(|s| s.connected) {
            canvases.push(Canvas::make(&taker, info, who)?);
        }
        Ok(Taken { taker, done: t.done, canvases })
    }

    /// Presents canvas `i` (the rectangles `changed`, or all of it if there are none) and
    /// waits for the driver to say it is on the screen.
    fn show(&self, i: usize, changed: &[Rect], who: &str) -> Result<(), String> {
        let c = &self.canvases[i];
        self.taker
            .present(c.id, changed)
            .map_err(|e| format!("{who}: presenting on screen {}: {e}", c.screen))?;
        let end = time::after_ms(DONE_MS);
        loop {
            match self.done.wait(end) {
                Ok(bits) if bits & (1 << c.screen) != 0 => return Ok(()),
                // Another screen's bit, or the screens changing: not what this waits for.
                Ok(_) => {}
                Err(e) => {
                    return Err(format!("{who}: screen {} never said it was done: {e}", c.screen));
                }
            }
        }
    }

    /// Draws picture `number` on every screen, shows it, and has the runner look.
    fn picture(&self, number: u32, name: &str, key: &mut Pad, who: &str) -> Result<(), String> {
        for (i, c) in self.canvases.iter().enumerate() {
            let (w, h) = (c.width, c.height);
            if number == 2 {
                // All of the buffer changes, but the driver is only told about one rectangle.
                c.draw(|x, y| test::pattern(2, w, h, x, y));
                let r = test::changed(w, h);
                let r = Rect { x: r.x, y: r.y, width: r.width, height: r.height };
                self.show(i, &[r], who)?;
            } else {
                c.draw(|x, y| test::picture(number, w, h, x, y));
                self.show(i, &[], who)?;
            }
        }
        if number == 1 {
            // The kernel talks: if it still drew on the screen, its text would be in the
            // runner's screenshot.
            for i in 0..8 {
                granite::println!(
                    "Device self-test: the kernel log carries on while the screen is drawn on ({i})"
                );
            }
        }
        for c in &self.canvases {
            granite::println!(
                "Device self-test: picture {number} on {name} screen {}: {}x{}",
                c.screen,
                c.width,
                c.height
            );
        }
        granite::println!("Device self-test: look at the pictures ({number} on {name})");
        keyboard_events(key, "the runner's answer about the screens")
    }
}

/// Checks that `r` was refused with `want`.
fn refused<T>(
    r: Result<T, CallError<DisplayError>>,
    want: DisplayError,
    what: &str,
) -> Result<(), String> {
    match r {
        Err(CallError::Service(e)) if e == want => Ok(()),
        Err(e) => Err(format!("{what} was refused with {e}, not {want:?}")),
        Ok(_) => Err(format!("{what} was allowed; it should be refused with {want:?}")),
    }
}

/// The screens of a display, checked: numbered in order, and every connected one showing a
/// mode it lists. `firmware` is the firmware screen's size if this is the framebuffer driver.
fn screens(
    client: &display::Client,
    firmware: Option<(u32, u32)>,
    who: &str,
) -> Result<Vec<ScreenInfo>, String> {
    let listed = client.screens().map_err(|e| format!("{who}: screens: {e}"))?;
    let list = listed.screens;
    if list.is_empty() || list.iter().enumerate().any(|(i, s)| s.screen != i as u32) {
        return Err(format!("{who}: the screens aren't numbered from 0: {list:?}"));
    }
    for s in list.iter().filter(|s| s.connected) {
        let m = &s.current;
        if m.width == 0
            || m.height == 0
            || !s.modes.iter().any(|x| x.width == m.width && x.height == m.height)
        {
            return Err(format!("{who}: screen {} shows a mode it doesn't list: {s:?}", s.screen));
        }
    }
    if !list.iter().any(|s| s.connected) {
        return Err(format!("{who}: nothing is plugged in: {list:?}"));
    }
    if let Some((width, height)) = firmware {
        let ok = list.len() == 1
            && list[0].connector == "Firmware"
            && (list[0].current.width, list[0].current.height) == (width, height);
        if !ok {
            return Err(format!(
                "{who}: the machine has one screen of {width}x{height}, the driver says {list:?}"
            ));
        }
    }
    Ok(list)
}

/// Tests the display `info`. `firmware` is the size the machine description gives the
/// firmware's screen, if this is its driver; `key` is the virtio keyboard the runner answers
/// on. Returns how many screens it has.
pub fn run(
    d: &Devmgr,
    info: &DeviceInfo,
    firmware: Option<(u32, u32)>,
    key: &mut Pad,
) -> Result<usize, String> {
    let name = info.name.as_str();
    let open = || -> Result<display::Client, String> {
        let ep = d.client.open(info.id).map_err(|e| format!("opening {name}: {e}"))?;
        let mut c = display::Client::new(Endpoint(ep));
        c.set_timeout_ms(DONE_MS);
        Ok(c)
    };
    let client = open()?;
    let list = screens(&client, firmware, name)?;
    let first = list.iter().find(|s| s.connected).ok_or("no connected screen")?;
    let mode = || Mode { ..first.current.clone() };

    // Before anyone takes the display, only `screens` works.
    refused(client.create_buffer(first.screen), DisplayError::NotTaken, "a buffer without taking")?;
    refused(client.present(1, &[]), DisplayError::NotTaken, "presenting without taking")?;
    refused(client.destroy_buffer(1), DisplayError::NotTaken, "destroying without taking")?;
    refused(
        client.set_mode(first.screen, &mode()),
        DisplayError::NotTaken,
        "a mode without taking",
    )?;

    // The first taker: pictures 1 and 2.
    let a = Taken::new(&client, &list, name)?;
    // The plain capability still isn't the taker's.
    refused(
        client.present(a.canvases[0].id, &[]),
        DisplayError::NotTaken,
        "presenting through the untaken capability",
    )?;
    refused(
        a.taker.create_buffer(list.len() as u32),
        DisplayError::NoScreen,
        "a buffer for a screen that isn't there",
    )?;
    refused(
        a.taker.present(a.canvases[0].id + 1000, &[]),
        DisplayError::NoSuchBuffer,
        "presenting a buffer that isn't there",
    )?;
    let (w, h) = (a.canvases[0].width, a.canvases[0].height);
    for (x, y, width, height) in
        [(0, 0, 0, 1), (w, 0, 1, 1), (0, h, 1, 1), (1, 0, w, 1), (u32::MAX, 0, 2, 1)]
    {
        let r = Rect { x, y, width, height };
        refused(
            a.taker.present(a.canvases[0].id, &[r]),
            DisplayError::BadArgument,
            "a rectangle outside the buffer",
        )?;
    }
    a.picture(1, name, key, name)?;
    a.picture(2, name, key, name)?;

    // A second taker ends the first: its capability is refused, and its buffers are gone,
    // their memory taken back (the kernel unmapped it, so `a`'s canvases mustn't be drawn in
    // again).
    let b = Taken::new(&client, &list, name)?;
    refused(
        a.taker.present(a.canvases[0].id, &[]),
        DisplayError::NotTaken,
        "the old taker presenting",
    )?;
    refused(
        a.taker.create_buffer(first.screen),
        DisplayError::NotTaken,
        "the old taker making a buffer",
    )?;
    refused(
        b.taker.destroy_buffer(a.canvases[0].id),
        DisplayError::NoSuchBuffer,
        "the old taker's buffer",
    )?;
    match a.canvases[0].memory.size() {
        Err(Error::BadHandle | Error::Revoked) => {}
        other => {
            return Err(format!(
                "{name}: the old taker's buffer memory answers {other:?}; it should be revoked"
            ));
        }
    }
    drop(a);

    // Buffers stop being made when the driver's allowance is used, and destroying them
    // gives the room back.
    let mut extra = Vec::new();
    let full = loop {
        match b.taker.create_buffer(first.screen) {
            Ok(made) => extra.push(made.buffer),
            Err(CallError::Service(DisplayError::NoMemory)) => break extra.len(),
            Err(e) => return Err(format!("{name}: buffer {} was refused with {e}", extra.len())),
        }
        if extra.len() > 64 {
            return Err(format!("{name}: {} buffers and still not refused", extra.len()));
        }
    };
    for id in &extra {
        b.taker.destroy_buffer(*id).map_err(|e| format!("{name}: destroying buffer {id}: {e}"))?;
    }
    if let Some(id) = extra.first() {
        refused(
            b.taker.destroy_buffer(*id),
            DisplayError::NoSuchBuffer,
            "destroying a buffer twice",
        )?;
    }
    for i in 0..full {
        let made = b.taker.create_buffer(first.screen);
        let made = made
            .map_err(|e| format!("{name}: buffer {i} of {full} after destroying them all: {e}"))?;
        b.taker.destroy_buffer(made.buffer).map_err(|e| format!("{name}: destroying: {e}"))?;
    }

    // Setting the mode a screen already shows is allowed, and ends its buffers (they belong
    // to the mode they were made for).
    refused(
        b.taker.set_mode(list.len() as u32, &mode()),
        DisplayError::NoScreen,
        "a mode for a screen that isn't there",
    )?;
    b.taker
        .set_mode(first.screen, &mode())
        .map_err(|e| format!("{name}: setting the same mode: {e}"))?;
    refused(
        b.taker.present(b.canvases[0].id, &[]),
        DisplayError::NoSuchBuffer,
        "a buffer from before the mode was set",
    )?;
    let generation = client.screens().map_err(|e| format!("{name}: screens: {e}"))?.generation;
    match b.done.wait(time::after_ms(DONE_MS)) {
        Ok(bits) if bits & SCREENS_CHANGED != 0 => {}
        other => {
            return Err(format!(
                "{name}: no word that the screens changed after a mode was set: {other:?}"
            ));
        }
    }
    drop(b);

    // The driver crashes and comes back (note 0012); a client finds it again, takes it and
    // draws: picture 3.
    let t0 = time::now();
    crashtest::crash_driver(d, info.id, client.endpoint(), CrashKind::NullWrite)?;
    let again = display::Client::new(Endpoint(crashtest::reconnect(
        d,
        info.id,
        crashtest::RESTART_LIMIT_MS,
    )?));
    let ms = time::now().saturating_sub(t0) / 1_000_000;
    match client.screens() {
        Err(CallError::Call(Error::PeerGone)) => {}
        other => return Err(format!("{name}: the crashed driver's capability answers {other:?}")),
    }
    let list = screens(&again, firmware, name)?;
    let c = Taken::new(&again, &list, name)?;
    c.picture(3, name, key, name)?;
    // A device with more than one screen (the test machines' virtio-gpu): the second is
    // unplugged and plugged in again.
    let plugged = firmware.is_none();
    if plugged {
        several_screens(&again, &list, c, name, key)?;
    }
    granite::println!(
        "Device self-test: {name}: {} screen(s), {full} more buffers than screens, generation {generation}, back {ms} ms after a crash.",
        list.len()
    );
    Ok(list.iter().filter(|s| s.connected).count())
}

/// Waits for the driver to say its screens changed, and returns them as they are now.
fn changed(
    client: &display::Client,
    done: &Notification,
    name: &str,
    what: &str,
) -> Result<Vec<ScreenInfo>, String> {
    let end = time::after_ms(DONE_MS);
    loop {
        match done.wait(end) {
            Ok(bits) if bits & SCREENS_CHANGED != 0 => break,
            Ok(_) => {}
            Err(e) => {
                return Err(format!("{name}: no word that the screens changed after {what}: {e}"));
            }
        }
    }
    Ok(client.screens().map_err(|e| format!("{name}: screens after {what}: {e}"))?.screens)
}

/// The test machines' virtio-gpu has two screens of different sizes plugged in, and an output
/// with nothing in it. The runner unplugs the second screen and plugs it in again; the
/// driver must say so each time, refuse to draw on a screen that isn't there, and draw on
/// both once it is back (picture 4, which the runner looks at).
fn several_screens(
    client: &display::Client,
    list: &[ScreenInfo],
    taken: Taken,
    name: &str,
    key: &mut Pad,
) -> Result<(), String> {
    let on: Vec<&ScreenInfo> = list.iter().filter(|s| s.connected).collect();
    let size = |s: &ScreenInfo| (s.current.width, s.current.height);
    if on.len() != 2 || on[0].screen != 0 || on[1].screen != 1 || size(on[0]) == size(on[1]) {
        return Err(format!(
            "{name}: expected screens 0 and 1 plugged in, of different sizes: {list:?}"
        ));
    }
    if !list.iter().any(|s| !s.connected && s.modes.is_empty() && s.current.width == 0) {
        return Err(format!("{name}: expected an output with nothing plugged in: {list:?}"));
    }
    if on[0].connector == on[1].connector {
        return Err(format!("{name}: two screens with one connector name: {list:?}"));
    }
    let second = size(on[1]);

    granite::println!("Device self-test: unplug screen 1");
    let now = changed(client, &taken.done, name, "unplugging")?;
    if now.len() != list.len() || now[1].connected || !now[0].connected || now[1].current.width != 0
    {
        return Err(format!("{name}: with screen 1 unplugged, the screens are {now:?}"));
    }
    refused(
        taken.taker.present(taken.canvases[1].id, &[]),
        DisplayError::NotConnected,
        "presenting on an unplugged screen",
    )?;
    refused(
        taken.taker.create_buffer(1),
        DisplayError::NotConnected,
        "a buffer for an unplugged screen",
    )?;
    // The first screen is untouched.
    taken.show(0, &[], name)?;

    granite::println!("Device self-test: plug screen 1");
    let now = changed(client, &taken.done, name, "plugging in")?;
    if !now[1].connected || size(&now[1]) != second || !now[0].connected {
        return Err(format!("{name}: with screen 1 plugged in again, the screens are {now:?}"));
    }
    // The buffer made before it was unplugged is still the right size, and still works.
    taken.picture(4, name, key, name)
}
