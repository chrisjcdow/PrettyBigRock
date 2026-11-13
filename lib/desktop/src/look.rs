// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! 1.3's colours (`classic/kernel/wm.c`), as [`Colour`]s the rest of the crate draws with.
//!
//! A `const` is a value the compiler works out once and pastes in wherever it is used, so these
//! cost nothing at run time. `Colour::solid(0xRRGGBB)` is a colour that is not see-through.
//! The names follow 1.3's where it had them (`C_ACCENT`, `C_BAR_A`, ...).

use pbr_draw::Colour;

/// The pink every highlight is made of: the start button, the banner of the start menu, the
/// line under the focused window's button.
pub const C_ACCENT: Colour = Colour::solid(0xd2577a);
/// The accent when pressed (the start button while the menu is open).
pub const C_ACCENT_PRESSED: Colour = Colour::solid(0xa83d5d);
/// The taskbar's colour along its top...
pub const C_BAR_A: Colour = Colour::solid(0x2a2e3d);
/// ... and along its bottom.
pub const C_BAR_B: Colour = Colour::solid(0x171a24);
/// The one-pixel line along the taskbar's top edge.
pub const C_BAR_EDGE: Colour = Colour::solid(0x464c63);

/// A window's button on the taskbar: the focused window's...
pub const C_TASK_FOCUSED: Colour = Colour::solid(0x48506c);
/// ... any other open window's...
pub const C_TASK_NORMAL: Colour = Colour::solid(0x323748);
/// ... and a minimised window's.
pub const C_TASK_MINIMISED: Colour = Colour::solid(0x232633);
/// The title on a taskbar button.
pub const C_TASK_TEXT: Colour = Colour::solid(0xeef0f6);
/// The title on a minimised window's button.
pub const C_TASK_TEXT_MINIMISED: Colour = Colour::solid(0x8d93a6);
/// The time, and the start button's name.
pub const C_WHITE: Colour = Colour::solid(0xffffff);
/// The date under the time.
pub const C_CLOCK_DATE: Colour = Colour::solid(0xa9afc2);

/// The start menu's paper.
pub const C_MENU: Colour = Colour::solid(0xf7f4ef);
/// The start menu banner's colour at its top (the accent)...
pub const C_BANNER_A: Colour = Colour::solid(0xd2577a);
/// ... and at its bottom.
pub const C_BANNER_B: Colour = Colour::solid(0x6a2a52);
/// The row under the pointer in the start menu.
pub const C_MENU_HOVER: Colour = Colour::solid(0xe9dde3);
/// A start menu row's label.
pub const C_MENU_TEXT: Colour = Colour::solid(0x23262e);
/// The line between the apps and Restart / Shut down.
pub const C_MENU_SEPARATOR: Colour = Colour::solid(0xd5d0d2);

/// The shadow of a desktop icon's label, so that white text reads on any wallpaper.
pub const C_LABEL_SHADOW: Colour = Colour::solid(0x101018);
/// The outline of the selected desktop icon...
pub const C_SELECT_EDGE: Colour = Colour::solid(0xf3d9e2);
/// ... and the box inside it: white, a quarter solid (1.3's alpha 60 of 256).
pub const C_SELECT_FILL: Colour = Colour::argb(60, 0xff, 0xff, 0xff);
