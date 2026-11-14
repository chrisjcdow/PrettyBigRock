// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The test harness: an app run against a pretend display server, on the host
//! (note 0023, section 6). Only with the `testing` feature (the standard library is used).
//!
//! A test makes a [`Harness`] with the app, clicks and types as a person would ("click the
//! button labelled Save", "type `hello`"), and checks the messages the app was sent, its state,
//! and what its windows show, against reference pictures in `tests/pictures/`.
//!
//! **After every step the harness checks the drawing itself**: it draws each window afresh,
//! whole, and compares that with what sandstone's drawing of only the changes left in the
//! window. A difference means a widget's look ([`Widget::look`](crate::Widget::look)) misses
//! something its pixels depend on, and the test fails there, saying where.
//!
//! The pretend display server behaves as note 0022 says where it matters to apps: windows get
//! a configure event when made, the newest normal window or dialog has the keyboard, popups
//! are made only in answer to a press in the last second and close when the user clicks
//! outside them, and the clipboard is used only with the serial of the last press, within a
//! second.

use crate::app::{App, Context, Settings, WindowId};
use crate::element::{Description, Element};
use crate::event::{Key, Modifiers};
use crate::platform::{NewWindow, Platform};
use crate::runtime::Runtime;
use crate::shortcut::{Shortcut, Trigger};
use pbr_draw::{Canvas, Damage, Rect};
use pbr_interfaces::pbr_input as input;
use pbr_window::{
    Cursor, Edge, Event as Incoming, KeyEvent, PointerEvent, PointerKind, Win, WindowFlags,
    WindowKind,
};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

pub use pbr_testpng::Picture;

/// A second, in nanoseconds: how long a serial is good for.
const SECOND: u64 = 1_000_000_000;

/// What the pretend clipboard holds.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Clip {
    Empty,
    Text(String),
    Picture(u32, u32, Vec<u32>),
}

/// A window of the pretend display server.
#[derive(Clone, Debug)]
pub struct FakeWindow {
    /// Its number.
    pub win: Win,
    /// What it is.
    pub kind: WindowKind,
    /// Its parent, for dialogs and popups.
    pub parent: Option<Win>,
    /// The app's part of its title.
    pub title: String,
    /// Logical pixels.
    pub size: (u32, u32),
    /// Where a popup opened in its parent.
    pub at: (i32, i32),
    /// Resizable, modal.
    pub flags: WindowFlags,
    /// The pointer shape the app chose.
    pub cursor: Cursor,
    /// How many times it was drawn, and the rectangles last said to have changed.
    pub draws: u32,
    /// The rectangles of the last draw.
    pub damage: Vec<Rect>,
    /// Its pixels, at its scale.
    pixels: Vec<u32>,
    factor: u32,
    configured: bool,
    focused: bool,
}

impl FakeWindow {
    fn device(&self) -> (u32, u32) {
        (self.size.0 * self.factor, self.size.1 * self.factor)
    }
}

/// The pretend display server.
struct FakeDisplay {
    now: u64,
    windows: Vec<FakeWindow>,
    next: u32,
    clipboard: Clip,
    timers: BTreeMap<u32, u64>,
    /// The serial of the last key or button press, and when it was.
    last_press: (u32, u64),
    next_serial: u32,
    /// The scale new windows get.
    factor: u32,
    /// Windows that have the keyboard in turn: the last is focused.
    focus_stack: Vec<Win>,
    /// Events to give the app at the next chance.
    pending: VecDeque<Incoming>,
    frames: Vec<Win>,
    moves: u32,
}

impl FakeDisplay {
    fn window(&mut self, win: Win) -> Option<&mut FakeWindow> {
        self.windows.iter_mut().find(|w| w.win == win)
    }

    fn serial_ok(&self, serial: u32) -> bool {
        serial != 0
            && serial == self.last_press.0
            && self.now.saturating_sub(self.last_press.1) <= SECOND
    }

    fn press_serial(&mut self) -> u32 {
        self.next_serial += 1;
        self.last_press = (self.next_serial, self.now);
        self.next_serial
    }
}

impl Platform for FakeDisplay {
    fn now(&self) -> u64 {
        self.now
    }

    fn create(&mut self, w: &NewWindow) -> Option<Win> {
        if w.kind == WindowKind::Popup && (w.parent.is_none() || !self.serial_ok(w.serial)) {
            return None;
        }
        if w.size.0 == 0 || w.size.1 == 0 {
            return None;
        }
        self.next += 1;
        let win = Win(self.next);
        self.windows.push(FakeWindow {
            win,
            kind: w.kind,
            parent: w.parent,
            title: w.title.clone(),
            size: w.size,
            at: w.at,
            flags: w.flags,
            cursor: Cursor::Arrow,
            draws: 0,
            damage: Vec::new(),
            pixels: Vec::new(),
            factor: self.factor,
            configured: false,
            focused: false,
        });
        if w.kind != WindowKind::Popup {
            self.focus_stack.push(win);
        }
        Some(win)
    }

    fn destroy(&mut self, win: Win) {
        self.windows.retain(|w| w.win != win);
        self.focus_stack.retain(|w| *w != win);
    }

    fn set_title(&mut self, win: Win, title: &str) {
        if let Some(w) = self.window(win) {
            w.title = title.to_string();
        }
    }

    fn set_limits(&mut self, _win: Win, _min: (u32, u32), _max: (u32, u32)) {}

    fn set_cursor(&mut self, win: Win, cursor: Cursor) {
        if let Some(w) = self.window(win) {
            w.cursor = cursor;
        }
    }

    fn draw(&mut self, win: Win, f: &mut dyn FnMut(&mut Canvas<'_>) -> Damage) -> bool {
        let Some(w) = self.window(win) else {
            return false;
        };
        if !w.configured {
            return false;
        }
        let (dw, dh) = w.device();
        let factor = w.factor;
        let Some(canvas) = Canvas::new(&mut w.pixels, dw, dh, dw) else {
            return false;
        };
        let mut canvas = canvas.with_scale(factor);
        let damage = f(&mut canvas);
        w.draws += 1;
        w.damage = damage.rects().to_vec();
        true
    }

    fn copy_text(&mut self, serial: u32, text: &str) -> bool {
        if !self.serial_ok(serial) {
            return false;
        }
        self.clipboard = Clip::Text(text.to_string());
        self.pending.push_back(Incoming::Clipboard { text: true, picture: false });
        true
    }

    fn paste_text(&mut self, serial: u32) -> Option<String> {
        match (&self.clipboard, self.serial_ok(serial)) {
            (Clip::Text(t), true) => Some(t.clone()),
            _ => None,
        }
    }

    fn copy_picture(&mut self, serial: u32, width: u32, height: u32, pixels: &[u32]) -> bool {
        let count = (width as usize).saturating_mul(height as usize);
        if !self.serial_ok(serial) || count == 0 || count > pixels.len() {
            return false;
        }
        self.clipboard = Clip::Picture(width, height, pixels[..count].to_vec());
        self.pending.push_back(Incoming::Clipboard { text: false, picture: true });
        true
    }

    fn paste_picture(&mut self, serial: u32) -> Option<(u32, u32, Vec<u32>)> {
        match (&self.clipboard, self.serial_ok(serial)) {
            (Clip::Picture(w, h, p), true) => Some((*w, *h, p.clone())),
            _ => None,
        }
    }

    fn set_timer(&mut self, id: u32, at: u64) {
        self.timers.insert(id, at);
    }

    fn cancel_timer(&mut self, id: u32) {
        self.timers.remove(&id);
    }

    fn start_move(&mut self, _win: Win, _serial: u32) {
        self.moves += 1;
    }

    fn start_resize(&mut self, _win: Win, _edge: Edge, _serial: u32) {}

    fn minimise(&mut self, _win: Win) {}

    fn maximise(&mut self, _win: Win, _on: bool) {}

    fn request_frame(&mut self, win: Win) {
        self.frames.push(win);
    }
}

/// The app, with every message it is sent written down.
struct Recorder<A: App> {
    app: A,
    log: Vec<A::Message>,
}

impl<A: App> App for Recorder<A> {
    type Message = A::Message;

    fn view(&self) -> Element<'_, A::Message> {
        self.app.view()
    }

    fn update(&mut self, message: A::Message, cx: &mut Context<'_, A::Message>) {
        self.log.push(message.clone());
        self.app.update(message, cx);
    }

    fn title(&self) -> String {
        self.app.title()
    }

    fn start(&mut self, cx: &mut Context<'_, A::Message>) {
        self.app.start(cx);
    }

    fn view_window(&self, window: WindowId) -> Option<Element<'_, A::Message>> {
        self.app.view_window(window)
    }

    fn window_title(&self, window: WindowId) -> String {
        self.app.window_title(window)
    }

    fn shortcuts(&self) -> Vec<(Shortcut, A::Message)> {
        self.app.shortcuts()
    }

    fn close_requested(&self, window: WindowId) -> Option<A::Message> {
        self.app.close_requested(window)
    }

    fn clipboard_changed(&self, text: bool, picture: bool) -> Option<A::Message> {
        self.app.clipboard_changed(text, picture)
    }

    fn notified(&self, bits: u64) -> Option<A::Message> {
        self.app.notified(bits)
    }
}

/// An app running against the pretend display server.
pub struct Harness<A: App>
where
    A::Message: Debug,
{
    rt: Runtime<Recorder<A>>,
    display: FakeDisplay,
    modifiers: Modifiers,
    /// Where the pointer is: the window and the point in it.
    pointer: Option<(Win, i32, i32)>,
    buttons: u32,
}

impl<A: App> Harness<A>
where
    A::Message: Debug,
{
    /// Runs `app` with a main window as `settings` say, at 1x.
    pub fn new(app: A, settings: Settings) -> Harness<A> {
        Harness::with_scale(app, settings, 1)
    }

    /// Runs `app` at a scale of `factor` (2 for a 2x screen).
    pub fn with_scale(app: A, settings: Settings, factor: u32) -> Harness<A> {
        let display = FakeDisplay {
            now: SECOND,
            windows: Vec::new(),
            next: 0,
            clipboard: Clip::Empty,
            timers: BTreeMap::new(),
            last_press: (0, 0),
            next_serial: 0,
            factor: factor.clamp(1, 8),
            focus_stack: Vec::new(),
            pending: VecDeque::new(),
            frames: Vec::new(),
            moves: 0,
        };
        let mut h = Harness {
            rt: Runtime::new(Recorder { app, log: Vec::new() }, settings),
            display,
            modifiers: Modifiers::NONE,
            pointer: None,
            buttons: 0,
        };
        assert!(h.rt.open(&mut h.display), "the main window wasn't made");
        h.sync();
        h
    }

    // ---- Looking at the app. ----

    /// The app.
    pub fn app(&self) -> &A {
        &self.rt.app().app
    }

    /// The app, to change its state directly (then [`Harness::refresh`]).
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.rt.app_mut().app
    }

    /// Every message the app has been sent, in order.
    pub fn messages(&self) -> &[A::Message] {
        &self.rt.app().log
    }

    /// The messages sent since the last call.
    pub fn take_messages(&mut self) -> Vec<A::Message> {
        std::mem::take(&mut self.rt.app_mut().log)
    }

    /// The app has ended.
    pub fn finished(&self) -> bool {
        self.rt.finished()
    }

    /// The app's windows, oldest first.
    pub fn windows(&self) -> Vec<WindowId> {
        self.rt.windows().into_iter().map(|(id, _)| id).collect()
    }

    /// The pretend display server's record of a window.
    pub fn window(&self, id: WindowId) -> Option<&FakeWindow> {
        let win = self.rt.display_window(id)?;
        self.display.windows.iter().find(|w| w.win == win)
    }

    /// The popup open over the main window (or over any window), if any.
    pub fn popup(&self) -> Option<WindowId> {
        self.windows().into_iter().find(|id| self.rt.is_popup(*id))
    }

    /// The newest dialog, if any.
    pub fn dialog(&self) -> Option<WindowId> {
        self.windows()
            .into_iter()
            .rev()
            .find(|id| self.window(*id).is_some_and(|w| w.kind == WindowKind::Dialog))
    }

    /// Every widget of a window, described.
    pub fn describe(&self, id: WindowId) -> Vec<Description> {
        self.rt.describe(id)
    }

    /// The main window's widgets as lines of text (what the gallery logs).
    pub fn describe_lines(&self, id: WindowId) -> Vec<String> {
        self.describe(id).iter().map(Description::line).collect()
    }

    /// Finds a widget by its label or its id, looking in the newest window first (a popup, then
    /// a dialog, then the main window).
    pub fn find(&self, text: &str) -> Option<(WindowId, Description)> {
        for id in self.windows().into_iter().rev() {
            let all = self.describe(id);
            if let Some(d) = all.iter().find(|d| d.key.as_deref() == Some(text)) {
                return Some((id, d.clone()));
            }
            if let Some(d) = all.iter().find(|d| d.what.label == text && d.what.role != "label") {
                return Some((id, d.clone()));
            }
            if let Some(d) = all.iter().find(|d| d.what.label == text) {
                return Some((id, d.clone()));
            }
        }
        None
    }

    /// The widget with the focus in the window that has the keyboard.
    pub fn focused(&self) -> Option<Description> {
        let win = *self.display.focus_stack.last()?;
        let id = self.rt.windows().into_iter().find(|(_, w)| *w == win)?.0;
        self.describe(id).into_iter().find(|d| d.focused)
    }

    /// The clipboard's text, if it has text.
    pub fn clipboard_text(&self) -> Option<String> {
        match &self.display.clipboard {
            Clip::Text(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// The clipboard's picture, if it has one.
    pub fn clipboard_picture(&self) -> Option<(u32, u32, Vec<u32>)> {
        match &self.display.clipboard {
            Clip::Picture(w, h, p) => Some((*w, *h, p.clone())),
            _ => None,
        }
    }

    /// Another app copies text: the clipboard changes, and this app is told.
    pub fn set_clipboard_text(&mut self, text: &str) {
        self.display.clipboard = Clip::Text(text.to_string());
        self.send(Incoming::Clipboard { text: true, picture: false });
    }

    /// How many times `start_move` was asked for.
    pub fn moves_started(&self) -> u32 {
        self.display.moves
    }

    /// The monotonic clock, in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.display.now / 1_000_000
    }

    // ---- Pictures. ----

    /// What a window shows, as a picture at its scale.
    pub fn picture(&self, id: WindowId) -> Picture {
        let w = self.window(id).expect("no such window");
        let (width, height) = w.device();
        Picture { width, height, pixels: w.pixels.iter().map(|p| p | 0xFF00_0000).collect() }
    }

    /// Compares the main window with `tests/pictures/sandstone/NAME.png` (written afresh with
    /// `PBR_UPDATE_PICTURES=1`), failing the test if they differ.
    pub fn check_picture(&self, name: &str) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/sandstone");
        self.check_window_picture_in(WindowId::MAIN, &dir, name);
    }

    /// Compares a window with the picture `dir/NAME.png`.
    pub fn check_window_picture_in(&self, id: WindowId, dir: &Path, name: &str) {
        let file = dir.join(format!("{name}.png"));
        if let Err(why) = pbr_testpng::check(&file, &self.picture(id)) {
            panic!("{name}: {why}");
        }
    }

    // ---- Doing things. ----

    /// Gives the app an event, then lets everything settle.
    pub fn send(&mut self, event: Incoming) {
        self.rt.handle(&mut self.display, event);
        self.sync();
    }

    /// Builds and draws again after the app's state was changed directly.
    pub fn refresh(&mut self) {
        self.rt.app_mut();
        self.send(Incoming::Lost(0));
    }

    /// Time passes: timers that come meanwhile go off, in order.
    pub fn advance(&mut self, ms: u64) {
        let end = self.display.now.saturating_add(ms.saturating_mul(1_000_000));
        loop {
            let due = self
                .display
                .timers
                .iter()
                .filter(|(_, at)| **at <= end)
                .min_by_key(|(_, at)| **at)
                .map(|(id, at)| (*id, *at));
            let Some((id, at)) = due else {
                break;
            };
            self.display.now = self.display.now.max(at);
            self.display.timers.remove(&id);
            self.send(Incoming::Timer(id));
        }
        self.display.now = end;
    }

    /// Changes the main window's size, as the user resizing it.
    pub fn resize(&mut self, width: u32, height: u32) {
        let win = self.rt.display_window(WindowId::MAIN).expect("a main window");
        if let Some(w) = self.display.window(win) {
            w.size = (width, height);
        }
        self.reconfigure(win);
    }

    /// Moves every window to a screen of scale `factor`.
    pub fn set_scale(&mut self, factor: u32) {
        self.display.factor = factor.clamp(1, 8);
        let wins: Vec<Win> = self.display.windows.iter().map(|w| w.win).collect();
        for win in wins {
            if let Some(w) = self.display.window(win) {
                w.factor = factor.clamp(1, 8);
            }
            self.reconfigure(win);
        }
    }

    /// The user asks to close a window (its close button).
    pub fn close(&mut self, id: WindowId) {
        let win = self.rt.display_window(id).expect("no such window");
        self.send(Incoming::CloseRequested { window: win });
    }

    /// Presses and lets go a named key, with `modifiers` held.
    pub fn key_with(&mut self, key: Key, modifiers: Modifiers) {
        // Of the named keys only Space types a character.
        let ch = (key == Key::Space).then_some(' ');
        self.key_raw(key_code(key), ch, modifiers);
    }

    /// Presses and lets go a named key.
    pub fn key(&mut self, key: Key) {
        self.key_with(key, Modifiers::NONE);
    }

    /// Types text: each character pressed and let go (Shift held for capitals, as a keyboard
    /// would). `\n` is Enter and `\t` is Tab.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => self.key(Key::Enter),
                '\t' => self.key(Key::Tab),
                _ => {
                    let m = if c.is_uppercase() {
                        Modifiers(Modifiers::SHIFT)
                    } else {
                        Modifiers::NONE
                    };
                    self.key_raw(char_code(c), Some(c), m);
                }
            }
        }
    }

    /// Presses a shortcut (Ctrl+S).
    pub fn shortcut(&mut self, s: Shortcut) {
        match s.trigger {
            Trigger::Char(c) => self.key_raw(char_code(c), Some(c), s.modifiers),
            Trigger::Key(k) => self.key_with(k, s.modifiers),
        }
    }

    /// A key press and release with exactly these values, to the window with the keyboard.
    pub fn key_raw(&mut self, code: u32, ch: Option<char>, modifiers: Modifiers) {
        let Some(win) = self.display.focus_stack.last().copied() else {
            return;
        };
        self.modifiers = modifiers;
        let serial = self.display.press_serial();
        let mut ev = KeyEvent {
            window: win,
            code: code as u16,
            modifiers: modifiers.0,
            value: input::VALUE_PRESSED,
            ch,
            typed: pbr_window::is_typed(input::VALUE_PRESSED, ch, modifiers.0),
            serial,
        };
        self.send(Incoming::Key(ev));
        ev.value = input::VALUE_RELEASED;
        ev.typed = false;
        ev.serial = 0;
        ev.window = self.display.focus_stack.last().copied().unwrap_or(win);
        self.send(Incoming::Key(ev));
        self.modifiers = Modifiers::NONE;
    }

    /// Moves the pointer to (`x`, `y`) in a window.
    pub fn move_in(&mut self, id: WindowId, x: i32, y: i32) {
        let win = self.rt.display_window(id).expect("no such window");
        if let Some((old, _, _)) = self.pointer
            && old != win
            && self.display.windows.iter().any(|w| w.win == old)
        {
            self.send_pointer(old, PointerKind::Left, 0, 0, 0);
        }
        self.pointer = Some((win, x, y));
        self.send_pointer(win, PointerKind::Moved, x, y, 0);
    }

    /// Moves the pointer in the main window.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.move_in(WindowId::MAIN, x, y);
    }

    /// Presses a button (`pbr.input`'s number) where the pointer is.
    pub fn press(&mut self, button: u16) {
        let Some((win, x, y)) = self.pointer else {
            return;
        };
        // A press outside an open popup closes it first, as the display server does.
        let popups: Vec<Win> = self
            .display
            .windows
            .iter()
            .filter(|w| w.kind == WindowKind::Popup && w.win != win)
            .map(|w| w.win)
            .collect();
        for p in popups {
            self.send(Incoming::CloseRequested { window: p });
        }
        if !self.display.windows.iter().any(|w| w.win == win) {
            return;
        }
        self.buttons |= 1 << button;
        let serial = self.display.press_serial();
        self.send_pointer(win, PointerKind::Button { button, pressed: true }, x, y, serial);
    }

    /// Lets go of a button.
    pub fn release(&mut self, button: u16) {
        let Some((win, x, y)) = self.pointer else {
            return;
        };
        self.buttons &= !(1 << button);
        if self.display.windows.iter().any(|w| w.win == win) {
            self.send_pointer(win, PointerKind::Button { button, pressed: false }, x, y, 0);
        }
    }

    /// Clicks the left button at (`x`, `y`) in a window.
    pub fn click_in(&mut self, id: WindowId, x: i32, y: i32) {
        self.move_in(id, x, y);
        self.press(input::BUTTON_LEFT as u16);
        self.release(input::BUTTON_LEFT as u16);
    }

    /// Clicks the left button at (`x`, `y`) in the main window.
    pub fn click(&mut self, x: i32, y: i32) {
        self.click_in(WindowId::MAIN, x, y);
    }

    /// Two quick clicks.
    pub fn double_click(&mut self, x: i32, y: i32) {
        self.click(x, y);
        self.advance(50);
        self.click(x, y);
    }

    /// Clicks the right button.
    pub fn right_click_in(&mut self, id: WindowId, x: i32, y: i32) {
        self.move_in(id, x, y);
        self.press(input::BUTTON_RIGHT as u16);
        self.release(input::BUTTON_RIGHT as u16);
    }

    /// Drags with the left button from one point of the main window to another.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.move_to(from.0, from.1);
        self.press(input::BUTTON_LEFT as u16);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release(input::BUTTON_LEFT as u16);
    }

    /// Turns the wheel `steps` towards the user (down) at (`x`, `y`) in the main window;
    /// negative turns it up.
    pub fn scroll(&mut self, x: i32, y: i32, steps: i32) {
        self.move_to(x, y);
        let win = self.rt.display_window(WindowId::MAIN).expect("a main window");
        // The wheel's up is positive on the wire.
        self.send_pointer(
            win,
            PointerKind::Scroll { axis: input::AXIS_WHEEL as u16, steps: -steps },
            x,
            y,
            0,
        );
    }

    /// Clicks the middle of the widget with this label or id (see [`Harness::find`]),
    /// failing the test if there is none.
    pub fn click_on(&mut self, text: &str) {
        let (id, d) = self.find(text).unwrap_or_else(|| panic!("nothing labelled {text:?}"));
        let r = d.rect;
        self.click_in(id, r.x + r.width / 2, r.y + r.height / 2);
    }

    // ---- Underneath. ----

    fn send_pointer(&mut self, win: Win, what: PointerKind, x: i32, y: i32, serial: u32) {
        let ev = PointerEvent {
            window: win,
            what,
            x,
            y,
            raw_x: x * 256,
            raw_y: y * 256,
            buttons: self.buttons,
            serial,
        };
        self.send(Incoming::Pointer(ev));
    }

    fn reconfigure(&mut self, win: Win) {
        let Some(w) = self.display.windows.iter().find(|w| w.win == win) else {
            return;
        };
        let ev = Incoming::Configure {
            window: win,
            size: w.size,
            scale: w.factor * 100,
            focused: w.focused,
            maximised: false,
            minimised: false,
            shown: true,
            serial: 0,
        };
        let (dw, dh) = w.device();
        if let Some(w) = self.display.window(win) {
            w.pixels = vec![0; (dw * dh) as usize];
            w.configured = true;
        }
        self.rt.handle(&mut self.display, ev);
    }

    /// Configures new windows, tells windows whose focus changed, gives the app what is
    /// waiting, and checks the drawing. Repeats until nothing changes.
    fn sync(&mut self) {
        for _ in 0..16 {
            let mut changed = false;
            if let Some(e) = self.display.pending.pop_front() {
                self.rt.handle(&mut self.display, e);
                changed = true;
            }
            let top = self.display.focus_stack.last().copied();
            let todo: Vec<(Win, bool, bool)> = self
                .display
                .windows
                .iter()
                .map(|w| (w.win, w.configured, Some(w.win) == top))
                .filter(|(win, configured, focused)| {
                    !configured
                        || self
                            .display
                            .windows
                            .iter()
                            .any(|w| w.win == *win && w.focused != *focused)
                })
                .collect();
            for (win, _, focused) in todo {
                if let Some(w) = self.display.window(win) {
                    w.focused = focused;
                }
                self.reconfigure(win);
                changed = true;
            }
            if !changed {
                break;
            }
        }
        self.verify();
    }

    /// Checks that every window shows what drawing it whole would.
    fn verify(&self) {
        for (id, win) in self.rt.windows() {
            let Some(w) = self.display.windows.iter().find(|w| w.win == win) else {
                continue;
            };
            if !w.configured || w.draws == 0 {
                continue;
            }
            let (dw, dh) = w.device();
            let mut whole = vec![0u32; (dw * dh) as usize];
            let Some(canvas) = Canvas::new(&mut whole, dw, dh, dw) else {
                continue;
            };
            let mut canvas = canvas.with_scale(w.factor);
            if !self.rt.draw_whole(id, &mut canvas) {
                continue;
            }
            if let Some(at) = whole
                .iter()
                .zip(&w.pixels)
                .position(|(a, b)| (a | 0xFF00_0000) != (b | 0xFF00_0000))
            {
                let (x, y) = (at as u32 % dw, at as u32 / dw);
                panic!(
                    "window {:?}: drawing only what changed left device pixel ({x}, {y}) as {:#08x}, \
                     but drawing it whole gives {:#08x}: a widget's `look` misses something its \
                     pixels depend on, or it drew outside its rectangle",
                    id,
                    w.pixels[at] & 0xFF_FFFF,
                    whole[at] & 0xFF_FFFF
                );
            }
        }
    }
}

/// `pbr.input`'s code for a named key.
fn key_code(key: Key) -> u32 {
    match key {
        Key::Enter => input::KEY_ENTER,
        Key::Escape => input::KEY_ESCAPE,
        Key::Backspace => input::KEY_BACKSPACE,
        Key::Tab => input::KEY_TAB,
        Key::Space => input::KEY_SPACE,
        Key::Delete => input::KEY_DELETE,
        Key::Insert => input::KEY_INSERT,
        Key::Home => input::KEY_HOME,
        Key::End => input::KEY_END,
        Key::PageUp => input::KEY_PAGE_UP,
        Key::PageDown => input::KEY_PAGE_DOWN,
        Key::Left => input::KEY_LEFT,
        Key::Right => input::KEY_RIGHT,
        Key::Up => input::KEY_UP,
        Key::Down => input::KEY_DOWN,
        Key::Function(n) => input::KEY_F1 + u32::from(n.clamp(1, 12)) - 1,
        Key::Modifier => input::KEY_LEFT_SHIFT,
        Key::Character => input::KEY_GRAVE,
    }
}

/// `pbr.input`'s code for the key that types `c` on a US keyboard (any other character comes
/// from the grave key, which is enough for a test).
fn char_code(c: char) -> u32 {
    let l = c.to_ascii_lowercase();
    match l {
        'a'..='z' => input::KEY_A + (l as u32 - 'a' as u32),
        '1'..='9' => input::KEY_1 + (l as u32 - '1' as u32),
        '0' => input::KEY_0,
        ' ' => input::KEY_SPACE,
        '-' => input::KEY_MINUS,
        '=' => input::KEY_EQUALS,
        ',' => input::KEY_COMMA,
        '.' => input::KEY_PERIOD,
        '/' => input::KEY_SLASH,
        _ => input::KEY_GRAVE,
    }
}
