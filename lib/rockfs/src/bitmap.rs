// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The block bitmap (ROCKFS.md, "The block bitmap"): one bit per block of the volume, 1 meaning
//! "in use".
//!
//! The whole bitmap is kept in memory while the volume is mounted, so allocating and freeing
//! cost no disk reads. Only the bitmap blocks that changed are written back, and only
//! occasionally (at unmount at the latest): if the machine stops first, the volume is still
//! marked "in use" and the check at the next mount rebuilds the bitmap from what files really
//! use (note 0016, section 2). That is why a change to a bit here is not a change to the disk.

use crate::layout::{BITS_PER_BITMAP_BLOCK, BLOCK};
use crate::runs::Run;
use alloc::vec;
use alloc::vec::Vec;

/// The in-memory block bitmap.
pub struct Bitmap {
    /// `bitmap_blocks * BLOCK` bytes; bit `n` is `bits[n / 8] >> (n % 8) & 1`.
    bits: Vec<u8>,
    /// Blocks in the volume. Bits at and past this number are padding and always read as used.
    block_count: u64,
    /// For each bitmap block, whether it differs from the disk.
    dirty: Vec<bool>,
    /// Free blocks, kept in step with the bits.
    free: u64,
    /// Where the last allocation ended: the next one starts searching here, so a file written in
    /// pieces ends up in one place.
    hint: u64,
}

impl Bitmap {
    /// Wraps the bitmap bytes read from the disk. Padding bits past `block_count` are forced to
    /// 1 (ROCKFS.md says they are, but a damaged disk might not agree and they must never be
    /// handed out).
    pub fn new(mut bits: Vec<u8>, block_count: u64) -> Bitmap {
        let need = block_count.div_ceil(BITS_PER_BITMAP_BLOCK) as usize * BLOCK;
        if bits.len() < need {
            bits.resize(need, 0xFF);
        }
        let mut bm =
            Bitmap { dirty: vec![false; bits.len() / BLOCK], bits, block_count, free: 0, hint: 0 };
        let mut n = block_count;
        while n < bm.bits.len() as u64 * 8 {
            bm.bits[(n >> 3) as usize] |= 1 << (n & 7);
            n += 1;
        }
        bm.free = bm.count_free();
        bm
    }

    /// A bitmap with nothing in use except the padding past `block_count`.
    pub fn empty(block_count: u64) -> Bitmap {
        let blocks = block_count.div_ceil(BITS_PER_BITMAP_BLOCK) as usize;
        Bitmap::new(vec![0; blocks * BLOCK], block_count)
    }

    fn count_free(&self) -> u64 {
        let mut free = 0;
        for (i, &byte) in self.bits.iter().enumerate() {
            let base = i as u64 * 8;
            if base >= self.block_count {
                break;
            }
            if base + 8 <= self.block_count {
                free += u64::from(byte.count_zeros());
            } else {
                for bit in 0..(self.block_count - base) {
                    free += u64::from(byte >> bit & 1 == 0);
                }
            }
        }
        free
    }

    /// Free blocks.
    pub fn free(&self) -> u64 {
        self.free
    }

    /// Whether block `n` is in use. Blocks outside the volume count as in use.
    pub fn is_used(&self, n: u64) -> bool {
        n >= self.block_count || self.bits[(n >> 3) as usize] >> (n & 7) & 1 == 1
    }

    fn set(&mut self, n: u64, used: bool) {
        let byte = &mut self.bits[(n >> 3) as usize];
        if used {
            *byte |= 1 << (n & 7);
        } else {
            *byte &= !(1 << (n & 7));
        }
        self.dirty[(n / BITS_PER_BITMAP_BLOCK) as usize] = true;
    }

    /// The first block in `from..to` that is used (`used`) or free (`!used`).
    ///
    /// Whole bytes that are all 1 or all 0 are skipped at once, because on a big disk most of
    /// the bitmap is one or the other.
    pub fn find(&self, mut from: u64, to: u64, used: bool) -> Option<u64> {
        let to = to.min(self.block_count);
        let skip = if used { 0x00 } else { 0xFF };
        while from < to {
            if from & 7 == 0 && self.bits[(from >> 3) as usize] == skip {
                from += 8;
                continue;
            }
            if self.is_used(from) == used {
                return Some(from);
            }
            from += 1;
        }
        None
    }

    /// Marks the blocks of `run` in use. Returns `false`, changing nothing, if any is outside
    /// the volume or already in use: that means two things claim one block, and the caller
    /// treats it as damage.
    pub fn mark_used(&mut self, run: Run) -> bool {
        self.mark(run, true)
    }

    /// Marks the blocks of `run` free. Returns `false`, changing nothing, if any is outside the
    /// volume or already free.
    pub fn mark_free(&mut self, run: Run) -> bool {
        self.mark(run, false)
    }

    fn mark(&mut self, run: Run, used: bool) -> bool {
        match run.start.checked_add(run.len) {
            Some(end) if end <= self.block_count => {}
            _ => return false,
        }
        if (run.start..run.end()).any(|n| self.is_used(n) == used) {
            return false;
        }
        for n in run.start..run.end() {
            self.set(n, used);
        }
        if used {
            self.free -= run.len;
        } else {
            self.free += run.len;
        }
        true
    }

    /// Finds `count` free blocks inside `lo..hi` and marks them used. Prefers one run (so files
    /// need few extents); if no run is long enough it gathers several. Returns `None`, changing
    /// nothing, if there are not `count` free blocks there.
    pub fn allocate(&mut self, count: u64, lo: u64, hi: u64) -> Option<Vec<Run>> {
        let hi = hi.min(self.block_count);
        if count == 0 || lo >= hi {
            return None;
        }
        let start = if (lo..hi).contains(&self.hint) { self.hint } else { lo };
        // Look from the hint to the end, then from the start up to the hint.
        let spans = [(start, hi), (lo, start)];
        let mut found: Option<Run> = None;
        'single: for &(from, to) in &spans {
            let mut at = from;
            while let Some(s) = self.find(at, to, false) {
                let e = self.find(s, to, true).unwrap_or(to);
                if e - s >= count {
                    found = Some(Run { start: s, len: count });
                    break 'single;
                }
                at = e;
            }
        }
        let runs = match found {
            Some(r) => vec![r],
            None => {
                // No single run is long enough: gather pieces in the same order.
                let mut runs = Vec::new();
                let mut need = count;
                'gather: for &(from, to) in &spans {
                    let mut at = from;
                    while let Some(s) = self.find(at, to, false) {
                        let e = self.find(s, to, true).unwrap_or(to);
                        let take = (e - s).min(need);
                        runs.push(Run { start: s, len: take });
                        need -= take;
                        if need == 0 {
                            break 'gather;
                        }
                        at = e;
                    }
                }
                if need != 0 {
                    return None;
                }
                runs
            }
        };
        for r in &runs {
            // These blocks were just found free inside the volume, so this cannot fail.
            let ok = self.mark_used(*r);
            debug_assert!(ok);
        }
        self.hint = runs.last().map_or(self.hint, Run::end);
        Some(runs)
    }

    /// Replaces every bit (used by a repair, which works out the true bitmap from scratch).
    pub fn replace(&mut self, bits: Vec<u8>) {
        let mut fresh = Bitmap::new(bits, self.block_count);
        fresh.hint = self.hint;
        fresh.dirty.fill(true);
        *self = fresh;
    }

    /// The raw bytes, for the check to compare against.
    pub fn bytes(&self) -> &[u8] {
        &self.bits
    }

    /// Bitmap blocks that differ from the disk, as (index, bytes).
    pub fn dirty_blocks(&self) -> impl Iterator<Item = (usize, &[u8])> {
        self.dirty
            .iter()
            .enumerate()
            .filter(|(_, d)| **d)
            .map(|(i, _)| (i, &self.bits[i * BLOCK..(i + 1) * BLOCK]))
    }

    /// Forgets that anything differs from the disk (after writing it).
    pub fn clear_dirty(&mut self) {
        self.dirty.fill(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_and_counts() {
        let bm = Bitmap::empty(100);
        assert_eq!(bm.free(), 100);
        assert!(!bm.is_used(99));
        assert!(bm.is_used(100));
        assert!(bm.is_used(1_000_000));
    }

    #[test]
    fn allocate_prefers_one_run_and_free_restores() {
        let mut bm = Bitmap::empty(200);
        assert!(bm.mark_used(Run { start: 0, len: 10 }));
        // Leave holes of 3, then a long free stretch.
        assert!(bm.mark_used(Run { start: 13, len: 1 }));
        assert!(bm.mark_used(Run { start: 17, len: 1 }));
        let a = bm.allocate(5, 10, 200).unwrap();
        assert_eq!(a.len(), 1, "one run when one fits");
        assert!(a[0].start >= 18);
        // Not enough contiguous space: pieces are gathered.
        let mut bm = Bitmap::empty(20);
        for n in (0..20).step_by(2) {
            assert!(bm.mark_used(Run { start: n, len: 1 }));
        }
        let pieces = bm.allocate(4, 0, 20).unwrap();
        assert_eq!(pieces.iter().map(|r| r.len).sum::<u64>(), 4);
        assert_eq!(bm.free(), 6);
        // Asking for more than there is changes nothing.
        assert!(bm.allocate(7, 0, 20).is_none());
        assert_eq!(bm.free(), 6);
        for r in pieces {
            assert!(bm.mark_free(r));
        }
        assert_eq!(bm.free(), 10);
    }

    #[test]
    fn marking_twice_is_refused() {
        let mut bm = Bitmap::empty(50);
        assert!(bm.mark_used(Run { start: 5, len: 5 }));
        assert!(!bm.mark_used(Run { start: 9, len: 2 }), "block 9 is already used");
        assert!(!bm.mark_free(Run { start: 3, len: 5 }), "block 3 is already free");
        assert!(!bm.mark_free(Run { start: 45, len: 10 }), "runs past the volume are refused");
        assert_eq!(bm.free(), 45);
    }

    #[test]
    fn find_skips_whole_bytes() {
        let mut bm = Bitmap::empty(1000);
        assert!(bm.mark_used(Run { start: 0, len: 500 }));
        assert_eq!(bm.find(0, 1000, false), Some(500));
        assert_eq!(bm.find(0, 400, false), None);
        assert_eq!(bm.find(500, 1000, true), None);
        assert_eq!(bm.find(3, 1000, true), Some(3));
    }
}
