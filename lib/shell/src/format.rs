// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning facts into the lines the shell prints.
//!
//! Everything here is a plain function from data to text, so the exact shape of each line can
//! be tested. Every function that makes more than a fragment returns text whose lines all end
//! with `\n`, ready for the terminal. (The terminal's `write` shows exactly what it is given,
//! so a line without its `\n` would run into the next prompt.)

use crate::fs::{Item, Space, Stat};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_sandbox::{AppInfo, FileRights, Grant, GrantReason};
use pbr_interfaces::pbr_users::{Filesystem, User, UserKind, VolumeInfo, VolumeState};

/// The most bytes `pbr.terminal`'s `read_line` accepts for a prompt.
pub const MAX_PROMPT: usize = 256;
/// The most bytes `pbr.terminal`'s `write` accepts at once.
pub const MAX_WRITE: usize = 4096;

/// A size the way 1.x showed it: `12 bytes`, `3.4 KB`, `1.2 MB`, `120 GB`. Units are 1024
/// bytes; the number is cut down, not rounded, to one decimal place, and shown without one
/// from 100 up or when it would be `.0`.
pub fn size(bytes: u64) -> String {
    if bytes == 1 {
        return String::from("1 byte");
    }
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut whole = bytes / 1024;
    let mut rest = bytes % 1024;
    let mut unit = 0;
    while whole >= 1024 && unit < UNITS.len() - 1 {
        rest = whole % 1024;
        whole /= 1024;
        unit += 1;
    }
    let tenth = rest * 10 / 1024;
    if whole >= 100 || tenth == 0 {
        format!("{whole} {}", UNITS[unit])
    } else {
        format!("{whole}.{tenth} {}", UNITS[unit])
    }
}

/// A time given as seconds since 1970 (UTC) as `2026-11-08 14:03:21 UTC`, or `unknown` for 0
/// (a filesystem that doesn't record times).
pub fn time(seconds: u64) -> String {
    if seconds == 0 {
        return String::from("unknown");
    }
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Turning a day count into a calendar date: Howard Hinnant's `civil_from_days`, which
    // works in "eras" of 400 years (146097 days) so that leap years fall out of the arithmetic.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The prompt: `<user>:<current folder>> `, for example `chris:/Home> `. A very long folder
/// path is cut from the left (`chris:...rest/of/the/path> `) so the prompt always fits what the
/// terminal accepts.
pub fn prompt(user: &str, cwd: &str) -> String {
    let user = cut_front(user, 32, "");
    let whole = format!("{user}:{cwd}> ");
    if whole.len() <= MAX_PROMPT {
        return whole;
    }
    let room = MAX_PROMPT - user.len() - ":> ".len();
    format!("{user}:{}> ", cut_front(cwd, room, "..."))
}

/// `text` if it fits in `max` bytes; otherwise `marker` and the last bytes of it that fit
/// (never splitting a character).
fn cut_front(text: &str, max: usize, marker: &str) -> String {
    if text.len() <= max {
        return String::from(text);
    }
    let mut start = text.len() - (max - marker.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{marker}{}", &text[start..])
}

/// `text` in pieces of at most `max` bytes, cut between characters, to be written one after
/// the other. (`pbr.terminal` takes at most 4096 bytes per `write`.)
pub fn pieces(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(max);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // A character wider than `max` can't be split; there is no sensible output.
        if end == 0 {
            break;
        }
        let (piece, tail) = rest.split_at(end);
        out.push(piece);
        rest = tail;
    }
    out
}

/// The lines of `ls`: folders first, then files, each sorted by name (capital and small
/// letters count as the same, as on RockFS, with the exact bytes as the tie-break). Folders
/// are `name/`, files are `name  <size>`. Names beginning with `.` are left out unless `all`.
/// A folder with nothing to show gives no text at all.
pub fn listing(items: &[Item], all: bool) -> String {
    let mut shown: Vec<&Item> = items.iter().filter(|i| all || !i.name.starts_with('.')).collect();
    shown.sort_by_cached_key(|i| (!i.folder, i.name.to_ascii_lowercase(), i.name.clone()));
    let mut out = String::new();
    for item in shown {
        if item.folder {
            out.push_str(&format!("{}/\n", item.name));
        } else {
            out.push_str(&format!("{}  {}\n", item.name, size(item.size)));
        }
    }
    out
}

/// The text of `info`: what is known about `path`.
pub fn info(path: &str, st: &Stat) -> String {
    let mut out = format!("Path: {path}\n");
    out.push_str(if st.folder { "Kind: folder\n" } else { "Kind: file\n" });
    if !st.folder {
        if st.size < 1024 {
            out.push_str(&format!("Size: {}\n", size(st.size)));
        } else {
            out.push_str(&format!("Size: {} ({} bytes)\n", size(st.size), st.size));
        }
    }
    let rights = match (st.readable, st.writable) {
        (true, true) => "read and write",
        (true, false) => "read",
        (false, true) => "write",
        (false, false) => "none",
    };
    out.push_str(&format!("Rights: {rights}\n"));
    out.push_str(&format!("Created: {}\n", time(st.created)));
    out.push_str(&format!("Modified: {}\n", time(st.modified)));
    out.push_str(&format!("Hidden: {}\n", if st.hidden { "yes" } else { "no" }));
    if st.read_only {
        out.push_str("Volume: read only\n");
    }
    out
}

/// One line of `df`: `/Home  120 GB total, 80 GB free  (RockFS, "Home")`.
pub fn space_line(path: &str, sp: &Space) -> String {
    let what = if sp.label.is_empty() {
        sp.filesystem.clone()
    } else {
        format!("{}, \"{}\"", sp.filesystem, sp.label)
    };
    format!("{path}  {} total, {} free  ({what})\n", size(sp.total), size(sp.free))
}

/// Rows of cells as aligned columns, two spaces apart. Nothing is padded after the last cell.
pub fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = alloc::vec![0usize; columns];
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            out.push_str(cell);
            if i + 1 < row.len() {
                let pad = widths[i] - cell.chars().count() + 2;
                out.extend(core::iter::repeat_n(' ', pad));
            }
        }
        // An empty last cell would leave spaces behind the one before it.
        out.truncate(out.trim_end_matches(' ').len());
        out.push('\n');
    }
    out
}

/// The table `volumes` prints: id, label, filesystem, state, size, and notes (`read only`,
/// `in memory`).
pub fn volumes(list: &[VolumeInfo]) -> String {
    if list.is_empty() {
        return String::from("There are no volumes.\n");
    }
    let cell = |s: &str| String::from(s);
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(["ID", "Label", "Filesystem", "State", "Size", "Notes"].map(cell).to_vec());
    for v in list {
        let filesystem = match v.filesystem {
            Filesystem::RockFs => "RockFS",
            Filesystem::Fat32 => "FAT32",
            Filesystem::Empty => "empty",
            Filesystem::Unknown => "unknown",
        };
        let state = match v.state {
            VolumeState::Ready => "ready",
            VolumeState::Starting => "starting",
            VolumeState::Unsupported => "unsupported",
            VolumeState::Damaged => "damaged",
            VolumeState::Failed => "failed",
        };
        let mut notes: Vec<&str> = Vec::new();
        if v.read_only {
            notes.push("read only");
        }
        if v.in_memory {
            notes.push("in memory");
        }
        rows.push(alloc::vec![
            format!("{}", v.id),
            if v.label.is_empty() { cell("(no label)") } else { v.label.clone() },
            cell(filesystem),
            cell(state),
            size(v.size),
            notes.join(", "),
        ]);
    }
    table(&rows)
}

/// The lines of `apps`: `<id>  <name>  [trusted  ]running|ended`.
pub fn apps(list: &[AppInfo]) -> String {
    if list.is_empty() {
        return String::from("No apps have been started.\n");
    }
    let mut out = String::new();
    for a in list {
        let trusted = if a.trusted { "trusted  " } else { "" };
        let state = if a.running { "running" } else { "ended" };
        out.push_str(&format!("{}  {}  {trusted}{state}\n", a.id, a.app));
    }
    out
}

/// One line of `grants`: `<path>  <read|read write>  <manifest|rule|chosen|trusted>`.
pub fn grant(g: &Grant) -> String {
    let rights = match (g.rights.contains(FileRights::READ), g.rights.contains(FileRights::WRITE)) {
        (true, true) => "read write",
        (true, false) => "read",
        (false, true) => "write",
        (false, false) => "none",
    };
    let why = match g.reason {
        GrantReason::Manifest => "manifest",
        GrantReason::Rule => "rule",
        GrantReason::Chosen => "chosen",
        GrantReason::Trusted => "trusted",
    };
    format!("{}  {rights}  {why}\n", g.path)
}

/// The lines of `users`: name, kind (`admin` or `standard`) and display name, in columns.
pub fn users(list: &[User]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|u| {
            let kind = match u.kind {
                UserKind::Admin => "admin",
                UserKind::Standard => "standard",
            };
            alloc::vec![u.name.clone(), String::from(kind), u.display.clone()]
        })
        .collect();
    table(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn item(name: &str, folder: bool, size: u64) -> Item {
        Item { name: name.to_string(), folder, size }
    }

    #[test]
    fn sizes() {
        for (n, want) in [
            (0, "0 bytes"),
            (1, "1 byte"),
            (12, "12 bytes"),
            (1023, "1023 bytes"),
            (1024, "1 KB"),
            (1536, "1.5 KB"),
            (3482, "3.4 KB"),
            (102_399, "99.9 KB"),
            (102_400, "100 KB"),
            (1_310_720, "1.2 MB"),
            (1024 * 1024, "1 MB"),
            (5 * 1024 * 1024 * 1024, "5 GB"),
            (3 * 1024u64.pow(4) + 1024u64.pow(4) / 2, "3.5 TB"),
            (u64::MAX, "16777215 TB"),
        ] {
            assert_eq!(size(n), want, "{n}");
        }
    }

    #[test]
    fn times() {
        assert_eq!(time(0), "unknown");
        assert_eq!(time(1), "1970-01-01 00:00:01 UTC");
        assert_eq!(time(86_399), "1970-01-01 23:59:59 UTC");
        assert_eq!(time(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(time(1_000_000_000), "2001-09-09 01:46:40 UTC");
        assert_eq!(time(1_767_225_599), "2025-12-31 23:59:59 UTC");
        assert_eq!(time(1_794_146_601), "2026-11-08 14:03:21 UTC");
        assert_eq!(time(4_107_542_400), "2100-03-01 00:00:00 UTC");
        // The largest value doesn't overflow or panic.
        assert!(time(u64::MAX).ends_with("UTC"));
    }

    #[test]
    fn prompts() {
        assert_eq!(prompt("chris", "/Home"), "chris:/Home> ");
        assert_eq!(prompt("alice", "/"), "alice:/> ");
        let long = format!("/{}", "abcdefgh/".repeat(100));
        let p = prompt("chris", &long);
        assert!(p.len() <= MAX_PROMPT, "{}", p.len());
        assert!(p.starts_with("chris:..."));
        assert!(p.ends_with("abcdefgh/> "));
        // Never splits a character, and copes with a huge user name.
        let wide = format!("/{}", "é".repeat(300));
        assert!(prompt("chris", &wide).len() <= MAX_PROMPT);
        assert!(prompt(&"u".repeat(1000), &wide).len() <= MAX_PROMPT);
        // A path that just fits is left alone.
        let fits = format!("/{}", "a".repeat(MAX_PROMPT - "chris:".len() - 3));
        assert_eq!(prompt("chris", &fits), format!("chris:{fits}> "));
    }

    #[test]
    fn writing_in_pieces() {
        assert!(pieces("", 10).is_empty());
        assert_eq!(pieces("abc", 10), vec!["abc"]);
        assert_eq!(pieces("abcdef", 4), vec!["abcd", "ef"]);
        // Characters are never cut in the middle.
        assert_eq!(pieces("ééé", 3), vec!["é", "é", "é"]);
        let big = "x".repeat(10_000);
        let parts = pieces(&big, MAX_WRITE);
        assert!(parts.iter().all(|p| p.len() <= MAX_WRITE));
        assert_eq!(parts.concat(), big);
        // A limit too small for one character gives up rather than looping.
        assert!(pieces("é", 1).is_empty());
    }

    #[test]
    fn listing_order_and_shape() {
        let items = vec![
            item("zeta.txt", false, 12),
            item("Beta", true, 0),
            item("alpha.txt", false, 3482),
            item("alpha", true, 0),
            item(".hidden", false, 1),
            item(".config", true, 0),
            item("Zed.txt", false, 1_310_720),
        ];
        assert_eq!(
            listing(&items, false),
            "alpha/\nBeta/\nalpha.txt  3.4 KB\nZed.txt  1.2 MB\nzeta.txt  12 bytes\n"
        );
        assert_eq!(
            listing(&items, true),
            ".config/\nalpha/\nBeta/\n.hidden  1 byte\nalpha.txt  3.4 KB\nZed.txt  1.2 MB\n\
             zeta.txt  12 bytes\n"
        );
        assert_eq!(listing(&[], false), "");
        assert_eq!(listing(&[item(".x", false, 1)], false), "");
    }

    #[test]
    fn listing_ties_use_exact_bytes() {
        let items = vec![item("a", false, 0), item("A", false, 0)];
        assert_eq!(listing(&items, false), "A  0 bytes\na  0 bytes\n");
    }

    #[test]
    fn info_text() {
        let st = Stat {
            folder: false,
            size: 3482,
            readable: true,
            writable: true,
            created: 0,
            modified: 1_000_000_000,
            hidden: false,
            read_only: false,
        };
        assert_eq!(
            info("/Home/a.txt", &st),
            "Path: /Home/a.txt\nKind: file\nSize: 3.4 KB (3482 bytes)\nRights: read and write\n\
             Created: unknown\nModified: 2001-09-09 01:46:40 UTC\nHidden: no\n"
        );
        let ro = Stat { folder: true, writable: false, read_only: true, hidden: true, ..st };
        let text = info("/System", &ro);
        assert!(text.contains("Kind: folder\n") && !text.contains("Size:"));
        assert!(text.contains("Rights: read\n") && text.contains("Hidden: yes\n"));
        assert!(text.ends_with("Volume: read only\n"));
    }

    #[test]
    fn df_lines() {
        let sp = |label: &str| Space {
            total: 120 * 1024 * 1024 * 1024,
            free: 80 * 1024 * 1024 * 1024,
            label: label.to_string(),
            filesystem: "RockFS".to_string(),
        };
        assert_eq!(
            space_line("/Home", &sp("Home")),
            "/Home  120 GB total, 80 GB free  (RockFS, \"Home\")\n"
        );
        assert_eq!(
            space_line("/Volumes/x", &sp("")),
            "/Volumes/x  120 GB total, 80 GB free  (RockFS)\n"
        );
    }

    #[test]
    fn tables() {
        let rows = vec![
            vec!["a".to_string(), "bbb".to_string(), "c".to_string()],
            vec!["aaaa".to_string(), "b".to_string(), "".to_string()],
        ];
        assert_eq!(table(&rows), "a     bbb  c\naaaa  b\n");
        assert_eq!(table(&[]), "");
        // Uneven rows and wide characters don't break the columns.
        let rows = vec![vec!["é".to_string()], vec!["ab".to_string(), "x".to_string()]];
        assert_eq!(table(&rows), "é\nab  x\n");
    }

    fn volume(id: u32, label: &str) -> VolumeInfo {
        VolumeInfo {
            id,
            disk: 1,
            partition: id,
            filesystem: Filesystem::RockFs,
            state: VolumeState::Ready,
            label: label.to_string(),
            size: 64 * 1024 * 1024,
            read_only: false,
            in_memory: false,
        }
    }

    #[test]
    fn volume_table() {
        let mut ram = volume(2, "");
        ram.in_memory = true;
        ram.filesystem = Filesystem::Empty;
        ram.state = VolumeState::Starting;
        let mut fat = volume(3, "USB");
        fat.filesystem = Filesystem::Fat32;
        fat.read_only = true;
        fat.in_memory = true;
        let text = volumes(&[volume(1, "Home"), ram, fat]);
        assert_eq!(
            text,
            "ID  Label       Filesystem  State     Size   Notes\n\
             1   Home        RockFS      ready     64 MB\n\
             2   (no label)  empty       starting  64 MB  in memory\n\
             3   USB         FAT32       ready     64 MB  read only, in memory\n"
        );
        assert_eq!(volumes(&[]), "There are no volumes.\n");
    }

    #[test]
    fn app_lines() {
        let a = |id, app: &str, trusted, running| AppInfo {
            id,
            app: app.to_string(),
            trusted,
            running,
        };
        assert_eq!(
            apps(&[
                a(1, "shell", true, true),
                a(7, "notes", false, true),
                a(8, "paint", false, false)
            ]),
            "1  shell  trusted  running\n7  notes  running\n8  paint  ended\n"
        );
        assert_eq!(apps(&[]), "No apps have been started.\n");
    }

    #[test]
    fn grant_lines() {
        let g = |path: &str, rights, reason| Grant { path: path.to_string(), rights, reason };
        let rw = FileRights::READ | FileRights::WRITE;
        assert_eq!(
            grant(&g("/Home/Documents", rw, GrantReason::Manifest)),
            "/Home/Documents  read write  manifest\n"
        );
        assert_eq!(
            grant(&g("/Home/a b.txt", FileRights::READ, GrantReason::Chosen)),
            "/Home/a b.txt  read  chosen\n"
        );
        assert_eq!(grant(&g("/", FileRights::READ, GrantReason::Rule)), "/  read  rule\n");
        assert_eq!(grant(&g("/x", rw, GrantReason::Trusted)), "/x  read write  trusted\n");
    }

    #[test]
    fn user_lines() {
        let u = |name: &str, kind, display: &str| User {
            name: name.to_string(),
            kind,
            display: display.to_string(),
        };
        assert_eq!(
            users(&[
                u("chris", UserKind::Admin, "Chris"),
                u("alice", UserKind::Standard, "Alice B")
            ]),
            "chris  admin     Chris\nalice  standard  Alice B\n"
        );
    }
}
