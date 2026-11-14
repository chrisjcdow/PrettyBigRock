// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The [`App`] trait every sandstone app implements, and the [`Context`] its `update` is given
//! (note 0023, section 5).
//!
//! An app keeps its own state; [`App::view`] describes its window from that state; sandstone
//! lays it out, draws it, and turns what the user does into the app's messages, which go to
//! [`App::update`]. This is the shape of Elm and of Rust's `iced`.

use crate::dialog::Dialog;
use crate::element::Element;
use crate::platform::Platform;
use crate::shortcut::Shortcut;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_window::Win;

/// One of the app's windows, as sandstone numbers them: [`WindowId::MAIN`] is the window the
/// app starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u32);

impl WindowId {
    /// The app's main window.
    pub const MAIN: WindowId = WindowId(0);
}

/// An app: its state, how it looks, and what it does.
///
/// `type Message` is an *associated type*: each app names its own message type (usually an
/// `enum` of everything its widgets can send). `Clone` because a widget keeps a message to
/// send each time it is used.
pub trait App {
    /// What the app's widgets send.
    type Message: Clone;

    /// What the main window shows now, built afresh from the app's state.
    fn view(&self) -> Element<'_, Self::Message>;

    /// What happens when the user does something, or a timer or task finishes.
    fn update(&mut self, message: Self::Message, cx: &mut Context<'_, Self::Message>);

    /// The app's part of the main window's title, after the manifest's ("Notepad — letter.txt"
    /// is the title "letter.txt"). Sandstone sets it again whenever it changes.
    fn title(&self) -> String {
        String::new()
    }

    /// Called once, after the main window opens: the place to move the focus
    /// ([`Context::focus`]) or start a timer.
    fn start(&mut self, _cx: &mut Context<'_, Self::Message>) {}

    /// What another window the app opened ([`Context::open_window`]) shows. Dialogs made with
    /// [`Context::show`] are sandstone's and don't come here.
    fn view_window(&self, _window: WindowId) -> Option<Element<'_, Self::Message>> {
        None
    }

    /// The title of another window the app opened.
    fn window_title(&self, _window: WindowId) -> String {
        String::new()
    }

    /// Shortcuts that work anywhere in the main window, with what they send. (A menu's items'
    /// shortcuts work without being listed here.)
    fn shortcuts(&self) -> Vec<(Shortcut, Self::Message)> {
        Vec::new()
    }

    /// The user asked to close a window (its close button, Ctrl+W). `None` closes it (the main
    /// window's closing ends the app); a message is sent instead to let the app ask first
    /// ("Save changes?"), and the app closes it with [`Context::close_window`] or
    /// [`Context::quit`].
    fn close_requested(&self, _window: WindowId) -> Option<Self::Message> {
        None
    }

    /// The clipboard changed: whether it has text, and a picture. A message, if the app wants
    /// one (to grey out Paste).
    fn clipboard_changed(&self, _text: bool, _picture: bool) -> Option<Self::Message> {
        None
    }

    /// Another thread of the app signalled bits 8 to 63 of the event loop's notification
    /// (note 0023, section 4). A message, if the app wants one.
    fn notified(&self, _bits: u64) -> Option<Self::Message> {
        None
    }
}

/// A window an app opens besides its main window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowSpec {
    /// A dialog (above its parent, the main window) rather than a normal window.
    pub dialog: bool,
    /// While it is open the main window takes no input (dialogs only).
    pub modal: bool,
    /// The user may resize it.
    pub resizable: bool,
    /// Logical pixels.
    pub size: (u32, u32),
}

/// What the app's main window starts as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Logical pixels.
    pub size: (u32, u32),
    /// The user may resize it.
    pub resizable: bool,
    /// The smallest the user may make it (0 for no limit).
    pub min_size: (u32, u32),
    /// Open the main window as a `System` window over another app's window, which it blocks
    /// input to while it is open: that app's number and its own number for the window. Only
    /// the file picker may (note 0025); the display server refuses anyone else.
    pub over: Option<(u32, u32)>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { size: (480, 360), resizable: true, min_size: (0, 0), over: None }
    }
}

/// A timer set with [`Context::after`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimerId(pub u32);

/// What `update` asked for, kept until sandstone acts on it after `update` returns.
pub(crate) struct Asked<M> {
    pub quit: bool,
    pub focus: Vec<(WindowId, String)>,
    pub dialogs: Vec<(WindowId, Dialog<M>)>,
    pub open: Vec<(WindowId, WindowSpec)>,
    pub close: Vec<WindowId>,
    pub next_window: u32,
}

impl<M> Asked<M> {
    pub fn new() -> Asked<M> {
        Asked {
            quit: false,
            focus: Vec::new(),
            dialogs: Vec::new(),
            open: Vec::new(),
            close: Vec::new(),
            next_window: 1,
        }
    }
}

/// What [`App::update`] is given: the way to open windows and dialogs, move the focus, use
/// the clipboard, set timers and end the app.
pub struct Context<'c, M> {
    pub(crate) asked: &'c mut Asked<M>,
    pub(crate) timers: &'c mut BTreeMap<u32, M>,
    pub(crate) next_timer: &'c mut u32,
    pub(crate) platform: &'c mut dyn Platform,
    /// The serial of the press that led to this message (0 if none).
    pub(crate) serial: u32,
    /// The window it happened in, and its display server number.
    pub(crate) window: WindowId,
    pub(crate) win: Option<Win>,
    pub(crate) clipboard: (bool, bool),
}

impl<M> Context<'_, M> {
    /// Ends the app (its windows close).
    pub fn quit(&mut self) {
        self.asked.quit = true;
    }

    /// Moves the focus to the element with id `id` in the main window (after this update's
    /// view is built).
    pub fn focus(&mut self, id: &str) {
        self.focus_in(WindowId::MAIN, id);
    }

    /// Moves the focus to the element with id `id` in `window`.
    pub fn focus_in(&mut self, window: WindowId, id: &str) {
        self.asked.focus.push((window, String::from(id)));
    }

    /// Shows a dialog over the main window: a message, a question, or a line to type
    /// ([`Dialog`]). What the user chooses comes back as a message.
    pub fn show(&mut self, dialog: Dialog<M>) -> WindowId {
        let id = self.new_window_id();
        self.asked.dialogs.push((id, dialog));
        id
    }

    /// Opens another window, whose contents come from [`App::view_window`].
    pub fn open_window(&mut self, spec: WindowSpec) -> WindowId {
        let id = self.new_window_id();
        self.asked.open.push((id, spec));
        id
    }

    /// Closes a window the app opened (or a dialog). Closing the main window ends the app.
    pub fn close_window(&mut self, window: WindowId) {
        if window == WindowId::MAIN {
            self.asked.quit = true;
        } else {
            self.asked.close.push(window);
        }
    }

    fn new_window_id(&mut self) -> WindowId {
        let id = WindowId(self.asked.next_window);
        self.asked.next_window = self.asked.next_window.wrapping_add(1).max(1);
        id
    }

    /// Sends `message` to `update` after `ms` milliseconds.
    pub fn after(&mut self, ms: u64, message: M) -> TimerId {
        let id = *self.next_timer;
        *self.next_timer = self.next_timer.wrapping_add(1).max(1);
        let at = self.platform.now().saturating_add(ms.saturating_mul(1_000_000));
        self.timers.insert(id, message);
        self.platform.set_timer(id, at);
        TimerId(id)
    }

    /// Cancels a timer that hasn't come yet.
    pub fn cancel(&mut self, timer: TimerId) {
        self.timers.remove(&timer.0);
        self.platform.cancel_timer(timer.0);
    }

    /// The monotonic clock, in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.platform.now() / 1_000_000
    }

    /// The serial of the key or button press this message came from, 0 if it didn't come from
    /// one (a timer). The clipboard and the file picker need it.
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// The window this message came from.
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// The display server's number for that window (for the file picker, note 0025).
    pub fn display_window(&self) -> Option<Win> {
        self.win
    }

    /// Whether the clipboard has text, and a picture.
    pub fn clipboard(&self) -> (bool, bool) {
        self.clipboard
    }

    /// Copies text, in answer to the press this message came from. False if the display
    /// server refused.
    pub fn copy_text(&mut self, text: &str) -> bool {
        self.serial != 0 && self.platform.copy_text(self.serial, text)
    }

    /// Pastes text, in answer to the press this message came from.
    pub fn paste_text(&mut self) -> Option<String> {
        if self.serial == 0 {
            return None;
        }
        self.platform.paste_text(self.serial)
    }

    /// Copies a picture (`0x00RRGGBB` pixels, row after row).
    pub fn copy_picture(&mut self, width: u32, height: u32, pixels: &[u32]) -> bool {
        self.serial != 0 && self.platform.copy_picture(self.serial, width, height, pixels)
    }

    /// Pastes a picture: its width, height and pixels.
    pub fn paste_picture(&mut self) -> Option<(u32, u32, Vec<u32>)> {
        if self.serial == 0 {
            return None;
        }
        self.platform.paste_picture(self.serial)
    }

    /// Starts moving the window this message came from with the pointer.
    pub fn start_move(&mut self) {
        if let Some(w) = self.win {
            self.platform.start_move(w, self.serial);
        }
    }
}
