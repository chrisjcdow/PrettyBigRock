// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Rectangles: the one shape everything else here is described with.

/// A rectangle with its sides along the axes: its top left corner and its size. `x` grows to
/// the right and `y` grows downwards, as on a screen.
///
/// A rectangle whose width or height is zero or less is **empty**: it holds no pixels, and
/// every operation treats all empty rectangles alike. Nothing here panics or wraps around,
/// whatever the numbers are, because sizes and places can come from programs that lie.
///
/// `#[derive(...)]` asks Rust to write the obvious code for copying, comparing and printing
/// the type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// Pixels across.
    pub width: i32,
    /// Pixels down.
    pub height: i32,
}

impl Rect {
    /// The rectangle with no pixels.
    pub const EMPTY: Rect = Rect { x: 0, y: 0, width: 0, height: 0 };

    /// A rectangle from its top left corner and its size.
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { x, y, width, height }
    }

    /// The rectangle from `left` up to (not including) `right`, and `top` up to `bottom`.
    /// Edges are 64-bit here so that callers can work them out without overflowing; the result
    /// is cut to what an `i32` can hold.
    pub fn from_edges(left: i64, top: i64, right: i64, bottom: i64) -> Rect {
        let cut = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let (left, top, right, bottom) = (cut(left), cut(top), cut(right), cut(bottom));
        if right <= left || bottom <= top {
            return Rect::EMPTY;
        }
        // The differences fit in an `i64` but perhaps not an `i32`; a rectangle that wide is
        // cut short, which only matters to code that is already far outside any canvas.
        Rect {
            x: left as i32,
            y: top as i32,
            width: cut(right - left) as i32,
            height: cut(bottom - top) as i32,
        }
    }

    /// Whether it holds no pixels.
    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// The edge just past its last column, as a 64-bit number so that it can't overflow.
    pub const fn right(&self) -> i64 {
        self.x as i64 + if self.width > 0 { self.width as i64 } else { 0 }
    }

    /// The edge just past its last row.
    pub const fn bottom(&self) -> i64 {
        self.y as i64 + if self.height > 0 { self.height as i64 } else { 0 }
    }

    /// How many pixels it holds.
    pub const fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.width as i64 * self.height as i64 }
    }

    /// Whether the pixel at (`x`, `y`) is inside.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        !self.is_empty()
            && x >= self.x
            && y >= self.y
            && (x as i64) < self.right()
            && (y as i64) < self.bottom()
    }

    /// Whether all of `other` is inside (an empty `other` is inside anything).
    pub const fn contains_rect(&self, other: &Rect) -> bool {
        other.is_empty()
            || (!self.is_empty()
                && other.x >= self.x
                && other.y >= self.y
                && other.right() <= self.right()
                && other.bottom() <= self.bottom())
    }

    /// The part both rectangles share, or [`Rect::EMPTY`].
    pub fn intersect(&self, other: &Rect) -> Rect {
        if self.is_empty() || other.is_empty() {
            return Rect::EMPTY;
        }
        Rect::from_edges(
            i64::from(self.x.max(other.x)),
            i64::from(self.y.max(other.y)),
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        )
    }

    /// The smallest rectangle holding both (their *bounding box*). An empty rectangle adds
    /// nothing.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return if other.is_empty() { Rect::EMPTY } else { *other };
        }
        if other.is_empty() {
            return *self;
        }
        Rect::from_edges(
            i64::from(self.x.min(other.x)),
            i64::from(self.y.min(other.y)),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    /// The same rectangle moved by (`dx`, `dy`); a move past what an `i32` can hold stops at
    /// the limit.
    pub const fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            width: self.width,
            height: self.height,
        }
    }

    /// The rectangle with every edge moved inwards by `by` (outwards if `by` is negative).
    /// Shrinking past nothing gives [`Rect::EMPTY`].
    pub fn inset(&self, by: i32) -> Rect {
        if self.is_empty() {
            return Rect::EMPTY;
        }
        let by = i64::from(by);
        Rect::from_edges(
            i64::from(self.x) + by,
            i64::from(self.y) + by,
            self.right() - by,
            self.bottom() - by,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: i32 = i32::MAX;
    const MIN: i32 = i32::MIN;

    #[test]
    fn empty_rectangles_hold_nothing() {
        for r in [
            Rect::EMPTY,
            Rect::new(5, 5, 0, 10),
            Rect::new(5, 5, 10, 0),
            Rect::new(5, 5, -3, 10),
            Rect::new(5, 5, 10, MIN),
        ] {
            assert!(r.is_empty(), "{r:?}");
            assert_eq!(r.area(), 0);
            assert!(!r.contains(5, 5));
            assert_eq!(r.intersect(&Rect::new(0, 0, 100, 100)), Rect::EMPTY);
            assert_eq!(r.union(&Rect::new(1, 2, 3, 4)), Rect::new(1, 2, 3, 4));
            assert_eq!(Rect::new(1, 2, 3, 4).union(&r), Rect::new(1, 2, 3, 4));
            assert_eq!(r.inset(-5), Rect::EMPTY);
            assert!(Rect::new(0, 0, 1, 1).contains_rect(&r));
            // A negative size doesn't move the far edges backwards.
            assert!(r.right() >= i64::from(r.x) && r.bottom() >= i64::from(r.y));
        }
        assert_eq!(Rect::EMPTY.union(&Rect::new(9, 9, -1, -1)), Rect::EMPTY);
    }

    #[test]
    fn edges_area_and_contains() {
        let r = Rect::new(-2, 3, 4, 5);
        assert_eq!((r.right(), r.bottom(), r.area()), (2, 8, 20));
        assert!(r.contains(-2, 3) && r.contains(1, 7));
        assert!(!r.contains(2, 3) && !r.contains(-2, 8) && !r.contains(-3, 3) && !r.contains(0, 2));
        assert_eq!(Rect::from_edges(-2, 3, 2, 8), r);
        assert_eq!(Rect::from_edges(2, 3, -2, 8), Rect::EMPTY);
        assert_eq!(Rect::from_edges(0, 8, 2, 8), Rect::EMPTY);
    }

    #[test]
    fn intersect_and_union() {
        let (a, b) = (Rect::new(0, 0, 10, 10), Rect::new(5, -5, 10, 10));
        assert_eq!(a.intersect(&b), Rect::new(5, 0, 5, 5));
        assert_eq!(b.intersect(&a), Rect::new(5, 0, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, -5, 15, 15));
        // Touching edges share nothing.
        assert_eq!(a.intersect(&Rect::new(10, 0, 5, 5)), Rect::EMPTY);
        assert_eq!(a.intersect(&Rect::new(0, 10, 5, 5)), Rect::EMPTY);
        assert_eq!(a.intersect(&a), a);
        assert!(a.contains_rect(&Rect::new(2, 2, 8, 8)));
        assert!(!a.contains_rect(&Rect::new(2, 2, 9, 8)));
        assert!(!Rect::EMPTY.contains_rect(&a));
    }

    #[test]
    fn translate_and_inset() {
        let r = Rect::new(10, 20, 30, 40);
        assert_eq!(r.translate(-15, 5), Rect::new(-5, 25, 30, 40));
        assert_eq!(r.inset(5), Rect::new(15, 25, 20, 30));
        assert_eq!(r.inset(-5), Rect::new(5, 15, 40, 50));
        assert_eq!(r.inset(15), Rect::EMPTY, "the width is used up");
        assert_eq!(r.inset(MAX), Rect::EMPTY);
    }

    #[test]
    fn nothing_overflows_at_the_limits() {
        let huge = Rect::new(MAX - 1, MAX - 1, MAX, MAX);
        assert_eq!(huge.right(), i64::from(MAX) * 2 - 1);
        assert_eq!(huge.area(), i64::from(MAX) * i64::from(MAX));
        assert!(huge.contains(MAX, MAX));
        let far = Rect::new(MIN, MIN, MAX, MAX);
        assert_eq!(huge.intersect(&far), Rect::EMPTY);
        // Their bounding box is wider than an `i32` can say: it is cut short, not wrapped.
        let both = huge.union(&far);
        assert_eq!((both.x, both.y, both.width, both.height), (MIN, MIN, MAX, MAX));
        assert_eq!(huge.translate(MAX, MAX).x, MAX);
        assert_eq!(far.translate(MIN, MIN).y, MIN);
        assert_eq!(far.inset(MIN).x, MIN);
        let all = Rect::from_edges(i64::MIN, i64::MIN, i64::MAX, i64::MAX);
        assert_eq!((all.x, all.width), (MIN, MAX));
        assert!(huge.intersect(&Rect::new(0, 0, MAX, MAX)).contains(MAX - 1, MAX - 1));
    }
}
