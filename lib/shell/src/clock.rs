// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The wall clock as the shell shows it: `time`, `date` and `tz`, and how long the computer has
//! been up.
//!
//! The machine's clock counts seconds since 1 January 1970 in UTC. The user's `timezone`
//! setting (note 0024, section 5) says how many minutes east of UTC they are; this module
//! turns the two into a calendar date and a time of day, writes an offset the way 1.3 did
//! (`2 hours`), and reads one the way a person types it (`2`, `-5`, `5:30`).
//!
//! The setting itself is written as `+HH:MM` (`lib/manifest`'s `timezone_minutes` reads it).

use alloc::format;
use alloc::string::String;

/// The earliest offset `tz` accepts, in hours east of UTC: 1.3's range.
pub const MIN_HOURS: i32 = -12;
/// The latest offset `tz` accepts, in hours.
pub const MAX_HOURS: i32 = 14;

/// A date and time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    /// The year (4 digits for any date this century).
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    /// 0 to 23.
    pub hour: u32,
    /// 0 to 59.
    pub minute: u32,
    /// 0 to 59.
    pub second: u32,
}

/// The date and time `seconds` after 1 January 1970 UTC, moved `offset_minutes` east.
pub fn civil(seconds: u64, offset_minutes: i32) -> Civil {
    // Seconds are held as `i64` so that an offset can take them below zero; a time far beyond
    // anything a clock holds is cut down rather than overflow.
    let t = i64::try_from(seconds).unwrap_or(i64::MAX / 2) + i64::from(offset_minutes) * 60;
    let (days, of_day) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Turning a day count into a calendar date: Howard Hinnant's `civil_from_days`, which
    // works in "eras" of 400 years (146097 days) so that leap years fall out of the arithmetic.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    Civil {
        year: year_of_era + era * 400 + i64::from(month <= 2),
        month,
        day,
        hour: (of_day / 3600) as u32,
        minute: (of_day % 3600 / 60) as u32,
        second: (of_day % 60) as u32,
    }
}

/// An offset in hours as 1.3 wrote it: `2`, `-5`, `0`; and for a zone that isn't a whole
/// number of hours, as a decimal: `5.5`, `-3.5`, `5.75`.
pub fn hours_text(offset_minutes: i32) -> String {
    let sign = if offset_minutes < 0 { "-" } else { "" };
    let minutes = offset_minutes.unsigned_abs();
    let (hours, rest) = (minutes / 60, minutes % 60);
    match rest {
        0 => format!("{sign}{hours}"),
        // 15 minutes is .25 of an hour, 30 is .5, 45 is .75; any other is shown to two places.
        15 => format!("{sign}{hours}.25"),
        30 => format!("{sign}{hours}.5"),
        45 => format!("{sign}{hours}.75"),
        _ => format!("{sign}{hours}.{:02}", rest * 100 / 60),
    }
}

/// The line `time` and `date` print: `2026-11-08 14:03:21 (offset 2 h)`, 1.3's.
pub fn time_line(seconds: u64, offset_minutes: i32) -> String {
    let c = civil(seconds, offset_minutes);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} (offset {} h)\n",
        c.year,
        c.month,
        c.day,
        c.hour,
        c.minute,
        c.second,
        hours_text(offset_minutes)
    )
}

/// What `tz` with no argument prints: 1.3's words, with the example.
pub fn offset_line(offset_minutes: i32) -> String {
    let unit = if offset_minutes == 60 { "hour" } else { "hours" };
    format!("Clock offset is {} {unit}. Use e.g. 'tz 2' for SAST.\n", hours_text(offset_minutes))
}

/// What `tz 2` prints when it worked.
pub fn offset_set_line(offset_minutes: i32) -> String {
    let unit = if offset_minutes == 60 { "hour" } else { "hours" };
    format!("Clock offset set to {} {unit}.\n", hours_text(offset_minutes))
}

/// The offset as the `timezone` setting holds it: `+02:00`, `-03:30`.
pub fn setting_value(offset_minutes: i32) -> String {
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let minutes = offset_minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Why a typed offset isn't one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffsetError {
    /// Not hours (and perhaps minutes) as digits.
    NotAnOffset,
    /// Well formed, but not from -12 to 14 hours.
    OutOfRange,
}

/// Reads an offset as a person types it, as minutes east of UTC: whole hours with an optional
/// sign (`2`, `+2`, `-5`), or hours and minutes (`5:30`, `-03:30`). From -12 hours to +14.
pub fn parse_offset(text: &str) -> Result<i32, OffsetError> {
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let digits = |s: &str, most: usize| {
        (!s.is_empty() && s.len() <= most && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<i32>().ok())
            .flatten()
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((h, m)) => {
            // Two digits for the minutes, so `5:3` isn't taken to mean 5:03.
            let minutes = digits(m, 2)
                .filter(|minutes| m.len() == 2 && *minutes < 60)
                .ok_or(OffsetError::NotAnOffset)?;
            (digits(h, 2).ok_or(OffsetError::NotAnOffset)?, minutes)
        }
        None => (digits(rest, 2).ok_or(OffsetError::NotAnOffset)?, 0),
    };
    let total = sign * (hours * 60 + minutes);
    if (MIN_HOURS * 60..=MAX_HOURS * 60).contains(&total) {
        Ok(total)
    } else {
        Err(OffsetError::OutOfRange)
    }
}

/// What `uptime` prints, from the seconds since the computer started: 1.3's `Up 1 h 2 min 3 s`.
pub fn uptime_line(seconds: u64) -> String {
    format!("Up {} h {} min {} s\n", seconds / 3600, (seconds / 60) % 60, seconds % 60)
}

/// A length of time as `fetch` shows it: `12 min 5 s`, or `1 h 12 min 5 s` from an hour up.
pub fn duration_text(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{} h {} min {} s", seconds / 3600, (seconds / 60) % 60, seconds % 60)
    } else {
        format!("{} min {} s", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_of_day() {
        let c = civil(0, 0);
        assert_eq!((c.year, c.month, c.day, c.hour, c.minute, c.second), (1970, 1, 1, 0, 0, 0));
        // 29 February 2000, a leap day, and the 1 March after it.
        assert_eq!(civil(951_782_400, 0).day, 29);
        assert_eq!((civil(951_868_800, 0).month, civil(951_868_800, 0).day), (3, 1));
        // 8 November 2026, 14:03:21 UTC.
        let c = civil(1_794_146_601, 0);
        assert_eq!((c.year, c.month, c.day, c.hour, c.minute, c.second), (2026, 11, 8, 14, 3, 21));
        // The offset moves it, across midnight both ways, and before 1970.
        let c = civil(1_794_146_601, 10 * 60);
        assert_eq!((c.month, c.day, c.hour, c.minute), (11, 9, 0, 3));
        let c = civil(30, -60);
        assert_eq!((c.year, c.month, c.day, c.hour, c.minute), (1969, 12, 31, 23, 0));
        // Nothing panics at the ends.
        let _ = civil(u64::MAX, 14 * 60);
        let _ = civil(0, -12 * 60);
    }

    #[test]
    fn the_time_line_is_1_3s_with_the_offset() {
        assert_eq!(time_line(1_794_146_601, 0), "2026-11-08 14:03:21 (offset 0 h)\n");
        assert_eq!(time_line(1_794_146_601, 120), "2026-11-08 16:03:21 (offset 2 h)\n");
        assert_eq!(time_line(1_794_146_601, -300), "2026-11-08 09:03:21 (offset -5 h)\n");
        assert_eq!(time_line(1_794_146_601, 330), "2026-11-08 19:33:21 (offset 5.5 h)\n");
    }

    #[test]
    fn offsets_in_hours() {
        for (minutes, want) in [
            (0, "0"),
            (120, "2"),
            (-300, "-5"),
            (330, "5.5"),
            (-210, "-3.5"),
            (345, "5.75"),
            (-45, "-0.75"),
            (15, "0.25"),
            (10, "0.16"),
            (-30, "-0.5"),
        ] {
            assert_eq!(hours_text(minutes), want, "{minutes}");
        }
        assert_eq!(offset_line(120), "Clock offset is 2 hours. Use e.g. 'tz 2' for SAST.\n");
        assert_eq!(offset_line(60), "Clock offset is 1 hour. Use e.g. 'tz 2' for SAST.\n");
        assert_eq!(offset_set_line(-300), "Clock offset set to -5 hours.\n");
        assert_eq!(offset_set_line(0), "Clock offset set to 0 hours.\n");
    }

    #[test]
    fn the_setting_is_written_as_plus_or_minus_hh_mm() {
        for (minutes, want) in
            [(0, "+00:00"), (120, "+02:00"), (-300, "-05:00"), (330, "+05:30"), (-210, "-03:30")]
        {
            assert_eq!(setting_value(minutes), want);
            // And the setting's reader agrees with what is written.
            assert_eq!(pbr_manifest::settings::timezone_minutes(want), Some(minutes));
        }
    }

    #[test]
    fn typed_offsets() {
        for (text, want) in [
            ("0", 0),
            ("2", 120),
            ("+2", 120),
            ("-5", -300),
            ("14", 14 * 60),
            ("-12", -12 * 60),
            ("5:30", 330),
            ("-03:30", -210),
            ("+05:45", 345),
            ("00", 0),
        ] {
            assert_eq!(parse_offset(text), Ok(want), "{text:?}");
        }
        for text in ["15", "-13", "99", "14:01", "-12:30", "+20:00"] {
            assert_eq!(parse_offset(text), Err(OffsetError::OutOfRange), "{text:?}");
        }
        for text in [
            "", "+", "-", "x", "2h", "1.5", "5:", ":30", "5:3", "5:60", "5:300", "100", "--2",
            "2 ", "²",
        ] {
            assert_eq!(parse_offset(text), Err(OffsetError::NotAnOffset), "{text:?}");
        }
    }

    #[test]
    fn how_long_the_computer_has_been_up() {
        assert_eq!(uptime_line(0), "Up 0 h 0 min 0 s\n");
        assert_eq!(uptime_line(3 * 3600 + 25 * 60 + 9), "Up 3 h 25 min 9 s\n");
        assert_eq!(duration_text(125), "2 min 5 s");
        assert_eq!(duration_text(3600 + 12 * 60 + 5), "1 h 12 min 5 s");
        assert_eq!(duration_text(0), "0 min 0 s");
        let _ = uptime_line(u64::MAX);
    }
}
