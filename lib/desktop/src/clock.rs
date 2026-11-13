// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The taskbar's clock (docs/design/0024-the-session-desktop.md, section 6): the wall clock's
//! seconds since 1970 (UTC), moved by the user's time zone, as 1.3 showed it: "14:05" and
//! "13 Nov 2026".

use alloc::format;
use alloc::string::String;

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The date of day `days` after 1 January 1970: (year, month 1 to 12, day 1 to 31). Howard
/// Hinnant's `civil_from_days`, which counts in 400-year cycles of the Gregorian calendar
/// starting from 1 March (so that the leap day is the last day of a "year").
pub fn date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], from March
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The time ("14:05") and date ("13 Nov 2026") `seconds` after 1970 (UTC), `offset` minutes
/// east of UTC.
pub fn local(seconds: u64, offset: i32) -> (String, String) {
    let t = i64::try_from(seconds).unwrap_or(i64::MAX / 2) + i64::from(offset) * 60;
    let (days, of_day) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = date(days);
    let time = format!("{:02}:{:02}", of_day / 3600, of_day % 3600 / 60);
    (time, format!("{d} {} {y}", MONTHS[(m - 1) as usize % 12]))
}

/// Seconds until the next whole minute, `seconds` after 1970: when the clock next changes.
pub fn until_next_minute(seconds: u64) -> u64 {
    60 - seconds % 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), (1970, 1, 1));
        assert_eq!(date(-1), (1969, 12, 31));
        assert_eq!(date(11_016), (2000, 2, 29));
        assert_eq!(date(11_017), (2000, 3, 1));
        // 13 November 2026.
        assert_eq!(date(20_770), (2026, 11, 13));
    }

    #[test]
    fn local_time() {
        // 13 November 2026, 14:46:30 UTC.
        let t = 20_770 * 86_400 + 14 * 3600 + 46 * 60 + 30;
        assert_eq!(local(t, 0), ("14:46".into(), "13 Nov 2026".into()));
        assert_eq!(local(t, 120), ("16:46".into(), "13 Nov 2026".into()));
        // Across midnight both ways.
        assert_eq!(local(t, 10 * 60), ("00:46".into(), "14 Nov 2026".into()));
        assert_eq!(local(20_770 * 86_400 + 30, -60), ("23:00".into(), "12 Nov 2026".into()));
        assert_eq!(until_next_minute(t), 30);
        // Nothing panics at the ends.
        let _ = local(u64::MAX, -14 * 60);
        let _ = local(0, -12 * 60);
    }
}
