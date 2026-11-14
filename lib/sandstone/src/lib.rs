// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sandstone: the toolkit PrettyBigRock's apps are written with
//! (docs/design/0023-draw-window-and-sandstone.md, section 5).
//!
//! An app is a type that implements [`App`]: it keeps its own state, describes its window from
//! that state in [`App::view`] as a tree of [`Element`]s (buttons, fields, rows and columns),
//! and changes its state in [`App::update`] when the user does something. Sandstone lays the
//! view out, draws it through the display server (`lib/window`, `lib/draw`), turns clicks and
//! keys into the app's messages, keeps the widgets' own small state between views (the focus,
//! a field's cursor, how far a list is scrolled), and draws again only what changed.
//!
//! ```ignore
//! struct Counter { n: i32 }
//! #[derive(Clone)]
//! enum Msg { Add }
//! impl App for Counter {
//!     type Message = Msg;
//!     fn view(&self) -> Element<'_, Msg> {
//!         Column::new().padding(8).spacing(6)
//!             .push(Label::new(&format!("{}", self.n)))
//!             .push(Button::new("Add").on_press(Msg::Add))
//!             .into()
//!     }
//!     fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
//!         match m { Msg::Add => self.n += 1 }
//!     }
//! }
//! ```
//!
//! The parts:
//! - [`element`]: elements, the [`Widget`] trait, and the state tree;
//! - [`layout`] and [`geometry`]: sizes, limits, rows and columns;
//! - [`event`] and [`shortcut`]: input as widgets see it;
//! - [`theme`]: the look, carried over from PrettyBigRock 1.3;
//! - [`widget`]: the widgets;
//! - [`dialog`]: messages, questions and lines to type, in dialog windows;
//! - the runtime (between the display server and the app) and [`run`];
//! - `testing` (with the `testing` feature): [`testing::Harness`], which runs an app against a
//!   pretend display server on the host, for tests.
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, so a
//! PrettyBigRock program can use it; the harness and the tests use the standard library.

#![cfg_attr(not(any(test, feature = "testing")), no_std)]

extern crate alloc;

pub mod app;
pub mod dialog;
pub mod element;
pub mod event;
pub mod geometry;
mod image;
pub mod layout;
mod platform;
mod run;
mod runtime;
mod shell;
pub mod shortcut;
#[cfg(feature = "testing")]
pub mod testing;
pub mod theme;
pub mod widget;

pub use app::{App, Context, Settings, TimerId, WindowId, WindowSpec};
pub use dialog::Dialog;
pub use element::{Describe, Description, DrawCx, Element, Identify, LayoutCx, Look, Tree, Widget};
pub use event::{Event, Key, KeyPress, Modifiers, Pointer, PointerKind, Role, Status};
pub use geometry::{Align, Length, Limits, Padding, Point, Rect, Size};
pub use image::Image;
pub use layout::Node;
pub use platform::{NewWindow, Platform};
#[cfg(target_os = "prettybigrock")]
pub use run::run;
pub use run::{RunError, run_on};
pub use runtime::Runtime;
pub use shell::Shell;
pub use shortcut::Shortcut;
pub use theme::Theme;
pub use widget::*;
