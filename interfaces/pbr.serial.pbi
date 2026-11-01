// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Serial ports (docs/design/0011-device-manager-and-drivers.md, section 8). Served by serial
/// drivers.
interface pbr.serial version 1;

/// The most bytes one call moves.
const MAX_TRANSFER: u32 = 4096;

/// Sends bytes. Answers once all of them are queued or sent; `written` can be fewer if the
/// port stays busy.
@1 fn write(data: bytes<4096>) -> (written: u32) error SerialError;

/// Up to `max` bytes that have arrived. Answers at once, with nothing if nothing has.
@2 fn read(max: u32) -> (data: bytes<4096>) error SerialError;

/// A notification that is signalled (bit 0) when `read` has something.
@3 fn events() -> (readable: handle<notification>) error SerialError;

/// What can go wrong.
flexible enum SerialError {
    /// Bytes arrived faster than they were read, and some were lost.
    Overrun @1,
    /// A size was too big.
    BadArgument @2,
}
