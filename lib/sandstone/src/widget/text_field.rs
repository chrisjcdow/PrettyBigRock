// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`TextField`]: one line of text to edit (note 0023, section 5; note 0025, section 2: "text
//! fields everywhere: the same keys as Notepad"), in the look of 1.3's dialog field
//! (`classic/kernel/dialog.c`).
//!
//! The app owns the text. The field is given it in every view (`TextField::new(&text)`) and
//! tells the app each change through `on_input`, so the app's copy is always the truth; the
//! field itself only remembers what the app can't: where the cursor is, where the selection
//! started, how far the text is scrolled across and what can be undone.
//!
//! Positions are counted in *characters*, not bytes. A Rust `String` holds UTF-8, in which
//! `é` takes two bytes and `€` three, and slicing a `str` in the middle of one panics. So the
//! text is turned into a `Vec<char>` (one entry per character) before it is cut up, and put
//! back together afterwards.

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
use pbr_draw::Canvas;

/// How far in from the edge the text starts.
const PAD: i32 = 5;
/// The narrowest a field is.
const MIN_WIDTH: i32 = 40;
/// How many characters of margin are kept between the cursor and the field's ends when the
/// text scrolls.
const MARGIN_CHARS: i64 = 3;
/// The cursor's size: a bar 2 wide and 12 high.
const BAR_WIDTH: i32 = 2;
const BAR_HEIGHT: i32 = 12;
/// How many steps of undo are kept.
const MAX_UNDO: usize = 100;
/// The corner radius of the field (1.3's dialog field was squarer than its buttons).
const RADIUS: i32 = 2;

/// One line of text to edit. See the module's description for who owns the text.
///
/// `'a` is a lifetime: the field borrows the app's text (and `on_input` may borrow from the
/// app too), and Rust checks that the field doesn't outlive what it borrows. `M` is the app's
/// message type.
pub struct TextField<'a, M> {
    value: &'a str,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_submit: Option<M>,
    placeholder: &'a str,
    select_all: bool,
    width: Length,
}

/// A step of undo or redo: the text as it was, and where the cursor and the other end of the
/// selection were.
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

/// What a field remembers between views.
struct State {
    /// Where the cursor is: a character index from 0 (before the first character) to the
    /// number of characters (after the last).
    cursor: usize,
    /// The other end of the selection. Equal to `cursor` when nothing is selected.
    anchor: usize,
    /// How many pixels the text is scrolled to the left (never negative).
    scroll: i32,
    /// The left button went down in the field and hasn't come up: moves now select.
    dragging: bool,
    /// The last change was typing, and the cursor hasn't moved since: the next typed character
    /// joins the same undo step.
    typing: bool,
    /// The text as the field last knew it. If the app's text differs, the app changed it
    /// itself.
    last: String,
    /// How many characters `last` has.
    len: usize,
    /// What Ctrl+Z goes back to, the newest last.
    undo: Vec<Snapshot>,
    /// What Ctrl+Y goes forward to, the newest last.
    redo: Vec<Snapshot>,
}

impl State {
    /// Brings what the field remembers in line with the app's text, which the app may have
    /// changed to anything: the cursor and anchor are kept inside it.
    fn sync(&mut self, value: &str, len: usize) {
        if self.last != value {
            self.last = value.to_string();
            self.typing = false;
        }
        self.len = len;
        self.cursor = self.cursor.min(len);
        self.anchor = self.anchor.min(len);
    }

    /// The selected range of characters, start first (empty when nothing is selected).
    fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    /// Saves a step for Ctrl+Z, forgetting the oldest if there are too many, and forgets
    /// what could have been redone.
    fn push_undo(&mut self) {
        let snapshot =
            Snapshot { text: self.last.clone(), cursor: self.cursor, anchor: self.anchor };
        push_capped(&mut self.undo, snapshot);
        self.redo.clear();
    }
}

/// Adds to a history, dropping the oldest step beyond [`MAX_UNDO`].
fn push_capped(list: &mut Vec<Snapshot>, snapshot: Snapshot) {
    if list.len() >= MAX_UNDO {
        list.remove(0);
    }
    list.push(snapshot);
}

impl<'a, M> TextField<'a, M> {
    /// A field showing `value`. Without [`TextField::on_input`] it is read-only: the text can
    /// be selected and copied but not changed.
    pub fn new(value: &'a str) -> TextField<'a, M> {
        TextField {
            value,
            on_input: None,
            on_submit: None,
            placeholder: "",
            select_all: false,
            width: Length::Fill,
        }
    }

    /// What it sends after every change: the new text. The app is expected to keep it and show
    /// it in the next view.
    pub fn on_input(mut self, f: impl Fn(String) -> M + 'a) -> TextField<'a, M> {
        self.on_input = Some(Box::new(f));
        self
    }

    /// What Enter sends. Without it Enter isn't used by the field, so the window's default
    /// button gets it.
    pub fn on_submit(mut self, message: M) -> TextField<'a, M> {
        self.on_submit = Some(message);
        self
    }

    /// Text shown dim while the field is empty.
    pub fn placeholder(mut self, text: &'a str) -> TextField<'a, M> {
        self.placeholder = text;
        self
    }

    /// Selects the whole text when the field is first shown, so that typing replaces it (the
    /// file name of a rename dialog).
    pub fn select_all(mut self) -> TextField<'a, M> {
        self.select_all = true;
        self
    }

    /// How wide it wants to be: all the room there is by default, and never under 40.
    pub fn width(mut self, w: Length) -> TextField<'a, M> {
        self.width = w;
        self
    }
}

impl<'a, M: Clone + 'a> From<TextField<'a, M>> for Element<'a, M> {
    fn from(w: TextField<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

// ---- Arithmetic on positions. ----
//
// Pixel positions are worked out in 64 bits and brought back to 32 at the end, so that a text
// of any length can't make a sum wrap round (in a release build Rust wraps silently, in a
// debug build it panics; neither is allowed here).

/// A count as an `i64`.
fn wide(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// An `i64` brought within what an `i32` holds.
fn narrow(n: i64) -> i32 {
    n.clamp(i64::from(i32::MIN) / 2, i64::from(i32::MAX) / 2) as i32
}

/// The width of the room the text has: the field less its padding on both sides.
fn room(width: i32) -> i64 {
    i64::from(width.saturating_sub(2 * PAD)).max(0)
}

/// How far the text should be scrolled so that the cursor is in view with a margin of a few
/// characters, starting from how far it is now. Never more than lets the end of the text show
/// at the right, never negative. Used by drawing as well as by events, so that both always
/// agree and drawing never depends on an event having been handled first.
fn view_scroll(scroll: i32, cursor: usize, len: usize, width: i32, advance: i32) -> i32 {
    let room = room(width);
    let advance = i64::from(advance.max(1));
    let at = |i: usize| wide(i).saturating_mul(advance);
    // A margin the room can hold on both sides, however narrow the field is.
    let margin = (MARGIN_CHARS * advance).min((room - 2).max(0) / 2);
    let cursor_at = at(cursor.min(len));
    let mut s = i64::from(scroll);
    if cursor_at - s < margin {
        s = cursor_at - margin;
    } else if cursor_at - s > room - margin {
        s = cursor_at - (room - margin);
    }
    // The cursor bar is a pixel into the text's end, so one more is shown.
    let most = at(len).saturating_add(1).saturating_sub(room).max(0);
    narrow(s.clamp(0, most))
}

/// The character boundary nearest to the pixel column `x`, in a field at `rect` scrolled by
/// `scroll`.
fn index_at(x: i32, rect: Rect, scroll: i32, len: usize, advance: i32) -> usize {
    let advance = i64::from(advance.max(1));
    let from_text = i64::from(x) - i64::from(rect.x) - i64::from(PAD) + i64::from(scroll);
    let boundary = (from_text + advance / 2).div_euclid(advance);
    boundary.clamp(0, wide(len)) as usize
}

/// Where character boundary `i` is, across, in a field at `rect` scrolled by `scroll`.
fn x_of(i: usize, rect: Rect, scroll: i32, advance: i32) -> i32 {
    let at = wide(i).saturating_mul(i64::from(advance));
    narrow(i64::from(rect.x) + i64::from(PAD) + at - i64::from(scroll))
}

// ---- Words. ----

/// What kind of character this is, for finding words: 0 for spaces, 1 for letters, digits and
/// `_`, 2 for everything else (punctuation runs count as words too).
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Where Ctrl+Left goes from `from`: back over spaces, then over the word before them.
fn word_left(chars: &[char], from: usize) -> usize {
    let mut i = from.min(chars.len());
    while i > 0 && class(chars[i - 1]) == 0 {
        i -= 1;
    }
    if i > 0 {
        let kind = class(chars[i - 1]);
        while i > 0 && class(chars[i - 1]) == kind {
            i -= 1;
        }
    }
    i
}

/// Where Ctrl+Right goes from `from`: over the rest of the word, then the spaces after it,
/// to the start of the next word.
fn word_right(chars: &[char], from: usize) -> usize {
    let mut i = from.min(chars.len());
    if i < chars.len() && class(chars[i]) != 0 {
        let kind = class(chars[i]);
        while i < chars.len() && class(chars[i]) == kind {
            i += 1;
        }
    }
    while i < chars.len() && class(chars[i]) == 0 {
        i += 1;
    }
    i
}

/// The word (or run of spaces) at boundary `at`, as a range: what a double click selects.
/// The character after the boundary is the one meant, or the last one when it is the end.
fn word_at(chars: &[char], at: usize) -> (usize, usize) {
    if chars.is_empty() {
        return (0, 0);
    }
    let i = at.min(chars.len() - 1);
    let kind = class(chars[i]);
    let mut start = i;
    while start > 0 && class(chars[start - 1]) == kind {
        start -= 1;
    }
    let mut end = i + 1;
    while end < chars.len() && class(chars[end]) == kind {
        end += 1;
    }
    (start, end)
}

/// Pasted text made fit for one line: line breaks and tabs become spaces (a "\r\n" is one
/// break) and other control characters are dropped.
fn one_line(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            '\n' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

impl<'a, M: Clone + 'a> TextField<'a, M> {
    /// Replaces characters `a` to `b` of the text with `insert`, puts the cursor after it, and
    /// tells the app. `typing`: the change is a typed character, which joins the step before
    /// it if that was typing too and the cursor hasn't moved since.
    fn change(
        &self,
        s: &mut State,
        chars: &[char],
        (a, b): (usize, usize),
        insert: &[char],
        typing: bool,
        shell: &mut Shell<'_, M>,
    ) {
        let Some(on_input) = &self.on_input else {
            return;
        };
        if !(typing && s.typing && a == b) {
            s.push_undo();
        }
        let mut new = String::with_capacity(chars.len() + insert.len());
        new.extend(&chars[..a]);
        new.extend(insert);
        new.extend(&chars[b..]);
        s.len = chars.len() - (b - a) + insert.len();
        s.cursor = a + insert.len();
        s.anchor = s.cursor;
        s.last = new.clone();
        s.typing = typing;
        shell.publish(on_input(new));
    }

    /// Ctrl+Z (`redo` false) or Ctrl+Y: goes to the step before or after, with the cursor as
    /// it was then.
    fn step(&self, s: &mut State, redo: bool, shell: &mut Shell<'_, M>) {
        let Some(on_input) = &self.on_input else {
            return;
        };
        let (from, to) = if redo { (&mut s.redo, &mut s.undo) } else { (&mut s.undo, &mut s.redo) };
        let Some(entry) = from.pop() else {
            return;
        };
        let now = Snapshot { text: s.last.clone(), cursor: s.cursor, anchor: s.anchor };
        push_capped(to, now);
        s.cursor = entry.cursor;
        s.anchor = entry.anchor;
        s.len = entry.text.chars().count();
        s.last = entry.text.clone();
        s.typing = false;
        shell.publish(on_input(entry.text));
    }

    /// Handles a key press for the focused field.
    fn key(&self, tree: &mut Tree, node: &Node, k: &KeyPress, shell: &mut Shell<'_, M>) -> Status {
        let m = k.modifiers;
        if m.alt() || m.meta() {
            return Status::Ignored;
        }
        let (shift, ctrl) = (m.shift(), m.ctrl());
        let editable = self.on_input.is_some();
        let chars: Vec<char> = self.value.chars().collect();
        let n = chars.len();
        let s = tree.state_mut::<State>();
        s.sync(self.value, n);
        let (a, b) = s.selection();
        let selected = a != b;

        // Where the cursor goes for a key that moves it; `shift` keeps the anchor in place, so
        // that the selection grows.
        let moved = match k.key {
            Key::Left if !shift && selected => Some(a),
            Key::Right if !shift && selected => Some(b),
            Key::Left if ctrl => Some(word_left(&chars, s.cursor)),
            Key::Right if ctrl => Some(word_right(&chars, s.cursor)),
            Key::Left => Some(s.cursor.saturating_sub(1)),
            Key::Right => Some((s.cursor + 1).min(n)),
            Key::Home => Some(0),
            Key::End => Some(n),
            _ => None,
        };
        let status = if let Some(to) = moved {
            s.cursor = to;
            if !shift {
                s.anchor = to;
            }
            s.typing = false;
            Status::Captured
        } else {
            match k.key {
                Key::Backspace | Key::Delete if !editable => Status::Ignored,
                Key::Backspace | Key::Delete => {
                    let range = if selected {
                        (a, b)
                    } else if k.key == Key::Backspace {
                        let from = if ctrl {
                            word_left(&chars, s.cursor)
                        } else {
                            s.cursor.saturating_sub(1)
                        };
                        (from, s.cursor)
                    } else {
                        let to =
                            if ctrl { word_right(&chars, s.cursor) } else { (s.cursor + 1).min(n) };
                        (s.cursor, to)
                    };
                    if range.0 != range.1 {
                        self.change(s, &chars, range, &[], false, shell);
                    }
                    Status::Captured
                }
                Key::Enter => match &self.on_submit {
                    Some(message) => {
                        shell.publish(message.clone());
                        Status::Captured
                    }
                    None => Status::Ignored,
                },
                // These are for the window: Tab moves the focus, Escape closes a dialog.
                Key::Tab
                | Key::Escape
                | Key::Up
                | Key::Down
                | Key::PageUp
                | Key::PageDown
                | Key::Insert
                | Key::Function(_)
                | Key::Modifier => Status::Ignored,
                _ if ctrl => self.shortcut(s, &chars, k, shell),
                _ if k.typed => match k.ch {
                    Some(c) if editable && !c.is_control() => {
                        self.change(s, &chars, (a, b), &[c], true, shell);
                        Status::Captured
                    }
                    _ => Status::Ignored,
                },
                _ => Status::Ignored,
            }
        };
        if status == Status::Captured {
            s.scroll = view_scroll(
                s.scroll,
                s.cursor,
                s.len,
                node.rect.width,
                shell.theme().font.advance(),
            );
            shell.redraw();
        }
        status
    }

    /// The Ctrl shortcuts (select all, copy, cut, paste, undo, redo), by the character on the
    /// key. Others are left for the app's own shortcuts.
    fn shortcut(
        &self,
        s: &mut State,
        chars: &[char],
        k: &KeyPress,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let editable = self.on_input.is_some();
        let (a, b) = s.selection();
        let Some(c) = k.ch.and_then(|c| c.to_lowercase().next()) else {
            return Status::Ignored;
        };
        match c {
            'a' if !k.modifiers.shift() => {
                s.anchor = 0;
                s.cursor = chars.len();
                s.typing = false;
                Status::Captured
            }
            'c' if !k.modifiers.shift() => {
                if a != b {
                    let text: String = chars[a..b].iter().collect();
                    shell.copy_text(&text);
                }
                Status::Captured
            }
            'x' if editable && !k.modifiers.shift() => {
                if a != b {
                    let text: String = chars[a..b].iter().collect();
                    // Only cut what the clipboard really took, or the text would be lost.
                    if shell.copy_text(&text) {
                        self.change(s, chars, (a, b), &[], false, shell);
                    }
                }
                Status::Captured
            }
            'v' if editable && !k.modifiers.shift() => {
                if let Some(text) = shell.paste_text() {
                    let pasted = one_line(&text);
                    if !pasted.is_empty() || a != b {
                        self.change(s, chars, (a, b), &pasted, false, shell);
                    }
                }
                Status::Captured
            }
            'z' if editable && !k.modifiers.shift() => {
                self.step(s, false, shell);
                Status::Captured
            }
            'y' if editable && !k.modifiers.shift() => {
                self.step(s, true, shell);
                Status::Captured
            }
            'z' if editable => {
                self.step(s, true, shell);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }

    /// Handles the pointer: a press places the cursor and takes the focus, moves while the
    /// button is down select, and letting go ends it.
    fn pointer(
        &self,
        tree: &mut Tree,
        node: &Node,
        p: &Pointer,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let chars: Vec<char> = self.value.chars().collect();
        let n = chars.len();
        let s = tree.state_mut::<State>();
        s.sync(self.value, n);
        let advance = shell.theme().font.advance();
        let rect = node.rect;
        match p.kind {
            PointerKind::Pressed { button: MouseButton::Left, clicks }
                if rect.contains(p.at.x, p.at.y) =>
            {
                shell.focus(id);
                let at = index_at(p.at.x, rect, s.scroll, n, advance);
                match clicks {
                    0 | 1 => {
                        s.cursor = at;
                        if !p.modifiers.shift() {
                            s.anchor = at;
                        }
                    }
                    2 => {
                        (s.anchor, s.cursor) = word_at(&chars, at);
                    }
                    _ => {
                        s.anchor = 0;
                        s.cursor = n;
                    }
                }
                s.dragging = true;
                s.typing = false;
            }
            // The pointer moves come even outside the field, so a drag can go past its ends.
            PointerKind::Moved if s.dragging => {
                s.cursor = index_at(p.at.x, rect, s.scroll, n, advance);
            }
            PointerKind::Released { button: MouseButton::Left } if s.dragging => {
                s.dragging = false;
                return Status::Captured;
            }
            _ => return Status::Ignored,
        }
        s.scroll = view_scroll(s.scroll, s.cursor, n, rect.width, advance);
        shell.redraw();
        Status::Captured
    }
}

impl<'a, M: Clone + 'a> Widget<'a, M> for TextField<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, Length::Shrink)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<State>()
    }

    fn state(&self) -> Box<dyn Any> {
        let len = self.value.chars().count();
        Box::new(State {
            cursor: len,
            anchor: if self.select_all { 0 } else { len },
            scroll: 0,
            dragging: false,
            typing: false,
            last: self.value.to_string(),
            len,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let width = match self.width {
            Length::Fixed(w) => Length::Fixed(w.max(MIN_WIDTH)),
            other => other,
        };
        let limits = limits.width(width);
        Node::new(limits.resolve(Size::new(MIN_WIDTH, cx.theme.control_height)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<State>();
        let t = cx.theme;
        let r = node.rect;
        let keyboard = cx.has_keyboard(tree);
        let len = self.value.chars().count();
        let (cursor, anchor) = (s.cursor.min(len), s.anchor.min(len));
        let advance = t.font.advance();
        let scroll = view_scroll(s.scroll, cursor, len, r.width, advance);

        let paper = if self.on_input.is_some() { t.field } else { t.gutter };
        let edge = if keyboard { t.accent } else { t.field_edge };
        canvas.fill_round_rect(r, RADIUS, edge);
        canvas.fill_round_rect(r.inset(1), (RADIUS - 1).max(0), paper);

        // Everything else stays clear of the edge, and the text is cut off cleanly where it
        // scrolls out of the field.
        let inner = Rect::new(
            r.x.saturating_add(PAD - 1),
            r.y.saturating_add(1),
            r.width.saturating_sub(2 * (PAD - 1)).max(0),
            r.height.saturating_sub(2).max(0),
        );
        let mut c = canvas.clipped(inner);
        let bar_y = r.y + (r.height - BAR_HEIGHT) / 2;

        if anchor != cursor {
            let (a, b) = (cursor.min(anchor), cursor.max(anchor));
            let (x0, x1) = (x_of(a, r, scroll, advance), x_of(b, r, scroll, advance));
            let colour = if keyboard { t.selection } else { t.selection_inactive };
            let sel = Rect::new(x0, bar_y - 2, (x1 - x0).max(0), BAR_HEIGHT + 4);
            c.fill_rect(sel, colour);
        }
        let text_x = x_of(0, r, scroll, advance);
        let text_y = super::button::text_top(r, t.font.size);
        if self.value.is_empty() {
            let shown = super::label::fit(self.placeholder, t.font, r.width - 2 * PAD);
            c.text(text_x, text_y, &shown, t.text_dim, t.font);
        } else {
            c.text(text_x, text_y, self.value, t.text, t.font);
        }
        if keyboard {
            let x = x_of(cursor, r, scroll, advance) - BAR_WIDTH / 2;
            c.fill_rect(Rect::new(x, bar_y, BAR_WIDTH, BAR_HEIGHT), t.accent);
        }
    }

    fn look(&self, tree: &Tree, node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<State>();
        let len = self.value.chars().count();
        let (cursor, anchor) = (s.cursor.min(len), s.anchor.min(len));
        let scroll = view_scroll(s.scroll, cursor, len, node.rect.width, cx.theme.font.advance());
        Some(
            Look::new()
                .str(self.value)
                .str(self.placeholder)
                .u64(wide(cursor) as u64)
                .u64(wide(anchor) as u64)
                .i32(scroll)
                .bool(self.on_input.is_some())
                .bool(cx.has_keyboard(tree))
                .finish(),
        )
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        match event {
            Event::Pointer(p) => self.pointer(tree, node, p, shell),
            Event::Key(k) if shell.is_focused(tree.id) => self.key(tree, node, k, shell),
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        true
    }

    fn cursor(&self, _tree: &Tree, _node: &Node, _at: Point) -> pbr_window::Cursor {
        pbr_window::Cursor::Text
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "text field",
            label: self.placeholder.to_string(),
            value: self.value.to_string(),
            enabled: true,
            ..Describe::default()
        })
    }
}
