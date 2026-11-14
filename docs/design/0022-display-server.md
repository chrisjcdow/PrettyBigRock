# 0022: The display server and compositor

Status: Accepted · 9 November 2026 · Related: DESIGN.md principles 1 and 5, sections 7.1, 7.3 and 8
("Display server and compositor"), Q8; notes 0006, 0013, 0017, 0019, 0020, 0021, 0023, 0024,
0025; milestone M6

## The problem

DESIGN.md section 8 asks for a display server where each window is a shared buffer the app
draws into, the compositor puts the screens together, several screens of different sharpness
make one desktop from the start, and the protocol leaves room for GPUs and for a Wayland front
end. Section 7 adds what makes it safe: apps start empty, and the file picker and clipboard are
part of the system, outside the apps.

This note settles the display server's place among the services, its protocol (`pbr.window`),
how screens make one desktop, how windows are drawn, decorated and stacked, where keyboard and
pointer events go, the clipboard, what it guarantees against hostile apps, and DESIGN.md's Q8.
It follows Chris's answers to the M6 plan: our own protocol shaped so that Wayland can be
translated to it (Q1), title bars and frames drawn by the display server (Q2), window buffers
allocated by the display server (Q3), a system-wide display server with a per-session desktop
app (Q4), and the clipboard in the display server (Q15).

## The design

### 1. The parts

```
 root ──┬── devmgr ──── display drivers (note 0020), input drivers
        ├── input  (note 0021)
        ├── display ◀── pbr.window ── apps (each with one capability, badged per app)
        │      ▲    ◀── pbr.window ── the desktop app (with the Desktop flag; note 0024)
        │      └─────── pbr.display.sessions ── the users service
        └── users ──── sandbox manager ──launches── apps
```

- **`services/display`** is supervised by the root program. It may look up `devices` and
  `input` (note 0018's lookup table). It takes every display device's screens (`pbr.display`
  version 2) and the seat (`pbr.seat`), and holds no one's files.
- **Sessions.** The users service may look up `display`, and gets `pbr.display.sessions`
  (section 9). For each session it starts, it asks for a **session capability** and gives it
  to that session's sandbox manager (note 0024, section 2). M6 has one session at a time; the
  protocol has several so that switching between logged-in users can come later. Only the
  active session's windows are shown and get input.
- **Apps' capabilities.** For each app it launches with the manifest key `windows`, the
  sandbox manager asks its session capability for a **client capability** (`client`), giving
  the app's number, its manifest title, and flags (`System`, `Desktop`, `Trusted`; section 8).
  The answer is derived from the session capability (note 0013), with a watched badge: the
  display server knows which app every request comes from, the sandbox manager can revoke one
  app's, and revoking the session's takes every app's.
- **An app can't make a window without that capability**, and can't name itself: its title,
  flags and app number are what the sandbox manager said.

### 2. Screens and the desktop's space

- **One coordinate space** in **logical pixels**. Each screen has a place in it (its top-left
  corner), a size in logical pixels (its mode divided by its scale), a **scale** and a
  **rotation** (0, 90, 180, 270 degrees). The compositor draws rotated itself (note 0020,
  section 2).
- **Scale** is in hundredths (`100` is 1×, `200` is 2×). M6 accepts multiples of 100 and tests 1×
  and 2× (Chris's answer to Q7); the protocol carries any value, so that fractional scales can
  come with outline fonts. The default is 2× for a screen of at least 180 dots per inch (from
  its size in millimetres), otherwise 1×.
- **The arrangement** (places, scales, rotations, which screen is the main one, mirroring) is
  set by the desktop app, which remembers it per screen identity (note 0020) in its own folder
  (note 0024). Until it says, or for a screen it doesn't know: screens in device and connector
  order, left to right, tops aligned, the first the main one.
- **Mirroring**: a screen set to mirror another shows the same part of the desktop, scaled to
  fit and centred, and isn't part of the space itself (as for a projector).
- **A screen going** (unplugged, or its driver given up on): every window mostly on it moves to
  the main screen (or, if that went, the first left), keeping its place relative to the screen's
  corner as far as it fits, and is told the new scale. **A screen coming** gets its remembered
  place or the default one; no windows move to it by themselves.
- **The main screen** has the desktop's panel (the taskbar) and is where new windows open.

### 3. Windows

**Kinds:**

| Kind | Who may make it | Decorated | Notes |
|------|-----------------|-----------|-------|
| `Normal` | Any client | Yes | Moved, resized (if it says it can be), minimised, maximised, closed |
| `Dialog` | Any client | Yes | Has a parent window of the same client; stays above it; blocks input to it while open if `Modal` |
| `Popup` | Any client | No | Menus, lists and tooltips. Has a parent of the same client; see below |
| `System` | `System` clients only | Yes, marked as system (section 6) | The file picker, the permissions viewer; always above every app's windows |
| `Background` | The `Desktop` client only | No | One per screen, below everything: the wallpaper and icons |
| `Panel` | The `Desktop` client only | No | Along an edge of a screen; takes that strip out of the screen's work area (where windows maximise); the taskbar |

**Popups** are what an app could use to draw something that looks like a system window, so they
are limited: a popup is made only in answer to an input event delivered to its parent in the
last second (it quotes the event's serial), it opens touching or inside its parent, it is at
most half its screen in each direction, it never covers a `System` window, and it closes when
the parent loses the focus or the user clicks outside it.

**Size and scale.** A window has a size in logical pixels, chosen by the app at creation and
then changed only through the display server: the user resizes it, maximises it, or moves it to
a screen of another scale. Each time, the display server sends a **configure** event (new
size, scale and state, with a serial); the app asks for buffers for that serial, draws, and
commits. Until it does, the display server keeps showing the old picture (scaled, if the scale
changed), so a slow app never leaves a hole.

A window's scale is that of the screen holding most of it. While it spans two screens of
different scales, its picture is drawn on the other one resized.

**Buffers** (Q3): the display server makes them, in its own budget, and lends them:
- `buffers(window, serial)` answers with one memory object holding **two frames** of 32-bit
  `Bgrx` pixels at the window's size times its scale, and their stride. The app maps it read
  and write; the display server keeps the original and maps it read only. Because the display
  server made it, the app can't revoke it, so the display server can never be made to fault
  (note 0006, section 5).
- The app draws in a frame it holds, then **commits** it with the rectangles that changed. The
  frame then belongs to the display server until a **released** event gives it back. (With a
  GPU later, commit gains a fence the GPU signals; nothing else changes.)
- New buffers for a new size or scale replace the old ones; the old memory goes when the app
  closes its copy.
- **Limits**: a window's buffers are no larger than the largest screen (in device pixels); a
  client has at most 32 windows; a session's windows together at most 96 MiB. Past these,
  `create` or `buffers` answer `NoMemory`. (Charging this memory to the session rather than to
  the display server is an open question.)

**Frames.** An app asks `frame(window)` and gets a **frame** event when the display server has
shown its last commit and it is a good time to draw again (about 60 times a second while the
window can be seen; never while it is minimised or covered entirely). Animations (Snake) draw
on frame events; apps that only change when the user does something just commit.

**Not responding.** The display server sends a **ping** to a client every 2 seconds while one
of its windows has the focus, and when the user clicks one of its windows. A client that doesn't
`pong` within 5 seconds has "(not responding)" in its title bars, and its window menu offers
**Stop** (section 6). The display server never waits for an app: it calls no app, and reads no
memory an app made.

**Stacking.** Within a session, back to front: backgrounds; normal windows and dialogs in the
order they were last focused; panels; popups; system windows; the pointer. Clicking a window
raises it (with its dialogs).

### 4. Input

- **The keyboard** goes to the window with the **focus**, and nowhere else. A window gets the
  focus when the user clicks it, chooses it in the taskbar, or switches to it with Alt+Tab. A
  new window gets the focus only if its client has the focus already or was launched by the
  user less than 2 seconds ago (the sandbox manager says when, in `client`); otherwise its
  taskbar button is highlighted. So an app in the background can't pop up a window and catch
  what the user was typing.
- **Keys the display server keeps** and never sends to apps: the Windows key and Ctrl+Esc (the
  start menu, sent to the desktop client), Alt+Tab, and Ctrl+W (close the focused window, as
  in 1.3: the app gets a close request).
- **The pointer** goes to the window under it. While a button is held, it goes to the window it
  was pressed in, until every button is released, even outside it. There is no other way to
  hold on to the pointer or the keyboard in M6. (Locking the pointer for games is an open
  question.)
- **Pointer devices.** Relative movement is scaled by a speed setting (from the desktop) and
  moves one pointer across all screens; it can't leave the space. A **tablet** (an absolute
  pointer) is tied to one screen: by default the *n*-th tablet to the *n*-th screen when there
  are as many tablets as screens, otherwise to the whole space; the desktop's settings can tie
  a tablet (by its name) to any screen.
- **Pointer shapes**: an app chooses one of a fixed set for each window (arrow, text, hand,
  move, the four resize directions, wait, crosshair, none). The display server draws the
  pointer, at each screen's scale. Pictures of an app's own come later.
- **Events carry serials.** Every key press and button press delivered to a client has a serial,
  which the client quotes to make a popup, start moving or resizing its window from its own
  content, and paste. A serial is good for one second, for the client it was sent to.

### 5. The clipboard (Q15)

Each session has one clipboard in the display server, holding at most one item in up to two
forms: **text** (UTF-8) and **a picture** (`Bgrx` pixels, with its size). At most 16 MiB.

- **Copying** (`copy`) is allowed only to a client whose window has the focus. The app passes a
  memory object it made and a length; the display server copies it into its own memory with
  `memory_read` (a system call, which fails cleanly if the app revokes the memory meanwhile),
  never by mapping it.
- **Pasting** (`paste`) is allowed only to a client whose window has the focus, and only with
  the serial of a key press or button press delivered to it in the last second. So an app can
  paste when the user presses Ctrl+V or chooses Paste, and never on its own. The answer is a
  new, read-only memory object the display server made, holding a copy.
- **Clipboard events** tell the focused client which forms are there, so it can grey out Paste.
- The clipboard is emptied when the session ends. Drag and drop comes later, on the same rules.

### 6. Decorations and system windows (Q2)

- **The display server draws** every `Normal`, `Dialog` and `System` window's frame, shadow and
  title bar, in 1.3's style (note 0024 has the look): the icon, the **title from the manifest**
  ("Notepad"), then the app's own title in a lighter style after a dash ("Notepad — letter.txt",
  at most 64 bytes of it), then minimise, maximise and close. The app draws only inside.
- **Trusted apps** (Terminal, Files) have a "trusted" mark in their title bar, so the user can
  always see which windows can reach all their files (DESIGN.md section 7.1).
- **System windows** have a frame of their own colour and the rock emblem in the title bar.
  Nothing an app makes can carry either: the title bar is drawn by the display server from what
  the sandbox manager said, popups (which have no title bar) can't cover a system window, and
  a system window is always above every app window. The file picker and the permissions viewer
  are the only system clients in M6 (note 0025).
- **The window menu** (the icon, or a right click on the title bar): Minimise, Maximise or
  Restore, Move to screen…, **What can this app reach?** (the permissions viewer for that app,
  note 0025), **Stop** (when not responding), and Close. The display server doesn't launch or
  stop anything itself: it sends these to the desktop client (section 9), which asks the
  sandbox manager.
- Dragging the title bar moves a window, including from one screen to another; dragging an
  edge resizes a resizable one; a double click on the title bar maximises or restores it.

### 7. Composing

- **What is drawn**: each screen's part of the space, from the back (section 3's stacking),
  with `lib/draw` (note 0023): window pictures, their decorations and shadows, and the pointer,
  then rotated for the screen if needed, into the driver's buffer for that screen.
- **Only what changed**: commits, moves, the pointer, and the clock all add changed rectangles;
  a screen is drawn again only where something changed, and presented with those rectangles
  (note 0020), one picture at a time per screen.
- **Windows of another scale**: a window committed at 2× and shown on a 1× screen (while it
  spans two) is halved; one at 1× on a 2× screen is doubled. Once it is mostly on the new screen
  it is told the new scale and redraws sharply.
- **In step with the screens**: the display server presents a screen at most once per its
  refresh (60 times a second if unknown), and only when the previous present is done.
- **Speed.** All drawing is done by the processor. `lib/draw` and the compositor are built
  optimised even in test builds (note 0026, section 6), since emulated x86 is slow.

### 8. What it guarantees

Against an app that is hostile, the display server makes sure that it:

| Guarantee | How |
|-----------|-----|
| can't see another window's pixels | Each client's buffers are its own memory objects; nothing gives a client another's; there is no screenshot operation for apps (a trusted screenshot tool later would be a flag of its own) |
| can't receive input meant for another window | Keys go only to the focused window, the pointer only to the window under it or the one pressed in (section 4) |
| can't grab the keyboard or pointer, or take the focus | No grab operation; the focus rule in section 4 |
| can't paste on its own, or copy in the background | Section 5 |
| can't pass for a system window, another app, or the desktop | Section 6; titles come from manifests; the `System` and `Desktop` flags come from the sandbox manager |
| can't stall or crash the display server | It never waits for an app, never reads memory an app made except by `memory_read`, and checks every request; events go through a ring (`granite::ring`) where a client can only make itself lose events |
| can't use up the display server | The limits in section 3, and the watched badge freeing everything when its capability goes |

### 9. The interfaces

```
/// Windows. Served by the display server to each app, through a capability badged for it.
interface pbr.window version 1;

enum WindowKind { Normal @1, Dialog @2, Popup @3, System @4, Background @5, Panel @6 }
bits WindowFlags: u32 { Resizable @0, Modal @1 }
struct Size { width: u32, height: u32 }
struct Point { x: i32, y: i32 }
struct Rect { x: i32, y: i32, width: u32, height: u32 }
enum Cursor { Arrow @1, Text @2, Hand @3, Move @4, ResizeNs @5, ResizeEw @6,
              ResizeNesw @7, ResizeNwse @8, Wait @9, Crosshair @10, None @11 }
enum Edge { Top @1, Bottom @2, Left @3, Right @4, TopLeft @5, TopRight @6,
            BottomLeft @7, BottomRight @8 }
enum ClipForm { Text @1, Picture @2 }

/// The ring of 32-byte events (section 10), asked once; bit 0 of the app's `doorbell` is
/// signalled when events are added.
@1 fn events(doorbell: handle<notification>) -> (ring: handle<memory>) error WindowError;

/// A new window. `parent` is 0 or one of this client's windows; `at` places a popup
/// relative to its parent; `serial` is an input event's, for popups. Its first configure
/// event follows.
@2 fn create(kind: WindowKind, parent: u32, title: string<64>, size: Size, flags: WindowFlags,
             at: Point, serial: u32) -> (window: u32) error WindowError;
@3 fn destroy(window: u32) -> () error WindowError;
@4 fn set_title(window: u32, title: string<64>) -> () error WindowError;
@5 fn set_limits(window: u32, min: Size, max: Size) -> () error WindowError;
@6 fn set_cursor(window: u32, cursor: Cursor) -> () error WindowError;

/// Two frames for the configure event `serial`'s size and scale.
@10 fn buffers(window: u32, serial: u32)
        -> (memory: handle<memory>, width: u32, height: u32, stride: u32) error WindowError;
/// Hands frame 0 or 1 to the display server, with the rectangles that changed (all if empty).
@11 fn commit(window: u32, frame: u32, changed: list<Rect, 64>) -> () error WindowError;
/// Asks for a frame event.
@12 fn frame(window: u32) -> () error WindowError;
@13 fn pong(serial: u32) -> () error WindowError;

/// Asks for something only the user's own action may start (the serial says which).
@20 fn start_move(window: u32, serial: u32) -> () error WindowError;
@21 fn start_resize(window: u32, edge: Edge, serial: u32) -> () error WindowError;
@22 fn minimise(window: u32) -> () error WindowError;
@23 fn maximise(window: u32, on: bool) -> () error WindowError;

/// The clipboard (section 5).
@30 fn copy(serial: u32, form: ClipForm, data: handle<memory>, length: u64, width: u32,
            height: u32) -> () error WindowError;
@31 fn paste(serial: u32, form: ClipForm)
        -> (data: handle<memory>, length: u64, width: u32, height: u32) error WindowError;

// For the Desktop client only (`NotAllowed` for others).
struct ScreenPlace { screen: u32, identity: string<64>, x: i32, y: i32, scale: u32,
                     rotation: u32, mirrors: u32, main: bool, size: Size }
struct WindowInfo { window: u64, app: u32, title: string<128>, minimised: bool,
                    focused: bool, attention: bool, not_responding: bool }
@100 fn screens() -> (screens: list<ScreenPlace, 16>) error WindowError;
@101 fn arrange(screens: list<ScreenPlace, 16>) -> () error WindowError;
@102 fn windows() -> (windows: list<WindowInfo, 64>) error WindowError;
/// Focus, raise, minimise, restore or close any of the session's windows.
@103 fn act(window: u64, action: u32) -> () error WindowError;
/// Pointer speed (hundredths) and which screen each tablet is tied to.
@104 fn pointer(speed: u32, tablets: list<TabletTie, 8>) -> () error WindowError;
struct TabletTie { name: string<64>, screen: u32 }

// For System clients only (`NotAllowed` for others): the file picker opens its window over the
// app that asked (note 0025), so it names another client's window by that app's number (as
// the sandbox manager gave it in `client`) and the app's own window number.
/// Whether `serial` is a key or button press delivered to that window in the last second.
@110 fn check_serial(app: u32, window: u32, serial: u32) -> (recent: bool) error WindowError;
/// A `System` window placed over another app's window, which it blocks input to while open.
@111 fn create_over(app: u32, window: u32, title: string<64>, size: Size, flags: WindowFlags)
        -> (window: u32) error WindowError;

flexible enum WindowError {
    NotAllowed @1, NoSuchWindow @2, BadKind @3, BadSerial @4, NoMemory @5, TooMany @6,
    BadArgument @7, NotFocused @8, Empty @9, Stale @10,
}
```

```
/// Sessions, for the users service.
interface pbr.display.sessions version 1;
/// A capability for a new session, for its sandbox manager (derived, watched).
@1 fn start(user: string<32>) -> (session: handle<pbr.display.session>, number: u32)
    error SessionsError;
/// Shows session `number` and gives it the input (one at a time).
@2 fn activate(number: u32) -> () error SessionsError;
/// Shows the shutting-down screen on every screen (note 0024, section 7) and stops showing
/// windows. Nothing else is drawn until the display server starts again.
@3 fn farewell(restarting: bool) -> () error SessionsError;
/// The screens, for the About window's system information (note 0026, section 3).
struct ScreenSummary { connector: string<32>, width: u32, height: u32, scale: u32 }
@4 fn screens() -> (screens: list<ScreenSummary, 16>) error SessionsError;
flexible enum SessionsError { NotAllowed @1, TooMany @2, BadSession @3 }
```

```
/// One session, for its sandbox manager.
interface pbr.display.session version 1;
bits ClientFlags: u32 { System @0, Desktop @1, Trusted @2 }
/// A client capability for one app (derived from this one, watched). `launched` is when the
/// user launched it (monotonic nanoseconds; 0 if not by the user).
@1 fn client(app: u32, title: string<64>, flags: ClientFlags, launched: u64)
        -> (client: handle<pbr.window>) error SessionError;
flexible enum SessionError { TooMany @1, BadArgument @2 }
```

(The largest lists and strings that fit are settled by the interface compiler when the files
are written, and recorded here; `window: u64` in `WindowInfo` is the client's badge and its own
window number together, so the desktop can name any window.)

### 10. Events

A client's events come through its ring, 32 bytes each, little-endian:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | Kind |
| 2 | 2 | Reserved, 0 |
| 4 | 4 | Window (0 for the client as a whole) |
| 8 | 4 | Serial (for configure, key, button and ping events; otherwise 0) |
| 12 | 4 × 3 | Three values, by kind (below) |
| 24 | 8 | Time, nanoseconds on the monotonic clock |

| Kind | Event | Values |
|------|-------|--------|
| 0 | Lost (the ring was full; the client should redraw everything and assume no key held) | how many |
| 1 | Configure | width and height (logical pixels), then scale (low 16 bits) and states (high 16: bit 0 focused, 1 maximised, 2 minimised, 3 shown) |
| 2 | Close requested | |
| 3 | Released (a frame is the app's again) | the frame |
| 4 | Frame (a good time to draw) | |
| 5 | Ping | |
| 6 | Key | `pbr.seat`'s code (low 16) and modifiers (high 16), the value, then the first character; a second character, if any, follows as a key event of value 4 |
| 7 | Pointer moved, or entered the window | x and y in 1/256 logical pixels from the window's content corner, then the buttons held (bits) |
| 8 | Pointer left the window | |
| 9 | Button | the button, the value (0 released, 1 pressed), the buttons held |
| 10 | Scroll | the axis, the steps |
| 11 | Clipboard changed | forms there (bit 0 text, 1 picture), only to the focused client |
| 12 | Disconnected (sent by `lib/window`, never by the server: the display server has gone) | |
| 100 | Desktop: windows changed | |
| 101 | Desktop: the start menu key | |
| 102 | Desktop: a window menu choice | the window (two values) and the choice (1 permissions, 2 stop) |
| 103 | Desktop: screens changed | |

### 11. When the display server ends

- Its clients' capabilities fail and their receiver watches fire (note 0019); `lib/window`
  turns that into a disconnected event. The root program restarts it with the restart policy
  (note 0018).
- **Apps with windows end too.** The sandbox manager, whose session capability's watch fires,
  stops every app in the session that was given `windows`, and the users service asks the new
  display server for a new session capability and gives it to the sandbox manager, which starts
  the desktop again. This is note 0018's rule (dependants start afresh with new capabilities),
  and it means unsaved work in windowed apps is lost when the display server crashes; RockFS
  keeps what was saved. Keeping windows across a restart is an open question.
- If the display server is given up on, the root program starts the console instead (note
  0024, section 8).

### 12. A Wayland front end (DESIGN.md Q8, Chris's answer to Q1)

**Decision:** PrettyBigRock's own protocol is the display server's only protocol. A Wayland
front end, when ported toolkits need it (M12), will be a separate service that speaks Wayland to
its clients and `pbr.window` to the display server, as a translator. The protocol is shaped to
make that straightforward:

| Wayland | `pbr.window` |
|---------|--------------|
| `wl_surface`, `commit`, damage | a window, `commit` with changed rectangles |
| `wl_buffer` from `wl_shm` | the display server's buffers (the front end copies a client's shared memory into them) |
| `wl_callback` from `frame` | `frame` and the frame event |
| `xdg_toplevel`, `xdg_popup` (with a serial) | `Normal`, `Popup` (with a serial) |
| `configure` and `ack_configure` | configure with a serial, `buffers(serial)` |
| `wl_data_device` selection with serials | `copy`, `paste` with serials |
| `wl_seat` keyboard and pointer focus | section 4 |
| `wp_fractional_scale`, `wl_output` scale | the scale in configure events |
| `xdg_decoration` (server side) | section 6 (client-side decorations would be a later flag) |

What doesn't translate exactly is listed as the front end's problem, not the protocol's:
Wayland clients allocate their own buffers (the front end copies), and Wayland passes file
descriptors (the front end turns them into capabilities).

### 13. Testing

- **On the host**: the display server's logic as libraries, like `lib/shell`: the arrangement
  (defaults, scales from millimetres, a screen going and coming, mirroring), stacking and focus
  rules, input routing (focus, pressed-window, popups, serials and their one-second life), the
  clipboard's rules, the limits, configure and buffer exchanges with a hostile client (stale
  serials, frames it doesn't hold, windows it doesn't own, lists too long), and the compositor's
  drawing against reference pictures (two windows, a shadow over a window, a window across two
  screens of different scale, rotation).
- **In QEMU**, with test programs before any real app exists (stage 5, note 0026): windows
  appear where asked and are drawn (the description of the screens, below, and screenshots);
  a window dragged to the 2× screen is told the new scale and redraws; a screen going moves its
  windows; mirroring; keys reach only the focused window and the pointer only the one under it;
  a hostile client (`tests/programs/windowprobe`) tries every row of section 8's table and
  fails each; a hung client is marked not responding while the others carry on; the display
  server is crashed and comes back, and apps end and the desktop starts again.
- **The description of the screens.** When started with `selftest` or `describe`, the display
  server writes to the kernel log, after each change, one line per window: `[display] screen 1
  window 3 "Notepad — letter.txt" app 7 at 120,80 size 400x300 scale 200 focused`, plus the
  pointer's place and the clipboard's forms, so the runner can check what is on the screens
  without comparing pictures, as the console's mirror does for text (note 0018).

## Alternatives considered

- **Wayland as our protocol** (Q1). It would bend the design around Unix file descriptors and
  global object numbers, and make client-allocated buffers, which Q3 rules out, the norm.
- **Apps drawing their own title bars** (Wayland's usual way). Any app could then draw a
  convincing file picker or permissions prompt, which the whole sandbox relies on the user
  trusting.
- **Apps allocating their buffers.** Revoking memory unmaps it everywhere (note 0006), so an
  app could make the display server fault at will.
- **One program for the display server and the desktop** (taskbar, start menu). A bug in the
  taskbar would then be a bug in the program holding every screen and all input; and the
  desktop is per session while the screens are not.
- **A display server per session.** Screens and input are shared; something has to own them.
- **A separate clipboard service** (Q15). It would need the display server to vouch that a
  paste comes from the user, which is most of the work anyway.
- **Composing with a GPU.** There is no GPU driver yet; the buffer and fence design leaves the
  door open (note 0020).

## Open questions

- Keeping apps' windows across a display server restart.
- Charging window buffers to the session (or the app) rather than the display server.
- Locking or confining the pointer (games, ported 3D programs), and a pointer picture of an
  app's own.
- Drag and drop, with the clipboard's rules.
- Fractional scales (with outline fonts, M12); colour management.
- A trusted screenshot and screen recording tool (a flag of its own).
- Switching between several logged-in users (DESIGN.md "Beyond").
- Accessibility: a screen reader needs a way to read windows' contents that the sandbox allows.

## Changes
- 14 November 2026, M6 stage 6: the start-menu key's event to the desktop carries a serial,
  recorded as a press on the desktop's panel, so the desktop can open its start menu as a
  popup of the panel (a popup needs a press's serial, and a panel never has the keyboard). A
  popup of a panel may be as tall and wide as its screen (an app's popups stay within half), so
  the start menu fits on a small or 2x screen. `pbr.display.sessions` gains `@5 halted`, the
  farewell screen's last words when the computer can't be turned off by software. The window
  menu's description line gives its place and rows (`menu for window W of app A at X,Y size
  WxH: Minimise|...|Stop|Close`) so the desktop boot's runner can click Stop. A panel's `at`
  is (screen, edge) with 1 top, 2 bottom, 3 left, 4 right: not `pbr.window`'s resize `Edge`.
- 13 November 2026, building (M6 stage 5). The design held; what building settled or changed:
  - **The rules live in `lib/compositor`** (host-tested, like `lib/shell`): the space of
    screens, sessions, clients and windows with every check on what a client asks, input
    routing, the clipboard, decorations' geometry, drawing and composing, and the
    description lines. `services/display` is the program around it.
  - **`events` takes the app's own notification as the doorbell** and answers only the ring.
    Note 0023's event loop waits on one notification for everything (the doorbell bit 0, the
    receiver watch bit 1, the app's own bits 8 to 63), which only works if the app makes it.
    Signalling a notification never waits and fails cleanly if the app took it back, so the
    display server still never waits for an app.
  - **A key's second character comes with the value 4 (`VALUE_SECOND`)**, not 3: 3 is
    `pbr.seat`'s "still held" (note 0021, Changes), which a key event can also carry.
  - **`pbr.display.sessions.start` answers with the session's number too, and `activate`
    takes the number.** A service can't tell which of its own capabilities it has been passed
    back (the kernel doesn't say a received capability's badge), and only the users service
    can call `pbr.display.sessions` at all.
  - **Capabilities' rights.** A session's capability is derived with send, transfer, copy and
    revoke: the sandbox manager can make a copy for each app and ask for the app's client
    capability through it, so that revoking that copy takes exactly that app's windows (stage
    6). A client's capability is derived with send and transfer only: it can't be copied, so
    its watched badge goes when the app is done with it, and the app's windows go then.
  - **Backgrounds and panels name their screen with `at.x`**, and a panel its edge with
    `at.y` (an `Edge`: top, bottom, left, right); a panel's thickness is its height (or width
    at the left or right), at most a quarter of the screen. One background per screen and one
    panel per edge, per session; one whose screen went isn't shown.
  - **Where windows open**: a normal window cascades across the main screen's work area,
    24 pixels further down and right than the one before (eight steps, then round again); a
    dialog is centred on its parent; a system window is centred on the main screen, and one
    from `create_over` on the window it is over.
  - **Copying needs a press's serial from the last second too**, as pasting does (section 5
    asked only for the focus): an app with the focus could otherwise copy on a timer. A copy
    with the same serial as the last one from the same client adds its form to the same item.
  - **`Stale` and `BadSerial` for `buffers`**: `Stale` for one of the window's own last eight
    configure serials, `BadSerial` for anything else. A configure that changed only the state
    gets the same memory again (a copy of the same capability), not new buffers.
  - **The size limits on buffers are per side**: no wider than the widest screen and no
    taller than the tallest (1920 x 1200 while there are no screens).
  - **A decorated window is at least 160 logical pixels wide**, so its title bar's parts
    always fit: the icon, some of the title, the "trusted" mark and three buttons.
  - **A dismissed popup** (its parent lost the focus, or the user clicked outside it) is
    hidden at once and its app gets a close request; it stays until the app destroys it.
  - **Keys**: a key's release goes where its press went, even if the focus moved meanwhile,
    so no app thinks a key is still held. Alt+Tab goes through the focused window, then the
    others from the top down, then minimised ones, for as long as Alt is held.
  - **"Covered entirely"** (no frame events) is worked out by taking the windows above out of
    the window's rectangle, giving up (and sending frames) past 64 pieces.
  - **The window menu** is drawn by the display server over everything; "Move to screen…" is
    a row for each other screen.
  - **A tablet's screen**: tablets in the seat's order are tied to screens in the space's
    order when there are as many of each.
  - **The description** is a block from `view N` to `view N done`: a line per screen, a
    line per window of the active session back to front (with its kind, `drawn` once it has
    committed a frame, and `focused`, `maximised`, `minimised`, `dismissed`, `attention`,
    `not-responding`), the clipboard, the pointer (whose moves count as changes), and the
    window menu.
  - **The display server calls its display drivers from its main thread** (setting screens
    up, presenting), so a slow driver delays its answers to apps: on the emulated BIOS
    machine the framebuffer's set-up took about ten seconds. Drivers are system programs, so
    this is waiting on the trusted side, not on an app; moving screen set-up to the watcher
    thread is left for when it matters on real hardware.
  - **The display server checks its displays' drivers once a second** (a driver that crashed
    while nothing was being drawn would otherwise go unnoticed until the next picture), and
    a screen whose present hasn't said it is done within two seconds is drawn again.
  - **Testing**: the host tests are as section 13 lists (`lib/compositor/tests/rules.rs`
    and `draw.rs`, ten reference pictures). In QEMU, stage 5 runs the desktop boot with the
    root program standing in for the users service and the sandbox manager (note 0026,
    Changes).
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
