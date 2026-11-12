// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Keyboard layouts (docs/design/0021-input-and-keyboard-layouts.md, section 4).
//!
//! A keyboard doesn't send letters: it sends *which key* was pressed, by its place on the
//! keyboard (a "key code", see `pbr_devices::keys`). A **layout** is the table that says what
//! each key types, with Shift, with AltGr (the right Alt key on layouts that have one), and
//! with Caps Lock and Num Lock. Layouts are small text files (`services/input/layouts/*.kbd`);
//! [`parse`] reads one, and [`Layout::key`] looks a key up.
//!
//! A **dead key** (an accent key such as the apostrophe on the US-International layout) types
//! nothing by itself: it waits for the next key and puts its accent on it. [`Composer`] keeps
//! that waiting.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod error;
mod layout;
mod parse;

pub use error::{ParseError, Problem};
pub use layout::Layout;
pub use parse::parse;

use pbr_devices::keys::key;

/// The largest layout file, in bytes.
pub const MAX_SIZE: usize = 16384;
/// The longest line of a layout file, in bytes.
pub const MAX_LINE: usize = 400;

/// What the modifier keys are doing when a key is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// A Shift key is held.
    pub shift: bool,
    /// AltGr is held (ignored by layouts with no `altgr` line).
    pub altgr: bool,
    /// Caps Lock is on.
    pub caps_lock: bool,
    /// Num Lock is on.
    pub num_lock: bool,
}

/// A dead key of one layout (an index into that layout's tables; only meaningful with the
/// layout it came from).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadKey(pub(crate) u8);

/// What a key press types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    /// Nothing: the key does not type, or not with these modifiers.
    Nothing,
    /// A character.
    Char(char),
    /// A dead key: nothing yet, but the next character gets its accent.
    Dead(DeadKey),
}

/// Whether Caps Lock swaps `lower` for `upper`: they must be a lower-case letter and its
/// capital, and a single character each, in the Latin, Greek or Cyrillic parts of Unicode.
///
/// So `a`/`A`, `é`/`É` and `ÿ`/`Ÿ` pair; `ß` does not (its capital is two letters, `SS`);
/// `1`/`!` and `a`/`B` do not. Caps Lock leaves digits and punctuation alone for this reason.
pub fn caps_pair(lower: char, upper: char) -> bool {
    // The blocks are Basic Latin to Latin Extended-B, Greek, Cyrillic and Cyrillic Supplement,
    // and Latin Extended Additional.
    let cased = |c: char| matches!(u32::from(c), 0x41..=0x24F | 0x370..=0x3FF | 0x400..=0x52F | 0x1E00..=0x1EFF);
    if lower == upper || !cased(lower) || !cased(upper) {
        return false;
    }
    // `to_uppercase` gives an iterator because some letters become several (`ß` is `SS`).
    let mut capital = lower.to_uppercase();
    capital.next() == Some(upper) && capital.next().is_none()
}

/// The editing key a keypad key is when Num Lock is off: `KP_7` is `HOME`, `KP_8` `UP`, `KP_9`
/// `PAGE_UP`, `KP_4` `LEFT`, `KP_6` `RIGHT`, `KP_1` `END`, `KP_2` `DOWN`, `KP_3` `PAGE_DOWN`,
/// `KP_0` `INSERT` and `KP_PERIOD` `DELETE`. `None` for every other code (`KP_5` included).
pub fn keypad_editing(code: u16) -> Option<u16> {
    Some(match code {
        key::KP_7 => key::HOME,
        key::KP_8 => key::UP,
        key::KP_9 => key::PAGE_UP,
        key::KP_4 => key::LEFT,
        key::KP_6 => key::RIGHT,
        key::KP_1 => key::END,
        key::KP_2 => key::DOWN,
        key::KP_3 => key::PAGE_DOWN,
        key::KP_0 => key::INSERT,
        key::KP_PERIOD => key::DELETE,
        _ => return None,
    })
}

/// Dead keys in progress: remembers a dead key until the next key that types.
///
/// The input service keeps one for the whole seat (`lib/seat`): a dead key pressed on one
/// keyboard waits for the next key on any.
#[derive(Clone, Copy, Debug, Default)]
pub struct Composer {
    waiting: Option<DeadKey>,
}

impl Composer {
    /// A composer with no dead key waiting.
    pub fn new() -> Composer {
        Composer::default()
    }

    /// Whether a dead key is waiting.
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// Forgets a waiting dead key (the layout changed, or an editing key was pressed).
    pub fn reset(&mut self) {
        self.waiting = None;
    }

    /// Takes what a key press typed (from [`Layout::key`] of the same layout) and gives the
    /// characters to type now: none, one or two, in order.
    ///
    /// A dead key waiting is settled by the next key that types: a letter it combines with
    /// becomes the accented letter; a space, or the same dead key again, types the accent
    /// alone; any other character types the accent and then itself; another dead key types
    /// the first one's accent and waits itself. A key that types nothing (Shift) leaves it
    /// waiting.
    pub fn feed(&mut self, layout: &Layout, typed: Typed) -> [Option<char>; 2] {
        let Some(dead) = self.waiting else {
            return match typed {
                Typed::Nothing => [None, None],
                Typed::Char(c) => [Some(c), None],
                Typed::Dead(d) => {
                    self.waiting = Some(d);
                    [None, None]
                }
            };
        };
        match typed {
            Typed::Nothing => [None, None],
            Typed::Char(c) => {
                self.waiting = None;
                match layout.combine(dead, c) {
                    Some(combined) => [Some(combined), None],
                    None => [Some(layout.accent(dead)), Some(c)],
                }
            }
            Typed::Dead(d) => {
                // The same dead key twice types its accent once; another one starts afresh.
                self.waiting = if d == dead { None } else { Some(d) };
                [Some(layout.accent(dead)), None]
            }
        }
    }
}

#[cfg(test)]
mod test_layouts;
#[cfg(test)]
mod test_parse;
#[cfg(test)]
mod test_typing;
