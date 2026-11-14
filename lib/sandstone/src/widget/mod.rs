// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sandstone's widgets (note 0023, section 5, the table "The widgets in M6").
//!
//! Each group of widgets is a module of its own; everything public in one is part of
//! sandstone's API (`*` brings in every public name of a module).

mod button;
mod buttons;
mod label;
mod layout;
mod list;
mod menu;
pub mod scrollbar;
mod terminal;
pub mod text_editor;
mod text_field;

pub use button::Button;
pub use buttons::*;
pub use label::Label;
pub use layout::{Column, Grid, Panel, Row, Space, Stretch};
pub use list::*;
pub use menu::*;
pub use terminal::*;
pub use text_editor::*;
pub use text_field::*;

// Drawing shared by the widgets written on Button's pattern.
#[allow(unused_imports)]
pub(crate) use button::{draw_face, text_top};
#[allow(unused_imports)]
pub(crate) use label::fit;
