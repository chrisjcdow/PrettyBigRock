// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The words the shell says about itself: `help`, `about`, `potato`, and the usage line each
//! command shows when it is given the wrong arguments.
//!
//! `COMMANDS` is the one list of commands that the user is told about. `potato` is deliberately
//! missing from it, as in 1.x ("you have to know about it").

use alloc::string::String;

/// Every command people are told about: (usage, what it does), in the order `help` shows them.
/// The first word of the usage is the command's name. Uppercase words are things you type;
/// `[ ]` are optional.
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "this list"),
    ("about", "about PrettyBigRock"),
    ("clear", "clear the screen"),
    ("echo TEXT...", "print text"),
    ("ls [-a] [PATH]", "list a folder (-a shows names starting with a dot)"),
    ("cd [PATH]", "go to a folder (/Home if no PATH)"),
    ("pwd", "show the current folder"),
    ("cat FILE", "show a file (up to 64 KB)"),
    ("info PATH", "kind, size, rights and times of a file or folder"),
    ("mkdir PATH", "make a folder"),
    ("write FILE TEXT...", "replace a file's contents with a line of text"),
    ("append FILE TEXT...", "add a line of text to the end of a file"),
    ("cp [-r] FROM TO", "copy a file (-r: a folder and all that is in it)"),
    ("mv FROM TO", "move or rename"),
    ("rm [-r] PATH", "remove a file or an empty folder (-r: a folder and all that is in it)"),
    ("volumes", "list the disks and partitions (administrators)"),
    ("df", "size and free space of your home and each volume"),
    ("format VOLUME LABEL", "put RockFS on an empty volume (administrators; no questions asked)"),
    ("setup VOLUME NAME", "make a volume the system volume at the next start (administrators)"),
    ("whoami", "your user name"),
    ("users", "list the users (administrators)"),
    ("adduser NAME [admin|standard] [DISPLAY NAME...]", "add a user (administrators)"),
    ("run APP [FILE...] [-- ARGS...]", "start an app, giving it the FILEs"),
    ("apps", "list the apps of this session"),
    ("grants APP", "show what an app can reach, and why"),
    ("revoke APP PATH", "take PATH back from a running app"),
    ("stop APP", "stop an app"),
    ("shutdown", "save everything, then it is safe to turn off the computer (administrators)"),
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
         This is milestone 5: files and sandboxes. Every app sees only the files it was given.\n",
    )
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
    fn about_says_where_we_are() {
        let a = about();
        assert!(a.contains("milestone 5: files and sandboxes"));
        assert!(a.ends_with('\n'));
    }
}
