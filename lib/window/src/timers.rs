// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The app's timers: a few ids, each with the moment it should fire.
//!
//! Times are nanoseconds on the monotonic clock, passed in by the caller so that this can be
//! tested without a clock.

use alloc::vec::Vec;

/// Timers waiting to fire.
#[derive(Debug, Default)]
pub struct TimerQueue {
    /// (id, time, order set in). Few timers are ever live, so a plain list is quick enough
    /// and keeps the code obvious.
    entries: Vec<(u32, u64, u64)>,
    counter: u64,
}

impl TimerQueue {
    /// No timers.
    pub fn new() -> TimerQueue {
        TimerQueue::default()
    }

    /// Sets timer `id` to fire at `at`. If it was already set, this replaces it.
    pub fn set(&mut self, id: u32, at: u64) {
        self.cancel(id);
        self.counter += 1;
        self.entries.push((id, at, self.counter));
    }

    /// Cancels timer `id`; true if it was set.
    pub fn cancel(&mut self, id: u32) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.0 != id);
        self.entries.len() != before
    }

    /// When the nearest timer fires.
    pub fn next(&self) -> Option<u64> {
        self.entries.iter().map(|e| e.1).min()
    }

    /// Takes the timer that fires first among those whose time has come at `now`. Timers with
    /// equal times fire in the order they were set.
    pub fn pop_due(&mut self, now: u64) -> Option<u32> {
        let i = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.1 <= now)
            .min_by_key(|(_, e)| (e.1, e.2))
            .map(|(i, _)| i)?;
        Some(self.entries.remove(i).0)
    }

    /// Whether no timer is set.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_in_time_order_only_when_due() {
        let mut q = TimerQueue::new();
        assert!(q.is_empty() && q.next().is_none() && q.pop_due(u64::MAX).is_none());
        q.set(1, 300);
        q.set(2, 100);
        q.set(3, 200);
        assert_eq!(q.next(), Some(100));
        assert_eq!(q.pop_due(99), None);
        assert_eq!(q.pop_due(250), Some(2));
        assert_eq!(q.pop_due(250), Some(3));
        assert_eq!(q.pop_due(250), None);
        assert_eq!(q.next(), Some(300));
        assert_eq!(q.pop_due(300), Some(1));
        assert!(q.is_empty());
    }

    #[test]
    fn equal_times_keep_the_order_they_were_set() {
        let mut q = TimerQueue::new();
        q.set(9, 50);
        q.set(4, 50);
        q.set(7, 50);
        assert_eq!(
            [q.pop_due(50), q.pop_due(50), q.pop_due(50), q.pop_due(50)],
            [Some(9), Some(4), Some(7), None]
        );
    }

    #[test]
    fn cancelling_and_resetting() {
        let mut q = TimerQueue::new();
        q.set(1, 10);
        q.set(2, 20);
        assert!(q.cancel(1));
        assert!(!q.cancel(1));
        assert_eq!(q.pop_due(100), Some(2));
        q.set(5, 10);
        q.set(5, 90); // the same id again moves it
        assert_eq!(q.pop_due(50), None);
        assert_eq!(q.pop_due(90), Some(5));
        assert_eq!(q.pop_due(90), None);
    }
}
