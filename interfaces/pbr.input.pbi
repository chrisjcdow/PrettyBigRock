// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Keyboards, mice and tablets (docs/design/0011-device-manager-and-drivers.md, section 8;
/// tablets: note 0021, section 7). Served by
/// input drivers.
///
/// Events reach the client through a shared ring made with `granite::ring`, each entry 16 bytes,
/// little-endian: `kind` u16, `code` u16, `value` i32, `time` u64 (nanoseconds). If the client
/// falls behind and the ring fills, the driver drops new events instead, and the next event it
/// can write has kind 0 and the number dropped in `value`.
interface pbr.input version 1;

/// What the device is.
enum InputKind {
    Keyboard @1,
    Mouse @2,
    /// Sends both: the PS/2 controller is one device with a keyboard and a mouse on it.
    KeyboardAndMouse @3,
    /// An absolute pointer: it says where it is, not how far it moved. A touch screen is one.
    Tablet @4,
}

/// Size of one event in the ring.
const EVENT_SIZE: u32 = 16;

/// Event kinds.
const EVENT_LOST: u32 = 0;
const EVENT_KEY: u32 = 1;
const EVENT_MOVE: u32 = 2;
const EVENT_BUTTON: u32 = 3;
const EVENT_SCROLL: u32 = 4;
/// An absolute pointer's position (a `Tablet`'s). `code` is 0. Each event is only 16 bytes, so
/// `value` carries both coordinates: x in its low 16 bits and y in its high 16 bits, each from
/// 0 to 65535 across the device's whole surface (read `value` as unsigned: x is `value & 0xFFFF`
/// and y is `(value >> 16) & 0xFFFF`). A driver posts one event per report, so x and y always
/// belong together.
const EVENT_POSITION: u32 = 5;

/// Values of key and button events (scroll and move events carry a signed amount).
const VALUE_RELEASED: u32 = 0;
const VALUE_PRESSED: u32 = 1;
const VALUE_REPEAT: u32 = 2;

/// Codes of move and scroll events.
const AXIS_X: u32 = 1;
const AXIS_Y: u32 = 2;
/// The wheel: up (away from the user) is positive.
const AXIS_WHEEL: u32 = 3;
/// A sideways wheel or a tilting one: right is positive.
const AXIS_HWHEEL: u32 = 4;

/// Codes of button events.
const BUTTON_LEFT: u32 = 1;
const BUTTON_RIGHT: u32 = 2;
const BUTTON_MIDDLE: u32 = 3;

/// Codes of key events: PrettyBigRock's own numbering, by position on a keyboard, not by what
/// is printed on the key.
const KEY_A: u32 = 1;
const KEY_B: u32 = 2;
const KEY_C: u32 = 3;
const KEY_D: u32 = 4;
const KEY_E: u32 = 5;
const KEY_F: u32 = 6;
const KEY_G: u32 = 7;
const KEY_H: u32 = 8;
const KEY_I: u32 = 9;
const KEY_J: u32 = 10;
const KEY_K: u32 = 11;
const KEY_L: u32 = 12;
const KEY_M: u32 = 13;
const KEY_N: u32 = 14;
const KEY_O: u32 = 15;
const KEY_P: u32 = 16;
const KEY_Q: u32 = 17;
const KEY_R: u32 = 18;
const KEY_S: u32 = 19;
const KEY_T: u32 = 20;
const KEY_U: u32 = 21;
const KEY_V: u32 = 22;
const KEY_W: u32 = 23;
const KEY_X: u32 = 24;
const KEY_Y: u32 = 25;
const KEY_Z: u32 = 26;
const KEY_1: u32 = 27;
const KEY_2: u32 = 28;
const KEY_3: u32 = 29;
const KEY_4: u32 = 30;
const KEY_5: u32 = 31;
const KEY_6: u32 = 32;
const KEY_7: u32 = 33;
const KEY_8: u32 = 34;
const KEY_9: u32 = 35;
const KEY_0: u32 = 36;
const KEY_ENTER: u32 = 37;
const KEY_ESCAPE: u32 = 38;
const KEY_BACKSPACE: u32 = 39;
const KEY_TAB: u32 = 40;
const KEY_SPACE: u32 = 41;
const KEY_MINUS: u32 = 42;
const KEY_EQUALS: u32 = 43;
const KEY_LEFT_BRACKET: u32 = 44;
const KEY_RIGHT_BRACKET: u32 = 45;
const KEY_BACKSLASH: u32 = 46;
const KEY_SEMICOLON: u32 = 47;
const KEY_APOSTROPHE: u32 = 48;
const KEY_GRAVE: u32 = 49;
const KEY_COMMA: u32 = 50;
const KEY_PERIOD: u32 = 51;
const KEY_SLASH: u32 = 52;
const KEY_CAPS_LOCK: u32 = 53;
const KEY_F1: u32 = 54;
const KEY_F2: u32 = 55;
const KEY_F3: u32 = 56;
const KEY_F4: u32 = 57;
const KEY_F5: u32 = 58;
const KEY_F6: u32 = 59;
const KEY_F7: u32 = 60;
const KEY_F8: u32 = 61;
const KEY_F9: u32 = 62;
const KEY_F10: u32 = 63;
const KEY_F11: u32 = 64;
const KEY_F12: u32 = 65;
const KEY_INSERT: u32 = 66;
const KEY_DELETE: u32 = 67;
const KEY_HOME: u32 = 68;
const KEY_END: u32 = 69;
const KEY_PAGE_UP: u32 = 70;
const KEY_PAGE_DOWN: u32 = 71;
const KEY_RIGHT: u32 = 72;
const KEY_LEFT: u32 = 73;
const KEY_DOWN: u32 = 74;
const KEY_UP: u32 = 75;
const KEY_LEFT_SHIFT: u32 = 76;
const KEY_RIGHT_SHIFT: u32 = 77;
const KEY_LEFT_CTRL: u32 = 78;
const KEY_RIGHT_CTRL: u32 = 79;
const KEY_LEFT_ALT: u32 = 80;
const KEY_RIGHT_ALT: u32 = 81;
const KEY_LEFT_META: u32 = 82;
const KEY_RIGHT_META: u32 = 83;
/// The key beside the left Shift key on European keyboards (ISO), which US keyboards lack.
const KEY_102ND: u32 = 84;
/// The keypad. What its keys type depends on Num Lock, which the input service keeps (note 0021).
const KEY_KP_0: u32 = 85;
const KEY_KP_1: u32 = 86;
const KEY_KP_2: u32 = 87;
const KEY_KP_3: u32 = 88;
const KEY_KP_4: u32 = 89;
const KEY_KP_5: u32 = 90;
const KEY_KP_6: u32 = 91;
const KEY_KP_7: u32 = 92;
const KEY_KP_8: u32 = 93;
const KEY_KP_9: u32 = 94;
const KEY_KP_PERIOD: u32 = 95;
const KEY_KP_PLUS: u32 = 96;
const KEY_KP_MINUS: u32 = 97;
const KEY_KP_MULTIPLY: u32 = 98;
const KEY_KP_DIVIDE: u32 = 99;
const KEY_KP_ENTER: u32 = 100;
const KEY_NUM_LOCK: u32 = 101;
const KEY_SCROLL_LOCK: u32 = 102;
const KEY_PRINT_SCREEN: u32 = 103;
const KEY_PAUSE: u32 = 104;
/// The menu key, beside the right Ctrl key.
const KEY_MENU: u32 = 105;

/// What the device is, and its name.
@1 fn info() -> (kind: InputKind, name: string<64>);

/// Starts the flow of events: the ring (made by the driver) and a notification signalled
/// (bit 0) after events are added. One client at a time: opening again takes the device over,
/// and the previous client's ring and doorbell stop working (so a client that crashed can't
/// lock everyone out).
@2 fn open() -> (ring: handle<memory>, doorbell: handle<notification>) error InputError;

/// What can go wrong.
flexible enum InputError {
    /// The ring couldn't be made.
    NoRing @1,
}
