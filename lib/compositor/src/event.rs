// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `pbr.window`'s events: 32 bytes each, as they go into a client's ring
//! (docs/design/0022-display-server.md, section 10; `interfaces/pbr.window.pbi` has the table).
//!
//! The kinds and values are the interface file's constants, repeated here as `u16` and `u32`
//! so that this library and `lib/window` (which reads them) say the same thing in one place
//! each; a test checks they agree with the generated ones.

/// Bytes in one event.
pub const EVENT_SIZE: usize = 32;

/// Event kinds (`EVENT_*` in the interface file).
#[allow(missing_docs)] // each is described in the interface file
pub mod kind {
    pub const LOST: u16 = 0;
    pub const CONFIGURE: u16 = 1;
    pub const CLOSE: u16 = 2;
    pub const RELEASED: u16 = 3;
    pub const FRAME: u16 = 4;
    pub const PING: u16 = 5;
    pub const KEY: u16 = 6;
    pub const POINTER: u16 = 7;
    pub const LEFT: u16 = 8;
    pub const BUTTON: u16 = 9;
    pub const SCROLL: u16 = 10;
    pub const CLIPBOARD: u16 = 11;
    pub const DISCONNECTED: u16 = 12;
    pub const WINDOWS: u16 = 100;
    pub const START_MENU: u16 = 101;
    pub const WINDOW_MENU: u16 = 102;
    pub const SCREENS: u16 = 103;
}

/// Window states, in the high 16 bits of a configure event's third value.
#[allow(missing_docs)] // each is described in the interface file
pub mod state {
    pub const FOCUSED: u32 = 1;
    pub const MAXIMISED: u32 = 2;
    pub const MINIMISED: u32 = 4;
    pub const SHOWN: u32 = 8;
}

/// The value of a key event that carries a key's second character (`VALUE_SECOND`). Values 0
/// to 3 are `pbr.seat`'s: released, pressed, repeat, still held.
pub const VALUE_SECOND: u32 = 4;

/// One event, before it is laid out.
///
/// `#[derive(...)]` asks the compiler to write the obvious code for copying, comparing and
/// printing the type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// One of [`kind`].
    pub kind: u16,
    /// The client's own number for the window; 0 for the client as a whole.
    pub window: u32,
    /// For configure, key, button and ping events; otherwise 0.
    pub serial: u32,
    /// The first of three values, which mean what the kind says.
    pub a: u32,
    /// The second value.
    pub b: u32,
    /// The third value.
    pub c: u32,
    /// Nanoseconds on the monotonic clock.
    pub time: u64,
}

impl Event {
    /// An event of `kind` about `window` with its three values; no serial.
    pub fn new(kind: u16, window: u32, values: [u32; 3], time: u64) -> Event {
        Event { kind, window, serial: 0, a: values[0], b: values[1], c: values[2], time }
    }

    /// The same event with a serial.
    pub fn with_serial(mut self, serial: u32) -> Event {
        self.serial = serial;
        self
    }

    /// Laid out as the ring holds it: little-endian, with the reserved bytes 0.
    pub fn encode(&self) -> [u8; EVENT_SIZE] {
        let mut e = [0u8; EVENT_SIZE];
        e[0..2].copy_from_slice(&self.kind.to_le_bytes());
        e[4..8].copy_from_slice(&self.window.to_le_bytes());
        e[8..12].copy_from_slice(&self.serial.to_le_bytes());
        e[12..16].copy_from_slice(&self.a.to_le_bytes());
        e[16..20].copy_from_slice(&self.b.to_le_bytes());
        e[20..24].copy_from_slice(&self.c.to_le_bytes());
        e[24..32].copy_from_slice(&self.time.to_le_bytes());
        e
    }

    /// Read back from the ring's bytes. Any 32 bytes are some event: what the fields mean is
    /// for the reader to check.
    pub fn decode(e: &[u8; EVENT_SIZE]) -> Event {
        let u32_at = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
        let mut time = [0u8; 8];
        time.copy_from_slice(&e[24..32]);
        Event {
            kind: u16::from_le_bytes([e[0], e[1]]),
            window: u32_at(4),
            serial: u32_at(8),
            a: u32_at(12),
            b: u32_at(16),
            c: u32_at(20),
            time: u64::from_le_bytes(time),
        }
    }
}
