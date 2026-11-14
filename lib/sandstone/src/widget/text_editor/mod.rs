// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`TextEditor`]: many lines of text with a cursor, a selection, the clipboard, undo, and scroll
//! bars; Notepad's editor (note 0023, section 5; note 0026, section 1). The text itself is a
//! [`Content`], owned by the app (see `content.rs`).
//!
//! The editor **borrows** the app's [`Content`] while a view is built and never changes it. What
//! the user does is sent as an [`Action`] through [`TextEditor::on_action`]; the app applies it
//! with [`Content::perform`] in `update`, so the app sees every change. A tiny app:
//!
//! ```ignore
//! struct Notes { text: Content }
//! enum Msg { Edit(Action) }
//! impl App for Notes {
//!     type Message = Msg;
//!     fn view(&self) -> Element<'_, Msg> {
//!         TextEditor::new(&self.text).on_action(Msg::Edit).into()
//!     }
//!     fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
//!         let Msg::Edit(action) = m;
//!         self.text.perform(action);
//!     }
//! }
//! ```
//!
//! **The look** is 1.3's Notepad (`classic/kernel/notepad.c`, `np_draw`): white paper; a margin
//! of line numbers; characters 6 pixels wide on lines 12 apart; a pink bar for the cursor,
//! drawn only while the editor has the keyboard (it does not blink); the selection behind the
//! characters. **Scrolling**: the wheel, the bars, and, whenever the cursor has moved, the
//! least scrolling that shows it again. Only the lines in view are drawn and looked at, so a
//! text of a hundred thousand lines costs what a short one does.

mod content;
mod view;

pub use content::{Action, Content, Motion, Pos, TAB_WIDTH};

use crate::Look;
use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::event::{Button as MouseButton, Event, Key, KeyPress, PointerKind, Status};
use crate::geometry::{Length, Limits, Point, Rect};
use crate::layout::Node;
use crate::shell::Shell;
use crate::widget::scrollbar::{self, WHEEL_STEP};
use alloc::boxed::Box;
use alloc::format;
use core::any::{Any, TypeId};
use pbr_draw::Canvas;
use pbr_interfaces::pbr_input as input;
use pbr_window::Cursor;
use view::{BREAK_W, DEFAULT_SIZE, Geo, LINE_H, PAD, visible_text};

/// A many-line text editor showing a [`Content`].
///
/// It keeps its own small state between views (how far it is scrolled, a drag in progress).
/// Everything the user does to the text comes back as an [`Action`] through
/// [`TextEditor::on_action`]:
/// - **without `on_action` the editor sends nothing**, so it only shows the text: it can still
///   be scrolled and take the focus, and Ctrl+C copies what the app has selected; moving the
///   cursor and selecting are actions too, and need the app to apply them;
/// - **with [`TextEditor::read_only`]** only the actions that don't change the text are sent
///   (moving, selecting, copying), and keys that would change it are left for others (Tab
///   then moves the focus on).
pub struct TextEditor<'a, M> {
    content: &'a Content,
    on_action: Option<Box<dyn Fn(Action) -> M + 'a>>,
    line_numbers: bool,
    read_only: bool,
    width: Length,
    height: Length,
}

impl<'a, M> TextEditor<'a, M> {
    /// An editor showing `content`, with line numbers, filling the room it is given.
    pub fn new(content: &'a Content) -> TextEditor<'a, M> {
        TextEditor {
            content,
            on_action: None,
            line_numbers: true,
            read_only: false,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// What it sends when the user does something to the text: the app applies it with
    /// [`Content::perform`].
    pub fn on_action(mut self, f: impl Fn(Action) -> M + 'a) -> TextEditor<'a, M> {
        self.on_action = Some(Box::new(f));
        self
    }

    /// Whether the margin with the line numbers shows (it does by default).
    pub fn line_numbers(mut self, on: bool) -> TextEditor<'a, M> {
        self.line_numbers = on;
        self
    }

    /// Only moving, selecting and copying are sent: the text can't be changed through it.
    pub fn read_only(mut self, on: bool) -> TextEditor<'a, M> {
        self.read_only = on;
        self
    }

    /// How wide it wants to be (all the room by default).
    pub fn width(mut self, w: Length) -> TextEditor<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be (all the room by default).
    pub fn height(mut self, h: Length) -> TextEditor<'a, M> {
        self.height = h;
        self
    }

    /// Sends `action`, unless there is nobody to send it to or it would change a read-only
    /// text.
    fn send(&self, shell: &mut Shell<'_, M>, action: Action) {
        if let Some(f) = &self.on_action
            && !(self.read_only && action.edits())
        {
            shell.publish(f(action));
        }
    }

    fn can_edit(&self) -> bool {
        self.on_action.is_some() && !self.read_only
    }
}

impl<'a, M: 'a> From<TextEditor<'a, M>> for Element<'a, M> {
    fn from(w: TextEditor<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// What a drag that began with a press is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    None,
    /// Selecting text.
    Text,
    /// Moving the vertical thumb, grabbed this far from its top.
    Vertical(i32),
    /// Moving the horizontal thumb, grabbed this far from its left.
    Horizontal(i32),
}

/// What an editor remembers between views.
pub(crate) struct State {
    /// Where things are, as of the last layout.
    geo: Geo,
    /// How far it is scrolled across and down, in pixels.
    scroll: (i32, i32),
    /// The cursor and the size of the room for text the last time the cursor was scrolled
    /// to: when either differs, it is scrolled to again.
    seen: Option<(Pos, i32, i32)>,
    drag: Drag,
    /// The place the pointer last selected up to, during a drag.
    target: Pos,
}

impl State {
    fn new() -> State {
        State { geo: Geo::default(), scroll: (0, 0), seen: None, drag: Drag::None, target: (0, 0) }
    }
}

impl<'a, M: 'a> Widget<'a, M> for TextEditor<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<State>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(State::new())
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let size = limits.resolve(DEFAULT_SIZE);
        let c = self.content;
        let geo =
            Geo::new(size, self.line_numbers, c.line_count(), c.widest(), cx.theme.font.advance());
        let s = tree.state_mut::<State>();
        s.geo = geo;
        s.scroll = geo.clamp(s.scroll);
        // The cursor is scrolled to when it has moved (or the room has changed), and not
        // otherwise, so that the wheel can look elsewhere.
        let now = (c.cursor(), geo.view_w, geo.view_h);
        if s.seen != Some(now) {
            s.seen = Some(now);
            if geo.view_w > 0 && geo.view_h > 0 {
                s.scroll = geo.reveal(c, c.cursor(), s.scroll);
            }
        }
        Node::new(size)
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let (t, c) = (cx.theme, self.content);
        let s = tree.state::<State>();
        let g = s.geo;
        let r = node.rect;
        let origin = Point::new(r.x, r.y);
        let mut canvas = canvas.clipped(r);
        let canvas = &mut canvas;
        canvas.fill_rect(r, t.field);
        let rows = g.rows(s.scroll.1, c.line_count());

        if g.gutter > 0 {
            let margin = Rect::new(r.x, r.y, g.gutter, g.view_h);
            canvas.fill_rect(margin, t.gutter);
            canvas.fill_rect(Rect::new(r.x + g.gutter, r.y, 1, g.view_h), t.gutter_edge);
            // Numbers are right-aligned in a room of at least three digits, as 1.3's `%3d`.
            let room = view::digits(c.line_count()).max(3);
            let mut numbers = canvas.clipped(margin);
            for n in rows.clone() {
                let label = format!("{:>room$}", n + 1);
                let y = r.y + g.line_y(n, s.scroll.1);
                numbers.text(r.x + PAD, y, &label, t.line_number, t.font);
            }
        }

        let area = g.text_rect(origin);
        let mut text = canvas.clipped(area);
        let adv = g.adv;
        // Where the text's first column is, in the window.
        let x0 = area.x + PAD - s.scroll.0;
        let keyboard = cx.has_keyboard(tree);
        let selection = c.selection();
        let (first_cell, cells) = visible_cells(&g, s.scroll.0);
        for n in rows {
            let line = c.line(n);
            let top = r.y + g.line_y(n, s.scroll.1);
            if let Some((a, b)) = selection
                && n >= a.0
                && n <= b.0
            {
                let from = if n == a.0 { a.1 } else { 0 };
                let to = if n == b.0 { b.1 } else { c.line_len(n) };
                let left = x0 + view::times(content::display_column(line, from), adv);
                let mut right = x0 + view::times(content::display_column(line, to), adv);
                if n < b.0 {
                    right += BREAK_W;
                }
                let colour = if keyboard { t.selection } else { t.selection_inactive };
                text.fill_rect(Rect::new(left, top - 1, right - left, LINE_H), colour);
            }
            let shown = visible_text(line, first_cell, cells);
            text.text(x0 + view::times(first_cell, adv), top, &shown, t.text, t.font);
        }
        if keyboard {
            let (l, col) = c.cursor();
            let top = r.y + g.line_y(l, s.scroll.1);
            let x = x0 + view::times(content::display_column(c.line(l), col), adv) - 1;
            text.fill_rect(Rect::new(x, top - 1, 2, LINE_H), t.accent);
        }

        // The scroll bars, and the corners they leave.
        if g.vbar
            && let Some(bar) =
                scrollbar::bar(g.vtrack(origin), true, g.content_h, g.view_h, s.scroll.1)
        {
            bar.draw(canvas, t, matches!(s.drag, Drag::Vertical(_)));
        }
        if g.hbar {
            if g.left > 0 {
                let under = Rect::new(r.x, r.y + g.view_h, g.left, scrollbar::THICKNESS);
                canvas.fill_rect(under, t.scroll_track);
            }
            if let Some(bar) =
                scrollbar::bar(g.htrack(origin), false, g.content_w, g.view_w, s.scroll.0)
            {
                bar.draw(canvas, t, matches!(s.drag, Drag::Horizontal(_)));
            }
        }
        if g.vbar && g.hbar {
            let corner = Rect::new(
                r.x + r.width - scrollbar::THICKNESS,
                r.y + g.view_h,
                scrollbar::THICKNESS,
                scrollbar::THICKNESS,
            );
            canvas.fill_rect(corner, t.scroll_track);
        }
    }

    fn look(&self, tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let c = self.content;
        let s = tree.state::<State>();
        let g = s.geo;
        let mut look = Look::new()
            .bool(self.line_numbers)
            .bool(cx.has_keyboard(tree))
            .i32(s.scroll.0)
            .i32(s.scroll.1)
            .i32(g.content_w)
            .i32(g.content_h)
            .u64(c.line_count() as u64)
            .u64(c.cursor().0 as u64)
            .u64(c.cursor().1 as u64)
            .bool(matches!(s.drag, Drag::Vertical(_)))
            .bool(matches!(s.drag, Drag::Horizontal(_)));
        if let Some((a, b)) = c.selection() {
            look = look.u64(a.0 as u64).u64(a.1 as u64).u64(b.0 as u64).u64(b.1 as u64);
        }
        // What shows of the lines in view, and nothing else of the text.
        let (first_cell, cells) = visible_cells(&g, s.scroll.0);
        for n in g.rows(s.scroll.1, c.line_count()) {
            look = look.u64(n as u64).str(&visible_text(c.line(n), first_cell, cells));
            // Where the line ends decides how far a selected line break reaches.
            look = look.u64(c.line_len(n) as u64);
        }
        Some(look.finish())
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        match event {
            Event::Key(k) if shell.is_focused(id) => self.key(tree.state::<State>(), k, shell),
            Event::Pointer(p) => self.pointer(tree, node, p, shell),
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        true
    }

    fn cursor(&self, tree: &Tree, node: &Node, at: Point) -> Cursor {
        let g = tree.state::<State>().geo;
        let origin = Point::new(node.rect.x, node.rect.y);
        if g.text_rect(origin).contains(at.x, at.y) { Cursor::Text } else { Cursor::Arrow }
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        let c = self.content;
        let (line, col) = c.cursor();
        let mut value = format!("line {}, column {}", line + 1, col + 1);
        if c.selection().is_some() {
            value.push_str(&format!(", {} selected", c.selection_len()));
        }
        Some(Describe {
            role: "text editor",
            label: alloc::string::String::new(),
            value,
            enabled: true,
            checked: None,
        })
    }
}

/// The first column drawn when scrolled `scroll_x`, and how many columns are drawn.
fn visible_cells(g: &Geo, scroll_x: i32) -> (usize, usize) {
    let adv = g.adv.max(1);
    let first = (scroll_x - PAD).max(0) / adv;
    let last = (scroll_x + g.view_w - PAD).max(0) / adv + 1;
    (first as usize, (last - first + 1).max(0) as usize)
}

impl<'a, M: 'a> TextEditor<'a, M> {
    /// A key pressed while the editor has the focus.
    fn key(&self, s: &State, k: &KeyPress, shell: &mut Shell<'_, M>) -> Status {
        let m = k.modifiers;
        if m.alt() || m.meta() {
            return Status::Ignored;
        }
        let (ctrl, select) = (m.ctrl(), m.shift());
        let go = |motion| Action::Move { motion, select };
        let action = match (k.key, ctrl) {
            (Key::Left, false) => go(Motion::Left),
            (Key::Right, false) => go(Motion::Right),
            (Key::Up, false) => go(Motion::Up),
            (Key::Down, false) => go(Motion::Down),
            (Key::Home, false) => go(Motion::LineStart),
            (Key::End, false) => go(Motion::LineEnd),
            (Key::PageUp, false) => go(Motion::PageUp(s.geo.page())),
            (Key::PageDown, false) => go(Motion::PageDown(s.geo.page())),
            (Key::Left, true) => go(Motion::WordLeft),
            (Key::Right, true) => go(Motion::WordRight),
            (Key::Home, true) => go(Motion::TextStart),
            (Key::End, true) => go(Motion::TextEnd),
            (Key::Enter, false) if self.can_edit() => Action::NewLine,
            (Key::Backspace, _) if self.can_edit() => Action::Backspace { word: ctrl },
            (Key::Delete, _) if self.can_edit() => Action::Delete { word: ctrl },
            // Shift+Tab is left for the window: the way out of the editor, since Tab is a tab.
            (Key::Tab, false) if self.can_edit() && !select => Action::InsertChar('\t'),
            (_, true) => return self.shortcut(k, shell),
            _ => match k.ch {
                Some(c) if k.typed && !c.is_control() && self.can_edit() => Action::InsertChar(c),
                _ => return Status::Ignored,
            },
        };
        // Moving needs somebody to send the move to.
        if self.on_action.is_none() {
            return Status::Ignored;
        }
        self.send(shell, action);
        Status::Captured
    }

    /// Ctrl and a letter: the clipboard, select all, undo and redo. Anything else (Ctrl+S) is
    /// not for the editor.
    fn shortcut(&self, k: &KeyPress, shell: &mut Shell<'_, M>) -> Status {
        let Some(letter) = k.ch.and_then(|c| c.to_lowercase().next()) else {
            return Status::Ignored;
        };
        match letter {
            'a' if self.on_action.is_some() => self.send(shell, Action::SelectAll),
            'c' => {
                let text = self.content.selected_text();
                if !text.is_empty() {
                    shell.copy_text(&text);
                }
            }
            'x' if self.can_edit() => {
                let text = self.content.selected_text();
                // Only cut what has been copied: a refused copy must not lose the text.
                if !text.is_empty() && shell.copy_text(&text) {
                    self.send(shell, Action::DeleteSelection);
                }
            }
            'v' if self.can_edit() => {
                if let Some(text) = shell.paste_text()
                    && !text.is_empty()
                {
                    self.send(shell, Action::InsertText(text));
                }
            }
            'z' if self.can_edit() => {
                let redo = k.modifiers.shift();
                self.send(shell, if redo { Action::Redo } else { Action::Undo });
            }
            'y' if self.can_edit() => self.send(shell, Action::Redo),
            _ => return Status::Ignored,
        }
        Status::Captured
    }

    fn pointer(
        &self,
        tree: &mut Tree,
        node: &Node,
        p: &crate::event::Pointer,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let c = self.content;
        let s = tree.state_mut::<State>();
        let g = s.geo;
        let r = node.rect;
        let origin = Point::new(r.x, r.y);
        let inside = r.contains(p.at.x, p.at.y);
        match p.kind {
            PointerKind::Pressed { button: MouseButton::Left, clicks } if inside => {
                // The scroll bars first: they don't move the cursor.
                if let Some(status) = self.press_on_bars(s, origin, p.at, shell) {
                    return status;
                }
                shell.focus(id);
                let at = g.position_at(c, origin, s.scroll, p.at);
                let select = p.modifiers.shift();
                let action = match clicks {
                    0 | 1 => Action::Move { motion: Motion::To(at), select },
                    2 => Action::SelectWord(at),
                    _ => Action::SelectLine(at.0),
                };
                s.drag = Drag::Text;
                s.target = at;
                self.send(shell, action);
                Status::Captured
            }
            PointerKind::Moved => {
                let left_down = p.buttons & (1 << input::BUTTON_LEFT) != 0;
                if !left_down {
                    s.drag = Drag::None;
                    return Status::Ignored;
                }
                match s.drag {
                    Drag::None => Status::Ignored,
                    Drag::Text => {
                        let at = g.position_at(c, origin, s.scroll, p.at);
                        if at != s.target {
                            s.target = at;
                            self.send(shell, Action::Move { motion: Motion::To(at), select: true });
                        }
                        Status::Captured
                    }
                    Drag::Vertical(grab) => {
                        let track = g.vtrack(origin);
                        if let Some(bar) =
                            scrollbar::bar(track, true, g.content_h, g.view_h, s.scroll.1)
                        {
                            let to = bar.offset_at(p.at.y - grab, g.content_h, g.view_h);
                            self.scroll_to(s, (s.scroll.0, to), shell);
                        }
                        Status::Captured
                    }
                    Drag::Horizontal(grab) => {
                        let track = g.htrack(origin);
                        if let Some(bar) =
                            scrollbar::bar(track, false, g.content_w, g.view_w, s.scroll.0)
                        {
                            let to = bar.offset_at(p.at.x - grab, g.content_w, g.view_w);
                            self.scroll_to(s, (to, s.scroll.1), shell);
                        }
                        Status::Captured
                    }
                }
            }
            PointerKind::Released { button: MouseButton::Left } if s.drag != Drag::None => {
                s.drag = Drag::None;
                shell.redraw();
                Status::Captured
            }
            PointerKind::Scrolled { dx, dy } if inside => {
                // Shift turns the ordinary wheel sideways.
                let (dx, dy) = if p.modifiers.shift() && dx == 0 { (dy, 0) } else { (dx, dy) };
                let (mx, my) = g.max_scroll();
                if !((dx != 0 && mx > 0) || (dy != 0 && my > 0)) {
                    return Status::Ignored;
                }
                let to = (
                    s.scroll.0.saturating_add(dx.saturating_mul(WHEEL_STEP)),
                    s.scroll.1.saturating_add(dy.saturating_mul(WHEEL_STEP)),
                );
                self.scroll_to(s, to, shell);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }

    /// A press at `at` on a scroll bar: a thumb is grabbed, the track on either side of it
    /// pages. `None` if the press is not on a bar.
    fn press_on_bars(
        &self,
        s: &mut State,
        origin: Point,
        at: Point,
        shell: &mut Shell<'_, M>,
    ) -> Option<Status> {
        let g = s.geo;
        if g.vbar && g.vtrack(origin).contains(at.x, at.y) {
            let bar = scrollbar::bar(g.vtrack(origin), true, g.content_h, g.view_h, s.scroll.1)?;
            if bar.thumb.contains(at.x, at.y) {
                s.drag = Drag::Vertical(at.y - bar.thumb_start());
                shell.redraw();
            } else {
                let page = (g.view_h - LINE_H).max(LINE_H);
                let down = at.y > bar.thumb.y;
                let to = s.scroll.1.saturating_add(if down { page } else { -page });
                self.scroll_to(s, (s.scroll.0, to), shell);
            }
            return Some(Status::Captured);
        }
        if g.hbar && g.htrack(origin).contains(at.x, at.y) {
            let bar = scrollbar::bar(g.htrack(origin), false, g.content_w, g.view_w, s.scroll.0)?;
            if bar.thumb.contains(at.x, at.y) {
                s.drag = Drag::Horizontal(at.x - bar.thumb_start());
                shell.redraw();
            } else {
                let page = (g.view_w - 2 * g.adv).max(g.adv);
                let right = at.x > bar.thumb.x;
                let to = s.scroll.0.saturating_add(if right { page } else { -page });
                self.scroll_to(s, (to, s.scroll.1), shell);
            }
            return Some(Status::Captured);
        }
        // The corner, and the strip under the margin, belong to the bars too.
        let below = at.y >= origin.y + g.view_h && g.hbar;
        let beside = at.x >= origin.x + g.size.width - scrollbar::THICKNESS && g.vbar;
        (below || beside).then_some(Status::Captured)
    }

    /// Scrolls to `to` (kept within the text), drawing again if that moved it.
    fn scroll_to(&self, s: &mut State, to: (i32, i32), shell: &mut Shell<'_, M>) {
        let to = s.geo.clamp(to);
        if to != s.scroll {
            s.scroll = to;
            shell.redraw();
        }
    }
}
