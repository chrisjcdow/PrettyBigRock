// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! FAT's dates and times.
//!
//! FAT stores a date as a 16-bit number (years since 1980, month, day) and a time as another
//! (hours, minutes, and seconds in units of two), with no time zone: it is whatever the clock
//! on the wall of the computer that wrote it said. M5 treats these as UTC (time zones are an
//! open question in note 0016). The rest of PrettyBigRock counts seconds since 1970 UTC, so
//! this module converts.

/// 1980-01-01 00:00:00 UTC in seconds since 1970: the earliest time FAT can record.
const MIN: u64 = 315_532_800;
/// 2107-12-31 23:59:59 UTC: the latest. (Years are stored in 7 bits, so 1980 + 127.)
const MAX: u64 = days_from_civil(2108, 1, 1) as u64 * 86_400 - 1;

/// Days from 1970-01-01 to the given date in the proleptic Gregorian calendar (the calendar we
/// use, extended backwards and forwards without change). This is Howard Hinnant's well-known
/// algorithm: it counts in "eras" of 400 years, the length after which the calendar repeats.
const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // year of the era, 0..=399
    let mp = (month as u64 + 9) % 12; // March is 0, February is 11
    let doy = (153 * mp + 2) / 5 + day as u64 - 1; // day of the (March-based) year
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of the era
    era * 146_097 + doe as i64 - 719_468
}

/// The inverse of [`days_from_civil`]: (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe as i64 + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Seconds since 1970 for a FAT date, time and (for creation times) tenths of seconds.
///
/// Returns 0, "not recorded", for a date of 0 (FAT's way of saying so) or one that is not a
/// real date (month 13, February 30): a hostile or damaged disk may hold anything. A time with
/// impossible fields (hour 31) counts as midnight.
pub(crate) fn from_fat(date: u16, time: u16, tenths: u8) -> u64 {
    let year = 1980 + i64::from(date >> 9);
    let month = u32::from((date >> 5) & 0xF);
    let day = u32::from(date & 0x1F);
    if date == 0 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return 0;
    }
    let (hour, minute, second) =
        (u64::from(time >> 11), u64::from((time >> 5) & 0x3F), u64::from(time & 0x1F) * 2);
    let seconds_of_day = if hour < 24 && minute < 60 && second < 60 {
        hour * 3600 + minute * 60 + second
    } else {
        0
    };
    // The creation time's extra byte counts 10 ms units, 0..=199: up to one extra second
    // (the time field has only 2-second steps). Anything above that is nonsense.
    let extra = if tenths <= 199 { u64::from(tenths) / 100 } else { 0 };
    days_from_civil(year, month, day) as u64 * 86_400 + seconds_of_day + extra
}

/// A time in seconds since 1970 as FAT's (date, time, tenths). Times before 1980 or after 2107
/// are clamped to the nearest time FAT can hold. `tenths` is 100 when the second is odd, so the
/// creation time is kept exactly; modification times (which have no such field) lose a second
/// if it is odd.
pub(crate) fn to_fat(seconds: u64) -> (u16, u16, u8) {
    let seconds = seconds.clamp(MIN, MAX);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let of_day = seconds % 86_400;
    let (hour, minute, second) = (of_day / 3600, of_day / 60 % 60, of_day % 60);
    // `year` is within 1980..=2107 after the clamp above, so these casts cannot lose anything.
    let date = (((year - 1980) as u16) << 9) | ((month as u16) << 5) | day as u16;
    let time = ((hour as u16) << 11) | ((minute as u16) << 5) | (second / 2) as u16;
    (date, time, if second % 2 == 1 { 100 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // 2000-01-01 00:00:00 UTC is 946684800; 1980-01-01 is the earliest FAT date.
        let (d, t, x) = to_fat(946_684_800);
        assert_eq!((d, t, x), (((2000 - 1980) << 9) | (1 << 5) | 1, 0, 0));
        assert_eq!(from_fat(d, t, x), 946_684_800);
        let (d, t, _) = to_fat(MIN);
        assert_eq!(d, (1 << 5) | 1);
        assert_eq!(from_fat(d, t, 0), MIN);
        // 2026-11-07 12:34:56 UTC.
        let secs = 1_794_054_896;
        let (d, t, x) = to_fat(secs);
        assert_eq!(d, ((2026 - 1980) << 9) | (11 << 5) | 7);
        assert_eq!(t, (12 << 11) | (34 << 5) | 28);
        assert_eq!(from_fat(d, t, x), secs);
    }

    #[test]
    fn leap_days_and_round_trips() {
        // 2024-02-29 and 2100-02-28 (2100 is not a leap year), then every day in a few years.
        for secs in [1_709_208_000u64, 4_107_456_000, MIN, MAX - 1] {
            let (d, t, x) = to_fat(secs);
            assert_eq!(from_fat(d, t, x), secs, "{secs}");
        }
        let mut secs = MIN;
        while secs < 1_000_000_000 {
            let (d, t, x) = to_fat(secs);
            assert_eq!(from_fat(d, t, x), secs);
            secs += 86_400 * 7 + 3601;
        }
        assert_eq!(to_fat(MIN + 1).2, 100);
        // A modification time loses an odd second (no tenths field to carry it).
        let (d, t, _) = to_fat(MIN + 1);
        assert_eq!(from_fat(d, t, 0), MIN);
    }

    #[test]
    fn clamping() {
        assert_eq!(to_fat(0), to_fat(MIN));
        assert_eq!(to_fat(u64::MAX), to_fat(MAX));
        let (d, t, _) = to_fat(MAX);
        assert_eq!(d >> 9, 127);
        assert_eq!((d >> 5) & 0xF, 12);
        assert_eq!(d & 0x1F, 31);
        assert_eq!(t, (23 << 11) | (59 << 5) | 29);
    }

    #[test]
    fn nonsense_is_not_recorded() {
        assert_eq!(from_fat(0, 0x1234, 0), 0);
        assert_eq!(from_fat((46 << 9) | (13 << 5) | 1, 0, 0), 0); // month 13
        assert_eq!(from_fat((46 << 9) | (2 << 5) | 30, 0, 0), 0); // February 30
        assert_eq!(from_fat((46 << 9) | (1 << 5), 0, 0), 0); // day 0
        assert_eq!(from_fat((46 << 9) | (2 << 5) | 29, 0, 0), 0); // 2026 is not a leap year
        let date = (46 << 9) | (3 << 5) | 1;
        // Hour 31: counts as midnight; tenths 250: ignored.
        assert_eq!(from_fat(date, 31 << 11, 250), from_fat(date, 0, 0));
    }
}
