// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A user's rules (note 0017, section 5): `System/rules/<user>.txt` on the system volume, in
//! the same style as manifests.
//!
//! ```text
//! deny paint pictures
//! allow notes documents read
//! trust backup
//! ```
//!
//! Rules that contradict each other (`allow` and `deny` of one place for one app, `trust` and
//! `distrust` of one app) or say the same thing twice refuse the whole file, as anything else
//! wrong does. The sandbox manager then launches nothing for that user until the file is fixed:
//! a rule it can't read might have been a `deny`, so guessing could give an app what the user
//! meant to keep from it.

use crate::lines::{self, Limits, ReadError};
use crate::{Place, Rights, is_name};
use alloc::string::String;
use alloc::vec::Vec;

/// The largest rules file.
pub const MAX_SIZE: usize = 16384;
/// The longest line.
pub const MAX_LINE: usize = 200;
/// The most rules.
pub const MAX_RULES: usize = 512;

const LIMITS: Limits = Limits { max_size: MAX_SIZE, max_line: MAX_LINE, quotes: false };

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `allow APP PLACE read [write]`: the app may have the place, with at most these rights.
    Allow(String, Place, Rights),
    /// `deny APP PLACE`: it may never have it.
    Deny(String, Place),
    /// `trust APP`: it may be trusted, if its manifest asks.
    Trust(String),
    /// `distrust APP`: it may not.
    Distrust(String),
}

impl Rule {
    /// The app it is about.
    fn app(&self) -> &str {
        match self {
            Rule::Allow(a, ..) | Rule::Deny(a, _) | Rule::Trust(a) | Rule::Distrust(a) => a,
        }
    }

    /// The place it is about (`None` for trust), so two rules that collide can be found.
    fn place(&self) -> Option<Place> {
        match self {
            Rule::Allow(_, p, _) | Rule::Deny(_, p) => Some(*p),
            Rule::Trust(_) | Rule::Distrust(_) => None,
        }
    }
}

/// What was wrong with a rules file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file itself: too long, not text.
    Read(ReadError),
    /// A line that isn't one of the four rules.
    UnknownRule,
    /// A rule written wrongly: a bad app name, place or rights.
    BadValue,
    /// Two rules about the same app and place (or the same app's trust).
    Conflict,
    /// More than [`MAX_RULES`].
    TooMany,
}

impl From<ReadError> for Error {
    fn from(e: ReadError) -> Error {
        Error::Read(e)
    }
}

/// A user's rules, checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules(Vec<Rule>);

impl Rules {
    /// No rules: what a user has without a rules file.
    pub fn none() -> Rules {
        Rules(Vec::new())
    }

    /// Parses a rules file. On an error, also returns the line it was on.
    pub fn parse(text: &[u8]) -> Result<Rules, Error> {
        Rules::parse_at(text).map_err(|(e, _)| e)
    }

    /// [`Rules::parse`], with the number of the line that was wrong (0 for the whole file).
    pub fn parse_at(text: &[u8]) -> Result<Rules, (Error, usize)> {
        let mut rules: Vec<Rule> = Vec::new();
        for line in lines::read(text, LIMITS).map_err(|e| (Error::Read(e), 0))? {
            let at = |e| (e, line.number);
            let w = line.plain_words().map_err(|e| at(Error::Read(e)))?;
            let app =
                |s: &str| if is_name(s) { Ok(String::from(s)) } else { Err(at(Error::BadValue)) };
            let place = |s: &str| Place::from_word(s).ok_or(at(Error::BadValue));
            let rule = match (line.key, w.as_slice()) {
                ("allow", [a, p, rights @ ..]) => Rule::Allow(
                    app(a)?,
                    place(p)?,
                    Rights::parse(rights).ok_or(at(Error::BadValue))?,
                ),
                ("deny", [a, p]) => Rule::Deny(app(a)?, place(p)?),
                ("trust", [a]) => Rule::Trust(app(a)?),
                ("distrust", [a]) => Rule::Distrust(app(a)?),
                ("allow" | "deny" | "trust" | "distrust", _) => return Err(at(Error::BadValue)),
                _ => return Err(at(Error::UnknownRule)),
            };
            if rules.iter().any(|r| r.app() == rule.app() && r.place() == rule.place()) {
                return Err(at(Error::Conflict));
            }
            if rules.len() == MAX_RULES {
                return Err(at(Error::TooMany));
            }
            rules.push(rule);
        }
        Ok(Rules(rules))
    }

    /// The rules, in the file's order.
    pub fn rules(&self) -> &[Rule] {
        &self.0
    }

    /// The rights an `allow` rule gives `app` for `place`, if there is one.
    pub fn allows(&self, app: &str, place: Place) -> Option<Rights> {
        self.0.iter().find_map(|r| match r {
            Rule::Allow(a, p, rights) if a == app && *p == place => Some(*rights),
            _ => None,
        })
    }

    /// Whether a rule denies `app` the `place`.
    pub fn denies(&self, app: &str, place: Place) -> bool {
        self.0.iter().any(|r| matches!(r, Rule::Deny(a, p) if a == app && *p == place))
    }

    /// Whether a rule trusts `app`.
    pub fn trusts(&self, app: &str) -> bool {
        self.0.iter().any(|r| matches!(r, Rule::Trust(a) if a == app))
    }

    /// Whether a rule distrusts `app`.
    pub fn distrusts(&self, app: &str) -> bool {
        self.0.iter().any(|r| matches!(r, Rule::Distrust(a) if a == app))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    const RULES: &str = "# Chris's rules\ndeny paint pictures\nallow notes documents read\nallow notes music read write # for the playlist\ntrust backup\ndistrust shell\n";

    #[test]
    fn reads_rules() {
        let r = Rules::parse(RULES.as_bytes()).unwrap();
        assert_eq!(r.rules().len(), 5);
        assert!(r.denies("paint", Place::Pictures) && !r.denies("paint", Place::Music));
        assert!(!r.denies("notes", Place::Pictures));
        assert_eq!(r.allows("notes", Place::Documents), Some(Rights::Read));
        assert_eq!(r.allows("notes", Place::Music), Some(Rights::ReadWrite));
        assert_eq!(r.allows("paint", Place::Documents), None);
        assert!(r.trusts("backup") && !r.trusts("shell") && r.distrusts("shell"));
        assert_eq!(Rules::parse(b""), Ok(Rules::none()));
    }

    #[test]
    fn refuses_the_wrong() {
        let p = |s: &str| Rules::parse_at(s.as_bytes()).map(|_| ());
        assert_eq!(p("permit notes documents read"), Err((Error::UnknownRule, 1)));
        assert_eq!(p("\nallow notes documents"), Err((Error::BadValue, 2)));
        assert_eq!(p("allow notes documents write"), Err((Error::BadValue, 1)));
        assert_eq!(p("allow Notes documents read"), Err((Error::BadValue, 1)));
        assert_eq!(p("allow notes home read"), Err((Error::BadValue, 1)));
        assert_eq!(p("deny notes"), Err((Error::BadValue, 1)));
        assert_eq!(p("deny notes pictures read"), Err((Error::BadValue, 1)));
        assert_eq!(p("trust"), Err((Error::BadValue, 1)));
        assert_eq!(p("trust a b"), Err((Error::BadValue, 1)));
        assert_eq!(p("trust \"a\""), Err((Error::BadValue, 1)), "quotes mean nothing in rules");
        assert_eq!(p("deny a music\nallow a music read"), Err((Error::Conflict, 2)));
        assert_eq!(p("allow a music read\nallow a music read write"), Err((Error::Conflict, 2)));
        assert_eq!(p("deny a music\ndeny a music"), Err((Error::Conflict, 2)));
        assert_eq!(p("trust a\ndistrust a"), Err((Error::Conflict, 2)));
        assert_eq!(p("trust a\ntrust a"), Err((Error::Conflict, 2)));
        assert!(p("trust a\ndeny a music\nallow a pictures read\ntrust b").is_ok());
        let many: String = (0..513).map(|i| format!("trust a{i}\n")).collect();
        assert_eq!(p(&many), Err((Error::TooMany, 513)));
        assert_eq!(p("deny a music \u{e9}"), Err((Error::Read(ReadError::NotText), 0)));
    }

    #[test]
    fn random_damage_never_panics() {
        crate::damage(RULES.as_bytes(), 20_000, |b| {
            if let Ok(r) = Rules::parse(b) {
                assert!(r.rules().len() <= 5 + 2);
            }
        });
    }
}
