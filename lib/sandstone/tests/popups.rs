// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The core's popups, timers and clipboard, with a small widget of the test's own that opens
//! a popup of buttons (the way a menu does), so they are tested apart from the menus.

use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;
use std::any::{Any, TypeId};

/// A button that opens a popup holding `items`, each a button sending its message.
struct Opener<M> {
    items: Vec<(&'static str, M)>,
}

#[derive(Default)]
struct OpenerState {
    open: bool,
    closed: u32,
}

impl<'a, M: Clone + 'a> Widget<'a, M> for Opener<M> {
    fn tag(&self) -> TypeId {
        TypeId::of::<OpenerState>()
    }
    fn state(&self) -> Box<dyn Any> {
        Box::new(OpenerState::default())
    }
    fn layout(&self, _t: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        Node::new(limits.resolve(Size::new(60, 20)))
    }
    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut pbr_draw::Canvas<'_>, cx: &DrawCx<'_>) {
        let c =
            if tree.state::<OpenerState>().open { cx.theme.accent } else { cx.theme.button_edge };
        canvas.fill_rect(node.rect, c);
    }
    fn look(&self, tree: &Tree, _n: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        Some(Look::new().bool(tree.state::<OpenerState>().open).finish())
    }
    fn on_event(
        &self,
        tree: &mut Tree,
        node: &Node,
        event: &Event,
        shell: &mut Shell<'_, M>,
    ) -> Status {
        let id = tree.id;
        let s = tree.state_mut::<OpenerState>();
        match event {
            Event::Pointer(p)
                if p.pressed(event::Button::Left).is_some()
                    && node.rect.contains(p.at.x, p.at.y) =>
            {
                s.open = true;
                shell.open_popup(id, Point::new(node.rect.x, node.rect.y + node.rect.height));
                shell.redraw();
                Status::Captured
            }
            Event::PopupClosed if s.open => {
                s.open = false;
                s.closed += 1;
                shell.redraw();
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
    fn popup<'b>(&'b self, _tree: &Tree) -> Option<Element<'b, M>>
    where
        'a: 'b,
        M: 'b,
    {
        let mut col = Column::new().padding(2).spacing(2);
        for (label, m) in &self.items {
            col = col.push(Button::new(label).on_press(m.clone()));
        }
        Some(col.into())
    }
    fn describe(&self, tree: &Tree, _n: &Node) -> Option<Describe> {
        Some(Describe {
            role: "opener",
            label: "Open".into(),
            enabled: true,
            checked: Some(tree.state::<OpenerState>().open),
            ..Describe::default()
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Pick(&'static str),
    Tick,
    Copy,
    Paste,
}

#[derive(Default)]
struct App1 {
    picked: Vec<&'static str>,
    ticks: u32,
    pasted: Option<String>,
    has_text: bool,
}

impl App for App1 {
    type Message = Msg;
    fn view(&self) -> Element<'_, Msg> {
        Column::new()
            .padding(10)
            .spacing(4)
            .push(
                Element::new(Opener {
                    items: vec![("One", Msg::Pick("one")), ("Two", Msg::Pick("two"))],
                })
                .with_id("opener"),
            )
            .push(Button::new("Copy").on_press(Msg::Copy))
            .push(Button::new("Paste").on_press_maybe(self.has_text.then_some(Msg::Paste)))
            .into()
    }
    fn update(&mut self, m: Msg, cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Pick(p) => self.picked.push(p),
            Msg::Tick => {
                self.ticks += 1;
                if self.ticks < 3 {
                    cx.after(100, Msg::Tick);
                }
            }
            Msg::Copy => {
                assert!(cx.copy_text("copied"));
            }
            Msg::Paste => self.pasted = cx.paste_text(),
        }
    }
    fn start(&mut self, cx: &mut Context<'_, Msg>) {
        cx.after(100, Msg::Tick);
    }
    fn clipboard_changed(&self, text: bool, _picture: bool) -> Option<Msg> {
        // A message only to make the app see it; the state is set in `update`.
        text.then_some(Msg::Tick).filter(|_| false)
    }
}

fn harness() -> Harness<App1> {
    Harness::new(App1::default(), Settings { size: (200, 150), ..Settings::default() })
}

#[test]
fn a_popup_opens_below_its_owner_and_an_item_sends_and_closes_it() {
    let mut h = harness();
    h.click_on("opener");
    let p = h.popup().expect("a popup");
    let w = h.window(p).unwrap();
    assert_eq!(w.kind, pbr_window::WindowKind::Popup);
    assert_eq!(w.at, (10, 30));
    assert!(w.size.0 >= 64 && w.size.1 > 40, "{:?}", w.size);
    assert_eq!(h.find("opener").unwrap().1.what.checked, Some(true));
    h.click_on("Two");
    assert!(h.popup().is_none());
    assert_eq!(h.app().picked, vec!["two"]);
    assert_eq!(h.find("opener").unwrap().1.what.checked, Some(false));
}

#[test]
fn escape_or_a_click_outside_closes_a_popup() {
    let mut h = harness();
    h.click_on("opener");
    assert!(h.popup().is_some());
    h.key(Key::Escape);
    assert!(h.popup().is_none());
    h.click_on("opener");
    h.click(150, 140);
    assert!(h.popup().is_none());
    assert!(h.app().picked.is_empty());
    assert_eq!(h.find("opener").unwrap().1.what.checked, Some(false));
}

#[test]
fn timers_come_in_order() {
    let mut h = harness();
    h.advance(50);
    assert_eq!(h.app().ticks, 0);
    h.advance(60);
    assert_eq!(h.app().ticks, 1);
    h.advance(1000);
    assert_eq!(h.app().ticks, 3);
}

#[test]
fn the_clipboard_needs_a_press() {
    let mut h = harness();
    h.click_on("Copy");
    assert_eq!(h.clipboard_text().as_deref(), Some("copied"));
    h.app_mut().has_text = true;
    h.refresh();
    h.click_on("Paste");
    assert_eq!(h.app().pasted.as_deref(), Some("copied"));
}
