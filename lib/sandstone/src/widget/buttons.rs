// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Icon`], [`IconButton`], [`ToggleButton`] and [`CheckBox`]: small pictures, and the
//! buttons of toolbars, Paint's tools and settings dialogs (note 0023, section 5, the table).
//!
//! They follow `button.rs` (read it first): a builder the app makes in its `view`, a small
//! state that sandstone keeps between views (is the pointer over it, is it held down), and the
//! [`Widget`] methods. The face of a button is drawn by `draw_face`, shared with `Button`, so
//! every kind of button looks like one family.
//!
//! The pictures are [`Image`]s: the app's own `0xAARRGGBB` pixels, borrowed for as long as the
//! view lasts (that is the `'a` in `Icon<'a>`: a *lifetime*, which tells the compiler the icon
//! may not outlive the pixels it shows).

use super::{draw_face, fit, text_top};
use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::event::{Button as MouseButton, Event, Key, Pointer, PointerKind, Status};
use crate::geometry::{Length, Limits, Rect, Size};
use crate::layout::Node;
use crate::shell::Shell;
use crate::{Image, Look, Theme};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use core::any::{Any, TypeId};
use pbr_draw::Canvas;

/// How big a toolbar button is when it shows only a 16 x 16 icon.
const TOOL: i32 = 26;
/// Room between a picture and the text after it.
const GAP: i32 = 4;
/// Room between an icon button's edge and what is in it, when it has a label.
const TOOL_SIDE: i32 = 6;
/// How much wider than its content a toggle button is, on each side (as `Button`).
const SIDE: i32 = 12;
/// The narrowest a toggle button with a label is (as `Button`).
const MIN_WIDTH: i32 = 64;
/// The least room kept on each side of what is inside a button, when it is squeezed.
const SQUEEZED: i32 = 2;
/// A check box's box: its side, the room after it, and its corner radius.
const BOX: i32 = 14;
const BOX_GAP: i32 = 6;
const BOX_RADIUS: i32 = 2;
/// No picture is taken as bigger than this, so that sums with it cannot overflow.
const IMAGE_MAX: u32 = 1 << 12;

/// What these widgets remember between views (all except `Icon`, which remembers nothing).
#[derive(Default)]
pub(crate) struct Hold {
    /// The pointer is over it.
    hovered: bool,
    /// The left button went down on it and hasn't come up.
    pressed: bool,
}

/// What a pointer event did to a button.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointed {
    /// Nothing to do with it.
    Ignored,
    /// Used (a press, or a release that was not over it), but not a click.
    Used,
    /// The button went down over it, and came up over it too: a click.
    Clicked,
}

/// Tracks the pointer for a button in `rect`, as `Button` does: hovering, pressing, and a
/// click only when the button is let go of over it. A button that can't be used still follows
/// the pointer (so it is right when it can be used again) but never presses.
fn pointer<M>(
    s: &mut Hold,
    rect: Rect,
    p: &Pointer,
    enabled: bool,
    shell: &mut Shell<'_, M>,
) -> Pointed {
    let inside = rect.contains(p.at.x, p.at.y) && p.kind != PointerKind::Left;
    if inside != s.hovered {
        s.hovered = inside;
        shell.redraw();
    }
    if !enabled {
        if s.pressed {
            s.pressed = false;
            shell.redraw();
        }
        return Pointed::Ignored;
    }
    if inside && p.pressed(MouseButton::Left).is_some() {
        s.pressed = true;
        shell.redraw();
        return Pointed::Used;
    }
    if s.pressed && p.released(MouseButton::Left) {
        s.pressed = false;
        shell.redraw();
        return if inside { Pointed::Clicked } else { Pointed::Used };
    }
    Pointed::Ignored
}

/// A picture's size in logical pixels, never more than `IMAGE_MAX` a side.
fn dims(image: &Image<'_>) -> (i32, i32) {
    (image.width().min(IMAGE_MAX) as i32, image.height().min(IMAGE_MAX) as i32)
}

/// Draws `image` in `dest`, at full strength or faded (for a button that can't be used): half
/// its alpha, so that it is still clear what it is, but quieter.
fn draw_image(canvas: &mut Canvas<'_>, image: &Image<'_>, dest: Rect, faded: bool) {
    if faded {
        image.draw_faded(canvas, dest, 128);
    } else {
        image.draw(canvas, dest);
    }
}

/// Draws an optional picture and an optional label side by side, the pair centred in `r` and
/// the label cut short ("…") if there isn't room. `shift` moves it all (1 for a held button).
fn draw_content(
    canvas: &mut Canvas<'_>,
    r: Rect,
    t: &Theme,
    image: Option<&Image<'_>>,
    label: &str,
    enabled: bool,
    shift: i32,
) {
    let (iw, ih) = image.map_or((0, 0), dims);
    let gap = if image.is_some() && !label.is_empty() { GAP } else { 0 };
    let room = r.width.saturating_sub(2 * SQUEEZED).saturating_sub(iw).saturating_sub(gap);
    let text = if label.is_empty() || room <= 0 { String::new() } else { fit(label, t.font, room) };
    let (iw, gap) = if text.is_empty() { (iw, 0) } else { (iw, gap) };
    let total = iw + gap + t.font.width(&text);
    let x = r.x + (r.width - total) / 2 + shift;
    if let Some(image) = image {
        let y = r.y + (r.height - ih) / 2 + shift;
        draw_image(canvas, image, Rect::new(x, y, iw, ih), !enabled);
    }
    if !text.is_empty() {
        let colour = if enabled { t.text } else { t.text_disabled };
        canvas.text(x + iw + gap, text_top(r, t.font.size) + shift, &text, colour, t.font);
    }
}

// ---------------------------------------------------------------- Icon

/// A small picture: an [`Image`] shown at its own size (16 x 16 for an icon) or at another
/// with [`Icon::size`]. It takes no events. A label can be given for tests, and later for a
/// screen reader; it is never drawn.
pub struct Icon<'a> {
    image: Image<'a>,
    size: Option<(i32, i32)>,
    label: String,
}

impl<'a> Icon<'a> {
    /// An icon showing `image`.
    pub fn new(image: Image<'a>) -> Icon<'a> {
        Icon { image, size: None, label: String::new() }
    }

    /// Draws it `width` by `height` logical pixels instead of at its own size.
    pub fn size(mut self, width: i32, height: i32) -> Icon<'a> {
        self.size = Some((width, height));
        self
    }

    /// What it is called, for tests and a screen reader (nothing is drawn for it).
    pub fn label(mut self, label: &str) -> Icon<'a> {
        self.label = label.to_string();
        self
    }
}

impl<'a, M: 'a> From<Icon<'a>> for Element<'a, M> {
    fn from(w: Icon<'a>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M> Widget<'a, M> for Icon<'a> {
    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        let (w, h) = self.size.unwrap_or_else(|| dims(&self.image));
        Node::new(limits.resolve(Size::new(w.max(0), h.max(0))))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, _cx: &DrawCx<'_>) {
        self.image.draw(canvas, node.rect);
    }

    fn look(&self, _tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        Some(self.image.look(Look::new()).finish())
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "icon",
            label: self.label.clone(),
            enabled: true,
            ..Describe::default()
        })
    }
}

// ---------------------------------------------------------------- IconButton

/// A small button showing a picture, and perhaps a short label after it, for toolbars. It is
/// flat (only the toolbar's paper) until the pointer is over it, then it shows a button's
/// face. It sends its message when let go of over it. With no message it is greyed out.
///
/// It **never takes the keyboard focus**: clicking a toolbar button must not take the focus
/// from the text being edited, and every toolbar command is in a menu as well.
pub struct IconButton<'a, M> {
    image: Image<'a>,
    label: String,
    on_press: Option<M>,
    width: Length,
}

impl<'a, M> IconButton<'a, M> {
    /// A button showing `image`, greyed out until it has [`IconButton::on_press`].
    pub fn new(image: Image<'a>) -> IconButton<'a, M> {
        IconButton { image, label: String::new(), on_press: None, width: Length::Shrink }
    }

    /// A label after the picture.
    pub fn label(mut self, label: &str) -> IconButton<'a, M> {
        self.label = label.to_string();
        self
    }

    /// What it sends when used.
    pub fn on_press(mut self, message: M) -> IconButton<'a, M> {
        self.on_press = Some(message);
        self
    }

    /// What it sends, if anything (greyed out with `None`).
    pub fn on_press_maybe(mut self, message: Option<M>) -> IconButton<'a, M> {
        self.on_press = message;
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> IconButton<'a, M> {
        self.width = w;
        self
    }
}

impl<'a, M: Clone + 'a> From<IconButton<'a, M>> for Element<'a, M> {
    fn from(w: IconButton<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: Clone + 'a> Widget<'a, M> for IconButton<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, Length::Shrink)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<Hold>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(Hold::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width);
        let (iw, ih) = dims(&self.image);
        // 26 x 26 for the usual 16 x 16 icon; a bigger picture makes a bigger button.
        let w = if self.label.is_empty() {
            (iw + 10).max(TOOL)
        } else {
            2 * TOOL_SIDE + iw + GAP + cx.theme.font.width(&self.label)
        };
        Node::new(limits.resolve(Size::new(w, (ih + 10).max(TOOL))))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<Hold>();
        let enabled = self.on_press.is_some();
        let (hovered, pressed) = (enabled && s.hovered, enabled && s.pressed && s.hovered);
        // Flat until the pointer is over it: nothing is drawn, so the toolbar shows through.
        if hovered || pressed {
            draw_face(canvas, node.rect, cx.theme, pressed, hovered, false);
        }
        let shift = i32::from(pressed);
        draw_content(canvas, node.rect, cx.theme, Some(&self.image), &self.label, enabled, shift);
    }

    fn look(&self, tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<Hold>();
        let enabled = self.on_press.is_some();
        Some(
            self.image
                .look(Look::new())
                .str(&self.label)
                .bool(enabled)
                .bool(enabled && s.hovered)
                .bool(enabled && s.pressed && s.hovered)
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
        let (Event::Pointer(p), Some(message)) = (event, &self.on_press) else {
            // Disabled, it still follows the pointer, so that it is right when enabled again.
            if let Event::Pointer(p) = event {
                pointer(tree.state_mut::<Hold>(), node.rect, p, false, shell);
            }
            return Status::Ignored;
        };
        match pointer(tree.state_mut::<Hold>(), node.rect, p, true, shell) {
            Pointed::Ignored => Status::Ignored,
            Pointed::Used => Status::Captured,
            Pointed::Clicked => {
                shell.publish(message.clone());
                Status::Captured
            }
        }
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "button",
            label: self.label.clone(),
            enabled: self.on_press.is_some(),
            ..Describe::default()
        })
    }
}

// ---------------------------------------------------------------- ToggleButton

/// What a toggle sends: the app is given the new value, and keeps it.
type OnToggle<'a, M> = Box<dyn Fn(bool) -> M + 'a>;

/// A button that stays pressed while it is on: a Bold button, one of Paint's tools. It only
/// shows the value it is given; when the user toggles it (a click, or Space while it has the
/// focus) it sends `f(new value)` and the app keeps the value, as for every sandstone widget.
/// With no [`ToggleButton::on_toggle`] it is greyed out.
pub struct ToggleButton<'a, M> {
    on: bool,
    label: String,
    image: Option<Image<'a>>,
    on_toggle: Option<OnToggle<'a, M>>,
    width: Length,
}

impl<'a, M> ToggleButton<'a, M> {
    /// A toggle button that is `on` or not, greyed out until it has
    /// [`ToggleButton::on_toggle`].
    pub fn new(on: bool) -> ToggleButton<'a, M> {
        ToggleButton {
            on,
            label: String::new(),
            image: None,
            on_toggle: None,
            width: Length::Shrink,
        }
    }

    /// A label.
    pub fn label(mut self, label: &str) -> ToggleButton<'a, M> {
        self.label = label.to_string();
        self
    }

    /// A picture (before the label, if there is one).
    pub fn image(mut self, image: Image<'a>) -> ToggleButton<'a, M> {
        self.image = Some(image);
        self
    }

    /// What it sends when toggled: `f` is given the new value. A closure (a function written
    /// in place, `|on| Msg::Bold(on)`) that may borrow from the app's state, as the element
    /// does.
    pub fn on_toggle(mut self, f: impl Fn(bool) -> M + 'a) -> ToggleButton<'a, M> {
        self.on_toggle = Some(Box::new(f));
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> ToggleButton<'a, M> {
        self.width = w;
        self
    }
}

impl<'a, M: 'a> From<ToggleButton<'a, M>> for Element<'a, M> {
    fn from(w: ToggleButton<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: 'a> Widget<'a, M> for ToggleButton<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, Length::Shrink)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<Hold>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(Hold::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width);
        let (iw, ih) = self.image.as_ref().map_or((0, 0), dims);
        let text = cx.theme.font.width(&self.label);
        let w = match (self.image.is_some(), self.label.is_empty()) {
            (true, true) => (iw + 10).max(TOOL),
            (false, _) => (text + 2 * SIDE).max(MIN_WIDTH),
            (true, false) => iw + GAP + text + 2 * SIDE,
        };
        let h = cx.theme.control_height.max(if ih > 0 { ih + 8 } else { 0 });
        Node::new(limits.resolve(Size::new(w, h)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<Hold>();
        let enabled = self.on_toggle.is_some();
        // On looks as a held button does, and so does one being pressed.
        let held = self.on || (enabled && s.pressed && s.hovered);
        let hovered = enabled && s.hovered;
        draw_face(canvas, node.rect, cx.theme, held, hovered, cx.has_keyboard(tree));
        let shift = i32::from(held);
        draw_content(canvas, node.rect, cx.theme, self.image.as_ref(), &self.label, enabled, shift);
    }

    fn look(&self, tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<Hold>();
        let enabled = self.on_toggle.is_some();
        let look = match &self.image {
            Some(image) => image.look(Look::new().bool(true)),
            None => Look::new().bool(false),
        };
        Some(
            look.str(&self.label)
                .bool(self.on)
                .bool(enabled)
                .bool(enabled && s.hovered)
                .bool(enabled && s.pressed && s.hovered)
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
        let id = tree.id;
        let enabled = self.on_toggle.is_some();
        match event {
            Event::Pointer(p) => {
                match pointer(tree.state_mut::<Hold>(), node.rect, p, enabled, shell) {
                    Pointed::Ignored => Status::Ignored,
                    Pointed::Used => Status::Captured,
                    Pointed::Clicked => {
                        if let Some(f) = &self.on_toggle {
                            shell.publish(f(!self.on));
                        }
                        Status::Captured
                    }
                }
            }
            Event::Key(k) if shell.is_focused(id) && k.key == Key::Space && !k.repeat => {
                match &self.on_toggle {
                    Some(f) => {
                        shell.publish(f(!self.on));
                        Status::Captured
                    }
                    None => Status::Ignored,
                }
            }
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        self.on_toggle.is_some()
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "toggle button",
            label: self.label.clone(),
            enabled: self.on_toggle.is_some(),
            checked: Some(self.on),
            ..Describe::default()
        })
    }
}

// ---------------------------------------------------------------- CheckBox

/// A box that is ticked or not, with a label after it. Clicking the box *or the label* toggles
/// it, and so does Space while it has the focus; it sends `f(new value)` and the app keeps the
/// value. With no [`CheckBox::on_toggle`] it is greyed out.
pub struct CheckBox<'a, M> {
    label: String,
    checked: bool,
    on_toggle: Option<OnToggle<'a, M>>,
    width: Length,
}

impl<'a, M> CheckBox<'a, M> {
    /// A check box with `label`, ticked if `checked`, greyed out until it has
    /// [`CheckBox::on_toggle`].
    pub fn new(label: &str, checked: bool) -> CheckBox<'a, M> {
        CheckBox { label: label.to_string(), checked, on_toggle: None, width: Length::Shrink }
    }

    /// What it sends when toggled: `f` is given the new value.
    pub fn on_toggle(mut self, f: impl Fn(bool) -> M + 'a) -> CheckBox<'a, M> {
        self.on_toggle = Some(Box::new(f));
        self
    }

    /// How wide it wants to be (the label is cut short if there isn't room).
    pub fn width(mut self, w: Length) -> CheckBox<'a, M> {
        self.width = w;
        self
    }
}

impl<'a, M: 'a> From<CheckBox<'a, M>> for Element<'a, M> {
    fn from(w: CheckBox<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: 'a> Widget<'a, M> for CheckBox<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, Length::Shrink)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<Hold>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(Hold::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width);
        let text = if self.label.is_empty() {
            0
        } else {
            BOX_GAP.saturating_add(cx.theme.font.width(&self.label))
        };
        let w = BOX.saturating_add(text);
        Node::new(limits.resolve(Size::new(w, cx.theme.control_height)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<Hold>();
        let t = cx.theme;
        let enabled = self.on_toggle.is_some();
        let r = node.rect;
        let at = Rect::new(r.x, r.y + (r.height - BOX) / 2, BOX, BOX);
        let edge = if cx.has_keyboard(tree) {
            t.accent
        } else if enabled {
            t.field_edge
        } else {
            t.button_edge
        };
        // While it is being pressed the box greys, as a button's face does.
        let paper = if enabled && s.pressed && s.hovered { t.button_pressed } else { t.field };
        canvas.fill_round_rect(at, BOX_RADIUS, edge);
        canvas.fill_round_rect(at.inset(1), BOX_RADIUS - 1, paper);
        if self.checked {
            // Two strokes, 2 pixels wide, so that the mark is as heavy as the font's bold and
            // as crisp at 2x as at 1x (the font's own tick is only four rows tall).
            let colour = if enabled { t.strong } else { t.text_disabled };
            let (x, y) = (at.x, at.y);
            canvas.line((x + 3, y + 7), (x + 5, y + 9), 2, colour);
            canvas.line((x + 5, y + 9), (x + 10, y + 4), 2, colour);
        }
        let room = r.width.saturating_sub(BOX + BOX_GAP);
        if !self.label.is_empty() && room > 0 {
            let colour = if enabled { t.text } else { t.text_disabled };
            let label = fit(&self.label, t.font, room);
            canvas.text(r.x + BOX + BOX_GAP, text_top(r, t.font.size), &label, colour, t.font);
        }
    }

    fn look(&self, tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<Hold>();
        let enabled = self.on_toggle.is_some();
        Some(
            Look::new()
                .str(&self.label)
                .bool(self.checked)
                .bool(enabled)
                .bool(enabled && s.pressed && s.hovered)
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
        let id = tree.id;
        let enabled = self.on_toggle.is_some();
        match event {
            Event::Pointer(p) => {
                match pointer(tree.state_mut::<Hold>(), node.rect, p, enabled, shell) {
                    Pointed::Ignored => Status::Ignored,
                    Pointed::Used => Status::Captured,
                    Pointed::Clicked => {
                        if let Some(f) = &self.on_toggle {
                            shell.publish(f(!self.checked));
                        }
                        Status::Captured
                    }
                }
            }
            Event::Key(k) if shell.is_focused(id) && k.key == Key::Space && !k.repeat => {
                match &self.on_toggle {
                    Some(f) => {
                        shell.publish(f(!self.checked));
                        Status::Captured
                    }
                    None => Status::Ignored,
                }
            }
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        self.on_toggle.is_some()
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "check box",
            label: self.label.clone(),
            enabled: self.on_toggle.is_some(),
            checked: Some(self.checked),
            ..Describe::default()
        })
    }
}
