// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What sandstone needs from the display server, as a trait: the real one is `lib/window`'s
//! [`Connection`]; the test harness has a pretend one (`sandstone::testing`). Writing it as a
//! trait is what lets every widget be tested on the host, without a screen (note 0023,
//! section 6).

use alloc::string::String;
use alloc::vec::Vec;
use pbr_draw::{Canvas, Damage};
use pbr_window::{Connection, Cursor, Edge, Win, WindowFlags, WindowKind};

/// A window to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWindow {
    /// What it is.
    pub kind: WindowKind,
    /// Its parent (dialogs and popups), if any.
    pub parent: Option<Win>,
    /// The app's part of its title.
    pub title: String,
    /// Logical pixels.
    pub size: (u32, u32),
    /// Resizable, modal.
    pub flags: WindowFlags,
    /// For a popup, where in its parent it opens.
    pub at: (i32, i32),
    /// For a popup, the input event that asked for it.
    pub serial: u32,
    /// For a `System` window over another app's window: that app and its window (see
    /// [`crate::Settings::over`]).
    pub over: Option<(u32, u32)>,
}

/// The display server, as sandstone uses it. Failures that need no answer are ignored (a
/// window that has gone, a title that couldn't be set); those that do come back as `None` or
/// `false`.
pub trait Platform {
    /// The monotonic clock, in nanoseconds.
    fn now(&self) -> u64;
    /// Makes a window.
    fn create(&mut self, window: &NewWindow) -> Option<Win>;
    /// Destroys a window.
    fn destroy(&mut self, window: Win);
    /// Sets the app's part of a window's title.
    fn set_title(&mut self, window: Win, title: &str);
    /// Sets how small and large the user may make a window (0 for no limit).
    fn set_limits(&mut self, window: Win, min: (u32, u32), max: (u32, u32));
    /// Sets the pointer's shape over a window.
    fn set_cursor(&mut self, window: Win, cursor: Cursor);
    /// Draws in a window: `f` draws on a canvas over its frame and answers what it changed,
    /// in logical pixels. False if the window can't be drawn in now (no configure yet).
    fn draw(&mut self, window: Win, f: &mut dyn FnMut(&mut Canvas<'_>) -> Damage) -> bool;
    /// Copies text, with the serial of the key or button press that asked.
    fn copy_text(&mut self, serial: u32, text: &str) -> bool;
    /// Pastes text.
    fn paste_text(&mut self, serial: u32) -> Option<String>;
    /// Copies a picture (`0x00RRGGBB` pixels, row after row).
    fn copy_picture(&mut self, serial: u32, width: u32, height: u32, pixels: &[u32]) -> bool;
    /// Pastes a picture: its width, height and pixels.
    fn paste_picture(&mut self, serial: u32) -> Option<(u32, u32, Vec<u32>)>;
    /// Sets timer `id` to come at `at` (monotonic nanoseconds).
    fn set_timer(&mut self, id: u32, at: u64);
    /// Cancels a timer.
    fn cancel_timer(&mut self, id: u32);
    /// Starts moving a window with the pointer (from a press in its own content).
    fn start_move(&mut self, window: Win, serial: u32);
    /// Starts resizing a window from an edge.
    fn start_resize(&mut self, window: Win, edge: Edge, serial: u32);
    /// Minimises a window.
    fn minimise(&mut self, window: Win);
    /// Maximises a window (or restores it).
    fn maximise(&mut self, window: Win, on: bool);
    /// Asks for a frame event: a good time to draw an animation's next step.
    fn request_frame(&mut self, window: Win);
}

impl Platform for Connection {
    fn now(&self) -> u64 {
        granite::time::now()
    }

    fn create(&mut self, w: &NewWindow) -> Option<Win> {
        if let Some((app, window)) = w.over {
            return self.create_over(app, window, &w.title, w.size, w.flags).ok();
        }
        let parent = w.parent.unwrap_or(Win(0));
        self.create_child(w.kind, parent, &w.title, w.size, w.flags, w.at, w.serial).ok()
    }

    fn destroy(&mut self, window: Win) {
        let _ = Connection::destroy(self, window);
    }

    fn set_title(&mut self, window: Win, title: &str) {
        let _ = Connection::set_title(self, window, title);
    }

    fn set_limits(&mut self, window: Win, min: (u32, u32), max: (u32, u32)) {
        let _ = Connection::set_limits(self, window, min, max);
    }

    fn set_cursor(&mut self, window: Win, cursor: Cursor) {
        let _ = Connection::set_cursor(self, window, cursor);
    }

    fn draw(&mut self, window: Win, f: &mut dyn FnMut(&mut Canvas<'_>) -> Damage) -> bool {
        Connection::draw(self, window, f).unwrap_or(false)
    }

    fn copy_text(&mut self, serial: u32, text: &str) -> bool {
        Connection::copy_text(self, serial, text).is_ok()
    }

    fn paste_text(&mut self, serial: u32) -> Option<String> {
        Connection::paste_text(self, serial).ok()
    }

    fn copy_picture(&mut self, serial: u32, width: u32, height: u32, pixels: &[u32]) -> bool {
        Connection::copy_picture(self, serial, width, height, pixels).is_ok()
    }

    fn paste_picture(&mut self, serial: u32) -> Option<(u32, u32, Vec<u32>)> {
        Connection::paste_picture(self, serial).ok()
    }

    fn set_timer(&mut self, id: u32, at: u64) {
        Connection::set_timer(self, id, at);
    }

    fn cancel_timer(&mut self, id: u32) {
        Connection::cancel_timer(self, id);
    }

    fn start_move(&mut self, window: Win, serial: u32) {
        let _ = Connection::start_move(self, window, serial);
    }

    fn start_resize(&mut self, window: Win, edge: Edge, serial: u32) {
        let _ = Connection::start_resize(self, window, edge, serial);
    }

    fn minimise(&mut self, window: Win) {
        let _ = Connection::minimise(self, window);
    }

    fn maximise(&mut self, window: Win, on: bool) {
        let _ = Connection::maximise(self, window, on);
    }

    fn request_frame(&mut self, window: Win) {
        let _ = Connection::request_frame(self, window);
    }
}
