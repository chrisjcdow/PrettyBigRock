# 0023: Drawing, windows and sandstone: the libraries apps are built with

Status: Accepted · 9 November 2026 · Related: DESIGN.md sections 8 ("Text and fonts") and 9, D16;
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
  outside, and a **scale** (1 to 8). Drawing calls take logical coordinates and multiply by
  the scale, so an app draws the same at 1× and 2×. `canvas.sub(area)` gives a canvas on part
  of another, with its own (0, 0) and unable to draw outside the area: how a window is shared
  out among its parts.
- **A canvas is always solid.** Its pixels' fourth byte is unused (screens and window frames
  are `Bgrx`); drawing sets it to 255. **Pictures** drawn *from* may be solid or carry alpha.
- **Colours** are `0xAARRGGBB`, with alpha 0 (clear) to 255 (solid), not multiplied into the
  other three; drawing blends, rounding to the nearest value, so alpha 255 gives exactly the
  colour and alpha 0 changes nothing.
- **What it draws:** filled and outlined rectangles, rounded rectangles (corner radius),
  vertical and horizontal gradients, soft shadows (as 1.3's windows have), lines of any width
  with round ends (Paint's brush), circles and ellipses, filling a region of one colour (a
  flood fill), copying and blending pictures, and scaling a picture (nearest for whole-number
  factors, smooth for others; the compositor's windows of another scale), rotating by quarter
  turns.
- **No anti-aliasing.** A device pixel is in a shape if its centre is. Shapes are worked out
  at the device's resolution (a circle at 2× is a true circle of twice the radius), and are
  crisp like the font; pictures compare exactly and flood fill has clean edges. The thin
  (one-pixel) line is 1.3's (Bresenham).
- **Smooth scaling is by area**: each new pixel is the average of the picture's pixels it
  covers, weighted by how much of each it covers (and, with alpha, by how solid each is).
- **Text:** UTF-8 strings in a bitmap font, at whole-number sizes (Chris's answer to Q7),
  plain or bold (1.3's bold: each dot drawn again one to the right); measuring a string's
  width; drawing clipped. Every character takes one cell (6 dots wide, 7 in bold; lines are
  11 dots apart). A character the font lacks is drawn as a box.
- **Integer arithmetic only**, so the same drawing gives the same pixels on x86-64, ARM64 and
  the host: that is what lets tests compare pictures exactly.
- **Nothing a caller passes can make it misbehave**: positions and sizes may be anything
  (negative, huge), nothing outside the clip is touched, and the work done is bounded by the
  clip, not by the shape (a rectangle a million pixels wide on a small canvas costs what a
  small one costs).
- **Changed areas:** a `Damage` type collects rectangles, in device pixels, and merges two
  when the rectangle around both holds at most 1024 pixels more than they do (so overlapping
  and nearby ones join, and a cross does not become a big square); at most 64, past which it
  uses their bounding box. For `present` and `commit`. A canvas can be given one, and then
  records every rectangle it changes.

### 3. Fonts: `lib/font`

- 1.x's hand-drawn 5 × 9 font stays the system's font, drawn at 1× and 2×.
- It gains the characters the keyboard layouts can type (note 0021): Latin-1 (`£`, `é`, `ê`,
  `ë`, `ô`, `ü`, `ß`…), `€`, and the Latin Extended-A letters, in the same style; and the
  punctuation apps' own text uses: curly quotes, en and em dashes, the ellipsis, a bullet, a
  minus sign, four arrows and a check mark. 334 characters. Accents on capitals need room
  above, so the font's cell becomes 5 × 11 (two rows above), with the line height 11; the
  console's rows change to match.
- **The drawings are text.** `lib/font/glyphs.txt` holds every character as 11 lines of `#`
  and `.`, which can be read and changed by eye; the table the library uses is generated from
  it, and a test fails if the two differ (`PBR_UPDATE_FONT=1 cargo test -p pbr-font` writes
  the table again).
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

- **Reference pictures.** Tests compare drawing with pictures kept in the repository
  (`tests/pictures/`) as PNG files, written without compression (PNG allows that), so that
  GitHub shows them and a small host-only helper of our own (`lib/testpng`, using `lib/crc`)
  can write and read them with no outside code. `PBR_UPDATE_PICTURES=1 cargo xtask test` writes
  them afresh when a change to the look is meant; the change then shows in the commit's
  pictures. Pictures are compared exactly; when one differs, what was drawn is saved beside
  the system's temporary files and the test says where.
- **`lib/draw`** on the host: every operation at 1× and 2×, clipped at each edge, with alpha,
  outside the canvas, at hostile sizes (zero, huge, negative positions), against pictures; text
  in every glyph of the font; damage merging against a model. Each operation's rule is also
  written a second time in its tests, as the slowest and plainest per-pixel function, and the
  two must agree on many generated cases: the pictures show a person that the look is right,
  the models show that every pixel follows the rule.
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
- 14 November 2026, building sandstone (M6 stage 7). Section 5 held: view and update, state kept
  between views, the widgets of the table. The core (the `App` trait, elements and messages,
  the `Context`, the state tree, layout, focus, shortcuts, damage, popups and dialogs) and the
  harness were written first by Opus; the widgets by Sonnet subagents to six briefs on it,
  each reviewed with its pictures; the gallery likewise. What building settled or changed:
  - **`App`** has `view` and `update` as section 5 says, and optional methods with defaults:
    `title` (the part after the manifest's, "Notepad — letter.txt"), `start`, `shortcuts`,
    `close_requested` (`None` closes; a message lets Notepad ask first), `clipboard_changed`,
    `notified` (bits 8 to 63), and `view_window`/`window_title` for other windows the app opens
    with `Context::open_window`. **`view` returns `Element<'_, Message>`**: an element may
    borrow the app's state (the text editor shows the app's text, a list the app's rows)
    without copying it, and the view is dropped before `update` runs.
  - **Widgets** are values behind a `Widget` trait (`Box<dyn Widget>`); containers give their
    children through `children()` and get the tree walks (focus order, looks, descriptions,
    the pointer's shape, popups, shortcuts) for free. `Element::map` changes the message type,
    which is how sandstone's own dialogs sit beside an app's view.
  - **Widgets' state** is a tree beside the elements, matched after each view by kind (the
    widget's type) and, among siblings, by the app's id or else the next unused state of the
    same kind: an element put in before others leaves their state alone.
  - **Layout** is limits down, sizes up (flexbox's shape, cut down): `Shrink`, `Fill`,
    `Portion(n)`, `Fixed(n)`; a child that fills across is first measured with no limit across,
    then laid out at the row's depth. Layout can't send messages, so a widget whose content
    depends on its size (the terminal's columns) hears `Event::Resized` once the window is laid
    out at a new size.
  - **Drawing only what changed** is done by each drawing widget's *look*, a hash of everything
    its pixels depend on: the rectangles whose look or place changed are drawn again (the whole
    tree, clipped to them) and committed with that damage. A widget that draws part of a big
    content (a list, the editor, a scrolled area) hashes only what is in view; the terminal
    gives a look per row. **The harness checks this after every step of every test**, drawing
    each window whole and comparing it with what drawing the changes left.
  - **Focus**: Tab and Shift+Tab through focusable widgets in tree order; a main window starts
    with nothing focused (the app may focus something in `start`); a dialog starts with its
    line to type, or its default button, focused. Toolbar buttons are never focusable. Keys go
    to the focused widget, then the shortcuts (menus' items', then the app's), then Tab, Enter
    (the default button) and Escape (the cancel button; a dialog's close).
  - **Serials**: a button's message comes when it is let go, and the release has no serial, so
    the last press's serial is used for what follows within 0.9 seconds (copying, a popup).
  - **Popups** are windows of their own whose contents the widget that opened them gives (a
    menu's items); keys go to an open popup; it closes when it sends a message, on Escape, or
    when the display server dismisses it.
  - **Dialogs** (`Dialog::message`, `question`, `input`) are sandstone's: modal `Dialog`
    windows sized to their contents, made of its own widgets; closing one is its cancel button.
  - **The theme** is 1.3's colours and sizes. **Its heading style is one setting**,
    `theme::HEADING`, which the desktop's start menu uses too, so they always match; it is the
    font at size 2 until Chris chooses.
  - **`TerminalView` draws from a `Cells` trait** that Terminal implements over `lib/terminal`'s
    grid, rather than wrapping the grid: `lib/terminal` is GPL and sandstone MPL-2.0.
  - **`Image`**: an app's pixels borrowed for an icon, a button or a list row. `Canvas` is drawn
    again only when the app changes its version number.
  - **The harness** (`sandstone::testing`, with the `testing` feature, which brings in
    `lib/testpng`, a GPL test helper) runs an app against a pretend display server that keeps
    note 0022's rules where they matter (configure events, the keyboard to the newest window,
    popups only in answer to a recent press, the clipboard only with the last press's serial).
  - **`sandstone::picker`** waits for stage 8, with the picker. `pbr.picker`'s calls wait for
    the user, so they will run on a helper thread and answer the event loop through its own
    bits (section 4), not block it.
  - **Tests**: about 200 on the host (`lib/sandstone/tests/`), with reference pictures of every
    widget at 1x and 2x in `tests/pictures/sandstone/`; in QEMU, the gallery
    (`tests/programs/gallery`), described below in note 0026's Changes.
- 13 November 2026, building `lib/window` (M6 stage 5, written by a Sonnet subagent to a brief
  and reviewed). Section 4 held; what building settled:
  - **One notification really does carry everything**: `pbr.window.events` now takes the
    app's own notification as its doorbell (note 0022, Changes), so the ring (bit 0), the
    display server going (bit 1, a receiver watch) and the app's own bits (8 to 63) wake the
    same wait.
  - **`draw(window, |canvas| damage)`**: the closure returns the rectangles it changed (a
    `pbr_draw::Damage`, logical pixels; empty means all). The two frames take turns, so the
    frame drawn in is one commit behind: before the closure runs, the rectangles of the last
    commit are copied into it from the other frame. That is what lets an app redraw only what
    changed.
  - **Waiting for a frame**: if both frames are with the display server, `draw` reads the ring
    (queueing other events, in order) for up to thirty seconds for a released event, then
    gives up with `TimedOut`. Calls to the display server wait thirty seconds too: it answers
    at once unless it is busy with a display driver itself, which on an emulated machine has
    taken ten seconds (the BIOS framebuffer's set-up, in the desktop boot).
  - **After a lost event** every window's frames are taken as the app's again: released events
    may have been among those lost, and an app waiting for one would wait for ever. At worst a
    commit is refused once as `Stale`.
  - **Buttons, scrolling and leaving carry no position on the wire**: `lib/window` fills in the
    last position the window was sent.
  - Configure events are handled inside (buffers asked for only when the size or scale
    changed) and still given to the app, so it knows to redraw.
  - **Tests**: the plain parts on the host (decoding every event, garbage included; the timer
    queue; which frame is held after commits and releases, and what is copied before drawing).
    The connection itself is tested in QEMU by the desktop boot, where `tests/programs/wintest`
    and `windowprobe` use it, rather than against a fake display server on the host as section
    6 planned: the system calls are the part a fake would leave out.
- 12 November 2026, building (M6 stage 3: `lib/draw`, `lib/font`, `lib/testpng`). What
  building settled or changed, now also in sections 2, 3 and 6 above:
  - **A canvas is always solid; only pictures drawn from carry alpha.** Window frames and
    screens are `Bgrx` (notes 0020 and 0022), so there is nothing for a canvas's own alpha to
    mean, and mixing onto a see-through canvas would need a division for every pixel.
  - **No anti-aliasing**, and the pixel-centre rule for every shape (the draft didn't say).
    It keeps 1.3's crisp look beside the bitmap font, and Paint's flood fill needs clean
    edges. Smooth edges can come with outline fonts (M12), as a new option, not a change.
  - **Smooth scaling is averaging by area** (the draft said only "smooth").
  - **Damage joins two rectangles only when that is cheap** (1024 wasted pixels at most), not
    whenever they overlap: a window's frame is four thin bars, and joining them would show
    the whole window again.
  - **A soft shadow** fades as the square of the distance left (`(2B − e)² / (2B)²` of the
    colour's alpha, `e` twice the distance outside the rounded rectangle, `B` the blur), with
    the distance's square root rounded **up**, so that the pixels at full strength are exactly
    the rounded rectangle's. (The brief for it said rounded down, which gave full strength to
    a few pixels just outside each corner; the subagent writing it found that.)
  - **Lines.** One device pixel wide: Bresenham's, as 1.3's. Wider: every pixel whose centre
    is within half the width of the segment. An even width at an odd scale is centred half a
    pixel up and left, so that a horizontal line is exactly as many rows thick as asked.
  - **Flood fill** compares exact colours, spreads up, down, left and right, and only inside
    the clip.
  - **Text**: one cell a character, bold as 1.3's double strike, sizes 1 to 16. The console
    still shows `?` for a character it can't draw (`pbr_font::glyph`); `lib/draw` shows a box
    (`pbr_font::lookup`).
  - **The font has 334 characters**: the draft's Latin-1, Latin Extended-A and `€`, plus
    curly quotes, dashes, the ellipsis, a bullet, a minus sign, arrows and a check mark, which
    the apps' own text needs ("Open with…", "•" for unsaved, "−" on Calculator). With five
    dots of width the en dash, em dash and minus sign are the same bar; `Đ` and `Ð` are the
    same letter. The drawings moved from a table of numbers to `lib/font/glyphs.txt`.
  - **The console's lines.** The 11-row cell is the whole line: the console and the kernel's
    screen have as many lines as before (the two rows of space that were under each line are
    now the two rows above the capitals), with each letter two rows lower in its cell. The
    cursor's underline is on the cell's last row, where the tails of `g` and `y` end.
  - **The heading font: not added.** Headings are the same font at size 2 or 3, or bold, as
    1.3's were (`tests/pictures/draw/look-2x.png` and `text-styles-2x.png` show them). The
    question stays open for Chris until the desktop's real look is on screen (stage 6).
  - **Reference pictures** are RGB PNG files (RGBA only when a picture has alpha) in
    `tests/pictures/draw/`, 20 of them, about 7 MiB as files (git compresses them).
  - `lib/draw` is MPL-2.0 as planned; `lib/testpng` is GPL (it is only ever a test's helper).
  - Written by Sonnet subagents to four briefs (`lib/testpng`; the font's characters; shapes,
    lines and flood fill; pictures and text) on a core written first (the canvas, colours,
    rectangles, damage), and reviewed in full. Ten glyphs were redrawn in review.
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
