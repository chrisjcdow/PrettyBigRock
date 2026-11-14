// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The look: colours, corner radii, spacing and text styles (note 0023, section 5; note 0024,
//! section 6). One theme in M6, [`Theme::STANDARD`], carried over from PrettyBigRock 1.3
//! (`classic/kernel/wm.c`, `notepad.c` and `apps.c`): the cream window body, the grey buttons
//! with a light top half, white fields, the pink accent and the dark green of 1.3's title bars.
//!
//! Everything is in logical pixels; the window's canvas scales it.

use pbr_draw::{Colour, TextStyle};

/// How headings are drawn: the theme's one setting for them, which the desktop's start menu
/// uses too (`apps/desktop`). **Chris hasn't chosen yet** (asked 13 November 2026, again with
/// the gallery's pictures in M6 stage 7): the font at twice its size until he does.
pub const HEADING: TextStyle = TextStyle::PLAIN.sized(2);

/// The look of everything sandstone draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// A window's body (1.3's `C_BODY`).
    pub background: Colour,
    /// Text.
    pub text: Colour,
    /// Quieter text: a status bar, a hint, a column heading.
    pub text_dim: Colour,
    /// Text of something that can't be used now.
    pub text_disabled: Colour,
    /// The pink every highlight is made of: the focus ring, the text cursor, a pressed
    /// toggle.
    pub accent: Colour,
    /// Text on the accent.
    pub on_accent: Colour,
    /// 1.3's dark green (its title bars, the About window's labels): headings in the window.
    pub strong: Colour,
    /// Text that reports a failure.
    pub error: Colour,

    /// A button's edge...
    pub button_edge: Colour,
    /// ... while pressed.
    pub button_edge_pressed: Colour,
    /// Its face, top half...
    pub button_top: Colour,
    /// ... and bottom half.
    pub button_bottom: Colour,
    /// Its face while pressed (or a toggle while on).
    pub button_pressed: Colour,
    /// Its face under the pointer, top half (bottom half stays `button_bottom`).
    pub button_hover: Colour,

    /// A text field's or list's paper.
    pub field: Colour,
    /// Its edge.
    pub field_edge: Colour,
    /// Selected text, and a selected row, while the window has the keyboard...
    pub selection: Colour,
    /// ... and while it hasn't.
    pub selection_inactive: Colour,
    /// The margin beside a text editor's lines, and a list's column headings.
    pub gutter: Colour,
    /// The line between the margin and the text.
    pub gutter_edge: Colour,
    /// A text editor's line numbers.
    pub line_number: Colour,

    /// A toolbar's or menu bar's paper.
    pub bar: Colour,
    /// The line under a toolbar (or over a status bar).
    pub bar_edge: Colour,
    /// A menu's paper.
    pub menu: Colour,
    /// The menu row under the pointer, or chosen with the keyboard.
    pub menu_hover: Colour,
    /// The line between groups of menu rows.
    pub separator: Colour,
    /// A scroll bar's track...
    pub scroll_track: Colour,
    /// ... and its thumb.
    pub scroll_thumb: Colour,

    /// Corner radius of buttons and fields.
    pub radius: i32,
    /// Room between neighbours in rows and columns.
    pub spacing: i32,
    /// Room inside a dialog, a panel or a toolbar.
    pub padding: i32,
    /// A button's height (1.3's buttons were 24 high).
    pub control_height: i32,

    /// Ordinary text.
    pub font: TextStyle,
    /// Bold text (1.3's double strike).
    pub bold: TextStyle,
    /// Headings: dialogs' titles, the start menu's name. See [`HEADING`].
    pub heading: TextStyle,
}

impl Theme {
    /// PrettyBigRock's look.
    pub const STANDARD: Theme = Theme {
        background: Colour::solid(0xf5f2ec),
        text: Colour::solid(0x22252c),
        text_dim: Colour::solid(0x5a5f6a),
        text_disabled: Colour::solid(0x9a9ea8),
        accent: Colour::solid(0xd2577a),
        on_accent: Colour::solid(0xffffff),
        strong: Colour::solid(0x2f6b55),
        error: Colour::solid(0xb03a3a),

        button_edge: Colour::solid(0xb4b8c0),
        button_edge_pressed: Colour::solid(0x8d929c),
        button_top: Colour::solid(0xfdfdfd),
        button_bottom: Colour::solid(0xeceef1),
        button_pressed: Colour::solid(0xd9dbe0),
        button_hover: Colour::solid(0xf5f0f2),

        field: Colour::solid(0xffffff),
        field_edge: Colour::solid(0xa9adb6),
        selection: Colour::solid(0xf6dde6),
        selection_inactive: Colour::solid(0xe6e3de),
        gutter: Colour::solid(0xf3f1ec),
        gutter_edge: Colour::solid(0xe0ddd5),
        line_number: Colour::solid(0xa3a7b0),

        bar: Colour::solid(0xebe7df),
        bar_edge: Colour::solid(0xc9c4ba),
        menu: Colour::solid(0xf7f4ef),
        menu_hover: Colour::solid(0xe9dde3),
        separator: Colour::solid(0xd5d0d2),
        scroll_track: Colour::solid(0xebe7df),
        scroll_thumb: Colour::solid(0xb4b8c0),

        radius: 4,
        spacing: 6,
        padding: 8,
        control_height: 24,

        font: TextStyle::PLAIN,
        bold: TextStyle::BOLD,
        heading: HEADING,
    };

    /// How tall a line of ordinary text is.
    pub fn line_height(&self) -> i32 {
        self.font.line_height()
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::STANDARD
    }
}
