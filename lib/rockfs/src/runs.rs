// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Runs of consecutive disk blocks, and sets of them.
//!
//! A **run** is "blocks 5000 to 5099": a start and a length. Allocating, freeing and
//! remembering which blocks a half-finished save has written are all most easily described
//! as operations on runs, which stay small even for a very large file.

use alloc::vec::Vec;

/// Consecutive blocks: `start`, `start + 1`, ... `start + len - 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// The first block.
    pub start: u64,
    /// How many blocks. Never 0 in a [`RunSet`].
    pub len: u64,
}

impl Run {
    /// One past the last block. Block numbers are checked against the volume's size long before
    /// they get here, so this cannot wrap; `saturating_add` keeps even a bug from panicking.
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
}

/// A set of blocks kept as sorted, non-touching runs.
///
/// `RockFs` keeps two for each file with changes not yet committed (note 0014, section 6):
/// the blocks those changes wrote (nothing on the disk points at them yet, so they can be
/// freed at once if overwritten), and the old blocks the commit will release.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSet {
    runs: Vec<Run>,
}

impl RunSet {
    /// The runs, in order.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Adds `r`, joining it to neighbours it touches or overlaps.
    pub fn insert(&mut self, r: Run) {
        if r.len == 0 {
            return;
        }
        // The first run that ends at or after `r.start`: everything before it is clear of `r`.
        let first = self.runs.partition_point(|x| x.end() < r.start);
        let mut start = r.start;
        let mut end = r.end();
        let mut last = first;
        while last < self.runs.len() && self.runs[last].start <= end {
            start = start.min(self.runs[last].start);
            end = end.max(self.runs[last].end());
            last += 1;
        }
        self.runs.splice(first..last, [Run { start, len: end - start }]);
    }

    /// Removes the blocks of `r` from the set. Returns (the blocks that were in the set,
    /// the blocks of `r` that were not), each as runs in order.
    pub fn take(&mut self, r: Run) -> (Vec<Run>, Vec<Run>) {
        let mut inside = Vec::new();
        let mut outside = Vec::new();
        let mut cursor = r.start;
        let end = r.end();
        let first = self.runs.partition_point(|x| x.end() <= r.start);
        let mut last = first;
        let mut keep: Vec<Run> = Vec::new();
        while last < self.runs.len() && self.runs[last].start < end {
            let x = self.runs[last];
            let (lo, hi) = (x.start.max(r.start), x.end().min(end));
            if lo > cursor {
                outside.push(Run { start: cursor, len: lo - cursor });
            }
            inside.push(Run { start: lo, len: hi - lo });
            cursor = hi;
            // The parts of `x` sticking out on either side of `r` stay in the set.
            if x.start < lo {
                keep.push(Run { start: x.start, len: lo - x.start });
            }
            if x.end() > hi {
                keep.push(Run { start: hi, len: x.end() - hi });
            }
            last += 1;
        }
        if cursor < end {
            outside.push(Run { start: cursor, len: end - cursor });
        }
        self.runs.splice(first..last, keep);
        (inside, outside)
    }
}

/// Sorts `runs` and joins those that touch, so freeing a many-piece list makes few calls.
pub fn merge_runs(mut runs: Vec<Run>) -> Vec<Run> {
    runs.sort_by_key(|r| r.start);
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs {
        match out.last_mut() {
            Some(last) if last.end() >= r.start => {
                let end = last.end().max(r.end());
                last.len = end - last.start;
            }
            _ if r.len == 0 => {}
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;

    fn set_of(s: &RunSet) -> BTreeSet<u64> {
        s.runs().iter().flat_map(|r| r.start..r.end()).collect()
    }

    /// Checks `RunSet` against a plain set of block numbers, over many random operations.
    #[test]
    fn matches_a_model() {
        let mut seed = 12345u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let mut s = RunSet::default();
        let mut model = BTreeSet::new();
        for _ in 0..3000 {
            let r = Run { start: rnd(60), len: 1 + rnd(8) };
            if rnd(2) == 0 {
                s.insert(r);
                model.extend(r.start..r.end());
            } else {
                let (inside, outside) = s.take(r);
                let want_in: BTreeSet<u64> =
                    (r.start..r.end()).filter(|b| model.contains(b)).collect();
                let want_out: BTreeSet<u64> =
                    (r.start..r.end()).filter(|b| !model.contains(b)).collect();
                let got_in: BTreeSet<u64> = inside.iter().flat_map(|x| x.start..x.end()).collect();
                let got_out: BTreeSet<u64> =
                    outside.iter().flat_map(|x| x.start..x.end()).collect();
                assert_eq!((got_in, got_out), (want_in, want_out));
                for b in r.start..r.end() {
                    model.remove(&b);
                }
            }
            assert_eq!(set_of(&s), model);
            // The runs stay sorted and never touch.
            for w in s.runs().windows(2) {
                assert!(w[0].end() < w[1].start);
            }
        }
    }

    #[test]
    fn merging() {
        let m = merge_runs(alloc::vec![
            Run { start: 10, len: 2 },
            Run { start: 0, len: 3 },
            Run { start: 3, len: 1 },
            Run { start: 11, len: 5 },
        ]);
        assert_eq!(m, alloc::vec![Run { start: 0, len: 4 }, Run { start: 10, len: 6 }]);
    }
}
