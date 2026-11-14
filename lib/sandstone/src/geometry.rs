// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sizes, lengths and limits: the numbers layout works with (note 0023, section 5).
//!
//! Everything is in *logical pixels*: the window's canvas multiplies by its scale when it
//! draws, so a layout is the same at 1x and 2x.

pub use pbr_draw::Rect;

/// Larger than any window can be: what "no limit" means to layout. Small enough that adding
/// two of them can't overflow an `i32`.
pub const UNBOUNDED: i32 = 1 << 24;

/// A width and a height, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    /// Across.
    pub width: i32,
    /// Down.
    pub height: i32,
}

impl Size {
    /// Nothing.
    pub const ZERO: Size = Size { width: 0, height: 0 };

    /// A size.
    pub const fn new(width: i32, height: i32) -> Size {
        Size { width, height }
    }

    /// The bigger of each side.
    pub fn max(self, other: Size) -> Size {
        Size::new(self.width.max(other.width), self.height.max(other.height))
    }

    /// The smaller of each side.
    pub fn min(self, other: Size) -> Size {
        Size::new(self.width.min(other.width), self.height.min(other.height))
    }
}

/// A point, in logical pixels from the window's content corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    /// Across.
    pub x: i32,
    /// Down.
    pub y: i32,
}

impl Point {
    /// A point.
    pub const fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }
}

/// How much of one side a widget wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Length {
    /// As much as its content needs (the default).
    #[default]
    Shrink,
    /// All the room there is, shared equally with the other `Fill`s of its row or column.
    Fill,
    /// Room shared out in proportion: `Portion(2)` gets twice what `Fill` (which is
    /// `Portion(1)`) does.
    Portion(u16),
    /// Exactly this many logical pixels (if the room allows).
    Fixed(i32),
}

impl Length {
    /// How big a share of the room left over it wants: 0 for `Shrink` and `Fixed`.
    pub fn share(self) -> u32 {
        match self {
            Length::Fill => 1,
            Length::Portion(n) => u32::from(n),
            Length::Shrink | Length::Fixed(_) => 0,
        }
    }
}

/// Room around a widget's content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Padding {
    /// Above.
    pub top: i32,
    /// To the right.
    pub right: i32,
    /// Below.
    pub bottom: i32,
    /// To the left.
    pub left: i32,
}

impl Padding {
    /// None.
    pub const ZERO: Padding = Padding { top: 0, right: 0, bottom: 0, left: 0 };

    /// The same on every side.
    pub const fn all(n: i32) -> Padding {
        Padding { top: n, right: n, bottom: n, left: n }
    }

    /// `v` above and below, `h` left and right.
    pub const fn symmetric(v: i32, h: i32) -> Padding {
        Padding { top: v, right: h, bottom: v, left: h }
    }

    /// Left and right together.
    pub const fn across(&self) -> i32 {
        self.left + self.right
    }

    /// Top and bottom together.
    pub const fn down(&self) -> i32 {
        self.top + self.bottom
    }
}

/// Where something smaller than its room goes in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Left, or top (the default).
    #[default]
    Start,
    /// The middle.
    Center,
    /// Right, or bottom.
    End,
}

impl Align {
    /// Where a thing `size` long starts in room `room` long.
    pub fn offset(self, room: i32, size: i32) -> i32 {
        let spare = (room - size).max(0);
        match self {
            Align::Start => 0,
            Align::Center => spare / 2,
            Align::End => spare,
        }
    }
}

/// The smallest and largest size a widget may take: what its parent allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// At least this.
    pub min: Size,
    /// At most this.
    pub max: Size,
}

impl Limits {
    /// Limits from `min` to `max` (a `max` smaller than `min` is raised to it).
    pub fn new(min: Size, max: Size) -> Limits {
        let min = Size::new(min.width.max(0), min.height.max(0));
        Limits { min, max: max.max(min) }
    }

    /// Anything from nothing up to `max`.
    pub fn loose(max: Size) -> Limits {
        Limits::new(Size::ZERO, max)
    }

    /// Exactly `size`.
    pub fn exact(size: Size) -> Limits {
        Limits::new(size, size)
    }

    /// The same limits, with the width narrowed by a widget's wish for it: `Fixed` pins it
    /// (within the limits), `Fill` and `Portion` take all the room there is (when there is a
    /// limit), and `Shrink` leaves the limits as they are.
    pub fn width(self, length: Length) -> Limits {
        match length {
            Length::Shrink => self,
            Length::Fixed(w) => {
                let w = w.clamp(self.min.width, self.max.width);
                Limits::new(Size::new(w, self.min.height), Size::new(w, self.max.height))
            }
            Length::Fill | Length::Portion(_) if self.max.width < UNBOUNDED => {
                Limits::new(Size::new(self.max.width, self.min.height), self.max)
            }
            Length::Fill | Length::Portion(_) => self,
        }
    }

    /// As [`Limits::width`], for the height.
    pub fn height(self, length: Length) -> Limits {
        match length {
            Length::Shrink => self,
            Length::Fixed(h) => {
                let h = h.clamp(self.min.height, self.max.height);
                Limits::new(Size::new(self.min.width, h), Size::new(self.max.width, h))
            }
            Length::Fill | Length::Portion(_) if self.max.height < UNBOUNDED => {
                Limits::new(Size::new(self.min.width, self.max.height), self.max)
            }
            Length::Fill | Length::Portion(_) => self,
        }
    }

    /// The limits for what is inside `padding`.
    pub fn shrink(self, padding: Padding) -> Limits {
        let less = |n: i32, by: i32| if n >= UNBOUNDED { n } else { (n - by).max(0) };
        Limits::new(
            Size::new(
                less(self.min.width, padding.across()),
                less(self.min.height, padding.down()),
            ),
            Size::new(
                less(self.max.width, padding.across()),
                less(self.max.height, padding.down()),
            ),
        )
    }

    /// `size`, brought within the limits.
    pub fn resolve(self, size: Size) -> Size {
        Size::new(
            size.width.clamp(self.min.width, self.max.width),
            size.height.clamp(self.min.height, self.max.height),
        )
    }
}
