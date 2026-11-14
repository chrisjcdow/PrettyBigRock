// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sandstone's widgets (note 0023, section 5, the table "The widgets in M6").

mod button;
mod label;
mod layout;

pub use button::Button;
pub use label::Label;
pub use layout::{Column, Grid, Panel, Row, Space, Stretch};

// Shared with the widgets written on this pattern.
#[allow(unused_imports)]
pub(crate) use button::{draw_face, text_top};
#[allow(unused_imports)]
pub(crate) use label::fit;
