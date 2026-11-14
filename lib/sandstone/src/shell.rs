// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What a widget can ask for while it handles an event: send the app a message, take the
//! focus, be drawn again, open a popup, use the clipboard.

use crate::geometry::Point;
use crate::platform::Platform;
use crate::theme::Theme;
use alloc::string::String;
use alloc::vec::Vec;

/// A popup asked for by a widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopupAsk {
    /// Open one for the widget with this state number, at this point of the window.
    Open { owner: u64, at: Point },
    /// Close the open one.
    Close,
    /// Move to the next (+1) or previous (-1) popup of the same owner (a menu bar's
    /// neighbouring menu).
    Step(i32),
}

/// What the widgets of one window have asked for during one event: the window's part of a
/// [`Shell`], kept by sandstone.
#[derive(Debug, Default)]
pub(crate) struct Asks {
    /// The widget with the focus, and the one that asked for it.
    pub focus: Option<u64>,
    pub focus_asked: Option<u64>,
    /// Draw again; lay out again first.
    pub redraw: bool,
    pub relayout: bool,
    pub popup: Option<PopupAsk>,
    /// The serial of the key or button press being handled (0 for other events).
    pub serial: u32,
    /// The monotonic clock, in nanoseconds.
    pub now: u64,
    /// The window has the keyboard.
    pub window_focused: bool,
    /// The clipboard has text, a picture (as the last clipboard event said).
    pub clipboard: (bool, bool),
    /// The look in use (its font decides where a click lands in text).
    pub theme: Theme,
}

/// What a widget's [`on_event`](crate::Widget::on_event) is given to answer through.
pub struct Shell<'s, M> {
    messages: &'s mut Vec<M>,
    pub(crate) asks: &'s mut Asks,
    platform: &'s mut dyn Platform,
}

impl<'s, M> Shell<'s, M> {
    pub(crate) fn new(
        messages: &'s mut Vec<M>,
        asks: &'s mut Asks,
        platform: &'s mut dyn Platform,
    ) -> Shell<'s, M> {
        Shell { messages, asks, platform }
    }

    /// A shell for a widget with another message type (inside [`Element::map`](crate::Element::map)):
    /// the same window, collecting into `messages`.
    pub fn with_messages<'t, N>(&'t mut self, messages: &'t mut Vec<N>) -> Shell<'t, N> {
        Shell { messages, asks: &mut *self.asks, platform: &mut *self.platform }
    }

    /// Sends the app a message (after this event, sandstone gives it to `update`).
    pub fn publish(&mut self, message: M) {
        self.messages.push(message);
    }

    /// Whether the widget whose state number is `id` has the focus of this window.
    pub fn is_focused(&self, id: u64) -> bool {
        self.asks.focus == Some(id)
    }

    /// The state number of the widget with the focus of this window, if any (a container can
    /// tell whether the focus is inside it with [`Tree::contains`](crate::Tree::contains)).
    pub fn focused(&self) -> Option<u64> {
        self.asks.focus
    }

    /// Whether the widget has the focus and the window has the keyboard.
    pub fn has_keyboard(&self, id: u64) -> bool {
        self.asks.window_focused && self.is_focused(id)
    }

    /// Gives the focus to the widget whose state number is `id` (a click on a text field).
    pub fn focus(&mut self, id: u64) {
        self.asks.focus = Some(id);
        self.asks.focus_asked = Some(id);
        self.asks.redraw = true;
    }

    /// The widget's state changed how it looks: draw the window again (only what changed).
    pub fn redraw(&mut self) {
        self.asks.redraw = true;
    }

    /// The widget's state changed its size or its children's places (scrolling): lay the
    /// window out again, then draw it.
    pub fn relayout(&mut self) {
        self.asks.relayout = true;
        self.asks.redraw = true;
    }

    /// Opens a popup for the widget whose state number is `owner`, with its top left corner
    /// at `at` in the window. Sandstone asks the widget for the contents
    /// ([`Widget::popup`](crate::Widget::popup)) and sizes the popup to them. Only during a
    /// key or button press: the display server makes popups only in answer to one (note 0022,
    /// section 3). Any other popup of the window closes first.
    pub fn open_popup(&mut self, owner: u64, at: Point) {
        self.asks.popup = Some(PopupAsk::Open { owner, at });
    }

    /// Closes the window's popup.
    pub fn close_popup(&mut self) {
        self.asks.popup = Some(PopupAsk::Close);
    }

    /// From inside a popup: asks its owner to open the next (+1) or previous (-1) popup in
    /// its place (Left and Right in a menu move along the menu bar).
    pub fn step_popup(&mut self, step: i32) {
        self.asks.popup = Some(PopupAsk::Step(step));
    }

    /// The look in use: what a widget's layout and drawing were given, for working out where
    /// a click landed.
    pub fn theme(&self) -> &Theme {
        &self.asks.theme
    }

    /// The serial of the key or button press being handled, 0 for any other event.
    pub fn serial(&self) -> u32 {
        self.asks.serial
    }

    /// The monotonic clock, in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.asks.now / 1_000_000
    }

    /// Whether the clipboard has text (a field greys out Paste otherwise).
    pub fn clipboard_has_text(&self) -> bool {
        self.asks.clipboard.0
    }

    /// Copies text, in answer to the key or button press being handled. False if the display
    /// server refused (no press, or not in the last second).
    pub fn copy_text(&mut self, text: &str) -> bool {
        let serial = self.asks.serial;
        serial != 0 && self.platform.copy_text(serial, text)
    }

    /// Pastes text, in answer to the key or button press being handled.
    pub fn paste_text(&mut self) -> Option<String> {
        let serial = self.asks.serial;
        if serial == 0 {
            return None;
        }
        self.platform.paste_text(serial)
    }
}
