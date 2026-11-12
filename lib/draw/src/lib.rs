// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drawing: pixels, shapes, text and pictures
//! (docs/design/0023-draw-window-and-sandstone.md, section 2).
//!
//! Everything that puts a picture together uses this library: apps (through `lib/window` and
//! sandstone), the display server composing windows, and the console. It draws on a
//! [`Canvas`], a view of 32-bit pixels that belong to someone else: a window's frame, a
//! screen's buffer or a picture in memory.
//!
//! Three rules hold for every call:
//! - **Nothing is drawn outside the canvas's clip rectangle**, and nothing panics, whatever
//!   positions and sizes are given: they may come from a program that lies.
//! - **Positions and sizes are in logical pixels**, multiplied by the canvas's scale, so an
//!   app draws the same at 1× and 2×.
//! - **Whole numbers only.** No floating point is used anywhere, so the same calls give
//!   exactly the same pixels on x86-64, on ARM64 and on the computer running the tests. That
//!   is what lets tests compare pictures pixel for pixel (`tests/pictures/draw`).
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, not the
//! standard library (which needs an operating system underneath), so it builds for programs,
//! for the kernel and for the host. `alloc` is the part of the standard library that needs
//! only a heap: `Vec` and friends.

#![no_std]

extern crate alloc;

mod canvas;
mod colour;
mod damage;
mod flood;
mod geometry;
mod line;
mod picture;
mod shapes;
mod text;

pub use canvas::{Canvas, MAX_SCALE, MAX_SIDE};
pub use colour::Colour;
pub use damage::{Damage, MAX_RECTS, MERGE_WASTE};
pub use geometry::Rect;
pub use picture::{Picture, Turn};
pub use text::TextStyle;
