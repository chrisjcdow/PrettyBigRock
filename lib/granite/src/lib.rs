// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `granite`: PrettyBigRock's native library, the bedrock every program is built on
//! (docs/design/0009-programs-and-granite.md, section 6).
//!
//! A program looks like this:
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//!
//! granite::main!(main);
//!
//! fn main(start: granite::Start) -> i32 {
//!     granite::println!("Hello from a program");
//!     0
//! }
//! ```
//!
//! Everything here is safe to use: the few unsafe operations (system calls, the message page,
//! mapped memory) are wrapped. Nothing panics on anything another program can cause; those are
//! errors.
//!
//! This library is under the Mozilla Public License 2.0, so programs under any licence can use
//! it (DESIGN.md, D16).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod endpoint;
pub mod handle;
#[cfg(target_os = "prettybigrock")]
mod heap;
pub mod lock;
pub mod log;
pub mod memory;
pub mod message;
pub mod notification;
pub mod process;
pub mod ring;
#[cfg(target_os = "prettybigrock")]
pub mod start;
pub mod sys;
pub mod thread;
pub mod time;

pub use handle::Handle;
#[cfg(target_os = "prettybigrock")]
pub use start::Start;
pub use sys::{Error, Result};
