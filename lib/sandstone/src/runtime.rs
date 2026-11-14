// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The part of sandstone between the display server and the app (note 0023, section 5): it
//! keeps the app's windows, builds their views, keeps the widgets' state between views, routes
//! keys and the pointer, moves the focus, opens popups and dialogs, and draws what changed.
//!
//! It is written against the [`Platform`] trait, so the same code runs the app in
//! PrettyBigRock (`run`, with `lib/window`'s connection) and in the test harness (with a
//! pretend display server).
//!
//! **One event, start to finish:** the event is turned into sandstone's own ([`Event`]) and
//! given to the widgets of its window (built afresh from the app's state, with their states
//! from the [`Tree`]); the messages they send go to the app's `update`; then every window whose
//! look may have changed is built again, laid out, and each drawing widget's look compared
//! with what it was ([`Widget::looks`](crate::Widget::looks)); the rectangles that changed are
//! drawn again, and only those are sent to the display server.

use crate::app::{App, Asked, Context, Settings, WindowId, WindowSpec};
use crate::dialog::{self, Answer, Dialog};
use crate::element::{Description, DrawCx, Element, LayoutCx, Tree};
use crate::event::{Button, Event, Key, KeyPress, Modifiers, Pointer, PointerKind, Role, Status};
use crate::geometry::{Limits, Point, Rect, Size, UNBOUNDED};
use crate::layout::Node;
use crate::platform::{NewWindow, Platform};
use crate::shell::{Asks, PopupAsk, Shell};
use crate::theme::Theme;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_draw::Damage;
use pbr_interfaces::pbr_input as input;
use pbr_window::{Cursor, Event as Incoming, KeyEvent, PointerEvent, Win, WindowFlags, WindowKind};

/// Two presses closer together than this (and in nearly the same place) are a double click.
const DOUBLE_CLICK_NS: u64 = 500_000_000;
/// How far apart, in logical pixels each way, two presses of a double click may be.
const DOUBLE_CLICK_SLOP: i32 = 4;
/// How long after a press its serial is still used for what follows (the display server
/// accepts a serial for a second; this leaves room for the time the app takes).
const PRESS_GOOD_NS: u64 = 900_000_000;
/// The widest a dialog is before its text wraps (its text wraps itself narrower).
const DIALOG_MAX: Size = Size::new(480, 600);

/// What the widgets of every window send: the app's messages, and sandstone's own dialogs'.
pub(crate) enum Msg<M> {
    App(M),
    Dialog(Answer),
}

/// What a window is for.
enum Purpose<M> {
    /// The app's main window.
    Main,
    /// Another window the app opened ([`Context::open_window`]).
    Other,
    /// A dialog of sandstone's ([`Context::show`]).
    Dialog(Dialog<M>),
    /// A popup opened by the widget whose state number is `owner`, in window `parent`.
    Popup { parent: WindowId, owner: u64 },
}

/// What a window must do before it is up to date.
#[derive(Clone, Copy, Debug, Default)]
struct Needs {
    /// Something changed: build, lay out and compare looks.
    update: bool,
    /// Draw all of it (new size, new scale, a lost event, the focus).
    all: bool,
}

/// One of the app's windows.
struct Window<M> {
    id: WindowId,
    win: Win,
    role: Purpose<M>,
    /// Logical pixels, and the canvas's whole-number scale, from the last configure.
    size: Size,
    factor: u32,
    focused: bool,
    configured: bool,
    tree: Option<Tree>,
    node: Node,
    /// Each drawing widget's rectangle and look, as last drawn.
    looks: BTreeMap<u64, (Rect, u64)>,
    needs: Needs,
    /// The widget with the focus, and an id the app asked to focus (resolved when built).
    focus: Option<u64>,
    focus_key: Option<String>,
    /// The title last set.
    title: String,
    cursor: Cursor,
    /// The last press: when, where, which button, and its click count.
    last_press: Option<(u64, Point, Button, u32)>,
    /// Its open popup.
    popup: Option<WindowId>,
    /// Its size or scale changed: once laid out again, its widgets hear `Event::Resized`.
    resized: bool,
}

impl<M> Window<M> {
    fn new(id: WindowId, win: Win, role: Purpose<M>) -> Window<M> {
        Window {
            id,
            win,
            role,
            size: Size::ZERO,
            factor: 1,
            focused: false,
            configured: false,
            tree: None,
            node: Node::default(),
            looks: BTreeMap::new(),
            needs: Needs { update: true, all: true },
            focus: None,
            focus_key: None,
            title: String::new(),
            cursor: Cursor::Arrow,
            last_press: None,
            popup: None,
            resized: false,
        }
    }
}

/// An app and its windows: everything between the display server and the app.
pub struct Runtime<A: App> {
    app: A,
    theme: Theme,
    settings: Settings,
    windows: Vec<Window<A::Message>>,
    asked: Asked<A::Message>,
    timers: BTreeMap<u32, A::Message>,
    next_timer: u32,
    clipboard: (bool, bool),
    modifiers: Modifiers,
    /// The serial of the last key or button press, and when it came: a button's message comes
    /// when it is let go, and copying or opening a popup then still needs the press's serial.
    last_press: (u32, u64),
    started: bool,
    quit: bool,
}

/// Builds window `i`'s view and calls `f` with it: the app's view for its windows, sandstone's
/// for a dialog, and for a popup the contents its owner gives (found in its parent's view).
/// The view borrows the app, so it can't outlive this call; hence `f`, rather than returning
/// it. `None` if the window has nothing to show (its owner has gone).
fn with_root<A: App, R>(
    app: &A,
    windows: &[Window<A::Message>],
    i: usize,
    theme: &Theme,
    f: impl FnOnce(&Element<'_, Msg<A::Message>>) -> R,
) -> Option<R> {
    match &windows[i].role {
        Purpose::Popup { parent, owner } => {
            let j = windows.iter().position(|w| w.id == *parent)?;
            let parent_tree = windows[j].tree.as_ref()?;
            with_plain_root(app, &windows[j], theme, |root| {
                let popup = root.widget.find_popup(parent_tree, *owner)?;
                Some(f(&popup))
            })?
        }
        _ => with_plain_root(app, &windows[i], theme, f),
    }
}

/// [`with_root`] for a window that isn't a popup.
fn with_plain_root<A: App, R>(
    app: &A,
    w: &Window<A::Message>,
    theme: &Theme,
    f: impl FnOnce(&Element<'_, Msg<A::Message>>) -> R,
) -> Option<R> {
    let root: Element<'_, Msg<A::Message>> = match &w.role {
        Purpose::Main => app.view().map(Msg::App),
        Purpose::Other => app.view_window(w.id)?.map(Msg::App),
        Purpose::Dialog(d) => dialog::view(d, theme).map(Msg::Dialog),
        Purpose::Popup { .. } => return None,
    };
    Some(f(&root))
}

/// The state tree for `root`: `old`, brought up to date, if it was for the same kind of
/// widget; otherwise a new one.
fn diffed<M>(root: &Element<'_, M>, old: Option<Tree>) -> Tree {
    match old {
        Some(mut t) if t.is_for(root) => {
            root.widget.diff(&mut t);
            t
        }
        _ => root.tree(),
    }
}

/// Lays out `root` within `limits` and makes every rectangle relative to the window.
fn laid_out<M>(root: &Element<'_, M>, tree: &mut Tree, limits: Limits, theme: &Theme) -> Node {
    let mut node = root.widget.layout(tree, limits, &LayoutCx { theme });
    node.move_to(0, 0);
    node.place(0, 0);
    node
}

impl<A: App> Runtime<A> {
    /// An app, not yet shown.
    pub fn new(app: A, settings: Settings) -> Runtime<A> {
        Runtime {
            app,
            theme: Theme::STANDARD,
            settings,
            windows: Vec::new(),
            asked: Asked::new(),
            timers: BTreeMap::new(),
            next_timer: 1,
            clipboard: (false, false),
            modifiers: Modifiers::NONE,
            last_press: (0, 0),
            started: false,
            quit: false,
        }
    }

    /// The app.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// The app, to change it (tests).
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// The look in use.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The app has ended (it quit, its main window closed, or the display server went).
    pub fn finished(&self) -> bool {
        self.quit
    }

    /// Opens the main window. False if the display server refused.
    pub fn open(&mut self, p: &mut dyn Platform) -> bool {
        let s = &self.settings;
        let mut flags = WindowFlags(0);
        if s.resizable {
            flags = flags | WindowFlags::RESIZABLE;
        }
        let title = self.app.title();
        let new = NewWindow {
            kind: WindowKind::Normal,
            parent: None,
            title: title.clone(),
            size: s.size,
            flags,
            at: (0, 0),
            serial: 0,
        };
        let Some(win) = p.create(&new) else {
            return false;
        };
        if s.min_size != (0, 0) {
            p.set_limits(win, s.min_size, (0, 0));
        }
        let mut w = Window::new(WindowId::MAIN, win, Purpose::Main);
        w.title = title;
        self.windows.push(w);
        true
    }

    /// The display server's number for one of the app's windows.
    pub fn display_window(&self, id: WindowId) -> Option<Win> {
        self.windows.iter().find(|w| w.id == id).map(|w| w.win)
    }

    /// The app's windows, as (sandstone's number, the display server's), oldest first.
    pub fn windows(&self) -> Vec<(WindowId, Win)> {
        self.windows.iter().map(|w| (w.id, w.win)).collect()
    }

    /// The popup open over a window, if any.
    pub fn popup_of(&self, id: WindowId) -> Option<WindowId> {
        self.windows.iter().find(|w| w.id == id).and_then(|w| w.popup)
    }

    /// Every widget of a window, described, in tree order (for tests and the gallery's log).
    pub fn describe(&self, id: WindowId) -> Vec<Description> {
        let Some(i) = self.index(id) else {
            return Vec::new();
        };
        let w = &self.windows[i];
        let Some(tree) = w.tree.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        with_root(&self.app, &self.windows, i, &self.theme, |root| {
            root.widget.describe_all(tree, &w.node, w.focus, &mut out);
        });
        out
    }

    /// Draws all of a window afresh on `canvas`, from the state it was last drawn with: what
    /// the window should show, for the harness to compare with what drawing only the changes
    /// left in it. False if the window hasn't been drawn yet.
    pub fn draw_whole(&self, id: WindowId, canvas: &mut pbr_draw::Canvas<'_>) -> bool {
        let Some(i) = self.index(id) else {
            return false;
        };
        let w = &self.windows[i];
        let Some(tree) = w.tree.as_ref() else {
            return false;
        };
        let whole = Rect::new(0, 0, w.size.width, w.size.height);
        let window_focused = w.focused || matches!(w.role, Purpose::Popup { .. });
        let theme = self.theme;
        with_root(&self.app, &self.windows, i, &theme, |root| {
            canvas.fill(theme.background);
            let cx = DrawCx { theme: &theme, focus: w.focus, window_focused, clip: whole };
            root.widget.draw(tree, &w.node, canvas, &cx);
        })
        .is_some()
    }

    /// Whether a window is a popup, a dialog of sandstone's, or the app's.
    pub fn is_popup(&self, id: WindowId) -> bool {
        self.index(id).is_some_and(|i| matches!(self.windows[i].role, Purpose::Popup { .. }))
    }

    fn index(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    fn index_of_win(&self, win: Win) -> Option<usize> {
        self.windows.iter().position(|w| w.win == win)
    }

    // ---- Events. ----

    /// Handles one event from the display server, then brings every window up to date.
    /// Answers false once the app has ended.
    pub fn handle(&mut self, p: &mut dyn Platform, event: Incoming) -> bool {
        match event {
            Incoming::Configure { window, size, scale, focused, .. } => {
                self.configure(p, window, size, scale, focused);
            }
            Incoming::CloseRequested { window } => self.close_requested(p, window),
            Incoming::Key(k) => self.key(p, k),
            Incoming::Pointer(e) => self.pointer(p, e),
            Incoming::Clipboard { text, picture } => {
                self.clipboard = (text, picture);
                if let Some(m) = self.app.clipboard_changed(text, picture) {
                    self.update(p, m, 0, WindowId::MAIN);
                }
            }
            Incoming::Lost(_) => {
                for w in &mut self.windows {
                    w.needs = Needs { update: true, all: true };
                }
            }
            Incoming::Timer(id) => {
                if let Some(m) = self.timers.remove(&id) {
                    self.update(p, m, 0, WindowId::MAIN);
                }
            }
            Incoming::Notified(bits) => {
                if let Some(m) = self.app.notified(bits) {
                    self.update(p, m, 0, WindowId::MAIN);
                }
            }
            Incoming::Disconnected => self.quit = true,
            _ => {}
        }
        self.settle(p);
        !self.quit
    }

    fn configure(
        &mut self,
        p: &mut dyn Platform,
        window: Win,
        size: (u32, u32),
        scale: u32,
        focused: bool,
    ) {
        let Some(i) = self.index_of_win(window) else {
            return;
        };
        let w = &mut self.windows[i];
        let size =
            Size::new(size.0.min(UNBOUNDED as u32) as i32, size.1.min(UNBOUNDED as u32) as i32);
        let factor = pbr_window::frames::pixel_scale(scale);
        if !w.configured || size != w.size || factor != w.factor {
            w.needs = Needs { update: true, all: true };
            w.resized = true;
        }
        w.size = size;
        w.factor = factor;
        w.configured = true;
        if focused != w.focused {
            w.focused = focused;
            w.needs = Needs { update: true, all: true };
            self.dispatch(p, i, &Event::WindowFocus(focused), 0);
        }
        if !self.started && self.windows[i].id == WindowId::MAIN {
            self.started = true;
            let win = Some(self.windows[i].win);
            let mut cx = Context {
                asked: &mut self.asked,
                timers: &mut self.timers,
                next_timer: &mut self.next_timer,
                platform: p,
                serial: 0,
                window: WindowId::MAIN,
                win,
                clipboard: self.clipboard,
            };
            self.app.start(&mut cx);
            self.after_update();
        }
    }

    fn close_requested(&mut self, p: &mut dyn Platform, window: Win) {
        let Some(i) = self.index_of_win(window) else {
            return;
        };
        let id = self.windows[i].id;
        match &self.windows[i].role {
            Purpose::Main => match self.app.close_requested(id) {
                Some(m) => self.update(p, m, 0, id),
                None => self.quit = true,
            },
            Purpose::Other => match self.app.close_requested(id) {
                Some(m) => self.update(p, m, 0, id),
                None => self.close(p, id),
            },
            Purpose::Dialog(d) => match d.cancel() {
                Some(n) => self.answer(p, i, Answer::Button(n), 0),
                None => self.close(p, id),
            },
            Purpose::Popup { .. } => self.close(p, id),
        }
    }

    fn key(&mut self, p: &mut dyn Platform, k: KeyEvent) {
        self.modifiers = Modifiers(k.modifiers);
        let Some(i) = self.index_of_win(k.window) else {
            return;
        };
        let press = KeyPress {
            key: Key::from_code(k.code),
            code: k.code,
            ch: k.ch,
            typed: k.typed,
            modifiers: Modifiers(k.modifiers),
            repeat: k.value == input::VALUE_REPEAT,
        };
        match k.value {
            input::VALUE_RELEASED => {
                self.dispatch(p, i, &Event::KeyReleased(press), 0);
                return;
            }
            // Pressed, repeating, or a key's second character (note 0022, section 10). A key
            // still held after a reset (3) is not a press.
            input::VALUE_PRESSED
            | input::VALUE_REPEAT
            | pbr_interfaces::pbr_window::VALUE_SECOND => {}
            _ => return,
        }
        let serial = k.serial;

        // An open popup takes every key; Escape closes it if it doesn't use it.
        if let Some(pi) = self.windows[i].popup.and_then(|id| self.index(id)) {
            let used = self.dispatch(p, pi, &Event::Key(press), serial);
            if used == Status::Ignored && press.key == Key::Escape {
                let id = self.windows[pi].id;
                self.close(p, id);
            }
            return;
        }
        if self.dispatch(p, i, &Event::Key(press), serial) == Status::Captured {
            return;
        }
        if self.shortcut(p, i, &press, serial) {
            return;
        }
        match press.key {
            Key::Tab if !press.modifiers.ctrl() && !press.modifiers.alt() => {
                self.move_focus(i, if press.modifiers.shift() { -1 } else { 1 });
            }
            Key::Enter => {
                self.dispatch(p, i, &Event::Activate(Role::Default), serial);
            }
            Key::Escape => {
                if self.dispatch(p, i, &Event::Activate(Role::Cancel), serial) == Status::Ignored {
                    // A dialog with no cancel button closes on Escape as on its close button.
                    if matches!(self.windows.get(i).map(|w| &w.role), Some(Purpose::Dialog(_))) {
                        let win = self.windows[i].win;
                        self.close_requested(p, win);
                    }
                }
            }
            Key::Function(10) if press.modifiers.for_shortcuts() == Modifiers::NONE => {
                self.dispatch(p, i, &Event::MenuKey, serial);
            }
            _ => {}
        }
    }

    /// Looks for a shortcut matching `press`: the menus' (in window `i`), then the app's (in
    /// its main window). True if one matched.
    fn shortcut(&mut self, p: &mut dyn Platform, i: usize, press: &KeyPress, serial: u32) -> bool {
        let mut found: Option<Msg<A::Message>> = None;
        with_root(&self.app, &self.windows, i, &self.theme, |root| {
            let mut all = Vec::new();
            root.widget.all_shortcuts(&mut all);
            found = all.into_iter().find(|(s, _)| s.matches(press)).map(|(_, m)| m);
        });
        if found.is_none() && self.windows[i].id == WindowId::MAIN {
            found = self
                .app
                .shortcuts()
                .into_iter()
                .find(|(s, _)| s.matches(press))
                .map(|(_, m)| Msg::App(m));
        }
        match found {
            Some(m) => {
                self.deliver(p, i, m, serial);
                true
            }
            None => false,
        }
    }

    /// Moves window `i`'s focus to the next (`step` 1) or previous (-1) focusable widget.
    fn move_focus(&mut self, i: usize, step: i32) {
        let w = &self.windows[i];
        let Some(tree) = w.tree.as_ref() else {
            return;
        };
        let mut order = Vec::new();
        with_root(&self.app, &self.windows, i, &self.theme, |root| {
            root.widget.focus_order(tree, &mut order)
        });
        if order.is_empty() {
            return;
        }
        let n = order.len() as i32;
        let next = match w.focus.and_then(|f| order.iter().position(|&id| id == f)) {
            Some(at) => (at as i32 + step).rem_euclid(n),
            None if step > 0 => 0,
            None => n - 1,
        };
        let w = &mut self.windows[i];
        w.focus = Some(order[next as usize]);
        w.needs.update = true;
    }

    fn pointer(&mut self, p: &mut dyn Platform, e: PointerEvent) {
        let Some(i) = self.index_of_win(e.window) else {
            return;
        };
        let at = Point::new(e.x, e.y);
        let kind = match e.what {
            pbr_window::PointerKind::Moved => PointerKind::Moved,
            pbr_window::PointerKind::Left => PointerKind::Left,
            pbr_window::PointerKind::Button { button, pressed: false } => {
                PointerKind::Released { button: Button::from_code(button) }
            }
            pbr_window::PointerKind::Button { button, pressed: true } => {
                let button = Button::from_code(button);
                let now = p.now();
                let w = &mut self.windows[i];
                let clicks = match w.last_press {
                    Some((t, last, b, n))
                        if b == button
                            && now.saturating_sub(t) <= DOUBLE_CLICK_NS
                            && (last.x - at.x).abs() <= DOUBLE_CLICK_SLOP
                            && (last.y - at.y).abs() <= DOUBLE_CLICK_SLOP =>
                    {
                        n + 1
                    }
                    _ => 1,
                };
                w.last_press = Some((now, at, button, clicks));
                PointerKind::Pressed { button, clicks }
            }
            pbr_window::PointerKind::Scroll { axis, steps } => match u32::from(axis) {
                // The wheel's up is positive; sandstone's `dy` counts down.
                input::AXIS_WHEEL => PointerKind::Scrolled { dx: 0, dy: steps.saturating_neg() },
                input::AXIS_HWHEEL => PointerKind::Scrolled { dx: steps, dy: 0 },
                _ => return,
            },
        };
        let event =
            Event::Pointer(Pointer { kind, at, buttons: e.buttons, modifiers: self.modifiers });
        self.dispatch(p, i, &event, e.serial);
        if matches!(kind, PointerKind::Moved | PointerKind::Released { .. }) {
            self.update_cursor(p, i, at);
        }
    }

    fn update_cursor(&mut self, p: &mut dyn Platform, i: usize, at: Point) {
        let Some(w) = self.windows.get(i) else {
            return;
        };
        let Some(tree) = w.tree.as_ref() else {
            return;
        };
        let cursor = with_root(&self.app, &self.windows, i, &self.theme, |root| {
            root.widget.cursor_at(tree, &w.node, at)
        });
        if let Some(c) = cursor
            && c != self.windows[i].cursor
        {
            self.windows[i].cursor = c;
            p.set_cursor(self.windows[i].win, c);
        }
    }

    /// Gives `event` to window `i`'s widgets, then acts on what they asked for and gives their
    /// messages to the app.
    fn dispatch(&mut self, p: &mut dyn Platform, i: usize, event: &Event, serial: u32) -> Status {
        let now = p.now();
        let serial = if serial != 0 {
            self.last_press = (serial, now);
            serial
        } else if now.saturating_sub(self.last_press.1) < PRESS_GOOD_NS {
            self.last_press.0
        } else {
            0
        };
        let Some(mut tree) = self.windows[i].tree.take() else {
            return Status::Ignored;
        };
        let w = &self.windows[i];
        let mut asks = Asks {
            focus: w.focus,
            serial,
            now: p.now(),
            window_focused: w.focused || matches!(w.role, Purpose::Popup { .. }),
            clipboard: self.clipboard,
            theme: self.theme,
            ..Asks::default()
        };
        let mut messages: Vec<Msg<A::Message>> = Vec::new();
        let node = core::mem::take(&mut self.windows[i].node);
        let status = with_root(&self.app, &self.windows, i, &self.theme, |root| {
            let mut shell = Shell::new(&mut messages, &mut asks, p);
            root.widget.on_event(&mut tree, &node, event, &mut shell)
        })
        .unwrap_or(Status::Ignored);
        let w = &mut self.windows[i];
        w.tree = Some(tree);
        w.node = node;
        w.focus = asks.focus;
        if asks.redraw || asks.relayout {
            w.needs.update = true;
        }
        let id = w.id;
        let is_popup = matches!(w.role, Purpose::Popup { .. });

        if let Some(ask) = asks.popup {
            self.popup(p, i, ask, serial);
        }
        // A popup closes once it has sent something (a menu item was chosen).
        if is_popup && !messages.is_empty() {
            self.close(p, id);
        }
        // The window may have closed (a dialog answered): find it again by its number.
        for m in messages {
            match self.index(id) {
                Some(i) => self.deliver(p, i, m, serial),
                None => {
                    if let Msg::App(m) = m {
                        self.update(p, m, serial, WindowId::MAIN);
                    }
                }
            }
        }
        status
    }

    /// Acts on a widget's message: the app's go to `update`, a dialog's to the dialog.
    fn deliver(&mut self, p: &mut dyn Platform, i: usize, m: Msg<A::Message>, serial: u32) {
        let id = self.windows[i].id;
        match m {
            Msg::App(m) => {
                // A popup's messages are the app's, as if from the popup's parent.
                let from = match self.windows[i].role {
                    Purpose::Popup { parent, .. } => parent,
                    _ => id,
                };
                self.update(p, m, serial, from);
            }
            Msg::Dialog(a) => self.answer(p, i, a, serial),
        }
    }

    /// A dialog's answer: the line typed is kept; a button closes the dialog and sends the
    /// app what it stands for.
    fn answer(&mut self, p: &mut dyn Platform, i: usize, a: Answer, serial: u32) {
        let id = self.windows[i].id;
        let Purpose::Dialog(d) = &mut self.windows[i].role else {
            return;
        };
        match a {
            Answer::Text(s) => {
                if let Some(input) = &mut d.input {
                    input.value = s;
                }
                self.windows[i].needs.update = true;
            }
            Answer::Button(n) => {
                let m = d.answer(n);
                self.close(p, id);
                if let Some(m) = m {
                    self.update(p, m, serial, WindowId::MAIN);
                }
            }
        }
    }

    /// Gives the app a message.
    fn update(&mut self, p: &mut dyn Platform, m: A::Message, serial: u32, window: WindowId) {
        let win = self.windows.iter().find(|w| w.id == window).map(|w| w.win);
        let mut cx = Context {
            asked: &mut self.asked,
            timers: &mut self.timers,
            next_timer: &mut self.next_timer,
            platform: p,
            serial,
            window,
            win,
            clipboard: self.clipboard,
        };
        self.app.update(m, &mut cx);
        self.after_update();
    }

    /// After the app changed: every window's view may have.
    fn after_update(&mut self) {
        for w in &mut self.windows {
            w.needs.update = true;
        }
    }

    // ---- Popups. ----

    fn popup(&mut self, p: &mut dyn Platform, i: usize, ask: PopupAsk, serial: u32) {
        match ask {
            PopupAsk::Open { owner, at } => self.open_popup(p, i, owner, at, serial),
            PopupAsk::Close => {
                let target = match self.windows[i].role {
                    Purpose::Popup { .. } => Some(self.windows[i].id),
                    _ => self.windows[i].popup,
                };
                if let Some(id) = target {
                    self.close(p, id);
                }
            }
            PopupAsk::Step(step) => {
                if let Purpose::Popup { parent, .. } = self.windows[i].role
                    && let Some(pi) = self.index(parent)
                {
                    self.dispatch(p, pi, &Event::PopupStep(step), serial);
                }
            }
        }
    }

    fn open_popup(&mut self, p: &mut dyn Platform, i: usize, owner: u64, at: Point, serial: u32) {
        if let Some(old) = self.windows[i].popup {
            self.close(p, old);
        }
        let parent = self.windows[i].id;
        let parent_win = self.windows[i].win;
        let id = WindowId(self.asked.next_window);
        self.asked.next_window = self.asked.next_window.wrapping_add(1).max(1);
        self.windows.push(Window::new(id, Win(0), Purpose::Popup { parent, owner }));
        let k = self.windows.len() - 1;
        let theme = self.theme;
        let size = with_root(&self.app, &self.windows, k, &theme, |root| {
            let mut tree = root.tree();
            laid_out(root, &mut tree, Limits::loose(Size::new(UNBOUNDED, UNBOUNDED)), &theme).size()
        });
        let created = size.filter(|s| s.width > 0 && s.height > 0).and_then(|s| {
            p.create(&NewWindow {
                kind: WindowKind::Popup,
                parent: Some(parent_win),
                title: String::new(),
                size: (s.width as u32, s.height as u32),
                flags: WindowFlags(0),
                at: (at.x, at.y),
                serial,
            })
        });
        match created {
            Some(win) => {
                self.windows[k].win = win;
                self.windows[i].popup = Some(id);
            }
            None => {
                self.windows.pop();
                self.dispatch(p, i, &Event::PopupClosed, 0);
            }
        }
    }

    // ---- Opening and closing windows. ----

    /// Closes one of the app's windows (with its popup), telling a popup's owner.
    fn close(&mut self, p: &mut dyn Platform, id: WindowId) {
        let Some(i) = self.index(id) else {
            return;
        };
        if let Some(child) = self.windows[i].popup {
            self.close(p, child);
        }
        let Some(i) = self.index(id) else {
            return;
        };
        let w = self.windows.remove(i);
        p.destroy(w.win);
        if let Purpose::Popup { parent, .. } = w.role
            && let Some(pi) = self.index(parent)
        {
            self.windows[pi].popup = None;
            self.dispatch(p, pi, &Event::PopupClosed, 0);
        }
    }

    /// Acts on what `update` asked for: windows and dialogs to open or close, the focus to
    /// move, the end.
    fn act_on_asked(&mut self, p: &mut dyn Platform) {
        if self.asked.quit {
            self.quit = true;
        }
        for id in core::mem::take(&mut self.asked.close) {
            self.close(p, id);
        }
        let main_win = self.display_window(WindowId::MAIN);
        for (id, d) in core::mem::take(&mut self.asked.dialogs) {
            let title = String::from(d.title());
            let focus = dialog::first_focus(&d);
            self.windows.push(Window::new(id, Win(0), Purpose::Dialog(d)));
            if let Some(w) = self.windows.last_mut() {
                w.focus_key = Some(String::from(focus));
            }
            let k = self.windows.len() - 1;
            let theme = self.theme;
            let size = with_root(&self.app, &self.windows, k, &theme, |root| {
                let mut tree = root.tree();
                laid_out(root, &mut tree, Limits::loose(DIALOG_MAX), &theme).size()
            })
            .unwrap_or(Size::new(240, 120));
            let new = NewWindow {
                kind: WindowKind::Dialog,
                parent: main_win,
                title: title.clone(),
                size: (size.width.max(1) as u32, size.height.max(1) as u32),
                flags: WindowFlags::MODAL,
                at: (0, 0),
                serial: 0,
            };
            match p.create(&new) {
                Some(win) => {
                    self.windows[k].win = win;
                    self.windows[k].title = title;
                }
                None => {
                    self.windows.pop();
                }
            }
        }
        for (id, spec) in core::mem::take(&mut self.asked.open) {
            self.open_other(p, id, &spec, main_win);
        }
        for (id, key) in core::mem::take(&mut self.asked.focus) {
            if let Some(i) = self.index(id) {
                self.windows[i].focus_key = Some(key);
                self.windows[i].needs.update = true;
            }
        }
    }

    fn open_other(
        &mut self,
        p: &mut dyn Platform,
        id: WindowId,
        spec: &WindowSpec,
        main: Option<Win>,
    ) {
        let mut flags = WindowFlags(0);
        if spec.resizable {
            flags = flags | WindowFlags::RESIZABLE;
        }
        if spec.modal && spec.dialog {
            flags = flags | WindowFlags::MODAL;
        }
        let title = self.app.window_title(id);
        let new = NewWindow {
            kind: if spec.dialog { WindowKind::Dialog } else { WindowKind::Normal },
            parent: if spec.dialog { main } else { None },
            title: title.clone(),
            size: spec.size,
            flags,
            at: (0, 0),
            serial: 0,
        };
        if let Some(win) = p.create(&new) {
            let mut w = Window::new(id, win, Purpose::Other);
            w.title = title;
            self.windows.push(w);
        }
    }

    // ---- Drawing. ----

    /// Brings everything up to date: what `update` asked for, then every window that needs
    /// it is built, laid out and drawn.
    pub fn settle(&mut self, p: &mut dyn Platform) {
        // Acting may update the app again (a dialog that couldn't open), so a few rounds.
        for _ in 0..4 {
            self.act_on_asked(p);
            if self.asked.close.is_empty()
                && self.asked.dialogs.is_empty()
                && self.asked.open.is_empty()
            {
                break;
            }
        }
        if self.quit {
            return;
        }
        self.render_all(p);
        // Widgets that depend on their size hear of a new one once it is laid out; what they
        // send may change the view again.
        let resized: Vec<WindowId> = self
            .windows
            .iter_mut()
            .filter(|w| w.resized && w.tree.is_some())
            .map(|w| {
                w.resized = false;
                w.id
            })
            .collect();
        if !resized.is_empty() {
            for id in resized {
                if let Some(i) = self.index(id) {
                    self.dispatch(p, i, &Event::Resized, 0);
                }
            }
            self.act_on_asked(p);
            self.render_all(p);
        }
        self.update_titles(p);
    }

    /// Builds and draws every window that needs it.
    fn render_all(&mut self, p: &mut dyn Platform) {
        let mut i = 0;
        // A window may close while others are drawn (a popup whose owner went), so the
        // length is looked at each time.
        while i < self.windows.len() {
            if self.windows[i].configured
                && (self.windows[i].needs.update || self.windows[i].needs.all)
            {
                self.render(p, i);
            }
            i += 1;
        }
    }

    fn update_titles(&mut self, p: &mut dyn Platform) {
        for i in 0..self.windows.len() {
            let title = match self.windows[i].role {
                Purpose::Main => self.app.title(),
                Purpose::Other => self.app.window_title(self.windows[i].id),
                _ => continue,
            };
            if title != self.windows[i].title {
                p.set_title(self.windows[i].win, &title);
                self.windows[i].title = title;
            }
        }
    }

    /// Builds, lays out and draws window `i`, sending the display server only what changed.
    fn render(&mut self, p: &mut dyn Platform, i: usize) {
        let theme = self.theme;
        let old_tree = self.windows[i].tree.take();
        let old_looks = core::mem::take(&mut self.windows[i].looks);
        let w = &self.windows[i];
        let (size, factor, win, all, focus_key) =
            (w.size, w.factor, w.win, w.needs.all, w.focus_key.clone());
        let mut focus = w.focus;
        let window_focused = w.focused || matches!(w.role, Purpose::Popup { .. });
        let whole = Rect::new(0, 0, size.width, size.height);

        let done = with_root(&self.app, &self.windows, i, &theme, |root| {
            let mut tree = diffed(root, old_tree);
            let node = laid_out(root, &mut tree, Limits::exact(size), &theme);
            if let Some(key) = &focus_key
                && let Some(id) = tree.find_key(key)
            {
                focus = Some(id);
            }
            if focus.is_some_and(|f| !tree.contains(f)) {
                focus = None;
            }
            let cx = DrawCx { theme: &theme, focus, window_focused, clip: whole };
            let mut list = Vec::new();
            root.widget.looks(&tree, &node, whole, &cx, &mut list);
            let looks: BTreeMap<u64, (Rect, u64)> =
                list.into_iter().map(|(id, r, l)| (id, (r, l))).collect();

            let mut damage = Damage::new();
            if all {
                damage.add(whole);
            } else {
                for (id, (r, l)) in &looks {
                    match old_looks.get(id) {
                        Some((r0, l0)) if r0 == r && l0 == l => {}
                        Some((r0, _)) => {
                            damage.add(*r0);
                            damage.add(*r);
                        }
                        None => damage.add(*r),
                    }
                }
                for (id, (r0, _)) in &old_looks {
                    if !looks.contains_key(id) {
                        damage.add(*r0);
                    }
                }
            }
            let mut drawn = true;
            if !damage.is_empty() {
                drawn = p.draw(win, &mut |canvas| {
                    for r in damage.rects() {
                        let r = r.intersect(&whole);
                        let mut c = canvas.clipped(r);
                        c.fill(theme.background);
                        let cx = DrawCx { theme: &theme, focus, window_focused, clip: r };
                        root.widget.draw(&tree, &node, &mut c, &cx);
                    }
                    damage.clone()
                });
            }
            (tree, node, looks, drawn)
        });

        let w = &mut self.windows[i];
        w.factor = factor;
        match done {
            Some((tree, node, looks, drawn)) => {
                w.tree = Some(tree);
                w.node = node;
                w.focus = focus;
                w.focus_key = None;
                w.looks = looks;
                w.needs = if drawn { Needs::default() } else { Needs { update: true, all: true } };
            }
            None => {
                // Nothing to show: a popup whose owner has gone closes.
                w.needs = Needs::default();
                let id = w.id;
                if matches!(w.role, Purpose::Popup { .. }) {
                    self.close(p, id);
                }
            }
        }
    }
}

impl Tree {
    /// Whether this tree was made for an element of `root`'s kind.
    pub(crate) fn is_for<M>(&self, root: &Element<'_, M>) -> bool {
        self.made_for(root.widget.kind(), root.widget.tag())
    }
}
