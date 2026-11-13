// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The display server's rules, tested through `Desk`'s public calls as the display server
//! makes them (docs/design/0022-display-server.md, section 13): the arrangement of screens,
//! stacking and the focus, where input goes, serials and their one-second life, the clipboard,
//! the limits, and configure and buffer exchanges with a hostile client.

use pbr_compositor::desk::{MAX_WINDOWS, act, flags};
use pbr_compositor::event::{Event, kind, state};
use pbr_compositor::{ClientId, Desk, Out, Output, Place, Tablet};
use pbr_devices::keys::key;
use pbr_interfaces::pbr_window::{self as win, WindowError, WindowFlags, WindowKind};
use pbr_seat::Event as SeatEvent;
use pbr_seat::event::{kind as seat, mods, value};

const MS: u64 = 1_000_000;
const SEC: u64 = 1_000_000_000;

/// A screen of `w` by `h` device pixels, `mm` millimetres wide.
fn output(index: u32, w: u32, h: u32, mm: u32) -> Output {
    Output {
        device: 7,
        index,
        connector: format!("Virtual-{}", index + 1),
        identity: String::from("QEMU 1234"),
        width: w,
        height: h,
        width_mm: mm,
        height_mm: mm * h / w.max(1),
    }
}

/// A desk with screens, one active session, and everything it sent kept in order.
struct H {
    d: Desk,
    now: u64,
    session: u32,
    sent: Vec<(ClientId, Event)>,
    freed: Vec<(u32, u32, u32)>,
}

impl H {
    fn new(screens: &[Output]) -> H {
        let mut d = Desk::new();
        d.set_outputs(screens, 0);
        let session = d.start_session("chris").unwrap();
        assert!(d.activate(session));
        let mut h = H { d, now: 10 * SEC, session, sent: Vec::new(), freed: Vec::new() };
        h.drain();
        h
    }

    fn one() -> H {
        H::new(&[output(0, 1280, 800, 340)])
    }

    fn drain(&mut self) {
        for o in self.d.take_out() {
            match o {
                Out::Event(c, e) => self.sent.push((c, e)),
                Out::Free { window, generation } => {
                    self.freed.push((window.client, window.number, generation))
                }
            }
        }
    }

    /// Takes what `c` was sent since last asked.
    fn events(&mut self, c: ClientId) -> Vec<Event> {
        self.drain();
        let (mine, rest): (Vec<_>, Vec<_>) = self.sent.drain(..).partition(|(to, _)| *to == c);
        self.sent = rest;
        mine.into_iter().map(|(_, e)| e).collect()
    }

    fn kinds(&mut self, c: ClientId) -> Vec<u16> {
        self.events(c).iter().map(|e| e.kind).collect()
    }

    fn later(&mut self, ns: u64) {
        self.now += ns;
        self.d.tick(self.now);
        self.drain();
    }

    /// A client launched by the user just now (so its first window takes the focus), or not.
    fn client(&mut self, app: u32, title: &str, f: u32, launched: bool) -> ClientId {
        let at = if launched { self.now } else { 0 };
        self.d.add_client(self.session, app, title, f, at).unwrap()
    }

    fn create(
        &mut self,
        c: ClientId,
        kind: WindowKind,
        parent: u32,
        size: (u32, u32),
    ) -> Result<u32, WindowError> {
        self.d.create(c, kind, parent, "", size, WindowFlags(0), (0, 0), 0, self.now)
    }

    /// A normal window, with buffers, committed: drawn.
    fn window(&mut self, c: ClientId, size: (u32, u32)) -> u32 {
        let w = self.create(c, WindowKind::Normal, 0, size).unwrap();
        self.draw(c, w);
        w
    }

    /// The serial of the last configure `c`'s window `w` was sent (taking `c`'s events).
    fn configure_serial(&mut self, c: ClientId, w: u32) -> u32 {
        self.events(c)
            .iter()
            .rev()
            .find(|e| e.kind == kind::CONFIGURE && e.window == w)
            .map(|e| e.serial)
            .expect("a configure event")
    }

    fn draw(&mut self, c: ClientId, w: u32) {
        let s = self.configure_serial(c, w);
        self.d.buffers(c, w, s).unwrap();
        self.d.commit(c, w, 0, &[], self.now).unwrap();
        self.drain();
    }

    fn input(&mut self, kind: u16, code: u16, mods: u16, value: i32, first: u32) {
        let e = SeatEvent { kind, device: 1, code, mods, value, first, second: 0, time: self.now };
        self.d.seat(&e, self.now);
        self.drain();
    }

    fn press_key(&mut self, code: u16, m: u16, c: char) {
        self.input(seat::KEY, code, m, value::PRESSED, c as u32);
        self.input(seat::KEY, code, m, value::RELEASED, 0);
    }

    /// Moves the pointer to (x, y), in whole logical pixels, with a mouse.
    fn point(&mut self, x: i32, y: i32) {
        let desc = self.d.describe(0);
        let line = desc.iter().find(|l| l.starts_with("pointer at ")).cloned();
        let (cx, cy) = match line {
            Some(l) => {
                let at = l.split_whitespace().nth(2).unwrap();
                let (a, b) = at.split_once(',').unwrap();
                (a.parse::<i32>().unwrap(), b.parse::<i32>().unwrap())
            }
            None => (0, 0),
        };
        self.input(seat::MOVE, 1, 0, x - cx, 0);
        self.input(seat::MOVE, 2, 0, y - cy, 0);
    }

    fn button(&mut self, down: bool) {
        self.input(seat::BUTTON, 1, 0, if down { value::PRESSED } else { value::RELEASED }, 0);
    }

    fn click(&mut self, x: i32, y: i32) {
        self.point(x, y);
        self.button(true);
        self.button(false);
    }

    /// Where `c`'s window `w` is, from the description: (x, y, width, height).
    fn place(&self, c_app: u32, w: u32) -> (i32, i32, i32, i32) {
        let line = self.line(c_app, w);
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|w| *w == "at").unwrap();
        let (x, y) = words[at + 1].split_once(',').unwrap();
        let (ww, hh) = words[at + 3].split_once('x').unwrap();
        (x.parse().unwrap(), y.parse().unwrap(), ww.parse().unwrap(), hh.parse().unwrap())
    }

    fn line(&self, app: u32, w: u32) -> String {
        self.d
            .describe(0)
            .into_iter()
            .find(|l| l.contains(&format!(" window {w} ")) && l.contains(&format!(" app {app} at")))
            .unwrap_or_else(|| panic!("no window {w} of app {app}: {:?}", self.d.describe(0)))
    }

    fn focused(&self, app: u32, w: u32) -> bool {
        self.line(app, w).ends_with(" focused") || self.line(app, w).contains(" focused ")
    }
}

#[test]
fn the_events_constants_match_the_interface() {
    let pairs: [(u16, u32); 18] = [
        (kind::LOST, win::EVENT_LOST),
        (kind::CONFIGURE, win::EVENT_CONFIGURE),
        (kind::CLOSE, win::EVENT_CLOSE),
        (kind::RELEASED, win::EVENT_RELEASED),
        (kind::FRAME, win::EVENT_FRAME),
        (kind::PING, win::EVENT_PING),
        (kind::KEY, win::EVENT_KEY),
        (kind::POINTER, win::EVENT_POINTER),
        (kind::LEFT, win::EVENT_LEFT),
        (kind::BUTTON, win::EVENT_BUTTON),
        (kind::SCROLL, win::EVENT_SCROLL),
        (kind::CLIPBOARD, win::EVENT_CLIPBOARD),
        (kind::DISCONNECTED, win::EVENT_DISCONNECTED),
        (kind::WINDOWS, win::EVENT_WINDOWS),
        (kind::START_MENU, win::EVENT_START_MENU),
        (kind::WINDOW_MENU, win::EVENT_WINDOW_MENU),
        (kind::SCREENS, win::EVENT_SCREENS),
        (pbr_compositor::event::VALUE_SECOND as u16, win::VALUE_SECOND),
    ];
    for (a, b) in pairs {
        assert_eq!(u32::from(a), b);
    }
    assert_eq!(state::FOCUSED, win::STATE_FOCUSED);
    assert_eq!(state::MAXIMISED, win::STATE_MAXIMISED);
    assert_eq!(state::MINIMISED, win::STATE_MINIMISED);
    assert_eq!(state::SHOWN, win::STATE_SHOWN);
    assert_eq!(pbr_compositor::event::EVENT_SIZE as u32, win::EVENT_SIZE);
    let e = Event::new(kind::KEY, 3, [1, 2, 3], 99).with_serial(42);
    assert_eq!(Event::decode(&e.encode()), e);
}

// ---- Screens -------------------------------------------------------------------------

#[test]
fn screens_go_left_to_right_and_a_sharp_one_is_2x() {
    // 1280 pixels across 340 mm is 95 dots per inch; 2560 across 300 mm is 216.
    let mut d = Desk::new();
    d.set_outputs(&[output(1, 2560, 1600, 300), output(0, 1280, 800, 340)], 0);
    let s = d.space().screens();
    assert_eq!(s.len(), 2);
    // Device and connector order, whatever order the drivers said them in.
    assert_eq!(s[0].output.index, 0);
    assert_eq!((s[0].place.x, s[0].place.y, s[0].place.scale), (0, 0, 100));
    assert_eq!((s[1].place.x, s[1].place.y, s[1].place.scale), (1280, 0, 200));
    assert_eq!(s[1].logical_size(), (1280, 800));
    assert!(s[0].place.main && !s[1].place.main);
}

#[test]
fn a_screen_going_moves_its_windows_and_coming_back_gets_its_place() {
    let mut h = H::new(&[output(0, 1280, 800, 340), output(1, 1024, 768, 270)]);
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    h.d.arrange(
        desk,
        &[(2, Place { x: 1280, y: 100, scale: 200, rotation: 0, mirrors: 0, main: false })],
        h.now,
    )
    .unwrap();
    let c = h.client(7, "Notepad", 0, true);
    let w = h.window(c, (200, 100));
    // Drag it by its title bar onto screen 2.
    let (x, y, _, _) = h.place(7, w);
    h.point(x + 40, y - 10);
    h.button(true);
    h.point(1280 + 100, 100 + 60);
    h.button(false);
    let line = h.line(7, w);
    assert!(line.starts_with("screen 2 ") && line.contains("scale 200"), "{line}");
    let configure = h.events(c).into_iter().rev().find(|e| e.kind == kind::CONFIGURE).unwrap();
    assert_eq!(configure.c & 0xFFFF, 200, "told the new scale");
    let before = h.place(7, w);
    // Screen 2 is unplugged: the window comes to the main screen, as far right as fits.
    h.d.set_outputs(&[output(0, 1280, 800, 340)], h.now);
    let line = h.line(7, w);
    assert!(line.starts_with("screen 1 ") && line.contains("scale 100"), "{line}");
    let after = h.place(7, w);
    assert_eq!(after.1 - before.1, 0 - 100, "same place from the screen's top");
    assert!(after.0 >= 0 && after.0 + 200 <= 1280);
    assert!(h.kinds(desk).contains(&kind::SCREENS));
    // Plugged in again: its place and scale are remembered.
    h.d.set_outputs(&[output(0, 1280, 800, 340), output(1, 1024, 768, 270)], h.now);
    let s = h.d.space().screen(3).expect("a new number");
    assert_eq!((s.place.x, s.place.y, s.place.scale), (1280, 100, 200));
    assert!(h.line(7, w).starts_with("screen 1 "), "no window moves to it by itself");
}

#[test]
fn arrangements_that_cant_be_are_refused_whole() {
    let mut h = H::new(&[output(0, 1280, 800, 340), output(1, 1024, 768, 270)]);
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    let app = h.client(2, "Notepad", 0, false);
    let p = |scale, rotation, mirrors| Place { x: 0, y: 0, scale, rotation, mirrors, main: false };
    assert_eq!(h.d.arrange(app, &[(1, p(100, 0, 0))], h.now), Err(WindowError::NotAllowed));
    for bad in [p(150, 0, 0), p(900, 0, 0), p(100, 45, 0), p(100, 0, 2)] {
        assert_eq!(h.d.arrange(desk, &[(2, bad)], h.now), Err(WindowError::BadArgument));
    }
    assert_eq!(h.d.arrange(desk, &[(9, p(100, 0, 0))], h.now), Err(WindowError::NoSuchWindow));
    assert_eq!(
        h.d.arrange(desk, &[(1, p(100, 0, 0)), (1, p(100, 0, 0))], h.now),
        Err(WindowError::NoSuchWindow)
    );
    // Mirroring a screen that mirrors: refused. Mirroring: the screen leaves the space.
    assert_eq!(
        h.d.arrange(desk, &[(2, p(100, 0, 1)), (1, p(100, 0, 2))], h.now),
        Err(WindowError::BadArgument)
    );
    h.d.arrange(desk, &[(2, p(100, 0, 1))], h.now).unwrap();
    assert_eq!(h.d.space().in_space().count(), 1);
    assert_eq!(h.d.space().shows(2).unwrap().number, 1);
    // Turned a quarter, it is taller than wide.
    h.d.arrange(desk, &[(2, Place { x: 1280, ..p(100, 90, 0) })], h.now).unwrap();
    assert_eq!(h.d.space().screen(2).unwrap().logical_size(), (768, 1024));
    // Its device's top right corner is the space's top left.
    let (x, y) = h.d.space().surface_to_space(2, 65535, 0).unwrap();
    assert_eq!(((x >> 8) as i32, (y >> 8) as i32), (1280, 0));
}

#[test]
fn a_tablet_is_tied_to_its_screen_and_the_pointer_cant_leave_the_space() {
    let mut h = H::new(&[output(0, 1280, 800, 340), output(1, 1024, 768, 270)]);
    h.d.set_tablets(vec![Tablet { id: 5, name: "t0".into() }, Tablet { id: 6, name: "t1".into() }]);
    let pos = |h: &mut H, dev: u16, x: i32, y: u32| {
        let e = SeatEvent {
            kind: seat::POSITION,
            device: dev,
            code: 0,
            mods: 0,
            value: x,
            first: y,
            second: 0,
            time: 0,
        };
        h.d.seat(&e, h.now);
    };
    pos(&mut h, 6, 32768, 32768);
    assert!(
        h.d.describe(0).iter().any(|l| l == "pointer at 1792,384 screen 2"),
        "{:?}",
        h.d.describe(0)
    );
    pos(&mut h, 5, 0, 0);
    assert!(h.d.describe(0).iter().any(|l| l == "pointer at 0,0 screen 1"));
    h.input(seat::MOVE, 1, 0, -500, 0);
    h.input(seat::MOVE, 2, 0, 5000, 0);
    assert!(h.d.describe(0).iter().any(|l| l == "pointer at 0,799 screen 1"));
}

// ---- Stacking and the focus ----------------------------------------------------------

#[test]
fn only_the_user_gives_a_new_window_the_focus() {
    let mut h = H::one();
    let a = h.client(7, "Notepad", 0, true);
    let wa = h.window(a, (300, 200));
    assert!(h.focused(7, wa));
    // An app launched long ago (or not by the user) opens a window: no focus, attention.
    let b = h.client(8, "Snake", 0, false);
    let wb = h.window(b, (300, 200));
    assert!(h.focused(7, wa));
    assert!(h.line(8, wb).contains("attention"));
    // The app with the focus opens a second window: it gets it.
    let wa2 = h.window(a, (100, 100));
    assert!(h.focused(7, wa2));
    // Launched by the user more than two seconds ago: too late.
    let c = h.client(9, "Paint", 0, true);
    h.later(3 * SEC);
    let wc = h.window(c, (100, 100));
    assert!(!h.focused(9, wc) && h.focused(7, wa2));
    // Clicking it gives it the focus and raises it.
    let (x, y, _, _) = h.place(9, wc);
    h.click(x + 5, y + 5);
    assert!(h.focused(9, wc));
    assert!(!h.line(9, wc).contains("attention"));
}

#[test]
fn dialogs_stay_above_and_a_modal_one_blocks_its_parent() {
    let mut h = H::one();
    let a = h.client(7, "Notepad", 0, true);
    let w = h.window(a, (400, 300));
    let dlg = h
        .d
        .create(a, WindowKind::Dialog, w, "Save?", (200, 100), WindowFlags::MODAL, (0, 0), 0, h.now)
        .unwrap();
    h.draw(a, dlg);
    assert!(h.focused(7, dlg));
    h.events(a);
    // A click on the parent, outside the dialog: the dialog keeps the focus, the parent hears
    // nothing of the button.
    let (x, y, _, _) = h.place(7, w);
    h.click(x + 3, y + 290);
    assert!(h.focused(7, dlg));
    assert!(!h.events(a).iter().any(|e| e.kind == kind::BUTTON));
    // Another app's window comes above both; choosing the parent brings the dialog up with it.
    let b = h.client(8, "Snake", 0, true);
    let wb = h.window(b, (500, 400));
    assert!(h.focused(8, wb));
    h.click(x + 3, y + 290);
    assert!(h.focused(7, dlg));
    let order: Vec<String> =
        h.d.describe(0).into_iter().filter(|l| l.contains(" window ")).collect();
    let pos = |app: u32, win: u32| {
        order
            .iter()
            .position(|l| {
                l.contains(&format!(" window {win} ")) && l.contains(&format!(" app {app} at"))
            })
            .unwrap()
    };
    assert!(pos(8, wb) < pos(7, w) && pos(7, w) < pos(7, dlg));
    // Windows are numbered per client, so a client can only ever name its own: a number it
    // doesn't have is no window at all.
    assert_eq!(
        h.d.create(b, WindowKind::Dialog, 9, "", (10, 10), WindowFlags(0), (0, 0), 0, h.now),
        Err(WindowError::NoSuchWindow)
    );
}

#[test]
fn minimising_moves_the_focus_and_alt_tab_cycles() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let wa = h.window(a, (200, 200));
    let b = h.client(8, "B", 0, true);
    let wb = h.window(b, (200, 200));
    let c = h.client(9, "C", 0, true);
    let wc = h.window(c, (200, 200));
    assert!(h.focused(9, wc));
    h.d.minimise(c, wc, h.now).unwrap();
    assert!(h.focused(8, wb));
    assert!(h.line(9, wc).contains("minimised"));
    // Alt+Tab, Tab, Tab while Alt is held: B (top) -> A -> C (restored) -> back.
    h.input(seat::KEY, key::LEFT_ALT, mods::ALT, value::PRESSED, 0);
    h.press_key(key::TAB, mods::ALT, '\t');
    assert!(h.focused(7, wa));
    h.press_key(key::TAB, mods::ALT, '\t');
    assert!(h.focused(9, wc) && !h.line(9, wc).contains("minimised"));
    h.input(seat::KEY, key::LEFT_ALT, 0, value::RELEASED, 0);
    assert!(!h.events(a).iter().any(|e| e.kind == kind::KEY && (e.a & 0xFFFF) as u16 == key::TAB));
}

#[test]
fn layers_backgrounds_panels_popups_and_system_windows() {
    let mut h = H::one();
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    let app = h.client(7, "Notepad", 0, true);
    // Only the desktop has backgrounds and panels, and only system clients system windows.
    assert_eq!(
        h.d.create(app, WindowKind::Background, 0, "", (1, 1), WindowFlags(0), (1, 0), 0, h.now),
        Err(WindowError::NotAllowed)
    );
    assert_eq!(
        h.d.create(app, WindowKind::Panel, 0, "", (1, 40), WindowFlags(0), (1, 2), 0, h.now),
        Err(WindowError::NotAllowed)
    );
    assert_eq!(h.create(app, WindowKind::System, 0, (100, 100)), Err(WindowError::NotAllowed));
    let bg =
        h.d.create(desk, WindowKind::Background, 0, "", (1, 1), WindowFlags(0), (1, 0), 0, h.now)
            .unwrap();
    assert_eq!(h.place(1, bg), (0, 0, 1280, 800));
    assert_eq!(
        h.d.create(desk, WindowKind::Background, 0, "", (1, 1), WindowFlags(0), (1, 0), 0, h.now),
        Err(WindowError::TooMany)
    );
    let panel =
        h.d.create(desk, WindowKind::Panel, 0, "", (1, 40), WindowFlags(0), (1, 2), 0, h.now)
            .unwrap();
    assert_eq!(h.place(1, panel), (0, 760, 1280, 40));
    assert_eq!(h.d.work_area(1), Some(pbr_draw::Rect::new(0, 0, 1280, 760)));
    // A panel too thick for its screen.
    assert_eq!(
        h.d.create(desk, WindowKind::Panel, 0, "", (1, 300), WindowFlags(0), (1, 1), 0, h.now),
        Err(WindowError::BadArgument)
    );
    // Maximised, a window fills the work area, above the panel.
    let w = h.window(app, (300, 200));
    h.d.maximise(app, w, true, h.now).unwrap();
    assert_eq!(h.place(7, w), (1, 27, 1278, 732));
    let order: Vec<String> =
        h.d.describe(0).into_iter().filter(|l| l.contains(" window ")).collect();
    assert!(
        order[0].contains("background") && order[1].contains("app 7") && order[2].contains("panel")
    );
}

#[test]
fn the_desktops_list_and_acting_on_windows() {
    let mut h = H::one();
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    let a = h.client(7, "Notepad", 0, true);
    let wa = h.window(a, (300, 200));
    h.d.set_title(a, wa, "letter.txt").unwrap();
    let b = h.client(8, "Snake", 0, true);
    let wb = h.window(b, (300, 200));
    assert_eq!(h.d.window_list(a), Err(WindowError::NotAllowed));
    let list = h.d.window_list(desk).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].title, "Notepad \u{2014} letter.txt");
    assert!(list[1].focused && !list[0].focused);
    h.d.act(desk, list[0].window, act::FOCUS, h.now).unwrap();
    assert!(h.focused(7, wa));
    h.d.act(desk, list[1].window, act::MINIMISE, h.now).unwrap();
    assert!(h.line(8, wb).contains("minimised"));
    h.d.act(desk, list[1].window, act::RESTORE, h.now).unwrap();
    assert!(!h.line(8, wb).contains("minimised"));
    h.events(b);
    h.d.act(desk, list[1].window, act::CLOSE, h.now).unwrap();
    assert!(h.kinds(b).contains(&kind::CLOSE));
    assert_eq!(h.d.act(a, list[1].window, act::CLOSE, h.now), Err(WindowError::NotAllowed));
    assert_eq!(
        h.d.act(desk, list[1].window ^ 0xFF, act::CLOSE, h.now),
        Err(WindowError::NoSuchWindow)
    );
    assert!(h.kinds(desk).contains(&kind::WINDOWS));
}

// ---- Input ---------------------------------------------------------------------------

#[test]
fn keys_go_only_to_the_focused_window_and_releases_follow_presses() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let wa = h.window(a, (200, 200));
    let b = h.client(8, "B", 0, true);
    let _wb = h.window(b, (200, 200));
    h.events(a);
    h.events(b);
    h.input(seat::KEY, key::H, 0, value::PRESSED, 'h' as u32);
    let to_b = h.events(b);
    let k = to_b.iter().find(|e| e.kind == kind::KEY).expect("b gets the key");
    assert_eq!((k.a & 0xFFFF, k.b, k.c), (u32::from(key::H), 1, 'h' as u32));
    assert_ne!(k.serial, 0);
    assert!(!h.events(a).iter().any(|e| e.kind == kind::KEY));
    // The focus moves to A while H is held: the release still goes to B.
    let (x, y, _, _) = h.place(7, wa);
    h.click(x + 5, y + 5);
    h.events(a);
    h.input(seat::KEY, key::H, 0, value::RELEASED, 0);
    assert!(h.events(b).iter().any(|e| e.kind == kind::KEY && e.b == 0));
    assert!(!h.events(a).iter().any(|e| e.kind == kind::KEY));
}

#[test]
fn the_keys_the_display_server_keeps() {
    let mut h = H::one();
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    let a = h.client(7, "A", 0, true);
    let wa = h.window(a, (200, 200));
    h.events(a);
    h.press_key(key::LEFT_META, mods::META, '\0');
    h.press_key(key::ESCAPE, mods::CTRL, '\u{1b}');
    let starts = h.kinds(desk).iter().filter(|k| **k == kind::START_MENU).count();
    assert_eq!(starts, 2);
    h.press_key(key::W, mods::CTRL, 'w');
    let to_a = h.events(a);
    assert!(to_a.iter().any(|e| e.kind == kind::CLOSE && e.window == wa));
    assert!(!to_a.iter().any(|e| e.kind == kind::KEY), "none of those keys reach the app");
    // Ctrl+S does: shortcuts are the app's.
    h.press_key(key::S, mods::CTRL, 's');
    assert!(h.events(a).iter().any(|e| e.kind == kind::KEY && e.c == 's' as u32));
}

#[test]
fn a_dead_key_and_a_letter_that_dont_combine_type_two_characters() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let _w = h.window(a, (200, 200));
    h.events(a);
    let e = SeatEvent {
        kind: seat::KEY,
        device: 1,
        code: key::Q,
        mods: 0,
        value: value::PRESSED,
        first: '^' as u32,
        second: 'q' as u32,
        time: 0,
    };
    h.d.seat(&e, h.now);
    let keys: Vec<Event> = h.events(a).into_iter().filter(|e| e.kind == kind::KEY).collect();
    assert_eq!(keys.len(), 2);
    assert_eq!((keys[0].b, keys[0].c), (1, '^' as u32));
    assert_eq!((keys[1].b, keys[1].c), (4, 'q' as u32));
    assert_eq!(keys[0].serial, keys[1].serial);
}

#[test]
fn the_pointer_goes_to_the_window_under_it_or_the_one_pressed_in() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let wa = h.window(a, (200, 200));
    let b = h.client(8, "B", 0, true);
    let wb = h.window(b, (200, 200));
    let (ax, ay, _, _) = h.place(7, wa);
    let (bx, by, _, _) = h.place(8, wb);
    assert!(bx > ax + 10, "cascaded");
    h.events(a);
    h.events(b);
    // Over A where B doesn't cover it.
    h.point(ax + 2, ay + 100);
    let pa = h.events(a);
    let p = pa.iter().find(|e| e.kind == kind::POINTER).unwrap();
    assert_eq!((p.a as i32 >> 8, p.b as i32 >> 8), (2, 100));
    assert!(h.events(b).is_empty());
    // Press in A, drag over B and off it: everything still goes to A, even outside.
    h.button(true);
    h.point(bx + 100, by + 100);
    h.point(1270, 790);
    let pa = h.events(a);
    assert!(pa.iter().any(|e| e.kind == kind::POINTER && (e.a as i32 >> 8) == 1270 - ax));
    assert!(!h.events(b).iter().any(|e| matches!(e.kind, kind::POINTER | kind::BUTTON)));
    h.button(false);
    assert!(h.events(a).iter().any(|e| e.kind == kind::BUTTON && e.b == 0));
    // Let go: the pointer is over nothing now.
    // (A was raised by the press: this is a part of B that A doesn't cover.)
    h.point(bx + 190, by + 190);
    assert!(h.events(b).iter().any(|e| e.kind == kind::POINTER));
}

#[test]
fn dragging_a_title_bar_moves_and_its_buttons_act() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (300, 200));
    let (x, y, _, _) = h.place(7, w);
    h.point(x + 50, y - 10);
    h.button(true);
    h.point(x + 150, y + 90);
    h.button(false);
    assert_eq!(h.place(7, w), (x + 100, y + 100, 300, 200));
    // The close button asks; minimise does.
    let (x, y, ..) = h.place(7, w);
    h.events(a);
    let close = pbr_compositor::decor::button(
        pbr_draw::Rect::new(x, y, 300, 200),
        pbr_compositor::decor::Button::Close,
    );
    h.click(close.x + 8, close.y + 8);
    assert!(h.kinds(a).contains(&kind::CLOSE));
    // Pressed on the button but let go off it: nothing.
    h.point(close.x + 8, close.y + 8);
    h.button(true);
    h.point(close.x - 100, close.y + 8);
    h.button(false);
    assert!(!h.kinds(a).contains(&kind::CLOSE));
    // A double click on the title bar maximises.
    h.click(x + 50, y - 10);
    h.click(x + 50, y - 10);
    assert!(h.line(7, w).contains("maximised"));
}

#[test]
fn resizing_by_an_edge_keeps_the_limits() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h
        .d
        .create(a, WindowKind::Normal, 0, "", (300, 200), WindowFlags::RESIZABLE, (0, 0), 0, h.now)
        .unwrap();
    h.draw(a, w);
    h.d.set_limits(a, w, (150, 100), (400, 0)).unwrap();
    let (x, y, ..) = h.place(7, w);
    // The bottom right corner, just outside the frame.
    h.point(x + 300 + 2, y + 200 + 2);
    h.button(true);
    h.point(x + 900, y + 400);
    h.later(50 * MS);
    h.button(false);
    assert_eq!(h.place(7, w), (x, y, 400, 398));
    // The left edge, past the smallest (the limit says 150; a title bar needs 160).
    h.point(x - 2, y + 50);
    h.button(true);
    h.point(x + 390, y + 50);
    h.button(false);
    assert_eq!(h.place(7, w), (x + 240, y, 160, 398));
    let c = h.events(a).into_iter().rev().find(|e| e.kind == kind::CONFIGURE).unwrap();
    assert_eq!((c.a, c.b), (160, 398));
}

#[test]
fn serials_open_popups_for_one_second_for_their_own_client() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (400, 300));
    let b = h.client(8, "B", 0, false);
    let wb = h.window(b, (400, 300));
    let popup = |h: &mut H, c, parent, serial, at: (i32, i32), size| {
        h.d.create(c, WindowKind::Popup, parent, "", size, WindowFlags(0), at, serial, h.now)
    };
    // No press: no popup.
    assert_eq!(popup(&mut h, a, w, 0, (10, 10), (100, 100)), Err(WindowError::BadSerial));
    let (x, y, ..) = h.place(7, w);
    h.events(a);
    h.click(x + 10, y + 10);
    let serial = h.events(a).iter().find(|e| e.kind == kind::BUTTON && e.b == 1).unwrap().serial;
    // Not B's serial, not for B's window, not too big, not away from its parent.
    assert_eq!(popup(&mut h, b, wb, serial, (10, 10), (100, 100)), Err(WindowError::BadSerial));
    assert_eq!(popup(&mut h, a, w, serial, (10, 10), (700, 100)), Err(WindowError::BadArgument));
    assert_eq!(popup(&mut h, a, w, serial, (900, 10), (100, 100)), Err(WindowError::BadArgument));
    let p = popup(&mut h, a, w, serial, (10, 10), (100, 100)).unwrap();
    h.draw(a, p);
    // A second later the serial is no good.
    h.later(SEC + 1);
    assert_eq!(popup(&mut h, a, w, serial, (10, 10), (100, 100)), Err(WindowError::BadSerial));
    // A click outside the popup dismisses it.
    h.events(a);
    h.click(x + 300, y + 250);
    assert!(h.events(a).iter().any(|e| e.kind == kind::CLOSE && e.window == p));
    assert!(h.line(7, p).contains("dismissed"));
}

#[test]
fn start_move_only_while_the_press_is_held() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (300, 200));
    let (x, y, ..) = h.place(7, w);
    h.events(a);
    h.point(x + 10, y + 10);
    h.button(true);
    let serial = h.events(a).iter().find(|e| e.kind == kind::BUTTON).unwrap().serial;
    h.d.start_move(a, w, serial, h.now).unwrap();
    h.point(x + 60, y + 30);
    h.button(false);
    assert_eq!(h.place(7, w).0, x + 50);
    assert_eq!(h.d.start_move(a, w, serial, h.now), Err(WindowError::BadSerial));
}

// ---- Pings ---------------------------------------------------------------------------

#[test]
fn a_client_that_doesnt_answer_is_not_responding_until_it_does() {
    let mut h = H::one();
    let a = h.client(7, "Hung", 0, true);
    let w = h.create(a, WindowKind::Normal, 0, (200, 200)).unwrap();
    let pings: Vec<Event> = h.events(a).into_iter().filter(|e| e.kind == kind::PING).collect();
    assert_eq!(pings.len(), 1, "pinged when it got the focus");
    let serial = pings[0].serial;
    h.later(4 * SEC);
    assert!(!h.line(7, w).contains("not-responding"));
    h.later(2 * SEC);
    assert!(h.line(7, w).contains("not-responding"));
    assert!(
        h.d.full_title(pbr_compositor::WindowId { client: a, number: w })
            .ends_with("(not responding)")
    );
    assert_eq!(h.d.pong(a, serial + 1000, h.now), Err(WindowError::BadSerial));
    h.d.pong(a, serial, h.now).unwrap();
    assert!(!h.line(7, w).contains("not-responding"));
    // Pinged again two seconds later.
    h.events(a);
    h.later(2 * SEC);
    assert!(h.kinds(a).contains(&kind::PING));
}

// ---- Buffers, commits and frames, with a hostile client ------------------------------

#[test]
fn buffers_need_the_newest_configures_serial() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let b = h.client(8, "B", 0, true);
    let w = h.create(a, WindowKind::Normal, 0, (300, 200)).unwrap();
    let wb = h.create(b, WindowKind::Normal, 0, (300, 200)).unwrap();
    let first = h.configure_serial(a, w);
    let theirs = h.configure_serial(b, wb);
    h.d.maximise(a, w, true, h.now).unwrap();
    let newest = h.configure_serial(a, w);
    assert_eq!(h.d.buffers(a, w, first).map(|_| ()), Err(WindowError::Stale));
    assert_eq!(h.d.buffers(a, w, theirs).map(|_| ()), Err(WindowError::BadSerial));
    assert_eq!(h.d.buffers(a, w, 0).map(|_| ()), Err(WindowError::BadSerial));
    assert_eq!(h.d.buffers(a, 9, theirs).map(|_| ()), Err(WindowError::NoSuchWindow));
    // B's window 1 is its own, and A's serial is no serial of B's.
    assert_eq!(h.d.buffers(b, wb, newest).map(|_| ()), Err(WindowError::BadSerial));
    let made = h.d.buffers(a, w, newest).unwrap();
    assert_eq!((made.buffers.width, made.buffers.stride), (1278, 1278 * 4));
    // A configure that only changed the state (a click gave it the focus): the same memory.
    let (x, y, ..) = h.place(7, w);
    h.click(x + 5, y + 5);
    let focused = h.configure_serial(a, w);
    assert_ne!(focused, newest);
    assert!(h.d.buffers(a, w, focused).unwrap().reused);
}

#[test]
fn commits_trade_frames_and_old_buffers_are_let_go_of() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h.create(a, WindowKind::Normal, 0, (300, 200)).unwrap();
    assert_eq!(h.d.commit(a, w, 0, &[], h.now), Err(WindowError::Stale), "no buffers yet");
    let s = h.configure_serial(a, w);
    let g1 = h.d.buffers(a, w, s).unwrap().buffers.generation;
    assert_eq!(h.d.commit(a, w, 2, &[], h.now), Err(WindowError::BadArgument));
    h.d.commit(a, w, 0, &[], h.now).unwrap();
    assert_eq!(h.d.commit(a, w, 0, &[], h.now), Err(WindowError::Stale), "frame 0 is the server's");
    h.d.commit(a, w, 1, &[(0, 0, 10, 10), (-50, -50, 99999, 99999)], h.now).unwrap();
    let released: Vec<u32> =
        h.events(a).iter().filter(|e| e.kind == kind::RELEASED).map(|e| e.a).collect();
    assert_eq!(released, vec![0]);
    // Damage stays inside the window's content, whatever the app says.
    let content = pbr_draw::Rect::new(h.place(7, w).0, h.place(7, w).1, 300, 200);
    for r in h.d.take_damage().unwrap_or_default() {
        assert!(r.intersect(&content) == r || r.is_empty() || !content.intersect(&r).is_empty());
    }
    // Resized: new buffers; the old ones stay on the screen until the new ones come.
    h.d.maximise(a, w, true, h.now).unwrap();
    let s = h.configure_serial(a, w);
    let g2 = h.d.buffers(a, w, s).unwrap().buffers.generation;
    assert!(g2 > g1);
    h.drain();
    assert!(h.freed.is_empty());
    h.d.commit(a, w, 0, &[], h.now).unwrap();
    h.drain();
    assert_eq!(h.freed, vec![(a, w, g1)]);
    // Gone: everything is let go of.
    h.d.remove_client(a, h.now);
    h.drain();
    assert!(h.freed.contains(&(a, w, g2)));
}

#[test]
fn frames_only_for_windows_that_can_be_seen() {
    let mut h = H::one();
    let b = h.client(8, "Cover", 0, true);
    let wb = h.window(b, (1000, 700));
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (300, 200));
    h.d.want_frame(a, w).unwrap();
    h.d.send_frames(h.now, &|_| true);
    assert!(!h.kinds(a).contains(&kind::FRAME), "its screen is busy");
    h.d.send_frames(h.now, &|_| false);
    assert!(h.kinds(a).contains(&kind::FRAME));
    // B is raised over A, which is cascaded inside it: A is covered entirely.
    let (bx, by, ..) = h.place(8, wb);
    h.click(bx + 990, by + 690);
    h.d.want_frame(a, w).unwrap();
    h.d.send_frames(h.now, &|_| false);
    assert!(!h.kinds(a).contains(&kind::FRAME), "covered");
    h.d.minimise(b, wb, h.now).unwrap();
    h.d.send_frames(h.now, &|_| false);
    assert!(h.kinds(a).contains(&kind::FRAME));
    h.d.minimise(a, w, h.now).unwrap();
    h.d.want_frame(a, w).unwrap();
    h.d.send_frames(h.now, &|_| false);
    assert!(!h.kinds(a).contains(&kind::FRAME), "minimised");
}

#[test]
fn the_limits() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    assert_eq!(h.create(a, WindowKind::Normal, 0, (0, 10)), Err(WindowError::BadArgument));
    assert_eq!(h.create(a, WindowKind::Normal, 0, (20000, 10)), Err(WindowError::BadArgument));
    assert_eq!(h.create(a, WindowKind::Dialog, 0, (10, 10)), Err(WindowError::BadKind));
    assert_eq!(h.create(a, WindowKind::Normal, 99, (10, 10)), Err(WindowError::BadArgument));
    // Bigger than any screen: no buffers.
    let big = h.create(a, WindowKind::Normal, 0, (1300, 100)).unwrap();
    let s = h.configure_serial(a, big);
    assert_eq!(h.d.buffers(a, big, s).map(|_| ()), Err(WindowError::NoMemory));
    // 96 MiB a session: each of these is 2 x 1280 x 800 x 4 = 8 MB.
    let mut made = 0;
    let mut refused = false;
    for _ in 0..20 {
        let w = h.create(a, WindowKind::Normal, 0, (1280, 800)).unwrap();
        let s = h.configure_serial(a, w);
        match h.d.buffers(a, w, s) {
            Ok(_) => made += 1,
            Err(WindowError::NoMemory) => {
                refused = true;
                break;
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(refused && made == 12, "{made}");
    // 32 windows a client.
    let c = h.client(9, "Many", 0, false);
    for _ in 0..MAX_WINDOWS {
        h.create(c, WindowKind::Normal, 0, (100, 100)).unwrap();
    }
    assert_eq!(h.create(c, WindowKind::Normal, 0, (100, 100)), Err(WindowError::TooMany));
}

// ---- The clipboard -------------------------------------------------------------------

#[test]
fn copying_and_pasting_only_with_the_focus_and_a_fresh_press() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let wa = h.window(a, (200, 200));
    let b = h.client(8, "B", 0, false);
    let wb = h.window(b, (200, 200));
    h.events(a);
    h.press_key(key::C, mods::CTRL, 'c');
    let serial = h.events(a).iter().find(|e| e.kind == kind::KEY).unwrap().serial;
    // B, in the background, can't copy or paste, even with A's serial.
    assert_eq!(h.d.check_copy(b, serial, false, 5, (0, 0), h.now), Err(WindowError::NotFocused));
    assert_eq!(h.d.paste(b, serial, false, h.now).map(|_| ()), Err(WindowError::NotFocused));
    // A can, with its serial, not with none.
    assert_eq!(h.d.check_copy(a, 0, false, 5, (0, 0), h.now), Err(WindowError::BadSerial));
    assert_eq!(
        h.d.check_copy(a, serial, false, 17 * 1024 * 1024, (0, 0), h.now),
        Err(WindowError::NoMemory)
    );
    assert_eq!(h.d.check_copy(a, serial, true, 100, (5, 6), h.now), Err(WindowError::BadArgument));
    h.d.check_copy(a, serial, false, 5, (0, 0), h.now).unwrap();
    assert_eq!(
        h.d.copied(a, serial, None, vec![0xff, 0xfe], h.now),
        Err(WindowError::BadArgument),
        "not UTF-8"
    );
    h.d.copied(a, serial, None, b"hello".to_vec(), h.now).unwrap();
    h.d.copied(a, serial, Some((1, 1)), vec![1, 2, 3, 4], h.now).unwrap();
    assert!(h.d.describe(0).iter().any(|l| l == "clipboard text picture"));
    assert_eq!(h.d.paste(a, serial, true, h.now).unwrap(), (&[1u8, 2, 3, 4][..], 1, 1));
    // A new copy (another serial) replaces both forms.
    h.press_key(key::C, mods::CTRL, 'c');
    let s2 = h.events(a).iter().find(|e| e.kind == kind::KEY).unwrap().serial;
    h.d.copied(a, s2, None, b"bye".to_vec(), h.now).unwrap();
    assert_eq!(h.d.paste(a, s2, true, h.now).map(|_| ()), Err(WindowError::Empty));
    // A second later the serial is no good for pasting.
    h.later(SEC + 1);
    assert_eq!(h.d.paste(a, s2, false, h.now).map(|_| ()), Err(WindowError::BadSerial));
    // B gets the focus and hears what the clipboard holds; A no longer may paste.
    let (x, y, ..) = h.place(8, wb);
    h.click(x + 5, y + 5);
    assert!(h.events(b).iter().any(|e| e.kind == kind::CLIPBOARD && e.a == 1));
    assert!(h.focused(8, wb) && !h.focused(7, wa));
}

// ---- System clients and sessions -----------------------------------------------------

#[test]
fn system_clients_check_serials_and_open_windows_over_others() {
    let mut h = H::one();
    let a = h.client(7, "Notepad", 0, true);
    let w = h.window(a, (400, 300));
    let picker = h.client(3, "Files", flags::SYSTEM | flags::TRUSTED, false);
    let (x, y, ..) = h.place(7, w);
    h.events(a);
    h.click(x + 10, y + 10);
    let serial = h.events(a).iter().find(|e| e.kind == kind::BUTTON && e.b == 1).unwrap().serial;
    assert_eq!(h.d.check_serial(a, 7, w, serial, h.now), Err(WindowError::NotAllowed));
    assert_eq!(h.d.check_serial(picker, 7, w, serial, h.now), Ok(true));
    assert_eq!(h.d.check_serial(picker, 7, w, serial + 1, h.now), Ok(false));
    assert_eq!(h.d.check_serial(picker, 99, w, serial, h.now), Err(WindowError::NoSuchWindow));
    assert_eq!(
        h.d.create_over(a, 7, w, "Open", (300, 200), WindowFlags(0), h.now),
        Err(WindowError::NotAllowed)
    );
    let over = h.d.create_over(picker, 7, w, "Open", (300, 200), WindowFlags(0), h.now).unwrap();
    h.draw(picker, over);
    assert!(h.focused(3, over));
    assert!(h.line(3, over).contains(" system"));
    // A click on Notepad's window goes to the picker instead.
    h.events(a);
    h.click(x + 2, y + 290);
    assert!(h.focused(3, over));
    assert!(!h.events(a).iter().any(|e| e.kind == kind::BUTTON));
    // Notepad's window goes: the picker is told to close.
    h.d.destroy(a, w, h.now).unwrap();
    assert!(h.events(picker).iter().any(|e| e.kind == kind::CLOSE));
}

#[test]
fn a_session_ending_takes_its_windows_and_only_the_active_one_is_shown() {
    let mut h = H::one();
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (200, 200));
    let other = h.d.start_session("guest").unwrap();
    let g = h.d.add_client(other, 1, "G", 0, h.now).unwrap();
    let gw =
        h.d.create(g, WindowKind::Normal, 0, "", (100, 100), WindowFlags(0), (0, 0), 0, h.now)
            .unwrap();
    assert!(h.d.describe(0).iter().all(|l| !l.contains("\"G\"")), "not the active session");
    h.events(a);
    h.input(seat::KEY, key::H, 0, value::PRESSED, 'h' as u32);
    assert!(h.events(a).iter().any(|e| e.kind == kind::KEY));
    assert!(h.events(g).iter().all(|e| e.kind != kind::KEY));
    h.d.end_session(h.session, h.now);
    h.drain();
    assert!(h.freed.iter().any(|f| f.0 == a && f.1 == w));
    assert_eq!(h.d.active(), None);
    assert!(h.d.activate(other));
    assert!(h.d.describe(0).iter().any(|l| l.contains(&format!("window {gw} \"G\""))));
    assert!(h.d.start_session("x").is_some());
}

#[test]
fn the_window_menu_sends_its_choices_to_the_desktop() {
    let mut h = H::new(&[output(0, 1280, 800, 340), output(1, 1024, 768, 270)]);
    let desk = h.client(1, "Desktop", flags::DESKTOP, false);
    let a = h.client(7, "A", 0, true);
    let w = h.window(a, (300, 200));
    let (x, y, ..) = h.place(7, w);
    let icon = pbr_compositor::decor::icon(pbr_draw::Rect::new(x, y, 300, 200));
    h.click(icon.x + 4, icon.y + 4);
    assert!(h.d.describe(0).iter().any(|l| l.starts_with("menu for window")));
    let scene = h.d.scene();
    let menu = scene.menu.expect("a menu");
    let row = menu.items.iter().position(|i| *i == pbr_compositor::MenuItem::Permissions).unwrap();
    h.events(desk);
    h.click(menu.rect.x + 10, menu.rect.y + 2 + row as i32 * 22 + 5);
    let e = h.events(desk).into_iter().find(|e| e.kind == kind::WINDOW_MENU).expect("told");
    assert_eq!((e.a, e.b, e.c), (w, a, 1));
    // Move to screen 2.
    h.click(icon.x + 4, icon.y + 4);
    let menu = h.d.scene().menu.unwrap();
    let row = menu.items.iter().position(|i| *i == pbr_compositor::MenuItem::MoveTo(2)).unwrap();
    h.click(menu.rect.x + 10, menu.rect.y + 2 + row as i32 * 22 + 5);
    assert!(h.line(7, w).starts_with("screen 2 "));
}

#[test]
fn describing_the_screens() {
    let mut h = H::one();
    let a = h.client(7, "Notepad", flags::TRUSTED, true);
    let w = h.window(a, (400, 300));
    h.d.set_title(a, w, "letter.txt").unwrap();
    h.point(100, 100);
    let lines = h.d.describe(5);
    assert_eq!(lines[0], "view 5");
    assert_eq!(lines[1], "screen 1 \"Virtual-1\" 1280x800 at 0,0 scale 100 main");
    let (x, y, ..) = h.place(7, w);
    assert_eq!(
        lines[2],
        format!(
            "screen 1 window 1 \"Notepad \u{2014} letter.txt\" app 7 at {x},{y} size 400x300 scale 100 drawn focused"
        )
    );
    assert_eq!(lines[3], "clipboard empty");
    assert_eq!(lines[4], "pointer at 100,100 screen 1");
    assert_eq!(lines.last().unwrap(), "view 5 done");
}
