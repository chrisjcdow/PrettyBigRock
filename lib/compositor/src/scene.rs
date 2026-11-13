// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What is on the screens, for drawing them ([`Scene`]) and for describing them in words
//! ([`Desk::describe`], docs/design/0022-display-server.md, section 13).
//!
//! The description is how the boot tests check what the display server shows without
//! comparing pictures: after each change it writes, in self-test boots, a block of lines to the
//! kernel log, one per window, then the pointer and the clipboard:
//!
//! ```text
//! view 12
//! screen 1 "Virtual-1" 1280x800 at 0,0 scale 100 main
//! screen 1 window 3 "Notepad — letter.txt" app 7 at 120,80 size 400x300 scale 100 drawn focused
//! pointer at 300,200 screen 1
//! clipboard text
//! view 12 done
//! ```
//!
//! A test reads the last block that has its `done` line.

use crate::decor::Button;
use crate::desk::{Desk, Farewell, Grab, Menu, Shown, WindowId, flags};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_draw::Rect;
use pbr_interfaces::pbr_window::{Cursor, WindowKind};

/// One window to draw.
#[derive(Clone, Debug)]
pub struct SceneWindow {
    /// Which window.
    pub id: WindowId,
    /// What kind.
    pub kind: WindowKind,
    /// The app's picture's place in the space.
    pub content: Rect,
    /// Has a frame and title bar drawn round it.
    pub decorated: bool,
    /// Has the focus.
    pub focused: bool,
    /// The title bar's words.
    pub title: String,
    /// The app can reach all the user's files: the title bar says so.
    pub trusted: bool,
    /// A system window: its own frame colour and the rock emblem.
    pub system: bool,
    /// Maximised.
    pub maximised: bool,
    /// The frame on the screen, if the app has committed one.
    pub shown: Option<Shown>,
    /// A title bar button being held down.
    pub pressed: Option<Button>,
}

/// Everything on the screens, back to front.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// The windows, back to front.
    pub windows: Vec<SceneWindow>,
    /// The pointer: where, and its shape. `None` while no pointing device has been used.
    pub pointer: Option<(i32, i32, Cursor)>,
    /// The window menu, if it is open.
    pub menu: Option<Menu>,
    /// Shutting down: only the farewell is drawn.
    pub farewell: Option<Farewell>,
}

impl Desk {
    /// What to draw now.
    pub fn scene(&self) -> Scene {
        let mut scene = Scene { farewell: self.farewell, ..Scene::default() };
        if self.farewell.is_some() {
            return scene;
        }
        let focus = self.focused();
        if let Some(session) = self.active.and_then(|a| self.session(a)) {
            for id in &session.stack {
                if !self.visible(*id) {
                    continue;
                }
                let Some(w) = self.windows.get(id) else { continue };
                let cflags = self.clients.get(&id.client).map_or(0, |c| c.flags);
                scene.windows.push(SceneWindow {
                    id: *id,
                    kind: w.kind,
                    content: w.content(),
                    decorated: w.decorated(),
                    focused: focus == Some(*id),
                    title: self.full_title(*id),
                    trusted: cflags & flags::TRUSTED != 0,
                    system: w.kind == WindowKind::System,
                    maximised: w.restore.is_some(),
                    shown: w.shown,
                    pressed: match self.pointer.grab {
                        Grab::Button(g, b) if g == *id => Some(b),
                        _ => None,
                    },
                });
            }
        }
        if self.pointer.seen {
            let (x, y) = self.pointer_pixel();
            scene.pointer = Some((x, y, self.cursor()));
        }
        scene.menu = self.menu.clone();
        scene
    }

    /// The description of the screens (see the top of this file), numbered `view`.
    pub fn describe(&self, view: u64) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("view {view}"));
        for s in self.space.screens() {
            let (w, h) = s.logical_size();
            let mut line = format!(
                "screen {} \"{}\" {}x{} at {},{} scale {}",
                s.number, s.output.connector, w, h, s.place.x, s.place.y, s.place.scale
            );
            if s.place.rotation != 0 {
                line.push_str(&format!(" rotation {}", s.place.rotation));
            }
            if s.place.mirrors != 0 {
                line.push_str(&format!(" mirrors {}", s.place.mirrors));
            }
            if s.place.main {
                line.push_str(" main");
            }
            lines.push(line);
        }
        if let Some(session) = self.active.and_then(|a| self.session(a)) {
            let focus = session.focus;
            for id in &session.stack {
                let Some(w) = self.windows.get(id) else { continue };
                let c = self.clients.get(&id.client);
                let screen = if self.visible(*id) {
                    match w.kind {
                        WindowKind::Background | WindowKind::Panel => w.screen,
                        _ => self.space.holding_most(w.content()).unwrap_or(0),
                    }
                } else {
                    0
                };
                let mut line = format!(
                    "screen {screen} window {} \"{}\" app {} at {},{} size {}x{} scale {}",
                    id.number,
                    self.full_title(*id),
                    c.map_or(0, |c| c.app),
                    w.place.0,
                    w.place.1,
                    w.size.0,
                    w.size.1,
                    w.scale,
                );
                let kind = match w.kind {
                    WindowKind::Normal => "",
                    WindowKind::Dialog => " dialog",
                    WindowKind::Popup => " popup",
                    WindowKind::System => " system",
                    WindowKind::Background => " background",
                    WindowKind::Panel => " panel",
                };
                line.push_str(kind);
                if w.shown.is_some() {
                    line.push_str(" drawn");
                }
                for (on, word) in [
                    (focus == Some(*id), " focused"),
                    (w.restore.is_some(), " maximised"),
                    (w.minimised, " minimised"),
                    (w.dismissed, " dismissed"),
                    (w.attention, " attention"),
                    (c.is_some_and(|c| c.not_responding), " not-responding"),
                ] {
                    if on {
                        line.push_str(word);
                    }
                }
                lines.push(line);
            }
            let clip = &session.clipboard;
            let mut line = String::from("clipboard");
            if clip.text.is_some() {
                line.push_str(" text");
            }
            if clip.picture.is_some() {
                line.push_str(" picture");
            }
            if clip.forms() == 0 {
                line.push_str(" empty");
            }
            lines.push(line);
        }
        if self.pointer.seen {
            let (x, y) = self.pointer_pixel();
            let screen =
                self.space.in_space().find(|s| s.rect().contains(x, y)).map_or(0, |s| s.number);
            lines.push(format!("pointer at {x},{y} screen {screen}"));
        }
        if let Some(m) = &self.menu {
            // Where it is and its rows, so that a test can click one (row `i` is 2 pixels plus
            // `i` rows down from its top).
            let rows: Vec<String> = m.items.iter().map(|i| i.label()).collect();
            lines.push(format!(
                "menu for window {} of app {} at {},{} size {}x{}: {}",
                m.window.number,
                self.app_of(m.window),
                m.rect.x,
                m.rect.y,
                m.rect.width,
                m.rect.height,
                rows.join("|")
            ));
        }
        if let Some(f) = self.farewell {
            lines.push(String::from(match f {
                Farewell::ShuttingDown => "farewell",
                Farewell::Restarting => "farewell restarting",
                Farewell::Halted => "farewell halted",
            }));
        }
        lines.push(format!("view {view} done"));
        lines
    }

    fn app_of(&self, id: WindowId) -> u32 {
        self.clients.get(&id.client).map_or(0, |c| c.app)
    }
}
