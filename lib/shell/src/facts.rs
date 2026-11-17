// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the shell prints from facts about the computer: `mem`, `fetch`, `disk`, `calc` and
//! `check` (docs/design/0026-apps-m6-tests-and-stages.md, sections 1 to 3).
//!
//! The facts come from `pbr.sysinfo` and `pbr.users`; this module only turns them into text, as
//! plain functions from data to a string whose lines all end in `\n`, so that the exact shape of
//! each line can be tested on the host. The look is 1.3's (`classic/kernel/term.c`).

use crate::clock;
use crate::format;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_sysinfo::{ScreenSummary, VolumeInfo};

/// Bytes in a megabyte, as 1.3 counted (`mem_total_mb`).
const MB: u64 = 1024 * 1024;

/// What `mem` prints: 1.3's `Installed memory: N MB`, and how much of it is free.
pub fn mem(total: u64, available: u64) -> String {
    format!("Installed memory: {} MB\nFree memory: {} MB\n", total / MB, available / MB)
}

/// What `calc EXPR` prints: `= 14`, or the reason it can't be worked out as `calc: …` (the
/// command's name is added by the caller's `show_plain`; this gives only the reason).
///
/// A sum too big to show (`pbr_calc::format` gives `Error`) is told in words, not as `= Error`.
pub fn calc(expr: &str) -> Result<String, String> {
    let value = pbr_calc::eval(expr).map_err(|e| format!("{e}"))?;
    let text = pbr_calc::format(value);
    if text == pbr_calc::ERROR_TEXT {
        return Err(String::from("the answer is too big to show"));
    }
    Ok(format!("= {text}\n"))
}

/// 1.3's picture of Table Mountain at sunset: the sun and a cloud, the mountain, the sea.
/// (1.3 drew it in colour; the shell's terminal has none, so it is only the shapes.)
pub const MOUNTAIN: [&str; 7] = [
    "            \\|/  ",
    "     ~~~~~~~ -o- ",
    "  /\\/_______\\    ",
    " /            \\  ",
    "/              \\^",
    "~~~~~~~~~~~~~~~~~",
    "  ~   ~   ~   ~  ",
];

/// What `fetch` shows beside the mountain. A fact the system couldn't give is `None` and is
/// shown as `unknown`.
#[derive(Clone, Debug, Default)]
pub struct Summary<'a> {
    /// The system's name and version (`PrettyBigRock`, `2.0.0-alpha.1`).
    pub version: Option<(&'a str, &'a str)>,
    /// The kind of computer the program was built for: `x86-64` or `ARM64`.
    pub arch: &'a str,
    /// The screens.
    pub screens: Option<&'a [ScreenSummary]>,
    /// Memory in bytes: the total and what is free.
    pub memory: Option<(u64, u64)>,
    /// Seconds since the computer started.
    pub uptime: u64,
    /// The volume the system started from.
    pub disk: Option<&'a VolumeInfo>,
}

/// The screens as one line's worth: `1024 x 768`, with `(2x)` for a scale of 2 or more, and a
/// comma between several.
fn displays(screens: &[ScreenSummary]) -> String {
    if screens.is_empty() {
        return String::from("none");
    }
    let each: Vec<String> = screens
        .iter()
        .map(|s| {
            let scale = if s.scale > 1 { format!(" ({}x)", s.scale) } else { String::new() };
            format!("{} x {}{scale}", s.width, s.height)
        })
        .collect();
    each.join(", ")
}

/// What `fetch` prints: the mountain, with a line of facts beside each row of it, 1.3's seven.
pub fn fetch(summary: &Summary<'_>) -> String {
    let title = match summary.version {
        Some((name, version)) => format!("{name} {version}"),
        None => String::from("PrettyBigRock"),
    };
    let unknown = || String::from("unknown");
    let facts = [
        title.clone(),
        "-".repeat(title.chars().count()),
        format!("Kernel:  {}, written in Rust", summary.arch),
        format!("Display: {}", summary.screens.map_or_else(unknown, displays)),
        format!(
            "Memory:  {}",
            summary.memory.map_or_else(unknown, |(total, free)| format!(
                "{} MB, {} MB free",
                total / MB,
                free / MB
            ))
        ),
        format!("Uptime:  {}", clock::duration_text(summary.uptime)),
        format!(
            "Disk:    {}",
            summary.disk.map_or_else(unknown, |v| format!(
                "{} ({}, {} free)",
                if v.label.is_empty() { "(no label)" } else { &v.label },
                format::size(v.size),
                format::size(v.free)
            ))
        ),
    ];
    let mut out = String::new();
    for (art, fact) in MOUNTAIN.iter().zip(facts) {
        out.push_str(art);
        out.push(' ');
        out.push_str(&fact);
        out.push('\n');
    }
    out
}

/// The volume `fetch` and `about` speak of: the one the system started from, else the first.
pub fn system_volume(volumes: &[VolumeInfo]) -> Option<&VolumeInfo> {
    volumes.iter().find(|v| v.system).or_else(|| volumes.first())
}

/// The table `disk` prints: each volume's label, filesystem, size and free space, and notes
/// (`system`, `read only`, `in memory`).
pub fn disk(volumes: &[VolumeInfo]) -> String {
    if volumes.is_empty() {
        return String::from("There are no disks.\n");
    }
    let cell = |s: &str| String::from(s);
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(["Label", "Filesystem", "Size", "Free", "Notes"].map(cell).to_vec());
    for v in volumes {
        let mut notes: Vec<&str> = Vec::new();
        if v.system {
            notes.push("system");
        }
        if v.read_only {
            notes.push("read only");
        }
        if v.in_memory {
            notes.push("in memory");
        }
        // The free space is known only for a volume whose filesystem is being served; 0 for
        // one that isn't (an empty volume, or a kind this system can't read) is "not known".
        let served = v.filesystem == "RockFS" || v.filesystem == "FAT32";
        rows.push(alloc::vec![
            if v.label.is_empty() { cell("(no label)") } else { v.label.clone() },
            v.filesystem.clone(),
            format::size(v.size),
            if served { format::size(v.free) } else { cell("-") },
            notes.join(", "),
        ]);
    }
    format::table(&rows)
}

/// What `check` says before it asks the users service to check: what is being done, and for the
/// system volume (`volume` is `None` or 0) that every app of the session is stopped and started
/// again, because they all hold files on it (`pbr.users`' `check`).
pub fn check_intro(volume: Option<u32>, fix: bool) -> String {
    let what = if fix { "Checking and repairing" } else { "Checking" };
    match volume {
        None | Some(0) => format!(
            "{what} the system volume...\n\
             This restarts your session: every app closes and starts again.\n"
        ),
        Some(n) => format!("{what} volume {n}...\n"),
    }
}

/// What `check` prints once the report is in: 1.3's words. With no problems `No problems
/// found.`; otherwise the report, then how many problems there were and, if they weren't
/// repaired, the command that repairs them.
pub fn check_result(volume: Option<u32>, fix: bool, report: &str, problems: u32) -> String {
    if problems == 0 {
        return String::from("No problems found.\n");
    }
    let mut out = String::from(report);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if fix {
        out.push_str(&format!("Fixed {problems} problem(s).\n"));
    } else {
        let command = match volume {
            None | Some(0) => String::from("check fix"),
            Some(n) => format!("check {n} fix"),
        };
        out.push_str(&format!("{problems} problem(s). Type '{command}' to repair them.\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(label: &str, filesystem: &str, size: u64, free: u64) -> VolumeInfo {
        VolumeInfo {
            label: label.into(),
            filesystem: filesystem.into(),
            size,
            free,
            read_only: false,
            in_memory: false,
            system: false,
        }
    }

    #[test]
    fn memory_in_megabytes() {
        assert_eq!(
            mem(2048 * MB, 1500 * MB + 5),
            "Installed memory: 2048 MB\nFree memory: 1500 MB\n"
        );
        assert_eq!(mem(0, 0), "Installed memory: 0 MB\nFree memory: 0 MB\n");
    }

    #[test]
    fn sums() {
        assert_eq!(calc("(2+3)*4^2").as_deref(), Ok("= 80\n"));
        assert_eq!(calc("12*(3+4)/2").as_deref(), Ok("= 42\n"));
        assert_eq!(calc("1/3").as_deref(), Ok("= 0.333333333\n"));
        assert_eq!(calc("0.1+0.2").as_deref(), Ok("= 0.3\n"));
        assert_eq!(calc("-2^2").as_deref(), Ok("= 4\n"), "1.3's rule: the sign goes first");
        assert_eq!(calc("1/0"), Err(String::from("you can't divide by zero")));
        assert_eq!(calc("2+"), Err(String::from("a number or a bracket was expected")));
        assert_eq!(calc(""), Err(String::from("a number or a bracket was expected")));
        assert_eq!(calc("(1"), Err(String::from("a bracket was never closed")));
        assert_eq!(calc("10^400"), Err(String::from("the answer is too big to show")));
    }

    #[test]
    fn the_mountain_is_17_wide_and_plain_ascii() {
        assert_eq!(MOUNTAIN.len(), 7);
        for row in MOUNTAIN {
            assert_eq!(row.len(), 17, "{row:?}");
            assert!(row.is_ascii());
        }
        assert_eq!(MOUNTAIN[4], "/              \\^");
    }

    #[test]
    fn fetch_puts_a_fact_beside_each_row_of_the_mountain() {
        let screens = [
            ScreenSummary { connector: "Virtual-1".into(), width: 1024, height: 768, scale: 1 },
            ScreenSummary { connector: "Virtual-2".into(), width: 1280, height: 720, scale: 2 },
        ];
        let disk = volume("Home", "RockFS", 64 * MB, 50 * MB);
        let text = fetch(&Summary {
            version: Some(("PrettyBigRock", "2.0.0-alpha.1")),
            arch: "x86-64",
            screens: Some(&screens),
            memory: Some((2048 * MB, 1800 * MB)),
            uptime: 125,
            disk: Some(&disk),
        });
        let want = "\
            \x20           \\|/   PrettyBigRock 2.0.0-alpha.1\n\
            \x20    ~~~~~~~ -o-  ---------------------------\n\
            \x20 /\\/_______\\     Kernel:  x86-64, written in Rust\n\
            \x20/            \\   Display: 1024 x 768, 1280 x 720 (2x)\n\
            /              \\^ Memory:  2048 MB, 1800 MB free\n\
            ~~~~~~~~~~~~~~~~~ Uptime:  2 min 5 s\n\
            \x20 ~   ~   ~   ~   Disk:    Home (64 MB, 50 MB free)\n";
        assert_eq!(text, want);
    }

    #[test]
    fn fetch_says_unknown_for_what_it_could_not_ask() {
        let text = fetch(&Summary { arch: "ARM64", ..Summary::default() });
        assert!(text.contains(" PrettyBigRock\n"));
        assert!(text.contains("Display: unknown\n"));
        assert!(text.contains("Memory:  unknown\n"));
        assert!(text.contains("Disk:    unknown\n"));
        assert!(text.contains("Kernel:  ARM64, written in Rust\n"));
        assert!(text.contains("Uptime:  0 min 0 s\n"));
        assert_eq!(text.lines().count(), 7);
        // No screens at all is not "unknown": the display server said so.
        let none = fetch(&Summary { screens: Some(&[]), ..Summary::default() });
        assert!(none.contains("Display: none\n"));
        // A volume with no label.
        let v = volume("", "FAT32", MB, 0);
        let text = fetch(&Summary { disk: Some(&v), ..Summary::default() });
        assert!(text.contains("Disk:    (no label) (1 MB, 0 bytes free)\n"));
    }

    #[test]
    fn the_system_volume_is_the_one_marked() {
        let mut list = vec![volume("A", "FAT32", 1, 0), volume("B", "RockFS", 2, 1)];
        assert_eq!(system_volume(&list).unwrap().label, "A", "none marked: the first");
        list[1].system = true;
        assert_eq!(system_volume(&list).unwrap().label, "B");
        assert!(system_volume(&[]).is_none());
    }

    #[test]
    fn the_disk_table() {
        assert_eq!(disk(&[]), "There are no disks.\n");
        let mut home = volume("Home", "RockFS", 64 * MB, 50 * MB);
        home.system = true;
        home.in_memory = true;
        let mut pics = volume("PICTURES", "FAT32", 128 * MB, 100 * MB);
        pics.read_only = true;
        let blank = volume("", "empty", 8 * MB, 0);
        let text = disk(&[home, pics, blank]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("Label") && lines[0].contains("Filesystem"));
        assert!(lines[1].starts_with("Home") && lines[1].contains("RockFS"));
        assert!(lines[1].contains("64 MB") && lines[1].contains("50 MB"));
        assert!(lines[1].ends_with("system, in memory"));
        assert!(lines[2].contains("FAT32") && lines[2].ends_with("read only"));
        assert!(lines[3].starts_with("(no label)") && lines[3].contains("empty"));
        // Free space of a volume nothing serves is not given as 0 bytes.
        assert!(!lines[3].contains("0 bytes") && lines[3].contains(" -"));
        // Columns line up: every Filesystem column starts at the same place.
        let at = lines[0].find("Filesystem").unwrap();
        assert!(lines[1..].iter().all(|l| l[..at].chars().all(|c| c != '\n')));
        assert_eq!(lines[1].find("RockFS"), Some(at));
        assert_eq!(lines[2].find("FAT32"), Some(at));
    }

    #[test]
    fn what_check_says_first() {
        assert_eq!(
            check_intro(None, false),
            "Checking the system volume...\n\
             This restarts your session: every app closes and starts again.\n"
        );
        assert_eq!(
            check_intro(Some(0), true).lines().next(),
            Some("Checking and repairing the system volume...")
        );
        assert!(check_intro(Some(0), true).contains("restarts your session"));
        // Another volume doesn't restart the session.
        assert_eq!(check_intro(Some(2), false), "Checking volume 2...\n");
        assert_eq!(check_intro(Some(2), true), "Checking and repairing volume 2...\n");
    }

    #[test]
    fn what_check_says_last() {
        assert_eq!(check_result(None, false, "", 0), "No problems found.\n");
        assert_eq!(
            check_result(Some(3), true, "ignored when nothing was wrong", 0),
            "No problems found.\n"
        );
        assert_eq!(
            check_result(None, false, "inode 4 has no name\nblock 9 is lost", 2),
            "inode 4 has no name\nblock 9 is lost\n2 problem(s). Type 'check fix' to repair them.\n"
        );
        assert_eq!(
            check_result(Some(2), false, "block 9 is lost\n", 1),
            "block 9 is lost\n1 problem(s). Type 'check 2 fix' to repair them.\n"
        );
        assert_eq!(
            check_result(Some(2), true, "repaired block 9\n", 1),
            "repaired block 9\nFixed 1 problem(s).\n"
        );
        // Problems with no report text still say how many.
        assert_eq!(check_result(None, true, "", 3), "Fixed 3 problem(s).\n");
    }
}
