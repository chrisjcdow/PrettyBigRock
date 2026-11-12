// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Commands: turning the words of a line into a [`Command`] with its arguments checked.
//!
//! Nothing here touches the system. A line goes in; out comes either a typed command (a Rust
//! `enum`, a value that is exactly one of several named variants, each holding its own
//! arguments), or an error saying what was wrong in words the user can act on, with the
//! command's usage line.
//!
//! The rules, for every command:
//! - options (`-a`, `-r`) may come anywhere among the arguments; `--` ends them, so a name that
//!   begins with `-` can still be used; a command without options refuses anything that looks
//!   like one rather than treat it as a name;
//! - too few and too many arguments are both errors, naming what is missing or unexpected;
//! - text-taking commands (`echo`, `write`, `append`, the end of `adduser`, the end of `run`)
//!   take the rest of the words as they are and join them with single spaces;
//! - sizes the services limit (names, labels, arguments) are checked here, because the services
//!   would only refuse an over-long one as "malformed", which tells the user nothing.

use crate::help;
use crate::words::{self, WordsError};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use pbr_interfaces::pbr_users::UserKind;

/// The longest app name `pbr.sandbox` accepts, in bytes.
pub const MAX_APP: usize = 64;
/// The longest arguments for an app `pbr.sandbox` accepts, in bytes.
pub const MAX_APP_ARGS: usize = 3072;
/// The longest user name `pbr.users` accepts, in bytes.
pub const MAX_USER_NAME: usize = 32;
/// The longest setting name `pbr.users` accepts, in bytes.
pub const MAX_SETTING_KEY: usize = 32;
/// The longest setting value `pbr.users` accepts, in bytes.
pub const MAX_SETTING_VALUE: usize = 128;
/// The longest display name `pbr.users` accepts, in bytes.
pub const MAX_DISPLAY_NAME: usize = 128;
/// The longest volume label `pbr.users` accepts, in bytes.
pub const MAX_LABEL: usize = 64;

/// Which app a command means: its number, or its name (then the most recently started running
/// app of that name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRef {
    /// An app's number, as `apps` shows it.
    Id(u32),
    /// An app's name.
    Name(String),
}

impl fmt::Display for AppRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppRef::Id(n) => write!(f, "{n}"),
            AppRef::Name(name) => f.write_str(name),
        }
    }
}

/// One command, with its arguments. Paths are as typed: the shell makes them absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `help`
    Help,
    /// `about`
    About,
    /// `clear`
    Clear,
    /// `echo TEXT…`: the words joined by single spaces.
    Echo(String),
    /// `ls [-a] [PATH]`
    Ls {
        /// Show names beginning with `.` too.
        all: bool,
        /// The folder, or the current one.
        path: Option<String>,
    },
    /// `cd [PATH]`: `/Home` if there is no path.
    Cd(Option<String>),
    /// `pwd`
    Pwd,
    /// `cat FILE`
    Cat(String),
    /// `info PATH`
    Info(String),
    /// `mkdir PATH`
    Mkdir(String),
    /// `write FILE TEXT…`
    Write {
        /// The file.
        file: String,
        /// The words joined by single spaces (the shell adds the final `\n`).
        text: String,
    },
    /// `append FILE TEXT…`
    Append {
        /// The file.
        file: String,
        /// The words joined by single spaces (the shell adds the final `\n`).
        text: String,
    },
    /// `cp [-r] FROM TO`
    Cp {
        /// Copy a folder and everything in it.
        recursive: bool,
        /// What to copy.
        from: String,
        /// Where to: a new name, or a folder to copy into.
        to: String,
    },
    /// `mv FROM TO`
    Mv {
        /// What to move.
        from: String,
        /// Where to: a new name, or a folder to move into.
        to: String,
    },
    /// `rm [-r] PATH`
    Rm {
        /// Remove a folder and everything in it.
        recursive: bool,
        /// What to remove.
        path: String,
    },
    /// `volumes`
    Volumes,
    /// `df`
    Df,
    /// `format VOLUME LABEL`
    Format {
        /// The volume's number, from `volumes`.
        volume: u32,
        /// The label to give it.
        label: String,
    },
    /// `setup VOLUME NAME`
    Setup {
        /// The volume's number, from `volumes`.
        volume: u32,
        /// The first administrator's name.
        name: String,
    },
    /// `whoami`
    Whoami,
    /// `settings`
    Settings,
    /// `set KEY VALUE`
    Set {
        /// The setting's name (`layout`, `timezone`).
        key: String,
        /// What to set it to.
        value: String,
    },
    /// `users`
    Users,
    /// `adduser NAME [admin|standard] [DISPLAY NAME…]`
    AddUser {
        /// The new user's name.
        name: String,
        /// Administrator or standard (standard if not said).
        kind: UserKind,
        /// The name people see (the user name if not said).
        display: String,
    },
    /// `run APP [FILE…] [-- ARGS…]`
    Run {
        /// The app.
        app: String,
        /// The files to give it, as typed.
        files: Vec<String>,
        /// The arguments after `--`, joined by single spaces.
        args: String,
    },
    /// `apps`
    Apps,
    /// `grants APP`
    Grants(AppRef),
    /// `revoke APP PATH`
    Revoke {
        /// The app.
        app: AppRef,
        /// The path to take back, as typed.
        path: String,
    },
    /// `stop APP`
    Stop(AppRef),
    /// `shutdown`
    Shutdown,
    /// `potato`
    Potato,
}

impl Command {
    /// The word that typed this command, for messages (`ls: …`).
    pub fn name(&self) -> &'static str {
        match self {
            Command::Help => "help",
            Command::About => "about",
            Command::Clear => "clear",
            Command::Echo(_) => "echo",
            Command::Ls { .. } => "ls",
            Command::Cd(_) => "cd",
            Command::Pwd => "pwd",
            Command::Cat(_) => "cat",
            Command::Info(_) => "info",
            Command::Mkdir(_) => "mkdir",
            Command::Write { .. } => "write",
            Command::Append { .. } => "append",
            Command::Cp { .. } => "cp",
            Command::Mv { .. } => "mv",
            Command::Rm { .. } => "rm",
            Command::Volumes => "volumes",
            Command::Df => "df",
            Command::Format { .. } => "format",
            Command::Setup { .. } => "setup",
            Command::Whoami => "whoami",
            Command::Settings => "settings",
            Command::Set { .. } => "set",
            Command::Users => "users",
            Command::AddUser { .. } => "adduser",
            Command::Run { .. } => "run",
            Command::Apps => "apps",
            Command::Grants(_) => "grants",
            Command::Revoke { .. } => "revoke",
            Command::Stop(_) => "stop",
            Command::Shutdown => "shutdown",
            Command::Potato => "potato",
        }
    }
}

/// What is wrong with a command's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// An option the command doesn't have (`-x`, or `--long`).
    UnknownOption(String),
    /// A required argument is missing: its name.
    Missing(&'static str),
    /// An argument too many: the first extra one.
    Extra(String),
    /// A number was needed.
    NotANumber {
        /// What the number is (`VOLUME`).
        what: &'static str,
        /// What was typed.
        text: String,
    },
    /// Longer than a service accepts.
    TooLong {
        /// What it is.
        what: &'static str,
        /// The most bytes allowed.
        max: usize,
    },
}

/// Why a line isn't a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// No command by that name.
    UnknownCommand(String),
    /// A command, with arguments that don't fit.
    Bad {
        /// The command's name.
        command: &'static str,
        /// What is wrong.
        problem: Problem,
    },
}

/// Cuts a long argument down so that an error message stays a line.
fn shorten(text: &str) -> String {
    match text.char_indices().nth(40) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => String::from(text),
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnknownCommand(word) => {
                write!(f, "{}: no such command. Type help for the commands.", shorten(word))
            }
            ParseError::Bad { command, problem } => {
                write!(f, "{command}: ")?;
                match problem {
                    Problem::UnknownOption(o) => write!(f, "unknown option {}", shorten(o))?,
                    Problem::Missing(what) => write!(f, "{what} is missing")?,
                    Problem::Extra(x) => {
                        write!(f, "too many arguments (didn't expect \"{}\")", shorten(x))?;
                    }
                    Problem::NotANumber { what, text } => {
                        write!(f, "{what} must be a number, not \"{}\"", shorten(text))?;
                    }
                    Problem::TooLong { what, max } => {
                        write!(f, "{what} is too long ({max} bytes at most)")?;
                    }
                }
                write!(f, ". Usage: {}", help::usage(command))
            }
        }
    }
}

/// Why a line couldn't be turned into a command: the words, or the arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineError {
    /// The line couldn't be split into words.
    Words(WordsError),
    /// The words aren't a command.
    Parse(ParseError),
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineError::Words(e) => write!(f, "shell: {e}"),
            LineError::Parse(e) => e.fmt(f),
        }
    }
}

/// Reads one line typed at the prompt: `Ok(None)` for an empty line, which does nothing.
pub fn parse_line(line: &str) -> Result<Option<Command>, LineError> {
    let words = words::split(line).map_err(LineError::Words)?;
    parse(&words).map_err(LineError::Parse)
}

/// Reads a command from its words: `Ok(None)` if there are none.
pub fn parse(words: &[String]) -> Result<Option<Command>, ParseError> {
    let Some((name, args)) = words.split_first() else { return Ok(None) };
    let command = match name.as_str() {
        "help" => no_arguments("help", args, Command::Help)?,
        "about" => no_arguments("about", args, Command::About)?,
        "clear" => no_arguments("clear", args, Command::Clear)?,
        "pwd" => no_arguments("pwd", args, Command::Pwd)?,
        "volumes" => no_arguments("volumes", args, Command::Volumes)?,
        "df" => no_arguments("df", args, Command::Df)?,
        "whoami" => no_arguments("whoami", args, Command::Whoami)?,
        "settings" => no_arguments("settings", args, Command::Settings)?,
        "users" => no_arguments("users", args, Command::Users)?,
        "apps" => no_arguments("apps", args, Command::Apps)?,
        "shutdown" => no_arguments("shutdown", args, Command::Shutdown)?,
        "potato" => no_arguments("potato", args, Command::Potato)?,
        "echo" => Command::Echo(args.join(" ")),
        "ls" => {
            let (flags, operands) = scan("ls", args, "a")?;
            let path = at_most_one("ls", &operands)?.map(String::from);
            Command::Ls { all: flags.contains(&'a'), path }
        }
        "cd" => {
            let (_, operands) = scan("cd", args, "")?;
            Command::Cd(at_most_one("cd", &operands)?.map(String::from))
        }
        "cat" => Command::Cat(one_path("cat", "FILE", args)?),
        "info" => Command::Info(one_path("info", "PATH", args)?),
        "mkdir" => Command::Mkdir(one_path("mkdir", "PATH", args)?),
        "write" | "append" => {
            let command = if name == "write" { "write" } else { "append" };
            let Some((file, text)) = args.split_first() else {
                return Err(bad(command, Problem::Missing("FILE")));
            };
            let (file, text) = (file.clone(), text.join(" "));
            if command == "write" {
                Command::Write { file, text }
            } else {
                Command::Append { file, text }
            }
        }
        "cp" => {
            let (flags, operands) = scan("cp", args, "r")?;
            let [from, to] = fixed("cp", &operands, ["FROM", "TO"])?;
            Command::Cp { recursive: flags.contains(&'r'), from: from.into(), to: to.into() }
        }
        "mv" => {
            let (_, operands) = scan("mv", args, "")?;
            let [from, to] = fixed("mv", &operands, ["FROM", "TO"])?;
            Command::Mv { from: from.into(), to: to.into() }
        }
        "rm" => {
            let (flags, operands) = scan("rm", args, "r")?;
            let [path] = fixed("rm", &operands, ["PATH"])?;
            Command::Rm { recursive: flags.contains(&'r'), path: path.into() }
        }
        "format" => {
            let (_, operands) = scan("format", args, "")?;
            let [volume, label] = fixed("format", &operands, ["VOLUME", "LABEL"])?;
            Command::Format {
                volume: number("format", "VOLUME", volume)?,
                label: limited("format", "LABEL", label, MAX_LABEL)?,
            }
        }
        "setup" => {
            let (_, operands) = scan("setup", args, "")?;
            let [volume, user] = fixed("setup", &operands, ["VOLUME", "NAME"])?;
            Command::Setup {
                volume: number("setup", "VOLUME", volume)?,
                name: limited("setup", "NAME", user, MAX_USER_NAME)?,
            }
        }
        "set" => {
            let (_, operands) = scan("set", args, "")?;
            let [key, value] = fixed("set", &operands, ["KEY", "VALUE"])?;
            Command::Set {
                key: limited("set", "KEY", key, MAX_SETTING_KEY)?,
                value: limited("set", "VALUE", value, MAX_SETTING_VALUE)?,
            }
        }
        "adduser" => add_user(args)?,
        "run" => run(args)?,
        "grants" => {
            let (_, operands) = scan("grants", args, "")?;
            let [app] = fixed("grants", &operands, ["APP"])?;
            Command::Grants(app_ref("grants", app)?)
        }
        "revoke" => {
            let (_, operands) = scan("revoke", args, "")?;
            let [app, path] = fixed("revoke", &operands, ["APP", "PATH"])?;
            Command::Revoke { app: app_ref("revoke", app)?, path: path.into() }
        }
        "stop" => {
            let (_, operands) = scan("stop", args, "")?;
            let [app] = fixed("stop", &operands, ["APP"])?;
            Command::Stop(app_ref("stop", app)?)
        }
        other => return Err(ParseError::UnknownCommand(other.into())),
    };
    Ok(Some(command))
}

fn bad(command: &'static str, problem: Problem) -> ParseError {
    ParseError::Bad { command, problem }
}

/// Whether a word is an option: it starts with `-` and is more than a lone `-` (which is
/// an ordinary argument, by convention meaning "standard input" elsewhere).
fn is_option(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-')
}

/// Separates `args` into the single-letter options in `allowed` (the flags found) and the other
/// arguments. A `--` ends the options. Anything else that looks like an option is an error.
///
/// The `'a` in the signature is a *lifetime*: it says the returned `&str`s borrow from `args`
/// and can't outlive it, so no text is copied.
fn scan<'a>(
    command: &'static str,
    args: &'a [String],
    allowed: &str,
) -> Result<(Vec<char>, Vec<&'a str>), ParseError> {
    let mut flags = Vec::new();
    let mut operands = Vec::new();
    let mut options_over = false;
    for arg in args {
        if options_over || !is_option(arg) {
            operands.push(arg.as_str());
        } else if arg == "--" {
            options_over = true;
        } else if arg.starts_with("--") {
            return Err(bad(command, Problem::UnknownOption(arg.clone())));
        } else {
            for c in arg[1..].chars() {
                if !allowed.contains(c) {
                    return Err(bad(command, Problem::UnknownOption(format!("-{c}"))));
                }
                if !flags.contains(&c) {
                    flags.push(c);
                }
            }
        }
    }
    Ok((flags, operands))
}

/// A command that takes nothing: `command` itself if `args` is empty.
fn no_arguments(
    name: &'static str,
    args: &[String],
    command: Command,
) -> Result<Command, ParseError> {
    match args.first() {
        None => Ok(command),
        Some(extra) if is_option(extra) => Err(bad(name, Problem::UnknownOption(extra.clone()))),
        Some(extra) => Err(bad(name, Problem::Extra(extra.clone()))),
    }
}

/// Exactly `N` arguments, called `names`. (`N` is a *const generic*: a number that is part of
/// the function's type, so the answer is an array with exactly `N` places and the caller can
/// unpack it with `let [from, to] = …`.)
fn fixed<'a, const N: usize>(
    command: &'static str,
    operands: &[&'a str],
    names: [&'static str; N],
) -> Result<[&'a str; N], ParseError> {
    if operands.len() < N {
        return Err(bad(command, Problem::Missing(names[operands.len()])));
    }
    if operands.len() > N {
        return Err(bad(command, Problem::Extra(operands[N].into())));
    }
    Ok(core::array::from_fn(|i| operands[i]))
}

/// No argument or one.
fn at_most_one<'a>(
    command: &'static str,
    operands: &[&'a str],
) -> Result<Option<&'a str>, ParseError> {
    match operands {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        [_, extra, ..] => Err(bad(command, Problem::Extra((*extra).into()))),
    }
}

/// A command whose only argument is one path, with no options.
fn one_path(
    command: &'static str,
    what: &'static str,
    args: &[String],
) -> Result<String, ParseError> {
    let (_, operands) = scan(command, args, "")?;
    let [path] = fixed(command, &operands, [what])?;
    Ok(path.into())
}

/// A whole number: only digits, and it fits in 32 bits.
fn number(command: &'static str, what: &'static str, text: &str) -> Result<u32, ParseError> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    match text.parse::<u32>() {
        Ok(n) if digits => Ok(n),
        _ => Err(bad(command, Problem::NotANumber { what, text: text.into() })),
    }
}

/// `text`, if it is at most `max` bytes.
fn limited(
    command: &'static str,
    what: &'static str,
    text: &str,
    max: usize,
) -> Result<String, ParseError> {
    if text.len() > max {
        return Err(bad(command, Problem::TooLong { what, max }));
    }
    Ok(text.into())
}

/// An app: all digits means its number, anything else its name.
fn app_ref(command: &'static str, text: &str) -> Result<AppRef, ParseError> {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return number(command, "APP", text).map(AppRef::Id);
    }
    limited(command, "APP", text, MAX_APP).map(AppRef::Name)
}

/// `adduser NAME [admin|standard] [DISPLAY NAME…]`.
fn add_user(args: &[String]) -> Result<Command, ParseError> {
    let Some((name, rest)) = args.split_first() else {
        return Err(bad("adduser", Problem::Missing("NAME")));
    };
    let name = limited("adduser", "NAME", name, MAX_USER_NAME)?;
    // The kind is optional, so the first word after the name is the kind only if it says so;
    // otherwise it is the start of the display name.
    let (kind, rest) = match rest.split_first() {
        Some((word, tail)) if word == "admin" => (UserKind::Admin, tail),
        Some((word, tail)) if word == "standard" => (UserKind::Standard, tail),
        _ => (UserKind::Standard, rest),
    };
    let display = if rest.is_empty() { name.clone() } else { rest.join(" ") };
    let display = limited("adduser", "DISPLAY NAME", &display, MAX_DISPLAY_NAME)?;
    Ok(Command::AddUser { name, kind, display })
}

/// `run APP [FILE…] [-- ARGS…]`.
fn run(args: &[String]) -> Result<Command, ParseError> {
    let Some((app, rest)) = args.split_first() else {
        return Err(bad("run", Problem::Missing("APP")));
    };
    if app == "--" {
        return Err(bad("run", Problem::Missing("APP")));
    }
    if is_option(app) {
        return Err(bad("run", Problem::UnknownOption(app.clone())));
    }
    let app = limited("run", "APP", app, MAX_APP)?;
    let (files, app_args) = match rest.iter().position(|w| w == "--") {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    if let Some(option) = files.iter().find(|w| is_option(w)) {
        return Err(bad("run", Problem::UnknownOption(option.clone())));
    }
    let args = limited("run", "the arguments", &app_args.join(" "), MAX_APP_ARGS)?;
    Ok(Command::Run { app, files: files.to_vec(), args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn ok(line: &str) -> Command {
        parse_line(line).unwrap().unwrap()
    }

    fn err(line: &str) -> String {
        match parse_line(line) {
            Err(e) => e.to_string(),
            other => panic!("{line:?} should be refused, got {other:?}"),
        }
    }

    fn s(text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn empty_lines_do_nothing() {
        assert_eq!(parse_line(""), Ok(None));
        assert_eq!(parse_line("    "), Ok(None));
        assert_eq!(parse(&[]), Ok(None));
    }

    #[test]
    fn simple_commands() {
        assert_eq!(ok("help"), Command::Help);
        assert_eq!(ok("  about  "), Command::About);
        assert_eq!(ok("clear"), Command::Clear);
        assert_eq!(ok("pwd"), Command::Pwd);
        assert_eq!(ok("volumes"), Command::Volumes);
        assert_eq!(ok("df"), Command::Df);
        assert_eq!(ok("whoami"), Command::Whoami);
        assert_eq!(ok("settings"), Command::Settings);
        assert_eq!(
            ok("set layout us-intl"),
            Command::Set { key: "layout".into(), value: "us-intl".into() }
        );
        assert_eq!(
            ok("set timezone +02:00"),
            Command::Set { key: "timezone".into(), value: "+02:00".into() }
        );
        assert!(err("set").contains("KEY is missing"));
        assert!(err("set layout").contains("VALUE is missing"));
        assert!(err("set layout gb us").contains("too many"));
        assert_eq!(ok("users"), Command::Users);
        assert_eq!(ok("apps"), Command::Apps);
        assert_eq!(ok("shutdown"), Command::Shutdown);
        assert_eq!(ok("potato"), Command::Potato);
    }

    #[test]
    fn echo_joins_words() {
        assert_eq!(ok("echo"), Command::Echo(s("")));
        assert_eq!(ok("echo  a   b  "), Command::Echo(s("a b")));
        assert_eq!(ok(r#"echo "a  b" -n --"#), Command::Echo(s("a  b -n --")));
    }

    #[test]
    fn ls_and_cd() {
        assert_eq!(ok("ls"), Command::Ls { all: false, path: None });
        assert_eq!(ok("ls -a"), Command::Ls { all: true, path: None });
        assert_eq!(ok("ls /Home -a"), Command::Ls { all: true, path: Some(s("/Home")) });
        assert_eq!(ok("ls -aa x"), Command::Ls { all: true, path: Some(s("x")) });
        assert_eq!(ok("ls -- -a"), Command::Ls { all: false, path: Some(s("-a")) });
        assert_eq!(ok("ls -"), Command::Ls { all: false, path: Some(s("-")) });
        assert_eq!(ok(r#"ls "My Files""#), Command::Ls { all: false, path: Some(s("My Files")) });
        assert_eq!(ok("cd"), Command::Cd(None));
        assert_eq!(ok("cd .."), Command::Cd(Some(s(".."))));
        assert!(err("cd a b").contains("didn't expect \"b\""));
        assert!(err("cd -x").contains("unknown option -x"));
    }

    #[test]
    fn one_path_commands() {
        assert_eq!(ok("cat a.txt"), Command::Cat(s("a.txt")));
        assert_eq!(ok("info /x"), Command::Info(s("/x")));
        assert_eq!(ok("mkdir new"), Command::Mkdir(s("new")));
        assert_eq!(err("cat"), "cat: FILE is missing. Usage: cat FILE");
        assert_eq!(err("mkdir"), "mkdir: PATH is missing. Usage: mkdir PATH");
        assert!(err("info a b").starts_with("info: too many arguments"));
        assert!(err("cat -n a").starts_with("cat: unknown option -n"));
    }

    #[test]
    fn write_and_append() {
        assert_eq!(
            ok("write a.txt hello world"),
            Command::Write { file: s("a.txt"), text: s("hello world") }
        );
        assert_eq!(
            ok(r#"write a.txt "two  words" -x"#),
            Command::Write { file: s("a.txt"), text: s("two  words -x") }
        );
        assert_eq!(ok("write a.txt"), Command::Write { file: s("a.txt"), text: s("") });
        assert_eq!(ok(r#"append log "" """#), Command::Append { file: s("log"), text: s(" ") });
        assert_eq!(err("write"), "write: FILE is missing. Usage: write FILE TEXT...");
        assert_eq!(err("append"), "append: FILE is missing. Usage: append FILE TEXT...");
    }

    #[test]
    fn copy_move_remove() {
        assert_eq!(ok("cp a b"), Command::Cp { recursive: false, from: s("a"), to: s("b") });
        assert_eq!(ok("cp -r a b"), Command::Cp { recursive: true, from: s("a"), to: s("b") });
        assert_eq!(ok("cp a b -r"), Command::Cp { recursive: true, from: s("a"), to: s("b") });
        assert_eq!(ok("cp -- -r b"), Command::Cp { recursive: false, from: s("-r"), to: s("b") });
        assert_eq!(ok("mv a b"), Command::Mv { from: s("a"), to: s("b") });
        assert_eq!(ok("rm a"), Command::Rm { recursive: false, path: s("a") });
        assert_eq!(ok("rm -r a"), Command::Rm { recursive: true, path: s("a") });
        assert_eq!(err("cp a"), "cp: TO is missing. Usage: cp [-r] FROM TO");
        assert_eq!(err("cp"), "cp: FROM is missing. Usage: cp [-r] FROM TO");
        assert!(err("cp a b c").contains("didn't expect \"c\""));
        assert!(err("mv -r a b").contains("unknown option -r"));
        assert!(err("rm -rf a").contains("unknown option -f"));
        assert!(err("rm --recursive a").contains("unknown option --recursive"));
        assert!(err("rm").contains("PATH is missing"));
        assert!(err("rm a b").contains("too many"));
        assert!(err("rm -r").contains("PATH is missing"));
    }

    #[test]
    fn disks() {
        assert_eq!(ok("format 3 Data"), Command::Format { volume: 3, label: s("Data") });
        assert_eq!(
            ok(r#"format 12 "My Disk""#),
            Command::Format { volume: 12, label: s("My Disk") }
        );
        assert_eq!(ok("setup 1 chris"), Command::Setup { volume: 1, name: s("chris") });
        assert!(err("format x Data").contains("VOLUME must be a number, not \"x\""));
        assert!(err("format -1 Data").contains("unknown option -1"));
        assert!(err("format +1 Data").contains("must be a number"));
        assert!(err("format 99999999999 Data").contains("must be a number"));
        assert!(err("format 3").contains("LABEL is missing"));
        assert!(err("format").contains("VOLUME is missing"));
        assert!(
            err(&format!("format 1 {}", "x".repeat(65)))
                .contains("LABEL is too long (64 bytes at most)")
        );
        assert!(parse_line(&format!("format 1 {}", "x".repeat(64))).is_ok());
        assert!(err(&format!("setup 1 {}", "x".repeat(33))).contains("NAME is too long"));
        assert!(err("setup 1").contains("NAME is missing"));
        assert!(err("setup 1 a b").contains("too many"));
    }

    #[test]
    fn users() {
        let add = |name: &str, kind, display: &str| Command::AddUser {
            name: s(name),
            kind,
            display: s(display),
        };
        assert_eq!(ok("adduser alice"), add("alice", UserKind::Standard, "alice"));
        assert_eq!(ok("adduser alice admin"), add("alice", UserKind::Admin, "alice"));
        assert_eq!(ok("adduser alice standard"), add("alice", UserKind::Standard, "alice"));
        assert_eq!(
            ok("adduser alice Alice Brown"),
            add("alice", UserKind::Standard, "Alice Brown")
        );
        assert_eq!(
            ok("adduser alice admin Alice Brown"),
            add("alice", UserKind::Admin, "Alice Brown")
        );
        assert_eq!(
            ok(r#"adduser alice admin "Alice  B""#),
            add("alice", UserKind::Admin, "Alice  B")
        );
        // Only the exact words choose a kind.
        assert_eq!(ok("adduser alice Admin"), add("alice", UserKind::Standard, "Admin"));
        assert_eq!(
            err("adduser"),
            "adduser: NAME is missing. Usage: adduser NAME [admin|standard] [DISPLAY NAME...]"
        );
        assert!(err(&format!("adduser {}", "a".repeat(33))).contains("NAME is too long"));
        assert!(
            err(&format!("adduser a {}", "x".repeat(129))).contains("DISPLAY NAME is too long")
        );
        assert!(parse_line(&format!("adduser a {}", "x".repeat(128))).is_ok());
    }

    #[test]
    fn run_command() {
        let run = |app: &str, files: &[&str], args: &str| Command::Run {
            app: s(app),
            files: files.iter().map(|f| s(f)).collect(),
            args: s(args),
        };
        assert_eq!(ok("run notes"), run("notes", &[], ""));
        assert_eq!(ok("run notes a.txt b.txt"), run("notes", &["a.txt", "b.txt"], ""));
        assert_eq!(ok("run notes a.txt -- x y"), run("notes", &["a.txt"], "x y"));
        assert_eq!(ok("run notes -- x  y"), run("notes", &[], "x y"));
        assert_eq!(ok("run notes --"), run("notes", &[], ""));
        assert_eq!(ok(r#"run notes "a b.txt""#), run("notes", &["a b.txt"], ""));
        // Everything after the first `--` is an argument, options and all.
        assert_eq!(ok("run notes -- -a -- -b"), run("notes", &[], "-a -- -b"));
        assert_eq!(err("run"), "run: APP is missing. Usage: run APP [FILE...] [-- ARGS...]");
        assert!(err("run --").contains("APP is missing"));
        assert!(err("run -x").contains("unknown option -x"));
        assert!(err("run notes -x").contains("unknown option -x"));
        assert!(err(&format!("run {}", "a".repeat(65))).contains("APP is too long"));
        assert!(
            err(&format!("run a -- {}", "x".repeat(3073))).contains("the arguments is too long")
        );
        assert!(parse_line(&format!("run a -- {}", "x".repeat(3072))).is_ok());
    }

    #[test]
    fn apps_and_permissions() {
        assert_eq!(ok("grants 7"), Command::Grants(AppRef::Id(7)));
        assert_eq!(ok("grants notes"), Command::Grants(AppRef::Name(s("notes"))));
        assert_eq!(ok("stop 12"), Command::Stop(AppRef::Id(12)));
        assert_eq!(
            ok("revoke notes /Home/Documents"),
            Command::Revoke { app: AppRef::Name(s("notes")), path: s("/Home/Documents") }
        );
        assert_eq!(ok("revoke 3 x"), Command::Revoke { app: AppRef::Id(3), path: s("x") });
        // An app called by a number that doesn't fit is an error, not a name.
        assert!(err("grants 99999999999").contains("APP must be a number"));
        // A name that merely starts with digits is a name.
        assert_eq!(ok("stop 7zip"), Command::Stop(AppRef::Name(s("7zip"))));
        assert!(err("grants").contains("APP is missing"));
        assert!(err("revoke 3").contains("PATH is missing"));
        assert!(err("stop").contains("APP is missing"));
        assert!(err("stop 1 2").contains("too many"));
        assert!(err(&format!("stop {}", "a".repeat(65))).contains("APP is too long"));
    }

    #[test]
    fn commands_without_arguments_refuse_them() {
        for line in [
            "help x",
            "about x",
            "clear x",
            "pwd x",
            "volumes x",
            "df x",
            "whoami x",
            "settings x",
            "users x",
            "apps x",
            "shutdown x",
            "potato x",
        ] {
            let e = err(line);
            assert!(e.contains("too many arguments"), "{line}: {e}");
        }
        assert!(err("shutdown -f").contains("unknown option -f"));
        assert_eq!(err("pwd x"), "pwd: too many arguments (didn't expect \"x\"). Usage: pwd");
        // Options anywhere count as options.
        assert!(err("df -h").contains("unknown option -h"));
    }

    #[test]
    fn unknown_commands() {
        assert_eq!(
            err("frobnicate now"),
            "frobnicate: no such command. Type help for the commands."
        );
        assert_eq!(err("LS"), "LS: no such command. Type help for the commands.");
        assert_eq!(err("?"), "?: no such command. Type help for the commands.");
        assert_eq!(err("/bin/ls"), "/bin/ls: no such command. Type help for the commands.");
        // A huge word doesn't make a huge message.
        let long = err(&"x".repeat(4000));
        assert!(long.len() < 100, "{long}");
        assert!(long.contains("..."));
    }

    #[test]
    fn broken_lines() {
        assert_eq!(err(r#"echo "oops"#), "shell: a quote was opened but never closed");
        assert!(err(&"x ".repeat(2049)).contains("too long"));
        // The longest allowed line, as one word and as many.
        assert!(matches!(
            parse_line(&"x".repeat(4096)),
            Err(LineError::Parse(ParseError::UnknownCommand(_)))
        ));
        assert_eq!(ok(&format!("echo {}", "ab ".repeat(1363))).name(), "echo");
    }

    #[test]
    fn hostile_input_never_panics() {
        // A spread of odd lines: whatever they are, they give a command or an error.
        let pieces = [
            "",
            " ",
            "-",
            "--",
            "---",
            "-a",
            "-r",
            "\"",
            "\"\"",
            "\\",
            "\\\"",
            "..",
            "/",
            "//",
            "é",
            "ls",
            "cp",
            "rm",
            "run",
            "adduser",
            "format",
            "revoke",
            "99999999999999999999",
            "\u{0}",
        ];
        for a in pieces {
            for b in pieces {
                for c in pieces {
                    let _ = parse_line(&format!("{a} {b} {c}"));
                    let _ = parse_line(&format!("ls {a} {b} {c}"));
                    let _ = parse_line(&format!("run {a} {b} {c}"));
                    let _ = parse_line(&format!("adduser {a} {b} {c}"));
                    let _ = parse_line(&format!("revoke {a} {b} {c}"));
                }
            }
        }
    }

    #[test]
    fn every_command_name_matches_its_help_entry() {
        for line in [
            "help",
            "about",
            "clear",
            "echo",
            "ls",
            "cd",
            "pwd",
            "cat a",
            "info a",
            "mkdir a",
            "write a",
            "append a",
            "cp a b",
            "mv a b",
            "rm a",
            "volumes",
            "df",
            "format 1 a",
            "setup 1 a",
            "whoami",
            "settings",
            "set a b",
            "users",
            "adduser a",
            "run a",
            "apps",
            "grants 1",
            "revoke 1 a",
            "stop 1",
            "shutdown",
            "potato",
        ] {
            let name = ok(line).name();
            assert_eq!(line.split(' ').next(), Some(name));
            // Everything but `potato` is in `help`.
            let listed = help::COMMANDS.iter().any(|(u, _)| u.split(' ').next() == Some(name));
            assert_eq!(listed, name != "potato", "{name}");
        }
        assert_eq!(vec![ok("help")].len(), 1);
    }
}
