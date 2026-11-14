// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sizes and dates.

use pbr_browse::format::{date, date_parts, size};

#[test]
fn small_sizes_are_in_bytes() {
    assert_eq!(size(0), "0 bytes");
    assert_eq!(size(1), "1 byte");
    assert_eq!(size(2), "2 bytes");
    assert_eq!(size(999), "999 bytes");
    assert_eq!(size(1000), "1000 bytes");
    assert_eq!(size(1023), "1023 bytes");
}

#[test]
fn one_decimal_under_ten_and_whole_from_ten() {
    assert_eq!(size(1024), "1.0 KB");
    assert_eq!(size(1536), "1.5 KB");
    assert_eq!(size(9 * 1024 + 921), "9.9 KB");
    assert_eq!(size(10 * 1024), "10 KB");
    assert_eq!(size(10 * 1024 + 511), "10 KB");
    assert_eq!(size(512 * 1024), "512 KB");
    assert_eq!(size(1024 * 1024), "1.0 MB");
    assert_eq!(size(9 * 1024 * 1024 + 900 * 1024), "9.9 MB");
    assert_eq!(size(512 * 1024 * 1024), "512 MB");
    assert_eq!(size(1 << 30), "1.0 GB");
    assert_eq!(size(5 << 30), "5.0 GB");
    assert_eq!(size(1 << 40), "1.0 TB");
    assert_eq!(size(300 << 40), "300 TB");
}

#[test]
fn rounding_at_the_edges() {
    // 9.95 KB rounds to 10, so it is shown whole.
    assert_eq!(size(10_189), "10 KB");
    // 9.94 KB is 9.9.
    assert_eq!(size(10_178), "9.9 KB");
    // 1023.4 KB stays in KB, 1023.6 KB is 1.0 MB, and so is exactly 1024 KB.
    assert_eq!(size(1023 * 1024 + 400), "1023 KB");
    assert_eq!(size(1023 * 1024 + 614), "1.0 MB");
    assert_eq!(size(1024 * 1024 - 1), "1.0 MB");
    assert_eq!(size(1024 * 1024 * 1024 - 1), "1.0 GB");
    assert_eq!(size((1u64 << 40) - 1), "1.0 TB");
}

#[test]
fn the_biggest_size_does_not_panic() {
    assert_eq!(size(u64::MAX), "16777216 TB");
    assert_eq!(size(u64::MAX - 1), "16777216 TB");
    assert!(!size(1 << 63).is_empty());
}

#[test]
fn dates_in_utc() {
    assert_eq!(date(0, 0), "");
    assert_eq!(date(0, 120), "");
    assert_eq!(date(1, 0), "1 Jan 1970 00:00");
    assert_eq!(date(1_731_618_540, 0), "14 Nov 2024 21:09");
    // 29 February 2024, 12:30 UTC.
    assert_eq!(date(1_709_209_800, 0), "29 Feb 2024 12:30");
    // 31 December 1999, 23:59 UTC; one minute on is the new millennium.
    assert_eq!(date(946_684_740, 0), "31 Dec 1999 23:59");
    assert_eq!(date(946_684_800, 0), "1 Jan 2000 00:00");
}

#[test]
fn dates_in_a_time_zone() {
    // South Africa, 2 hours east: 31 Dec 1999 23:59 UTC is already 1 January.
    assert_eq!(date(946_684_740, 120), "1 Jan 2000 01:59");
    // A zone to the west, crossing midnight backwards: 1 Jan 2000 00:30 UTC in UTC-5.
    assert_eq!(date(946_684_800 + 30 * 60, -300), "31 Dec 1999 19:30");
    // And just before the epoch, from 1 second after it.
    assert_eq!(date(1, -720), "31 Dec 1969 12:00");
    assert_eq!(date(1, 840), "1 Jan 1970 14:00");
    // An offset outside the real ones is held to them.
    assert_eq!(date(1, i32::MAX), date(1, 840));
    assert_eq!(date(1, i32::MIN), date(1, -720));
}

#[test]
fn dates_far_away_do_not_panic() {
    // Year 9999 and beyond: 1 Jan 10000 is 253_402_300_800 seconds after 1970.
    assert_eq!(date(253_402_300_799, 0), "31 Dec 9999 23:59");
    assert_eq!(date(253_402_300_800, 0), "1 Jan 10000 00:00");
    assert!(!date(u64::MAX, 0).is_empty());
    assert!(!date(u64::MAX, 840).is_empty());
    assert!(!date(i64::MAX as u64, -720).is_empty());
}

#[test]
fn date_parts_any_i64() {
    assert_eq!(date_parts(0), (1970, 1, 1, 0, 0));
    assert_eq!(date_parts(-1), (1969, 12, 31, 23, 59));
    assert_eq!(date_parts(-86_400 * 365), (1969, 1, 1, 0, 0));
    // 29 Feb 2000 (a leap year because of the 400-year rule), and 1 Mar 1900 (not a leap year).
    assert_eq!(date_parts(951_782_400), (2000, 2, 29, 0, 0));
    assert_eq!(date_parts(-2_203_891_200), (1900, 3, 1, 0, 0));
    for s in [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1] {
        let (_, month, day, hour, minute) = date_parts(s);
        assert!((1..=12).contains(&month) && (1..=31).contains(&day) && hour < 24 && minute < 60);
    }
}

#[test]
fn every_day_of_four_hundred_years_is_one_after_the_other() {
    // Walk a whole 400-year cycle a day at a time and check the calendar by its own rules.
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let (mut year, mut month, mut day) = (1970i64, 1u32, 1u32);
    for days in 0..146_097i64 {
        assert_eq!(date_parts(days * 86_400 + 3_661), (year, month, day, 1, 1), "day {days}");
        let in_month = match month {
            2 if leap(year) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        day += 1;
        if day > in_month {
            day = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
    }
}
