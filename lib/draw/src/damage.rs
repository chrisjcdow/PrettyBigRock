// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Keeping track of which parts of a picture have changed.
//!
//! Showing a picture costs time for every pixel: a display driver copies it to the screen
//! (`present`, docs/design/0020-screens-and-virtio-gpu.md) and the display server draws
//! windows over each other (`commit`, note 0022). Both take a list of the rectangles that
//! changed, at most 64, and only do the work there. *Damage* is the usual name for that list:
//! the parts of the old picture that are no longer right.

use crate::Rect;

/// The most rectangles kept: what `present` and `commit` can carry.
pub const MAX_RECTS: usize = 64;

/// Two rectangles are joined into one when the one rectangle around both holds at most this
/// many pixels more than the two hold separately. Joining saves a rectangle and costs those
/// extra pixels being shown again; a few hundred pixels cost less than a rectangle does.
pub const MERGE_WASTE: i64 = 1024;

/// The rectangles of a picture that have changed.
///
/// [`Damage::add`] takes rectangles in any order, overlapping or not. What it keeps always
/// covers every pixel that was added, and sometimes a little more: rectangles that overlap or
/// lie close together are joined (see [`MERGE_WASTE`]), and if there would be more than
/// [`MAX_RECTS`], they are all replaced by the one rectangle around them.
///
/// It holds its rectangles in a fixed array, so it never needs memory from the heap.
#[derive(Clone, Debug)]
pub struct Damage {
    rects: [Rect; MAX_RECTS],
    count: usize,
}

impl Default for Damage {
    fn default() -> Self {
        Self::new()
    }
}

impl Damage {
    /// Nothing changed yet.
    pub const fn new() -> Damage {
        Damage { rects: [Rect::EMPTY; MAX_RECTS], count: 0 }
    }

    /// Whether nothing has changed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The rectangles that changed. None of them is empty.
    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.count]
    }

    /// The one rectangle around everything that changed ([`Rect::EMPTY`] if nothing did).
    pub fn bounds(&self) -> Rect {
        self.rects().iter().fold(Rect::EMPTY, |all, r| all.union(r))
    }

    /// Forgets everything, ready for the next picture.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Records that the pixels of `rect` changed. An empty rectangle adds nothing.
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        let mut new = rect;
        // Join `new` with any rectangle it is cheap to join with. The joined rectangle is
        // bigger, so it may now be worth joining with one that was passed over before: start
        // again each time, until nothing more joins. Each join removes a rectangle, so this
        // ends.
        'again: loop {
            for i in 0..self.count {
                let old = self.rects[i];
                let both = old.union(&new);
                if both.area() - old.area() - new.area() <= MERGE_WASTE {
                    // Remove `old` by moving the last one into its place.
                    self.count -= 1;
                    self.rects[i] = self.rects[self.count];
                    new = both;
                    continue 'again;
                }
            }
            break;
        }
        if self.count == MAX_RECTS {
            // No room: everything becomes one rectangle.
            new = self.bounds().union(&new);
            self.count = 0;
        }
        self.rects[self.count] = new;
        self.count += 1;
    }

    /// Records everything `other` has recorded.
    pub fn add_all(&mut self, other: &Damage) {
        for r in other.rects() {
            self.add(*r);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn nothing_and_empty_rectangles() {
        let mut d = Damage::new();
        assert!(d.is_empty() && d.rects().is_empty());
        assert_eq!(d.bounds(), Rect::EMPTY);
        d.add(Rect::EMPTY);
        d.add(Rect::new(5, 5, 0, 9));
        d.add(Rect::new(5, 5, 9, -1));
        assert!(d.is_empty());
        d.add(Rect::new(1, 2, 3, 4));
        assert_eq!(d.rects(), [Rect::new(1, 2, 3, 4)]);
        d.clear();
        assert!(d.is_empty());
    }

    #[test]
    fn overlapping_and_nearby_rectangles_are_joined() {
        let mut d = Damage::new();
        d.add(Rect::new(0, 0, 100, 20));
        d.add(Rect::new(50, 10, 100, 20)); // overlaps: joined (the box wastes 500 pixels)
        assert_eq!(d.rects(), [Rect::new(0, 0, 150, 30)]);
        d.add(Rect::new(10, 5, 5, 5)); // inside: nothing new
        assert_eq!(d.rects(), [Rect::new(0, 0, 150, 30)]);
        d.add(Rect::new(0, 32, 150, 11)); // the next line of text, two pixels below
        assert_eq!(d.rects(), [Rect::new(0, 0, 150, 43)]);
        d.add(Rect::new(1000, 1000, 10, 10)); // far away: kept apart
        assert_eq!(d.rects().len(), 2);
        assert_eq!(d.bounds(), Rect::new(0, 0, 1010, 1010));
    }

    #[test]
    fn a_cross_is_not_joined_into_a_big_square() {
        let mut d = Damage::new();
        d.add(Rect::new(0, 450, 1000, 10));
        d.add(Rect::new(450, 0, 10, 1000));
        assert_eq!(d.rects().len(), 2);
    }

    #[test]
    fn joining_can_make_more_joining_worthwhile() {
        let mut d = Damage::new();
        // Two rectangles too far apart to join, then one that bridges them.
        d.add(Rect::new(0, 0, 10, 100));
        d.add(Rect::new(40, 0, 10, 100));
        assert_eq!(d.rects().len(), 2);
        d.add(Rect::new(10, 0, 22, 100));
        assert_eq!(d.rects(), [Rect::new(0, 0, 50, 100)]);
    }

    #[test]
    fn past_the_limit_everything_becomes_one_rectangle() {
        let mut d = Damage::new();
        for i in 0..MAX_RECTS as i32 {
            d.add(Rect::new(i * 1000, i * 1000, 10, 10));
        }
        assert_eq!(d.rects().len(), MAX_RECTS);
        d.add(Rect::new(-500, 70_000, 10, 10));
        assert_eq!(d.rects(), [Rect::new(-500, 0, 63_510, 70_010)]);
        // And it carries on from there.
        d.add(Rect::new(200_000, 0, 1, 1));
        assert_eq!(d.rects().len(), 2);
    }

    #[test]
    fn add_all_and_huge_rectangles() {
        let (mut a, mut b) = (Damage::new(), Damage::new());
        a.add(Rect::new(0, 0, 5, 5));
        b.add(Rect::new(500, 500, 5, 5));
        b.add(Rect::new(2, 2, 5, 5));
        a.add_all(&b);
        assert_eq!(a.rects().len(), 2);
        assert_eq!(a.bounds(), Rect::new(0, 0, 505, 505));
        // Rectangles at the limits of the numbers don't overflow anything.
        let mut d = Damage::new();
        d.add(Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX));
        d.add(Rect::new(i32::MAX - 5, i32::MAX - 5, i32::MAX, i32::MAX));
        d.add(Rect::new(0, 0, i32::MAX, i32::MAX));
        assert!(!d.is_empty());
    }

    /// A small random number generator (xorshift), so that the test is the same every run.
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> i32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n) as i32
        }
    }

    /// Against a model: a grid with a mark for every pixel added. Whatever is added, in
    /// whatever order, every marked pixel is inside some rectangle, nothing is outside the
    /// bounding box of what was added, and joining never wastes more than it is allowed.
    #[test]
    fn it_always_covers_what_was_added_and_little_more() {
        const SIDE: i32 = 256;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..300 {
            let mut model = vec![false; (SIDE * SIDE) as usize];
            let mut d = Damage::new();
            let (mut all, mut added, mut collapsed) = (Rect::EMPTY, 0i64, false);
            // Some rounds have few big rectangles, some many small ones (past the limit).
            let (count, size) = match round % 3 {
                0 => (rng.below(12) + 1, 120),
                1 => (rng.below(80) + 1, 30),
                _ => (rng.below(400) + 1, 6),
            };
            for _ in 0..count {
                let r = Rect::new(
                    rng.below(SIDE as u64),
                    rng.below(SIDE as u64),
                    rng.below(size),
                    rng.below(size),
                )
                .intersect(&Rect::new(0, 0, SIDE, SIDE));
                let before = d.rects().len();
                d.add(r);
                collapsed |= before == MAX_RECTS && d.rects().len() == 1;
                all = all.union(&r);
                added += r.area();
                for y in r.y..r.y + r.height.max(0) {
                    for x in r.x..r.x + r.width.max(0) {
                        model[(y * SIDE + x) as usize] = true;
                    }
                }
                assert!(d.rects().len() <= MAX_RECTS);
            }
            let rects: Vec<Rect> = d.rects().to_vec();
            for y in 0..SIDE {
                for x in 0..SIDE {
                    if model[(y * SIDE + x) as usize] {
                        assert!(rects.iter().any(|r| r.contains(x, y)), "round {round}: {x},{y}");
                    }
                }
            }
            assert_eq!(d.bounds(), all, "round {round}");
            assert!(rects.iter().all(|r| !r.is_empty() && all.contains_rect(r)));
            if !collapsed {
                // Each join wastes at most MERGE_WASTE, and there is at most one join for
                // each rectangle added.
                let kept: i64 = rects.iter().map(Rect::area).sum();
                assert!(kept <= added + MERGE_WASTE * i64::from(count), "round {round}");
            }
        }
    }
}
