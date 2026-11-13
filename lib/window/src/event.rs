// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Events: what the display server tells an app, and how the 32 bytes it puts in the ring
//! become an [`Event`] (docs/design/0022-display-server.md, section 10).
//!
//! Nothing here makes a system call, so all of it is tested on the host. The ring is shared
//! with another program, so the bytes may be anything at all: [`decode`] never panics, and
//! skips what it does not know (the display server may be newer than this library).

use pbr_interfaces::pbr_window as wire;

/// The `pbr.seat` modifier bits, as they come in a key event.
const MOD_CTRL: u16 = 2;
const MOD_ALT: u16 = 4;
const MOD_META: u16 = 16;

/// A window, named by the number the display server gave it. `Copy` (a plain number that can
/// be copied freely, unlike a capability) because it is only a name: the window's state lives
/// in the [`Connection`](crate::Connection).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Win(pub u32);

impl Win {
    /// The display server's number for the window.
    pub fn number(self) -> u32 {
        self.0
    }
}

/// A key press, repeat or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The window with the focus.
    pub window: Win,
    /// `pbr.seat`'s code for the key.
    pub code: u16,
    /// The modifiers held (`pbr.seat`'s `MOD_*` bits).
    pub modifiers: u16,
    /// 0 released, 1 pressed, 2 repeat, 3 held (after a reset), 4 a second character.
    pub value: u32,
    /// The character the key typed, if any.
    pub ch: Option<char>,
    /// Whether a text field should insert `ch`: see [`is_typed`].
    pub typed: bool,
    /// Quote this to copy, paste or start a move (good for one second).
    pub serial: u32,
}

/// What a pointer event is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// Moved, or entered the window.
    Moved,
    /// Left the window.
    Left,
    /// A button went down or up.
    Button {
        /// `pbr.input`'s button number.
        button: u16,
        /// Down (true) or up (false).
        pressed: bool,
    },
    /// Scrolling.
    Scroll {
        /// `pbr.input`'s axis number.
        axis: u16,
        /// How far, signed.
        steps: i32,
    },
}

/// A pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerEvent {
    /// The window it is over (or was pressed in).
    pub window: Win,
    /// What happened.
    pub what: PointerKind,
    /// Where, in whole logical pixels from the window's content corner (rounded down, so
    /// just left of the window it is -1).
    pub x: i32,
    /// As `x`, downwards.
    pub y: i32,
    /// `x` in 1/256 logical pixels.
    pub raw_x: i32,
    /// `y` in 1/256 logical pixels.
    pub raw_y: i32,
    /// The buttons held (bit n for button n); 0 for scrolling.
    pub buttons: u32,
    /// Button events only: quote it to start a move or a resize, or to paste.
    pub serial: u32,
}

/// Something that happened, for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A window's size, scale or state. Its buffers are already replaced if they needed to
    /// be: draw again.
    Configure {
        /// The window.
        window: Win,
        /// Logical pixels.
        size: (u32, u32),
        /// Hundredths: 100 is 1x.
        scale: u32,
        /// Has the keyboard.
        focused: bool,
        /// Maximised.
        maximised: bool,
        /// Minimised.
        minimised: bool,
        /// On some screen.
        shown: bool,
        /// The event's serial.
        serial: u32,
    },
    /// The user asked to close the window (nothing closes until it is destroyed).
    CloseRequested {
        /// The window.
        window: Win,
    },
    /// A key.
    Key(KeyEvent),
    /// The pointer.
    Pointer(PointerEvent),
    /// A good time to draw (after `request_frame`).
    Frame {
        /// The window.
        window: Win,
    },
    /// The clipboard changed.
    Clipboard {
        /// There is text.
        text: bool,
        /// There is a picture.
        picture: bool,
    },
    /// This many events were lost: draw everything again and assume no key is held.
    Lost(u32),
    /// A timer set with `set_timer` has come.
    Timer(u32),
    /// The app's own bits (8 to 63), as signalled, were set.
    Notified(u64),
    /// The display server has gone. Given once.
    Disconnected,
    /// To the desktop: the windows changed.
    Windows,
    /// To the desktop: the start menu key.
    StartMenu,
    /// To the desktop: a window menu choice (`pbr_window::CHOICE_*`).
    WindowMenu {
        /// The client's number (high 32 bits) and the window's (low 32), as `WindowInfo`.
        window: u64,
        /// The choice.
        choice: u32,
    },
    /// To the desktop: the screens changed.
    Screens,
}

/// What came out of the ring: an event for the app, or one the library deals with itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// Give this to the app.
    App(Event),
    /// A frame is the app's again.
    Released {
        /// The window.
        window: Win,
        /// Which frame, 0 or 1.
        frame: u32,
    },
    /// Answer with `pong`.
    Ping {
        /// Quote it.
        serial: u32,
    },
}

/// Whether a key event is text to insert: a press, repeat or second character that has a
/// character, with no Ctrl, Alt or Meta held (note 0021, section 2), so that a text field
/// does not type the `s` of Ctrl+S. Shift and AltGr do not count: they are how some
/// characters are typed.
pub fn is_typed(value: u32, ch: Option<char>, modifiers: u16) -> bool {
    matches!(value, 1 | 2 | 4) && ch.is_some() && modifiers & (MOD_CTRL | MOD_ALT | MOD_META) == 0
}

/// A position in 1/256 pixels as whole pixels, rounding down (an arithmetic shift does that
/// for negative numbers too).
fn whole(raw: i32) -> i32 {
    raw >> 8
}

fn u32_at(b: &[u8; 32], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads one 32-byte ring entry. `None` for a kind this library does not know, or one that is
/// never sent by the server (disconnected).
///
/// Button and scroll events do not carry a position; they come out with 0,0, and the
/// connection fills in the last one it saw.
pub fn decode(b: &[u8; 32]) -> Option<Incoming> {
    let kind = u16::from_le_bytes([b[0], b[1]]);
    let window = Win(u32_at(b, 4));
    let serial = u32_at(b, 8);
    let (a, bb, c) = (u32_at(b, 12), u32_at(b, 16), u32_at(b, 20));
    let pointer = |what, raw_x: i32, raw_y: i32, buttons, serial| {
        Incoming::App(Event::Pointer(PointerEvent {
            window,
            what,
            x: whole(raw_x),
            y: whole(raw_y),
            raw_x,
            raw_y,
            buttons,
            serial,
        }))
    };
    Some(match u32::from(kind) {
        wire::EVENT_LOST => Incoming::App(Event::Lost(a)),
        wire::EVENT_CONFIGURE => {
            let states = c >> 16;
            Incoming::App(Event::Configure {
                window,
                size: (a, bb),
                scale: c & 0xFFFF,
                focused: states & wire::STATE_FOCUSED != 0,
                maximised: states & wire::STATE_MAXIMISED != 0,
                minimised: states & wire::STATE_MINIMISED != 0,
                shown: states & wire::STATE_SHOWN != 0,
                serial,
            })
        }
        wire::EVENT_CLOSE => Incoming::App(Event::CloseRequested { window }),
        wire::EVENT_RELEASED => Incoming::Released { window, frame: a },
        wire::EVENT_FRAME => Incoming::App(Event::Frame { window }),
        wire::EVENT_PING => Incoming::Ping { serial },
        wire::EVENT_KEY => {
            let code = a as u16;
            let modifiers = (a >> 16) as u16;
            let ch = if c == 0 { None } else { char::from_u32(c) };
            Incoming::App(Event::Key(KeyEvent {
                window,
                code,
                modifiers,
                value: bb,
                ch,
                typed: is_typed(bb, ch, modifiers),
                serial,
            }))
        }
        wire::EVENT_POINTER => pointer(PointerKind::Moved, a as i32, bb as i32, c, 0),
        wire::EVENT_LEFT => pointer(PointerKind::Left, 0, 0, 0, 0),
        wire::EVENT_BUTTON => {
            let what = PointerKind::Button { button: a as u16, pressed: bb != 0 };
            pointer(what, 0, 0, c, serial)
        }
        wire::EVENT_SCROLL => {
            pointer(PointerKind::Scroll { axis: a as u16, steps: bb as i32 }, 0, 0, 0, 0)
        }
        wire::EVENT_CLIPBOARD => Incoming::App(Event::Clipboard {
            text: a & wire::FORM_TEXT != 0,
            picture: a & wire::FORM_PICTURE != 0,
        }),
        wire::EVENT_WINDOWS => Incoming::App(Event::Windows),
        wire::EVENT_START_MENU => Incoming::App(Event::StartMenu),
        wire::EVENT_WINDOW_MENU => Incoming::App(Event::WindowMenu {
            window: u64::from(a) | u64::from(bb) << 32,
            choice: c,
        }),
        wire::EVENT_SCREENS => Incoming::App(Event::Screens),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(kind: u16, window: u32, serial: u32, v: [u32; 3]) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..2].copy_from_slice(&kind.to_le_bytes());
        b[4..8].copy_from_slice(&window.to_le_bytes());
        b[8..12].copy_from_slice(&serial.to_le_bytes());
        for (i, x) in v.iter().enumerate() {
            b[12 + 4 * i..16 + 4 * i].copy_from_slice(&x.to_le_bytes());
        }
        b
    }

    fn app(b: [u8; 32]) -> Event {
        match decode(&b) {
            Some(Incoming::App(e)) => e,
            other => panic!("not an app event: {other:?}"),
        }
    }

    #[test]
    fn configure_unpacks_scale_and_states() {
        let states = wire::STATE_FOCUSED | wire::STATE_SHOWN;
        let e = app(raw(1, 7, 99, [240, 160, states << 16 | 200]));
        assert_eq!(
            e,
            Event::Configure {
                window: Win(7),
                size: (240, 160),
                scale: 200,
                focused: true,
                maximised: false,
                minimised: false,
                shown: true,
                serial: 99
            }
        );
        let e = app(raw(1, 1, 1, [1, 2, (2 | 4) << 16 | 100]));
        assert!(matches!(
            e,
            Event::Configure { maximised: true, minimised: true, focused: false, .. }
        ));
    }

    #[test]
    fn simple_kinds() {
        assert_eq!(app(raw(0, 0, 0, [5, 0, 0])), Event::Lost(5));
        assert_eq!(app(raw(2, 3, 0, [0; 3])), Event::CloseRequested { window: Win(3) });
        assert_eq!(app(raw(4, 3, 0, [0; 3])), Event::Frame { window: Win(3) });
        assert_eq!(app(raw(11, 0, 0, [3, 0, 0])), Event::Clipboard { text: true, picture: true });
        assert_eq!(app(raw(11, 0, 0, [2, 0, 0])), Event::Clipboard { text: false, picture: true });
        assert_eq!(app(raw(100, 0, 0, [0; 3])), Event::Windows);
        assert_eq!(app(raw(101, 0, 0, [0; 3])), Event::StartMenu);
        assert_eq!(app(raw(103, 0, 0, [0; 3])), Event::Screens);
        assert_eq!(
            app(raw(102, 0, 0, [0x1234, 0x5, 2])),
            Event::WindowMenu { window: 0x5_0000_1234, choice: 2 }
        );
    }

    #[test]
    fn internal_kinds() {
        assert_eq!(
            decode(&raw(3, 4, 0, [1, 0, 0])),
            Some(Incoming::Released { window: Win(4), frame: 1 })
        );
        assert_eq!(decode(&raw(5, 0, 77, [0; 3])), Some(Incoming::Ping { serial: 77 }));
    }

    #[test]
    fn keys() {
        let a = 22 | u32::from(MOD_CTRL) << 16;
        let Event::Key(k) = app(raw(6, 2, 8, [a, 1, u32::from('v')])) else { panic!() };
        assert_eq!((k.code, k.modifiers, k.value, k.ch, k.serial), (22, 2, 1, Some('v'), 8));
        assert!(!k.typed, "Ctrl+V is not text");
        let Event::Key(k) = app(raw(6, 2, 9, [22, 1, u32::from('v')])) else { panic!() };
        assert!(k.typed);
        // The second character of a key follows as value 4.
        let Event::Key(k) = app(raw(6, 2, 0, [22, 4, 0x301])) else { panic!() };
        assert_eq!((k.value, k.ch, k.typed), (4, Some('\u{301}'), true));
        // Not a character: no char, not typed. Release is never typed.
        let Event::Key(k) = app(raw(6, 2, 0, [37, 1, 0])) else { panic!() };
        assert_eq!((k.ch, k.typed), (None, false));
        let Event::Key(k) = app(raw(6, 2, 0, [22, 0, u32::from('v')])) else { panic!() };
        assert!(!k.typed);
        // A value that is not a character (a surrogate) is no character.
        let Event::Key(k) = app(raw(6, 2, 0, [22, 1, 0xD800])) else { panic!() };
        assert_eq!(k.ch, None);
    }

    #[test]
    fn typed_rule() {
        assert!(is_typed(1, Some('a'), 0));
        assert!(is_typed(2, Some('A'), 1)); // shift
        assert!(is_typed(1, Some('@'), 8)); // altgr
        assert!(!is_typed(1, Some('a'), MOD_CTRL));
        assert!(!is_typed(1, Some('a'), MOD_ALT));
        assert!(!is_typed(2, Some('a'), MOD_META));
        assert!(!is_typed(3, Some('a'), 0));
        assert!(!is_typed(0, Some('a'), 0));
        assert!(!is_typed(1, None, 0));
    }

    #[test]
    fn pointer_positions_round_down_and_may_be_negative() {
        let Event::Pointer(p) = app(raw(7, 1, 0, [(10 * 256 + 128) as u32, (-1i32) as u32, 5]))
        else {
            panic!()
        };
        assert_eq!((p.x, p.y, p.raw_x, p.raw_y, p.buttons), (10, -1, 10 * 256 + 128, -1, 5));
        assert_eq!(p.what, PointerKind::Moved);
        let Event::Pointer(p) = app(raw(7, 1, 0, [(-300i32) as u32, i32::MIN as u32, 0])) else {
            panic!()
        };
        assert_eq!((p.x, p.y), (-2, -(1 << 23)));
    }

    #[test]
    fn buttons_scrolls_and_leaving() {
        let Event::Pointer(p) = app(raw(9, 1, 42, [1, 1, 1])) else { panic!() };
        assert_eq!(p.what, PointerKind::Button { button: 1, pressed: true });
        assert_eq!((p.serial, p.buttons), (42, 1));
        let Event::Pointer(p) = app(raw(9, 1, 43, [2, 0, 0])) else { panic!() };
        assert_eq!(p.what, PointerKind::Button { button: 2, pressed: false });
        let Event::Pointer(p) = app(raw(10, 1, 0, [2, (-3i32) as u32, 0])) else { panic!() };
        assert_eq!(p.what, PointerKind::Scroll { axis: 2, steps: -3 });
        let Event::Pointer(p) = app(raw(8, 1, 0, [0; 3])) else { panic!() };
        assert_eq!(p.what, PointerKind::Left);
    }

    #[test]
    fn unknown_and_never_sent_kinds_are_skipped() {
        for kind in [12u16, 13, 50, 99, 104, 1000, u16::MAX] {
            assert_eq!(decode(&raw(kind, 1, 1, [1, 2, 3])), None, "kind {kind}");
        }
    }

    #[test]
    fn any_bytes_decode_without_panicking() {
        // A small deterministic scrambler: every kind gets many nonsense bodies.
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..20_000u32 {
            let mut b = [0u8; 32];
            for byte in b.iter_mut() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *byte = (s >> 24) as u8;
            }
            if i % 2 == 0 {
                b[0] = (i % 110) as u8;
                b[1] = 0;
            }
            let _ = decode(&b);
        }
        let _ = decode(&[0xFF; 32]);
        let _ = decode(&[0; 32]);
    }
}
