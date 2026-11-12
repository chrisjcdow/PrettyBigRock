// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A layout once it has been read: what every key types at each of its four levels.

use crate::{DeadKey, Mods, Typed, caps_pair};
use alloc::string::String;
use alloc::vec::Vec;
use pbr_devices::keys::key;

/// The number of levels a key has: plain, Shift, AltGr, AltGr with Shift.
pub(crate) const LEVELS: usize = 4;

/// One dead key's table: what it puts its accent on.
#[derive(Clone, Debug)]
pub(crate) struct DeadTable {
    /// The name the file gave it (`acute`). Kept for people reading a debug print.
    #[allow(dead_code)] // read by `Debug` output and the tests only
    pub(crate) name: String,
    /// Pairs of (letter, combined character). The letter is a space for the accent itself.
    pub(crate) pairs: Vec<(char, char)>,
}

/// A keyboard layout: the name people see, whether the right Alt key is AltGr, and what every
/// key types. Made by [`crate::parse`], which has checked it all.
///
/// `#[derive(Clone, Debug)]` asks the compiler to write the code that copies a layout and
/// prints it for debugging.
#[derive(Clone, Debug)]
pub struct Layout {
    pub(crate) title: String,
    pub(crate) altgr: bool,
    /// What each key code types at each level, indexed by the code. Keys the file leaves out
    /// are all `Nothing`, and so is code 0. A vector the length of the key table is a few
    /// hundred bytes, and finding a key is one lookup.
    pub(crate) slots: Vec<[Typed; LEVELS]>,
    pub(crate) dead: Vec<DeadTable>,
}

impl Layout {
    /// The name people see (`English (US)`).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the right Alt key is AltGr.
    pub fn has_altgr(&self) -> bool {
        self.altgr
    }

    /// What the key `code` (a [`pbr_devices::keys::key`] number) types with `mods`.
    ///
    /// Caps Lock swaps a letter with its capital, but leaves digits and punctuation alone; Num
    /// Lock decides whether the keypad's digits type at all (when it is off they are editing
    /// keys, and the caller handles that with [`crate::keypad_editing`]). Codes the layout
    /// does not mention, and numbers that are no key, type nothing.
    pub fn key(&self, code: u16, mods: Mods) -> Typed {
        let Some(levels) = self.slots.get(usize::from(code)) else {
            return Typed::Nothing;
        };
        if (key::KP_0..=key::KP_PERIOD).contains(&code) && !mods.num_lock {
            return Typed::Nothing;
        }
        let on_keypad = (key::KP_0..=key::KP_DIVIDE).contains(&code);
        // A layout with no AltGr key ignores the modifier, so a program that asks with it set
        // gets the plain answer.
        let first = if mods.altgr && self.altgr { 2 } else { 0 };
        if mods.caps_lock
            && !on_keypad
            && let (Typed::Char(lower), Typed::Char(upper)) = (levels[first], levels[first + 1])
            && caps_pair(lower, upper)
        {
            return Typed::Char(if mods.shift { lower } else { upper });
        }
        levels[first + usize::from(mods.shift)]
    }

    /// The accent a dead key types by itself (its table's `space` pair).
    ///
    /// A [`DeadKey`] only means something with the layout it came from. With another layout's
    /// it gives a space rather than failing.
    pub fn accent(&self, dead: DeadKey) -> char {
        self.combine(dead, ' ').unwrap_or(' ')
    }

    /// What `c` typed after `dead` becomes: the combined character, or `None` if they don't
    /// combine. (A space gives the accent.)
    pub fn combine(&self, dead: DeadKey, c: char) -> Option<char> {
        let table = self.dead.get(usize::from(dead.0))?;
        table.pairs.iter().find(|&&(base, _)| base == c).map(|&(_, result)| result)
    }
}
