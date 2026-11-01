// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Screens (docs/design/0011-device-manager-and-drivers.md, section 8). Served by display
/// drivers; the frame buffer is the device's own memory, so drawing needs no copy.
interface pbr.display version 1;

/// How a pixel is laid out in memory, 32 bits each.
enum PixelFormat {
    /// Blue in the lowest byte, then green, red, and one unused byte.
    Bgrx @1,
    /// Red in the lowest byte, then green, blue, and one unused byte.
    Rgbx @2,
}

/// The screen.
struct DisplayInfo {
    /// Pixels across.
    width: u32,
    /// Pixels down.
    height: u32,
    /// Bytes from the start of one row to the start of the next.
    stride: u32,
    /// How each pixel is laid out.
    format: PixelFormat,
}

/// The screen's size and format.
@1 fn info() -> (info: DisplayInfo) error DisplayError;

/// The frame buffer, to map write-combining. Everyone who asks gets the same memory.
@2 fn framebuffer() -> (memory: handle<device_memory>) error DisplayError;

/// What can go wrong.
flexible enum DisplayError {
    /// There is no screen to draw on.
    NoScreen @1,
}
