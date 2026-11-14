// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Keyboard shortcuts: Ctrl+S, Ctrl+Shift+C, F5 (note 0023, section 5; note 0021, section 2).
//!
//! A shortcut with a character matches **the character the key types**, not its place on the
//! keyboard, so Ctrl+Z is the key marked Z on a German keyboard too. Letters match in either
//! case: Shift is a modifier of its own here (Ctrl+Shift+C is not Ctrl+C).

use crate::event::{Key, KeyPress, Modifiers};
use alloc::string::String;

/// What a shortcut presses: a character, or a named key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A key that types this character (compared in lower case).
    Char(char),
    /// A named key (F5, Delete...).
    Key(Key),
}

/// A shortcut: modifiers and a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// The modifiers that must be held (and no others of Shift, Ctrl, Alt and Meta).
    pub modifiers: Modifiers,
    /// The key.
    pub trigger: Trigger,
}

impl Shortcut {
    /// Ctrl and a character: `Shortcut::ctrl('s')`.
    pub const fn ctrl(c: char) -> Shortcut {
        Shortcut { modifiers: Modifiers(Modifiers::CTRL), trigger: Trigger::Char(c) }
    }

    /// Ctrl, Shift and a character: Terminal's copy is `Shortcut::ctrl_shift('c')`.
    pub const fn ctrl_shift(c: char) -> Shortcut {
        Shortcut {
            modifiers: Modifiers(Modifiers::CTRL | Modifiers::SHIFT),
            trigger: Trigger::Char(c),
        }
    }

    /// Alt and a character.
    pub const fn alt(c: char) -> Shortcut {
        Shortcut { modifiers: Modifiers(Modifiers::ALT), trigger: Trigger::Char(c) }
    }

    /// A named key with no modifiers: `Shortcut::key(Key::Function(5))`.
    pub const fn key(key: Key) -> Shortcut {
        Shortcut { modifiers: Modifiers::NONE, trigger: Trigger::Key(key) }
    }

    /// The same shortcut with other modifiers.
    pub const fn with(self, modifiers: Modifiers) -> Shortcut {
        Shortcut { modifiers, trigger: self.trigger }
    }

    /// Whether `press` is this shortcut.
    pub fn matches(&self, press: &KeyPress) -> bool {
        if press.modifiers.for_shortcuts() != self.modifiers {
            return false;
        }
        match self.trigger {
            Trigger::Key(k) => press.key == k,
            Trigger::Char(c) => press.ch.is_some_and(|got| lower(got) == lower(c)),
        }
    }

    /// How a menu shows it: "Ctrl+S", "Ctrl+Shift+C", "F5".
    pub fn label(&self) -> String {
        let mut s = String::new();
        let m = self.modifiers;
        for (on, name) in
            [(m.ctrl(), "Ctrl+"), (m.alt(), "Alt+"), (m.shift(), "Shift+"), (m.meta(), "Win+")]
        {
            if on {
                s.push_str(name);
            }
        }
        match self.trigger {
            Trigger::Char(c) => s.extend(c.to_uppercase()),
            Trigger::Key(k) => s.push_str(&key_name(k)),
        }
        s
    }
}

/// A character in lower case, when it has exactly one lower-case form.
fn lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// A named key as a menu shows it.
fn key_name(k: Key) -> String {
    let name = match k {
        Key::Enter => "Enter",
        Key::Escape => "Esc",
        Key::Backspace => "Backspace",
        Key::Tab => "Tab",
        Key::Space => "Space",
        Key::Delete => "Del",
        Key::Insert => "Ins",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDn",
        Key::Left => "Left",
        Key::Right => "Right",
        Key::Up => "Up",
        Key::Down => "Down",
        Key::Function(n) => return alloc::format!("F{n}"),
        Key::Character | Key::Modifier => "?",
    };
    String::from(name)
}
