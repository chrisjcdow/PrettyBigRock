// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Windows (docs/design/0022-display-server.md). Served by the display server to each app,
/// through a capability badged for that app alone, which the session's sandbox manager asked
/// for (`pbr.display.session.client`). The badge is how the display server knows which app
/// every request comes from; an app can't name itself, its title or its flags.
///
/// A window is a picture the app draws in memory the display server made and lends it: two
/// frames, so that the app draws in one while the other is on the screen. Sizes and places are
/// in *logical* pixels; a window's frames are its size times its scale (100 is 1x, 200 is 2x),
/// which the display server sets from the screen it is on.
///
/// Events come through a ring (`events`), 32 bytes each, little-endian:
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | 0 | 2 | Kind (`EVENT_*` below) |
/// | 2 | 2 | Reserved, 0 |
/// | 4 | 4 | The window (0 for the client as a whole) |
/// | 8 | 4 | Serial (configure, key, button and ping events; otherwise 0) |
/// | 12 | 4 x 3 | Three values, by kind (below) |
/// | 24 | 8 | When, nanoseconds on the monotonic clock |
interface pbr.window version 1;

/// Size of one event in the ring.
const EVENT_SIZE: u32 = 32;

/// The ring was full and `a` events were lost: redraw everything, and assume no key is held.
const EVENT_LOST: u32 = 0;
/// The window's size (`a` by `b`, logical pixels), scale (the low 16 bits of `c`) and state
/// (its high 16: `STATE_*`). Ask for buffers with this event's serial before drawing at a new
/// size or scale.
const EVENT_CONFIGURE: u32 = 1;
/// The user asked to close the window (its close button, Ctrl+W, the taskbar), or a popup was
/// dismissed. Nothing is closed until the app destroys it.
const EVENT_CLOSE: u32 = 2;
/// Frame `a` is the app's again.
const EVENT_RELEASED: u32 = 3;
/// A good time to draw (after `frame`).
const EVENT_FRAME: u32 = 4;
/// Answer with `pong(serial)` within 5 seconds, or the windows are marked not responding.
const EVENT_PING: u32 = 5;
/// A key: `a` is `pbr.seat`'s code (low 16 bits) and modifiers (high 16), `b` the value
/// (released 0, pressed 1, repeat 2, still held 3), `c` the character typed (0 if none). A
/// second character, if any, follows as a key event with value 4 (`VALUE_SECOND`).
const EVENT_KEY: u32 = 6;
/// The pointer moved, or entered the window: x and y (`a`, `b`) in 1/256 logical pixels from
/// the corner of the window's content, and the buttons held (`c`, bit n for button n).
const EVENT_POINTER: u32 = 7;
/// The pointer left the window.
const EVENT_LEFT: u32 = 8;
/// A button: `a` which (`pbr.input`'s), `b` released 0 or pressed 1, `c` the buttons held.
const EVENT_BUTTON: u32 = 9;
/// Scrolling: `a` the axis (`pbr.input`'s), `b` the steps (signed).
const EVENT_SCROLL: u32 = 10;
/// The clipboard changed: `a` the forms there (bit 0 text, bit 1 picture). Only to the client
/// with the focus.
const EVENT_CLIPBOARD: u32 = 11;
/// The display server has gone (made by `lib/window`, never sent by the server).
const EVENT_DISCONNECTED: u32 = 12;
/// To the desktop: the windows changed (ask `windows()` again).
const EVENT_WINDOWS: u32 = 100;
/// To the desktop: the start menu key (the Windows key, or Ctrl+Esc).
const EVENT_START_MENU: u32 = 101;
/// To the desktop: a window menu choice. `a` and `b` are the window (as `WindowInfo.window`:
/// low then high 32 bits), `c` the choice (`CHOICE_*`).
const EVENT_WINDOW_MENU: u32 = 102;
/// To the desktop: screens came, went or changed (ask `screens()` again).
const EVENT_SCREENS: u32 = 103;

/// The value of a key event that carries a key's second character.
const VALUE_SECOND: u32 = 4;

/// Window states, in the high 16 bits of a configure event's third value.
const STATE_FOCUSED: u32 = 1;
const STATE_MAXIMISED: u32 = 2;
const STATE_MINIMISED: u32 = 4;
/// On some screen (not minimised, and not only on a screen that went).
const STATE_SHOWN: u32 = 8;

/// Window menu choices sent to the desktop.
const CHOICE_PERMISSIONS: u32 = 1;
const CHOICE_STOP: u32 = 2;

/// What `act` does.
const ACT_FOCUS: u32 = 1;
const ACT_RAISE: u32 = 2;
const ACT_MINIMISE: u32 = 3;
const ACT_RESTORE: u32 = 4;
const ACT_CLOSE: u32 = 5;

/// Clipboard forms, as bits.
const FORM_TEXT: u32 = 1;
const FORM_PICTURE: u32 = 2;

/// The largest clipboard item, in bytes.
const MAX_CLIPBOARD: u64 = 16777216;

/// What a window is (note 0022, section 3).
enum WindowKind {
    /// Moved, resized (if it says it can be), minimised, maximised, closed.
    Normal @1,
    /// Has a parent window of the same client and stays above it.
    Dialog @2,
    /// A menu, list or tooltip: no title bar. Made only in answer to an input event delivered
    /// to its parent in the last second (`serial`), touching or inside its parent, at most
    /// half its screen each way.
    Popup @3,
    /// The file picker and the permissions viewer: `System` clients only.
    System @4,
    /// The wallpaper of one screen (`at.x` is the screen): the `Desktop` client only.
    Background @5,
    /// A strip along one edge of a screen (`at.x` the screen, `at.y` the `Edge`, the size's
    /// height, or width at the left and right, its thickness): the `Desktop` client only.
    Panel @6,
}

/// Window flags.
bits WindowFlags: u32 {
    /// The user may resize it.
    Resizable @0,
    /// A dialog that blocks input to its parent while it is open.
    Modal @1,
}

struct Size { width: u32, height: u32 }
struct Point { x: i32, y: i32 }
struct Rect { x: i32, y: i32, width: u32, height: u32 }

/// Pointer shapes.
enum Cursor {
    Arrow @1, Text @2, Hand @3, Move @4, ResizeNs @5, ResizeEw @6, ResizeNesw @7, ResizeNwse @8,
    Wait @9, Crosshair @10, None @11,
}

/// A window's edges and corners, for resizing (and a panel's edge).
enum Edge { Top @1, Bottom @2, Left @3, Right @4, TopLeft @5, TopRight @6, BottomLeft @7,
            BottomRight @8 }

/// What the clipboard holds.
enum ClipForm { Text @1, Picture @2 }

/// The ring of events, asked for once. The display server signals bit 0 of `doorbell` (a
/// notification of the app's own, so that one notification can carry everything the app waits
/// for) whenever it adds events. Map the ring to read and write.
@1 fn events(doorbell: handle<notification>) -> (ring: handle<memory>) error WindowError;

/// A new window, numbered for this client. `parent` is 0 or one of this client's windows;
/// `at` places a popup relative to its parent's content (and names a background's or panel's
/// screen); `serial` is an input event's, for popups. Its first configure event follows.
@2 fn create(kind: WindowKind, parent: u32, title: string<64>, size: Size, flags: WindowFlags,
             at: Point, serial: u32) -> (window: u32) error WindowError;
@3 fn destroy(window: u32) -> () error WindowError;
/// The app's own part of the title, shown after the manifest's title and a dash.
@4 fn set_title(window: u32, title: string<64>) -> () error WindowError;
/// The smallest and largest the user may make it (0 for no limit).
@5 fn set_limits(window: u32, min: Size, max: Size) -> () error WindowError;
@6 fn set_cursor(window: u32, cursor: Cursor) -> () error WindowError;

/// Two frames for the configure event `serial`'s size and scale, one after the other in the
/// memory (frame 1 starts at `height * stride`), each pixel blue, green, red, then a byte
/// that isn't used. Map it to read and write.
@10 fn buffers(window: u32, serial: u32)
        -> (memory: handle<memory>, width: u32, height: u32, stride: u32) error WindowError;
/// Hands frame 0 or 1 to the display server, with the rectangles that changed, in logical
/// pixels (all of it if the list is empty). It is the display server's until a released
/// event gives it back.
@11 fn commit(window: u32, frame: u32, changed: list<Rect, 64>) -> () error WindowError;
/// Asks for a frame event.
@12 fn frame(window: u32) -> () error WindowError;
@13 fn pong(serial: u32) -> () error WindowError;

/// Things only the user's own action may start: `serial` is a button or key press delivered
/// to this window in the last second.
@20 fn start_move(window: u32, serial: u32) -> () error WindowError;
@21 fn start_resize(window: u32, edge: Edge, serial: u32) -> () error WindowError;
@22 fn minimise(window: u32) -> () error WindowError;
@23 fn maximise(window: u32, on: bool) -> () error WindowError;

/// Puts `length` bytes of `data` on the clipboard: text (UTF-8), or a picture of `width` by
/// `height` pixels as frames are. Only with the focus, and with a press's serial from the
/// last second. A copy with the same serial as the last adds its form to the same item.
@30 fn copy(serial: u32, form: ClipForm, data: handle<memory>, length: u64, width: u32,
            height: u32) -> () error WindowError;
/// The clipboard's `form`, in new memory to read: only with the focus, and with a press's
/// serial from the last second.
@31 fn paste(serial: u32, form: ClipForm)
        -> (data: handle<memory>, length: u64, width: u32, height: u32) error WindowError;

/// One screen's place in the desktop (the `Desktop` client only).
struct ScreenPlace {
    /// The display server's number for it, from 1, the same for as long as it is there.
    screen: u32,
    /// The device's description of itself (maker and serial number), then its connector.
    identity: string<64>,
    /// Its top-left corner, in logical pixels.
    x: i32,
    y: i32,
    /// Hundredths: 100 is 1x. M6 accepts 100 and 200.
    scale: u32,
    /// Degrees clockwise: 0, 90, 180 or 270.
    rotation: u32,
    /// The screen it shows the same as (0 for none); such a screen isn't part of the space.
    mirrors: u32,
    /// Has the panel, and where new windows open.
    main: bool,
    /// Its size in logical pixels (ignored by `arrange`).
    size: Size,
}

/// One window, for the taskbar (the `Desktop` client only).
struct WindowInfo {
    /// The client's number (high 32 bits) and its window's (low 32 bits).
    window: u64,
    /// The app's number, as the sandbox manager gave it.
    app: u32,
    /// As its title bar shows it.
    title: string<128>,
    minimised: bool,
    focused: bool,
    /// Wants the user to look (a window that opened without the focus).
    attention: bool,
    not_responding: bool,
}

/// Which screen a tablet is tied to, by its name (`pbr.seat`'s); screen 0 is the whole space.
struct TabletTie { name: string<64>, screen: u32 }

// For the Desktop client only (`NotAllowed` for others).
@100 fn screens() -> (screens: list<ScreenPlace, 16>) error WindowError;
@101 fn arrange(screens: list<ScreenPlace, 16>) -> () error WindowError;
/// The session's normal windows and dialogs, in the order they were opened.
@102 fn windows() -> (windows: list<WindowInfo, 64>) error WindowError;
/// Focus, raise, minimise, restore or close (`ACT_*`) any of the session's windows.
@103 fn act(window: u64, action: u32) -> () error WindowError;
/// Pointer speed (hundredths) and which screen each tablet is tied to.
@104 fn pointer(speed: u32, tablets: list<TabletTie, 8>) -> () error WindowError;

// For System clients only (`NotAllowed` for others): the file picker opens its window over the
// app that asked (note 0025), so it names another client's window by that app's number (as
// the sandbox manager gave it in `client`) and the app's own window number.
/// Whether `serial` is a key or button press delivered to that window in the last second.
@110 fn check_serial(app: u32, window: u32, serial: u32) -> (recent: bool) error WindowError;
/// A `System` window placed over another app's window, which it blocks input to while open.
@111 fn create_over(app: u32, window: u32, title: string<64>, size: Size, flags: WindowFlags)
        -> (window: u32) error WindowError;

/// What can go wrong.
flexible enum WindowError {
    /// This client may not do that.
    NotAllowed @1,
    /// No such window of this client's (or app, or screen).
    NoSuchWindow @2,
    /// That kind of window can't be made with these arguments.
    BadKind @3,
    /// The serial isn't one this client was given for that, or is more than a second old.
    BadSerial @4,
    /// Past a limit on memory: a window bigger than any screen, or the session's 96 MiB.
    NoMemory @5,
    /// Past a limit on numbers: 32 windows a client.
    TooMany @6,
    BadArgument @7,
    /// Only for a client with the focus.
    NotFocused @8,
    /// The clipboard has nothing in that form.
    Empty @9,
    /// A configure serial that has been overtaken by a newer one, or a frame this client
    /// doesn't hold.
    Stale @10,
}
