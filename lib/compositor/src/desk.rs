// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The display server's state and rules: sessions, clients and their windows, stacking and the
//! focus, buffers and frames, pings, the clipboard and the limits
//! (docs/design/0022-display-server.md, sections 1 and 3 to 8).
//!
//! [`Desk`] is plain computation: it is given what happened (a request from a client, an input
//! event, the time) and answers, and it queues what the display server must do in return: an
//! event for a client's ring ([`Out::Event`]), buffers it may now let go of ([`Out::Free`]),
//! parts of the space to draw again (`take_damage`). It makes no system calls, so all of it is
//! tested on the host, hostile clients included (`tests/`).
//!
//! **Who may do what** is decided here, from what the sandbox manager said about each client
//! when it asked for the client's capability (its flags, title and app number, note 0022,
//! section 1): the display server's program only finds out which client a request came from
//! (by the capability's badge) and passes that in. Every request is checked against the
//! client's own windows: a client can only ever name its own.
//!
//! **Serials.** Every configure, key press, button press and ping a client is sent carries a
//! number that is never used twice. A client quotes a configure's serial to ask for buffers of
//! that size, a ping's to answer it, and a press's to do what only the user's own action may
//! start: open a popup, start moving its window, copy or paste. A press's serial is good for
//! one second, for the client it was sent to, which is how an app is kept from pasting, or
//! opening something that looks like a menu, on its own.

use crate::decor::{self, Button};
use crate::event::{Event, kind, state};
use crate::space::{self, Output, Place, Screen, Space};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_draw::{Damage, Rect};
use pbr_interfaces::pbr_window::{Cursor, WindowError, WindowFlags, WindowKind};

/// A client: one app's capability. Numbers are never used twice while the display server runs,
/// so a capability that has gone can't be mistaken for a new one.
pub type ClientId = u32;

/// A window: its client, and the client's own number for it (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId {
    /// Whose it is.
    pub client: ClientId,
    /// The client's number for it.
    pub number: u32,
}

impl WindowId {
    /// As the desktop names any window: the client in the high 32 bits, the window in the low.
    pub fn packed(self) -> u64 {
        (u64::from(self.client) << 32) | u64::from(self.number)
    }

    /// The other way round.
    pub fn unpack(v: u64) -> WindowId {
        WindowId { client: (v >> 32) as u32, number: v as u32 }
    }
}

/// Client flags (`pbr.display.session`'s `ClientFlags`).
pub mod flags {
    /// May make `System` windows, and check other clients' serials.
    pub const SYSTEM: u32 = 1;
    /// Is the session's desktop.
    pub const DESKTOP: u32 = 2;
    /// Can reach all the user's files: its title bars say so.
    pub const TRUSTED: u32 = 4;
}

/// What the desktop's `act` does (`ACT_*`).
pub mod act {
    #![allow(missing_docs)]
    pub const FOCUS: u32 = 1;
    pub const RAISE: u32 = 2;
    pub const MINIMISE: u32 = 3;
    pub const RESTORE: u32 = 4;
    pub const CLOSE: u32 = 5;
}

/// What the farewell screen says (note 0024, section 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Farewell {
    /// "Shutting down..."
    ShuttingDown,
    /// "Restarting..."
    Restarting,
    /// The computer couldn't be turned off by software: "PrettyBigRock has shut down. You can
    /// turn the computer off now." (1.3's last screen.)
    Halted,
}

/// The most windows one client may have.
pub const MAX_WINDOWS: u32 = 32;
/// The most clients in a session.
pub const MAX_CLIENTS: usize = 64;
/// The most sessions at once.
pub const MAX_SESSIONS: usize = 4;
/// The most bytes of window buffers a session's windows may have together.
pub const SESSION_BUFFERS: u64 = 96 * 1024 * 1024;
/// The largest clipboard item.
pub const MAX_CLIPBOARD: u64 = 16 * 1024 * 1024;
/// How long a press's serial is good for, in nanoseconds.
pub const SERIAL_LIFE: u64 = 1_000_000_000;
/// How often a client whose window has the focus is pinged.
pub const PING_EVERY: u64 = 2_000_000_000;
/// How long a client has to answer a ping before it is "not responding".
pub const PING_WAIT: u64 = 5_000_000_000;
/// How long after the user launched an app its first window may still take the focus.
pub const LAUNCH_FOCUS: u64 = 2_000_000_000;
/// How many presses' serials are remembered for each client.
const PRESSES_KEPT: usize = 32;
/// The largest a window may be, in logical pixels, each way (the drawing library's limit).
pub const MAX_SIDE: i32 = 16384;
/// The screen size used for the limit on buffers while there are no screens at all.
const NO_SCREEN: (i32, i32) = (1920, 1200);
/// Between two configure events of a window being resized by the user.
const RESIZE_EVERY: u64 = 33_000_000;
/// How far a new window opens from the one opened before it, down and right.
const CASCADE: i32 = 24;

/// What the display server must do because of something the desk was told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Put this event in the client's ring.
    Event(ClientId, Event),
    /// The window's buffers of this generation are no longer needed: let go of them.
    Free {
        /// The window.
        window: WindowId,
        /// Which buffers.
        generation: u32,
    },
}

/// One window's buffers: two frames, made for one configure event's size and scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffers {
    /// Counts up for each window; 0 is never used.
    pub generation: u32,
    /// Each frame's width in device pixels.
    pub width: u32,
    /// Each frame's height in device pixels.
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
    /// The scale they were made for.
    pub scale: u32,
    /// The window's size they were made for, in logical pixels.
    pub logical: (i32, i32),
}

impl Buffers {
    /// Bytes in one frame.
    pub fn frame_bytes(&self) -> u64 {
        u64::from(self.stride) * u64::from(self.height)
    }

    /// Bytes in both.
    pub fn bytes(&self) -> u64 {
        2 * self.frame_bytes()
    }
}

/// What `buffers` answers: buffers to make, or the ones made already for that size and scale
/// to hand out again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Made {
    /// The buffers.
    pub buffers: Buffers,
    /// They exist already (a configure that changed only the state): give a copy of the same
    /// memory.
    pub reused: bool,
}

/// The frame on the screen for a window, and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// Its buffers.
    pub buffers: Buffers,
    /// Frame 0 or 1.
    pub frame: u32,
}

/// A window.
#[derive(Clone, Debug)]
pub(crate) struct Window {
    pub(crate) id: WindowId,
    pub(crate) kind: WindowKind,
    pub(crate) parent: Option<WindowId>,
    pub(crate) flags: WindowFlags,
    /// The app's own part of the title.
    pub(crate) title: String,
    /// The content's corner in the space, and its size, in logical pixels.
    pub(crate) place: (i32, i32),
    pub(crate) size: (i32, i32),
    /// The scale it was last told, in hundredths.
    pub(crate) scale: u32,
    pub(crate) min: (i32, i32),
    pub(crate) max: (i32, i32),
    pub(crate) cursor: Cursor,
    /// Where it was before it was maximised.
    pub(crate) restore: Option<((i32, i32), (i32, i32))>,
    pub(crate) minimised: bool,
    /// A popup that was dismissed (it stays until the app destroys it, but isn't shown).
    pub(crate) dismissed: bool,
    /// Opened without the focus: the taskbar shows it wants the user to look.
    pub(crate) attention: bool,
    /// A system window opened over another app's window, which it blocks input to.
    pub(crate) over: Option<WindowId>,
    /// For backgrounds and panels: the screen, and a panel's edge (1 top, 2 bottom, 3 left,
    /// 4 right) and thickness.
    pub(crate) screen: u32,
    pub(crate) edge: u32,
    pub(crate) thickness: i32,
    /// The last configure event's serial, and the few before it (to tell an overtaken
    /// serial of this window's from one it was never given).
    pub(crate) configured: u32,
    pub(crate) earlier: [u32; 8],
    /// The newest buffers made, and which of their frames the app holds.
    pub(crate) latest: Option<Buffers>,
    pub(crate) held: [bool; 2],
    pub(crate) shown: Option<Shown>,
    pub(crate) wants_frame: bool,
    /// When the last configure was sent while the user was resizing it.
    pub(crate) resized_at: u64,
    /// The order windows were opened in, for the desktop's list.
    pub(crate) opened: u64,
}

impl Window {
    /// Whether the display server draws a frame and title bar round it.
    pub(crate) fn decorated(&self) -> bool {
        matches!(self.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System)
    }

    /// The rectangle the app draws in, in the space.
    pub(crate) fn content(&self) -> Rect {
        Rect::new(self.place.0, self.place.1, self.size.0, self.size.1)
    }

    /// Everything it can change on the screen: content, decorations and shadow.
    pub(crate) fn bounds(&self) -> Rect {
        decor::bounds(self.content(), self.decorated())
    }

    /// The frame, or the content of an undecorated window: what covers what is below.
    pub(crate) fn outline(&self) -> Rect {
        if self.decorated() { decor::frame(self.content()) } else { self.content() }
    }

    /// Whether the user can focus it (and type into it).
    pub(crate) fn focusable(&self) -> bool {
        matches!(self.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System)
    }

    /// Its layer: backgrounds; normal windows and dialogs; panels; popups; system windows.
    pub(crate) fn layer(&self) -> u8 {
        match self.kind {
            WindowKind::Background => 0,
            WindowKind::Normal | WindowKind::Dialog => 1,
            WindowKind::Panel => 2,
            WindowKind::Popup => 3,
            WindowKind::System => 4,
        }
    }
}

/// A press delivered to a client: its serial, the window, and when.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Press {
    pub(crate) serial: u32,
    pub(crate) window: u32,
    pub(crate) time: u64,
}

/// A client: one app, as the sandbox manager described it.
#[derive(Clone, Debug)]
pub(crate) struct Client {
    pub(crate) session: u32,
    pub(crate) app: u32,
    /// Its manifest's title.
    pub(crate) title: String,
    pub(crate) flags: u32,
    pub(crate) launched: u64,
    /// How many windows it has, and the number the next one gets.
    pub(crate) windows: u32,
    pub(crate) next_window: u32,
    /// A ping not yet answered: its serial and when it was sent.
    pub(crate) ping: Option<(u32, u64)>,
    pub(crate) last_ping: u64,
    pub(crate) not_responding: bool,
    pub(crate) presses: VecDeque<Press>,
}

/// The clipboard: one item in up to two forms (note 0022, section 5).
#[derive(Clone, Debug, Default)]
pub(crate) struct Clipboard {
    /// The serial and client of the copy that started the item.
    pub(crate) serial: u32,
    pub(crate) client: ClientId,
    pub(crate) text: Option<Vec<u8>>,
    pub(crate) picture: Option<(u32, u32, Vec<u8>)>,
}

impl Clipboard {
    /// The forms there, as bits (1 text, 2 picture).
    pub(crate) fn forms(&self) -> u32 {
        u32::from(self.text.is_some()) | (u32::from(self.picture.is_some()) << 1)
    }

    fn bytes(&self) -> u64 {
        self.text.as_ref().map_or(0, |t| t.len() as u64)
            + self.picture.as_ref().map_or(0, |p| p.2.len() as u64)
    }
}

/// One user's windows.
#[derive(Clone, Debug)]
pub(crate) struct Session {
    pub(crate) number: u32,
    /// Whose it is (for the About window's summary, later).
    #[allow(dead_code)]
    pub(crate) user: String,
    /// Every window, back to front.
    pub(crate) stack: Vec<WindowId>,
    pub(crate) focus: Option<WindowId>,
    pub(crate) clipboard: Clipboard,
    /// Windows opened so far: where the next one cascades to.
    pub(crate) opened: u32,
}

/// The window menu (note 0022, section 6), drawn by the display server over everything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// The window it is for.
    pub window: WindowId,
    /// Its rectangle in the space.
    pub rect: Rect,
    /// Its rows.
    pub items: Vec<MenuItem>,
    /// The row under the pointer.
    pub hover: Option<usize>,
}

/// A row of the window menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// Minimise the window.
    Minimise,
    /// Maximise it.
    Maximise,
    /// Give it back its size from before it was maximised.
    Restore,
    /// Move it to this screen.
    MoveTo(u32),
    /// "What can this app reach?": sent to the desktop.
    Permissions,
    /// Stop the app (when it isn't responding): sent to the desktop.
    Stop,
    /// Ask the app to close the window.
    Close,
}

impl MenuItem {
    /// The row's words.
    pub fn label(self) -> String {
        match self {
            MenuItem::Minimise => String::from("Minimise"),
            MenuItem::Maximise => String::from("Maximise"),
            MenuItem::Restore => String::from("Restore"),
            MenuItem::MoveTo(n) => format!("Move to screen {n}"),
            MenuItem::Permissions => String::from("What can this app reach?"),
            MenuItem::Stop => String::from("Stop"),
            MenuItem::Close => String::from("Close"),
        }
    }
}

/// What the pointer is doing while a button is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    /// No button is held.
    None,
    /// Pressed in a window's content: everything goes to it until every button is let go.
    Pressed(WindowId),
    /// Moving a window: where the pointer is from its content's corner, in 1/256 pixels.
    Move(WindowId, i64, i64),
    /// Resizing a window by an edge: its content when it started, and the pointer then.
    Resize(WindowId, u32, Rect, (i64, i64)),
    /// Pressed on a title bar button: it acts if let go on it.
    Button(WindowId, Button),
    /// Pressed on the window menu.
    Menu,
    /// Pressed where nothing takes it.
    Nothing,
}

/// The pointer.
#[derive(Clone, Debug)]
pub(crate) struct Pointer {
    /// Where, in 1/256 logical pixels.
    pub(crate) x: i64,
    pub(crate) y: i64,
    /// Buttons held (bit n for button n).
    pub(crate) buttons: u32,
    pub(crate) grab: Grab,
    /// The window whose content it is in, which has had a pointer event.
    pub(crate) hover: Option<WindowId>,
    /// The last press on a title bar: the window and when, for a double click.
    pub(crate) last_title: Option<(WindowId, u64)>,
    /// Pointer speed, hundredths.
    pub(crate) speed: u32,
    /// Shown at all: there is some pointing device, or there was.
    pub(crate) seen: bool,
}

/// An input device the seat has, as far as the desk cares: tablets, and what they are tied to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tablet {
    /// The seat's number for it.
    pub id: u16,
    /// Its name.
    pub name: String,
}

/// The state of the whole display: screens, sessions and their windows, the pointer.
pub struct Desk {
    pub(crate) space: Space,
    pub(crate) sessions: Vec<Session>,
    pub(crate) active: Option<u32>,
    next_session: u32,
    pub(crate) clients: BTreeMap<ClientId, Client>,
    next_client: ClientId,
    pub(crate) windows: BTreeMap<WindowId, Window>,
    next_serial: u32,
    opened: u64,
    pub(crate) pointer: Pointer,
    pub(crate) menu: Option<Menu>,
    pub(crate) tablets: Vec<Tablet>,
    /// The desktop's ties: a tablet's name, and its screen (0 the whole space).
    pub(crate) ties: Vec<(String, u32)>,
    /// Keys whose press went to a window, so their release goes there too; and keys whose
    /// press the display server kept (Alt+Tab, the start menu key), whose release it keeps.
    pub(crate) keys_to: Vec<(u16, WindowId)>,
    pub(crate) kept_keys: Vec<u16>,
    /// The windows Alt+Tab goes through while Alt is held, and how far it has gone.
    pub(crate) switching: Option<(Vec<WindowId>, usize)>,
    /// Shutting down (`farewell`, `halted`).
    pub(crate) farewell: Option<Farewell>,
    damage: Damage,
    full_damage: bool,
    out: Vec<Out>,
    /// Bumped by anything the description lines would show differently.
    pub(crate) changes: u64,
}

impl Default for Desk {
    fn default() -> Self {
        Self::new()
    }
}

/// A request's answer.
pub type Answer<T> = Result<T, WindowError>;

impl Desk {
    /// Nothing at all: no screens, no sessions.
    pub fn new() -> Desk {
        Desk {
            space: Space::new(),
            sessions: Vec::new(),
            active: None,
            next_session: 1,
            clients: BTreeMap::new(),
            next_client: 1,
            windows: BTreeMap::new(),
            next_serial: 1,
            opened: 0,
            pointer: Pointer {
                x: 0,
                y: 0,
                buttons: 0,
                grab: Grab::None,
                hover: None,
                last_title: None,
                speed: 100,
                seen: false,
            },
            menu: None,
            tablets: Vec::new(),
            ties: Vec::new(),
            keys_to: Vec::new(),
            kept_keys: Vec::new(),
            switching: None,
            farewell: None,
            damage: Damage::new(),
            full_damage: true,
            out: Vec::new(),
            changes: 1,
        }
    }

    // ---- What the display server takes out --------------------------------------------

    /// Everything queued for the display server to do, in order.
    pub fn take_out(&mut self) -> Vec<Out> {
        core::mem::take(&mut self.out)
    }

    /// The parts of the space to draw again, in logical pixels; `None` for all of it.
    pub fn take_damage(&mut self) -> Option<Vec<Rect>> {
        let full = core::mem::replace(&mut self.full_damage, false);
        let rects: Vec<Rect> = self.damage.rects().to_vec();
        self.damage.clear();
        if full { None } else { Some(rects) }
    }

    /// Whether anything is waiting to be drawn.
    pub fn has_damage(&self) -> bool {
        self.full_damage || !self.damage.is_empty()
    }

    /// A number that changes whenever the description lines would (note 0022, section 13).
    pub fn changes(&self) -> u64 {
        self.changes
    }

    pub(crate) fn changed(&mut self) {
        self.changes = self.changes.wrapping_add(1);
    }

    pub(crate) fn damage(&mut self, r: Rect) {
        if !r.is_empty() {
            self.damage.add(r);
        }
    }

    pub(crate) fn damage_all(&mut self) {
        self.full_damage = true;
    }

    pub(crate) fn send(&mut self, client: ClientId, e: Event) {
        self.out.push(Out::Event(client, e));
    }

    /// A serial never used before (wrapping round after four thousand million, skipping 0).
    pub(crate) fn serial(&mut self) -> u32 {
        let s = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        s
    }

    // ---- Screens ----------------------------------------------------------------------

    /// The screens.
    pub fn space(&self) -> &Space {
        &self.space
    }

    /// The drivers' screens changed (or the display server found them for the first time):
    /// windows on a screen that went move to the main one, windows whose screen now has
    /// another scale are told, and the desktop hears.
    pub fn set_outputs(&mut self, outputs: &[Output], now: u64) {
        let old: Vec<Screen> = self.space.screens().to_vec();
        let changes = self.space.update(outputs);
        if !changes.any {
            return;
        }
        // Windows mostly on a screen that went go to the main screen (note 0022, section 2).
        for (number, rect) in &changes.gone {
            let ids: Vec<WindowId> = self.windows.keys().copied().collect();
            for id in ids {
                let Some(w) = self.windows.get(&id) else { continue };
                if matches!(w.kind, WindowKind::Background | WindowKind::Panel) {
                    continue;
                }
                // Popups follow their parents; dialogs and the rest go by where they are.
                if w.kind == WindowKind::Popup {
                    continue;
                }
                let was = holding_most_of(&old, w.content());
                if was != Some(*number) {
                    continue;
                }
                self.move_to_screen(id, *rect, None, now);
            }
        }
        self.screens_changed(now);
    }

    /// After any change to the screens: backgrounds and panels follow their screens, every
    /// window's scale is checked, the pointer stays on a screen, and everything is drawn again.
    fn screens_changed(&mut self, now: u64) {
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in &ids {
            self.fit_fixed(*id, now);
        }
        for id in &ids {
            self.check_scale(*id, now);
        }
        // Maximised windows fill their screen's work area, which may have changed size.
        for id in &ids {
            if self.windows.get(id).is_some_and(|w| w.restore.is_some()) {
                self.fill_work_area(*id, now);
            }
        }
        let (x, y) = self.space.clamp(self.pointer.x, self.pointer.y);
        self.pointer.x = x;
        self.pointer.y = y;
        self.menu = None;
        self.damage_all();
        self.changed();
        for s in self.sessions.iter().map(|s| s.number).collect::<Vec<_>>() {
            self.tell_desktop(s, Event::new(kind::SCREENS, 0, [0; 3], now));
        }
    }

    /// Moves a window from a screen whose part of the space was `from` to `to` (or the main
    /// screen), keeping its place from the screen's corner as far as it fits.
    fn move_to_screen(&mut self, id: WindowId, from: Rect, to: Option<u32>, now: u64) {
        let target = to.and_then(|n| self.space.screen(n)).or_else(|| self.space.main());
        let Some(target) = target.map(Screen::rect) else { return };
        let Some(w) = self.windows.get(&id) else { return };
        let outline = w.outline();
        let content = w.content();
        // The outline's place from the old screen's corner, kept if it fits on the new one.
        let dx = outline.x - from.x;
        let dy = outline.y - from.y;
        let fit = |off: i32, len: i32, room: i32| off.clamp(0, (room - len).max(0));
        let nx = target.x + fit(dx, outline.width, target.width);
        let ny = target.y + fit(dy, outline.height, target.height);
        let place = (nx + (content.x - outline.x), ny + (content.y - outline.y));
        self.set_place(id, place, now);
    }

    /// Keeps a background the size of its screen, and a panel along its edge.
    fn fit_fixed(&mut self, id: WindowId, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let (kind, screen, edge, thickness) = (w.kind, w.screen, w.edge, w.thickness);
        if !matches!(kind, WindowKind::Background | WindowKind::Panel) {
            return;
        }
        let Some(r) = self.space.screen(screen).filter(|s| s.in_space()).map(Screen::rect) else {
            return;
        };
        let (place, size) = match (kind, edge) {
            (WindowKind::Background, _) => ((r.x, r.y), (r.width, r.height)),
            (_, 1) => ((r.x, r.y), (r.width, thickness)),
            (_, 2) => ((r.x, r.y + r.height - thickness), (r.width, thickness)),
            (_, 3) => ((r.x, r.y), (thickness, r.height)),
            _ => ((r.x + r.width - thickness, r.y), (thickness, r.height)),
        };
        self.set_place(id, place, now);
        self.set_size(id, size, now);
    }

    /// The screen's part of the space without its panels: where windows maximise and open.
    pub fn work_area(&self, screen: u32) -> Option<Rect> {
        let s = self.space.screen(screen).filter(|s| s.in_space())?;
        let r = s.rect();
        let (mut top, mut bottom, mut left, mut right) = (0, 0, 0, 0);
        let session = self.active.and_then(|n| self.session(n))?;
        for id in &session.stack {
            let Some(w) = self.windows.get(id) else { continue };
            if w.kind != WindowKind::Panel || w.screen != screen {
                continue;
            }
            match w.edge {
                1 => top += w.thickness,
                2 => bottom += w.thickness,
                3 => left += w.thickness,
                _ => right += w.thickness,
            }
        }
        Some(Rect::new(
            r.x + left,
            r.y + top,
            (r.width - left - right).max(0),
            (r.height - top - bottom).max(0),
        ))
    }

    /// The desktop's arrangement of the screens.
    pub fn arrange(&mut self, client: ClientId, places: &[(u32, Place)], now: u64) -> Answer<()> {
        self.desktop_only(client)?;
        self.space.arrange(places).map_err(|e| match e {
            space::ArrangeError::NoSuchScreen => WindowError::NoSuchWindow,
            space::ArrangeError::BadArgument => WindowError::BadArgument,
        })?;
        self.screens_changed(now);
        Ok(())
    }

    // ---- Sessions and clients ---------------------------------------------------------

    pub(crate) fn session(&self, number: u32) -> Option<&Session> {
        self.sessions.iter().find(|s| s.number == number)
    }

    pub(crate) fn session_mut(&mut self, number: u32) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.number == number)
    }

    /// A new session for `user`, with its number; `None` if there are too many.
    pub fn start_session(&mut self, user: &str) -> Option<u32> {
        if self.sessions.len() >= MAX_SESSIONS {
            return None;
        }
        let number = self.next_session;
        self.next_session = self.next_session.wrapping_add(1).max(1);
        self.sessions.push(Session {
            number,
            user: String::from(user),
            stack: Vec::new(),
            focus: None,
            clipboard: Clipboard::default(),
            opened: 0,
        });
        Some(number)
    }

    /// Shows session `number` and gives it the input. `false` if there is no such session.
    pub fn activate(&mut self, number: u32) -> bool {
        if self.session(number).is_none() {
            return false;
        }
        if self.active != Some(number) {
            self.end_grab();
            self.active = Some(number);
            self.menu = None;
            self.damage_all();
            self.changed();
        }
        true
    }

    /// The active session's number.
    pub fn active(&self) -> Option<u32> {
        self.active
    }

    /// A session's capability has gone: every client of it goes, and with them their windows.
    pub fn end_session(&mut self, number: u32, now: u64) {
        let clients: Vec<ClientId> =
            self.clients.iter().filter(|(_, c)| c.session == number).map(|(id, _)| *id).collect();
        for c in clients {
            self.remove_client(c, now);
        }
        self.sessions.retain(|s| s.number != number);
        if self.active == Some(number) {
            self.active = None;
            self.damage_all();
        }
        self.changed();
    }

    /// A client for an app the session's sandbox manager is launching (`client`). `None` if
    /// there's no such session or it has too many clients.
    pub fn add_client(
        &mut self,
        session: u32,
        app: u32,
        title: &str,
        flags: u32,
        launched: u64,
    ) -> Option<ClientId> {
        self.session(session)?;
        if self.clients.values().filter(|c| c.session == session).count() >= MAX_CLIENTS {
            return None;
        }
        let id = self.next_client;
        self.next_client = self.next_client.wrapping_add(1).max(1);
        self.clients.insert(
            id,
            Client {
                session,
                app,
                title: clean_title(title),
                flags: flags & (flags::SYSTEM | flags::DESKTOP | flags::TRUSTED),
                launched,
                windows: 0,
                next_window: 1,
                ping: None,
                last_ping: 0,
                not_responding: false,
                presses: VecDeque::new(),
            },
        );
        Some(id)
    }

    /// A client's capability has gone (the app ended, or the sandbox manager took it back):
    /// its windows go.
    pub fn remove_client(&mut self, client: ClientId, now: u64) {
        let mine: Vec<WindowId> =
            self.windows.keys().filter(|w| w.client == client).copied().collect();
        for w in mine {
            self.remove_window(w, now);
        }
        if let Some(c) = self.clients.remove(&client) {
            self.tell_desktop(c.session, Event::new(kind::WINDOWS, 0, [0; 3], now));
        }
        self.keys_to.retain(|(_, w)| w.client != client);
        self.changed();
    }

    /// The client's flags, if it exists.
    pub fn client_flags(&self, client: ClientId) -> Option<u32> {
        self.clients.get(&client).map(|c| c.flags)
    }

    /// Sends an event to the session's desktop client, if it has one.
    pub(crate) fn tell_desktop(&mut self, session: u32, e: Event) {
        let desktop = self
            .clients
            .iter()
            .find(|(_, c)| c.session == session && c.flags & flags::DESKTOP != 0)
            .map(|(id, _)| *id);
        if let Some(d) = desktop {
            self.send(d, e);
        }
    }

    /// Sends the session's desktop client the start menu key (note 0022, section 4), with a
    /// serial recorded as a press on its panel, so that it may open the start menu as a popup
    /// of the panel (a popup needs a press's serial, and the desktop's own windows never have
    /// the keyboard).
    pub(crate) fn tell_desktop_start_menu(&mut self, session: u32, now: u64) {
        let Some(d) = self
            .clients
            .iter()
            .find(|(_, c)| c.session == session && c.flags & flags::DESKTOP != 0)
            .map(|(id, _)| *id)
        else {
            return;
        };
        let panel = self
            .windows
            .values()
            .filter(|w| w.id.client == d && w.kind == WindowKind::Panel)
            .map(|w| w.id)
            .min_by_key(|id| id.number);
        let serial = match panel {
            Some(p) => {
                let serial = self.serial();
                self.record_press(p, serial, now);
                serial
            }
            None => 0,
        };
        self.send(d, Event::new(kind::START_MENU, 0, [0; 3], now).with_serial(serial));
    }

    fn desktop_only(&self, client: ClientId) -> Answer<&Client> {
        let c = self.clients.get(&client).ok_or(WindowError::NotAllowed)?;
        if c.flags & flags::DESKTOP == 0 {
            return Err(WindowError::NotAllowed);
        }
        Ok(c)
    }

    fn system_only(&self, client: ClientId) -> Answer<&Client> {
        let c = self.clients.get(&client).ok_or(WindowError::NotAllowed)?;
        if c.flags & flags::SYSTEM == 0 {
            return Err(WindowError::NotAllowed);
        }
        Ok(c)
    }

    /// A window of this client's, by its number.
    fn own(&self, client: ClientId, number: u32) -> Answer<&Window> {
        self.windows.get(&WindowId { client, number }).ok_or(WindowError::NoSuchWindow)
    }

    // ---- Serials ----------------------------------------------------------------------

    /// Records a press delivered to a client.
    pub(crate) fn record_press(&mut self, id: WindowId, serial: u32, now: u64) {
        if let Some(c) = self.clients.get_mut(&id.client) {
            if c.presses.len() >= PRESSES_KEPT {
                c.presses.pop_front();
            }
            c.presses.push_back(Press { serial, window: id.number, time: now });
        }
    }

    /// Whether `serial` is a press delivered to `client` (to `window`, if given) in the last
    /// second.
    pub(crate) fn recent_press(
        &self,
        client: ClientId,
        window: Option<u32>,
        serial: u32,
        now: u64,
    ) -> bool {
        serial != 0
            && self.clients.get(&client).is_some_and(|c| {
                c.presses.iter().any(|p| {
                    p.serial == serial
                        && window.is_none_or(|w| w == p.window)
                        && now.saturating_sub(p.time) <= SERIAL_LIFE
                })
            })
    }

    // ---- Windows ----------------------------------------------------------------------

    /// `create`: a new window for `client`.
    #[allow(clippy::too_many_arguments)] // the request's own parameters
    pub fn create(
        &mut self,
        client: ClientId,
        kind: WindowKind,
        parent: u32,
        title: &str,
        size: (u32, u32),
        flags: WindowFlags,
        at: (i32, i32),
        serial: u32,
        now: u64,
    ) -> Answer<u32> {
        let c = self.clients.get(&client).ok_or(WindowError::NotAllowed)?;
        let session = c.session;
        let cflags = c.flags;
        // Who may make what (note 0022, section 3).
        match kind {
            WindowKind::System if cflags & flags::SYSTEM == 0 => {
                return Err(WindowError::NotAllowed);
            }
            WindowKind::Background | WindowKind::Panel if cflags & flags::DESKTOP == 0 => {
                return Err(WindowError::NotAllowed);
            }
            _ => {}
        }
        if c.windows >= MAX_WINDOWS {
            return Err(WindowError::TooMany);
        }
        let (w, h) = (size.0.min(MAX_SIDE as u32) as i32, size.1.min(MAX_SIDE as u32) as i32);
        if size.0 == 0 || size.1 == 0 || size.0 > MAX_SIDE as u32 || size.1 > MAX_SIDE as u32 {
            return Err(WindowError::BadArgument);
        }
        if flags.contains(WindowFlags::MODAL) && kind != WindowKind::Dialog {
            return Err(WindowError::BadArgument);
        }
        // The parent: one of this client's own windows, for dialogs and popups only.
        let parent = match (kind, parent) {
            (WindowKind::Dialog | WindowKind::Popup, 0) => return Err(WindowError::BadKind),
            (WindowKind::Dialog | WindowKind::Popup, p) => {
                let pw = self.own(client, p)?;
                if pw.kind == WindowKind::Background || pw.kind == WindowKind::Panel {
                    // A desktop's start menu is a popup of its panel; a dialog of a panel
                    // makes no sense.
                    if kind == WindowKind::Dialog {
                        return Err(WindowError::BadKind);
                    }
                }
                Some(pw.id)
            }
            (_, 0) => None,
            _ => return Err(WindowError::BadArgument),
        };
        let mut window = Window {
            id: WindowId { client, number: 0 },
            kind,
            parent,
            flags,
            title: clean_title(title),
            place: (0, 0),
            size: (w, h),
            scale: 100,
            min: (0, 0),
            max: (0, 0),
            cursor: Cursor::Arrow,
            restore: None,
            minimised: false,
            dismissed: false,
            attention: false,
            over: None,
            screen: 0,
            edge: 0,
            thickness: 0,
            configured: 0,
            earlier: [0; 8],
            latest: None,
            held: [false; 2],
            shown: None,
            wants_frame: false,
            resized_at: 0,
            opened: 0,
        };
        if window.decorated() {
            window.size.0 = window.size.0.max(decor::MIN_WIDTH);
        }
        match kind {
            WindowKind::Normal | WindowKind::System => {
                let k = self.session(session).map_or(0, |s| s.opened);
                window.place = self.opening_place(window.size, kind, k);
            }
            WindowKind::Dialog => {
                let p = parent.and_then(|p| self.windows.get(&p)).ok_or(WindowError::BadKind)?;
                window.place = centred(p.content(), window.size);
            }
            WindowKind::Popup => {
                let p = parent.and_then(|p| self.windows.get(&p)).ok_or(WindowError::BadKind)?;
                let pid = p.id;
                // Only in answer to the user: a press delivered to the parent (or, for a menu
                // of a menu, to the window the first menu belongs to) in the last second.
                let root = self.root_of(pid);
                if !self.recent_press(client, Some(pid.number), serial, now)
                    && !self.recent_press(client, Some(root.number), serial, now)
                {
                    return Err(WindowError::BadSerial);
                }
                let p = self.windows.get(&pid).ok_or(WindowError::BadKind)?;
                let pc = p.content();
                let place = (pc.x.saturating_add(at.0), pc.y.saturating_add(at.1));
                let r = Rect::new(place.0, place.1, w, h);
                // Touching or inside its parent.
                if p.outline().inset(-1).intersect(&r).is_empty() {
                    return Err(WindowError::BadArgument);
                }
                // At most half its screen each way, except the desktop's start menu (a popup of
                // its panel), which may be as tall as the screen: on a small or 2x screen half
                // would cut it short (note 0024, section 6).
                let screen = self.space.holding_most(r).and_then(|n| self.space.screen(n));
                let (sw, sh) = screen.map_or(NO_SCREEN, Screen::logical_size);
                let of_panel = p.kind == WindowKind::Panel;
                let (most_w, most_h) = if of_panel { (sw, sh) } else { (sw / 2, sh / 2) };
                if w > most_w || h > most_h {
                    return Err(WindowError::BadArgument);
                }
                window.place = place;
            }
            WindowKind::Background | WindowKind::Panel => {
                let screen = u32::try_from(at.0).map_err(|_| WindowError::BadArgument)?;
                let s = self
                    .space
                    .screen(screen)
                    .filter(|s| s.in_space())
                    .ok_or(WindowError::BadArgument)?;
                let (sw, sh) = s.logical_size();
                // One of each per screen per session (and one panel per edge).
                let taken = self.windows.values().any(|o| {
                    o.kind == kind
                        && o.screen == screen
                        && self.clients.get(&o.id.client).is_some_and(|c| c.session == session)
                        && (kind == WindowKind::Background || o.edge == at.1 as u32)
                });
                if taken {
                    return Err(WindowError::TooMany);
                }
                window.screen = screen;
                if kind == WindowKind::Panel {
                    let edge = u32::try_from(at.1).map_err(|_| WindowError::BadArgument)?;
                    let thickness = match edge {
                        1 | 2 => h,
                        3 | 4 => w,
                        _ => return Err(WindowError::BadArgument),
                    };
                    if thickness > (if edge <= 2 { sh } else { sw }) / 4 {
                        return Err(WindowError::BadArgument);
                    }
                    window.edge = edge;
                    window.thickness = thickness;
                }
            }
        }
        // Everything is checked: it exists from here on.
        let c = self.clients.get_mut(&client).ok_or(WindowError::NotAllowed)?;
        let number = c.next_window;
        c.next_window = c.next_window.wrapping_add(1).max(1);
        c.windows += 1;
        let launched = c.launched;
        window.id.number = number;
        self.opened += 1;
        window.opened = self.opened;
        let id = window.id;
        self.windows.insert(id, window);
        self.fit_fixed(id, now);
        if let Some(s) = self.session_mut(session) {
            s.stack.push(id);
            if matches!(kind, WindowKind::Normal | WindowKind::System) {
                s.opened = s.opened.wrapping_add(1);
            }
        }
        self.sort_stack(session);
        self.check_scale_quietly(id);
        // The focus (note 0022, section 4): only if the client has it already, or the user
        // launched it just now. Otherwise the taskbar shows it wants attention.
        let has_focus = self.focused_client(session) == Some(client);
        let fresh = launched != 0 && now.saturating_sub(launched) <= LAUNCH_FOCUS;
        let takes = match kind {
            WindowKind::Normal | WindowKind::Dialog | WindowKind::System => has_focus || fresh,
            _ => false,
        };
        let focusable = self.windows.get(&id).is_some_and(Window::focusable);
        if takes {
            self.focus(id, now);
        } else if focusable && let Some(w) = self.windows.get_mut(&id) {
            w.attention = true;
        }
        self.configure(id, now);
        if let Some(b) = self.windows.get(&id).map(Window::bounds) {
            self.damage(b);
        }
        self.tell_desktop(session, Event::new(kind::WINDOWS, 0, [0; 3], now));
        self.changed();
        Ok(number)
    }

    /// Where a new normal or system window opens: cascading down and right across the main
    /// screen's work area, each a little further than the last.
    fn opening_place(&self, size: (i32, i32), kind: WindowKind, k: u32) -> (i32, i32) {
        let main = self.space.main().map(|s| s.number);
        let area =
            main.and_then(|n| self.work_area(n)).or_else(|| self.space.main().map(Screen::rect));
        let area = area.unwrap_or(Rect::new(0, 0, NO_SCREEN.0, NO_SCREEN.1));
        let (fw, fh) = (size.0 + 2 * decor::BORDER, size.1 + 2 * decor::BORDER + decor::TITLE);
        if kind == WindowKind::System {
            let c = centred(area, (fw, fh));
            return (c.0 + decor::BORDER, c.1 + decor::BORDER + decor::TITLE);
        }
        let step = (k % 8) as i32 * CASCADE;
        let x = area.x + 32 + step;
        let y = area.y + 24 + step;
        // Kept on the work area if it fits; at its corner if it doesn't.
        let x = x.min(area.x + (area.width - fw).max(0));
        let y = y.min(area.y + (area.height - fh).max(0));
        (x + decor::BORDER, y + decor::BORDER + decor::TITLE)
    }

    /// `create_over`: a system window over another app's window, blocking input to it.
    #[allow(clippy::too_many_arguments)]
    pub fn create_over(
        &mut self,
        client: ClientId,
        app: u32,
        window: u32,
        title: &str,
        size: (u32, u32),
        flags: WindowFlags,
        now: u64,
    ) -> Answer<u32> {
        let session = self.system_only(client)?.session;
        let target = self.app_window(session, app, window)?;
        if flags.contains(WindowFlags::MODAL) {
            return Err(WindowError::BadArgument);
        }
        let had_focus = self.session(session).and_then(|s| s.focus) == Some(target)
            || self
                .session(session)
                .and_then(|s| s.focus)
                .is_some_and(|f| self.windows.get(&f).is_some_and(|w| w.over == Some(target)));
        let number =
            self.create(client, WindowKind::System, 0, title, size, flags, (0, 0), 0, now)?;
        let id = WindowId { client, number };
        let tc = self.windows.get(&target).map(Window::content);
        if let (Some(tc), Some(w)) = (tc, self.windows.get_mut(&id)) {
            w.over = Some(target);
            let old = w.bounds();
            w.place = centred(tc, w.size);
            let new = w.bounds();
            self.damage(old);
            self.damage(new);
        }
        self.check_scale(id, now);
        if had_focus {
            self.focus(id, now);
        }
        Ok(number)
    }

    /// The window `window` of the app the sandbox manager numbered `app`, in a session.
    fn app_window(&self, session: u32, app: u32, window: u32) -> Answer<WindowId> {
        self.clients
            .iter()
            .filter(|(_, c)| c.session == session && c.app == app)
            .map(|(id, _)| WindowId { client: *id, number: window })
            .find(|w| self.windows.contains_key(w))
            .ok_or(WindowError::NoSuchWindow)
    }

    /// `check_serial`, for system clients: whether a serial is a press delivered to another
    /// app's window in the last second.
    pub fn check_serial(
        &self,
        client: ClientId,
        app: u32,
        window: u32,
        serial: u32,
        now: u64,
    ) -> Answer<bool> {
        let session = self.system_only(client)?.session;
        let target = self.app_window(session, app, window)?;
        Ok(self.recent_press(target.client, Some(target.number), serial, now))
    }

    /// `destroy`.
    pub fn destroy(&mut self, client: ClientId, number: u32, now: u64) -> Answer<()> {
        let id = self.own(client, number)?.id;
        self.remove_window(id, now);
        Ok(())
    }

    /// Removes a window and everything that belongs to it (its dialogs and popups, and system
    /// windows over it), letting go of its buffers.
    pub(crate) fn remove_window(&mut self, id: WindowId, now: u64) {
        let children: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.parent == Some(id) || w.over == Some(id))
            .map(|w| w.id)
            .collect();
        for c in children {
            if c.client == id.client {
                self.remove_window(c, now);
            } else if let Some(w) = self.windows.get_mut(&c) {
                // Another client's system window over it: it blocks nothing now; its client
                // is told to close it.
                w.over = None;
                self.send(c.client, Event::new(kind::CLOSE, c.number, [0; 3], now));
            }
        }
        let Some(w) = self.windows.remove(&id) else { return };
        self.damage(w.bounds());
        for g in [w.latest.map(|b| b.generation), w.shown.map(|s| s.buffers.generation)]
            .into_iter()
            .flatten()
            .collect::<alloc::collections::BTreeSet<u32>>()
        {
            self.out.push(Out::Free { window: id, generation: g });
        }
        let session = self.clients.get(&id.client).map(|c| c.session);
        if let Some(c) = self.clients.get_mut(&id.client) {
            c.windows = c.windows.saturating_sub(1);
        }
        if self.menu.as_ref().is_some_and(|m| m.window == id) {
            self.menu = None;
        }
        if let Some(s) = session {
            let was_focus = self.session(s).and_then(|s| s.focus) == Some(id);
            if let Some(sess) = self.session_mut(s) {
                sess.stack.retain(|w| *w != id);
            }
            if was_focus {
                if let Some(sess) = self.session_mut(s) {
                    sess.focus = None;
                }
                self.focus_next(s, now);
            }
            if w.kind == WindowKind::Panel {
                // The work area grew back: maximised windows fill it again.
                let maxed: Vec<WindowId> =
                    self.windows.values().filter(|w| w.restore.is_some()).map(|w| w.id).collect();
                for m in maxed {
                    self.fill_work_area(m, now);
                }
            }
            self.tell_desktop(s, Event::new(kind::WINDOWS, 0, [0; 3], now));
        }
        if self.pointer.hover == Some(id) {
            self.pointer.hover = None;
        }
        if grab_window(self.pointer.grab) == Some(id) {
            self.pointer.grab = Grab::Nothing;
        }
        self.keys_to.retain(|(_, w)| *w != id);
        if let Some((list, _)) = &mut self.switching {
            list.retain(|w| *w != id);
        }
        self.changed();
    }

    /// `set_title`.
    pub fn set_title(&mut self, client: ClientId, number: u32, title: &str) -> Answer<()> {
        let id = self.own(client, number)?.id;
        let title = clean_title(title);
        let Some(w) = self.windows.get_mut(&id) else { return Err(WindowError::NoSuchWindow) };
        if w.title != title {
            w.title = title;
            let r = decor::title_bar(w.content());
            let decorated = w.decorated();
            if decorated {
                self.damage(r);
            }
            let session = self.clients.get(&client).map(|c| c.session);
            if let Some(s) = session {
                self.tell_desktop(s, Event::new(kind::WINDOWS, 0, [0; 3], 0));
            }
            self.changed();
        }
        Ok(())
    }

    /// `set_limits`: 0 means no limit.
    pub fn set_limits(
        &mut self,
        client: ClientId,
        number: u32,
        min: (u32, u32),
        max: (u32, u32),
    ) -> Answer<()> {
        let id = self.own(client, number)?.id;
        let side = |v: u32| v.min(MAX_SIDE as u32) as i32;
        let (min, max) = ((side(min.0), side(min.1)), (side(max.0), side(max.1)));
        if (max.0 != 0 && max.0 < min.0) || (max.1 != 0 && max.1 < min.1) {
            return Err(WindowError::BadArgument);
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.min = min;
            w.max = max;
        }
        Ok(())
    }

    /// `set_cursor`.
    pub fn set_cursor(&mut self, client: ClientId, number: u32, cursor: Cursor) -> Answer<()> {
        let id = self.own(client, number)?.id;
        if let Some(w) = self.windows.get_mut(&id) {
            w.cursor = cursor;
        }
        if self.pointer.hover == Some(id) {
            self.damage_pointer();
        }
        Ok(())
    }

    // ---- Configure, buffers, commits and frames ---------------------------------------

    /// The state bits a configure event carries for a window.
    fn states(&self, w: &Window) -> u32 {
        let session = self.clients.get(&w.id.client).map(|c| c.session);
        let focused = session.and_then(|s| self.session(s)).and_then(|s| s.focus) == Some(w.id);
        let mut st = 0;
        if focused {
            st |= state::FOCUSED;
        }
        if w.restore.is_some() {
            st |= state::MAXIMISED;
        }
        if w.minimised {
            st |= state::MINIMISED;
        }
        if self.visible(w.id) && self.space.holding_most(w.content()).is_some() {
            st |= state::SHOWN;
        }
        st
    }

    /// Sends a window a configure event with its size, scale and state, under a new serial.
    pub(crate) fn configure(&mut self, id: WindowId, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let st = self.states(w);
        let (width, height, scale) = (w.size.0 as u32, w.size.1 as u32, w.scale);
        let serial = self.serial();
        if let Some(w) = self.windows.get_mut(&id) {
            w.earlier.rotate_right(1);
            w.earlier[0] = w.configured;
            w.configured = serial;
        }
        let e = Event::new(
            kind::CONFIGURE,
            id.number,
            [width, height, (scale & 0xFFFF) | (st << 16)],
            now,
        )
        .with_serial(serial);
        self.send(id.client, e);
    }

    /// The bytes of buffers a session's windows hold now.
    fn session_bytes(&self, session: u32) -> u64 {
        self.windows
            .values()
            .filter(|w| self.clients.get(&w.id.client).is_some_and(|c| c.session == session))
            .map(|w| {
                let latest = w.latest.map_or(0, |b| b.bytes());
                let shown = match (w.shown, w.latest) {
                    (Some(s), Some(l)) if s.buffers.generation == l.generation => 0,
                    (Some(s), _) => s.buffers.bytes(),
                    _ => 0,
                };
                latest + shown
            })
            .sum()
    }

    /// `buffers`: two frames for the configure event `serial`'s size and scale. The display
    /// server makes the memory (or copies the existing one again, if `reused`).
    pub fn buffers(&mut self, client: ClientId, number: u32, serial: u32) -> Answer<Made> {
        let w = self.own(client, number)?;
        if serial != w.configured {
            // An older configure of this window's has been overtaken; anything else is no
            // serial it was given for this.
            return Err(if serial != 0 && w.earlier.contains(&serial) {
                WindowError::Stale
            } else {
                WindowError::BadSerial
            });
        }
        let factor = (w.scale / 100).max(1) as i32;
        let (pw, ph) = (w.size.0.saturating_mul(factor), w.size.1.saturating_mul(factor));
        let (lw, lh) = match self.space.largest_device() {
            (0, _) | (_, 0) => NO_SCREEN,
            l => l,
        };
        if pw > lw || ph > lh {
            return Err(WindowError::NoMemory);
        }
        let logical = w.size;
        let scale = w.scale;
        if let Some(l) = w.latest
            && l.logical == logical
            && l.scale == scale
        {
            return Ok(Made { buffers: l, reused: true });
        }
        let stride = (pw as u32) * 4;
        let generation =
            w.latest.map_or(0, |b| b.generation).max(w.shown.map_or(0, |s| s.buffers.generation))
                + 1;
        let new =
            Buffers { generation, width: pw as u32, height: ph as u32, stride, scale, logical };
        // The newest buffers that were never shown are dropped now; the shown ones stay on
        // the screen until the new ones are committed.
        let dropping = match (w.latest, w.shown) {
            (Some(l), Some(s)) if s.buffers.generation == l.generation => None,
            (Some(l), _) => Some(l),
            _ => None,
        };
        let session = self.clients.get(&client).map_or(0, |c| c.session);
        let after = self.session_bytes(session) - dropping.map_or(0, |d| d.bytes()) + new.bytes();
        if after > SESSION_BUFFERS {
            return Err(WindowError::NoMemory);
        }
        let id = WindowId { client, number };
        if let Some(d) = dropping {
            self.out.push(Out::Free { window: id, generation: d.generation });
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.latest = Some(new);
            w.held = [true, true];
        }
        Ok(Made { buffers: new, reused: false })
    }

    /// The display server couldn't make the memory `buffers` asked for: forget them.
    pub fn buffers_failed(&mut self, client: ClientId, number: u32, generation: u32) {
        let id = WindowId { client, number };
        if let Some(w) = self.windows.get_mut(&id)
            && w.latest.is_some_and(|l| l.generation == generation)
        {
            w.latest = None;
            w.held = [false; 2];
        }
    }

    /// `commit`: frame `frame` of the newest buffers, with the rectangles that changed.
    pub fn commit(
        &mut self,
        client: ClientId,
        number: u32,
        frame: u32,
        changed: &[(i32, i32, u32, u32)],
        now: u64,
    ) -> Answer<()> {
        let w = self.own(client, number)?;
        let id = w.id;
        let latest = w.latest.ok_or(WindowError::Stale)?;
        if frame > 1 {
            return Err(WindowError::BadArgument);
        }
        if !w.held[frame as usize] {
            return Err(WindowError::Stale);
        }
        let old = w.shown;
        let content = w.content();
        let first = old.is_none_or(|o| o.buffers.generation != latest.generation);
        // The frame before goes back to the app, or (from older buffers) is let go of.
        match old {
            Some(o) if o.buffers.generation == latest.generation => {
                self.send(client, Event::new(kind::RELEASED, number, [o.frame, 0, 0], now));
            }
            Some(o) => self.out.push(Out::Free { window: id, generation: o.buffers.generation }),
            None => {}
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.held[frame as usize] = false;
            if let Some(o) = old
                && o.buffers.generation == latest.generation
            {
                w.held[o.frame as usize] = true;
            }
            w.shown = Some(Shown { buffers: latest, frame });
        }
        // What to draw again: everything, the first time these buffers are shown (or when
        // the app said nothing), otherwise the rectangles it named, inside its content.
        if first || changed.is_empty() {
            self.damage(content);
        } else {
            for &(x, y, cw, ch) in changed {
                let r = Rect::new(
                    content.x.saturating_add(x),
                    content.y.saturating_add(y),
                    cw.min(MAX_SIDE as u32) as i32,
                    ch.min(MAX_SIDE as u32) as i32,
                )
                .intersect(&content);
                self.damage(r);
            }
        }
        if first {
            self.changed();
        }
        Ok(())
    }

    /// `frame`: the app wants a frame event.
    pub fn want_frame(&mut self, client: ClientId, number: u32) -> Answer<()> {
        let id = self.own(client, number)?.id;
        if let Some(w) = self.windows.get_mut(&id) {
            w.wants_frame = true;
        }
        Ok(())
    }

    /// Whether any window is waiting for a frame event.
    pub fn frames_wanted(&self) -> bool {
        self.windows.values().any(|w| w.wants_frame)
    }

    /// Sends frame events to the windows that asked and can be seen, now that what was
    /// committed is on the screens. `busy` says whether a part of the space is still waiting
    /// to be shown: windows there wait too.
    pub fn send_frames(&mut self, now: u64, busy: &dyn Fn(Rect) -> bool) {
        let ids: Vec<WindowId> =
            self.windows.values().filter(|w| w.wants_frame).map(|w| w.id).collect();
        for id in ids {
            let Some(w) = self.windows.get(&id) else { continue };
            let bounds = w.bounds();
            if !self.visible(id) || self.covered(id) || busy(bounds) {
                continue;
            }
            if let Some(w) = self.windows.get_mut(&id) {
                w.wants_frame = false;
            }
            self.send(id.client, Event::new(kind::FRAME, id.number, [0; 3], now));
        }
    }

    /// Whether a window is shown at all: in the active session, not minimised or dismissed,
    /// and nor is what it belongs to.
    pub(crate) fn visible(&self, id: WindowId) -> bool {
        let Some(w) = self.windows.get(&id) else { return false };
        let in_active =
            self.clients.get(&id.client).is_some_and(|c| Some(c.session) == self.active);
        if !in_active || w.minimised || w.dismissed {
            return false;
        }
        if matches!(w.kind, WindowKind::Background | WindowKind::Panel)
            && !self.space.screen(w.screen).is_some_and(|s| s.in_space())
        {
            return false;
        }
        match w.parent {
            Some(p) => self.visible(p),
            None => true,
        }
    }

    /// Whether a window is hidden entirely behind the windows above it.
    pub(crate) fn covered(&self, id: WindowId) -> bool {
        let Some(w) = self.windows.get(&id) else { return true };
        let Some(session) = self.clients.get(&id.client).and_then(|c| self.session(c.session))
        else {
            return true;
        };
        let Some(at) = session.stack.iter().position(|s| *s == id) else { return true };
        let mut left: Vec<Rect> = vec![w.content()];
        for above in &session.stack[at + 1..] {
            if !self.visible(*above) {
                continue;
            }
            let Some(a) = self.windows.get(above) else { continue };
            let cover = a.outline();
            let mut next = Vec::new();
            for r in &left {
                subtract(*r, cover, &mut next);
            }
            left = next;
            // Too many pieces to follow: say it can be seen.
            if left.len() > 64 {
                return false;
            }
            if left.is_empty() {
                return true;
            }
        }
        false
    }

    // ---- Moving, sizing, scales -------------------------------------------------------

    /// Moves a window's content corner to `place`, with what belongs to it.
    pub(crate) fn set_place(&mut self, id: WindowId, place: (i32, i32), now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let (dx, dy) = (place.0.saturating_sub(w.place.0), place.1.saturating_sub(w.place.1));
        if dx == 0 && dy == 0 {
            return;
        }
        let old = w.bounds();
        if let Some(w) = self.windows.get_mut(&id) {
            w.place = place;
        }
        self.damage(old);
        if let Some(b) = self.windows.get(&id).map(Window::bounds) {
            self.damage(b);
        }
        // Dialogs and popups keep their place on it; system windows over it too.
        let followers: Vec<(WindowId, (i32, i32))> = self
            .windows
            .values()
            .filter(|o| o.parent == Some(id) || o.over == Some(id))
            .map(|o| (o.id, (o.place.0.saturating_add(dx), o.place.1.saturating_add(dy))))
            .collect();
        for (f, p) in followers {
            self.set_place(f, p, now);
        }
        self.check_scale(id, now);
        self.changed();
    }

    /// Changes a window's size (the user resized or maximised it, or its screen changed),
    /// within its limits, and tells it.
    pub(crate) fn set_size(&mut self, id: WindowId, size: (i32, i32), now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let size = self.limit_size(w, size);
        if size == w.size {
            return;
        }
        let old = w.bounds();
        if let Some(w) = self.windows.get_mut(&id) {
            w.size = size;
        }
        self.damage(old);
        if let Some(b) = self.windows.get(&id).map(Window::bounds) {
            self.damage(b);
        }
        self.configure(id, now);
        self.changed();
    }

    /// A size within a window's limits (and at least 1, and the narrowest a title bar fits).
    pub(crate) fn limit_size(&self, w: &Window, size: (i32, i32)) -> (i32, i32) {
        let low =
            (w.min.0.max(1).max(if w.decorated() { decor::MIN_WIDTH } else { 1 }), w.min.1.max(1));
        let high = (
            if w.max.0 > 0 { w.max.0 } else { MAX_SIDE },
            if w.max.1 > 0 { w.max.1 } else { MAX_SIDE },
        );
        if matches!(w.kind, WindowKind::Background | WindowKind::Panel) {
            return (size.0.clamp(1, MAX_SIDE), size.1.clamp(1, MAX_SIDE));
        }
        (size.0.clamp(low.0, high.0.max(low.0)), size.1.clamp(low.1, high.1.max(low.1)))
    }

    /// The scale of the screen holding most of the window, which is the scale the window
    /// should draw at. Popups and dialogs go by their own place too.
    fn wanted_scale(&self, id: WindowId) -> Option<u32> {
        let w = self.windows.get(&id)?;
        let n = match w.kind {
            WindowKind::Background | WindowKind::Panel => w.screen,
            _ => self.space.holding_most(w.content())?,
        };
        self.space.screen(n).map(|s| s.place.scale)
    }

    /// Checks a window's scale against its screen's, telling it if it changed.
    pub(crate) fn check_scale(&mut self, id: WindowId, now: u64) {
        let Some(want) = self.wanted_scale(id) else { return };
        let Some(w) = self.windows.get_mut(&id) else { return };
        if w.scale != want {
            w.scale = want;
            self.configure(id, now);
            self.changed();
        }
    }

    /// The same, for a window not yet configured at all.
    fn check_scale_quietly(&mut self, id: WindowId) {
        if let Some(want) = self.wanted_scale(id)
            && let Some(w) = self.windows.get_mut(&id)
        {
            w.scale = want;
        }
    }

    /// Maximises or restores a window.
    pub(crate) fn set_maximised(&mut self, id: WindowId, on: bool, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        if !matches!(w.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System) {
            return;
        }
        match (on, w.restore) {
            (true, None) => {
                let saved = (w.place, w.size);
                if let Some(w) = self.windows.get_mut(&id) {
                    w.restore = Some(saved);
                }
                self.fill_work_area(id, now);
                // A configure even if the size happened to be the same: the state changed.
                self.configure(id, now);
            }
            (false, Some((place, size))) => {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.restore = None;
                }
                self.set_place(id, place, now);
                self.set_size(id, size, now);
                self.configure(id, now);
            }
            _ => {}
        }
        self.changed();
    }

    /// A maximised window takes the whole work area of the screen holding most of it.
    fn fill_work_area(&mut self, id: WindowId, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let Some(n) = self.space.holding_most(w.content()) else { return };
        let Some(area) = self.work_area(n) else { return };
        let place = (area.x + decor::BORDER, area.y + decor::BORDER + decor::TITLE);
        let size = (area.width - 2 * decor::BORDER, area.height - 2 * decor::BORDER - decor::TITLE);
        self.set_place(id, place, now);
        self.set_size(id, size, now);
    }

    /// Minimises a window (and with it its dialogs and popups); the focus moves on.
    pub(crate) fn set_minimised(&mut self, id: WindowId, on: bool, now: u64) {
        let Some(w) = self.windows.get_mut(&id) else { return };
        if !matches!(w.kind, WindowKind::Normal | WindowKind::System) || w.minimised == on {
            return;
        }
        w.minimised = on;
        let b = w.bounds();
        self.damage(b);
        let session = self.clients.get(&id.client).map(|c| c.session);
        if on {
            if let Some(s) = session {
                let focus = self.session(s).and_then(|s| s.focus);
                if focus.is_some_and(|f| f == id || self.belongs_to(f, id)) {
                    if let Some(sess) = self.session_mut(s) {
                        sess.focus = None;
                    }
                    self.configure(id, now);
                    self.focus_next(s, now);
                } else {
                    self.configure(id, now);
                }
            }
            self.dismiss_popups_of(id, now);
        } else {
            self.configure(id, now);
        }
        if let Some(s) = session {
            self.tell_desktop(s, Event::new(kind::WINDOWS, 0, [0; 3], now));
        }
        self.changed();
    }

    /// Whether `w` belongs to `owner`: one of its dialogs or popups, at any depth.
    pub(crate) fn belongs_to(&self, w: WindowId, owner: WindowId) -> bool {
        let mut at = self.windows.get(&w).and_then(|w| w.parent);
        let mut depth = 0;
        while let Some(p) = at {
            if p == owner {
                return true;
            }
            depth += 1;
            if depth > MAX_WINDOWS {
                return false;
            }
            at = self.windows.get(&p).and_then(|w| w.parent);
        }
        false
    }

    /// The window a dialog or popup belongs to in the end.
    pub(crate) fn root_of(&self, id: WindowId) -> WindowId {
        let mut at = id;
        for _ in 0..=MAX_WINDOWS {
            match self.windows.get(&at).and_then(|w| w.parent) {
                Some(p) => at = p,
                None => break,
            }
        }
        at
    }

    // ---- Stacking and the focus -------------------------------------------------------

    /// Puts a session's windows in layer order, keeping their order within each layer.
    fn sort_stack(&mut self, session: u32) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.number == session) else { return };
        let windows = &self.windows;
        // `sort_by_key` is stable: windows of a layer keep their order.
        s.stack.sort_by_key(|id| windows.get(id).map_or(0, Window::layer));
    }

    /// Raises a window to the top of its layer, with its dialogs and popups above it.
    pub(crate) fn raise(&mut self, id: WindowId) {
        let Some(session) = self.clients.get(&id.client).map(|c| c.session) else { return };
        let mut group: Vec<WindowId> = vec![id];
        // Its dialogs and popups (and theirs), in the order they were in.
        if let Some(s) = self.session(session) {
            for w in &s.stack {
                if *w != id && self.belongs_to(*w, id) {
                    group.push(*w);
                }
            }
        }
        let Some(s) = self.session_mut(session) else { return };
        let before = s.stack.clone();
        s.stack.retain(|w| !group.contains(w));
        s.stack.extend(group.iter().copied());
        let after = s.stack.clone();
        self.sort_stack(session);
        if self.session(session).is_some_and(|s| s.stack != before) || after != before {
            for g in group {
                if let Some(b) = self.windows.get(&g).map(Window::bounds) {
                    self.damage(b);
                }
            }
            self.changed();
        }
    }

    /// The client whose window has the focus in a session.
    pub(crate) fn focused_client(&self, session: u32) -> Option<ClientId> {
        self.session(session).and_then(|s| s.focus).map(|f| f.client)
    }

    /// What really gets the focus when the user chooses `id`: a modal dialog of it, or a
    /// system window over it, if one is open (and, for those, theirs).
    pub(crate) fn blocker(&self, id: WindowId) -> WindowId {
        let mut at = id;
        for _ in 0..=MAX_WINDOWS {
            let next = self.windows.values().find(|w| {
                !w.minimised
                    && !w.dismissed
                    && ((w.parent == Some(at) && w.flags.contains(WindowFlags::MODAL))
                        || w.over == Some(at))
            });
            match next {
                Some(n) => at = n.id,
                None => break,
            }
        }
        at
    }

    /// Gives a window the focus (or what blocks it), raising it.
    pub(crate) fn focus(&mut self, id: WindowId, now: u64) {
        let id = self.blocker(id);
        let Some(w) = self.windows.get(&id) else { return };
        if !w.focusable() {
            return;
        }
        let Some(session) = self.clients.get(&id.client).map(|c| c.session) else { return };
        if w.minimised {
            self.set_minimised(id, false, now);
        }
        // An owner's dialogs come up with it; a system window over another app's window
        // brings that one up first.
        if let Some(t) = self.windows.get(&id).and_then(|w| w.over) {
            self.raise(t);
        }
        self.raise(self.root_of(id));
        self.raise(id);
        let old = self.session(session).and_then(|s| s.focus);
        if old == Some(id) {
            return;
        }
        if let Some(s) = self.session_mut(session) {
            s.focus = Some(id);
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.attention = false;
        }
        if let Some(o) = old {
            // Popups close when what they belong to loses the focus.
            let root = self.root_of(o);
            if self.root_of(id) != root {
                self.dismiss_popups_of(root, now);
            }
            self.configure(o, now);
            if let Some(b) = self.windows.get(&o).map(|w| decor::title_bar(w.content())) {
                self.damage(b);
            }
        }
        self.configure(id, now);
        if let Some(b) = self.windows.get(&id).map(Window::bounds) {
            self.damage(b);
        }
        // The focused client hears what the clipboard holds, so it can grey out Paste.
        if old.map(|o| o.client) != Some(id.client) {
            let forms = self.session(session).map_or(0, |s| s.clipboard.forms());
            self.send(id.client, Event::new(kind::CLIPBOARD, 0, [forms, 0, 0], now));
            // And it is pinged at once, which is when a hung app is noticed first.
            self.ping(id.client, now);
        }
        self.tell_desktop(session, Event::new(kind::WINDOWS, 0, [0; 3], now));
        self.changed();
    }

    /// Gives the focus to the topmost window that can have it, after the focused one went.
    pub(crate) fn focus_next(&mut self, session: u32, now: u64) {
        let next =
            self.session(session).and_then(|s| {
                s.stack.iter().rev().copied().find(|id| {
                    self.windows.get(id).is_some_and(|w| w.focusable() && self.visible(*id))
                })
            });
        if let Some(n) = next {
            self.focus(n, now);
        } else {
            self.tell_desktop(session, Event::new(kind::WINDOWS, 0, [0; 3], now));
        }
    }

    /// Dismisses every popup belonging to `owner`: they are hidden, and their app is told.
    pub(crate) fn dismiss_popups_of(&mut self, owner: WindowId, now: u64) {
        let popups: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.kind == WindowKind::Popup && !w.dismissed && self.belongs_to(w.id, owner))
            .map(|w| w.id)
            .collect();
        for p in popups {
            self.dismiss(p, now);
        }
    }

    pub(crate) fn dismiss(&mut self, id: WindowId, now: u64) {
        if let Some(w) = self.windows.get_mut(&id)
            && !w.dismissed
        {
            w.dismissed = true;
            let b = w.bounds();
            self.damage(b);
            self.send(id.client, Event::new(kind::CLOSE, id.number, [0; 3], now));
            self.changed();
        }
    }

    // ---- Requests about a window's state ----------------------------------------------

    /// `minimise`.
    pub fn minimise(&mut self, client: ClientId, number: u32, now: u64) -> Answer<()> {
        let w = self.own(client, number)?;
        if !matches!(w.kind, WindowKind::Normal | WindowKind::System) {
            return Err(WindowError::BadKind);
        }
        self.set_minimised(w.id, true, now);
        Ok(())
    }

    /// `maximise`.
    pub fn maximise(&mut self, client: ClientId, number: u32, on: bool, now: u64) -> Answer<()> {
        let w = self.own(client, number)?;
        if !matches!(w.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System) {
            return Err(WindowError::BadKind);
        }
        self.set_maximised(w.id, on, now);
        Ok(())
    }

    /// `start_move`: the app's own title area was pressed (its serial says so).
    pub fn start_move(
        &mut self,
        client: ClientId,
        number: u32,
        serial: u32,
        now: u64,
    ) -> Answer<()> {
        let w = self.own(client, number)?;
        let id = w.id;
        if !matches!(w.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System) {
            return Err(WindowError::BadKind);
        }
        // Only while that press is still held: the move ends when the buttons are let go.
        if !self.recent_press(client, Some(number), serial, now)
            || self.pointer.grab != Grab::Pressed(id)
        {
            return Err(WindowError::BadSerial);
        }
        self.begin_move(id);
        Ok(())
    }

    /// `start_resize`.
    pub fn start_resize(
        &mut self,
        client: ClientId,
        number: u32,
        edge: u32,
        serial: u32,
        now: u64,
    ) -> Answer<()> {
        let w = self.own(client, number)?;
        let id = w.id;
        if !matches!(w.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System)
            || !w.flags.contains(WindowFlags::RESIZABLE)
        {
            return Err(WindowError::BadKind);
        }
        if !(1..=8).contains(&edge) {
            return Err(WindowError::BadArgument);
        }
        if !self.recent_press(client, Some(number), serial, now)
            || self.pointer.grab != Grab::Pressed(id)
        {
            return Err(WindowError::BadSerial);
        }
        let content = w.content();
        self.pointer.grab = Grab::Resize(id, edge, content, (self.pointer.x, self.pointer.y));
        self.damage_pointer();
        Ok(())
    }

    /// `pong`.
    pub fn pong(&mut self, client: ClientId, serial: u32, now: u64) -> Answer<()> {
        let c = self.clients.get_mut(&client).ok_or(WindowError::NotAllowed)?;
        match c.ping {
            Some((s, _)) if s == serial => {
                c.ping = None;
                c.last_ping = now;
                if c.not_responding {
                    c.not_responding = false;
                    let session = c.session;
                    self.damage_titles(client);
                    self.tell_desktop(session, Event::new(kind::WINDOWS, 0, [0; 3], now));
                    self.changed();
                }
                Ok(())
            }
            _ => Err(WindowError::BadSerial),
        }
    }

    /// Pings a client, unless a ping is already waiting for its answer.
    pub(crate) fn ping(&mut self, client: ClientId, now: u64) {
        let serial = self.serial();
        let Some(c) = self.clients.get_mut(&client) else { return };
        if c.ping.is_some() {
            return;
        }
        c.ping = Some((serial, now));
        c.last_ping = now;
        self.send(client, Event::new(kind::PING, 0, [0; 3], now).with_serial(serial));
    }

    fn damage_titles(&mut self, client: ClientId) {
        let bars: Vec<Rect> = self
            .windows
            .values()
            .filter(|w| w.id.client == client && w.decorated())
            .map(|w| decor::title_bar(w.content()))
            .collect();
        for b in bars {
            self.damage(b);
        }
    }

    /// What time brings: pings for the client with the focus, and the ones that didn't
    /// answer in time marked not responding. Returns when to call again, at the latest.
    pub fn tick(&mut self, now: u64) -> u64 {
        let mut next = u64::MAX;
        if let Some(active) = self.active
            && let Some(c) = self.focused_client(active)
        {
            let due = self.clients.get(&c).map_or(u64::MAX, |cl| {
                if cl.ping.is_some() { u64::MAX } else { cl.last_ping.saturating_add(PING_EVERY) }
            });
            if now >= due {
                self.ping(c, now);
            } else {
                next = next.min(due);
            }
        }
        let late: Vec<ClientId> = self
            .clients
            .iter()
            .filter(|(_, c)| {
                !c.not_responding && c.ping.is_some_and(|(_, t)| now.saturating_sub(t) >= PING_WAIT)
            })
            .map(|(id, _)| *id)
            .collect();
        for c in late {
            let session = self.clients.get_mut(&c).map(|cl| {
                cl.not_responding = true;
                cl.session
            });
            self.damage_titles(c);
            if let Some(s) = session {
                self.tell_desktop(s, Event::new(kind::WINDOWS, 0, [0; 3], now));
            }
            self.changed();
        }
        for c in self.clients.values() {
            if let (Some((_, t)), false) = (c.ping, c.not_responding) {
                next = next.min(t.saturating_add(PING_WAIT));
            }
        }
        next
    }

    // ---- The desktop's operations -----------------------------------------------------

    /// `screens` (desktop only).
    pub fn screens_for(&self, client: ClientId) -> Answer<Vec<(Screen, (i32, i32))>> {
        self.desktop_only(client)?;
        Ok(self.space.screens().iter().map(|s| (s.clone(), s.logical_size())).collect())
    }

    /// `windows` (desktop only): the session's normal windows, dialogs and system windows, in
    /// the order they were opened.
    pub fn window_list(&self, client: ClientId) -> Answer<Vec<WindowInfo>> {
        let session = self.desktop_only(client)?.session;
        let focus = self.session(session).and_then(|s| s.focus);
        let mut list: Vec<&Window> = self
            .windows
            .values()
            .filter(|w| {
                matches!(w.kind, WindowKind::Normal | WindowKind::Dialog | WindowKind::System)
                    && self.clients.get(&w.id.client).is_some_and(|c| c.session == session)
            })
            .collect();
        list.sort_by_key(|w| w.opened);
        Ok(list
            .into_iter()
            .take(64)
            .map(|w| {
                let c = self.clients.get(&w.id.client);
                WindowInfo {
                    window: w.id.packed(),
                    app: c.map_or(0, |c| c.app),
                    title: self.full_title(w.id),
                    minimised: w.minimised,
                    focused: focus == Some(w.id),
                    attention: w.attention,
                    not_responding: c.is_some_and(|c| c.not_responding),
                }
            })
            .collect())
    }

    /// `act` (desktop only): focus, raise, minimise, restore or close any of the session's
    /// windows.
    pub fn act(&mut self, client: ClientId, window: u64, action: u32, now: u64) -> Answer<()> {
        let session = self.desktop_only(client)?.session;
        let id = WindowId::unpack(window);
        let w = self.windows.get(&id).ok_or(WindowError::NoSuchWindow)?;
        if self.clients.get(&id.client).is_none_or(|c| c.session != session) {
            return Err(WindowError::NoSuchWindow);
        }
        if !w.focusable() {
            return Err(WindowError::BadKind);
        }
        match action {
            act::FOCUS => self.focus(id, now),
            act::RAISE => self.raise(id),
            act::MINIMISE => self.set_minimised(id, true, now),
            act::RESTORE => {
                self.set_minimised(id, false, now);
                self.set_maximised(id, false, now);
            }
            act::CLOSE => self.send(id.client, Event::new(kind::CLOSE, id.number, [0; 3], now)),
            _ => return Err(WindowError::BadArgument),
        }
        Ok(())
    }

    /// `pointer` (desktop only): the speed, and which screen each tablet is tied to.
    pub fn set_pointer(
        &mut self,
        client: ClientId,
        speed: u32,
        ties: Vec<(String, u32)>,
    ) -> Answer<()> {
        self.desktop_only(client)?;
        if !(10..=1000).contains(&speed) || ties.len() > 8 {
            return Err(WindowError::BadArgument);
        }
        if ties.iter().any(|(_, s)| *s != 0 && self.space.screen(*s).is_none()) {
            return Err(WindowError::NoSuchWindow);
        }
        self.pointer.speed = speed;
        self.ties = ties;
        Ok(())
    }

    /// The title bar's words: the manifest's title, then the app's own after a dash, and
    /// "(not responding)" when so.
    pub fn full_title(&self, id: WindowId) -> String {
        let Some(w) = self.windows.get(&id) else { return String::new() };
        let c = self.clients.get(&id.client);
        let mut t = c.map_or_else(String::new, |c| c.title.clone());
        if !w.title.is_empty() {
            if !t.is_empty() {
                t.push_str(" \u{2014} ");
            }
            t.push_str(&w.title);
        }
        if c.is_some_and(|c| c.not_responding) {
            t.push_str(" (not responding)");
        }
        t
    }

    // ---- The clipboard ----------------------------------------------------------------

    /// Whether a client may copy (note 0022, section 5): its window has the focus, the serial
    /// is a press delivered to it in the last second, and it fits.
    pub fn check_copy(
        &self,
        client: ClientId,
        serial: u32,
        picture: bool,
        length: u64,
        size: (u32, u32),
        now: u64,
    ) -> Answer<()> {
        let c = self.clients.get(&client).ok_or(WindowError::NotAllowed)?;
        if self.focused_client(c.session) != Some(client) {
            return Err(WindowError::NotFocused);
        }
        if !self.recent_press(client, None, serial, now) {
            return Err(WindowError::BadSerial);
        }
        if length > MAX_CLIPBOARD {
            return Err(WindowError::NoMemory);
        }
        if picture {
            let want = u64::from(size.0) * u64::from(size.1) * 4;
            if size.0 == 0 || size.1 == 0 || want != length {
                return Err(WindowError::BadArgument);
            }
        } else if length == 0 {
            return Err(WindowError::BadArgument);
        }
        Ok(())
    }

    /// What was copied, read from the app's memory by the display server: put on the
    /// clipboard (or added to the item its serial started).
    pub fn copied(
        &mut self,
        client: ClientId,
        serial: u32,
        picture: Option<(u32, u32)>,
        data: Vec<u8>,
        now: u64,
    ) -> Answer<()> {
        let session = self.clients.get(&client).ok_or(WindowError::NotAllowed)?.session;
        if picture.is_none() && core::str::from_utf8(&data).is_err() {
            return Err(WindowError::BadArgument);
        }
        let s = self.session_mut(session).ok_or(WindowError::NotAllowed)?;
        let mut item = if s.clipboard.serial == serial && s.clipboard.client == client {
            core::mem::take(&mut s.clipboard)
        } else {
            Clipboard { serial, client, text: None, picture: None }
        };
        match picture {
            Some((w, h)) => item.picture = Some((w, h, data)),
            None => item.text = Some(data),
        }
        if item.bytes() > MAX_CLIPBOARD {
            return Err(WindowError::NoMemory);
        }
        let forms = item.forms();
        s.clipboard = item;
        self.send(client, Event::new(kind::CLIPBOARD, 0, [forms, 0, 0], now));
        self.changed();
        Ok(())
    }

    /// `paste`: the clipboard's text or picture, for the focused client with a press's serial
    /// from the last second. A picture comes with its size.
    pub fn paste(
        &self,
        client: ClientId,
        serial: u32,
        picture: bool,
        now: u64,
    ) -> Answer<(&[u8], u32, u32)> {
        let c = self.clients.get(&client).ok_or(WindowError::NotAllowed)?;
        if self.focused_client(c.session) != Some(client) {
            return Err(WindowError::NotFocused);
        }
        if !self.recent_press(client, None, serial, now) {
            return Err(WindowError::BadSerial);
        }
        let clip = &self.session(c.session).ok_or(WindowError::NotAllowed)?.clipboard;
        match (picture, &clip.text, &clip.picture) {
            (false, Some(t), _) => Ok((t, 0, 0)),
            (true, _, Some((w, h, d))) => Ok((d, *w, *h)),
            _ => Err(WindowError::Empty),
        }
    }

    // ---- Shutting down ----------------------------------------------------------------

    /// `farewell`: the shutting-down screen on every screen, and nothing else from now on.
    pub fn farewell(&mut self, restarting: bool) {
        self.set_farewell(if restarting { Farewell::Restarting } else { Farewell::ShuttingDown });
    }

    /// `halted`: after `farewell`, when the computer couldn't be turned off by software: says
    /// it can be turned off by hand now (note 0024, section 7).
    pub fn halted(&mut self) {
        self.set_farewell(Farewell::Halted);
    }

    fn set_farewell(&mut self, f: Farewell) {
        self.farewell = Some(f);
        self.menu = None;
        self.end_grab();
        self.damage_all();
        self.changed();
    }

    /// Ends whatever the pointer was doing with its buttons held.
    pub(crate) fn end_grab(&mut self) {
        self.pointer.grab = Grab::None;
    }

    pub(crate) fn damage_pointer(&mut self) {
        let (x, y) = ((self.pointer.x >> 8) as i32, (self.pointer.y >> 8) as i32);
        self.damage(decor::pointer_bounds(x, y));
    }

    /// Starts moving a window with the pointer.
    pub(crate) fn begin_move(&mut self, id: WindowId) {
        let Some(w) = self.windows.get(&id) else { return };
        let dx = self.pointer.x - (i64::from(w.place.0) << 8);
        let dy = self.pointer.y - (i64::from(w.place.1) << 8);
        self.pointer.grab = Grab::Move(id, dx, dy);
        self.damage_pointer();
    }

    /// The user is resizing a window: its content would be `r`. Told at most every few
    /// frames, and always at the end.
    pub(crate) fn resize_to(&mut self, id: WindowId, r: Rect, now: u64, last: bool) {
        let Some(w) = self.windows.get(&id) else { return };
        if !last && now.saturating_sub(w.resized_at) < RESIZE_EVERY {
            return;
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.resized_at = now;
            w.restore = None;
        }
        self.set_place(id, (r.x, r.y), now);
        self.set_size(id, (r.width, r.height), now);
    }

    /// Moves a window to another screen (the window menu's "Move to screen").
    pub(crate) fn send_to_screen(&mut self, id: WindowId, screen: u32, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let from = self.space.holding_most(w.content()).and_then(|n| self.space.screen(n));
        let Some(from) = from.map(Screen::rect) else { return };
        let maxed = w.restore.is_some();
        if maxed {
            self.set_maximised(id, false, now);
        }
        self.move_to_screen(id, from, Some(screen), now);
        if maxed {
            self.set_maximised(id, true, now);
        }
    }
}

/// One window, for the desktop's taskbar (`pbr.window`'s `WindowInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    /// The client's and window's numbers together ([`WindowId::packed`]).
    pub window: u64,
    /// The sandbox manager's number for the app.
    pub app: u32,
    /// As its title bar shows it.
    pub title: String,
    /// Minimised.
    pub minimised: bool,
    /// Has the focus.
    pub focused: bool,
    /// Opened without the focus: wants the user to look.
    pub attention: bool,
    /// Its app hasn't answered a ping in time.
    pub not_responding: bool,
}

/// A title as it is kept: control characters dropped, at most 64 bytes.
fn clean_title(t: &str) -> String {
    let mut s = String::new();
    for c in t.chars().filter(|c| !c.is_control()) {
        if s.len() + c.len_utf8() > 64 {
            break;
        }
        s.push(c);
    }
    s
}

/// The corner that centres a size in a rectangle.
fn centred(r: Rect, size: (i32, i32)) -> (i32, i32) {
    (r.x + (r.width - size.0) / 2, r.y + (r.height - size.1) / 2)
}

/// Which of `screens` holds most of `r` (as [`Space::holding_most`], for a list from before a
/// change).
fn holding_most_of(screens: &[Screen], r: Rect) -> Option<u32> {
    screens
        .iter()
        .filter(|s| s.in_space())
        .map(|s| (s.rect().intersect(&r).area(), s.number))
        .filter(|(a, _)| *a > 0)
        .max_by_key(|(a, _)| *a)
        .map(|(_, n)| n)
}

/// The window a grab is about.
pub(crate) fn grab_window(g: Grab) -> Option<WindowId> {
    match g {
        Grab::Pressed(w) | Grab::Move(w, ..) | Grab::Resize(w, ..) | Grab::Button(w, _) => Some(w),
        _ => None,
    }
}

/// `r` with `cut` taken out of it: up to four pieces, added to `out`.
pub(crate) fn subtract(r: Rect, cut: Rect, out: &mut Vec<Rect>) {
    let i = r.intersect(&cut);
    if i.is_empty() {
        out.push(r);
        return;
    }
    let (rx, ry, rr, rb) = (i64::from(r.x), i64::from(r.y), r.right(), r.bottom());
    let (ix, iy, ir, ib) = (i64::from(i.x), i64::from(i.y), i.right(), i.bottom());
    for piece in [
        Rect::from_edges(rx, ry, rr, iy),
        Rect::from_edges(rx, ib, rr, rb),
        Rect::from_edges(rx, iy, ix, ib),
        Rect::from_edges(ir, iy, rr, ib),
    ] {
        if !piece.is_empty() {
            out.push(piece);
        }
    }
}
