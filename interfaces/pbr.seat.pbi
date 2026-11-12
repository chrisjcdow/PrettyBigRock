// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Keyboards and pointers, merged, with the keyboard layout applied
/// (docs/design/0021-input-and-keyboard-layouts.md). Served by the input service.
///
/// A "seat" is the usual name for the keyboards and pointers one person uses at one screen. It
/// has one consumer at a time: the display server, or the console in text mode. The consumer
/// reads events from a shared ring made with `granite::ring`, each entry 32 bytes,
/// little-endian:
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | 0 | 2 | Kind (`EVENT_*` below) |
/// | 2 | 2 | The device it came from (the `id` in `devices()`) |
/// | 4 | 2 | Code: the key (`pbr.input`'s `KEY_*`), the button, or the axis |
/// | 6 | 2 | Modifiers held after this event (`MOD_*` below) |
/// | 8 | 4 | Value: for keys and buttons `pbr.input`'s `VALUE_*` (0 released, 1 pressed, 2 repeat) or `VALUE_HELD`; the amount for movement and scrolling (signed); x for positions; the number dropped for `EVENT_LOST` |
/// | 12 | 4 | For keys: the first character typed (a Unicode scalar value), 0 if none. For positions: y |
/// | 16 | 4 | For keys: a second character, 0 if none (a dead key followed by a letter it doesn't combine with types both) |
/// | 20 | 4 | Reserved, 0 |
/// | 24 | 8 | When it happened, nanoseconds on the monotonic clock |
///
/// The characters of a key event are what the key types with Shift, AltGr, Caps Lock and Num
/// Lock applied, as the layout says. They are given even when Ctrl, Alt or Meta is held, and
/// then they don't type: Ctrl+S carries `s`, so that a shortcut follows the letter printed on
/// the key whatever the layout. Releases carry no characters.
///
/// Who may call what is decided by the badge on the caller's capability, which the root
/// program chooses when a service looks `input` up (note 0021, section 1): a consumer may do
/// everything but `set_layout`; the users service may only ask about layouts and set one.
interface pbr.seat version 1;

/// Size of one event in the ring.
const EVENT_SIZE: u32 = 32;

/// Event kinds.
/// The consumer fell behind and `value` events were dropped. A reset follows.
const EVENT_LOST: u32 = 0;
const EVENT_KEY: u32 = 1;
/// Relative movement: `code` is `pbr.input`'s `AXIS_X` or `AXIS_Y`, `value` in device units.
const EVENT_MOVE: u32 = 2;
const EVENT_BUTTON: u32 = 3;
/// `code` is `AXIS_WHEEL` (up is positive) or `AXIS_HWHEEL` (right is positive), `value` in
/// steps.
const EVENT_SCROLL: u32 = 4;
/// An absolute pointer's place: `value` is x and the field at offset 12 is y, each 0 to 65535
/// across the device's whole surface.
const EVENT_POSITION: u32 = 5;
/// Assume nothing is held, on any device. Sent first after `take`, after a loss, and when a
/// device goes; followed by an event with `VALUE_HELD` for every key and button still held.
const EVENT_RESET: u32 = 6;
/// A device came or went: ask `devices()` again.
const EVENT_DEVICES: u32 = 7;

/// A value key and button events have besides `pbr.input`'s three: this key or button is
/// still held. Only after a reset, where it stands for the press the consumer saw before (or
/// never saw), so that a consumer that acts on presses doesn't act twice.
const VALUE_HELD: u32 = 3;

/// Modifier bits.
const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_ALT: u32 = 4;
/// The right Alt key, on layouts that use it for a third character on each key.
const MOD_ALTGR: u32 = 8;
/// The Windows key.
const MOD_META: u32 = 16;
/// Caps Lock is on.
const MOD_CAPS_LOCK: u32 = 32;
/// Num Lock is on.
const MOD_NUM_LOCK: u32 = 64;

/// What a device is.
enum DeviceKind { Keyboard @1, Mouse @2, Tablet @3, Touch @4 }

/// One device of the seat.
struct Device {
    /// The input service's number for it, which events carry. A device that goes and comes
    /// back (its driver was restarted) gets a new one.
    id: u16,
    kind: DeviceKind,
    /// Its driver's name for it.
    name: string<64>,
    /// The device manager's number for it (`pbr.devices`), which is what ties a tablet to a
    /// screen.
    device: u32,
}

/// A keyboard layout there is.
struct Layout {
    /// Its file's name in `layouts/`, without `.kbd`: what settings hold.
    name: string<32>,
    /// The name people see.
    title: string<64>,
}

/// Become the consumer: the ring of events and its doorbell (bit 0, signalled when events are
/// added). Taking over stops the previous consumer's ring and doorbell, so a consumer that
/// crashed can't keep the seat.
@1 fn take() -> (ring: handle<memory>, doorbell: handle<notification>) error SeatError;

/// The devices there are now.
@2 fn devices() -> (devices: list<Device, 32>) error SeatError;

/// The layout in use, and the ones there are.
@3 fn layouts() -> (current: string<32>, available: list<Layout, 32>) error SeatError;

/// Changes the layout (the users service only).
@4 fn set_layout(name: string<32>) -> () error SeatError;

/// What can go wrong.
flexible enum SeatError {
    /// This caller's capability doesn't allow that.
    NotAllowed @1,
    NoSuchLayout @2,
    /// The ring couldn't be made.
    NoRing @3,
}
