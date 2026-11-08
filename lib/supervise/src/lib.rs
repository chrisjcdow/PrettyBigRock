// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! When to restart a program that ended, and when to give up: the policy every supervisor in
//! PrettyBigRock uses (docs/design/0012, section 4, for drivers; docs/design/0018, section 3,
//! for services: the root program, the device manager, the storage service and the users
//! service all use this).
//!
//! | Crash number within 30 seconds | Delay before restarting |
//! |:------------------------------:|-------------------------|
//! | 1 | at once |
//! | 2 | 100 ms |
//! | 3 | 1 s |
//! | 4 | none: give up (a driver's device is disabled; a service is left stopped, and logged) |
//!
//! A program that has run for 30 seconds since it was last started has its count reset.
//!
//! It is plain arithmetic on times the caller passes in, so it is tested on the host.

#![cfg_attr(not(test), no_std)]

/// How long a program must have run for its crashes to be forgotten.
pub const WINDOW_MS: u64 = 30_000;

/// What to do about a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Start it again after this many milliseconds.
    Restart {
        /// The wait.
        delay_ms: u64,
    },
    /// Leave it stopped.
    GiveUp,
}

/// One supervised program's crash history.
#[derive(Clone, Copy, Debug, Default)]
pub struct History {
    count: u32,
    started_at: u64,
}

impl History {
    /// A new history.
    pub const fn new() -> History {
        History { count: 0, started_at: 0 }
    }

    /// It was started at `now` (milliseconds on any steady clock).
    pub fn started(&mut self, now: u64) {
        self.started_at = now;
    }

    /// It ended at `now`: what to do about it.
    pub fn crashed(&mut self, now: u64) -> Decision {
        if now.saturating_sub(self.started_at) >= WINDOW_MS {
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        match self.count {
            1 => Decision::Restart { delay_ms: 0 },
            2 => Decision::Restart { delay_ms: 100 },
            3 => Decision::Restart { delay_ms: 1000 },
            _ => Decision::GiveUp,
        }
    }

    /// Forgets the crashes (for tests).
    pub fn forgive(&mut self) {
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_grow_then_it_gives_up() {
        let mut h = History::new();
        h.started(0);
        assert_eq!(h.crashed(10), Decision::Restart { delay_ms: 0 });
        h.started(10);
        assert_eq!(h.crashed(20), Decision::Restart { delay_ms: 100 });
        h.started(120);
        assert_eq!(h.crashed(130), Decision::Restart { delay_ms: 1000 });
        h.started(1130);
        assert_eq!(h.crashed(1140), Decision::GiveUp);
        // And it stays given up on.
        assert_eq!(h.crashed(1150), Decision::GiveUp);
    }

    #[test]
    fn a_long_run_forgives() {
        let mut h = History::new();
        h.started(0);
        assert_eq!(h.crashed(1), Decision::Restart { delay_ms: 0 });
        h.started(1);
        assert_eq!(h.crashed(2), Decision::Restart { delay_ms: 100 });
        h.started(102);
        // It ran for the whole window this time.
        assert_eq!(h.crashed(102 + WINDOW_MS), Decision::Restart { delay_ms: 0 });
        h.started(102 + WINDOW_MS);
        // One millisecond short of the window doesn't forgive.
        assert_eq!(h.crashed(101 + 2 * WINDOW_MS), Decision::Restart { delay_ms: 100 });
    }

    #[test]
    fn forgiving_starts_again() {
        let mut h = History::new();
        for _ in 0..3 {
            h.started(0);
            h.crashed(1);
        }
        h.forgive();
        assert_eq!(h.crashed(2), Decision::Restart { delay_ms: 0 });
    }

    #[test]
    fn a_clock_that_goes_backwards_is_harmless() {
        let mut h = History::new();
        h.started(1000);
        assert_eq!(h.crashed(5), Decision::Restart { delay_ms: 0 });
    }
}
