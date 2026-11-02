// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Small helpers: sizes and dates in the form `rockfs.py` prints them, error messages, and
//! the clock.

use pbr_fs::FsError;
use std::time::{SystemTime, UNIX_EPOCH};

/// The time now, in seconds since 1970 UTC.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A size as `rockfs.py` shows it: "512 bytes", "3.0 KB", "1.5 MB".
pub fn human(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut n = bytes as f64;
    for unit in ["KB", "MB", "GB", "TB"] {
        n /= 1024.0;
        if n < 1024.0 || unit == "TB" {
            return format!("{n:.1} {unit}");
        }
    }
    unreachable!("the loop returns at the last unit")
}

/// Seconds since 1970 as "YYYY-MM-DD HH:MM" in UTC.
pub fn date_time(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let minute_of_day = (seconds % 86_400) / 60;
    // Days since 1970 to a calendar date: the standard "civil from days" arithmetic, which
    // works in 400-year cycles of 146,097 days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// What an error means, in words for a person.
pub fn describe(e: FsError) -> &'static str {
    match e {
        FsError::NotFound => "not found",
        FsError::AlreadyExists => "already exists",
        FsError::NotAFolder => "not a folder",
        FsError::NotAFile => "is a folder, not a file",
        FsError::BadName => "that is not a valid name",
        FsError::FolderNotEmpty => "the folder is not empty",
        FsError::NoSpace => "there is not enough free space",
        FsError::ReadOnly => "this volume can only be read by this version of the tool",
        FsError::TooBig => "too big",
        FsError::Gone => "that item has been removed",
        FsError::Damaged => "the volume is damaged; run \"check\"",
        FsError::Disk => "the disk could not be read or written",
        FsError::BadArgument => "a folder can't be moved inside itself",
    }
}

/// A reasonably unique 16-byte volume ID from the clock and the process number. (Not random
/// in any strong sense: the ID only has to differ between volumes formatted by people.)
pub fn new_uuid() -> [u8; 16] {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut x = (nanos as u64) ^ (u64::from(std::process::id()) << 40) ^ 0x9E37_79B9_7F4A_7C15;
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        // splitmix64: a few multiplications and shifts that spread the bits around.
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    // Mark it as a version 4 (random) UUID, as the Python tool's IDs are.
    out[6] = out[6] & 0x0F | 0x40;
    out[8] = out[8] & 0x3F | 0x80;
    out
}

/// Formats a 16-byte ID as the usual 8-4-4-4-12 hexadecimal groups.
pub fn uuid_text(u: &[u8; 16]) -> String {
    let hex: String = u.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_python_tools() {
        assert_eq!(human(0), "0 bytes");
        assert_eq!(human(1023), "1023 bytes");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1536 * 1024), "1.5 MB");
    }

    #[test]
    fn dates_are_calendar_dates() {
        assert_eq!(date_time(0), "1970-01-01 00:00");
        assert_eq!(date_time(951_782_400 + 3_600 * 13 + 60 * 7), "2000-02-29 13:07");
        assert_eq!(date_time(1_800_000_000), "2027-01-15 08:00");
    }

    #[test]
    fn uuid_text_has_the_usual_shape() {
        let t = uuid_text(&new_uuid());
        assert_eq!(t.len(), 36);
        assert_eq!(t.matches('-').count(), 4);
    }
}
