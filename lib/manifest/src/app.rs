// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! App manifests (note 0017, section 4): the small text file, `apps/<name>.app`, beside each
//! app in the start-up archive, saying what it is and what it asks for.
//!
//! ```text
//! # apps/notes.app
//! title "Notes"
//! budget 16M
//! files documents read write
//! files pictures read
//! ```
//!
//! A manifest only *asks*; what the app gets is decided by [`crate::decide`], with the user's
//! rules. Anything unexpected refuses the whole manifest, and the app can't be launched.

use crate::lines::{self, Limits, ReadError};
use crate::{Place, Rights, size};
use alloc::string::String;
use alloc::vec::Vec;

/// The largest manifest.
pub const MAX_SIZE: usize = 4096;
/// The longest line.
pub const MAX_LINE: usize = 200;
/// The longest title, in bytes.
pub const MAX_TITLE: usize = 64;
/// The longest `args` value. The sandbox manager adds the launch's own arguments after it, and
/// a program can be given at most 3 KiB.
pub const MAX_ARGS: usize = 1024;
/// The smallest and largest budgets.
pub const MIN_BUDGET: u64 = 64 << 10;
/// See [`MIN_BUDGET`].
pub const MAX_BUDGET: u64 = 1 << 30;

const LIMITS: Limits = Limits { max_size: MAX_SIZE, max_line: MAX_LINE, quotes: true };

/// A parsed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The name people see.
    pub title: String,
    /// Its memory budget in bytes.
    pub budget: u64,
    /// The places it asks for, in the order asked, each once.
    pub places: Vec<(Place, Rights)>,
    /// Whether it asks for the user's whole tree.
    pub trusted: bool,
    /// Whether it asks for the session's terminal.
    pub terminal: bool,
    /// Arguments to start it with.
    pub args: String,
}

/// What was wrong with a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file itself: too long, not text, a broken quote.
    Read(ReadError),
    /// A key that isn't one of the known keys.
    UnknownKey,
    /// A key given twice (or `files` for one place twice).
    Repeated,
    /// A value that's missing, malformed, or where none belongs.
    BadValue,
    /// A size outside what is allowed, or a title or arguments too long.
    OutOfRange,
    /// No `title` or `budget`.
    Missing,
}

impl From<ReadError> for Error {
    fn from(e: ReadError) -> Error {
        Error::Read(e)
    }
}

/// Sets `slot` once: a second time is [`Error::Repeated`].
fn once<T>(slot: &mut Option<T>, v: T) -> Result<(), Error> {
    if slot.is_some() {
        return Err(Error::Repeated);
    }
    *slot = Some(v);
    Ok(())
}

impl App {
    /// Parses a manifest.
    pub fn parse(text: &[u8]) -> Result<App, Error> {
        let (mut title, mut budget, mut args) = (None, None, None);
        let (mut trusted, mut terminal) = (None, None);
        let mut places: Vec<(Place, Rights)> = Vec::new();
        for line in lines::read(text, LIMITS)? {
            match line.key {
                "title" => {
                    let w = line.words()?;
                    let [t] = w.as_slice() else { return Err(Error::BadValue) };
                    if !t.quoted || t.text.trim().is_empty() {
                        return Err(Error::BadValue);
                    }
                    if t.text.len() > MAX_TITLE {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut title, String::from(t.text))?;
                }
                "budget" => {
                    let w = line.plain_words()?;
                    let [s] = w.as_slice() else { return Err(Error::BadValue) };
                    let v = size(s).ok_or(Error::BadValue)?;
                    if !(MIN_BUDGET..=MAX_BUDGET).contains(&v) {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut budget, v)?;
                }
                "files" => {
                    let w = line.plain_words()?;
                    let (place, rights) = w.split_first().ok_or(Error::BadValue)?;
                    let place = Place::from_word(place).ok_or(Error::BadValue)?;
                    let rights = Rights::parse(rights).ok_or(Error::BadValue)?;
                    if places.iter().any(|&(p, _)| p == place) {
                        return Err(Error::Repeated);
                    }
                    places.push((place, rights));
                }
                "trusted" | "terminal" => {
                    if !line.rest.is_empty() {
                        return Err(Error::BadValue);
                    }
                    once(if line.key == "trusted" { &mut trusted } else { &mut terminal }, ())?;
                }
                "args" => {
                    // The arguments are passed as they are written: quotes in them are just
                    // characters to the app (which splits its arguments itself).
                    if line.rest.is_empty() {
                        return Err(Error::BadValue);
                    }
                    if line.rest.len() > MAX_ARGS {
                        return Err(Error::OutOfRange);
                    }
                    once(&mut args, String::from(line.rest))?;
                }
                _ => return Err(Error::UnknownKey),
            }
        }
        match (title, budget) {
            (Some(title), Some(budget)) => Ok(App {
                title,
                budget,
                places,
                trusted: trusted.is_some(),
                terminal: terminal.is_some(),
                args: args.unwrap_or_default(),
            }),
            _ => Err(Error::Missing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    const NOTES: &str = "# apps/notes.app\ntitle \"Chris's Notes \u{2014} #1\"\nbudget 16M\nfiles documents read write\nfiles pictures read # only looking\nargs --plain \"x\"\n";

    #[test]
    fn reads_a_manifest() {
        let a = App::parse(NOTES.as_bytes()).unwrap();
        assert_eq!(a.title, "Chris's Notes \u{2014} #1");
        assert_eq!(a.budget, 16 << 20);
        assert_eq!(
            a.places,
            vec![(Place::Documents, Rights::ReadWrite), (Place::Pictures, Rights::Read)]
        );
        assert!(!a.trusted && !a.terminal);
        assert_eq!(a.args, "--plain \"x\"");
        let s = App::parse(b"title \"Shell\"\r\nbudget 512K\r\ntrusted\r\nterminal\r\n").unwrap();
        assert!(s.trusted && s.terminal && s.places.is_empty() && s.args.is_empty());
        assert_eq!(s.budget, 512 << 10);
    }

    #[test]
    fn refuses_the_wrong() {
        let base = "title \"A\"\nbudget 1M\n";
        let p = |s: &str| App::parse(s.as_bytes());
        assert!(p(base).is_ok());
        let with = |extra: &str| p(&format!("{base}{extra}\n"));
        assert_eq!(p("budget 1M\n"), Err(Error::Missing));
        assert_eq!(p("title \"A\"\n"), Err(Error::Missing));
        assert_eq!(p(""), Err(Error::Missing));
        assert_eq!(with("colour red"), Err(Error::UnknownKey));
        assert_eq!(with("Files documents read"), Err(Error::UnknownKey));
        assert_eq!(with("title \"B\""), Err(Error::Repeated));
        assert_eq!(with("budget 2M"), Err(Error::Repeated));
        assert_eq!(with("trusted\ntrusted"), Err(Error::Repeated));
        assert_eq!(with("terminal\nterminal"), Err(Error::Repeated));
        assert_eq!(with("args a\nargs b"), Err(Error::Repeated));
        assert_eq!(with("files music read\nfiles music read write"), Err(Error::Repeated));
        assert_eq!(p("title A\nbudget 1M\n"), Err(Error::BadValue), "an unquoted title");
        assert_eq!(p("title \"A\" \"B\"\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(p("title \" \"\nbudget 1M\n"), Err(Error::BadValue));
        assert_eq!(p("title\nbudget 1M\n"), Err(Error::BadValue));
        let long = format!("title \"{}\"\nbudget 1M\n", "x".repeat(65));
        assert_eq!(p(&long), Err(Error::OutOfRange));
        assert_eq!(p("title \"A\"\nbudget 1G\n"), Err(Error::BadValue));
        assert_eq!(p("title \"A\"\nbudget 4K\n"), Err(Error::OutOfRange));
        assert_eq!(p("title \"A\"\nbudget 1025M\n"), Err(Error::OutOfRange));
        assert_eq!(p("title \"A\"\nbudget 99999999999999M\n"), Err(Error::BadValue));
        assert_eq!(p("title \"A\"\nbudget 999999999999M\n"), Err(Error::OutOfRange));
        assert_eq!(p("title \"A\"\nbudget \"1M\"\n"), Err(Error::Read(ReadError::BadQuote)));
        assert_eq!(p("title \"A\"\nbudget 1M 2M\n"), Err(Error::BadValue));
        assert_eq!(with("files"), Err(Error::BadValue));
        assert_eq!(with("files documents"), Err(Error::BadValue));
        assert_eq!(with("files documents write"), Err(Error::BadValue));
        assert_eq!(with("files documents write read"), Err(Error::BadValue));
        assert_eq!(with("files documents read write read"), Err(Error::BadValue));
        assert_eq!(with("files home read"), Err(Error::BadValue));
        assert_eq!(with("files ../secret read"), Err(Error::BadValue));
        assert_eq!(with("trusted yes"), Err(Error::BadValue));
        assert_eq!(with("args"), Err(Error::BadValue));
        assert_eq!(
            with(&format!("args {}", "x".repeat(MAX_LINE))),
            Err(Error::Read(ReadError::LineTooLong))
        );
        assert_eq!(p(&format!("{base}{}", "#".repeat(4096))), Err(Error::Read(ReadError::TooLong)));
        assert_eq!(p("title \"A\x01\"\nbudget 1M\n"), Err(Error::Read(ReadError::NotText)));
        assert_eq!(p("title \"A\"\nbudget 1M\u{e9}\n"), Err(Error::Read(ReadError::NotText)));
    }

    #[test]
    fn random_damage_never_panics_or_widens() {
        crate::damage(NOTES.as_bytes(), 20_000, |b| {
            if let Ok(a) = App::parse(b) {
                assert!((MIN_BUDGET..=MAX_BUDGET).contains(&a.budget));
                assert!(a.title.len() <= MAX_TITLE && a.args.len() <= MAX_ARGS);
                assert!(a.places.len() <= Place::ALL.len());
            }
        });
    }

    /// The manifests the start-up archive carries for the self-test parse (or don't) as meant.
    #[test]
    fn the_shipped_manifests() {
        let ok = App::parse(include_bytes!("../../../tests/programs/sandboxed/sandboxed.app"));
        let ok = ok.unwrap();
        assert!(ok.trusted && ok.places.len() == 3 && ok.title == "Sandbox test");
        let broken = App::parse(include_bytes!("../../../tests/programs/sandboxed/broken.app"));
        assert_eq!(broken, Err(Error::BadValue));
    }
}
