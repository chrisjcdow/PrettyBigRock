// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Button`]: a labelled button, as 1.3's `ui_button`.
//!
//! This file is also the pattern for the other widgets: a builder type the app makes in its
//! `view`, a small state type sandstone keeps between views, and the [`Widget`] methods.

use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::event::{Button as MouseButton, Event, Key, PointerKind, Role, Status};
use crate::geometry::{Length, Limits, Point, Rect, Size};
use crate::layout::Node;
use crate::shell::Shell;
use crate::{Look, Theme};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use core::any::{Any, TypeId};
use pbr_draw::{Canvas, Colour};
use pbr_font::ABOVE;

/// How much wider than its label a button is.
const SIDE: i32 = 12;
/// The narrowest a labelled button is.
const MIN_WIDTH: i32 = 64;

/// A button with a label. It sends its message when clicked (pressed and let go over it), when
/// it has the focus and Space or Enter is pressed, and, if it is the window's default or
/// cancel button, when Enter or Escape is pressed and nothing focused used it. With no message
/// it is greyed out.
pub struct Button<M> {
    label: String,
    on_press: Option<M>,
    role: Option<Role>,
    width: Length,
    height: Length,
}

/// What a button remembers between views.
#[derive(Default)]
pub(crate) struct State {
    /// The pointer is over it.
    pub hovered: bool,
    /// The left button went down on it and hasn't come up.
    pub pressed: bool,
}

impl<M> Button<M> {
    /// A button labelled `label`, greyed out until it has [`Button::on_press`].
    pub fn new(label: &str) -> Button<M> {
        Button {
            label: label.to_string(),
            on_press: None,
            role: None,
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    /// What it sends when used.
    pub fn on_press(mut self, message: M) -> Button<M> {
        self.on_press = Some(message);
        self
    }

    /// What it sends, if anything (greyed out with `None`).
    pub fn on_press_maybe(mut self, message: Option<M>) -> Button<M> {
        self.on_press = message;
        self
    }

    /// The button Enter presses in its window.
    pub fn default(mut self) -> Button<M> {
        self.role = Some(Role::Default);
        self
    }

    /// The button Escape presses in its window.
    pub fn cancel(mut self) -> Button<M> {
        self.role = Some(Role::Cancel);
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> Button<M> {
        self.width = w;
        self
    }

    /// How tall it wants to be (the theme's control height by default).
    pub fn height(mut self, h: Length) -> Button<M> {
        self.height = h;
        self
    }
}

impl<'a, M: Clone + 'a> From<Button<M>> for Element<'a, M> {
    fn from(w: Button<M>) -> Element<'a, M> {
        Element::new(w)
    }
}

/// Draws a button's face in `r`, as 1.3: a rounded edge, then a light top half and a slightly
/// darker bottom half, or one flat grey while pressed (or on, for a toggle). Shared with the
/// other buttons. `focused` draws the edge in the accent: the focus ring.
pub(crate) fn draw_face(
    canvas: &mut Canvas<'_>,
    r: Rect,
    t: &Theme,
    pressed: bool,
    hovered: bool,
    focused: bool,
) {
    let edge = if focused {
        t.accent
    } else if pressed {
        t.button_edge_pressed
    } else {
        t.button_edge
    };
    canvas.fill_round_rect(r, t.radius, edge);
    let inner = r.inset(1);
    let radius = (t.radius - 1).max(0);
    if pressed {
        canvas.fill_round_rect(inner, radius, t.button_pressed);
    } else {
        canvas.fill_round_rect(inner, radius, if hovered { t.button_hover } else { t.button_top });
        let half = Rect::new(r.x, r.y + r.height / 2, r.width, r.height - r.height / 2);
        canvas.clipped(half).fill_round_rect(inner, radius, t.button_bottom);
    }
}

/// Where text `height` dots tall is drawn to sit in the middle of `r`, as 1.3 placed it: 1.3's
/// font had 9 rows, ours has `ABOVE` more over the capitals.
pub(crate) fn text_top(r: Rect, size: u32) -> i32 {
    let size = size.clamp(1, 16) as i32;
    r.y + (r.height - 9 * size) / 2 - ABOVE as i32 * size
}

impl<'a, M: Clone + 'a> Widget<'a, M> for Button<M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn tag(&self) -> TypeId {
        TypeId::of::<State>()
    }

    fn state(&self) -> Box<dyn Any> {
        Box::new(State::default())
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let w = (cx.theme.font.width(&self.label) + 2 * SIDE).max(MIN_WIDTH);
        Node::new(limits.resolve(Size::new(w, cx.theme.control_height)))
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let s = tree.state::<State>();
        let t = cx.theme;
        let enabled = self.on_press.is_some();
        let pressed = enabled && s.pressed && s.hovered;
        draw_face(canvas, node.rect, t, pressed, enabled && s.hovered, cx.has_keyboard(tree));
        let colour: Colour = if enabled { t.text } else { t.text_disabled };
        let r = node.rect;
        let label = super::label::fit(&self.label, t.font, r.width - 2 * 4);
        let shift = i32::from(pressed);
        let x = r.x + (r.width - t.font.width(&label)) / 2 + shift;
        canvas.text(x, text_top(r, t.font.size) + shift, &label, colour, t.font);
    }

    fn look(&self, tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let s = tree.state::<State>();
        Some(
            Look::new()
                .str(&self.label)
                .bool(self.on_press.is_some())
                .bool(s.hovered)
                .bool(s.pressed)
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
        let Some(message) = &self.on_press else {
            return Status::Ignored;
        };
        let s = tree.state_mut::<State>();
        match event {
            Event::Pointer(p) => {
                let inside = node.rect.contains(p.at.x, p.at.y) && p.kind != PointerKind::Left;
                if inside != s.hovered {
                    s.hovered = inside;
                    shell.redraw();
                }
                if inside && p.pressed(MouseButton::Left).is_some() {
                    s.pressed = true;
                    shell.redraw();
                    return Status::Captured;
                }
                if s.pressed && p.released(MouseButton::Left) {
                    s.pressed = false;
                    shell.redraw();
                    if inside {
                        shell.publish(message.clone());
                    }
                    return Status::Captured;
                }
                Status::Ignored
            }
            Event::Key(k)
                if shell.is_focused(id)
                    && matches!(k.key, Key::Space | Key::Enter)
                    && !k.repeat =>
            {
                shell.publish(message.clone());
                Status::Captured
            }
            Event::Activate(role) if self.role == Some(*role) => {
                shell.publish(message.clone());
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }

    fn focusable(&self, _tree: &Tree) -> bool {
        self.on_press.is_some()
    }

    fn cursor(&self, _tree: &Tree, _node: &Node, _at: Point) -> pbr_window::Cursor {
        pbr_window::Cursor::Arrow
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
