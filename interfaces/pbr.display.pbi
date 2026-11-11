// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Screens (docs/design/0020-screens-and-virtio-gpu.md). Served by display drivers.
///
/// A display device has screens. A client draws in buffers the driver makes (32-bit pixels:
/// blue in the lowest byte, then green, red and one unused byte) and presents them, saying
/// which rectangles changed; the driver puts those on the screen and signals when it is done.
/// One client at a time presents: whoever last called `take`.
interface pbr.display version 2;

/// A rectangle of pixels in a buffer.
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// A size and refresh rate a screen can show.
struct Mode {
    /// Pixels across.
    width: u32,
    /// Pixels down.
    height: u32,
    /// Refreshes a second, times 1000; 0 if unknown.
    refresh_mhz: u32,
}

/// One screen: a connector that may or may not have something plugged in.
struct ScreenInfo {
    /// 0 up, on this device; the same for as long as the device is there.
    screen: u32,
    /// A name for the connector, the same at every start ("Virtual-1", "Firmware").
    connector: string<32>,
    /// From the screen's own description of itself (its maker and serial number) where it has
    /// one, so that a remembered arrangement finds the same screen again; empty if not.
    identity: string<64>,
    /// Something is plugged in.
    connected: bool,
    /// Its picture's width in millimetres, 0 if unknown.
    width_mm: u32,
    /// Its picture's height in millimetres, 0 if unknown.
    height_mm: u32,
    /// The mode it is showing; all 0 if it is off.
    current: Mode,
    /// The modes it can show, the one it prefers first.
    modes: list<Mode, 16>,
}

/// The screens, and a number that changes whenever anything about them does.
@1 fn screens() -> (screens: list<ScreenInfo, 16>, generation: u32) error DisplayError;

/// Become the client that presents. `taker` is the capability the operations below must be
/// called through; `done` gets bit n when screen n's last present is on the screen, and bit 63
/// when `screens()` would answer differently. Taking over ends the previous taker's
/// capability and buffers, so a client that crashed can't keep the screens.
@2 fn take() -> (taker: handle<pbr.display>, done: handle<notification>) error DisplayError;

/// Shows `mode` on a screen (one from its list, or any size the device can show), or turns
/// the screen off (width 0). Buffers made for another size are destroyed.
@3 fn set_mode(screen: u32, mode: Mode) -> () error DisplayError;

/// A buffer for a screen at its current size, to map read and write. `stride` is the bytes
/// from the start of one row to the start of the next.
@4 fn create_buffer(screen: u32) -> (buffer: u32, memory: handle<memory>, stride: u32)
    error DisplayError;

/// Destroys a buffer. If it is the one on the screen, the screen keeps its last picture.
/// A buffer whose present isn't done can't be destroyed yet (`Busy`).
@5 fn destroy_buffer(buffer: u32) -> () error DisplayError;

/// Shows a buffer on its screen, copying the rectangles in `changed` (all of it if the list
/// is empty). Answers at once; `done` says when the picture is on the screen, and until then
/// the buffer must not be drawn in.
@6 fn present(buffer: u32, changed: list<Rect, 64>) -> () error DisplayError;

/// What can go wrong.
flexible enum DisplayError {
    /// There is no such screen.
    NoScreen @1,
    /// Nothing is plugged in to that screen.
    NotConnected @2,
    /// The screen can't show that mode.
    BadMode @3,
    /// The driver has no memory left for buffers.
    NoMemory @4,
    /// There is no such buffer (or it was destroyed by a change of mode or taker).
    NoSuchBuffer @5,
    /// Not called through the current taker's capability.
    NotTaken @6,
    /// The previous present on that screen isn't done (or, destroying a buffer: its own).
    Busy @7,
    /// A rectangle outside the buffer, or a list too long.
    BadArgument @8,
}
