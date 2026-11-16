// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Calculator (docs/design/0026-apps-m6-tests-and-stages.md, section 1): 1.3's calculator as a
//! sandstone app tested on the host. `pbr-calc` does the sums (what each key does is
//! [`pbr_calc::Keypad`]); this crate is the window: a display, and the keys as buttons.
//!
//! The window is the size and the look of 1.3's: 244 by 338 logical pixels, not resizable, a
//! dark display with the small line of what is pending above the number, and 5 rows of 4
//! keys, the top row grey, the operators orange and `=` pink.
//!
//! A *widget* is one part of a window (a button, a label); sandstone draws it and gives it the
//! user's input (note 0023, section 5). A window is described by [`App::view`], which builds
//! its widgets afresh from the app's state each time something changes, and each widget sends
//! a *message* when the user uses it, which [`App::update`] acts on.
//!
//! Keys work as in 1.3, whichever key has the focus: digits, `.`, `+ - * / %`, `x` for times,
//! `=` or Enter, Backspace, and Escape or `c` to clear. Ctrl+C copies the display and Ctrl+V
//! pastes a number (note 0025 describes the clipboard).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use pbr_calc::{Key, Keypad, Op};
use pbr_draw::{Canvas, Colour, TextStyle};
use pbr_font::ABOVE;
use pbr_sandstone::layout::single;
use pbr_sandstone::{
    Align, App, Button, Column, Context, Describe, DrawCx, Element, Event, Grid, Identify,
    LayoutCx, Length, Limits, Look, Node, Padding, Panel, Rect, Settings, Shortcut, Size, Space,
    Status, Tree, Widget,
};

/// The window's size in logical pixels: 1.3's.
const WIDTH: u32 = 244;
const HEIGHT: u32 = 338;

/// The edge around everything, and the gap between keys.
const MARGIN: i32 = 10;
const GAP: i32 = 8;
/// The display's height, and the room between it and the first row of keys (1.3's keys start
/// 86 down: 10 + 64 + 12).
const DISPLAY_HEIGHT: i32 = 64;
const DISPLAY_TO_KEYS: i32 = 12;
/// A key's height.
const KEY_HEIGHT: i32 = 42;

/// 1.3's colours.
const BACKGROUND: Colour = Colour::solid(0xe9e6e0);
const DISPLAY_PAPER: Colour = Colour::solid(0x1f2430);
const PENDING_TEXT: Colour = Colour::solid(0x9aa3b8);
const NUMBER_TEXT: Colour = Colour::solid(0xffffff);
const GREY: Colour = Colour::solid(0x9aa0ac);
const ORANGE: Colour = Colour::solid(0xe0913a);
const PINK: Colour = Colour::solid(0xd2577a);

/// What the window's widgets send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// A calculator key, from a button or the keyboard.
    Press(Key),
    /// Ctrl+C: put the display on the clipboard.
    Copy,
    /// Ctrl+V: enter the number on the clipboard.
    Paste,
}

/// The window's size and kind.
pub fn settings() -> Settings {
    Settings {
        size: (WIDTH, HEIGHT),
        resizable: false,
        min_size: (WIDTH, HEIGHT),
        over: None,
        system: false,
    }
}

/// The calculator.
#[derive(Default)]
pub struct Calculator {
    pad: Keypad,
}

impl Calculator {
    /// A calculator showing 0.
    pub fn new() -> Calculator {
        Calculator::default()
    }

    /// What the display shows.
    pub fn display(&self) -> &str {
        self.pad.display()
    }

    /// The small line above the display: the number kept and the operator waiting.
    pub fn pending(&self) -> Option<String> {
        self.pad.pending()
    }
}

/// How a key is drawn.
#[derive(Clone, Copy)]
enum Style {
    /// An ordinary button: the digits and the point.
    Plain,
    /// One flat colour, with a white bold label.
    Coloured(Colour),
}

/// The keys, row by row as 1.3 laid them out: what each shows, what it does, its id (what the
/// tests and the log call it) and its style. The multiplication, division and minus signs
/// are the real ones, which the font has; 1.3 used `*`, `/` and `-`.
#[rustfmt::skip]
const KEYS: [[(&str, Key, &str, Style); 4]; 5] = [
    [
        ("C", Key::Clear, "clear", Style::Coloured(GREY)),
        ("+/\u{2212}", Key::Negate, "negate", Style::Coloured(GREY)),
        ("%", Key::Percent, "percent", Style::Coloured(GREY)),
        ("\u{f7}", Key::Op(Op::Divide), "divide", Style::Coloured(ORANGE)),
    ],
    [
        ("7", Key::Digit(7), "7", Style::Plain),
        ("8", Key::Digit(8), "8", Style::Plain),
        ("9", Key::Digit(9), "9", Style::Plain),
        ("\u{d7}", Key::Op(Op::Multiply), "times", Style::Coloured(ORANGE)),
    ],
    [
        ("4", Key::Digit(4), "4", Style::Plain),
        ("5", Key::Digit(5), "5", Style::Plain),
        ("6", Key::Digit(6), "6", Style::Plain),
        ("\u{2212}", Key::Op(Op::Subtract), "minus", Style::Coloured(ORANGE)),
    ],
    [
        ("1", Key::Digit(1), "1", Style::Plain),
        ("2", Key::Digit(2), "2", Style::Plain),
        ("3", Key::Digit(3), "3", Style::Plain),
        ("+", Key::Op(Op::Add), "plus", Style::Coloured(ORANGE)),
    ],
    [
        ("0", Key::Digit(0), "0", Style::Plain),
        (".", Key::Point, "point", Style::Plain),
        ("<", Key::Back, "back", Style::Plain),
        ("=", Key::Equals, "equals", Style::Coloured(PINK)),
    ],
];

impl App for Calculator {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let mut keys = Grid::new(4).spacing(GAP).width(Length::Fill);
        for (label, key, id, style) in KEYS.iter().flatten() {
            let mut button = Button::new(label)
                .on_press(Message::Press(*key))
                .width(Length::Fill)
                .height(Length::Fixed(KEY_HEIGHT));
            if let Style::Coloured(colour) = style {
                button = button.colour(*colour);
            }
            keys = keys.push(button.id(id));
        }
        let body = Column::new()
            .padding(MARGIN)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(Display::new(self.pad.display(), self.pad.pending()).id("display"))
            .push(Space::new(0, DISPLAY_TO_KEYS))
            .push(keys);
        Typing::new(
            Panel::new(body).background(BACKGROUND).width(Length::Fill).height(Length::Fill),
        )
        .into()
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Press(key) => self.pad.press(key),
            Message::Copy => {
                // An error isn't a number to copy.
                if !self.pad.is_error() {
                    cx.copy_text(self.pad.display());
                }
            }
            Message::Paste => {
                if let Some(text) = cx.paste_text() {
                    self.pad.paste(&text);
                }
            }
        }
    }

    fn shortcuts(&self) -> alloc::vec::Vec<(Shortcut, Message)> {
        alloc::vec![(Shortcut::ctrl('c'), Message::Copy), (Shortcut::ctrl('v'), Message::Paste)]
    }
}

// ---------------------------------------------------------------------------------------
// The display
// ---------------------------------------------------------------------------------------

/// The dark panel with the small pending line and the number, right-aligned. Sandstone has no
/// widget like it, so the app makes its own: a type implementing sandstone's [`Widget`] trait
/// (a *trait* is Rust's word for a set of methods a type promises to have, as an interface is
/// in Java or C#).
struct Display {
    number: String,
    pending: Option<String>,
}

impl Display {
    fn new(number: &str, pending: Option<String>) -> Display {
        Display { number: number.to_string(), pending }
    }

    /// The size the number is drawn at: the biggest of 3, 2 and 1 times the font's size that
    /// fits, as 1.3 chose.
    fn number_style(&self, room: i32) -> TextStyle {
        [3, 2]
            .into_iter()
            .map(|size| TextStyle::PLAIN.sized(size))
            .find(|style| style.width(&self.number) <= room)
            .unwrap_or(TextStyle::PLAIN)
    }
}

impl<'a> From<Display> for Element<'a, Message> {
    fn from(display: Display) -> Element<'a, Message> {
        Element::new(display)
    }
}

impl<'a> Widget<'a, Message> for Display {
    fn size(&self) -> (Length, Length) {
        (Length::Fill, Length::Fixed(DISPLAY_HEIGHT))
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(Length::Fill).height(Length::Fixed(DISPLAY_HEIGHT));
        Node::new(limits.resolve(Size::new(0, DISPLAY_HEIGHT)))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let r: Rect = node.rect;
        canvas.fill_round_rect(r, cx.theme.radius + 2, DISPLAY_PAPER);
        // Text sits 10 in from the right edge. The font has `ABOVE` rows over the capitals
        // that 1.3's did not, so text is drawn that many dots higher to put the capitals where
        // 1.3's were.
        let right = r.x + r.width - MARGIN;
        let room = r.width - 2 * MARGIN;
        if let Some(pending) = &self.pending {
            let style = TextStyle::PLAIN;
            let x = right - style.width(pending);
            canvas.text(x, r.y + 8 - ABOVE as i32, pending, PENDING_TEXT, style);
        }
        let style = self.number_style(room);
        let size = style.size as i32;
        let x = right - style.width(&self.number);
        let y = r.y + 56 - (9 + ABOVE as i32) * size;
        canvas.text(x, y, &self.number, NUMBER_TEXT, style);
    }

    fn look(&self, _tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        Some(
            Look::new()
                .str(&self.number)
                .bool(self.pending.is_some())
                .str(self.pending.as_deref().unwrap_or(""))
                .finish(),
        )
    }

    /// What the display shows, for the tests and the desktop boot's log: its label is the
    /// pending line (empty when there is none) and its value the number.
    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "display",
            label: self.pending.clone().unwrap_or_default(),
            value: self.number.clone(),
            enabled: true,
            checked: None,
        })
    }
}

// ---------------------------------------------------------------------------------------
// The keyboard
// ---------------------------------------------------------------------------------------

/// Wraps the whole window to turn typed keys into calculator keys, whichever widget has the
/// focus. Sandstone gives a key press to every widget in turn, last drawn first, and the first
/// to say it used it ([`Status::Captured`]) stops it going further; this widget is outside all
/// the others, so it sees every key before any button does, and passes on what is not a
/// calculator key (Tab, Space on a focused button, Ctrl+C) untouched.
struct Typing<'a> {
    child: [Element<'a, Message>; 1],
}

impl<'a> Typing<'a> {
    fn new(child: impl Into<Element<'a, Message>>) -> Typing<'a> {
        Typing { child: [child.into()] }
    }
}

impl<'a> From<Typing<'a>> for Element<'a, Message> {
    fn from(typing: Typing<'a>) -> Element<'a, Message> {
        Element::new(typing)
    }
}

/// The calculator key a key press stands for, if any: 1.3's keyboard (see [`Key::from_char`]
/// for the typed characters), with Enter for `=`, Backspace for `<` and Escape for `C`. A press
/// with Ctrl, Alt or the Windows key held is a shortcut's, not typing. A key held down repeats
/// digits and Backspace, as typing does, but not the others: a held Enter would otherwise add
/// the number to itself again and again.
fn key_for(press: &pbr_sandstone::KeyPress) -> Option<Key> {
    let m = press.modifiers;
    if m.ctrl() || m.alt() || m.meta() {
        return None;
    }
    let key = match press.key {
        pbr_sandstone::Key::Enter => Key::Equals,
        pbr_sandstone::Key::Backspace => Key::Back,
        pbr_sandstone::Key::Escape => Key::Clear,
        _ if press.typed => press.ch.and_then(Key::from_char)?,
        _ => return None,
    };
    (!press.repeat || matches!(key, Key::Digit(_) | Key::Back)).then_some(key)
}

impl<'a> Widget<'a, Message> for Typing<'a> {
    fn size(&self) -> (Length, Length) {
        self.child[0].widget().size()
    }

    fn children(&self) -> &[Element<'a, Message>] {
        &self.child
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let (w, h) = self.size();
        let limits = limits.width(w).height(h);
        single(
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
        shell: &mut pbr_sandstone::Shell<'_, Message>,
    ) -> Status {
        if let Event::Key(press) = event
            && let Some(key) = key_for(press)
        {
            shell.publish(Message::Press(key));
            return Status::Captured;
        }
        pbr_sandstone::element::children_event(&self.child, tree, node, event, shell)
    }
}

#[cfg(test)]
mod tests;
