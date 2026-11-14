// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `windowprobe`: a hostile windowed app that tries what the display server promises to refuse
//! (docs/design/0022-display-server.md, section 8; note 0026, section 8). It is launched
//! without the focus and never gets it.
//!
//! Each attempt is logged `windowprobe: refused: WHAT (ERR)` when the display server refuses
//! it as expected. The first attempt that is not refused as expected logs `windowprobe:
//! FAILED: WHAT: got RESULT` and ends the program with a number that is not 0 (the number of
//! the attempt). If all are refused it logs `windowprobe: all N attempts were refused` and
//! exits 0.
//!
//! Most calls go straight to the interface (`Connection::client`), since `lib/window` is
//! written to make the right calls, not the wrong ones.
//!
//! It also tries the file picker's rules (note 0025, section 1): asking without a press in its
//! window, or for a window it hasn't got, is refused; and once the runner has clicked its
//! window (it logs `windowprobe: click me` and waits), a second picker while the first is open
//! is refused as busy. The runner then cancels the first.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Debug;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::lock::Lock;
use granite::memory::Memory;
use granite::notification::Notification;
use granite::time;
use pbr_interfaces::pbr_picker::{self as picker, PickerError, Place};
use pbr_interfaces::pbr_window::{
    ClipForm, Edge, Point, Size, WindowError, WindowFlags, WindowKind,
};
use pbr_window::{Connection, Event, PointerKind, Win};

granite::main!(main);

/// A call's result through the generated client.
type Reply<T> = Result<T, CallError<WindowError>>;

/// The most windows a client may have (note 0022, section 3).
const MAX_WINDOWS: u32 = 32;

/// The attempts so far.
struct Probe {
    attempts: u32,
}

impl Probe {
    /// `got` must be the display server refusing with `want`. The `Err` is the exit status.
    fn refused<T: Debug>(
        &mut self,
        what: &str,
        got: Reply<T>,
        want: WindowError,
    ) -> Result<(), i32> {
        match got {
            Err(CallError::Service(e)) if e == want => {
                self.attempts += 1;
                granite::println!("windowprobe: refused: {what} ({e:?})");
                Ok(())
            }
            other => self.failed(what, &other),
        }
    }

    fn failed<T: Debug>(&self, what: &str, got: &T) -> Result<(), i32> {
        granite::println!("windowprobe: FAILED: {what}: got {got:?}");
        Err(self.attempts as i32 + 1)
    }

    /// `got` must have worked (a step the probe needs, not an attempt to be refused).
    fn worked<T: Debug>(&self, what: &str, got: Reply<T>) -> Result<T, i32> {
        match got {
            Ok(v) => Ok(v),
            Err(e) => {
                self.failed(what, &Err::<(), _>(e))?;
                unreachable!("failed always returns an error")
            }
        }
    }
}

fn main(mut start: granite::Start) -> i32 {
    // Its windows must not have the focus for the clipboard checks below (7). It is launched as
    // the user would launch it, and an app's first window takes the focus if it opens within 2
    // seconds of the launch (note 0022, section 4), so it waits longer than that first.
    granite::time::sleep_ms(2_500);
    let mut conn = match Connection::from_start(&mut start) {
        Ok(c) => c,
        Err(e) => {
            granite::println!("windowprobe: FAILED: connecting: got {e:?}");
            return 100;
        }
    };
    // Its capability for the file picker (its manifest asks for `picker`), at role 1004.
    let Some(pk) = start.take(1004) else {
        granite::println!("windowprobe: FAILED: no file picker at role 1004");
        return 109;
    };
    let mut probe = Probe { attempts: 0 };
    match run(&mut probe, &mut conn, Endpoint(pk)) {
        Ok(()) => {
            granite::println!("windowprobe: all {} attempts were refused", probe.attempts);
            0
        }
        Err(code) => code,
    }
}

/// Waits for `win`'s configure event and gives its serial. `Connection` asks for the window's
/// buffers as it passes.
fn wait_configure(conn: &mut Connection, win: Win) -> Option<u32> {
    let deadline = time::after_ms(5_000);
    loop {
        if let Event::Configure { window, serial, .. } = conn.next_event(deadline)?
            && window == win
        {
            return Some(serial);
        }
    }
}

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

/// Waits up to two minutes for a button press in `win`: its serial.
fn wait_press(conn: &mut Connection, win: Win) -> Option<u32> {
    let deadline = time::after_ms(120_000);
    loop {
        if let Event::Pointer(e) = conn.next_event(deadline)?
            && e.window == win
            && matches!(e.what, PointerKind::Button { pressed: true, .. })
        {
            return Some(e.serial);
        }
    }
}

/// The picker's rules (note 0025, section 1), from window `w`.
fn the_picker(p: &mut Probe, conn: &mut Connection, w: Win, pk: Endpoint) -> Result<(), i32> {
    let mut pk = picker::Client::new(pk);
    pk.set_timeout_ms(10_000);
    let n = w.number();
    let open = |c: &picker::Client, window: u32, serial: u32| {
        c.open(window, serial, "", Place::Documents, &[], false, false)
    };
    let refused = |p: &mut Probe,
                   what: &str,
                   got: Result<picker::open::Results, CallError<PickerError>>,
                   want: PickerError| {
        match got {
            Err(CallError::Service(e)) if e == want => {
                p.attempts += 1;
                granite::println!("windowprobe: refused: {what} ({e:?})");
                Ok(())
            }
            other => p.failed(what, &other.map(|r| r.path)),
        }
    };
    refused(p, "the picker with serial 0", open(&pk, n, 0), PickerError::NotAllowed)?;
    refused(p, "the picker with a made-up serial", open(&pk, n, 12345), PickerError::NotAllowed)?;
    refused(
        p,
        "the picker for a window it hasn't",
        open(&pk, 999, 12345),
        PickerError::NotAllowed,
    )?;
    let r = pk.save(n, 12345, "", Place::Documents, "x.txt", &[]);
    refused(
        p,
        "saving with a made-up serial",
        r.map(|r| picker::open::Results { path: r.path, node: r.node }),
        PickerError::NotAllowed,
    )?;
    // A real press: the runner clicks the window. The first picker opens (the runner cancels
    // it); a second, asked while it is open, is refused.
    granite::println!("windowprobe: click me");
    let serial = wait_press(conn, w).ok_or_else(|| {
        granite::println!("windowprobe: FAILED: nobody clicked its window");
        110
    })?;
    // The first waits for the user, so it is asked on a thread of its own. (`Arc` shares the
    // client between the two threads; `Lock` the place the answer is left.)
    pk.set_timeout_ms(300_000);
    let pk = Arc::new(pk);
    let answer: Arc<Lock<Option<Result<(), PickerError>>>> = Arc::new(Lock::new(None));
    let (theirs, slot) = (pk.clone(), answer.clone());
    let first = granite::thread::spawn(move || {
        let r = open(&theirs, n, serial).map(|_| ()).map_err(|e| match e {
            CallError::Service(e) => e,
            _ => PickerError::Failed,
        });
        *slot.lock() = Some(r);
    })
    .map_err(|_| 111)?;
    granite::time::sleep_ms(500);
    refused(p, "a second picker while one is open", open(&pk, n, serial), PickerError::Busy)?;
    let _ = first.join();
    match answer.lock().take() {
        Some(Err(PickerError::Cancelled)) => {
            granite::println!("windowprobe: the first picker was cancelled");
            Ok(())
        }
        other => p.failed("the first picker", &other),
    }
}

fn run(p: &mut Probe, conn: &mut Connection, pk: Endpoint) -> Result<(), i32> {
    let none = WindowFlags::default();
    let origin = Point { x: 0, y: 0 };

    // 1. The ring is asked for once.
    let bell = Notification::create().map_err(|_| 101)?;
    p.refused("a second events call", conn.client().events(bell), WindowError::BadArgument)?;

    // 2. Window kinds only the system may make.
    for (kind, what) in [
        (WindowKind::System, "a System window"),
        (WindowKind::Background, "a Background window"),
        (WindowKind::Panel, "a Panel window"),
    ] {
        let r = conn.client().create(kind, 0, "x", &size(50, 50), none, &origin, 0);
        p.refused(what, r, WindowError::NotAllowed)?;
    }

    // 3. The desktop's operations.
    let c = conn.client();
    p.refused("screens", c.screens(), WindowError::NotAllowed)?;
    p.refused("arrange", c.arrange(&[]), WindowError::NotAllowed)?;
    p.refused("windows", c.windows(), WindowError::NotAllowed)?;
    p.refused("act", c.act(1 << 32 | 1, 1), WindowError::NotAllowed)?;
    p.refused("pointer", c.pointer(100, &[]), WindowError::NotAllowed)?;

    // 4. The system clients' operations.
    p.refused("check_serial", c.check_serial(1, 1, 1), WindowError::NotAllowed)?;
    let r = c.create_over(1, 1, "x", &size(50, 50), none);
    p.refused("create_over", r, WindowError::NotAllowed)?;

    // 5. Its own window, and what it may not do with it. The window is made and, before
    // anything reads its configure event (which `Connection` answers by asking for buffers),
    // a commit finds none.
    // Resizable, so that `start_resize` gets as far as the serial.
    let win = conn.create(WindowKind::Normal, "probe", (100, 80), WindowFlags::RESIZABLE).map_err(
        |e| {
            granite::println!("windowprobe: FAILED: making its own window: got {e:?}");
            102
        },
    )?;
    let w = win.number();
    let c = conn.client();
    p.refused("a commit before it has buffers", c.commit(w, 0, &[]), WindowError::Stale)?;
    let serial = wait_configure(conn, win).ok_or_else(|| {
        granite::println!("windowprobe: FAILED: its own window: got no configure event");
        103
    })?;
    let c = conn.client();
    let popup =
        |serial: u32| c.create(WindowKind::Popup, w, "p", &size(20, 20), none, &origin, serial);
    p.refused("a popup with serial 0", popup(0), WindowError::BadSerial)?;
    p.refused("a popup with a made-up serial", popup(12345), WindowError::BadSerial)?;
    p.refused("start_move with a made-up serial", c.start_move(w, 12345), WindowError::BadSerial)?;
    let r = c.start_resize(w, Edge::Right, 12345);
    p.refused("start_resize with a made-up serial", r, WindowError::BadSerial)?;
    p.refused("buffers with serial 0", c.buffers(w, 0), WindowError::BadSerial)?;
    let r = c.buffers(w, serial.wrapping_add(1000));
    p.refused("buffers with a serial from the future", r, WindowError::BadSerial)?;
    // (`Connection` has asked for the real buffers already.)
    p.refused("a commit of frame 2", c.commit(w, 2, &[]), WindowError::BadArgument)?;
    p.worked("a commit of frame 0", c.commit(w, 0, &[]))?;
    p.refused("a commit of frame 0 again", c.commit(w, 0, &[]), WindowError::Stale)?;

    // 6. Windows it does not have.
    p.refused("destroy of window 999", c.destroy(999), WindowError::NoSuchWindow)?;
    p.refused("set_title of window 999", c.set_title(999, "x"), WindowError::NoSuchWindow)?;
    p.refused("buffers of window 999", c.buffers(999, 1), WindowError::NoSuchWindow)?;
    p.refused("commit of window 999", c.commit(999, 0, &[]), WindowError::NoSuchWindow)?;
    p.refused("frame of window 999", c.frame(999), WindowError::NoSuchWindow)?;

    // 7. The clipboard, without the focus.
    let text = Memory::create(5).map_err(|_| 104)?;
    text.write(0, b"stolen").map_err(|_| 104)?;
    p.refused(
        "copy without the focus",
        c.copy(1, ClipForm::Text, text, 5, 0, 0),
        WindowError::NotFocused,
    )?;
    p.refused("paste without the focus", c.paste(1, ClipForm::Text), WindowError::NotFocused)?;

    // 8. An answer to a ping it was never sent.
    p.refused("pong with a made-up serial", c.pong(12345), WindowError::BadSerial)?;

    // 9. A window bigger than any screen.
    let big = conn.create(WindowKind::Normal, "big", (16_000, 100), none).map_err(|e| {
        granite::println!("windowprobe: FAILED: making a 16000x100 window: got {e:?}");
        105
    })?;
    let serial = wait_configure(conn, big).ok_or_else(|| {
        granite::println!("windowprobe: FAILED: the big window: got no configure event");
        106
    })?;
    let c = conn.client();
    p.refused(
        "buffers for a window bigger than any screen",
        c.buffers(big.number(), serial),
        WindowError::NoMemory,
    )?;
    conn.destroy(big).map_err(|e| {
        granite::println!("windowprobe: FAILED: destroying the big window: got {e:?}");
        107
    })?;

    // 9b. The file picker's rules, from its own window, while events can still be read.
    the_picker(p, conn, win, pk)?;

    // 10. Scribbling over the ring's bookkeeping (its first 64 bytes: the header the server
    // checks). The display server must go on answering. After this the probe does not rely
    // on events.
    conn.ring_memory().write(0, &[0xA5; 64]).map_err(|_| 108)?;
    let c = conn.client();
    let ring_win = p.worked(
        "making a window after scribbling on the ring",
        c.create(WindowKind::Normal, 0, "ring", &size(40, 40), none, &origin, 0),
    )?;
    let answered = c.set_title(ring_win, "still here");
    p.worked("a call after scribbling on the ring", answered)?;
    p.attempts += 1;
    granite::println!(
        "windowprobe: refused: scribbling on its event ring (the display server still answers)"
    );
    p.worked("destroying the ring window", c.destroy(ring_win))?;

    // 11. Too many windows. It has one (`win`), so the 32nd is the last allowed.
    let mut made: Vec<u32> = Vec::new();
    let mut owned = 1;
    loop {
        match c.create(WindowKind::Normal, 0, "n", &size(16, 16), none, &origin, 0) {
            Ok(n) => {
                made.push(n);
                owned += 1;
                if owned > MAX_WINDOWS + 4 {
                    return p.failed("32 windows a client", &"no limit after 36 windows");
                }
            }
            Err(CallError::Service(WindowError::TooMany)) if owned == MAX_WINDOWS => {
                p.attempts += 1;
                granite::println!(
                    "windowprobe: refused: window {} ({:?})",
                    owned + 1,
                    WindowError::TooMany
                );
                break;
            }
            other => return p.failed(&alloc::format!("the window after {owned}"), &other),
        }
    }
    for n in made {
        p.worked("destroying a spare window", c.destroy(n))?;
    }
    p.worked("destroying its window", c.destroy(w))?;
    Ok(())
}
