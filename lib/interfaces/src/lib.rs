// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

//! The code generated from the interface files in `interfaces/`: one module per interface,
//! named after it (`pbr.test.echo` is [`pbr_test_echo`]). See
//! docs/design/0008-messages-and-interfaces.md, section 9, and `cargo xtask interfaces` to
//! read it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

include!(concat!(env!("OUT_DIR"), "/interfaces.rs"));
