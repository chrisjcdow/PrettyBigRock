// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Label`]: text, on one line or wrapped to its width.

use crate::Look;
use crate::element::{Describe, DrawCx, Element, LayoutCx, Tree, Widget};
use crate::geometry::{Align, Length, Limits, Size, UNBOUNDED};
use crate::layout::Node;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use pbr_draw::{Canvas, Colour, TextStyle};

/// The character shown where a line was cut short.
const ELLIPSIS: char = '…';

/// Text. Lines break at `\n`; a wrapping label also breaks between words to fit its width.
/// A line too long for its room is cut short with "…".
pub struct Label {
    text: String,
    style: Option<TextStyle>,
    colour: Option<Colour>,
    wrap: bool,
    max_width: i32,
    width: Length,
    align: Align,
}

impl Label {
    /// A label in the theme's ordinary text.
    pub fn new(text: &str) -> Label {
        Label {
            text: text.to_string(),
            style: None,
            colour: None,
            wrap: false,
            max_width: UNBOUNDED,
            width: Length::Shrink,
            align: Align::Start,
        }
    }

    /// In another style (bold, the heading, a size).
    pub fn style(mut self, style: TextStyle) -> Label {
        self.style = Some(style);
        self
    }

    /// In the theme's bold.
    pub fn bold(self) -> Label {
        self.style(crate::Theme::STANDARD.bold)
    }

    /// In the theme's heading style.
    pub fn heading(self) -> Label {
        self.style(crate::Theme::STANDARD.heading)
    }

    /// In another colour.
    pub fn colour(mut self, colour: Colour) -> Label {
        self.colour = Some(colour);
        self
    }

    /// Breaks between words to fit its width.
    pub fn wrap(mut self) -> Label {
        self.wrap = true;
        self
    }

    /// Never wider than this (with `wrap`, the width it wraps at).
    pub fn max_width(mut self, w: i32) -> Label {
        self.max_width = w;
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> Label {
        self.width = w;
        self
    }

    /// Where each line goes when the label is wider than it.
    pub fn align(mut self, align: Align) -> Label {
        self.align = align;
        self
    }

    fn text_style(&self, cx_theme: &crate::Theme) -> TextStyle {
        self.style.unwrap_or(cx_theme.font)
    }

    /// The lines it shows in `width`.
    fn lines(&self, style: TextStyle, width: i32) -> Vec<String> {
        let mut out = Vec::new();
        for para in self.text.split('\n') {
            if !self.wrap {
                out.push(fit(para, style, width));
                continue;
            }
            wrap_into(para, style, width, &mut out);
        }
        out
    }
}

impl<'a, M: 'a> From<Label> for Element<'a, M> {
    fn from(w: Label) -> Element<'a, M> {
        Element::new(w)
    }
}

/// `text` cut to fit `width`, ending in "…" if it was cut.
pub(crate) fn fit(text: &str, style: TextStyle, width: i32) -> String {
    if style.width(text) <= width {
        return text.to_string();
    }
    let room = ((width / style.advance().max(1)) - 1).max(0) as usize;
    let mut s: String = text.chars().take(room).collect();
    s.push(ELLIPSIS);
    s
}

/// Breaks `para` into lines no wider than `width`, between words where it can (a word
/// longer than a line is broken where it must be).
fn wrap_into(para: &str, style: TextStyle, width: i32, out: &mut Vec<String>) {
    let per_line = (width / style.advance().max(1)).max(1) as usize;
    let mut line = String::new();
    let mut count = 0usize;
    for word in para.split(' ') {
        let n = word.chars().count();
        let need = if count == 0 { n } else { count + 1 + n };
        if need <= per_line {
            if count > 0 {
                line.push(' ');
            }
            line.push_str(word);
            count = need;
            continue;
        }
        if count > 0 {
            out.push(core::mem::take(&mut line));
        }
        // A word longer than a line, in pieces.
        let mut chars = word.chars().peekable();
        count = 0;
        while chars.peek().is_some() {
            let piece: String = chars.by_ref().take(per_line).collect();
            count = piece.chars().count();
            if chars.peek().is_some() {
                out.push(piece);
            } else {
                line = piece;
            }
        }
    }
    out.push(line);
}

impl<'a, M> Widget<'a, M> for Label {
    fn size(&self) -> (Length, Length) {
        (self.width, Length::Shrink)
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width);
        let style = self.text_style(cx.theme);
        let room = limits.max.width.min(self.max_width);
        let lines = self.lines(style, room);
        let widest = lines.iter().map(|l| style.width(l)).max().unwrap_or(0);
        let height = style.line_height().saturating_mul(lines.len() as i32);
        Node::new(limits.resolve(Size::new(widest.min(room), height)))
    }

    fn draw(&self, _tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let style = self.text_style(cx.theme);
        let colour = self.colour.unwrap_or(cx.theme.text);
        let r = node.rect;
        for (i, line) in self.lines(style, r.width).iter().enumerate() {
            let x = r.x + self.align.offset(r.width, style.width(line));
            canvas.text(x, r.y + style.line_height() * i as i32, line, colour, style);
        }
    }

    fn look(&self, _tree: &Tree, _node: &Node, cx: &DrawCx<'_>) -> Option<u64> {
        let style = self.text_style(cx.theme);
        let colour = self.colour.unwrap_or(cx.theme.text);
        Some(
            Look::new()
                .str(&self.text)
                .u64(u64::from(style.size))
                .bool(style.bold)
                .u64(u64::from(colour.0))
                .bool(self.wrap)
                .i32(self.align as i32)
                .finish(),
        )
    }

    fn describe(&self, _tree: &Tree, _node: &Node) -> Option<Describe> {
        Some(Describe {
            role: "label",
            label: self.text.clone(),
            enabled: true,
            ..Describe::default()
        })
    }
}
