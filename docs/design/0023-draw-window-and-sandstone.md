# 0023: Drawing, windows and sandstone: the libraries apps are built with

Status: Draft · 9 November 2026 · Related: DESIGN.md sections 8 ("Text and fonts") and 9, D16;
notes 0009, 0014, 0021, 0022, 0025, 0026; milestone M6

## The problem

DESIGN.md section 9 says the native library covers "windows and drawing (through the display
service)". M6's apps (seven of them, plus the desktop, the file picker and the permissions
viewer), the display server and the console all need to draw; every windowed program needs to
talk `pbr.window` and wait for its events; and the apps need the same buttons, text fields,
lists, menus and dialogs, looking and behaving the same. Chris named the native library
`granite` and kept **sandstone** for a layer on top, such as the window toolkit (Q6 of the M6
plan: the window client and drawing as granite-level libraries beside `granite`, sandstone the
widget toolkit, built in M6).

## The design

### 1. The layers

```
 apps ─── lib/sandstone   widgets, layout, focus, dialogs              (MPL-2.0)
              │
          lib/window      pbr.window's client half, the event loop     (MPL-2.0)
              │
          lib/draw        pixels, shapes, text, pictures              (MPL-2.0)
              │                 lib/font (the bitmap fonts)
          granite, lib/interfaces (generated code), lib/files
```

- **`lib/draw`** depends on nothing but `core`, `alloc` and `lib/font`, so it builds for the
  host and is tested there. The display server and the console use it too.
- **`lib/window`** can't be part of `granite` itself, for the reason `lib/files` isn't (note
  0014, Changes): it calls generated interface code, which is built on `granite`. It is a
  "granite-level" library: the lowest layer an app uses for windows.
- **`lib/sandstone`** is the toolkit apps are written with. An app can also use `lib/window` and
  `lib/draw` directly (a game, or a ported program with its own toolkit).
- All three are **MPL-2.0** like `granite` (D16), so that apps of any licence can use them.

### 2. `lib/draw`

- **A canvas** is a view of 32-bit `Bgrx` pixels (a window's frame, a screen's buffer, a
  picture in memory): its width, height and stride, a **clip rectangle** nothing is drawn
  outside, and a **scale**. Drawing calls take logical coordinates and multiply by the scale,
  so an app draws the same at 1× and 2×.
- **Colours** are `0xAARRGGBB`, with alpha 0 (clear) to 255 (solid); drawing blends.
- **What it draws:** filled and outlined rectangles, rounded rectangles (corner radius),
  vertical and horizontal gradients, soft shadows (as 1.3's windows have), lines of any width
  (Paint's brush), circles and ellipses, filling a region of one colour (a flood fill), copying
  and blending pictures, and scaling a picture (nearest for whole-number factors, smooth for
  others; the compositor's windows of another scale), rotating by quarter turns.
- **Text:** UTF-8 strings in a bitmap font, at whole-number scales (Chris's answer to Q7);
  measuring a string's width; drawing clipped. A character the font lacks is drawn as a box.
- **Integer arithmetic only**, so the same drawing gives the same pixels on x86-64, ARM64 and
  the host: that is what lets tests compare pictures exactly.
- **Changed areas:** a `Damage` type collects rectangles and merges overlapping and nearby ones
  (at most 64, past which it uses their bounding box), for `present` and `commit`.

### 3. Fonts: `lib/font`

- 1.x's hand-drawn 5 × 9 font stays the system's font, drawn at 1× and 2×.
- It gains the characters the keyboard layouts can type (note 0021): Latin-1 (`£`, `é`, `ê`,
  `ë`, `ô`, `ü`, `ß`…), `€`, and the Latin Extended-A letters, in the same style. Accents on
  capitals need room above, so the font's cell becomes 5 × 11 (two rows above), with the line
  height 11; the console's rows change to match.
- **A larger bitmap font for headings** is added only if the look needs one (the About window,
  the start menu's title), decided while building stage 3 with Chris.
- Outline fonts (TrueType), fractional scales and text shaping wait for M12.

### 4. `lib/window`

- **`Connection::from_start(&mut start)`** takes the app's `pbr.window` capability (start-up
  role 1003, given by the sandbox manager, note 0024), asks for the event ring, and sets a
  receiver watch on the display server (note 0019).
- **`Window`** wraps one window: `create(&conn, kind, title, size, flags)`, `set_title`,
  `set_limits`, `set_cursor`, `minimise`, `maximise`, `start_move`, `start_resize`. It handles
  configure events by itself: asks for buffers for the new serial, maps them, and keeps track of
  which frame it holds.
- **Drawing** is `window.draw(|canvas| { … })`: it gives a canvas over the frame the app holds
  (at the window's scale), and commits it with the damage the closure reports. If neither frame
  is the app's (both are with the display server), it waits for a released event.
- **The clipboard:** `conn.copy_text`, `copy_picture`, `paste_text(serial)`, `paste_picture`.
- **The event loop.** One notification carries everything an app waits for: the ring's
  doorbell (bit 0), the receiver watch (bit 1), and bits 8 to 63 for the app's own (another
  thread finishing work, say). `EventLoop::run(handler)` waits on it with a deadline set by the
  nearest **timer** (`set_timer(id, at)`, `cancel_timer(id)`), and calls the handler with one
  `Event` at a time:

  ```rust
  enum Event {
      Configure { window, size, scale, focused, maximised, minimised, shown },
      CloseRequested { window },
      Key(KeyEvent),            // code, modifiers, value, text, typed, serial
      Pointer(PointerEvent),    // moved, entered, left, button, scroll; position, buttons, serial
      Frame { window },
      ClipboardChanged { text, picture },
      Timer(u32),
      Notified(u64),
      Disconnected,             // the display server has gone (note 0022, section 11)
  }
  ```

  `KeyEvent.typed` is false when Ctrl, Alt or Meta is held, so that a text field doesn't type
  the `s` of Ctrl+S (note 0021, section 2). **Pings are answered by the loop itself**, between
  events: an app whose handler is stuck stops answering, which is how the display server knows
  it isn't responding.
- One thread runs the loop. An app may start others, and tell the loop with its own bits.

### 5. `lib/sandstone`

**The model: view and update.** An app is a type that implements `sandstone::App`:

```rust
trait App {
    type Message;
    /// What the window shows now, built afresh from the app's state.
    fn view(&self) -> Element<Self::Message>;
    /// What happens when the user does something, or a timer or task finishes.
    fn update(&mut self, message: Self::Message, cx: &mut Context);
}
```

The app keeps its own state (Notepad's text and file, Paint's picture); `view` describes the
window from it as a tree of **elements** (buttons, fields, rows and columns); sandstone lays it
out, draws it, and turns the user's clicks and keys into the app's **messages**, which go to
`update`. Widgets' own small state (which field has the focus, where its cursor is, how far a
list is scrolled) is kept by sandstone between views, matched by each element's place in the
tree or an id the app gives it. This is the shape of Elm and of Rust's `iced`: it suits Rust
well (no shared mutable objects pointing at each other), and it makes every app's behaviour
testable without a screen.

**The widgets in M6** (what the apps of note 0026 need):

| Widget | For |
|--------|-----|
| `Label`, `Icon` | Text and small pictures |
| `Button`, `IconButton`, `ToggleButton`, `CheckBox` | Toolbars, Calculator's keys, Paint's tools and colours |
| `TextField` | One line: names in dialogs, the picker's file name |
| `TextEditor` | Many lines: Notepad. Selection, the clipboard, scrolling, undo and redo, finding, long lines and large files (it keeps lines, not one string) |
| `TerminalView` | A grid of characters with a cursor, for Terminal (wraps `lib/terminal`'s grid) |
| `List` | Rows with an icon and columns (Files, the picker, the permissions viewer): selection with the keyboard and pointer, activation by double click or Enter, sorting by column |
| `Scroll` | Scroll bars around anything bigger than its space |
| `Row`, `Column`, `Grid`, `Space`, `Stretch`, `Panel` | Layout |
| `MenuBar`, `Menu`, context menus | Popups (note 0022, section 3), opened by a click or a key |
| `Toolbar`, `StatusBar` | |
| `Canvas` | Drawing of the app's own with `lib/draw`, with raw pointer and key events (Paint, Snake) |
| `Dialog` | A message, a question (with buttons), or a line of text to type, as 1.3's `wm_dialog` and `wm_message` |

- **The file picker** is reached through `sandstone::picker::open(...)` and `save(...)` (note
  0025); they answer with a message to `update` when the user has chosen.
- **Keyboard use**: Tab and Shift+Tab move the focus; Enter activates the default button and
  Escape the cancel button of a dialog; Space toggles; arrow keys move in lists and grids;
  **shortcuts** (`Shortcut::ctrl('s')`) match the character a key types, not its place
  (note 0021).
- **The look** is a `Theme`: 1.3's colours, corner radii, spacing and font, scaled with the
  window. One theme in M6 (note 0024, section 6 has the look).
- **Drawing**: after each `update`, sandstone builds the view again, lays it out, and draws
  what changed (each element compared with its predecessor) into the window, with that damage.
- **Window kinds**: an app's main window and its dialogs are `Normal` and `Dialog`
  windows; menus are `Popup`s; the picker and permissions viewer (system clients, note 0025)
  use `System`.

### 6. Testing

- **Reference pictures.** Tests compare drawing with pictures kept in the repository as PNG
  files, written without compression (PNG allows that), so that GitHub shows them and a small
  host-only helper of our own (`lib/testpng`, using `lib/crc`) can write and read them with no
  outside code. `PBR_UPDATE_PICTURES=1 cargo xtask test` writes them afresh when a change to the
  look is meant; the change then shows in the commit's pictures. Pictures are compared exactly.
- **`lib/draw`** on the host: every operation at 1× and 2×, clipped at each edge, with alpha,
  outside the canvas, at hostile sizes (zero, huge, negative positions), against pictures; text
  in every glyph of the font; damage merging against a model.
- **`lib/window`** on the host, against a fake display server: configure, buffers, frames,
  released and frame events, the timer queue, pings answered, the ring lost-event path, the
  display server going.
- **`lib/sandstone`** on the host: `sandstone::testing::Harness` runs an `App` against a fake
  window: it sends clicks and keys ("click the button labelled Save", "type `hello`"),
  checks messages and state, and compares the window with pictures. Each widget's behaviour
  (selection, editing, the clipboard, undo, scrolling, keyboard focus, menus) and look at 1× and
  2×.
- **In QEMU**: a widget gallery test program (`tests/programs/gallery`), driven by the runner,
  checked through the display server's description and screenshots (stage 7, note 0026).

## Alternatives considered

- **The window client inside `granite`.** Not possible without moving the generated interface
  code into `granite` (note 0014, Changes), which would make `granite` depend on every
  interface.
- **Immediate-mode widgets** (each frame the app calls `ui.button("OK")`, as egui does). Simple
  to start with, but keyboard focus, layout that depends on sizes measured later, and
  accessibility (later) are harder; and the whole window is drawn on every event.
- **Classic objects with callbacks** (GTK, Qt). In Rust they need shared, mutable objects
  pointing at each other (`Rc<RefCell<…>>` everywhere), which is the hardest kind of Rust to get
  right; view and update avoid it.
- **Floating point in drawing.** Pictures would differ in their last bits between processors,
  and tests couldn't compare exactly.
- **Reference pictures as BMP or raw files.** GitHub doesn't show them, so a change to the look
  couldn't be reviewed by eye.

## Open questions

- Outline fonts, fractional scales, text shaping, right-to-left text (M12).
- Themes (a dark one), and following a user setting.
- Accessibility: describing the element tree to a screen reader.
- Animations (a menu sliding open) and drawing on a GPU.
- Drag and drop between widgets and apps.

## Changes
- 9 November 2026: first draft, for discussion with Chris.
