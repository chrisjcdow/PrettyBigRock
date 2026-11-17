// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The words the shell says about itself: `help`, `about`, `potato`, `fortune`, and the usage
//! line each command shows when it is given the wrong arguments.
//!
//! `COMMANDS` is the one list of commands that the user is told about. `potato` is deliberately
//! missing from it, as in 1.x ("you have to know about it"). 1.3's other names for commands
//! (`dir`, `del`, `cls`...) are not listed either; each is mentioned beside the command it is
//! another name for ([`crate::command::alias`] is the list of what they mean).

use alloc::string::String;

/// Every command people are told about: (usage, what it does), in the order `help` shows them.
/// The first word of the usage is the command's name. Uppercase words are things you type;
/// `[ ]` are optional.
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "this list (also ?)"),
    ("about", "about PrettyBigRock (also ver)"),
    ("fetch", "system summary and a picture of Table Mountain (also neofetch)"),
    ("clear", "clear the screen (also cls)"),
    ("time", "show the date and time (also date)"),
    ("tz [HOURS]", "show or set the clock's offset from UTC: -12 to 14, as in tz 2 or tz 5:30"),
    ("uptime", "how long the computer has been on"),
    ("mem", "memory: installed and free"),
    ("echo TEXT...", "print text"),
    ("calc EXPR...", "work out a sum, as in calc (2+3)*4^2"),
    ("ls [-a] [PATH]", "list a folder (-a shows names starting with a dot; also dir)"),
    ("cd [PATH]", "go to a folder (/Home if no PATH)"),
    ("pwd", "show the current folder"),
    ("cat FILE", "show a file, up to 64 KB (also type)"),
    ("info PATH", "kind, size, rights and times of a file or folder"),
    ("mkdir PATH", "make a folder (also md)"),
    ("write FILE TEXT...", "replace a file's contents with a line of text"),
    ("append FILE TEXT...", "add a line of text to the end of a file"),
    ("cp [-r] FROM TO", "copy a file; -r: a folder and all that is in it (also copy)"),
    ("mv FROM TO", "move or rename (also move, ren)"),
    ("rm [-r] PATH", "remove a file or empty folder; -r: a folder and all in it (also del, rmdir)"),
    ("edit [FILE]", "open a file in Notepad"),
    ("volumes", "list the disks and partitions (administrators)"),
    ("df", "size and free space of your home and each volume"),
    ("disk", "each disk's filesystem, size and free space"),
    (
        "check [VOLUME] [fix]",
        "check a RockFS volume, and repair it with fix (administrators; also fsck)",
    ),
    ("format VOLUME LABEL", "put RockFS on an empty volume (administrators; no questions asked)"),
    ("setup VOLUME NAME", "make a volume the system volume at the next start (administrators)"),
    ("whoami", "your user name"),
    ("settings", "show your settings"),
    ("set KEY VALUE", "change a setting: layout (us, gb, us-intl) or timezone (+02:00)"),
    ("users", "list the users (administrators)"),
    ("adduser NAME [admin|standard] [DISPLAY NAME...]", "add a user (administrators)"),
    ("run APP [FILE...] [-- ARGS...]", "start an app, giving it the FILEs"),
    ("open APP", "start an app: notepad, files, calculator, paint, snake, about"),
    ("apps", "list the apps of this session"),
    ("grants APP", "show what an app can reach, and why"),
    ("revoke APP PATH", "take PATH back from a running app"),
    ("stop APP", "stop an app"),
    ("fortune", "a little wisdom"),
    ("shutdown", "save everything, then it is safe to turn off the computer (administrators)"),
    ("poweroff", "save everything and turn the computer off"),
    ("reboot", "save everything and restart the computer"),
];

/// The note `help` ends with.
const HELP_FOOTER: &str = "APP is an app's number (see apps) or its name. Put \"quotes\" around \
names and text that have spaces.\n";

/// The text of `help`.
pub fn help() -> String {
    let width = COMMANDS.iter().map(|(usage, _)| usage.chars().count()).max().unwrap_or(0);
    let mut out = String::from("Commands:\n");
    for (usage, what) in COMMANDS {
        out.push_str("  ");
        out.push_str(usage);
        out.extend(core::iter::repeat_n(' ', width - usage.chars().count() + 2));
        out.push_str(what);
        out.push('\n');
    }
    out.push_str(HELP_FOOTER);
    out
}

/// The text of `about`.
pub fn about() -> String {
    String::from(
        "PrettyBigRock 2, in development.\n\
         A small hobby operating system written in Rust, for x86-64 and ARM64.\n\
         Every app sees only the files it was given, and windows only its own.\n",
    )
}

/// The text of `about` (and `ver`) when the shell can ask the system its version
/// (`pbr.sysinfo`, note 0026, section 5): the name and version first (`PrettyBigRock
/// 2.0.0-alpha.1`), then [`about`]. Without it, just [`about`].
pub fn about_with(version: Option<(&str, &str)>) -> String {
    match version {
        Some((name, version)) => alloc::format!("{name} {version}\n{}", about()),
        None => about(),
    }
}

/// 1.3's ten fortunes, for `fortune`.
pub const FORTUNES: [&str; 10] = [
    "Every kernel starts with a single boot sector.",
    "Fynbos grows back stronger after a fire. So do rebooted machines.",
    "There is no cloud, only the tablecloth over the mountain.",
    "A bug in the hand is worth two in the backlog.",
    "The south-easter clears the sky; the reset button clears the RAM.",
    "Small programs have small bugs. Mostly.",
    "When in doubt, print it out.",
    "The mountain was here first. It can wait for your build to finish.",
    "Big rock. Pretty rock. Pretty big rock.",
    "Interrupts are just the hardware asking for attention.",
];

/// The text of `fortune`: one of [`FORTUNES`], chosen by `seed` (any number that differs from
/// one run to the next, such as the clock), and a line end.
pub fn fortune(seed: u64) -> String {
    let mut text = String::from(FORTUNES[(seed % FORTUNES.len() as u64) as usize]);
    text.push('\n');
    text
}

/// The text of `potato`, as 1.3 had it.
pub fn potato() -> String {
    String::from(
        "        .-~~~~-.\n\
         \x20     .'  .     '.\n\
         \x20    /  .    .    \\\n\
         \x20   |      .    .  |\n\
         \x20    \\  .     .   /\n\
         \x20     '-.______.-'\n\
         Hello, potato. <3\n",
    )
}

/// The usage of the command called `name`, for messages about wrong arguments. Commands that
/// aren't listed (`potato`) are just their name.
pub fn usage(name: &str) -> &str {
    COMMANDS
        .iter()
        .map(|(usage, _)| *usage)
        .find(|usage| usage.split(' ').next() == Some(name))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_every_command_but_not_potato() {
        let text = help();
        assert!(text.starts_with("Commands:\n"));
        assert!(text.ends_with('\n'));
        for (usage, what) in COMMANDS {
            assert!(text.contains(usage) && text.contains(what), "{usage}");
        }
        assert!(!text.contains("potato"));
        // Descriptions line up in one column: every line is padded to the widest usage.
        let widest = COMMANDS.iter().map(|(u, _)| u.len()).max().unwrap();
        for (usage, what) in COMMANDS {
            let line = alloc::format!("  {usage:<widest$}  {what}\n");
            assert!(text.contains(&line), "{usage}");
        }
    }

    #[test]
    fn every_listed_command_has_a_distinct_name() {
        let mut names: alloc::vec::Vec<&str> =
            COMMANDS.iter().map(|(u, _)| u.split(' ').next().unwrap()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn usage_lookup() {
        assert_eq!(usage("ls"), "ls [-a] [PATH]");
        assert_eq!(usage("run"), "run APP [FILE...] [-- ARGS...]");
        assert_eq!(usage("potato"), "potato");
        assert_eq!(usage("nonsense"), "nonsense");
        // A command is not found by a prefix of its name.
        assert_eq!(usage("l"), "l");
    }

    #[test]
    fn the_potato() {
        let p = potato();
        assert!(p.ends_with("Hello, potato. <3\n"));
        assert_eq!(p.lines().count(), 7);
        assert!(p.lines().all(|l| l.is_ascii()));
        assert!(p.contains("        .-~~~~-.\n"));
        assert!(p.contains("\n     /  .    .    \\\n"));
        assert!(p.contains("\n     \\  .     .   /\n"));
    }

    #[test]
    fn every_command_is_listed_with_what_it_does() {
        // The commands of note 0026, section 1, and M5's, none missing.
        let listed: alloc::vec::Vec<&str> =
            COMMANDS.iter().map(|(u, _)| u.split(' ').next().unwrap()).collect();
        for name in [
            "help", "about", "fetch", "clear", "time", "tz", "uptime", "mem", "echo", "calc", "ls",
            "cd", "pwd", "cat", "info", "mkdir", "write", "append", "cp", "mv", "rm", "edit",
            "volumes", "df", "disk", "check", "format", "setup", "whoami", "settings", "set",
            "users", "adduser", "run", "open", "apps", "grants", "revoke", "stop", "fortune",
            "shutdown", "poweroff", "reboot",
        ] {
            assert!(listed.contains(&name), "{name} is not in help");
        }
        assert!(COMMANDS.iter().all(|(u, what)| !u.is_empty() && !what.is_empty()));
    }

    #[test]
    fn help_tells_of_the_other_names_without_listing_them() {
        let text = help();
        assert!(text.contains("also dir)") && text.contains("(also cls)"));
        for (usage, _) in COMMANDS {
            let name = usage.split(' ').next().unwrap();
            for other in ["dir", "del", "copy", "move", "ren", "md", "rmdir", "type", "cls", "fsck"]
            {
                assert_ne!(name, other, "{other} is another name, not a listed command");
            }
        }
    }

    #[test]
    fn about_with_the_version_first() {
        assert_eq!(about_with(None), about());
        let text = about_with(Some(("PrettyBigRock", "2.0.0-alpha.1")));
        assert!(text.starts_with("PrettyBigRock 2.0.0-alpha.1\nPrettyBigRock 2, in development."));
        assert!(text.ends_with(&about()));
    }

    #[test]
    fn fortunes() {
        for seed in 0..40 {
            let f = fortune(seed);
            assert!(f.ends_with('\n') && f.lines().count() == 1);
            assert_eq!(f.trim_end(), FORTUNES[(seed % 10) as usize]);
        }
        assert_eq!(fortune(u64::MAX), alloc::format!("{}\n", FORTUNES[(u64::MAX % 10) as usize]));
        assert!(FORTUNES.iter().all(|f| f.is_ascii() && !f.is_empty()));
        assert_eq!(FORTUNES[8], "Big rock. Pretty rock. Pretty big rock.");
    }

    #[test]
    fn about_says_where_we_are() {
        let a = about();
        assert!(a.contains("Every app sees only the files it was given"));
        assert!(a.ends_with('\n'));
    }
}
