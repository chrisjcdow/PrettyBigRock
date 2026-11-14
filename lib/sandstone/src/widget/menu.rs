// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`MenuBar`], [`Menu`], [`MenuItem`], [`ContextMenu`], [`Toolbar`] and [`StatusBar`]
//! (note 0023, section 5; note 0022, section 3 for popups).
//!
//! A menu is a *popup*: a small window of its own that the display server shows over its
//! parent window and closes again when the user clicks anywhere else. The display server only
//! makes a popup in answer to a key or button press (so a program can't fill the screen with
//! them), which is why a widget opens one with [`Shell::open_popup`] while it handles a press.
//! Sandstone then asks the widget that opened it for the popup's contents
//! ([`Widget::popup`]), here a [`MenuList`], and gives that window its own events.
//!
//! A word on Rust, once for the file. Most types here are generic over `M`, the app's message
//! type (`MenuItem<M>` holds the `M` its row sends), and an element may borrow from the view
//! that made it for a lifetime written `'a` (see `element.rs`). Where a method ends in `?` it
//! returns early with `None` if a lookup came back empty, instead of carrying on without a
//! value.

use crate::Look;
use crate::element::{Describe, Description, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::event::{Button as MouseButton, Event, Key, PointerKind, Status};
use crate::geometry::{Align, Length, Limits, Padding, Point, Rect, Size};
use crate::layout::{self, Axis, Flex, Node};
use crate::shell::Shell;
use crate::shortcut::Shortcut;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::any::{Any, TypeId};
use pbr_draw::{Canvas, TextStyle};

// ---------------------------------------------------------------------------------------------
// Menus: the items.
// ---------------------------------------------------------------------------------------------

/// One row of a menu: a label that sends a message when chosen, or a separator line between
/// groups of rows.
pub struct MenuItem<M> {
    label: String,
    /// What choosing it sends; `None` for a separator.
    message: Option<M>,
    shortcut: Option<Shortcut>,
    enabled: bool,
    /// `Some` for a row that can be ticked: whether it is.
    checked: Option<bool>,
    separator: bool,
}

impl<M> MenuItem<M> {
    /// A row labelled `label` that sends `message` when chosen.
    pub fn new(label: &str, message: M) -> MenuItem<M> {
        MenuItem {
            label: label.to_string(),
            message: Some(message),
            shortcut: None,
            enabled: true,
            checked: None,
            separator: false,
        }
    }

    /// A line between groups of rows.
    pub fn separator() -> MenuItem<M> {
        MenuItem {
            label: String::new(),
            message: None,
            shortcut: None,
            enabled: false,
            checked: None,
            separator: true,
        }
    }

    /// The shortcut: shown at the right of the row, and it works while the menu is shut too
    /// (unless the row is disabled).
    pub fn shortcut(mut self, shortcut: Shortcut) -> MenuItem<M> {
        self.shortcut = Some(shortcut);
        self
    }

    /// Whether the row can be chosen (greyed out when not, and its shortcut does nothing).
    pub fn enabled(mut self, enabled: bool) -> MenuItem<M> {
        self.enabled = enabled;
        self
    }

    /// Makes the row tickable, and says whether it is ticked now ("Word wrap").
    pub fn checked(mut self, checked: bool) -> MenuItem<M> {
        self.checked = Some(checked);
        self
    }

    /// The row can be chosen.
    fn usable(&self) -> bool {
        self.enabled && !self.separator && self.message.is_some()
    }

    /// How tall its row is.
    fn height(&self) -> i32 {
        if self.separator { SEPARATOR_HEIGHT } else { ROW_HEIGHT }
    }

    /// The shortcut as the row shows it, or nothing.
    fn shortcut_label(&self) -> String {
        self.shortcut.map(|s| s.label()).unwrap_or_default()
    }
}

/// A menu of a menu bar: a title and the rows it opens.
pub struct Menu<M> {
    title: String,
    items: Vec<MenuItem<M>>,
}

impl<M> Menu<M> {
    /// A menu called `title` ("File") with these rows.
    pub fn new(title: &str, items: Vec<MenuItem<M>>) -> Menu<M> {
        Menu { title: title.to_string(), items }
    }
}

// ---------------------------------------------------------------------------------------------
// The menu itself: the contents of the popup.
// ---------------------------------------------------------------------------------------------

/// A row's height, and a separator's.
const ROW_HEIGHT: i32 = 22;
const SEPARATOR_HEIGHT: i32 = 9;
/// The narrowest a menu is, and the widest (a longer label is cut short with "…").
const MENU_MIN_WIDTH: i32 = 140;
const MENU_MAX_WIDTH: i32 = 640;
/// Where a row's check mark and label start, and how far the shortcut is from the right edge.
const CHECK_X: i32 = 8;
const LABEL_X: i32 = 24;
const SHORTCUT_RIGHT: i32 = 10;
/// The least room between a label and its shortcut.
const LABEL_GAP: i32 = 20;

/// What a menu's popup remembers: the row lit by the pointer or the arrow keys.
#[derive(Default)]
struct ListState {
    highlight: Option<usize>,
}

/// The contents of a menu's popup: the rows, drawn and handled as one widget. It borrows the
/// rows from the menu bar (or context menu) that opened it, for the lifetime `'a`.
struct MenuList<'a, M> {
    items: &'a [MenuItem<M>],
    /// Start with the first usable row lit (the menu was opened with the keyboard).
    first: bool,
}

impl<M> MenuList<'_, M> {
    /// The first usable row, if any.
    fn first_usable(&self) -> Option<usize> {
        self.items.iter().position(MenuItem::usable)
    }

    /// The row to light after moving `step` (1 down, -1 up) from `from`, skipping separators
    /// and rows that can't be chosen, and wrapping round at the ends.
    fn step(&self, from: Option<usize>, step: i32) -> Option<usize> {
        let n = self.items.len();
        if n == 0 {
            return None;
        }
        let start = match (from, step > 0) {
            (Some(i), _) => i,
            // Nothing lit yet: Down goes to the first row, Up to the last.
            (None, true) => n - 1,
            (None, false) => 0,
        };
        let mut i = start;
        for _ in 0..n {
            i = if step > 0 { (i + 1) % n } else { (i + n - 1) % n };
            if self.items[i].usable() {
                return Some(i);
            }
        }
        None
    }

    /// The last usable row, if any.
    fn last_usable(&self) -> Option<usize> {
        self.items.iter().rposition(MenuItem::usable)
    }

    /// Where row `i` is, given the popup's rectangle (rows start inside the 1-pixel edge).
    fn row_rect(&self, popup: Rect, i: usize) -> Rect {
        let y: i32 = self.items[..i.min(self.items.len())]
            .iter()
            .fold(popup.y.saturating_add(1), |y, it| y.saturating_add(it.height()));
        let h = self.items.get(i).map_or(0, MenuItem::height);
        Rect::new(popup.x.saturating_add(1), y, (popup.width - 2).max(0), h)
    }

    /// The row at `at`, if the point is over one.
    fn row_at(&self, popup: Rect, at: Point) -> Option<usize> {
        if !popup.contains(at.x, at.y) {
            return None;
        }
        let mut y = popup.y.saturating_add(1);
        for (i, it) in self.items.iter().enumerate() {
            let next = y.saturating_add(it.height());
            if at.y >= y && at.y < next && at.x > popup.x && at.x < popup.x + popup.width - 1 {
                return Some(i);
            }
            y = next;
        }
        None
    }

    /// How wide the menu is: the widest label, the widest shortcut and the margins, within
    /// the least and the most.
    fn width(&self, font: TextStyle) -> i32 {
        let widest = |f: &dyn Fn(&MenuItem<M>) -> String| {
            self.items.iter().map(|it| i64::from(font.width(&f(it)))).max().unwrap_or(0)
        };
        let label = widest(&|it| it.label.clone());
        let shortcut = widest(&|it| it.shortcut_label());
        let gap = if shortcut > 0 { i64::from(LABEL_GAP) + shortcut } else { 0 };
        let wanted = i64::from(LABEL_X) + label + gap + i64::from(SHORTCUT_RIGHT);
        wanted.clamp(i64::from(MENU_MIN_WIDTH), i64::from(MENU_MAX_WIDTH)) as i32
    }
}

impl<'a, M: Clone + 'a> Widget<'a, M> for MenuList<'a, M> {
    fn tag(&self) -> TypeId {
        TypeId::of::<ListState>()
    }

    fn state(&self) -> Box<dyn Any> {
        let highlight = if self.first { self.first_usable() } else { None };
        Box::new(ListState { highlight })
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let height: i32 = self.items.iter().fold(2, |h, it| h.saturating_add(it.height()));
        Node::new(limits.resolve(Size::new(self.width(cx.theme.font), height)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let t = cx.theme;
        let r = node.rect;
        canvas.fill_rect(r, t.button_edge);
        canvas.fill_rect(r.inset(1), t.menu);
        let highlight = tree.state::<ListState>().highlight;
        for (i, it) in self.items.iter().enumerate() {
            let row = self.row_rect(r, i);
            if it.separator {
                canvas.fill_rect(
                    Rect::new(r.x + 6, row.y + 4, (r.width - 12).max(0), 1),
                    t.separator,
                );
                continue;
            }
            if highlight == Some(i) {
                let lit = Rect::new(r.x + 2, row.y + 1, (r.width - 4).max(0), row.height - 2);
                canvas.fill_round_rect(lit, 3, t.menu_hover);
            }
            let colour = if it.enabled { t.text } else { t.text_disabled };
            let top = super::text_top(row, t.font.size);
            if it.checked == Some(true) {
                canvas.text(r.x + CHECK_X, top, "✓", colour, t.font);
            }
            let shortcut = it.shortcut_label();
            let sc_width = t.font.width(&shortcut);
            let room = r.width - LABEL_X - SHORTCUT_RIGHT;
            let room = if sc_width > 0 { room - sc_width - LABEL_GAP } else { room };
            let label = super::fit(&it.label, t.font, room.max(0));
            canvas.text(r.x + LABEL_X, top, &label, colour, t.font);
            if sc_width > 0 {
                let dim = if it.enabled { t.text_dim } else { t.text_disabled };
                let x = r.x + r.width - SHORTCUT_RIGHT - sc_width;
                canvas.text(x, top, &shortcut, dim, t.font);
            }
        }
    }

    fn look(&self, tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        let mut look =
            Look::new().u64(tree.state::<ListState>().highlight.map_or(0, |i| i as u64 + 1));
        for it in self.items {
            look = look
                .str(&it.label)
                .str(&it.shortcut_label())
                .bool(it.enabled)
                .bool(it.separator)
                .u64(it.checked.map_or(0, |c| 1 + u64::from(c)));
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
        let s = tree.state_mut::<ListState>();
        match event {
            Event::Pointer(p) => {
                let over =
                    if p.kind == PointerKind::Left { None } else { self.row_at(node.rect, p.at) };
                // The pointer lights the row under it, if that row can be chosen.
                let lit = over.filter(|&i| self.items[i].usable());
                let moved = matches!(p.kind, PointerKind::Moved | PointerKind::Left);
                if (moved || p.pressed(MouseButton::Left).is_some()) && lit != s.highlight {
                    s.highlight = lit;
                    shell.redraw();
                }
                if p.pressed(MouseButton::Left).is_some() && node.rect.contains(p.at.x, p.at.y) {
                    return Status::Captured;
                }
                // Letting go over a row chooses it: that makes both "click the title, click the
                // row" and "press on the title, drag down, let go" work.
                if p.released(MouseButton::Left)
                    && let Some(i) = lit
                    && let Some(message) = &self.items[i].message
                {
                    shell.publish(message.clone());
                    return Status::Captured;
                }
                Status::Ignored
            }
            Event::Key(k) => {
                let from = s.highlight;
                let to = match k.key {
                    Key::Down => self.step(from, 1),
                    Key::Up => self.step(from, -1),
                    Key::Home => self.first_usable(),
                    Key::End => self.last_usable(),
                    Key::Enter => {
                        if !k.repeat
                            && let Some(i) = from
                            && let Some(message) = &self.items[i].message
                            && self.items[i].usable()
                        {
                            shell.publish(message.clone());
                        }
                        return Status::Captured;
                    }
                    // The neighbouring menu of the menu bar, if this popup has one.
                    Key::Left => {
                        shell.step_popup(-1);
                        return Status::Captured;
                    }
                    Key::Right => {
                        shell.step_popup(1);
                        return Status::Captured;
                    }
                    Key::Function(10) => {
                        shell.close_popup();
                        return Status::Captured;
                    }
                    // Escape is left alone: the core closes the popup for it.
                    _ => return Status::Ignored,
                };
                if to != from {
                    s.highlight = to;
                    shell.redraw();
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }

    fn describe_all(
        &self,
        tree: &Tree,
        node: &Node,
        _focus: Option<u64>,
        out: &mut Vec<Description>,
    ) {
        let highlight = tree.state::<ListState>().highlight;
        for (i, it) in self.items.iter().enumerate() {
            if it.separator {
                continue;
            }
            out.push(Description {
                what: Describe {
                    role: "menu item",
                    label: it.label.clone(),
                    value: it.shortcut_label(),
                    enabled: it.usable(),
                    checked: it.checked,
                },
                rect: self.row_rect(node.rect, i),
                id: tree.id,
                key: None,
                // The lit row stands in for the focus.
                focused: highlight == Some(i),
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// MenuBar
// ---------------------------------------------------------------------------------------------

/// How far in the first title starts, and the room each side of a title's text.
const BAR_LEFT: i32 = 4;
const TITLE_SIDE: i32 = 8;
/// A press on a title this soon (milliseconds) after its menu was closed by that same press
/// (the display server closes a popup before it gives the press to the window under it) is
/// not a request to open it again.
const REPRESS_MS: u64 = 100;

/// What a menu bar remembers between views.
#[derive(Default)]
struct BarState {
    /// The menu whose popup is open.
    open: Option<usize>,
    /// The menu to open as soon as the popup now open has closed (opening a popup closes the
    /// one before it, and that closing must not make the bar forget the new one).
    pending: Option<usize>,
    /// The title under the pointer.
    hover: Option<usize>,
    /// The popup was opened with the keyboard: light its first row.
    first: bool,
    /// When (milliseconds) and which menu was last closed with nothing to follow it.
    closed: Option<(u64, usize)>,
}

/// The strip across the top of a window with the menus' titles. A press on a title opens its
/// menu under it; while one is open, moving over another title opens that one; F10 opens the
/// first from the keyboard, and Left and Right in a menu move along the bar.
pub struct MenuBar<M> {
    menus: Vec<Menu<M>>,
}

impl<M> MenuBar<M> {
    /// A menu bar with these menus, left to right.
    pub fn new(menus: Vec<Menu<M>>) -> MenuBar<M> {
        MenuBar { menus }
    }

    /// The title under `at`: its number and where it starts and how wide it is, given the
    /// bar's rectangle. Sums in `i64` so that a huge title can't overflow.
    fn title_at(&self, font: TextStyle, bar: Rect, at: Point) -> Option<(usize, i32, i32)> {
        if !bar.contains(at.x, at.y) {
            return None;
        }
        let mut x = i64::from(bar.x) + i64::from(BAR_LEFT);
        for (i, m) in self.menus.iter().enumerate() {
            let w = i64::from(font.width(&m.title)) + i64::from(2 * TITLE_SIDE);
            if i64::from(at.x) >= x && i64::from(at.x) < x + w {
                return Some((i, x.clamp(-1 << 30, 1 << 30) as i32, w.min(1 << 30) as i32));
            }
            x += w;
        }
        None
    }

    /// Where title `i` starts.
    fn title_x(&self, font: TextStyle, bar: Rect, i: usize) -> i32 {
        let x = self.menus[..i.min(self.menus.len())]
            .iter()
            .fold(i64::from(bar.x) + i64::from(BAR_LEFT), |x, m| {
                x + i64::from(font.width(&m.title)) + i64::from(2 * TITLE_SIDE)
            });
        x.clamp(-1 << 30, 1 << 30) as i32
    }
}

/// Asks for menu `i`'s popup, just under its title. `first` lights its first row.
fn open_menu<M>(
    state: &mut BarState,
    shell: &mut Shell<'_, M>,
    owner: u64,
    at: Point,
    i: usize,
    first: bool,
) {
    state.first = first;
    if state.open.is_some() {
        // Opening another popup closes this one first, and the bar hears of that; `pending`
        // tells it what is to follow.
        state.pending = Some(i);
    } else {
        state.open = Some(i);
    }
    shell.open_popup(owner, at);
    shell.redraw();
}

impl<'a, M: Clone + 'a> From<MenuBar<M>> for Element<'a, M> {
    fn from(bar: MenuBar<M>) -> Element<'a, M> {
        Element::new(bar)
    }
}

impl<'a, M: Clone + 'a> Widget<'a, M> for MenuBar<M> {
    fn size(&self) -> (Length, Length) {
        (Length::Fill, Length::Shrink)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<BarState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(BarState::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(Length::Fill);
        let titles: i64 = self
            .menus
            .iter()
            .map(|m| i64::from(cx.theme.font.width(&m.title)) + i64::from(2 * TITLE_SIDE))
            .sum();
        let wanted = (titles + 2 * i64::from(BAR_LEFT)).min(i64::from(i32::MAX)) as i32;
        Node::new(limits.resolve(Size::new(wanted, cx.theme.control_height)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<BarState>();
        let t = cx.theme;
        let r = node.rect;
        canvas.fill_rect(r, t.bar);
        canvas.fill_rect(Rect::new(r.x, r.y + r.height - 1, r.width, 1), t.bar_edge);
        for (i, m) in self.menus.iter().enumerate() {
            let x = self.title_x(t.font, r, i);
            let w = t.font.width(&m.title).saturating_add(2 * TITLE_SIDE);
            if s.open == Some(i) || s.hover == Some(i) {
                let lit = Rect::new(x, r.y + 2, w, (r.height - 5).max(0));
                canvas.fill_round_rect(lit, 3, t.menu_hover);
            }
            let top = super::text_top(Rect::new(x, r.y, w, r.height - 1), t.font.size);
            canvas.text(x.saturating_add(TITLE_SIDE), top, &m.title, t.text, t.font);
        }
    }

    fn look(&self, tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<BarState>();
        let at = |o: Option<usize>| o.map_or(0, |i| i as u64 + 1);
        let mut look = Look::new().u64(at(s.open)).u64(at(s.hover));
        for m in &self.menus {
            look = look.str(&m.title);
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
        let owner = tree.id;
        let font = shell.theme().font;
        let below = |x: i32| Point::new(x, node.rect.y.saturating_add(node.rect.height));
        let s = tree.state_mut::<BarState>();
        match event {
            Event::Pointer(p) => {
                let over = if p.kind == PointerKind::Left {
                    None
                } else {
                    self.title_at(font, node.rect, p.at)
                };
                let hover = over.map(|(i, _, _)| i);
                if hover != s.hover {
                    s.hover = hover;
                    shell.redraw();
                }
                let Some((i, x, _)) = over else {
                    return Status::Ignored;
                };
                if p.pressed(MouseButton::Left).is_some() {
                    let again = s.closed.is_some_and(|(at, m)| {
                        m == i && shell.now_ms().saturating_sub(at) < REPRESS_MS
                    });
                    s.closed = None;
                    if s.open == Some(i) && s.pending.is_none() {
                        shell.close_popup();
                    } else if !again {
                        open_menu(s, shell, owner, below(x), i, false);
                    }
                    return Status::Captured;
                }
                // With a menu open, the pointer moving onto another title opens that one. The
                // display server lets a popup be made only in answer to a press, and the core
                // lends us the last press's serial for a moment: a move that has none (the
                // press was too long ago) can't open anything.
                if p.kind == PointerKind::Moved
                    && s.open.is_some()
                    && s.pending.is_none()
                    && s.open != Some(i)
                    && shell.serial() != 0
                {
                    open_menu(s, shell, owner, below(x), i, false);
                }
                Status::Ignored
            }
            Event::MenuKey if !self.menus.is_empty() => {
                open_menu(s, shell, owner, below(self.title_x(font, node.rect, 0)), 0, true);
                Status::Captured
            }
            Event::PopupStep(step) => {
                let Some(current) = s.pending.or(s.open) else {
                    return Status::Ignored;
                };
                let n = self.menus.len() as i64;
                if n == 0 {
                    return Status::Ignored;
                }
                let next = (current as i64 + i64::from(*step)).rem_euclid(n) as usize;
                open_menu(s, shell, owner, below(self.title_x(font, node.rect, next)), next, true);
                Status::Captured
            }
            Event::PopupClosed => {
                let was = s.open;
                s.open = s.pending.take();
                // Only a close that left the pointer on its title can be a press that is about
                // to land on it (a row chosen leaves the pointer in the popup).
                if let (Some(m), None) = (was, s.open)
                    && s.hover == Some(m)
                {
                    s.closed = Some((shell.now_ms(), m));
                }
                if was.is_some() {
                    shell.redraw();
                    Status::Captured
                } else {
                    Status::Ignored
                }
            }
            _ => Status::Ignored,
        }
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        let titles: Vec<&str> = self.menus.iter().map(|m| m.title.as_str()).collect();
        Some(Describe {
            role: "menu bar",
            label: titles.join("|"),
            enabled: true,
            ..Describe::default()
        })
    }

    fn popup<'b>(&'b self, tree: &Tree) -> Option<Element<'b, M>>
    where
        'a: 'b,
        M: 'b,
    {
        let s = tree.state::<BarState>();
        let menu = self.menus.get(s.pending.or(s.open)?)?;
        Some(Element::new(MenuList { items: &menu.items, first: s.first }))
    }

    fn shortcuts(&self, out: &mut Vec<(Shortcut, M)>) {
        for it in self.menus.iter().flat_map(|m| &m.items) {
            if let (true, Some(shortcut), Some(message)) = (it.usable(), it.shortcut, &it.message) {
                out.push((shortcut, message.clone()));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ContextMenu
// ---------------------------------------------------------------------------------------------

/// What a context menu remembers: whether its popup was opened with the keyboard.
#[derive(Default)]
struct ContextState {
    first: bool,
}

/// One child with a menu that opens on a right click (or Shift+F10 while the focus is inside).
/// It is laid out as its child is.
pub struct ContextMenu<'a, M> {
    child: [Element<'a, M>; 1],
    items: Vec<MenuItem<M>>,
}

impl<'a, M: 'a> ContextMenu<'a, M> {
    /// `child`, with a menu of `items` on a right click.
    pub fn new(child: impl Into<Element<'a, M>>, items: Vec<MenuItem<M>>) -> ContextMenu<'a, M> {
        ContextMenu { child: [child.into()], items }
    }
}

impl<'a, M: Clone + 'a> From<ContextMenu<'a, M>> for Element<'a, M> {
    fn from(w: ContextMenu<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: Clone + 'a> Widget<'a, M> for ContextMenu<'a, M> {
    fn size(&self) -> (Length, Length) {
        self.child[0].widget.size()
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<ContextState>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(ContextState::default())
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.child
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let (w, h) = self.size();
        let limits = limits.width(w).height(h);
        layout::single(
            limits,
            Padding::ZERO,
            (Align::Start, Align::Start),
            &self.child[0],
            &mut tree.children[0],
            cx,
        )
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        // The child first, so that a list selects the row under the pointer before the menu
        // that acts on it opens.
        let used = crate::element::children_event(&self.child, tree, node, event, shell);
        let owner = tree.id;
        match event {
            Event::Pointer(p)
                if p.pressed(MouseButton::Right).is_some()
                    && node.rect.contains(p.at.x, p.at.y) =>
            {
                tree.state_mut::<ContextState>().first = false;
                shell.open_popup(owner, p.at);
                Status::Captured
            }
            // There is no context-menu key in `pbr.input`, so Shift+F10, from anywhere inside.
            Event::Key(k)
                if used == Status::Ignored
                    && k.key == Key::Function(10)
                    && k.modifiers.for_shortcuts().0 == crate::event::Modifiers::SHIFT
                    && shell.focused().is_some_and(|f| tree.contains(f)) =>
            {
                tree.state_mut::<ContextState>().first = true;
                let at = node.children.first().map_or(node.rect, |c| c.rect);
                shell.open_popup(owner, Point::new(at.x, at.y));
                Status::Captured
            }
            _ => used,
        }
    }

    fn popup<'b>(&'b self, tree: &Tree) -> Option<Element<'b, M>>
    where
        'a: 'b,
        M: 'b,
    {
        let first = tree.state::<ContextState>().first;
        Some(Element::new(MenuList { items: &self.items, first }))
    }
}

// ---------------------------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------------------------

/// A toolbar's height: 1.3's `NP_TOOL`.
const TOOLBAR_HEIGHT: i32 = 32;
/// Room at each end of a toolbar, and between its children.
const TOOLBAR_SIDE: i32 = 6;
const TOOLBAR_SPACING: i32 = 4;
/// How tall a separator line is.
const TOOL_SEPARATOR_HEIGHT: i32 = 18;

/// A row of tools across a window: buttons, a separator or two, and perhaps something at the
/// right end (Notepad's "Ln 3, Col 7").
pub struct Toolbar<'a, M> {
    children: Vec<Element<'a, M>>,
    /// Where the stretch that pushes the right-hand children to the far end is, once there is
    /// one.
    stretch: Option<usize>,
}

impl<'a, M: 'a> Toolbar<'a, M> {
    /// An empty toolbar.
    pub fn new() -> Toolbar<'a, M> {
        Toolbar { children: Vec::new(), stretch: None }
    }

    /// Adds a child at the left end's side, after those already there.
    pub fn push(mut self, child: impl Into<Element<'a, M>>) -> Toolbar<'a, M> {
        let child = child.into();
        match self.stretch {
            // Before the stretch, so that it stays on the left.
            Some(at) => {
                self.children.insert(at, child);
                self.stretch = Some(at + 1);
            }
            None => self.children.push(child),
        }
        self
    }

    /// Adds a thin line between groups of tools.
    pub fn separator(self) -> Toolbar<'a, M> {
        self.push(Element::new(ToolSeparator))
    }

    /// Adds a child at the right end.
    pub fn push_right(mut self, child: impl Into<Element<'a, M>>) -> Toolbar<'a, M> {
        if self.stretch.is_none() {
            self.stretch = Some(self.children.len());
            self.children.push(crate::widget::Stretch::new().into());
        }
        self.children.push(child.into());
        self
    }
}

impl<'a, M: 'a> Default for Toolbar<'a, M> {
    fn default() -> Toolbar<'a, M> {
        Toolbar::new()
    }
}

impl<'a, M: 'a> From<Toolbar<'a, M>> for Element<'a, M> {
    fn from(w: Toolbar<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// The thin line between groups of tools. (Spacing on either side comes from the toolbar's.)
struct ToolSeparator;

impl<'a, M> Widget<'a, M> for ToolSeparator {
    fn size(&self) -> (Length, Length) {
        (Length::Fixed(1), Length::Fixed(TOOL_SEPARATOR_HEIGHT))
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        Node::new(limits.resolve(Size::new(1, TOOL_SEPARATOR_HEIGHT)))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        canvas.fill_rect(node.rect, cx.theme.bar_edge);
    }

    fn look(&self, _tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        Some(Look::new().u64(u64::from(cx.theme.bar_edge.0)).finish())
    }
}

impl<'a, M: 'a> Widget<'a, M> for Toolbar<'a, M> {
    fn size(&self) -> (Length, Length) {
        (Length::Fill, Length::Fixed(TOOLBAR_HEIGHT))
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.children
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        // The bottom pixel row is the edge line, so the children are centred above it.
        let flex = Flex {
            axis: Axis::Across,
            padding: Padding { top: 0, right: TOOLBAR_SIDE, bottom: 1, left: TOOLBAR_SIDE },
            spacing: TOOLBAR_SPACING,
            align: Align::Center,
        };
        let (w, h) = self.size();
        layout::flex(flex, limits.width(w).height(h), &self.children, &mut tree.children, cx)
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let r = node.rect;
        canvas.fill_rect(r, cx.theme.bar);
        canvas.fill_rect(Rect::new(r.x, r.y + r.height - 1, r.width, 1), cx.theme.bar_edge);
        crate::element::draw_children(&self.children, tree, node, canvas, cx);
    }

    fn look(&self, _tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        Some(
            Look::new().u64(u64::from(cx.theme.bar.0)).u64(u64::from(cx.theme.bar_edge.0)).finish(),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// StatusBar
// ---------------------------------------------------------------------------------------------

/// A status bar's height, the room at its ends, and between its texts.
const STATUS_HEIGHT: i32 = 20;
const STATUS_SIDE: i32 = 8;
const STATUS_GAP: i32 = 16;

/// A strip along the bottom of a window with a few short texts: some at the left, some at the
/// right.
pub struct StatusBar {
    left: Vec<String>,
    right: Vec<String>,
}

impl StatusBar {
    /// An empty status bar.
    pub fn new() -> StatusBar {
        StatusBar { left: Vec::new(), right: Vec::new() }
    }

    /// Adds a text after those at the left.
    pub fn push(mut self, text: &str) -> StatusBar {
        self.left.push(text.to_string());
        self
    }

    /// Adds a text after those at the right (which are shown in the order they are given,
    /// ending at the right edge).
    pub fn push_right(mut self, text: &str) -> StatusBar {
        self.right.push(text.to_string());
        self
    }

    /// How wide `texts` are side by side with the gaps between them.
    fn run(font: TextStyle, texts: &[String]) -> i64 {
        let widths: i64 = texts.iter().map(|t| i64::from(font.width(t))).sum();
        widths + i64::from(STATUS_GAP) * texts.len().saturating_sub(1) as i64
    }
}

impl Default for StatusBar {
    fn default() -> StatusBar {
        StatusBar::new()
    }
}

impl<'a, M: 'a> From<StatusBar> for Element<'a, M> {
    fn from(w: StatusBar) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M> Widget<'a, M> for StatusBar {
    fn size(&self) -> (Length, Length) {
        (Length::Fill, Length::Fixed(STATUS_HEIGHT))
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(Length::Fill).height(Length::Fixed(STATUS_HEIGHT));
        let (left, right) =
            (Self::run(cx.theme.font, &self.left), Self::run(cx.theme.font, &self.right));
        let between = if left > 0 && right > 0 { i64::from(STATUS_GAP) } else { 0 };
        let wanted = (left + right + between + 2 * i64::from(STATUS_SIDE)).min(i64::from(i32::MAX));
        Node::new(limits.resolve(Size::new(wanted as i32, STATUS_HEIGHT)))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let t = cx.theme;
        let r = node.rect;
        canvas.fill_rect(r, t.bar);
        canvas.fill_rect(Rect::new(r.x, r.y, r.width, 1), t.bar_edge);
        let top = super::text_top(Rect::new(r.x, r.y + 1, r.width, r.height - 1), t.font.size);
        let mut x = i64::from(r.x) + i64::from(STATUS_SIDE);
        for text in &self.left {
            canvas.text(x.clamp(-1 << 30, 1 << 30) as i32, top, text, t.text_dim, t.font);
            x += i64::from(t.font.width(text)) + i64::from(STATUS_GAP);
        }
        // The right-hand texts end 8 pixels from the right edge, but never run into the left
        // ones.
        let wanted = i64::from(r.x) + i64::from(r.width)
            - i64::from(STATUS_SIDE)
            - Self::run(t.font, &self.right);
        let mut x = wanted.max(x);
        for text in &self.right {
            canvas.text(x.clamp(-1 << 30, 1 << 30) as i32, top, text, t.text_dim, t.font);
            x += i64::from(t.font.width(text)) + i64::from(STATUS_GAP);
        }
    }

    fn look(&self, _tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let mut look = Look::new().u64(u64::from(cx.theme.bar.0)).u64(self.left.len() as u64);
        for text in self.left.iter().chain(&self.right) {
            look = look.str(text);
        }
        Some(look.finish())
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        let all: Vec<&str> = self.left.iter().chain(&self.right).map(String::as_str).collect();
        Some(Describe {
            role: "status bar",
            label: all.join("|"),
            enabled: true,
            ..Describe::default()
        })
    }
}
