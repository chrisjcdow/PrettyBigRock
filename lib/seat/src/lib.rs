// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The input service's rules (docs/design/0021-input-and-keyboard-layouts.md), as plain
//! computation that is built and tested on the host:
//!
//! - [`event`]: `pbr.seat`'s 32-byte events, for the service that writes them and the
//!   consumers that read them;
//! - [`seat`]: every keyboard and pointer merged into one stream, with the layout applied,
//!   the modifiers kept and key repeats made;
//! - [`outbox`]: what happens when the consumer's ring is full.
//!
//! `services/input` is the program around them: it finds the devices, reads their rings,
//! and serves `pbr.seat`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod event;
pub mod outbox;
pub mod seat;

pub use event::{EVENT_SIZE, Event};
pub use outbox::{Broken, Outbox, Sink};
pub use seat::{DeviceInfo, DeviceKind, Seat, Source};

#[cfg(test)]
mod tests;
