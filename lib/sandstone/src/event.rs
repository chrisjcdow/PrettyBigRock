// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Input as widgets see it: the display server's key and pointer events (`lib/window`, note
//! 0022 section 10), with positions in the window's logical pixels, keys named, and clicks
//! counted.

use crate::geometry::Point;
use pbr_interfaces::pbr_input as input;

/// Whether a widget used an event. A used event goes no further: not to other widgets, nor to
/// the shortcuts, nor to Tab and Enter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not for this widget.
    Ignored,
    /// Used.
    Captured,
}

/// The modifier keys held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub u16);

impl Modifiers {
    /// No modifier.
    pub const NONE: Modifiers = Modifiers(0);
    /// `pbr.seat`'s bits (note 0021, section 2).
    pub const SHIFT: u16 = 1;
    /// Ctrl.
    pub const CTRL: u16 = 2;
    /// Alt (the left one).
    pub const ALT: u16 = 4;
    /// AltGr (the right Alt, on layouts that use it).
    pub const ALTGR: u16 = 8;
    /// The Windows key.
    pub const META: u16 = 16;

    /// Shift is held.
    pub fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    /// Ctrl is held.
    pub fn ctrl(self) -> bool {
        self.0 & Self::CTRL != 0
    }

    /// Alt is held.
    pub fn alt(self) -> bool {
        self.0 & Self::ALT != 0
    }

    /// The Windows key is held.
    pub fn meta(self) -> bool {
        self.0 & Self::META != 0
    }

    /// Only the modifiers shortcuts care about: Shift, Ctrl, Alt and Meta (not AltGr, Caps
    /// Lock or Num Lock, which are part of typing a character).
    pub fn for_shortcuts(self) -> Modifiers {
        Modifiers(self.0 & (Self::SHIFT | Self::CTRL | Self::ALT | Self::META))
    }
}

/// A key, by what it does rather than where it is. Keys that type a character are
/// [`Key::Character`]: widgets look at the character itself (note 0021: shortcuts and typing
/// follow the letter on the key, whatever the layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Enter (Return).
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Tab.
    Tab,
    /// The space bar.
    Space,
    /// Delete.
    Delete,
    /// Insert.
    Insert,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// The arrows.
    Left,
    /// The arrows.
    Right,
    /// The arrows.
    Up,
    /// The arrows.
    Down,
    /// F1 to F12.
    Function(u8),
    /// Any other key: its character, if it types one, is in the event.
    Character,
    /// A modifier key on its own (Shift, Ctrl, Alt, the Windows key).
    Modifier,
}

impl Key {
    /// The key for `pbr.input`'s position code.
    pub fn from_code(code: u16) -> Key {
        match u32::from(code) {
            input::KEY_ENTER => Key::Enter,
            input::KEY_ESCAPE => Key::Escape,
            input::KEY_BACKSPACE => Key::Backspace,
            input::KEY_TAB => Key::Tab,
            input::KEY_SPACE => Key::Space,
            input::KEY_DELETE => Key::Delete,
            input::KEY_INSERT => Key::Insert,
            input::KEY_HOME => Key::Home,
            input::KEY_END => Key::End,
            input::KEY_PAGE_UP => Key::PageUp,
            input::KEY_PAGE_DOWN => Key::PageDown,
            input::KEY_LEFT => Key::Left,
            input::KEY_RIGHT => Key::Right,
            input::KEY_UP => Key::Up,
            input::KEY_DOWN => Key::Down,
            c @ input::KEY_F1..=input::KEY_F12 => Key::Function((c - input::KEY_F1 + 1) as u8),
            input::KEY_LEFT_SHIFT..=input::KEY_RIGHT_ALT => Key::Modifier,
            _ => Key::Character,
        }
    }
}

/// A key pressed (or repeating while held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// Which key, by what it does.
    pub key: Key,
    /// `pbr.input`'s position code, for widgets that want the key in the same place on every
    /// keyboard (a game's arrow keys).
    pub code: u16,
    /// The character it typed, if any (with Shift, AltGr and the layout applied; also given
    /// with Ctrl held, for shortcuts).
    pub ch: Option<char>,
    /// The character should be inserted: there is one, and no Ctrl, Alt or Meta is held.
    pub typed: bool,
    /// The modifiers held.
    pub modifiers: Modifiers,
    /// The key is repeating (held down), not freshly pressed.
    pub repeat: bool,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The left button.
    Left,
    /// The right button.
    Right,
    /// The middle button (the wheel pressed).
    Middle,
    /// Any other, by `pbr.input`'s number.
    Other(u16),
}

impl Button {
    /// The button for `pbr.input`'s number.
    pub fn from_code(code: u16) -> Button {
        match u32::from(code) {
            input::BUTTON_LEFT => Button::Left,
            input::BUTTON_RIGHT => Button::Right,
            input::BUTTON_MIDDLE => Button::Middle,
            _ => Button::Other(code),
        }
    }
}

/// What the pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// It moved (or came into the window).
    Moved,
    /// A button went down. `clicks` counts quick presses in the same place: 2 for a double
    /// click.
    Pressed {
        /// Which.
        button: Button,
        /// 1, 2, 3...
        clicks: u32,
    },
    /// A button went up.
    Released {
        /// Which.
        button: Button,
    },
    /// The wheel turned: `dy` steps down (positive) or up, `dx` right or left.
    Scrolled {
        /// Steps right.
        dx: i32,
        /// Steps down (the wheel turned towards the user).
        dy: i32,
    },
    /// It left the window.
    Left,
}

/// A pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointer {
    /// What happened.
    pub kind: PointerKind,
    /// Where, in the window's logical pixels (the last position, for `Left`).
    pub at: Point,
    /// The buttons held after it (bit n for `pbr.input`'s button n).
    pub buttons: u32,
    /// The modifiers held (Ctrl+click, Shift+click).
    pub modifiers: Modifiers,
}

impl Pointer {
    /// Whether this is a press of `button`; if so, how many clicks.
    pub fn pressed(&self, button: Button) -> Option<u32> {
        match self.kind {
            PointerKind::Pressed { button: b, clicks } if b == button => Some(clicks),
            _ => None,
        }
    }

    /// Whether this is a release of `button`.
    pub fn released(&self, button: Button) -> bool {
        matches!(self.kind, PointerKind::Released { button: b } if b == button)
    }
}

/// Which of a window's special buttons Enter or Escape stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The button Enter presses (Save, OK).
    Default,
    /// The button Escape presses (Cancel).
    Cancel,
}

/// An event, as given to [`Widget::on_event`](crate::Widget::on_event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pointer.
    Pointer(Pointer),
    /// A key pressed or repeating. Every widget sees it; the focused one acts on it (see
    /// [`Shell::is_focused`](crate::Shell::is_focused)).
    Key(KeyPress),
    /// A key let go: only for widgets that track keys held (a game on a `Canvas`).
    KeyReleased(KeyPress),
    /// Enter or Escape that nothing focused used: the window's default or cancel button
    /// presses itself.
    Activate(Role),
    /// F10, or a click on a menu bar's own key: the menu bar opens its first menu.
    MenuKey,
    /// The user asked to move from the open popup to the next (+1) or previous (-1) one: a
    /// menu bar opens its neighbour. Given to the parent window's widgets.
    PopupStep(i32),
    /// The popup this widget opened has closed (chosen from, dismissed, or Escape).
    PopupClosed,
    /// The window got (true) or lost the keyboard.
    WindowFocus(bool),
}
