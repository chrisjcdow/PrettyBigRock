// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Elements and widgets: what an app's `view` returns, and what every widget must be able to do
//! (note 0023, section 5).
//!
//! An app describes its window afresh after every change as a tree of [`Element`]s. Each
//! element holds one *widget* (a button, a text field, a row of other elements) as a value
//! implementing the [`Widget`] trait. A *trait* is Rust's word for a set of methods a type
//! promises to have (like an interface in Java or C#); `Box<dyn Widget>` is a pointer to some
//! value of a type implementing it, chosen while the program runs, which is how one tree can
//! hold buttons and text fields side by side.
//!
//! Elements are thrown away after each use. What a widget must remember between views (where
//! a text field's cursor is, how far a list is scrolled) is kept by sandstone in a [`Tree`]
//! of states that parallels the elements: after each new view, [`Tree::diff_children`] matches
//! each new element with the state of the element in the same place before (or with the same
//! id), and only makes a new state for elements that are new. That is how a view can be built
//! from nothing every time and still feel continuous to the user.

use crate::event::{Event, Status};
use crate::geometry::{Limits, Point, Rect};
use crate::layout::Node;
use crate::shell::Shell;
use crate::shortcut::Shortcut;
use crate::theme::Theme;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::any::{Any, TypeId};
use core::sync::atomic::{AtomicU64, Ordering};
use pbr_draw::Canvas;
use pbr_window::Cursor;

/// One part of a view: a widget, and the id the app gave it, if any.
///
/// `'a` is a *lifetime*: how long the things an element borrows from the app's state live.
/// An element may borrow (a text editor shows the app's text without copying it), so it
/// can't outlive the state it was made from; Rust checks that at compile time. `M` is the
/// app's message type, what its widgets send when the user does something.
pub struct Element<'a, M> {
    pub(crate) widget: Box<dyn Widget<'a, M> + 'a>,
    pub(crate) key: Option<String>,
}

impl<'a, M: 'a> Element<'a, M> {
    /// An element holding `widget`.
    pub fn new(widget: impl Widget<'a, M> + 'a) -> Element<'a, M> {
        Element { widget: Box::new(widget), key: None }
    }

    /// The same element with an id: the app can then move the focus to it
    /// ([`Context::focus`](crate::Context::focus)), tests can find it, and its state follows
    /// it even when elements before it come and go.
    pub fn with_id(mut self, id: &str) -> Element<'a, M> {
        self.key = Some(id.to_string());
        self
    }

    /// The same element, sending `f(message)` wherever it would have sent `message`. This is
    /// how a part of a view written with its own message type is put inside another's, and
    /// how sandstone puts an app's views beside its own dialogs.
    pub fn map<B: 'a>(self, f: impl Fn(M) -> B + 'a) -> Element<'a, B> {
        let key = self.key.clone();
        Element { widget: Box::new(Map { inner: self, f: Rc::new(f) }), key }
    }
}

impl<'a, M> Element<'a, M> {
    /// The widget inside.
    pub fn widget(&self) -> &(dyn Widget<'a, M> + 'a) {
        &*self.widget
    }

    /// The id the app gave it.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// A new state tree for this element and everything inside it.
    pub fn tree(&self) -> Tree {
        let widget = &self.widget;
        let mut tree = Tree::new(widget.kind(), widget.tag(), widget.state(), self.key.clone());
        self.widget.diff(&mut tree);
        tree
    }
}

/// Gives every widget type `.id("name")`, turning it into an [`Element`] with that id.
///
/// This is a *blanket implementation*: one `impl` for every type that can become an element,
/// so no widget has to write it.
pub trait Identify<'a, M>: Into<Element<'a, M>> {
    /// This widget as an element with an id (see [`Element::with_id`]).
    fn id(self, id: &str) -> Element<'a, M>;
}

impl<'a, M: 'a, T: Into<Element<'a, M>>> Identify<'a, M> for T {
    fn id(self, id: &str) -> Element<'a, M> {
        self.into().with_id(id)
    }
}

/// What a widget says about itself, for tests (and, later, a screen reader): the harness
/// finds "the button labelled Save" through this, and the gallery writes it to the log for
/// the desktop boot's runner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Describe {
    /// What kind of thing it is: `"button"`, `"check box"`, `"text field"`...
    pub role: &'static str,
    /// Its label, as the user reads it.
    pub label: String,
    /// What it holds (a field's text, a list's selected row), if anything.
    pub value: String,
    /// It can be used (not greyed out).
    pub enabled: bool,
    /// For check boxes and toggle buttons: whether it is on.
    pub checked: Option<bool>,
}

/// A widget described, with where it is: what [`Widget::describe_all`] collects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Description {
    /// What the widget said.
    pub what: Describe,
    /// Where it is in the window, in logical pixels.
    pub rect: Rect,
    /// Its state's number (what the focus is kept as).
    pub id: u64,
    /// The id the app gave it.
    pub key: Option<String>,
    /// It has the focus.
    pub focused: bool,
}

impl Description {
    /// The description as one line of text: `button "Save" at 10,20 size 64x24 focused`.
    /// The gallery writes these to the log, where the desktop boot's runner reads them.
    pub fn line(&self) -> String {
        let r = self.rect;
        let mut s = alloc::format!(
            "{} \"{}\" at {},{} size {}x{}",
            self.what.role,
            self.what.label.escape_debug(),
            r.x,
            r.y,
            r.width,
            r.height
        );
        if let Some(key) = &self.key {
            s.push_str(&alloc::format!(" id {key}"));
        }
        if self.focused {
            s.push_str(" focused");
        }
        if !self.what.enabled {
            s.push_str(" disabled");
        }
        match self.what.checked {
            Some(true) => s.push_str(" checked"),
            Some(false) => s.push_str(" unchecked"),
            None => {}
        }
        if !self.what.value.is_empty() {
            s.push_str(&alloc::format!(" value \"{}\"", self.what.value.escape_debug()));
        }
        s
    }
}

/// A small hash of everything that decides a widget's pixels, so that sandstone can tell what
/// changed between two views without drawing them (FNV-1a, 64 bits: simple and good enough
/// to tell one look from another).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look(u64);

impl Default for Look {
    fn default() -> Look {
        Look::new()
    }
}

impl Look {
    /// An empty look.
    pub const fn new() -> Look {
        Look(0xcbf2_9ce4_8422_2325)
    }

    /// Adds bytes.
    pub fn bytes(mut self, bytes: &[u8]) -> Look {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    /// Adds a number.
    pub fn u64(self, n: u64) -> Look {
        self.bytes(&n.to_le_bytes())
    }

    /// Adds a number.
    pub fn i32(self, n: i32) -> Look {
        self.bytes(&n.to_le_bytes())
    }

    /// Adds a yes or no.
    pub fn bool(self, b: bool) -> Look {
        self.bytes(&[u8::from(b)])
    }

    /// Adds a string (with its length, so that `"ab" + "c"` differs from `"a" + "bc"`).
    pub fn str(self, s: &str) -> Look {
        self.u64(s.len() as u64).bytes(s.as_bytes())
    }

    /// Adds a rectangle.
    pub fn rect(self, r: Rect) -> Look {
        self.i32(r.x).i32(r.y).i32(r.width).i32(r.height)
    }

    /// The hash.
    pub fn finish(self) -> u64 {
        self.0
    }
}

/// What layout needs to know besides the limits.
pub struct LayoutCx<'t> {
    /// The look: its font and spacing decide sizes.
    pub theme: &'t Theme,
}

/// What drawing needs to know besides the widget and its place.
pub struct DrawCx<'t> {
    /// The look.
    pub theme: &'t Theme,
    /// The state number of the widget with the focus in this window, if any.
    pub focus: Option<u64>,
    /// The window has the keyboard (a focused text field shows its cursor only then).
    pub window_focused: bool,
    /// The part of the window being drawn, in logical pixels. A widget with many parts (a
    /// long list) may skip what is outside it; nothing outside it is drawn anyway.
    pub clip: Rect,
}

impl DrawCx<'_> {
    /// The widget whose state is `tree` has the focus of its window.
    pub fn is_focused(&self, tree: &Tree) -> bool {
        self.focus == Some(tree.id)
    }

    /// The widget has the focus and its window has the keyboard: keys typed now go to it.
    pub fn has_keyboard(&self, tree: &Tree) -> bool {
        self.window_focused && self.is_focused(tree)
    }
}

/// What every widget can do. Most methods have a default that suits a widget without
/// children, or (through [`Widget::children`]) a container that only arranges its children;
/// a widget writes only what it needs.
///
/// The rules every widget keeps, which the rest of sandstone relies on:
/// - **It draws only inside its own rectangle** (sandstone also clips it there). That is what
///   lets sandstone redraw only what changed.
/// - **[`Widget::look`] covers everything its own pixels depend on**: its text, its colours,
///   its state, whether it has the focus. Two views whose looks are the same must draw the
///   same. (The harness checks this after every step: see `sandstone::testing`.)
/// - **Nothing it is given makes it panic**: sizes may be zero, text may be empty or huge.
pub trait Widget<'a, M> {
    /// How much room it wants across and down.
    fn size(&self) -> (crate::Length, crate::Length) {
        (crate::Length::Shrink, crate::Length::Shrink)
    }

    /// What kind of widget it is: a state is only matched with a widget of the same kind. The
    /// default, the name of its type, is right for every widget except one that wraps another
    /// invisibly.
    fn kind(&self) -> &'static str {
        core::any::type_name::<Self>()
    }

    /// The type of its state, so that a state is only given to a widget that can use it.
    /// `TypeId` is a number the compiler gives each type.
    fn tag(&self) -> TypeId {
        TypeId::of::<()>()
    }

    /// A new state, for an element seen for the first time. `Box<dyn Any>` holds a value of
    /// any type, which [`Tree::state`] gets back by asking for the type.
    fn state(&self) -> Box<dyn Any> {
        Box::new(())
    }

    /// The elements inside it, for containers.
    fn children(&self) -> &[Element<'a, M>] {
        &[]
    }

    /// Brings the state tree in line with this widget after a new view: matches the
    /// children's states.
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(self.children());
    }

    /// Works out its size within `limits`, and where its children go. The node's rectangle
    /// and its children's are relative to the node's own corner while layout runs; sandstone
    /// makes them relative to the window afterwards.
    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node;

    /// Draws it. `canvas` is the window's (positions are the window's), clipped to the
    /// widget's rectangle and to what is being redrawn.
    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        draw_children(self.children(), tree, node, canvas, cx);
    }

    /// A hash of everything its own pixels depend on (not its children's), or `None` if it
    /// draws nothing of its own. Sandstone adds its rectangle and the window's scale.
    fn look(&self, _tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        None
    }

    /// Handles an input event (given to every widget; each decides whether it is for it), and
    /// says whether it used it, which stops it going further.
    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        children_event(self.children(), tree, node, event, shell)
    }

    /// It can take the focus (Tab stops at it).
    fn focusable(&self, _tree: &Tree) -> bool {
        false
    }

    /// The pointer's shape over it.
    fn cursor(&self, _tree: &Tree, _node: &Node, _at: Point) -> Cursor {
        Cursor::Arrow
    }

    /// What it is, for tests: `None` for what only arranges others.
    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        None
    }

    /// The contents of the popup it has open (a menu's items), if this is the widget that
    /// opened it (see [`Shell::open_popup`]). The element may borrow the widget: the
    /// lifetime `'b` is that borrow's.
    fn popup<'b>(&'b self, _tree: &Tree) -> Option<Element<'b, M>>
    where
        'a: 'b,
        M: 'b,
    {
        None
    }

    /// The shortcuts it offers (a menu's items), with what each sends.
    fn shortcuts(&self, _out: &mut Vec<(Shortcut, M)>) {}

    // ---- Walking the tree. ----
    //
    // Each of these visits the widget and then its children; containers get them for free
    // through `children`. A widget that wraps another without `children` (one changing the
    // message type, or scrolling) writes them to pass the walk on.

    /// Collects the focusable widgets' state numbers, in Tab order.
    fn focus_order(&self, tree: &Tree, out: &mut Vec<u64>) {
        if self.focusable(tree) {
            out.push(tree.id);
        }
        for (child, t) in self.children().iter().zip(&tree.children) {
            child.widget.focus_order(t, out);
        }
    }

    /// Collects each drawing widget's state number, its rectangle within `clip`, and its look.
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
        for ((child, t), n) in self.children().iter().zip(&tree.children).zip(&node.children) {
            child.widget.looks(t, n, visible, cx, out);
        }
    }

    /// Collects every widget's description, in tree order.
    fn describe_all(
        &self,
        tree: &Tree,
        node: &Node,
        focus: Option<u64>,
        out: &mut Vec<Description>,
    ) {
        if let Some(what) = self.describe(tree, node) {
            out.push(Description {
                what,
                rect: node.rect,
                id: tree.id,
                key: tree.key.clone(),
                focused: focus == Some(tree.id),
            });
        }
        for ((child, t), n) in self.children().iter().zip(&tree.children).zip(&node.children) {
            child.widget.describe_all(t, n, focus, out);
        }
    }

    /// The pointer's shape at `at`: the topmost widget there decides.
    fn cursor_at(&self, tree: &Tree, node: &Node, at: Point) -> Cursor {
        let children = self.children().iter().zip(&tree.children).zip(&node.children);
        // `rev`: later children are drawn over earlier ones.
        for ((child, t), n) in children.rev() {
            if n.rect.contains(at.x, at.y) {
                return child.widget.cursor_at(t, n, at);
            }
        }
        self.cursor(tree, node, at)
    }

    /// The popup contents of the widget whose state number is `owner`, if it is here.
    fn find_popup<'b>(&'b self, tree: &Tree, owner: u64) -> Option<Element<'b, M>>
    where
        'a: 'b,
        M: 'b,
    {
        if tree.id == owner {
            return self.popup(tree);
        }
        for (child, t) in self.children().iter().zip(&tree.children) {
            if let Some(e) = child.widget.find_popup(t, owner) {
                return Some(e);
            }
        }
        None
    }

    /// Collects the shortcuts of every widget.
    fn all_shortcuts(&self, out: &mut Vec<(Shortcut, M)>) {
        self.shortcuts(out);
        for child in self.children() {
            child.widget.all_shortcuts(out);
        }
    }
}

/// Draws `children` (their nodes are `node`'s children), each clipped to its own rectangle,
/// skipping those outside what is being drawn. Containers use it.
pub fn draw_children<'a, M>(
    children: &[Element<'a, M>],
    tree: &Tree,
    node: &Node,
    canvas: &mut Canvas<'_>,
    cx: &DrawCx<'_>,
) {
    for ((child, t), n) in children.iter().zip(&tree.children).zip(&node.children) {
        if n.rect.intersect(&cx.clip).is_empty() {
            continue;
        }
        let mut c = canvas.clipped(n.rect);
        child.widget.draw(t, n, &mut c, cx);
    }
}

/// Gives `event` to `children` in turn (the last drawn, which is on top, first), until one
/// uses it. Containers use it.
pub fn children_event<'a, M>(
    children: &[Element<'a, M>],
    tree: &mut Tree,
    node: &Node,
    event: &Event,
    shell: &mut Shell<'_, M>,
) -> Status {
    let mut status = Status::Ignored;
    let all = children.iter().zip(tree.children.iter_mut()).zip(&node.children);
    for ((child, t), n) in all.rev() {
        if child.widget.on_event(t, n, event, shell) == Status::Captured {
            status = Status::Captured;
            break;
        }
    }
    status
}

/// Where state numbers come from: each new state gets the next, so a number names one
/// widget for as long as its state lives, and is never reused. An atomic counter (one that
/// threads can share safely) because a `static` must be safe to share, though only one thread
/// draws.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A widget's state, and its children's: the part of the window that lasts between views.
pub struct Tree {
    /// The state's number, unique for the life of the program.
    pub id: u64,
    /// The id the app gave the element, if any.
    pub key: Option<String>,
    kind: &'static str,
    tag: TypeId,
    state: Box<dyn Any>,
    /// The children's states, in the order of the widget's children.
    pub children: Vec<Tree>,
}

impl Tree {
    fn new(kind: &'static str, tag: TypeId, state: Box<dyn Any>, key: Option<String>) -> Tree {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Tree { id, key, kind, tag, state, children: Vec::new() }
    }

    /// The state, as the type the widget made it. Asking for another type is a bug in the
    /// widget, and panics (with the widget's own `tag` and `state` this can't happen).
    pub fn state<T: 'static>(&self) -> &T {
        match self.state.downcast_ref::<T>() {
            Some(s) => s,
            None => panic!("sandstone: a widget asked for a state of the wrong type"),
        }
    }

    /// The state, to change it.
    pub fn state_mut<T: 'static>(&mut self) -> &mut T {
        match self.state.downcast_mut::<T>() {
            Some(s) => s,
            None => panic!("sandstone: a widget asked for a state of the wrong type"),
        }
    }

    /// Matches `children`'s states with these children's: an element with an id takes the
    /// state that had the same id (wherever it was among its siblings); one without takes the
    /// next state of the same kind, without an id, that hasn't been taken, looking from just
    /// after the last one taken. So an element put in (or taken out) before others leaves
    /// theirs alone. Every other element gets a new state, and states nobody took are dropped.
    pub fn diff_children<'a, M>(&mut self, children: &[Element<'a, M>]) {
        // `Option` slots, so that a state can be taken out of the old list exactly once.
        let mut old: Vec<Option<Tree>> =
            core::mem::take(&mut self.children).into_iter().map(Some).collect();
        let mut new = Vec::with_capacity(children.len());
        let mut next = 0;
        for child in children {
            let (kind, tag) = (child.widget.kind(), child.widget.tag());
            let same = |t: &Tree| t.kind == kind && t.tag == tag;
            let found = match &child.key {
                Some(key) => old.iter().position(|t| {
                    t.as_ref().is_some_and(|t| same(t) && t.key.as_ref() == Some(key))
                }),
                None => {
                    let at = old
                        .iter()
                        .skip(next)
                        .position(|t| t.as_ref().is_some_and(|t| same(t) && t.key.is_none()));
                    at.map(|k| {
                        next += k + 1;
                        next - 1
                    })
                }
            };
            match found.and_then(|n| old[n].take()) {
                Some(mut tree) => {
                    child.widget.diff(&mut tree);
                    new.push(tree);
                }
                None => new.push(child.tree()),
            }
        }
        self.children = new;
    }

    /// The state number of the element with id `key`, here or inside.
    pub fn find_key(&self, key: &str) -> Option<u64> {
        if self.key.as_deref() == Some(key) {
            return Some(self.id);
        }
        self.children.iter().find_map(|c| c.find_key(key))
    }

    /// Whether this state was made for a widget of `kind` with state type `tag`.
    pub(crate) fn made_for(&self, kind: &'static str, tag: TypeId) -> bool {
        self.kind == kind && self.tag == tag
    }

    /// Whether a state numbered `id` is here or inside.
    pub fn contains(&self, id: u64) -> bool {
        self.id == id || self.children.iter().any(|c| c.contains(id))
    }
}

/// An element whose messages are changed on the way out ([`Element::map`]). It shares its
/// state with the element inside (same tag, same tree), so it is invisible to the tree.
struct Map<'a, A, B> {
    inner: Element<'a, A>,
    /// `Rc` (a pointer that counts its owners) so that a popup's contents, mapped the same
    /// way, can share the function.
    f: Rc<dyn Fn(A) -> B + 'a>,
}

impl<'a, A: 'a, B: 'a> Widget<'a, B> for Map<'a, A, B> {
    fn kind(&self) -> &'static str {
        self.inner.widget.kind()
    }

    fn size(&self) -> (crate::Length, crate::Length) {
        self.inner.widget.size()
    }

    fn tag(&self) -> TypeId {
        self.inner.widget.tag()
    }

    fn state(&self) -> Box<dyn Any> {
        self.inner.widget.state()
    }

    fn diff(&self, tree: &mut Tree) {
        self.inner.widget.diff(tree);
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        self.inner.widget.layout(tree, limits, cx)
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        self.inner.widget.draw(tree, node, canvas, cx);
    }

    fn look(&self, tree: &Tree, node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        self.inner.widget.look(tree, node, cx)
    }

    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, B>,
    ) -> Status {
        let mut local = Vec::new();
        let status = {
            let mut inner = shell.with_messages(&mut local);
            self.inner.widget.on_event(tree, node, event, &mut inner)
        };
        for m in local {
            shell.publish((self.f)(m));
        }
        status
    }

    fn focusable(&self, tree: &Tree) -> bool {
        self.inner.widget.focusable(tree)
    }

    fn cursor(&self, tree: &Tree, node: &Node, at: Point) -> Cursor {
        self.inner.widget.cursor(tree, node, at)
    }

    fn describe(&self, tree: &Tree, node: &Node) -> Option<Describe> {
        self.inner.widget.describe(tree, node)
    }

    fn focus_order(&self, tree: &Tree, out: &mut Vec<u64>) {
        self.inner.widget.focus_order(tree, out);
    }

    fn looks(
        &self,
        tree: &Tree,
        node: &Node,
        clip: Rect,
        cx: &DrawCx<'_>,
        out: &mut Vec<(u64, Rect, u64)>,
    ) {
        self.inner.widget.looks(tree, node, clip, cx, out);
    }

    fn describe_all(
        &self,
        tree: &Tree,
        node: &Node,
        focus: Option<u64>,
        out: &mut Vec<Description>,
    ) {
        self.inner.widget.describe_all(tree, node, focus, out);
    }

    fn cursor_at(&self, tree: &Tree, node: &Node, at: Point) -> Cursor {
        self.inner.widget.cursor_at(tree, node, at)
    }

    fn find_popup<'b>(&'b self, tree: &Tree, owner: u64) -> Option<Element<'b, B>>
    where
        'a: 'b,
        B: 'b,
    {
        let found = self.inner.widget.find_popup(tree, owner)?;
        let f = Rc::clone(&self.f);
        Some(Element { key: found.key.clone(), widget: Box::new(Map { inner: found, f }) })
    }

    fn all_shortcuts(&self, out: &mut Vec<(Shortcut, B)>) {
        let mut inner = Vec::new();
        self.inner.widget.all_shortcuts(&mut inner);
        out.extend(inner.into_iter().map(|(s, m)| (s, (self.f)(m))));
    }
}
