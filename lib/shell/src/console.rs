// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The terminal side of the shell (docs/design/0026-apps-m6-tests-and-stages.md, section 1).
//!
//! The shell's commands are the same wherever they are typed. What differs is the terminal:
//! the text shell (`apps/shell`) types on the console's screen through `pbr.terminal`, and
//! Terminal (`apps/terminal`) types in a window of its own. [`Console`] is the little set of
//! things the shell asks of either.
//!
//! In Rust a *trait* is a set of methods a type promises to have, and code written against a
//! trait (`Shell<C: Console>`) works with any type that has them, without knowing which. The
//! shell is run by one program or the other, never both at once, so the choice is made when
//! the program is built.

use alloc::string::String;

/// The terminal can't be used any more (the console has gone, or the window was closed). The
/// terminal's own code has logged why, if the user couldn't be told; the shell ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gone;

/// What the shell does with a terminal. Every method takes `&self`: a terminal is shared with
/// whatever shows it (a service's endpoint, or the queue to a window), and each method is
/// one whole request, so none needs the terminal to be changed in place.
pub trait Console {
    /// Shows `prompt`, then waits for the user to type a line and press Enter. The answer is
    /// the line without its line end. The prompt may be up to 256 bytes (`pbr.terminal`'s
    /// limit; the shell's prompt keeps to it).
    fn read_line(&self, prompt: &str) -> Result<String, Gone>;

    /// Shows `text` after what is already there. At most 4096 bytes: the shell cuts longer
    /// text into pieces first.
    fn write(&self, text: &str) -> Result<(), Gone>;

    /// Empties the screen and puts the cursor at the top left.
    fn clear(&self) -> Result<(), Gone>;
}
