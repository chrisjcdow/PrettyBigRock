// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The arithmetic behind device access (docs/design/0010-device-access.md, section 2): ranges
//! of physical addresses, I/O ports or interrupt numbers; slicing one range out of another;
//! the set of ranges the kernel keeps for itself; and the queue of free interrupt vectors.
//!
//! It is plain logic with no kernel in it, so it is tested on the host, against a simple model,
//! with random sequences of operations. Every operation checks for overflow: the numbers in a
//! slice request come from a program.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// A run of `len` units starting at `start`: bytes of physical memory, I/O ports or interrupt
/// numbers. A range is never empty and never wraps around the end of the numbers: [`Range::new`]
/// is the only way to make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    start: u64,
    len: u64,
}

/// Why a range or a slice was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The length is zero.
    Empty,
    /// The range runs past the largest number.
    Overflow,
    /// An offset or length isn't a multiple of the alignment.
    Unaligned,
    /// A slice reaches outside the range it is cut from.
    Outside,
}

impl Range {
    /// The range `[start, start + len)`.
    pub fn new(start: u64, len: u64) -> Result<Range, Error> {
        if len == 0 {
            return Err(Error::Empty);
        }
        start.checked_add(len).ok_or(Error::Overflow)?;
        Ok(Range { start, len })
    }

    /// The first number in the range.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// How many numbers it holds.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Always false: a range is never empty. (Provided because `len` is.)
    pub fn is_empty(&self) -> bool {
        false
    }

    /// One past the last number.
    pub fn end(&self) -> u64 {
        self.start + self.len
    }

    /// True if every number in `other` is in this range.
    pub fn contains(&self, other: &Range) -> bool {
        other.start >= self.start && other.end() <= self.end()
    }

    /// True if the two ranges share a number.
    pub fn overlaps(&self, other: &Range) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    /// The part of this range from `offset`, `length` long. `align` is the unit the offset and
    /// length must be multiples of (4096 for memory, 1 for ports and interrupts).
    pub fn slice(&self, offset: u64, length: u64, align: u64) -> Result<Range, Error> {
        if length == 0 {
            return Err(Error::Empty);
        }
        if align == 0
            || !offset.is_multiple_of(align)
            || !length.is_multiple_of(align)
            || !self.start.is_multiple_of(align)
        {
            return Err(Error::Unaligned);
        }
        let end = offset.checked_add(length).ok_or(Error::Outside)?;
        if end > self.len {
            return Err(Error::Outside);
        }
        Ok(Range { start: self.start + offset, len: length })
    }
}

/// A set of ranges, kept sorted and merged. The kernel's own ranges (RAM, the interrupt
/// controllers, its console) are one, and a slice may not touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeSet {
    ranges: Vec<Range>,
}

impl RangeSet {
    /// An empty set.
    pub const fn new() -> Self {
        RangeSet { ranges: Vec::new() }
    }

    /// Adds a range, merging it with any it touches.
    pub fn add(&mut self, r: Range) {
        let mut start = r.start;
        let mut end = r.end();
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut placed = false;
        for &x in &self.ranges {
            if x.end() < start {
                out.push(x);
            } else if x.start > end {
                if !placed {
                    out.push(Range { start, len: end - start });
                    placed = true;
                }
                out.push(x);
            } else {
                // Overlapping or touching: absorb it.
                start = start.min(x.start);
                end = end.max(x.end());
            }
        }
        if !placed {
            out.push(Range { start, len: end - start });
        }
        self.ranges = out;
    }

    /// True if any number in `r` is in the set.
    pub fn overlaps(&self, r: &Range) -> bool {
        // Sorted and disjoint, so the first range that ends after r starts is the only one
        // worth looking at.
        let i = self.ranges.partition_point(|x| x.end() <= r.start);
        self.ranges.get(i).is_some_and(|x| x.overlaps(r))
    }

    /// The ranges, in order.
    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }
}

/// The free interrupt vectors, as a queue: a vector given back goes to the back and is not used
/// again until all the others have been (docs/design/0010-device-access.md, section 4), so a
/// device still firing at a vector its dead driver owned won't disturb the next owner.
#[derive(Clone, Debug)]
pub struct VectorPool {
    free: VecDeque<u32>,
    first: u32,
    last: u32,
}

impl VectorPool {
    /// A pool of the vectors `first` to `last` inclusive, all free.
    pub fn new(first: u32, last: u32) -> Self {
        VectorPool { free: (first..=last).collect(), first, last }
    }

    /// Takes the vector that has been free longest.
    pub fn take(&mut self) -> Option<u32> {
        self.free.pop_front()
    }

    /// Gives a vector back. A vector outside the pool, or already free, is ignored (a double
    /// free can't hand the same vector to two owners).
    pub fn give(&mut self, v: u32) {
        if (self.first..=self.last).contains(&v) && !self.free.contains(&v) {
            self.free.push_back(v);
        }
    }

    /// How many vectors are free.
    pub fn available(&self) -> usize {
        self.free.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator, so the random tests repeat exactly.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    #[test]
    fn new_refuses_empty_and_wrapping_ranges() {
        assert_eq!(Range::new(5, 0), Err(Error::Empty));
        assert_eq!(Range::new(u64::MAX, 2), Err(Error::Overflow));
        // A range must end at a number that can be written down: the last one is `u64::MAX`.
        assert_eq!(Range::new(u64::MAX - 1, 2), Err(Error::Overflow));
        assert_eq!(Range::new(u64::MAX - 2, 2).unwrap().end(), u64::MAX);
        assert!(Range::new(0, u64::MAX).is_ok());
    }

    #[test]
    fn slices_stay_inside() {
        let r = Range::new(0x1000_0000, 0x10000).unwrap();
        assert_eq!(r.slice(0, 0x10000, 4096), Ok(r));
        assert_eq!(r.slice(0x1000, 0x2000, 4096).unwrap().start(), 0x1000_1000);
        assert_eq!(r.slice(0x1000, 0x10000, 4096), Err(Error::Outside));
        assert_eq!(r.slice(0x10000, 0x1000, 4096), Err(Error::Outside));
        assert_eq!(r.slice(0, 0, 4096), Err(Error::Empty));
        assert_eq!(r.slice(1, 0x1000, 4096), Err(Error::Unaligned));
        assert_eq!(r.slice(0, 0x800, 4096), Err(Error::Unaligned));
        assert_eq!(r.slice(0, 1, 0), Err(Error::Unaligned));
        // Numbers a program chose to make the sum wrap.
        assert_eq!(r.slice(u64::MAX - 0xFFF, 0x1000, 1), Err(Error::Outside));
        assert_eq!(r.slice(8, u64::MAX, 1), Err(Error::Outside));
    }

    #[test]
    fn a_range_that_starts_unaligned_cannot_be_sliced_by_page() {
        let r = Range::new(0x1800, 0x4000).unwrap();
        assert_eq!(r.slice(0, 0x1000, 4096), Err(Error::Unaligned));
        assert!(r.slice(0, 0x1000, 1).is_ok());
    }

    #[test]
    fn contains_and_overlaps_at_the_edges() {
        let a = Range::new(10, 10).unwrap(); // 10..20
        assert!(a.contains(&Range::new(10, 10).unwrap()));
        assert!(!a.contains(&Range::new(9, 10).unwrap()));
        assert!(!a.contains(&Range::new(11, 10).unwrap()));
        assert!(!a.overlaps(&Range::new(20, 5).unwrap()));
        assert!(!a.overlaps(&Range::new(5, 5).unwrap()));
        assert!(a.overlaps(&Range::new(19, 5).unwrap()));
        assert!(a.overlaps(&Range::new(5, 6).unwrap()));
        assert!(a.overlaps(&Range::new(0, 100).unwrap()));
    }

    #[test]
    fn range_sets_merge_touching_ranges() {
        let mut s = RangeSet::new();
        s.add(Range::new(10, 5).unwrap());
        s.add(Range::new(20, 5).unwrap());
        s.add(Range::new(15, 5).unwrap()); // fills the gap exactly
        assert_eq!(s.ranges(), &[Range::new(10, 15).unwrap()]);
        s.add(Range::new(0, 3).unwrap());
        s.add(Range::new(100, 1).unwrap());
        assert_eq!(s.ranges().len(), 3);
        s.add(Range::new(2, 200).unwrap()); // swallows everything
        assert_eq!(s.ranges(), &[Range::new(0, 202).unwrap()]);
    }

    #[test]
    fn range_sets_against_a_model() {
        // The model is one flag per number in a small space.
        const N: u64 = 200;
        let mut rng = Lcg(1);
        for _ in 0..200 {
            let mut set = RangeSet::new();
            let mut model = [false; N as usize];
            for _ in 0..30 {
                let start = rng.next() % N;
                let len = 1 + rng.next() % 12;
                let len = len.min(N - start);
                set.add(Range::new(start, len).unwrap());
                for i in start..start + len {
                    model[i as usize] = true;
                }
                // The set is sorted, disjoint and not touching.
                for w in set.ranges().windows(2) {
                    assert!(w[0].end() < w[1].start());
                }
                for _ in 0..10 {
                    let qs = rng.next() % N;
                    let ql = (1 + rng.next() % 10).min(N - qs);
                    let want = (qs..qs + ql).any(|i| model[i as usize]);
                    assert_eq!(set.overlaps(&Range::new(qs, ql).unwrap()), want);
                }
            }
        }
    }

    #[test]
    fn vectors_come_back_round_the_queue() {
        let mut p = VectorPool::new(0x30, 0x33);
        let a = p.take().unwrap();
        let b = p.take().unwrap();
        assert_eq!((a, b), (0x30, 0x31));
        p.give(a);
        // The vector just given back is used last.
        assert_eq!(p.take(), Some(0x32));
        assert_eq!(p.take(), Some(0x33));
        assert_eq!(p.take(), Some(0x30));
        assert_eq!(p.take(), None);
    }

    #[test]
    fn a_vector_cannot_be_given_back_twice_or_from_outside() {
        let mut p = VectorPool::new(1, 3);
        let v = p.take().unwrap();
        p.give(v);
        p.give(v);
        p.give(99);
        p.give(0);
        assert_eq!(p.available(), 3);
        let mut seen = alloc::vec::Vec::new();
        while let Some(v) = p.take() {
            assert!(!seen.contains(&v));
            seen.push(v);
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn vector_pool_never_loses_or_duplicates() {
        let mut rng = Lcg(7);
        let mut p = VectorPool::new(0x30, 0x3F);
        let mut held: Vec<u32> = Vec::new();
        for _ in 0..5000 {
            if rng.next().is_multiple_of(2) {
                if let Some(v) = p.take() {
                    assert!(!held.contains(&v));
                    held.push(v);
                }
            } else if !held.is_empty() {
                let i = (rng.next() as usize) % held.len();
                p.give(held.swap_remove(i));
            }
            assert_eq!(p.available() + held.len(), 16);
        }
    }
}
