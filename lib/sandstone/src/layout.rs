// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Layout: where each element goes (note 0023, section 5).
//!
//! Layout is one pass down the tree. A parent gives each child [`Limits`] (the smallest and
//! largest size it may take); the child answers with a [`Node`]: the size it chose and where
//! its own children go. Rows and columns ([`flex`]) first give the children that want only
//! what their content needs ([`Length::Shrink`], `Fixed`) their room, then share what is left
//! among those that want to fill ([`Length::Fill`], `Portion`). This is the shape of CSS's
//! flexbox and of `iced`, cut down to what PrettyBigRock's apps need.

use crate::element::{Element, LayoutCx, Tree};
use crate::geometry::{Align, Length, Limits, Padding, Rect, Size, UNBOUNDED};
use alloc::vec::Vec;

/// Where an element is, and where its children are.
///
/// While layout runs, a node's rectangle is relative to its parent's corner (a widget lays
/// its children out from (0, 0) without knowing where it will be put); once the whole tree is
/// laid out, sandstone makes every rectangle relative to the window ([`Node::place`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Where, and how big.
    pub rect: Rect,
    /// The children's nodes, in the order of the widget's children.
    pub children: Vec<Node>,
}

impl Node {
    /// A node of `size` with no children, at (0, 0).
    pub fn new(size: Size) -> Node {
        Node { rect: Rect::new(0, 0, size.width, size.height), children: Vec::new() }
    }

    /// A node of `size` with children (already placed relative to it).
    pub fn with_children(size: Size, children: Vec<Node>) -> Node {
        Node { rect: Rect::new(0, 0, size.width, size.height), children }
    }

    /// Its size.
    pub fn size(&self) -> Size {
        Size::new(self.rect.width, self.rect.height)
    }

    /// Puts it at (`x`, `y`) in its parent (while laying out).
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.rect.x = x;
        self.rect.y = y;
    }

    /// Makes this node's rectangle, and everything inside it, relative to the window, given
    /// where its parent's corner is.
    pub fn place(&mut self, parent_x: i32, parent_y: i32) {
        self.rect.x = self.rect.x.saturating_add(parent_x);
        self.rect.y = self.rect.y.saturating_add(parent_y);
        let (x, y) = (self.rect.x, self.rect.y);
        for c in &mut self.children {
            c.place(x, y);
        }
    }
}

/// Which way a row or column runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Left to right: a row.
    Across,
    /// Top to bottom: a column.
    Down,
}

impl Axis {
    /// (along, across) of a size.
    fn split(self, s: Size) -> (i32, i32) {
        match self {
            Axis::Across => (s.width, s.height),
            Axis::Down => (s.height, s.width),
        }
    }

    /// A size from (along, across).
    fn join(self, along: i32, across: i32) -> Size {
        match self {
            Axis::Across => Size::new(along, across),
            Axis::Down => Size::new(across, along),
        }
    }

    /// The length a widget wants along and across this axis.
    fn lengths(self, (w, h): (Length, Length)) -> (Length, Length) {
        match self {
            Axis::Across => (w, h),
            Axis::Down => (h, w),
        }
    }
}

/// How a row or column arranges its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flex {
    /// Which way it runs.
    pub axis: Axis,
    /// Room around the children.
    pub padding: Padding,
    /// Room between neighbours.
    pub spacing: i32,
    /// Where a child narrower than the row's height (or a column's width) goes.
    pub align: Align,
}

/// Lays out `children` (with their states `trees`) in a row or column within `limits`, and
/// answers the node: as small as the children allow, or bigger if the limits say so.
///
/// Every widget's `layout` starts by applying its own [`Widget::size`](crate::Widget::size)
/// to the limits it is given (`limits.width(w).height(h)`); this relies on that.
pub fn flex<'a, M>(
    f: Flex,
    limits: Limits,
    children: &[Element<'a, M>],
    trees: &mut [Tree],
    cx: &LayoutCx<'_>,
) -> Node {
    let inner = limits.shrink(f.padding);
    let (max_along, max_across) = f.axis.split(inner.max);
    let (_, min_across) = f.axis.split(inner.min);
    let gaps = f.spacing.saturating_mul(children.len().saturating_sub(1) as i32);
    let unbounded = max_along >= UNBOUNDED;
    let mut nodes: Vec<Node> = (0..children.len()).map(|_| Node::default()).collect();
    // A child that fills across is first measured with no limit across, so that it says how
    // deep its content needs (a `Fill` with no limit means nothing); once the row's depth is
    // known it is laid out again at that depth.
    let fills_across = |c: &Element<'a, M>| f.axis.lengths(c.widget.size()).1.share() > 0;
    let across_for = |c: &Element<'a, M>| if fills_across(c) { UNBOUNDED } else { max_across };

    // First those that want only their content's room (or all of them, when there is no end
    // to share: in a scrolled column, `Fill` means nothing).
    let mut used = gaps;
    let mut shares = 0u32;
    for (i, (child, tree)) in children.iter().zip(trees.iter_mut()).enumerate() {
        let (along, _) = f.axis.lengths(child.widget.size());
        if along.share() > 0 && !unbounded {
            shares += along.share();
            continue;
        }
        let room = if unbounded { UNBOUNDED } else { (max_along - used).max(0) };
        let l = Limits::loose(f.axis.join(room, across_for(child)));
        nodes[i] = child.widget.layout(tree, l, cx);
        used = used.saturating_add(f.axis.split(nodes[i].size()).0);
    }

    // Then share the rest among the fillers, in proportion; the last takes what rounding left.
    if shares > 0 {
        let mut left = (max_along - used).max(0);
        let mut shares_left = shares;
        for (i, (child, tree)) in children.iter().zip(trees.iter_mut()).enumerate() {
            let (along, _) = f.axis.lengths(child.widget.size());
            let share = along.share();
            if share == 0 {
                continue;
            }
            let room = if share == shares_left {
                left
            } else {
                (i64::from(left) * i64::from(share) / i64::from(shares_left)) as i32
            };
            shares_left -= share;
            left -= room;
            let l = Limits::new(f.axis.join(room, 0), f.axis.join(room, across_for(child)));
            nodes[i] = child.widget.layout(tree, l, cx);
        }
    }

    // How deep the row is: its deepest child, within the limits. Children that fill across
    // are then laid out again at that depth.
    let deepest = nodes.iter().map(|n| f.axis.split(n.size()).1).max().unwrap_or(0);
    let across = deepest.max(min_across).min(max_across);
    for (i, (child, tree)) in children.iter().zip(trees.iter_mut()).enumerate() {
        if fills_across(child) {
            let along = f.axis.split(nodes[i].size()).0;
            nodes[i] = child.widget.layout(tree, Limits::exact(f.axis.join(along, across)), cx);
        }
    }

    // Place them.
    let mut at = 0i32;
    for n in &mut nodes {
        let (along, depth) = f.axis.split(n.size());
        let off = f.align.offset(across, depth);
        match f.axis {
            Axis::Across => n.move_to(f.padding.left + at, f.padding.top + off),
            Axis::Down => n.move_to(f.padding.left + off, f.padding.top + at),
        }
        at = at.saturating_add(along).saturating_add(f.spacing);
    }
    let along = (at - f.spacing).max(0);
    let content = f.axis.join(along, across);
    let size = limits.resolve(Size::new(
        content.width.saturating_add(f.padding.across()),
        content.height.saturating_add(f.padding.down()),
    ));
    Node::with_children(size, nodes)
}

/// Lays out one child inside `padding` within `limits`, placed by `align` (across, down) when
/// the room is bigger than it: what a panel or a button does with its content.
pub fn single<'a, M>(
    limits: Limits,
    padding: Padding,
    align: (Align, Align),
    child: &Element<'a, M>,
    tree: &mut Tree,
    cx: &LayoutCx<'_>,
) -> Node {
    let inner = limits.shrink(padding);
    let (w, h) = child.widget.size();
    let child_limits = Limits::loose(inner.max).width(w).height(h);
    let mut node = child.widget.layout(tree, child_limits, cx);
    let content = node.size();
    let size = limits.resolve(Size::new(
        content.width.saturating_add(padding.across()),
        content.height.saturating_add(padding.down()),
    ));
    let room = Size::new(size.width - padding.across(), size.height - padding.down());
    node.move_to(
        padding.left + align.0.offset(room.width, content.width),
        padding.top + align.1.offset(room.height, content.height),
    );
    Node::with_children(size, alloc::vec![node])
}
