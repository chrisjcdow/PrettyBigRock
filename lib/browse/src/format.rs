// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sizes and dates as people read them.

use alloc::format;
use alloc::string::String;

/// The names of the months, in order.
const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The units above bytes; each is 1024 times the one before (base 1024, as people reading file
/// sizes on a computer expect).
const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

/// A size for a list: `0 bytes`, `1 byte`, `999 bytes`, `1.0 KB`, `1.5 KB`, `9.9 MB`, `10 KB`,
/// `512 MB`. Under 10 of a unit it shows one decimal, from 10 up none; 1023.6 KB is `1.0 MB`,
/// not `1024 KB`. The biggest unit is TB, so a huge size is a big number of TB.
pub fn size(bytes: u64) -> String {
    match bytes {
        0 => return String::from("0 bytes"),
        1 => return String::from("1 byte"),
        2..=1023 => return format!("{bytes} bytes"),
        _ => {}
    }
    // Everything is done in whole numbers (`u128` is wide enough that nothing here can
    // overflow), so rounding never depends on how a decimal fraction happens to be stored.
    let bytes = u128::from(bytes);
    // `unit` is the index into UNITS of the biggest unit that fits at least once.
    let mut unit = 0;
    while unit + 1 < UNITS.len() && bytes >= 1u128 << (10 * (unit + 2)) {
        unit += 1;
    }
    loop {
        let divisor = 1u128 << (10 * (unit + 1));
        let name = UNITS.get(unit).copied().unwrap_or("TB");
        // The size in tenths of the unit, rounded to the nearest.
        let tenths = (bytes * 10 + divisor / 2) / divisor;
        if tenths < 100 {
            return format!("{}.{} {name}", tenths / 10, tenths % 10);
        }
        let whole = (bytes + divisor / 2) / divisor;
        if whole >= 1024 && unit + 1 < UNITS.len() {
            unit += 1;
            continue;
        }
        return format!("{whole} {name}");
    }
}

/// A date and time as people read them: `14 Nov 2026 21:09`, in the user's time zone, which is
/// `offset_minutes` east of UTC (so `120` for South Africa; real zones are -720 to 840, and
/// anything else is held to that). `0` seconds means "no date" and gives an empty string. The
/// day has no leading zero.
pub fn date(seconds: u64, offset_minutes: i32) -> String {
    if seconds == 0 {
        return String::new();
    }
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    let offset = i64::from(offset_minutes.clamp(-720, 840)) * 60;
    let (year, month, day, hour, minute) = date_parts(seconds.saturating_add(offset));
    let month_name = MONTHS.get((month as usize).wrapping_sub(1)).copied().unwrap_or("???");
    format!("{day} {month_name} {year} {hour:02}:{minute:02}")
}

/// Seconds since 1 January 1970 (UTC) as `(year, month 1 to 12, day, hour, minute)`, also for
/// times before 1970 (negative) and for any `i64`.
pub fn date_parts(seconds: i64) -> (i64, u32, u32, u32, u32) {
    // `div_euclid` and `rem_euclid` divide rounding down, so a time before 1970 still gets a
    // day number and a time of day that count forwards from midnight.
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (of_day / 3600) as u32;
    let minute = (of_day % 3600 / 60) as u32;
    (year, month, day, hour, minute)
}

/// Turns a count of days since 1 January 1970 into a year, month and day of the usual
/// (Gregorian) calendar. This is Howard Hinnant's `civil_from_days`: it counts in "eras" of 400
/// years (146,097 days, which repeat exactly), starting each year on 1 March so the leap day
/// comes last, and so needs no table of month lengths.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Move the zero point from 1970-01-01 to 0000-03-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    // The day within the era, 0 to 146096.
    let day_of_era = z.rem_euclid(146_097);
    // The year within the era (0 to 399), allowing for the 100- and 400-year leap rules.
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    // The day within the year, counted from 1 March, and the month from 1 March (0 to 11).
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month =
        if month_from_march < 10 { month_from_march + 3 } else { month_from_march - 9 } as u32;
    // January and February belong to the year that began the previous March.
    (if month <= 2 { year + 1 } else { year }, month, day)
}
