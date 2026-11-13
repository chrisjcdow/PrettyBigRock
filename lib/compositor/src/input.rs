// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Where keys and the pointer go (docs/design/0022-display-server.md, section 4).
//!
//! The display server is the seat's consumer (note 0021): every key and pointer event of every
//! keyboard, mouse and tablet comes to it, merged, with the layout applied. It sends each on to
//! exactly one window, or keeps it:
//!
//! - **Keys** go to the window with the focus and nowhere else. A few combinations are kept and
//!   never sent to an app: the Windows key and Ctrl+Esc (the start menu, told to the desktop),
//!   Alt+Tab (switching windows) and Ctrl+W (close the focused window). A key's release goes
//!   where its press went, so no app is left thinking a key is held.
//! - **The pointer** goes to the window under it. While a button is held everything goes to the
//!   window it was pressed in, even outside it, until every button is let go: that is the only
//!   way anything holds on to the pointer. Presses on decorations are the display server's own:
//!   moving, resizing, the title bar's buttons and the window menu.
//! - **Mice** move one pointer across all the screens (scaled by the desktop's speed setting)
//!   and can't take it off them. A **tablet** says where on a screen it points: it is tied to
//!   one screen (the n-th tablet to the n-th screen when there are as many of each), or to the
//!   whole space.

use crate::decor::{self, Button, Part};
use crate::desk::{Desk, Grab, Menu, MenuItem, Tablet, WindowId};
use crate::event::{Event, VALUE_SECOND, kind};
use alloc::vec::Vec;
use pbr_devices::keys::key;
use pbr_draw::Rect;
use pbr_interfaces::pbr_window::{Cursor, WindowFlags, WindowKind};
use pbr_seat::Event as SeatEvent;
use pbr_seat::event::{kind as seat, mods, value};

/// Two presses on a title bar this close together are a double click.
const DOUBLE_CLICK: u64 = 500_000_000;
/// `pbr.input`'s axes.
const AXIS_X: u16 = 1;
const AXIS_Y: u16 = 2;
/// The right mouse button, which opens the window menu on a title bar.
const BUTTON_RIGHT: u16 = 2;

impl Desk {
    /// The seat's tablets changed (`pbr.seat.devices`): the display server passes the
    /// tablets, in the seat's order.
    pub fn set_tablets(&mut self, tablets: Vec<Tablet>) {
        self.tablets = tablets;
    }

    /// One event from the seat.
    pub fn seat(&mut self, e: &SeatEvent, now: u64) {
        match e.kind {
            seat::LOST | seat::RESET => self.reset(now),
            seat::KEY => self.key(e, now),
            seat::MOVE => {
                let step = i64::from(e.value) * 256 * i64::from(self.pointer.speed) / 100;
                let (mut x, mut y) = (self.pointer.x, self.pointer.y);
                match e.code {
                    AXIS_X => x = x.saturating_add(step),
                    AXIS_Y => y = y.saturating_add(step),
                    _ => return,
                }
                self.point_at(x, y, now);
            }
            seat::POSITION => {
                let Some((fx, fy)) = e.position() else { return };
                if let Some((x, y)) = self.tablet_point(e.device, u32::from(fx), u32::from(fy)) {
                    self.point_at(x, y, now);
                }
            }
            seat::BUTTON => {
                let bit = 1u32.checked_shl(u32::from(e.code)).unwrap_or(0);
                if bit == 0 {
                    return;
                }
                self.pointer.seen = true;
                match e.value {
                    value::PRESSED => self.press(e.code, bit, now),
                    value::RELEASED => self.release(e.code, bit, now),
                    // Held across a reset: held, but not a press anyone should act on.
                    value::HELD => self.pointer.buttons |= bit,
                    _ => {}
                }
            }
            seat::SCROLL => {
                let target = match self.pointer.grab {
                    Grab::Pressed(w) => Some(w),
                    Grab::None => self.pointer.hover,
                    _ => None,
                };
                if let Some(w) = target
                    && self.takes_input()
                {
                    let e = Event::new(
                        kind::SCROLL,
                        w.number,
                        [u32::from(e.code), e.value as u32, 0],
                        now,
                    );
                    self.send(w.client, e);
                }
            }
            _ => {}
        }
    }

    /// Whether input goes to windows at all: there is an active session, and it isn't
    /// shutting down.
    fn takes_input(&self) -> bool {
        self.active.is_some() && self.farewell.is_none()
    }

    /// The seat lost track (or a device went): nothing is held any more. Whatever a window
    /// was told is held is let go of first.
    fn reset(&mut self, now: u64) {
        for (code, w) in core::mem::take(&mut self.keys_to) {
            let e = Event::new(kind::KEY, w.number, [u32::from(code), 0, 0], now);
            self.send(w.client, e);
        }
        self.kept_keys.clear();
        self.switching = None;
        if let Grab::Pressed(w) = self.pointer.grab {
            for b in 0..32u32 {
                if self.pointer.buttons & (1 << b) != 0 {
                    self.send(w.client, Event::new(kind::BUTTON, w.number, [b, 0, 0], now));
                }
            }
        }
        if let Grab::Resize(id, edge, start, from) = self.pointer.grab {
            let r = self.resized(id, edge, start, from);
            self.resize_to(id, r, now, true);
        }
        self.pointer.buttons = 0;
        self.pointer.grab = Grab::None;
        self.damage_pointer();
    }

    // ---- Keys ---------------------------------------------------------------------------

    fn key(&mut self, e: &SeatEvent, now: u64) {
        if !self.takes_input() {
            return;
        }
        let code = e.code;
        match e.value {
            value::RELEASED => {
                if code == key::LEFT_ALT || code == key::RIGHT_ALT {
                    self.switching = None;
                }
                if let Some(i) = self.kept_keys.iter().position(|k| *k == code) {
                    self.kept_keys.swap_remove(i);
                    return;
                }
                if let Some(i) = self.keys_to.iter().position(|(k, _)| *k == code) {
                    let (_, w) = self.keys_to.swap_remove(i);
                    self.deliver_key(w, e, None, now);
                }
            }
            value::PRESSED => {
                if self.kept(e, now) {
                    self.kept_keys.push(code);
                    return;
                }
                let Some(w) = self.focused() else { return };
                let serial = self.serial();
                self.record_press(w, serial, now);
                self.keys_to.retain(|(k, _)| *k != code);
                self.keys_to.push((code, w));
                self.deliver_key(w, e, Some(serial), now);
            }
            value::REPEAT => {
                if self.kept_keys.contains(&code) {
                    // Alt+Tab held down goes on switching.
                    if code == key::TAB {
                        self.switch_window(now);
                    }
                    return;
                }
                if let Some(&(_, w)) = self.keys_to.iter().find(|(k, _)| *k == code) {
                    let serial = self.serial();
                    self.record_press(w, serial, now);
                    self.deliver_key(w, e, Some(serial), now);
                }
            }
            value::HELD => {
                if let Some(w) = self.focused() {
                    self.keys_to.retain(|(k, _)| *k != code);
                    self.keys_to.push((code, w));
                    self.deliver_key(w, e, None, now);
                }
            }
            _ => {}
        }
    }

    /// The window with the focus in the active session.
    pub(crate) fn focused(&self) -> Option<WindowId> {
        self.active.and_then(|a| self.session(a)).and_then(|s| s.focus)
    }

    /// Sends a key event (and its second character, if it types two) to a window.
    fn deliver_key(&mut self, w: WindowId, e: &SeatEvent, serial: Option<u32>, now: u64) {
        let a = u32::from(e.code) | (u32::from(e.mods) << 16);
        let mut chars = e.chars();
        let first = if e.value == value::RELEASED { None } else { chars.next() };
        let ev =
            Event::new(kind::KEY, w.number, [a, e.value as u32, first.map_or(0, u32::from)], now)
                .with_serial(serial.unwrap_or(0));
        self.send(w.client, ev);
        if e.value != value::RELEASED
            && let Some(second) = chars.next()
        {
            let ev = Event::new(kind::KEY, w.number, [a, VALUE_SECOND, u32::from(second)], now)
                .with_serial(serial.unwrap_or(0));
            self.send(w.client, ev);
        }
    }

    /// Whether a key press is one the display server keeps, doing what it means.
    fn kept(&mut self, e: &SeatEvent, now: u64) -> bool {
        let m = e.mods;
        let ctrl = m & mods::CTRL != 0;
        let alt = m & mods::ALT != 0;
        let others = m & (mods::ALT | mods::META) != 0;
        match e.code {
            key::LEFT_META | key::RIGHT_META => {
                self.start_menu(now);
                true
            }
            key::ESCAPE if ctrl && !others => {
                self.start_menu(now);
                true
            }
            key::TAB if alt && !ctrl => {
                self.switch_window(now);
                true
            }
            key::W if ctrl && !others => {
                if let Some(w) = self.focused() {
                    self.send(w.client, Event::new(kind::CLOSE, w.number, [0; 3], now));
                }
                true
            }
            _ => false,
        }
    }

    fn start_menu(&mut self, now: u64) {
        if let Some(a) = self.active {
            self.tell_desktop_start_menu(a, now);
        }
    }

    /// Alt+Tab: the next window, in the order they were last focused, while Alt stays held.
    fn switch_window(&mut self, now: u64) {
        let Some(a) = self.active else { return };
        if self.switching.is_none() {
            // The focused window first, then the others from the top down, then the
            // minimised ones.
            let candidates: Vec<WindowId> = self
                .session(a)
                .map(|s| {
                    s.stack
                        .iter()
                        .rev()
                        .copied()
                        .filter(|id| {
                            self.windows.get(id).is_some_and(|w| {
                                matches!(w.kind, WindowKind::Normal | WindowKind::System)
                                    && !w.dismissed
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let focus = self.focused().map(|f| self.root_of(f));
            let minimised = |id: &WindowId| self.windows.get(id).is_some_and(|w| w.minimised);
            let mut list: Vec<WindowId> = focus.into_iter().collect();
            list.extend(candidates.iter().filter(|w| Some(**w) != focus && !minimised(w)));
            list.extend(candidates.iter().filter(|w| Some(**w) != focus && minimised(w)));
            self.switching = Some((list, 0));
        }
        let Some((list, at)) = &mut self.switching else { return };
        if list.len() < 2 {
            return;
        }
        *at = (*at + 1) % list.len();
        let next = list[*at];
        self.focus(next, now);
    }

    // ---- The pointer --------------------------------------------------------------------

    /// Where a tablet's position is in the space: on the screen it is tied to, or across the
    /// whole space.
    fn tablet_point(&self, device: u16, fx: u32, fy: u32) -> Option<(i64, i64)> {
        let index = self.tablets.iter().position(|t| t.id == device)?;
        let name = &self.tablets[index].name;
        let screens: Vec<u32> = self.space.in_space().map(|s| s.number).collect();
        let screen = match self.ties.iter().find(|(n, _)| n == name) {
            Some((_, s)) => *s,
            // The n-th tablet to the n-th screen, when there are as many of each.
            None if self.tablets.len() == screens.len() => screens[index],
            None => 0,
        };
        if screen != 0 {
            return self.space.surface_to_space(screen, fx, fy);
        }
        // The whole space: the rectangle round every screen.
        let all = self.space.in_space().map(|s| s.rect()).fold(Rect::EMPTY, |a, r| a.union(&r));
        if all.is_empty() {
            return None;
        }
        let x = (i64::from(all.x) << 8)
            + i64::from(fx.min(65535)) * (i64::from(all.width) << 8) / 65536;
        let y = (i64::from(all.y) << 8)
            + i64::from(fy.min(65535)) * (i64::from(all.height) << 8) / 65536;
        Some(self.space.clamp(x, y))
    }

    /// The pointer moves to (`x`, `y`), in 1/256 logical pixels.
    fn point_at(&mut self, x: i64, y: i64, now: u64) {
        let (x, y) = self.space.clamp(x, y);
        self.pointer.seen = true;
        if (x >> 8, y >> 8) != (self.pointer.x >> 8, self.pointer.y >> 8) {
            self.damage_pointer();
            self.pointer.x = x;
            self.pointer.y = y;
            self.damage_pointer();
            // The description says where the pointer is (the boot tests move it from there).
            self.changed();
        } else {
            self.pointer.x = x;
            self.pointer.y = y;
        }
        self.moved(now);
    }

    /// The pointer's place in whole logical pixels.
    pub(crate) fn pointer_pixel(&self) -> (i32, i32) {
        ((self.pointer.x >> 8) as i32, (self.pointer.y >> 8) as i32)
    }

    /// The window under a point and what part of it, top to bottom, in the active session.
    pub(crate) fn under(&self, x: i32, y: i32) -> Option<(WindowId, Part)> {
        let session = self.active.and_then(|a| self.session(a))?;
        for id in session.stack.iter().rev() {
            if !self.visible(*id) {
                continue;
            }
            let Some(w) = self.windows.get(id) else { continue };
            let resizable = w.flags.contains(WindowFlags::RESIZABLE) && w.restore.is_none();
            if let Some(part) = decor::hit(w.content(), w.decorated(), resizable, x, y) {
                return Some((*id, part));
            }
        }
        None
    }

    /// The pointer moved: whatever is grabbing it follows, or the window under it hears.
    fn moved(&mut self, now: u64) {
        if !self.takes_input() {
            return;
        }
        let (px, py) = self.pointer_pixel();
        match self.pointer.grab {
            Grab::Move(id, dx, dy) => {
                let place =
                    (((self.pointer.x - dx) >> 8) as i32, ((self.pointer.y - dy) >> 8) as i32);
                self.set_place(id, place, now);
            }
            Grab::Resize(id, edge, start, from) => {
                let r = self.resized(id, edge, start, from);
                self.resize_to(id, r, now, false);
            }
            Grab::Pressed(w) => self.pointer_event(w, now),
            Grab::Menu | Grab::Button(..) | Grab::Nothing => self.menu_hover(px, py),
            Grab::None => {
                self.menu_hover(px, py);
                let over = match self.under(px, py) {
                    Some((id, Part::Content)) if self.menu_at(px, py).is_none() => Some(id),
                    _ => None,
                };
                if over != self.pointer.hover {
                    if let Some(old) = self.pointer.hover {
                        self.send(old.client, Event::new(kind::LEFT, old.number, [0; 3], now));
                    }
                    self.pointer.hover = over;
                }
                if let Some(w) = over {
                    self.pointer_event(w, now);
                }
            }
        }
    }

    /// Tells a window where the pointer is, from its content's corner.
    fn pointer_event(&mut self, w: WindowId, now: u64) {
        let Some(win) = self.windows.get(&w) else { return };
        let x = self.pointer.x - (i64::from(win.place.0) << 8);
        let y = self.pointer.y - (i64::from(win.place.1) << 8);
        let clamp = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32 as u32;
        let e =
            Event::new(kind::POINTER, w.number, [clamp(x), clamp(y), self.pointer.buttons], now);
        self.send(w.client, e);
    }

    /// Where a window being resized by `edge` would be now: its content when the resize
    /// began, moved by how far the pointer has gone, within its limits, the opposite edge
    /// staying put.
    pub(crate) fn resized(&self, id: WindowId, edge: u32, start: Rect, from: (i64, i64)) -> Rect {
        let dx = ((self.pointer.x - from.0) >> 8) as i32;
        let dy = ((self.pointer.y - from.1) >> 8) as i32;
        let (left, right) = (matches!(edge, 3 | 5 | 7), matches!(edge, 4 | 6 | 8));
        let (top, bottom) = (matches!(edge, 1 | 5 | 6), matches!(edge, 2 | 7 | 8));
        let mut w = start.width;
        let mut h = start.height;
        if left {
            w = w.saturating_sub(dx);
        } else if right {
            w = w.saturating_add(dx);
        }
        if top {
            h = h.saturating_sub(dy);
        } else if bottom {
            h = h.saturating_add(dy);
        }
        let Some(win) = self.windows.get(&id) else { return start };
        let (w, h) = self.limit_size(win, (w, h));
        let x = if left { start.x + start.width - w } else { start.x };
        let y = if top { start.y + start.height - h } else { start.y };
        Rect::new(x, y, w, h)
    }

    /// A button was pressed.
    fn press(&mut self, button: u16, bit: u32, now: u64) {
        let first = self.pointer.buttons == 0;
        self.pointer.buttons |= bit;
        if !self.takes_input() {
            return;
        }
        if !first {
            // Another button while one is held: to whoever has the pointer, if a window does.
            if let Grab::Pressed(w) = self.pointer.grab {
                self.deliver_button(w, button, true, now);
            }
            return;
        }
        let (px, py) = self.pointer_pixel();
        // The window menu: a press on it is acted on when let go; anywhere else closes it.
        if self.menu.is_some() {
            if self.menu_at(px, py).is_some() {
                self.pointer.grab = Grab::Menu;
            } else {
                self.close_menu();
                self.pointer.grab = Grab::Nothing;
            }
            return;
        }
        let hit = self.under(px, py);
        self.dismiss_popups_outside(hit.map(|h| h.0), now);
        let Some((id, part)) = hit else {
            self.pointer.grab = Grab::Nothing;
            return;
        };
        let Some(w) = self.windows.get(&id) else { return };
        let (kind, focusable) = (w.kind, w.focusable());
        // Input to a window with a modal dialog, or a system window, over it goes to that.
        let blocker = self.blocker(id);
        if focusable {
            self.focus(id, now);
            // Clicking an app's window is one of the times it is pinged (note 0022, section 3).
            self.ping(id.client, now);
        }
        if blocker != id {
            self.pointer.grab = Grab::Nothing;
            return;
        }
        match part {
            Part::Content => {
                self.pointer.grab = Grab::Pressed(id);
                if self.pointer.hover != Some(id) {
                    if let Some(old) = self.pointer.hover {
                        self.send(old.client, Event::new(kind::LEFT, old.number, [0; 3], now));
                    }
                    self.pointer.hover = Some(id);
                    self.pointer_event(id, now);
                }
                self.deliver_button(id, button, true, now);
            }
            Part::Title => {
                let double = self
                    .pointer
                    .last_title
                    .is_some_and(|(w, t)| w == id && now.saturating_sub(t) <= DOUBLE_CLICK);
                self.leave_hover(now);
                if button == BUTTON_RIGHT {
                    self.open_menu(id, px, py);
                    self.pointer.grab = Grab::Nothing;
                } else if double && kind != WindowKind::Dialog {
                    self.pointer.last_title = None;
                    let on = self.windows.get(&id).is_some_and(|w| w.restore.is_none());
                    self.set_maximised(id, on, now);
                    self.pointer.grab = Grab::Nothing;
                } else {
                    self.pointer.last_title = Some((id, now));
                    self.unmaximise_under_pointer(id, now);
                    self.begin_move(id);
                }
            }
            Part::Icon => {
                self.leave_hover(now);
                let r = self.windows.get(&id).map(|w| decor::icon(w.content()));
                if let Some(r) = r {
                    self.open_menu(id, r.x, r.y + r.height + 4);
                }
                self.pointer.grab = Grab::Nothing;
            }
            Part::Button(b) => {
                self.leave_hover(now);
                self.pointer.grab = Grab::Button(id, b);
            }
            Part::Edge(e) => {
                self.leave_hover(now);
                let content = self.windows.get(&id).map(|w| w.content()).unwrap_or(Rect::EMPTY);
                self.pointer.grab = Grab::Resize(id, e, content, (self.pointer.x, self.pointer.y));
            }
            Part::Border => {
                self.leave_hover(now);
                self.pointer.grab = Grab::Nothing;
            }
        }
        self.damage_pointer();
    }

    /// The pointer is on decorations now, not in the window's content.
    fn leave_hover(&mut self, now: u64) {
        if let Some(old) = self.pointer.hover.take() {
            self.send(old.client, Event::new(kind::LEFT, old.number, [0; 3], now));
        }
    }

    /// A maximised window being dragged by its title bar is restored first, with the pointer
    /// on its title bar at the same fraction of its width.
    fn unmaximise_under_pointer(&mut self, id: WindowId, now: u64) {
        let Some(w) = self.windows.get(&id) else { return };
        let Some((_, (rw, _))) = w.restore else { return };
        let (px, py) = self.pointer_pixel();
        let along = i64::from(px - w.place.0).clamp(0, i64::from(w.size.0.max(1)));
        let x = px - (along * i64::from(rw) / i64::from(w.size.0.max(1))) as i32;
        let y = py + decor::TITLE / 2;
        self.set_maximised(id, false, now);
        self.set_place(id, (x, y), now);
    }

    fn deliver_button(&mut self, w: WindowId, button: u16, down: bool, now: u64) {
        let e = Event::new(
            kind::BUTTON,
            w.number,
            [u32::from(button), u32::from(down), self.pointer.buttons],
            now,
        );
        if down {
            let serial = self.serial();
            self.record_press(w, serial, now);
            self.send(w.client, e.with_serial(serial));
        } else {
            self.send(w.client, e);
        }
    }

    /// A button was let go.
    fn release(&mut self, button: u16, bit: u32, now: u64) {
        let was_held = self.pointer.buttons & bit != 0;
        self.pointer.buttons &= !bit;
        if !self.takes_input() {
            if self.pointer.buttons == 0 {
                self.pointer.grab = Grab::None;
            }
            return;
        }
        let last = self.pointer.buttons == 0;
        let (px, py) = self.pointer_pixel();
        match self.pointer.grab {
            Grab::Pressed(w) => {
                if was_held {
                    self.deliver_button(w, button, false, now);
                }
            }
            Grab::Resize(id, edge, start, from) if last => {
                let r = self.resized(id, edge, start, from);
                self.resize_to(id, r, now, true);
            }
            Grab::Button(id, b) if last => {
                let on = self.windows.get(&id).is_some_and(|w| {
                    decor::hit(w.content(), true, false, px, py) == Some(Part::Button(b))
                });
                if on {
                    self.title_button(id, b, now);
                }
            }
            Grab::Menu if last => {
                if let Some(i) = self.menu_at(px, py) {
                    self.menu_choose(i, now);
                }
            }
            _ => {}
        }
        if last {
            self.pointer.grab = Grab::None;
            self.damage_pointer();
            // What is under the pointer now may be another window.
            self.moved(now);
        }
    }

    /// A title bar button was clicked.
    fn title_button(&mut self, id: WindowId, b: Button, now: u64) {
        match b {
            Button::Close => self.send(id.client, Event::new(kind::CLOSE, id.number, [0; 3], now)),
            Button::Maximise => {
                let on = self.windows.get(&id).is_some_and(|w| w.restore.is_none());
                self.set_maximised(id, on, now);
            }
            Button::Minimise => self.set_minimised(id, true, now),
        }
    }

    /// A press anywhere but on a popup (or on something that belongs to it) dismisses it.
    fn dismiss_popups_outside(&mut self, pressed: Option<WindowId>, now: u64) {
        let Some(a) = self.active else { return };
        let popups: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| {
                w.kind == WindowKind::Popup
                    && !w.dismissed
                    && self.clients.get(&w.id.client).is_some_and(|c| c.session == a)
            })
            .map(|w| w.id)
            .filter(|p| pressed.is_none_or(|x| x != *p && !self.belongs_to(x, *p)))
            .collect();
        for p in popups {
            self.dismiss(p, now);
        }
    }

    // ---- The window menu ----------------------------------------------------------------

    /// Opens the window menu for a window at (`x`, `y`), kept on the screen there.
    fn open_menu(&mut self, id: WindowId, x: i32, y: i32) {
        let Some(w) = self.windows.get(&id) else { return };
        let mut items = Vec::new();
        if matches!(w.kind, WindowKind::Normal | WindowKind::System) {
            items.push(MenuItem::Minimise);
        }
        items.push(if w.restore.is_some() { MenuItem::Restore } else { MenuItem::Maximise });
        let here = self.space.holding_most(w.content());
        for s in self.space.in_space() {
            if Some(s.number) != here {
                items.push(MenuItem::MoveTo(s.number));
            }
        }
        items.push(MenuItem::Permissions);
        if self.clients.get(&id.client).is_some_and(|c| c.not_responding) {
            items.push(MenuItem::Stop);
        }
        items.push(MenuItem::Close);
        let height = items.len() as i32 * decor::MENU_ROW + 4;
        let mut rect = Rect::new(x, y, decor::MENU_WIDTH, height);
        if let Some(s) = self.space.nearest(x, y).and_then(|n| self.space.screen(n)) {
            let r = s.rect();
            rect.x = rect.x.min(r.x + r.width - rect.width).max(r.x);
            rect.y = rect.y.min(r.y + r.height - rect.height).max(r.y);
        }
        if let Some(old) = self.menu.take() {
            self.damage(decor::menu_bounds(old.rect));
        }
        self.damage(decor::menu_bounds(rect));
        self.menu = Some(Menu { window: id, rect, items, hover: None });
        self.changed();
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.damage(decor::menu_bounds(m.rect));
            self.changed();
        }
    }

    /// The menu's row at a point.
    pub(crate) fn menu_at(&self, x: i32, y: i32) -> Option<usize> {
        let m = self.menu.as_ref()?;
        if !m.rect.contains(x, y) {
            return None;
        }
        let row = (y - m.rect.y - 2) / decor::MENU_ROW;
        usize::try_from(row).ok().filter(|r| *r < m.items.len())
    }

    fn menu_hover(&mut self, x: i32, y: i32) {
        let row = self.menu_at(x, y);
        if let Some(m) = &mut self.menu
            && m.hover != row
        {
            m.hover = row;
            let r = m.rect;
            self.damage(r);
        }
    }

    fn menu_choose(&mut self, row: usize, now: u64) {
        let Some(m) = self.menu.clone() else { return };
        self.close_menu();
        let id = m.window;
        if !self.windows.contains_key(&id) {
            return;
        }
        let packed = id.packed();
        let to_desktop = |choice: u32| {
            Event::new(kind::WINDOW_MENU, 0, [packed as u32, (packed >> 32) as u32, choice], now)
        };
        match m.items.get(row) {
            Some(MenuItem::Minimise) => self.set_minimised(id, true, now),
            Some(MenuItem::Maximise) => self.set_maximised(id, true, now),
            Some(MenuItem::Restore) => self.set_maximised(id, false, now),
            Some(MenuItem::MoveTo(n)) => self.send_to_screen(id, *n, now),
            Some(MenuItem::Permissions) => {
                if let Some(a) = self.active {
                    self.tell_desktop(a, to_desktop(1));
                }
            }
            Some(MenuItem::Stop) => {
                if let Some(a) = self.active {
                    self.tell_desktop(a, to_desktop(2));
                }
            }
            Some(MenuItem::Close) => {
                self.send(id.client, Event::new(kind::CLOSE, id.number, [0; 3], now));
            }
            None => {}
        }
    }

    /// The pointer's shape now: what a grab means, or what is under it.
    pub fn cursor(&self) -> Cursor {
        let edge_cursor = |e: u32| match e {
            1 | 2 => Cursor::ResizeNs,
            3 | 4 => Cursor::ResizeEw,
            5 | 8 => Cursor::ResizeNwse,
            _ => Cursor::ResizeNesw,
        };
        match self.pointer.grab {
            Grab::Move(..) => return Cursor::Move,
            Grab::Resize(_, e, ..) => return edge_cursor(e),
            Grab::Pressed(w) => {
                return self.windows.get(&w).map_or(Cursor::Arrow, |w| w.cursor);
            }
            _ => {}
        }
        let (x, y) = self.pointer_pixel();
        if self.menu_at(x, y).is_some() {
            return Cursor::Arrow;
        }
        match self.under(x, y) {
            Some((id, Part::Content)) => self.windows.get(&id).map_or(Cursor::Arrow, |w| w.cursor),
            Some((_, Part::Edge(e))) => edge_cursor(e),
            _ => Cursor::Arrow,
        }
    }
}
