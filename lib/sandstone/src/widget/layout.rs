// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The widgets that arrange others: [`Row`], [`Column`], [`Grid`], [`Space`], [`Stretch`] and
//! [`Panel`] (note 0023, section 5). They draw nothing of their own, except a panel's paper
//! and edge.

use crate::Look;
use crate::element::{DrawCx, Element, LayoutCx, Tree, Widget};
use crate::geometry::{Align, Length, Limits, Padding, Size, UNBOUNDED};
use crate::layout::{self, Axis, Flex, Node};
use alloc::vec::Vec;
use pbr_draw::{Canvas, Colour};

/// Writes the builder methods rows and columns share. A *macro* writes code: here, the same
/// methods for two types, which Rust would otherwise make us write twice.
macro_rules! flex_builders {
    ($t:ident) => {
        impl<'a, M: 'a> $t<'a, M> {
            /// An empty one.
            pub fn new() -> $t<'a, M> {
                $t {
                    children: Vec::new(),
                    padding: Padding::ZERO,
                    spacing: 0,
                    align: Align::Start,
                    width: Length::Shrink,
                    height: Length::Shrink,
                }
            }

            /// One holding `children`.
            pub fn with(children: Vec<Element<'a, M>>) -> $t<'a, M> {
                let mut s = $t::new();
                s.children = children;
                s
            }

            /// Adds a child at the end.
            pub fn push(mut self, child: impl Into<Element<'a, M>>) -> $t<'a, M> {
                self.children.push(child.into());
                self
            }

            /// Adds a child at the end, if there is one.
            pub fn push_maybe(self, child: Option<impl Into<Element<'a, M>>>) -> $t<'a, M> {
                match child {
                    Some(c) => self.push(c),
                    None => self,
                }
            }

            /// Room around the children, the same on every side.
            pub fn padding(mut self, n: i32) -> $t<'a, M> {
                self.padding = Padding::all(n);
                self
            }

            /// Room around the children.
            pub fn padding_each(mut self, p: Padding) -> $t<'a, M> {
                self.padding = p;
                self
            }

            /// Room between neighbours.
            pub fn spacing(mut self, n: i32) -> $t<'a, M> {
                self.spacing = n;
                self
            }

            /// Where a child that doesn't fill the other way goes.
            pub fn align(mut self, align: Align) -> $t<'a, M> {
                self.align = align;
                self
            }

            /// How wide it wants to be.
            pub fn width(mut self, w: Length) -> $t<'a, M> {
                self.width = w;
                self
            }

            /// How tall it wants to be.
            pub fn height(mut self, h: Length) -> $t<'a, M> {
                self.height = h;
                self
            }
        }

        impl<'a, M: 'a> Default for $t<'a, M> {
            fn default() -> $t<'a, M> {
                $t::new()
            }
        }

        impl<'a, M: 'a> From<$t<'a, M>> for Element<'a, M> {
            fn from(w: $t<'a, M>) -> Element<'a, M> {
                Element::new(w)
            }
        }
    };
}

/// Children side by side, left to right.
pub struct Row<'a, M> {
    children: Vec<Element<'a, M>>,
    padding: Padding,
    spacing: i32,
    align: Align,
    width: Length,
    height: Length,
}

/// Children one above another, top to bottom.
pub struct Column<'a, M> {
    children: Vec<Element<'a, M>>,
    padding: Padding,
    spacing: i32,
    align: Align,
    width: Length,
    height: Length,
}

flex_builders!(Row);
flex_builders!(Column);

impl<'a, M: 'a> Widget<'a, M> for Row<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.children
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let f = Flex {
            axis: Axis::Across,
            padding: self.padding,
            spacing: self.spacing,
            align: self.align,
        };
        layout::flex(
            f,
            limits.width(self.width).height(self.height),
            &self.children,
            &mut tree.children,
            cx,
        )
    }
}

impl<'a, M: 'a> Widget<'a, M> for Column<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.children
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let f = Flex {
            axis: Axis::Down,
            padding: self.padding,
            spacing: self.spacing,
            align: self.align,
        };
        layout::flex(
            f,
            limits.width(self.width).height(self.height),
            &self.children,
            &mut tree.children,
            cx,
        )
    }
}

/// Children in a grid of equal columns, filled row by row (Calculator's keys). Each row is as
/// tall as its tallest child; a grid that fills its height shares it equally among its rows.
pub struct Grid<'a, M> {
    children: Vec<Element<'a, M>>,
    columns: usize,
    spacing: i32,
    width: Length,
    height: Length,
}

impl<'a, M: 'a> Grid<'a, M> {
    /// An empty grid of `columns` columns (at least one).
    pub fn new(columns: usize) -> Grid<'a, M> {
        Grid {
            children: Vec::new(),
            columns: columns.max(1),
            spacing: 0,
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    /// Adds a child in the next cell.
    pub fn push(mut self, child: impl Into<Element<'a, M>>) -> Grid<'a, M> {
        self.children.push(child.into());
        self
    }

    /// Room between cells, both ways.
    pub fn spacing(mut self, n: i32) -> Grid<'a, M> {
        self.spacing = n;
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> Grid<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be.
    pub fn height(mut self, h: Length) -> Grid<'a, M> {
        self.height = h;
        self
    }
}

impl<'a, M: 'a> From<Grid<'a, M>> for Element<'a, M> {
    fn from(w: Grid<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: 'a> Widget<'a, M> for Grid<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.children
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        let cols = self.columns as i32;
        let rows = self.children.len().div_ceil(self.columns) as i32;
        let gap = self.spacing;
        let trees = &mut tree.children;

        // Each column's width: shared out if the width is limited, otherwise the widest child.
        let cell_w = if limits.max.width < UNBOUNDED {
            ((limits.max.width - gap * (cols - 1)) / cols).max(0)
        } else {
            let natural = |(c, t): (&Element<'a, M>, &mut Tree)| {
                c.widget.layout(t, Limits::loose(Size::new(UNBOUNDED, UNBOUNDED)), cx).size().width
            };
            self.children.iter().zip(trees.iter_mut()).map(natural).max().unwrap_or(0)
        };
        let shared_h = (limits.min.height > 0 && limits.max.height < UNBOUNDED && rows > 0)
            .then(|| ((limits.max.height - gap * (rows - 1)) / rows).max(0));

        let mut nodes = Vec::with_capacity(self.children.len());
        let mut y = 0;
        for r in 0..rows as usize {
            let range = r * self.columns..((r + 1) * self.columns).min(self.children.len());
            let max_h = shared_h.unwrap_or(UNBOUNDED);
            let mut row: Vec<Node> = Vec::new();
            for i in range.clone() {
                let l = Limits::loose(Size::new(cell_w, max_h));
                row.push(self.children[i].widget.layout(&mut trees[i], l, cx));
            }
            let h =
                shared_h.unwrap_or_else(|| row.iter().map(|n| n.rect.height).max().unwrap_or(0));
            for (k, i) in range.enumerate() {
                // A child that fills is laid out again at the row's height.
                let fills = self.children[i].widget.size().1.share() > 0;
                if fills && row[k].rect.height != h {
                    let w = row[k].rect.width;
                    row[k] = self.children[i].widget.layout(
                        &mut trees[i],
                        Limits::exact(Size::new(w, h)),
                        cx,
                    );
                }
                row[k].move_to(k as i32 * (cell_w + gap), y);
            }
            nodes.extend(row);
            y += h + gap;
        }
        let content = Size::new(cols * cell_w + gap * (cols - 1), (y - gap).max(0));
        Node::with_children(limits.resolve(content), nodes)
    }
}

/// Empty room of a fixed size.
pub struct Space {
    width: i32,
    height: i32,
}

impl Space {
    /// Room `width` across and `height` down.
    pub fn new(width: i32, height: i32) -> Space {
        Space { width, height }
    }
}

impl<'a, M: 'a> From<Space> for Element<'a, M> {
    fn from(w: Space) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M> Widget<'a, M> for Space {
    fn size(&self) -> (Length, Length) {
        (Length::Fixed(self.width), Length::Fixed(self.height))
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        Node::new(limits.resolve(Size::new(self.width, self.height)))
    }
}

/// Empty room that takes all there is: put one between a row's children to push those after
/// it to the far end (a dialog's buttons to the right).
pub struct Stretch;

impl Stretch {
    /// Room that fills.
    pub fn new() -> Stretch {
        Stretch
    }
}

impl Default for Stretch {
    fn default() -> Stretch {
        Stretch
    }
}

impl<'a, M: 'a> From<Stretch> for Element<'a, M> {
    fn from(w: Stretch) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M> Widget<'a, M> for Stretch {
    fn size(&self) -> (Length, Length) {
        (Length::Fill, Length::Fill)
    }

    fn layout(&self, _tree: &mut Tree, limits: Limits, _cx: &LayoutCx<'_>) -> Node {
        let l = limits.width(Length::Fill).height(Length::Fill);
        // In a direction with no limit, filling means nothing: take no room.
        Node::new(Size::new(
            if l.max.width >= UNBOUNDED { 0 } else { l.min.width },
            if l.max.height >= UNBOUNDED { 0 } else { l.min.height },
        ))
    }
}

/// One child, with padding, and optionally paper and an edge behind it (1.3's `ui_panel` is
/// [`Panel::boxed`]: white with a grey edge).
pub struct Panel<'a, M> {
    child: [Element<'a, M>; 1],
    padding: Padding,
    background: Option<Colour>,
    edge: Option<Colour>,
    radius: i32,
    width: Length,
    height: Length,
    align: (Align, Align),
}

impl<'a, M: 'a> Panel<'a, M> {
    /// A panel around `child`, with nothing drawn.
    pub fn new(child: impl Into<Element<'a, M>>) -> Panel<'a, M> {
        Panel {
            child: [child.into()],
            padding: Padding::ZERO,
            background: None,
            edge: None,
            radius: 0,
            width: Length::Shrink,
            height: Length::Shrink,
            align: (Align::Start, Align::Start),
        }
    }

    /// A white box with a grey edge, as 1.3's panels (and a field's paper).
    pub fn boxed(child: impl Into<Element<'a, M>>) -> Panel<'a, M> {
        let t = crate::Theme::STANDARD;
        Panel::new(child).background(t.field).edge(t.field_edge)
    }

    /// Room around the child, the same on every side.
    pub fn padding(mut self, n: i32) -> Panel<'a, M> {
        self.padding = Padding::all(n);
        self
    }

    /// Room around the child.
    pub fn padding_each(mut self, p: Padding) -> Panel<'a, M> {
        self.padding = p;
        self
    }

    /// Paper behind the child.
    pub fn background(mut self, c: Colour) -> Panel<'a, M> {
        self.background = Some(c);
        self
    }

    /// A one-pixel edge.
    pub fn edge(mut self, c: Colour) -> Panel<'a, M> {
        self.edge = Some(c);
        self
    }

    /// Round corners.
    pub fn radius(mut self, r: i32) -> Panel<'a, M> {
        self.radius = r;
        self
    }

    /// How wide it wants to be.
    pub fn width(mut self, w: Length) -> Panel<'a, M> {
        self.width = w;
        self
    }

    /// How tall it wants to be.
    pub fn height(mut self, h: Length) -> Panel<'a, M> {
        self.height = h;
        self
    }

    /// Where the child goes when the panel is bigger than it (across, down).
    pub fn align(mut self, across: Align, down: Align) -> Panel<'a, M> {
        self.align = (across, down);
        self
    }
}

impl<'a, M: 'a> From<Panel<'a, M>> for Element<'a, M> {
    fn from(w: Panel<'a, M>) -> Element<'a, M> {
        Element::new(w)
    }
}

impl<'a, M: 'a> Widget<'a, M> for Panel<'a, M> {
    fn size(&self) -> (Length, Length) {
        (self.width, self.height)
    }

    fn children(&self) -> &[Element<'a, M>] {
        &self.child
    }

    fn layout(&self, tree: &mut Tree, limits: Limits, cx: &LayoutCx<'_>) -> Node {
        let limits = limits.width(self.width).height(self.height);
        layout::single(limits, self.padding, self.align, &self.child[0], &mut tree.children[0], cx)
    }

    fn draw(&self, tree: &Tree, node: &Node, canvas: &mut Canvas<'_>, cx: &DrawCx<'_>) {
        let r = node.rect;
        match (self.background, self.edge) {
            (Some(bg), Some(edge)) => {
                canvas.fill_round_rect(r, self.radius, edge);
                canvas.fill_round_rect(r.inset(1), (self.radius - 1).max(0), bg);
            }
            (Some(bg), None) => canvas.fill_round_rect(r, self.radius, bg),
            (None, Some(edge)) => canvas.stroke_round_rect(r, self.radius, 1, edge),
            (None, None) => {}
        }
        crate::element::draw_children(&self.child, tree, node, canvas, cx);
    }

    fn look(&self, _tree: &Tree, _node: &Node, _cx: &DrawCx<'_>) -> Option<u64> {
        if self.background.is_none() && self.edge.is_none() {
            return None;
        }
        let colour = |c: Option<Colour>| c.map_or(0, |c| u64::from(c.0) | 1 << 32);
        Some(
            Look::new()
                .u64(colour(self.background))
                .u64(colour(self.edge))
                .i32(self.radius)
                .finish(),
        )
    }
}
