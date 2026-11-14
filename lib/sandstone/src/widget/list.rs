// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Scroll`] (scroll bars around anything bigger than its room) and [`List`] (rows with an icon
//! and columns: Files, the file picker, the permissions viewer), note 0023 section 5.
//!
//! Both scroll with the core's scroll bar ([`super::scrollbar`]): an *offset* is how far the
//! content has moved up (or left) in pixels, 0 showing its start. Both keep their offsets in a
//! small state value that sandstone holds between views, so a view built afresh after every
//! change still shows the same part of the content.

use super::scrollbar::{self, Bar, THICKNESS, WHEEL_STEP};
use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget, draw_children};
use crate::event::{Button as MouseButton, Event, Key, Modifiers, Pointer, PointerKind, Status};
use crate::geometry::{Length, Limits, Point, Rect, Size, UNBOUNDED};
use crate::layout::Node;
use crate::shell::Shell;
use crate::{Image, Look, Theme};
use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::any::{Any, TypeId};
use pbr_draw::Canvas;
use pbr_interfaces::pbr_input as input;
use pbr_window::Cursor;

/// The bit of [`Pointer::buttons`] that says the left button is held.
const LEFT_HELD: u32 = 1 << input::BUTTON_LEFT;

// ---------------------------------------------------------------------------------------------
// Scroll
// ---------------------------------------------------------------------------------------------

/// Scroll bars around one child that may be bigger than the room it is given.
///
/// The child is laid out with no limit along the scrolled directions (so a column of a hundred
/// buttons is as tall as it needs) and with the visible part's size across the others. A bar
/// appears only along a direction where the child is longer than the room. The wheel, the thumb
/// and clicks on the track scroll; Page Up, Page Down, Home and End scroll too when the focus
/// is inside and nothing in the child uses them.
pub struct Scroll<'a, M> {
    /// The one child. Kept in an array of one so that [`Widget::children`] can give it as a
    /// slice, the way [`Panel`](super::Panel) does.
    child: [Element<'a, M>; 1],
    vertical: bool,
    horizontal: bool,
    width: Length,
    height: Length,
}

impl<'a, M: 'a> Scroll<'a, M> {
    /// A scroll area around `child`, scrolling up and down.
    pub fn new(child: impl Into<Element<'a, M>>) -> Scroll<'a, M> {
        Scroll {
            child: [child.into()],
            vertical: true,
            horizontal: false,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Scrolls up and down only (the default).
    pub fn vertical(mut self) -> Scroll<'a, M> {
        self.vertical = true;
        self.horizontal = false;
        self
    }

    /// Scrolls left and right only.
    pub fn horizontal(mut self) -> Scroll<'a, M> {
        self.vertical = false;
        self.horizontal = true;
        self
    }

    /// Scrolls both ways.
    pub fn both(mut self) -> Scroll<'a, M> {
        self.vertical = true;
        self.horizontal = true;
        self
    }

    /// How wide it wants to be (all the room, by default).
    pub fn width(mut self, w: Length) -> Scroll<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be (all the room, by default).
    pub fn height(mut self, h: Length) -> Scroll<'a, M> {
        self.height = h;
        self
    }
}

impl<'a, M: 'a> From<Scroll<'a, M>> for Element<'a, M> {
    fn from(w: Scroll<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// A thumb being dragged: which bar, and how far into the thumb the pointer took hold (so the
/// thumb doesn't jump to put its start under the pointer).
#[derive(Clone, Copy)]
struct Drag {
    vertical: bool,
    grab: i32,
}

/// What a scroll area remembers between views.
#[derive(Default)]
struct ScrollState {
    /// How far the content is scrolled right and down. Brought within range at every layout.
    ox: i32,
    oy: i32,
    /// The child's size, and the size of the part of it that shows, as laid out last.
    content: Size,
    view: Size,
    /// Which bars are shown: (vertical, horizontal).
    bars: (bool, bool),
    /// A thumb is being dragged.
    drag: Option<Drag>,
    /// A button went down inside the child and hasn't come up: the child is then given the
    /// pointer wherever it goes (a button being held, a selection being dragged).
    child_pressed: bool,
    /// The pointer was last inside the visible part (so the child can be told once when it
    /// leaves).
    pointer_inside: bool,
}

/// Where the parts of a scroll area are, from its rectangle and state.
struct ScrollGeo {
    /// The part of the area that shows the child.
    viewport: Rect,
    vbar: Option<Bar>,
    hbar: Option<Bar>,
    /// The square where the two bars meet (empty if there aren't two).
    corner: Rect,
}

fn scroll_geo(rect: Rect, s: &ScrollState) -> ScrollGeo {
    let (vw, vh) = (s.view.width, s.view.height);
    let viewport = Rect::new(rect.x, rect.y, vw, vh).intersect(&rect);
    // Intersecting with the area's own rectangle keeps a bar inside it even when the area is
    // narrower than a bar.
    let vtrack = Rect::new(rect.x + vw, rect.y, THICKNESS, vh).intersect(&rect);
    let htrack = Rect::new(rect.x, rect.y + vh, vw, THICKNESS).intersect(&rect);
    let corner = if s.bars.0 && s.bars.1 {
        Rect::new(rect.x + vw, rect.y + vh, THICKNESS, THICKNESS).intersect(&rect)
    } else {
        Rect::EMPTY
    };
    ScrollGeo {
        viewport,
        vbar: if s.bars.0 {
            scrollbar::bar(vtrack, true, s.content.height, vh, s.oy)
        } else {
            None
        },
        hbar: if s.bars.1 {
            scrollbar::bar(htrack, false, s.content.width, vw, s.ox)
        } else {
            None
        },
        corner,
    }
}

impl ScrollState {
    /// (`ox`, `oy`) brought within range, along the directions that have a bar (0 along the
    /// others).
    fn in_range(&self, ox: i32, oy: i32) -> (i32, i32) {
        let ox =
            if self.bars.1 { scrollbar::clamp(ox, self.content.width, self.view.width) } else { 0 };
        let oy = if self.bars.0 {
            scrollbar::clamp(oy, self.content.height, self.view.height)
        } else {
            0
        };
        (ox, oy)
    }

    /// Moves the offsets to (`ox`, `oy`), within range. True if anything moved.
    fn set_offset(&mut self, ox: i32, oy: i32) -> bool {
        let (ox, oy) = self.in_range(ox, oy);
        let moved = (ox, oy) != (self.ox, self.oy);
        self.ox = ox;
        self.oy = oy;
        moved
    }

    /// Scrolls by (`dx`, `dy`) pixels. True if anything moved.
    fn scroll_by(&mut self, dx: i32, dy: i32) -> bool {
        self.set_offset(self.ox.saturating_add(dx), self.oy.saturating_add(dy))
    }
}

/// How far a click on the track beside the thumb scrolls: the visible length less one wheel
/// step, so that a line or two stay in view to keep the reader's place.
fn page_of(view: i32) -> i32 {
    (view - WHEEL_STEP).max(view / 2).max(1)
}

impl<'a, M: 'a> Scroll<'a, M> {
    /// Gives `event` to the child (if there is one).
    fn to_child(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        match (tree.children.first_mut(), node.children.first()) {
            (Some(t), Some(n)) => self.child[0].widget.on_event(t, n, event, shell),
            _ => Status::Ignored,
        }
    }

    /// A pointer event, for the child.
    fn pointer_event(&self, tree: &mut Tree, node: &Node, p: &Pointer, shell: &mut Shell<'_, M>) {
        let _ = self.to_child(tree, node, &Event::Pointer(*p), shell);
    }
}

impl<'a, M: 'a> Widget<'a, M> for Scroll<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<ScrollState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(ScrollState::default())
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.child
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let room = limits.max;
        // Whether a bar is needed depends on the child's size, which depends on how much room
        // the other bar leaves it (a vertical bar narrows the child, which may make it taller,
        // or wide enough to need a horizontal bar). Bars are only ever added, so at most two
        // changes and a third pass that finds nothing new settle it.
        let (mut need_v, mut need_h) = (false, false);
        let mut child = Node::default();
        for _ in 0..3 {
            let view = Size::new(
                (room.width - if need_v { THICKNESS } else { 0 }).max(0),
                (room.height - if need_h { THICKNESS } else { 0 }).max(0),
            );
            let across = if self.horizontal { UNBOUNDED } else { view.width };
            let down = if self.vertical { UNBOUNDED } else { view.height };
            child = match tree.children.first_mut() {
                Some(t) => {
                    self.child[0].widget.layout(t, Limits::loose(Size::new(across, down)), cx)
                }
                None => Node::default(),
            };
            let c = child.size();
            let want_v = self.vertical && c.height > view.height;
            let want_h = self.horizontal && c.width > view.width;
            if (!want_v || need_v) && (!want_h || need_h) {
                break;
            }
            need_v |= want_v;
            need_h |= want_h;
        }
        let content = child.size();
        let size = limits.resolve(Size::new(
            content.width.saturating_add(if need_v { THICKNESS } else { 0 }),
            content.height.saturating_add(if need_h { THICKNESS } else { 0 }),
        ));
        let view = Size::new(
            (size.width - if need_v { THICKNESS } else { 0 }).max(0),
            (size.height - if need_h { THICKNESS } else { 0 }).max(0),
        );
        let s = tree.state_mut::<ScrollState>();
        s.content = content;
        s.view = view;
        s.bars = (need_v, need_h);
        // The content may have got shorter or the window bigger since the offsets were set, so
        // they are brought within range. But a parent may first ask how big this area wants to
        // be with no limit across (see `flex`), and then the bars aren't what they will be: a
        // horizontal bar that isn't needed there would wrongly bring the horizontal offset to
        // 0, and a missing one would leave the vertical view too tall. So in such a pass the
        // child is placed within range but the offsets are left as they were; the real pass
        // that follows keeps them.
        let measuring = room.width >= UNBOUNDED || room.height >= UNBOUNDED;
        let (ox, oy) = s.in_range(s.ox, s.oy);
        if !measuring {
            s.ox = ox;
            s.oy = oy;
        }
        child.move_to(-ox, -oy);
        Node::with_children(size, vec![child])
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<ScrollState>();
        let g = scroll_geo(node.rect, s);
        {
            // The child draws as if it had all the room it asked for; the canvas and the
            // `DrawCx` are both cut to the viewport so that what is scrolled out of view is
            // neither drawn nor looked at.
            let mut inner = canvas.clipped(g.viewport);
            let cx = DrawCx {
                theme: cx.theme,
                focus: cx.focus,
                window_focused: cx.window_focused,
                clip: cx.clip.intersect(&g.viewport),
            };
            draw_children(&self.child, tree, node, &mut inner, &cx);
        }
        if let Some(bar) = &g.vbar {
            bar.draw(canvas, cx.theme, s.drag.is_some_and(|d| d.vertical));
        }
        if let Some(bar) = &g.hbar {
            bar.draw(canvas, cx.theme, s.drag.is_some_and(|d| !d.vertical));
        }
        canvas.fill_rect(g.corner, cx.theme.scroll_track);
    }

    fn look(&self, tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<ScrollState>();
        let drag = s.drag.map_or(0, |d| if d.vertical { 1 } else { 2 });
        Some(
            Look::new()
                .i32(s.ox)
                .i32(s.oy)
                .i32(s.content.width)
                .i32(s.content.height)
                .i32(s.view.width)
                .i32(s.view.height)
                .bool(s.bars.0)
                .bool(s.bars.1)
                .u64(drag)
                .finish(),
        )
    }

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
        // What is scrolled out of view never counts as changed: the child's rectangle is cut
        // to the viewport.
        let viewport = scroll_geo(node.rect, tree.state::<ScrollState>()).viewport;
        if let (Some(t), Some(n)) = (tree.children.first(), node.children.first()) {
            self.child[0].widget.looks(t, n, visible.intersect(&viewport), cx, out);
        }
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        match event {
            Event::Pointer(p) => self.on_pointer(tree, node, p, shell),
            Event::Key(k) => {
                if self.to_child(tree, node, event, shell) == Status::Captured {
                    return Status::Captured;
                }
                // Nothing in the child used the key. If the focus is somewhere inside, the
                // keys that mean "move the view" scroll it.
                let inside = tree
                    .children
                    .first()
                    .zip(shell.focused())
                    .is_some_and(|(t, focus)| t.contains(focus));
                if !inside || k.modifiers.for_shortcuts() != Modifiers::NONE {
                    return Status::Ignored;
                }
                let s = tree.state_mut::<ScrollState>();
                // Along the vertical bar if there is one, otherwise across.
                let vertical = s.bars.0;
                if !vertical && !s.bars.1 {
                    return Status::Ignored;
                }
                let (content, view, at) = if vertical {
                    (s.content.height, s.view.height, s.oy)
                } else {
                    (s.content.width, s.view.width, s.ox)
                };
                let target = match k.key {
                    Key::PageUp => at.saturating_sub(page_of(view)),
                    Key::PageDown => at.saturating_add(page_of(view)),
                    Key::Home => 0,
                    Key::End => scrollbar::max_offset(content, view),
                    _ => return Status::Ignored,
                };
                let moved =
                    if vertical { s.set_offset(s.ox, target) } else { s.set_offset(target, s.oy) };
                if moved {
                    shell.relayout();
                }
                Status::Captured
            }
            _ => self.to_child(tree, node, event, shell),
        }
    }

    fn cursor_at(&self, tree: &Tree, node: &Node, at: Point) -> Cursor {
        let viewport = scroll_geo(node.rect, tree.state::<ScrollState>()).viewport;
        if viewport.contains(at.x, at.y)
            && let (Some(t), Some(n)) = (tree.children.first(), node.children.first())
        {
            return self.child[0].widget.cursor_at(t, n, at);
        }
        self.cursor(tree, node, at)
    }

    fn describe(&self, tree: &Tree, _node: &Node) -> Option<Describe> {
        let s = tree.state::<ScrollState>();
        Some(Describe {
            role: "scroll",
            value: alloc::format!("{},{}", s.ox, s.oy),
            enabled: true,
            ..Describe::default()
        })
    }
}

impl<'a, M: 'a> Scroll<'a, M> {
    /// Everything a pointer does over a scroll area: dragging a thumb, clicking a track, the
    /// wheel, and (for what is inside the viewport) the child's own.
    fn on_pointer(
        &self,
        tree: &mut Tree,
        node: &Node,
        p: &Pointer,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let at = p.at;
        let g = scroll_geo(node.rect, tree.state::<ScrollState>());
        let in_view = g.viewport.contains(at.x, at.y);

        // A thumb is being dragged: the pointer belongs to it until the button comes up.
        if let Some(d) = tree.state::<ScrollState>().drag {
            match p.kind {
                PointerKind::Moved if p.buttons & LEFT_HELD != 0 => {
                    let bar = if d.vertical { g.vbar } else { g.hbar };
                    let s = tree.state_mut::<ScrollState>();
                    if let Some(bar) = bar {
                        let (pos, content, view) = if d.vertical {
                            (at.y, s.content.height, s.view.height)
                        } else {
                            (at.x, s.content.width, s.view.width)
                        };
                        let target = bar.offset_at(pos.saturating_sub(d.grab), content, view);
                        let moved = if d.vertical {
                            s.set_offset(s.ox, target)
                        } else {
                            s.set_offset(target, s.oy)
                        };
                        if moved {
                            shell.relayout();
                        }
                    }
                    return Status::Captured;
                }
                // Released, or moved with the button already up (it was let go outside the
                // window, where we heard nothing).
                PointerKind::Released { .. } | PointerKind::Moved => {
                    tree.state_mut::<ScrollState>().drag = None;
                    shell.relayout();
                    return Status::Captured;
                }
                _ => {}
            }
        }

        match p.kind {
            PointerKind::Pressed { button: MouseButton::Left, .. } => {
                for bar in [g.vbar, g.hbar].into_iter().flatten() {
                    if !bar.track.contains(at.x, at.y) {
                        continue;
                    }
                    let (along, thumb_start, view) = if bar.vertical {
                        (at.y, bar.thumb.y, tree.state::<ScrollState>().view.height)
                    } else {
                        (at.x, bar.thumb.x, tree.state::<ScrollState>().view.width)
                    };
                    let s = tree.state_mut::<ScrollState>();
                    if bar.thumb.contains(at.x, at.y) {
                        s.drag = Some(Drag { vertical: bar.vertical, grab: along - thumb_start });
                    } else {
                        // Above or before the thumb: back a page; otherwise forward one.
                        let page = if along < thumb_start { -page_of(view) } else { page_of(view) };
                        if bar.vertical {
                            s.scroll_by(0, page);
                        } else {
                            s.scroll_by(page, 0);
                        }
                    }
                    shell.relayout();
                    return Status::Captured;
                }
                if g.corner.contains(at.x, at.y) {
                    return Status::Captured;
                }
            }
            PointerKind::Scrolled { dx, dy } => {
                if in_view
                    && let Some(t) = tree.children.first_mut()
                    && let Some(n) = node.children.first()
                    && self.child[0].widget.on_event(t, n, &Event::Pointer(*p), shell)
                        == Status::Captured
                {
                    return Status::Captured;
                }
                if !node.rect.contains(at.x, at.y) {
                    return Status::Ignored;
                }
                let s = tree.state_mut::<ScrollState>();
                let (v, h) = s.bars;
                if !v && !h {
                    return Status::Ignored;
                }
                // With only a horizontal bar the wheel's usual up and down scrolls across.
                let (x_steps, y_steps) = if v { (dx, dy) } else { (dx.saturating_add(dy), 0) };
                if s.scroll_by(
                    x_steps.saturating_mul(WHEEL_STEP),
                    y_steps.saturating_mul(WHEEL_STEP),
                ) {
                    shell.relayout();
                }
                return Status::Captured;
            }
            _ => {}
        }

        // Everything else is the child's, as far as it is in view.
        let s = tree.state_mut::<ScrollState>();
        match p.kind {
            PointerKind::Pressed { .. } => {
                if !in_view {
                    return Status::Ignored;
                }
                s.child_pressed = true;
                s.pointer_inside = true;
                self.to_child(tree, node, &Event::Pointer(*p), shell)
            }
            PointerKind::Released { .. } => {
                if !s.child_pressed {
                    return Status::Ignored;
                }
                s.child_pressed = false;
                self.to_child(tree, node, &Event::Pointer(*p), shell)
            }
            PointerKind::Moved => {
                if in_view || s.child_pressed {
                    s.pointer_inside = in_view;
                    return self.to_child(tree, node, &Event::Pointer(*p), shell);
                }
                // The pointer is out over a bar or beyond the area: the child isn't given
                // where, only told, once, that it has left.
                if s.pointer_inside {
                    s.pointer_inside = false;
                    let left = Pointer { kind: PointerKind::Left, ..*p };
                    self.pointer_event(tree, node, &left, shell);
                }
                Status::Ignored
            }
            PointerKind::Left => {
                s.pointer_inside = false;
                self.to_child(tree, node, &Event::Pointer(*p), shell)
            }
            PointerKind::Scrolled { .. } => Status::Ignored,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------------------------

/// How tall a row is (1.3's `FL_ROW`).
const ROW: i32 = 20;
/// How tall the heading row is, with the line under it.
const HEAD: i32 = 21;
/// Room before a cell's text, in its column.
const CELL_PAD: i32 = 6;
/// Where a row's icon is, from the list's left edge, and how big it is.
const ICON_X: i32 = 8;
const ICON_SIZE: i32 = 16;
/// How much the first column's text moves right to make room for the icons.
const ICON_ROOM: i32 = 24;
/// How many rows tall a list that has no height of its own wants to be, at most.
const WANTED_ROWS: usize = 10;

/// One column of a [`List`]: its title, and how wide it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListColumn {
    /// What the heading says.
    pub title: String,
    /// How wide: `Fill` and `Portion` share what the fixed columns leave, `Fixed` is exact,
    /// `Shrink` fits the title.
    pub width: Length,
}

impl ListColumn {
    /// A column titled `title`, `width` wide (for example `("Name", Length::Fill)` and
    /// `("Size", Length::Fixed(80))`).
    pub fn new(title: &str, width: Length) -> ListColumn {
        ListColumn { title: title.to_string(), width }
    }
}

/// One row of a [`List`]: an icon, and a cell of text for each column.
///
/// `'a` is how long the things the row borrows live: a cell may be a `&str` borrowed from the
/// app's own data (no copy made for each of ten thousand rows) or an owned `String`, through
/// [`Cow`] ("clone on write": either a borrowed or an owned string, whichever the app has).
#[derive(Clone, Debug, Default)]
pub struct ListRow<'a> {
    /// The picture before the first cell, usually 16 by 16.
    pub icon: Option<Image<'a>>,
    /// The text of each cell, in the order of the columns. A missing one is empty.
    pub cells: Vec<Cow<'a, str>>,
}

impl<'a> ListRow<'a> {
    /// A row with no icon and no cells.
    pub fn new() -> ListRow<'a> {
        ListRow::default()
    }

    /// With an icon.
    pub fn icon(mut self, icon: Image<'a>) -> ListRow<'a> {
        self.icon = Some(icon);
        self
    }

    /// With an icon, if there is one.
    pub fn icon_maybe(mut self, icon: Option<Image<'a>>) -> ListRow<'a> {
        self.icon = icon;
        self
    }

    /// Adds a cell (a `&str` or a `String`).
    pub fn cell(mut self, text: impl Into<Cow<'a, str>>) -> ListRow<'a> {
        self.cells.push(text.into());
        self
    }

    /// Adds several cells.
    pub fn cells<S: Into<Cow<'a, str>>>(
        mut self,
        texts: impl IntoIterator<Item = S>,
    ) -> ListRow<'a> {
        self.cells.extend(texts.into_iter().map(Into::into));
        self
    }
}

/// A list of rows in columns, with a heading row, one selected row and its own scroll bar:
/// the shape of 1.3's Files window.
///
/// The app keeps the selection and the sort order: the list sends [`List::on_select`] when the
/// user picks a row (click, or the arrow keys), [`List::on_activate`] for a double click or
/// Enter, and [`List::on_sort`] for a click on a heading, and shows what the app says
/// ([`List::selected`], [`List::sorted`]) on the next view. Only the rows in view are drawn, so
/// a list of ten thousand costs what a list of twenty does.
pub struct List<'a, M> {
    columns: Vec<ListColumn>,
    rows: Vec<ListRow<'a>>,
    selected: Option<usize>,
    sorted: Option<(usize, bool)>,
    empty: String,
    // `Box<dyn Fn(usize) -> M>` holds any function from a row number to a message, whatever
    // closure the app wrote; it may borrow from the app's state for `'a`.
    on_select: Option<Box<dyn Fn(usize) -> M + 'a>>,
    on_activate: Option<Box<dyn Fn(usize) -> M + 'a>>,
    on_sort: Option<Box<dyn Fn(usize) -> M + 'a>>,
    width: Length,
    height: Length,
}

impl<'a, M: 'a> List<'a, M> {
    /// A list of `rows` under the headings of `columns`.
    pub fn new(columns: Vec<ListColumn>, rows: Vec<ListRow<'a>>) -> List<'a, M> {
        List {
            columns,
            rows,
            selected: None,
            sorted: None,
            empty: String::new(),
            on_select: None,
            on_activate: None,
            on_sort: None,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Which row is selected (the app keeps this). A row that isn't there selects nothing.
    pub fn selected(mut self, row: Option<usize>) -> List<'a, M> {
        self.selected = row;
        self
    }

    /// What to send when the user picks a row: with a click, or by moving the selection with
    /// the keyboard. A right click picks the row too.
    pub fn on_select(mut self, f: impl Fn(usize) -> M + 'a) -> List<'a, M> {
        self.on_select = Some(Box::new(f));
        self
    }

    /// What to send for a double click on a row, or Enter with a row selected.
    pub fn on_activate(mut self, f: impl Fn(usize) -> M + 'a) -> List<'a, M> {
        self.on_activate = Some(Box::new(f));
        self
    }

    /// The rows are sorted by this column: its heading shows an arrow, up for `ascending`.
    pub fn sorted(mut self, column: usize, ascending: bool) -> List<'a, M> {
        self.sorted = Some((column, ascending));
        self
    }

    /// What to send, with the column's number, for a click on a heading (the app sorts, and
    /// flips the order if it was that column already).
    pub fn on_sort(mut self, f: impl Fn(usize) -> M + 'a) -> List<'a, M> {
        self.on_sort = Some(Box::new(f));
        self
    }

    /// What to say when there are no rows ("This folder is empty.").
    pub fn empty(mut self, text: &str) -> List<'a, M> {
        self.empty = text.to_string();
        self
    }

    /// How wide it wants to be (all the room, by default).
    pub fn width(mut self, w: Length) -> List<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be (all the room, by default).
    pub fn height(mut self, h: Length) -> List<'a, M> {
        self.height = h;
        self
    }
}

impl<'a, M: 'a> From<List<'a, M>> for Element<'a, M> {
    fn from(w: List<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// What a list remembers between views.
#[derive(Default)]
struct ListState {
    /// How far the rows are scrolled, in pixels.
    offset: i32,
    /// The selection as of the last layout: when the app's differs, the selected row is
    /// brought into view (and not otherwise, so that scrolling away from the selection with
    /// the wheel isn't undone by the next view).
    seen: Option<usize>,
    /// A thumb is being dragged, grabbed this far into the thumb.
    drag: Option<i32>,
    /// Where each column starts and how wide it is, as laid out last (what a click on a
    /// heading is matched against: only layout is given the theme, whose font decides the
    /// width of a column that fits its title).
    spans: Vec<(i32, i32)>,
}

/// The height of `rows` rows, without overflowing.
fn rows_height(rows: usize) -> i32 {
    (rows as u64).saturating_mul(ROW as u64).min(i32::MAX as u64 / 2) as i32
}

/// Where the parts of a list are.
struct ListGeo {
    /// Inside the edge.
    inner: Rect,
    /// Where rows show (not under the scroll bar).
    rows: Rect,
    bar: Option<Bar>,
}

impl<'a, M: 'a> List<'a, M> {
    fn geo(&self, rect: Rect, offset: i32) -> ListGeo {
        let inner = rect.inset(1);
        let below = Rect::new(inner.x, inner.y + HEAD, inner.width, (inner.height - HEAD).max(0));
        let below = below.intersect(&inner);
        let content = rows_height(self.rows.len());
        let bar = if content > below.height {
            let w = THICKNESS.min(below.width);
            let track = Rect::new(below.x + below.width - w, below.y, w, below.height);
            scrollbar::bar(track, true, content, below.height, offset)
        } else {
            None
        };
        let bar_w = bar.map_or(0, |b| b.track.width);
        let rows = Rect::new(below.x, below.y, below.width - bar_w, below.height);
        ListGeo { inner, rows, bar }
    }

    /// Whether any row has an icon (then the first column's text makes room for them).
    fn has_icons(&self) -> bool {
        self.rows.iter().any(|r| r.icon.is_some())
    }

    /// Where each column starts (from the list's inner left edge) and how wide it is, in
    /// `total` pixels.
    fn spans(&self, total: i32, theme: &Theme) -> Vec<(i32, i32)> {
        let adv = theme.bold.advance();
        let wanted = |i: usize, c: &ListColumn| match c.width {
            Length::Fixed(n) => n.max(0),
            // A title, its margins, and the arrow that may follow it.
            _ => {
                let arrow = if self.sorted.is_some_and(|(s, _)| s == i) { 2 * adv } else { 0 };
                theme.bold.width(&c.title).saturating_add(2 * CELL_PAD + arrow)
            }
        };
        let mut left = total;
        let mut shares = 0u32;
        for (i, c) in self.columns.iter().enumerate() {
            match c.width.share() {
                0 => left = left.saturating_sub(wanted(i, c)),
                n => shares += n,
            }
        }
        let mut left = left.max(0);
        let mut out = Vec::with_capacity(self.columns.len());
        let mut x = 0i32;
        for (i, c) in self.columns.iter().enumerate() {
            let share = c.width.share();
            let w = if share == 0 {
                wanted(i, c)
            } else {
                // Proportional; the last takes what rounding left.
                let part = if share == shares {
                    left
                } else {
                    (i64::from(left) * i64::from(share) / i64::from(shares)) as i32
                };
                shares -= share;
                left -= part;
                part
            };
            out.push((x, w));
            x = x.saturating_add(w);
        }
        out
    }

    /// The rows that show with the list scrolled by `offset` in a view `view` tall:
    /// `first..last`, partly visible ones included.
    fn visible(&self, offset: i32, view: i32) -> core::ops::Range<usize> {
        let first = (offset.max(0) / ROW) as usize;
        let last = ((i64::from(offset.max(0)) + i64::from(view.max(0)) + i64::from(ROW) - 1)
            / i64::from(ROW)) as usize;
        first.min(self.rows.len())..last.min(self.rows.len())
    }

    /// The row the keyboard or the wheel moves to from the selected one, for the key.
    fn key_target(&self, key: Key, page: usize) -> Option<usize> {
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let current = self.selected.filter(|s| *s < n);
        Some(match (key, current) {
            (Key::Down, Some(c)) => (c + 1).min(n - 1),
            (Key::Up, Some(c)) => c.saturating_sub(1),
            (Key::PageDown, Some(c)) => c.saturating_add(page).min(n - 1),
            (Key::PageUp, Some(c)) => c.saturating_sub(page),
            (Key::Home, _) => 0,
            (Key::End, _) => n - 1,
            // Nothing selected yet: forwards keys start at the top, backwards ones at the end.
            (Key::Down | Key::PageDown, None) => 0,
            (Key::Up | Key::PageUp, None) => n - 1,
            _ => return None,
        })
    }
}

impl<'a, M: 'a> Widget<'a, M> for List<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<ListState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(ListState::default())
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let wanted_rows = self.rows.len().clamp(1, WANTED_ROWS);
        let size = limits.resolve(Size::new(200, 2 + HEAD + rows_height(wanted_rows)));
        let view = (size.height - 2 - HEAD).max(0);
        let content = rows_height(self.rows.len());
        let s = tree.state_mut::<ListState>();
        // A selection the app changed is brought into view; the offset is then brought within
        // range (the rows got fewer, or the window bigger).
        if self.selected != s.seen {
            if let Some(row) = self.selected.filter(|r| *r < self.rows.len()) {
                let top = rows_height(row);
                if top < s.offset {
                    s.offset = top;
                } else if top + ROW > s.offset + view {
                    s.offset = top + ROW - view;
                }
            }
            s.seen = self.selected;
        }
        s.offset = scrollbar::clamp(s.offset, content, view);
        let rows_width = self.geo(Rect::new(0, 0, size.width, size.height), s.offset).rows.width;
        s.spans = self.spans(rows_width, cx.theme);
        Node::new(size)
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<ListState>();
        let t = cx.theme;
        let keyboard = cx.has_keyboard(tree);
        let r = node.rect;
        let g = self.geo(r, s.offset);
        canvas.fill_rect(r, if keyboard { t.accent } else { t.field_edge });
        canvas.fill_rect(g.inner, t.field);

        // The heading: paper, the titles, the line under it.
        let spans = self.spans(g.rows.width, t);
        let head = Rect::new(g.inner.x, g.inner.y, g.inner.width, HEAD - 1);
        canvas.fill_rect(head, t.gutter);
        canvas
            .fill_rect(Rect::new(g.inner.x, g.inner.y + HEAD - 1, g.inner.width, 1), t.gutter_edge);
        {
            let mut c = canvas.clipped(Rect::new(g.inner.x, g.inner.y, g.rows.width, HEAD));
            for (i, (col, (x, w))) in self.columns.iter().zip(&spans).enumerate() {
                let arrow = match self.sorted {
                    Some((sc, asc)) if sc == i => Some(if asc { '↑' } else { '↓' }),
                    _ => None,
                };
                let room =
                    w - 2 * CELL_PAD - if arrow.is_some() { 2 * t.bold.advance() } else { 0 };
                let mut title = super::fit(&col.title, t.bold, room.max(0));
                if let Some(a) = arrow {
                    title.push(' ');
                    title.push(a);
                }
                let cell = Rect::new(g.inner.x + x, g.inner.y, *w, HEAD - 1);
                c.text(cell.x + CELL_PAD, super::text_top(cell, 1), &title, t.strong, t.bold);
            }
        }

        // The rows in view: only those, and only those the redraw reaches.
        let shown = g.rows.intersect(&cx.clip);
        if !shown.is_empty() {
            let mut c = canvas.clipped(shown);
            let first_y = i64::from(shown.y - g.rows.y) + i64::from(s.offset);
            let range = self.visible(i32::try_from(first_y).unwrap_or(0), shown.height);
            let icons = self.has_icons();
            let selection = if keyboard { t.selection } else { t.selection_inactive };
            for i in range {
                let row = &self.rows[i];
                let y = g.rows.y + rows_height(i) - s.offset;
                let band = Rect::new(g.rows.x, y, g.rows.width, ROW);
                if self.selected == Some(i) {
                    c.fill_rect(band, selection);
                }
                if let Some(icon) = &row.icon {
                    let at = Rect::new(
                        g.rows.x + ICON_X,
                        y + (ROW - ICON_SIZE) / 2,
                        ICON_SIZE,
                        ICON_SIZE,
                    );
                    icon.draw(&mut c, at);
                }
                for (ci, (x, w)) in spans.iter().enumerate() {
                    let Some(text) = row.cells.get(ci) else {
                        continue;
                    };
                    let indent = if ci == 0 && icons { ICON_ROOM } else { 0 };
                    let room = w - CELL_PAD - indent - CELL_PAD;
                    let shown_text = super::fit(text, t.font, room.max(0));
                    let cell = Rect::new(g.rows.x + x, y, *w, ROW);
                    c.text(
                        cell.x + CELL_PAD + indent,
                        super::text_top(cell, 1),
                        &shown_text,
                        t.text,
                        t.font,
                    );
                }
            }
        }

        // The empty list says so, in the middle of where its first row would be.
        if self.rows.is_empty() && !self.empty.is_empty() {
            let first = Rect::new(g.rows.x, g.rows.y, g.rows.width, ROW);
            let text = super::fit(&self.empty, t.font, (g.rows.width - 2 * CELL_PAD).max(0));
            let x = first.x + (first.width - t.font.width(&text)).max(0) / 2;
            let mut c = canvas.clipped(g.rows);
            c.text(x, super::text_top(first, 1), &text, t.text_dim, t.font);
        }

        if let Some(bar) = &g.bar {
            bar.draw(canvas, t, s.drag.is_some());
        }
    }

    fn look(&self, tree: &Tree, node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<ListState>();
        let g = self.geo(node.rect, s.offset);
        let mut look = Look::new()
            .bool(cx.has_keyboard(tree))
            .bool(cx.window_focused)
            .bool(cx.is_focused(tree))
            .i32(s.offset)
            .u64(self.rows.len() as u64)
            .u64(self.selected.map_or(0, |n| n as u64 + 1))
            .bool(s.drag.is_some())
            .bool(self.has_icons());
        let sorted = self.sorted.map_or(0, |(c, asc)| (c as u64) << 2 | 2 | u64::from(asc));
        look = look.u64(sorted);
        for c in &self.columns {
            let w = match c.width {
                Length::Shrink => 0,
                Length::Fill => 1,
                Length::Portion(n) => 2 + u64::from(n),
                Length::Fixed(n) => 0x1_0000_0000 + u64::from(n as u32),
            };
            look = look.str(&c.title).u64(w);
        }
        if self.rows.is_empty() {
            look = look.str(&self.empty);
        }
        // Only the rows in view: a list of ten thousand hashes what a list of twenty does.
        for i in self.visible(s.offset, g.rows.height) {
            let row = &self.rows[i];
            look = look.bool(row.icon.is_some());
            if let Some(icon) = &row.icon {
                look = icon.look(look);
            }
            look = look.u64(row.cells.len() as u64);
            for cell in &row.cells {
                look = look.str(cell);
            }
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
            Event::Pointer(p) => self.on_pointer(tree, node, p, shell),
            Event::Key(k) if shell.is_focused(id) => {
                let mods = k.modifiers;
                if mods.ctrl() || mods.alt() || mods.meta() {
                    return Status::Ignored;
                }
                if k.key == Key::Enter {
                    let row = self.selected.filter(|r| *r < self.rows.len());
                    return match (row, &self.on_activate) {
                        // A held Enter opens the row once, not over and over.
                        (Some(row), Some(f)) => {
                            if !k.repeat {
                                shell.publish(f(row));
                            }
                            Status::Captured
                        }
                        _ => Status::Ignored,
                    };
                }
                let offset = tree.state::<ListState>().offset;
                let view = self.geo(node.rect, offset).rows.height;
                let page = ((view / ROW) as usize).saturating_sub(1).max(1);
                let Some(target) = self.key_target(k.key, page) else {
                    return Status::Ignored;
                };
                if self.selected != Some(target)
                    && let Some(f) = &self.on_select
                {
                    shell.publish(f(target));
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        true
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        let titles: Vec<&str> = self.columns.iter().map(|c| c.title.as_str()).collect();
        let value = self
            .selected
            .and_then(|i| self.rows.get(i))
            .map(|r| r.cells.iter().map(|c| &**c).collect::<Vec<&str>>().join("|"))
            .unwrap_or_default();
        Some(Describe {
            role: "list",
            label: titles.join("|"),
            value,
            enabled: true,
            checked: None,
        })
    }
}

impl<'a, M: 'a> List<'a, M> {
    fn on_pointer(
        &self,
        tree: &mut Tree,
        node: &Node,
        p: &Pointer,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let at = p.at;
        let g = self.geo(node.rect, tree.state::<ListState>().offset);
        let content = rows_height(self.rows.len());
        let view = g.rows.height;

        // A thumb being dragged owns the pointer until the button comes up.
        if let Some(grab) = tree.state::<ListState>().drag {
            match p.kind {
                PointerKind::Moved if p.buttons & LEFT_HELD != 0 => {
                    if let Some(bar) = g.bar {
                        let target = bar.offset_at(at.y.saturating_sub(grab), content, view);
                        tree.state_mut::<ListState>().offset = target;
                        shell.redraw();
                    }
                    return Status::Captured;
                }
                PointerKind::Released { .. } | PointerKind::Moved => {
                    tree.state_mut::<ListState>().drag = None;
                    shell.redraw();
                    return Status::Captured;
                }
                _ => {}
            }
        }

        match p.kind {
            PointerKind::Pressed { button, clicks } => {
                if !node.rect.contains(at.x, at.y) {
                    return Status::Ignored;
                }
                let left = button == MouseButton::Left;
                if !left && button != MouseButton::Right {
                    return Status::Ignored;
                }
                shell.focus(id);
                if let Some(bar) = g.bar.filter(|b| b.track.contains(at.x, at.y)) {
                    if !left {
                        return Status::Captured;
                    }
                    let s = tree.state_mut::<ListState>();
                    if bar.thumb.contains(at.x, at.y) {
                        s.drag = Some(at.y - bar.thumb.y);
                    } else {
                        let page = page_of(view);
                        let by = if at.y < bar.thumb.y { -page } else { page };
                        s.offset = scrollbar::clamp(s.offset.saturating_add(by), content, view);
                    }
                    shell.redraw();
                    return Status::Captured;
                }
                // The heading: a click on a title sorts by it.
                let head = Rect::new(g.inner.x, g.inner.y, g.rows.width, HEAD);
                if head.contains(at.x, at.y) {
                    if left {
                        let column =
                            tree.state::<ListState>().spans.iter().position(|(x, w)| {
                                at.x >= g.inner.x + x && at.x < g.inner.x + x + w
                            });
                        if let (Some(c), Some(f)) = (column, &self.on_sort) {
                            shell.publish(f(c));
                        }
                    }
                    return Status::Captured;
                }
                if g.rows.contains(at.x, at.y) {
                    let offset = tree.state::<ListState>().offset;
                    let row = (i64::from(at.y - g.rows.y) + i64::from(offset)) / i64::from(ROW);
                    if let Some(row) = usize::try_from(row).ok().filter(|r| *r < self.rows.len()) {
                        // The second press of a double click opens the row; the first
                        // already selected it. Any other press (and a right one) selects.
                        let f =
                            if left && clicks == 2 { &self.on_activate } else { &self.on_select };
                        if let Some(f) = f {
                            shell.publish(f(row));
                        }
                    }
                }
                Status::Captured
            }
            PointerKind::Scrolled { dy, .. } => {
                if !node.rect.contains(at.x, at.y) || content <= view {
                    return Status::Ignored;
                }
                let s = tree.state_mut::<ListState>();
                let to = scrollbar::clamp(
                    s.offset.saturating_add(dy.saturating_mul(WHEEL_STEP)),
                    content,
                    view,
                );
                if to != s.offset {
                    s.offset = to;
                    shell.redraw();
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
