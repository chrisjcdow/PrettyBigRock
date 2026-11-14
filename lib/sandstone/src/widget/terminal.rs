// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`TerminalView`] and [`Canvas`]: the two widgets that show something the app draws or keeps
//! itself (note 0023, section 5; note 0026, section 1).
//!
//! `TerminalView` is a grid of characters with a cursor, in the look of 1.3's Terminal
//! (`classic/kernel/term.c`, `t_draw`). `Canvas` is a rectangle the app draws on with
//! `lib/draw` and gets raw pointer and key events from (Paint's picture, Snake's board).
//!
//! A note on names: inside sandstone, `Canvas` alone is the *widget* of this file. What is
//! drawn on, `lib/draw`'s canvas, is always written `pbr_draw::Canvas`.

use crate::Look;
use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::event::{Button as MouseButton, Event, Key, KeyPress, Pointer, PointerKind, Status};
use crate::geometry::{Length, Limits, Point, Rect, Size};
use crate::layout::Node;
use crate::shell::Shell;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::any::{Any, TypeId};
// `as DrawCanvas`: this file's own widget is called `Canvas`, so the one drawn on is renamed
// here (outside this file it is `pbr_draw::Canvas`).
use pbr_draw::{Canvas as DrawCanvas, Colour};
use pbr_window::Cursor;

/// How far in from the left (and right) edge the characters start.
const PAD_X: i32 = 8;
/// How far down from the top (and up from the bottom) the first row starts.
const PAD_Y: i32 = 6;
/// 1.3's cursor green, which is also its prompt's.
const CURSOR: Colour = Colour::solid(0x8fd18a);
/// 1.3's paper.
const PAPER: Colour = Colour::solid(0x14161d);
/// What a view of no known size asks for: 80 columns by 24 rows.
const DEFAULT_COLUMNS: i32 = 80;
const DEFAULT_ROWS: i32 = 24;
/// How many lines one step of the wheel is.
const WHEEL_LINES: i32 = 3;

// ---------------------------------------------------------------------------------------------
// TerminalView
// ---------------------------------------------------------------------------------------------

/// How tall a row of the grid is: 1.3's line height of 12 (the font itself is 11 rows high, so
/// this leaves a dot between lines), at the font's size.
fn line_of(t: &crate::Theme) -> i32 {
    12 * t.font.size.clamp(1, 16) as i32
}

/// One character cell of a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character (a space for an empty cell).
    pub ch: char,
    /// Its colour (`pbr_draw::Colour`).
    pub colour: Colour,
    /// Drawn bold.
    pub bold: bool,
}

impl Cell {
    /// A plain (not bold) cell.
    pub const fn new(ch: char, colour: Colour) -> Cell {
        Cell { ch, colour, bold: false }
    }
}

/// What a [`TerminalView`] shows, which the app implements. Terminal's own grid lives in a
/// GPL library that sandstone (MPL-2.0) must not depend on, so the view only asks the app
/// through this trait. A *trait* is Rust's word for a set of methods a type promises to have;
/// the view holds `&dyn Cells`, a reference to any type that has them.
pub trait Cells {
    /// Columns and rows.
    fn size(&self) -> (usize, usize);
    /// The character at a place, and its colour (a cell outside the size: a space).
    fn cell(&self, column: usize, row: usize) -> Cell;
    /// Where the cursor is, if it is shown.
    fn cursor(&self) -> Option<(usize, usize)>;
}

/// A grid of characters with a cursor: Terminal's window. It shows as many cells as fit (the
/// app sizes its grid to what [`TerminalView::on_resize`] tells it), keys go to the app
/// through [`TerminalView::on_key`], and the mouse selects text to copy.
///
/// `'a` is a lifetime: the view borrows the app's cells and closures for as long as the view
/// lives, and Rust checks that nothing is used after it is gone. `M` is the app's message type.
///
/// The look is 1.3's: dark paper, cells as wide as the font's advance and as tall as its line
/// (6 x 12), 8 pixels in and 6 down; the cursor is a green block with its character on it in
/// the paper's colour, shown only while the view has the keyboard. A selection is drawn
/// reversed: each cell filled in its own colour with the character in the paper's, which reads
/// well on dark paper.
pub struct TerminalView<'a, M> {
    cells: &'a dyn Cells,
    on_key: Option<Box<dyn Fn(KeyPress) -> Option<M> + 'a>>,
    on_paste: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_scroll: Option<Box<dyn Fn(i32) -> M + 'a>>,
    on_resize: Option<Box<dyn Fn(usize, usize) -> M + 'a>>,
    background: Colour,
    width: Length,
    height: Length,
}

/// A cell of the grid as (column, row).
type At = (usize, usize);

/// What a terminal view remembers between views.
#[derive(Default)]
struct TermState {
    /// The left button went down on the view and hasn't come up.
    dragging: bool,
    /// The cell the press was on.
    anchor: At,
    /// The selection: from one cell to another, either way round, both included.
    selection: Option<(At, At)>,
    /// The columns and rows last told to `on_resize`.
    said: Option<At>,
}

impl<'a, M> TerminalView<'a, M> {
    /// A view of `cells`.
    pub fn new(cells: &'a dyn Cells) -> TerminalView<'a, M> {
        TerminalView {
            cells,
            on_key: None,
            on_paste: None,
            on_scroll: None,
            on_resize: None,
            background: PAPER,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Every key press while the view has the focus, except its own (Ctrl+Shift+C and
    /// Ctrl+Shift+V) and presses of a modifier key alone, and except presses with Alt or the
    /// Windows key held (the window's). Give `Some(message)` for a key the app used (it is
    /// then Captured, so Tab and Enter reach the shell too); `None` leaves it for the window.
    pub fn on_key(mut self, f: impl Fn(KeyPress) -> Option<M> + 'a) -> TerminalView<'a, M> {
        self.on_key = Some(Box::new(f));
        self
    }

    /// What Ctrl+Shift+V, or a middle click, sends: the clipboard's text.
    pub fn on_paste(mut self, f: impl Fn(String) -> M + 'a) -> TerminalView<'a, M> {
        self.on_paste = Some(Box::new(f));
        self
    }

    /// What the wheel sends: lines, positive down (3 for each step of the wheel).
    pub fn on_scroll(mut self, f: impl Fn(i32) -> M + 'a) -> TerminalView<'a, M> {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// What is sent with the columns and rows that fit, once when the view first shows and
    /// again whenever that number changes (the window was resized).
    pub fn on_resize(mut self, f: impl Fn(usize, usize) -> M + 'a) -> TerminalView<'a, M> {
        self.on_resize = Some(Box::new(f));
        self
    }

    /// The paper's colour (1.3's `0x14161d` by default).
    pub fn background(mut self, colour: Colour) -> TerminalView<'a, M> {
        self.background = colour;
        self
    }

    /// How wide it wants to be (all the room there is by default).
    pub fn width(mut self, w: Length) -> TerminalView<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be (all the room there is by default).
    pub fn height(mut self, h: Length) -> TerminalView<'a, M> {
        self.height = h;
        self
    }
}

impl<'a, M: 'a> From<TerminalView<'a, M>> for Element<'a, M> {
    fn from(w: TerminalView<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// The columns and rows that fit in `rect` for cells `advance` by `line` big. Never under 1,
/// so that an app never has to cope with an empty grid.
fn fit(rect: Rect, advance: i32, line: i32) -> At {
    let across = (rect.width - 2 * PAD_X).max(0) / advance.max(1);
    let down = (rect.height - 2 * PAD_Y).max(0) / line.max(1);
    (across.max(1) as usize, down.max(1) as usize)
}

/// What the grid of cells shows: the cells that exist and fit.
fn shown(cells: &dyn Cells, rect: Rect, advance: i32, line: i32) -> At {
    let (c, r) = cells.size();
    let (fc, fr) = fit(rect, advance, line);
    (c.min(fc), r.min(fr))
}

/// The two ends of a selection in reading order (a row at a time, left to right).
fn ordered((a, b): (At, At)) -> (At, At) {
    if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) }
}

/// The part of `row` that is selected, as `[from, to)` columns of a grid `columns` wide.
fn span(selection: Option<(At, At)>, row: usize, columns: usize) -> Option<(usize, usize)> {
    let (start, end) = ordered(selection?);
    if row < start.1 || row > end.1 {
        return None;
    }
    let from = if row == start.1 { start.0 } else { 0 };
    let to = if row == end.1 { end.0.saturating_add(1) } else { columns };
    let (from, to) = (from.min(columns), to.min(columns));
    (from < to).then_some((from, to))
}

/// The text of a row's cells `[from, to)`, with trailing spaces removed.
fn row_text(cells: &dyn Cells, row: usize, from: usize, to: usize) -> String {
    let mut s: String = (from..to).map(|c| cells.cell(c, row).ch).collect();
    let kept = s.trim_end_matches([' ', '\0']).len();
    s.truncate(kept);
    s
}

impl<'a, M> TerminalView<'a, M> {
    /// Where on the grid the pixel (`x`, `y`) is, held within the cells shown (`None` if
    /// there are none).
    fn cell_at(&self, rect: Rect, x: i32, y: i32, advance: i32, line: i32) -> Option<At> {
        let (c, r) = shown(self.cells, rect, advance, line);
        if c == 0 || r == 0 {
            return None;
        }
        let col =
            (i64::from(x) - i64::from(rect.x) - i64::from(PAD_X)).div_euclid(advance.max(1) as i64);
        let row =
            (i64::from(y) - i64::from(rect.y) - i64::from(PAD_Y)).div_euclid(line.max(1) as i64);
        Some((col.clamp(0, c as i64 - 1) as usize, row.clamp(0, r as i64 - 1) as usize))
    }

    /// The selected text: rows joined with `\n`, each without its trailing spaces.
    fn selected_text(&self, s: &TermState) -> String {
        let Some(sel) = s.selection else {
            return String::new();
        };
        let (columns, rows) = self.cells.size();
        let ((_, first), (_, last)) = ordered(sel);
        let mut out = String::new();
        for row in first..=last.min(rows.saturating_sub(1)) {
            if row > first {
                out.push('\n');
            }
            if let Some((from, to)) = span(Some(sel), row, columns) {
                out.push_str(&row_text(self.cells, row, from, to));
            }
        }
        out
    }

    /// Tells the app how many columns and rows fit, if that differs from what it was last
    /// told. (Nothing is said before the first layout, when the rectangle is empty.)
    fn check_size(&self, tree: &mut Tree, node: &Node, shell: &mut Shell<'_, M>) {
        if node.rect.is_empty() {
            return;
        }
        let t = *shell.theme();
        let now = fit(node.rect, t.font.advance(), line_of(&t));
        let s = tree.state_mut::<TermState>();
        if s.said == Some(now) {
            return;
        }
        // A different size makes the old selection meaningless.
        if s.said.is_some() && s.selection.take().is_some() {
            s.dragging = false;
            shell.redraw();
        }
        s.said = Some(now);
        if let Some(f) = &self.on_resize {
            shell.publish(f(now.0, now.1));
        }
    }

    /// A key press for the focused view.
    fn key(&self, tree: &mut Tree, k: &KeyPress, shell: &mut Shell<'_, M>) -> Status {
        let m = k.modifiers;
        if m.alt() || m.meta() || k.key == Key::Modifier {
            return Status::Ignored;
        }
        if m.ctrl() && m.shift() {
            // The letter is by the character on the key: Shift makes it a capital.
            match k.ch.and_then(|c| c.to_lowercase().next()) {
                Some('c') => {
                    let text = self.selected_text(tree.state::<TermState>());
                    if !text.is_empty() {
                        shell.copy_text(&text);
                    }
                    return Status::Captured;
                }
                Some('v') => {
                    if let Some(on_paste) = &self.on_paste
                        && let Some(text) = shell.paste_text()
                    {
                        shell.publish(on_paste(text));
                    }
                    return Status::Captured;
                }
                _ => {}
            }
        }
        match self.on_key.as_ref().and_then(|f| f(*k)) {
            Some(message) => {
                shell.publish(message);
                Status::Captured
            }
            None => Status::Ignored,
        }
    }

    /// The pointer: a press takes the focus and starts a selection, moves while the button is
    /// down extend it, the wheel scrolls and a middle click pastes.
    fn pointer(
        &self,
        tree: &mut Tree,
        node: &Node,
        p: &Pointer,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let rect = node.rect;
        let (advance, line) = (shell.theme().font.advance(), line_of(shell.theme()));
        let inside = rect.contains(p.at.x, p.at.y);
        let s = tree.state_mut::<TermState>();
        match p.kind {
            PointerKind::Pressed { button: MouseButton::Left, .. } if inside => {
                shell.focus(id);
                if s.selection.take().is_some() {
                    shell.redraw();
                }
                if let Some(at) = self.cell_at(rect, p.at.x, p.at.y, advance, line) {
                    s.anchor = at;
                    s.dragging = true;
                }
                Status::Captured
            }
            PointerKind::Pressed { button: MouseButton::Middle, .. } if inside => {
                if let Some(on_paste) = &self.on_paste
                    && let Some(text) = shell.paste_text()
                {
                    shell.publish(on_paste(text));
                }
                Status::Captured
            }
            // Moves come even outside the view, so a drag can go past its edges.
            PointerKind::Moved if s.dragging => {
                if let Some(at) = self.cell_at(rect, p.at.x, p.at.y, advance, line) {
                    // Dragging within the first cell selects nothing.
                    let new = (at != s.anchor).then_some((s.anchor, at));
                    if new != s.selection {
                        s.selection = new;
                        shell.redraw();
                    }
                }
                Status::Captured
            }
            PointerKind::Released { button: MouseButton::Left } if s.dragging => {
                s.dragging = false;
                Status::Captured
            }
            PointerKind::Scrolled { dy, .. } if inside && dy != 0 => match &self.on_scroll {
                Some(f) => {
                    shell.publish(f(dy.saturating_mul(WHEEL_LINES)));
                    Status::Captured
                }
                None => Status::Ignored,
            },
            _ => Status::Ignored,
        }
    }

    /// Draws row `row`: its characters, the selection and the cursor, into `canvas` (already
    /// clipped to the row).
    fn draw_row(
        &self,
        canvas: &mut DrawCanvas<'_>,
        s: &TermState,
        node: &Node,
        row: usize,
        cx: &DrawCx<'_>,
        keyboard: bool,
    ) {
        let t = cx.theme;
        let (advance, line) = (t.font.advance(), line_of(t));
        let (columns, _) = shown(self.cells, node.rect, advance, line);
        let selected = span(s.selection, row, columns);
        let cursor = self.cells.cursor().filter(|_| keyboard);
        let y = node.rect.y + PAD_Y + (row as i32) * line;
        for col in 0..columns {
            let cell = self.cells.cell(col, row);
            let x = node.rect.x + PAD_X + (col as i32) * advance;
            let on_cursor = cursor == Some((col, row));
            let on_selection = selected.is_some_and(|(a, b)| col >= a && col < b);
            let colour = if on_cursor {
                canvas.fill_rect(Rect::new(x, y + 1, advance, line - 1), CURSOR);
                self.background
            } else if on_selection {
                canvas.fill_rect(Rect::new(x, y, advance, line), cell.colour);
                self.background
            } else {
                cell.colour
            };
            if cell.ch != ' ' && !cell.ch.is_control() {
                let mut buf = [0u8; 4];
                let text: &str = cell.ch.encode_utf8(&mut buf);
                let style = if cell.bold { t.bold } else { t.font };
                canvas.text(x, y, text, colour, style);
            }
        }
    }

    /// The hash of everything row `row` shows.
    fn row_look(
        &self,
        s: &TermState,
        node: &Node,
        row: usize,
        cx: &DrawCx<'_>,
        keyboard: bool,
    ) -> u64 {
        let t = cx.theme;
        let (columns, _) = shown(self.cells, node.rect, t.font.advance(), line_of(t));
        let mut look = Look::new().u64(columns as u64).u64(row as u64);
        for col in 0..columns {
            let c = self.cells.cell(col, row);
            look = look.u64(u64::from(c.ch)).u64(u64::from(c.colour.0)).bool(c.bold);
        }
        let (from, to) = span(s.selection, row, columns).unwrap_or((0, 0));
        look = look.u64(from as u64).u64(to as u64).u64(u64::from(self.background.0));
        let cursor = self.cells.cursor().filter(|(_, r)| *r == row && keyboard);
        look.bool(cursor.is_some()).u64(cursor.map_or(0, |(c, _)| c as u64)).finish()
    }
}

impl<'a, M: 'a> Widget<'a, M> for TerminalView<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<TermState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(TermState::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let (advance, line) = (cx.theme.font.advance(), line_of(cx.theme));
        let natural =
            Size::new(2 * PAD_X + DEFAULT_COLUMNS * advance, 2 * PAD_Y + DEFAULT_ROWS * line);
        Node::new(limits.resolve(natural))
    }

    /// Draws the paper, then every row that is inside what is being drawn (a row alone, when
    /// that is all that changed).
    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut DrawCanvas<'_>, cx: &DrawCx<'_>) {
        canvas.fill_rect(node.rect, self.background);
        let s = tree.state::<TermState>();
        let t = cx.theme;
        let (advance, line) = (t.font.advance(), line_of(t));
        let (_, rows) = shown(self.cells, node.rect, advance, line);
        let keyboard = cx.has_keyboard(tree);
        for row in 0..rows {
            let r = row_rect(node.rect, row, line);
            if r.is_empty() || r.intersect(&cx.clip).is_empty() {
                continue;
            }
            let mut c = canvas.clipped(r);
            self.draw_row(&mut c, s, node, row, cx, keyboard);
        }
    }

    /// The paper's look is its colour (its size is in its rectangle).
    fn look(&self, _tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        Some(Look::new().u64(u64::from(self.background.0)).finish())
    }

    /// One entry for the paper and one for each row, so that typing redraws a row, not the
    /// window. A row's number is made so that it is never equal to a state's number (those
    /// count up from 1 and never reach 2^47): the top bit, this state's number and the row.
    fn looks(
        &self,
        tree: &Tree,
        node: &Node,
        clip: Rect,
        cx: &DrawCx<'_>,
        out: &mut Vec<(u64, Rect, u64)>,
    ) {
        let visible = node.rect.intersect(&clip);
        if let Some(look) = self.look(tree, node, cx) {
            out.push((tree.id, visible, look));
        }
        let s = tree.state::<TermState>();
        let line = line_of(cx.theme);
        let (_, rows) = shown(self.cells, node.rect, cx.theme.font.advance(), line);
        let keyboard = cx.has_keyboard(tree);
        for row in 0..rows {
            let r = row_rect(node.rect, row, line).intersect(&visible);
            if r.is_empty() {
                continue;
            }
            let number = (1u64 << 63) | (tree.id << 16) | (row as u64 & 0xffff);
            out.push((number, r, self.row_look(s, node, row, cx, keyboard)));
        }
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        // The core sends `Event::Resized` once the window is laid out at a new size; checking on
        // every event as well costs a comparison, and also covers a layout that changed the
        // view's size without the window's (a neighbour that grew).
        self.check_size(tree, node, shell);
        match event {
            Event::Pointer(p) => self.pointer(tree, node, p, shell),
            Event::Key(k) if shell.is_focused(tree.id) => self.key(tree, k, shell),
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        true
    }

    fn cursor(&self, _tree: &Tree, _node: &Node, _at: Point) -> Cursor {
        Cursor::Text
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        let value = match self.cells.cursor() {
            Some((_, row)) if row < self.cells.size().1 => {
                row_text(self.cells, row, 0, self.cells.size().0)
            }
            _ => String::new(),
        };
        Some(Describe { role: "terminal", value, enabled: true, ..Describe::default() })
    }
}

/// Where row `row` is in a view at `rect`: the whole width, one line tall.
fn row_rect(rect: Rect, row: usize, line: i32) -> Rect {
    let y = i64::from(rect.y) + i64::from(PAD_Y) + (row as i64) * i64::from(line);
    let y = y.min(i64::from(i32::MAX) / 2) as i32;
    Rect::new(rect.x, y, rect.width, line).intersect(&rect)
}

// ---------------------------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------------------------

/// A rectangle the app draws on itself, with raw pointer and key events: Paint's picture,
/// Snake's board.
///
/// Mind the names: inside sandstone `Canvas` is this widget; what it draws on, which the
/// drawing function is given, is `pbr_draw::Canvas`.
///
/// **The canvas is drawn again only when its version changes** (or its size does, or
/// something forces the whole window to be drawn again). Sandstone can't see what the app's
/// function would draw, so the app says so by changing [`Canvas::version`] whenever its picture
/// changes: any number that differs from the last view's will do (a counter it adds 1 to).
pub struct Canvas<'a, M> {
    width: i32,
    height: i32,
    draw: Box<DrawFn<'a>>,
    version: u64,
    on_pointer: Option<Box<dyn Fn(Pointer) -> Option<M> + 'a>>,
    on_key: Option<Box<CanvasKeyFn<'a, M>>>,
    focusable: Option<bool>,
    cursor: Cursor,
    w: Length,
    h: Length,
}

/// The app's drawing function (a *type alias* gives a long type a short name).
type DrawFn<'a> = dyn Fn(&mut DrawCanvas<'_>, Rect) + 'a;
/// The app's key function: the key, and whether it went down (`true`) or up.
type CanvasKeyFn<'a, M> = dyn Fn(KeyPress, bool) -> Option<M> + 'a;

/// What a canvas remembers between views.
#[derive(Default)]
struct CanvasState {
    /// A button went down on it and hasn't all come up: moves and releases are its even
    /// outside it.
    held: bool,
}

impl<'a, M> Canvas<'a, M> {
    /// A canvas `width` by `height` logical pixels. `draw` is given the window's canvas
    /// (clipped to the widget, positions the window's) and the widget's rectangle.
    pub fn new(
        width: i32,
        height: i32,
        draw: impl Fn(&mut DrawCanvas<'_>, Rect) + 'a,
    ) -> Canvas<'a, M> {
        Canvas {
            width,
            height,
            draw: Box::new(draw),
            version: 0,
            on_pointer: None,
            on_key: None,
            focusable: None,
            cursor: Cursor::Arrow,
            w: Length::Fixed(width),
            h: Length::Fixed(height),
        }
    }

    /// The app changes this whenever its picture changes: it is the whole of the look.
    pub fn version(mut self, version: u64) -> Canvas<'a, M> {
        self.version = version;
        self
    }

    /// What the pointer sends: presses, moves, releases and the wheel, with positions
    /// relative to the widget's corner. Moves and releases still come while a button pressed
    /// on it is held, even outside it. `None`: not used.
    pub fn on_pointer(mut self, f: impl Fn(Pointer) -> Option<M> + 'a) -> Canvas<'a, M> {
        self.on_pointer = Some(Box::new(f));
        self
    }

    /// What keys send while it has the focus: presses (`true`, including repeats) and
    /// releases (`false`). `None`: not used (so Tab still moves the focus).
    pub fn on_key(mut self, f: impl Fn(KeyPress, bool) -> Option<M> + 'a) -> Canvas<'a, M> {
        self.on_key = Some(Box::new(f));
        self
    }

    /// Whether it can take the focus (a press on it does, and Tab stops at it). By default
    /// it can if it has `on_key`.
    pub fn focusable(mut self, yes: bool) -> Canvas<'a, M> {
        self.focusable = Some(yes);
        self
    }

    /// The pointer's shape over it (Paint's is `Cursor::Crosshair`).
    pub fn cursor(mut self, cursor: Cursor) -> Canvas<'a, M> {
        self.cursor = cursor;
        self
    }

    /// How wide it wants to be (exactly the width it was made with by default).
    pub fn width(mut self, w: Length) -> Canvas<'a, M> {
        self.w = w;
        self
    }

    /// How tall it wants to be (exactly the height it was made with by default).
    pub fn height(mut self, h: Length) -> Canvas<'a, M> {
        self.h = h;
        self
    }

    fn can_focus(&self) -> bool {
        self.focusable.unwrap_or(self.on_key.is_some())
    }
}

impl<'a, M: 'a> From<Canvas<'a, M>> for Element<'a, M> {
    fn from(w: Canvas<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: 'a> Widget<'a, M> for Canvas<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.w, self.h)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<CanvasState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(CanvasState::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.w).height(self.h);
        Node::new(limits.resolve(Size::new(self.width.max(0), self.height.max(0))))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut DrawCanvas<'_>, _cx: &DrawCx<'_>) {
        let mut c = canvas.clipped(node.rect);
        (self.draw)(&mut c, node.rect);
    }

    /// The version and the size, and nothing else.
    fn look(&self, _tree: &Tree, node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        Some(Look::new().u64(self.version).i32(node.rect.width).i32(node.rect.height).finish())
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let rect = node.rect;
        match event {
            Event::Pointer(p) => {
                let s = tree.state_mut::<CanvasState>();
                let inside = rect.contains(p.at.x, p.at.y);
                let wanted = match p.kind {
                    PointerKind::Pressed { .. } => inside,
                    PointerKind::Moved | PointerKind::Released { .. } => inside || s.held,
                    PointerKind::Scrolled { .. } => inside,
                    PointerKind::Left => false,
                };
                if !wanted {
                    return Status::Ignored;
                }
                let pressed = matches!(p.kind, PointerKind::Pressed { .. });
                if pressed {
                    s.held = true;
                    if self.can_focus() {
                        shell.focus(id);
                    }
                }
                if matches!(p.kind, PointerKind::Released { .. }) && p.buttons == 0 {
                    s.held = false;
                }
                let local = Pointer { at: Point::new(p.at.x - rect.x, p.at.y - rect.y), ..*p };
                let sent = self.on_pointer.as_ref().and_then(|f| f(local));
                let used = sent.is_some();
                if let Some(message) = sent {
                    shell.publish(message);
                }
                if used || pressed { Status::Captured } else { Status::Ignored }
            }
            Event::Key(k) if shell.is_focused(id) => self.key(k, true, shell),
            Event::KeyReleased(k) if shell.is_focused(id) => self.key(k, false, shell),
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        self.can_focus()
    }

    fn cursor(&self, _tree: &Tree, _node: &Node, _at: Point) -> Cursor {
        self.cursor
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "canvas",
            value: self.version.to_string(),
            enabled: true,
            ..Describe::default()
        })
    }
}

impl<'a, M> Canvas<'a, M> {
    /// A key press or release for the focused canvas.
    fn key(&self, k: &KeyPress, down: bool, shell: &mut Shell<'_, M>) -> Status {
        match self.on_key.as_ref().and_then(|f| f(*k, down)) {
            Some(message) => {
                shell.publish(message);
                Status::Captured
            }
            None => Status::Ignored,
        }
    }
}
