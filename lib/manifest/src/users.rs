// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The users (note 0017, section 3): `System/users.txt` on the system volume.
//!
//! ```text
//! # PrettyBigRock users
//! user chris admin "Chris"
//! user alice standard "Alice"
//! ```
//!
//! The users service reads it at start and writes it again when an administrator adds a user,
//! with [`format`], which [`parse`] reads back exactly.

use crate::is_name;
use crate::lines::{self, Limits, ReadError};
use alloc::string::String;
use alloc::vec::Vec;

/// The largest file.
pub const MAX_SIZE: usize = 16384;
/// The longest line.
pub const MAX_LINE: usize = 200;
/// The most users.
pub const MAX_USERS: usize = 64;
/// The longest display name, in bytes.
pub const MAX_DISPLAY: usize = 128;

const LIMITS: Limits = Limits { max_size: MAX_SIZE, max_line: MAX_LINE, quotes: true };

/// What kind of user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An administrator: also holds every volume, and can add users.
    Admin,
    /// Everyone else.
    Standard,
}

/// One user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// Their name, which names their home folder ([`is_name`]).
    pub name: String,
    /// Administrator or not.
    pub kind: Kind,
    /// The name people see: UTF-8, without `"`.
    pub display: String,
}

impl User {
    /// Whether this user could be written to `users.txt`: a good name, and a display name
    /// that is 1 to [`MAX_DISPLAY`] bytes of printable text without `"`.
    pub fn is_valid(&self) -> bool {
        is_name(&self.name) && good_display(&self.display)
    }
}

fn good_display(d: &str) -> bool {
    (1..=MAX_DISPLAY).contains(&d.len())
        && !d.trim().is_empty()
        && !d.chars().any(|c| c == '"' || c.is_control())
}

/// What was wrong with the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file itself: too long, not text, a broken quote.
    Read(ReadError),
    /// A line that isn't a `user` line.
    UnknownKey,
    /// A `user` line written wrongly.
    BadValue,
    /// Two users with one name.
    Repeated,
    /// More than [`MAX_USERS`].
    TooMany,
}

impl From<ReadError> for Error {
    fn from(e: ReadError) -> Error {
        Error::Read(e)
    }
}

/// Reads `users.txt`. On an error, also returns the line it was on (0 for the whole file).
pub fn parse(text: &[u8]) -> Result<Vec<User>, (Error, usize)> {
    let mut users: Vec<User> = Vec::new();
    for line in lines::read(text, LIMITS).map_err(|e| (Error::Read(e), 0))? {
        let at = |e| (e, line.number);
        if line.key != "user" {
            return Err(at(Error::UnknownKey));
        }
        let w = line.words().map_err(|e| at(Error::Read(e)))?;
        let [name, kind, display] = w.as_slice() else { return Err(at(Error::BadValue)) };
        let kind = match (kind.text, kind.quoted) {
            ("admin", false) => Kind::Admin,
            ("standard", false) => Kind::Standard,
            _ => return Err(at(Error::BadValue)),
        };
        let user = User { name: name.text.into(), kind, display: display.text.into() };
        if name.quoted || !display.quoted || !user.is_valid() {
            return Err(at(Error::BadValue));
        }
        if users.iter().any(|u| u.name == user.name) {
            return Err(at(Error::Repeated));
        }
        if users.len() == MAX_USERS {
            return Err(at(Error::TooMany));
        }
        users.push(user);
    }
    Ok(users)
}

/// Writes `users` as `users.txt`. Every user must be [`User::is_valid`] (`None` otherwise).
pub fn format(users: &[User]) -> Option<String> {
    let mut out =
        String::from("# PrettyBigRock users: user NAME admin|standard \"DISPLAY NAME\"\n");
    for u in users {
        if !u.is_valid() {
            return None;
        }
        let kind = match u.kind {
            Kind::Admin => "admin",
            Kind::Standard => "standard",
        };
        out.push_str("user ");
        out.push_str(&u.name);
        out.push(' ');
        out.push_str(kind);
        out.push_str(" \"");
        out.push_str(&u.display);
        out.push_str("\"\n");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    const USERS: &str = "# PrettyBigRock users\nuser chris admin \"Chris\"\nuser alice standard \"Alice M\u{fc}ller # 2\"\n";

    fn u(name: &str, kind: Kind, display: &str) -> User {
        User { name: name.into(), kind, display: display.into() }
    }

    #[test]
    fn reads_and_writes() {
        let users = parse(USERS.as_bytes()).unwrap();
        assert_eq!(
            users,
            vec![
                u("chris", Kind::Admin, "Chris"),
                u("alice", Kind::Standard, "Alice M\u{fc}ller # 2")
            ]
        );
        assert_eq!(parse(format(&users).unwrap().as_bytes()).unwrap(), users);
        assert_eq!(parse(b"").unwrap(), vec![]);
        assert_eq!(format(&[u("x", Kind::Admin, "A \"B\"")]), None);
        assert_eq!(format(&[u("X", Kind::Admin, "A")]), None);
    }

    #[test]
    fn refuses_the_wrong() {
        let p = |s: &str| parse(s.as_bytes()).map(|_| ());
        assert_eq!(p("person chris admin \"C\""), Err((Error::UnknownKey, 1)));
        assert_eq!(p("user chris admin"), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris admin C"), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris root \"C\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris \"admin\" \"C\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user \"chris\" admin \"C\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user Chris admin \"C\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user ../x admin \"C\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris admin \"\""), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris admin \"  \""), Err((Error::BadValue, 1)));
        assert_eq!(p("user chris admin \"C\" extra"), Err((Error::BadValue, 1)));
        let long = format!("user chris admin \"{}\"", "x".repeat(129));
        assert_eq!(p(&long), Err((Error::BadValue, 1)));
        assert_eq!(p("user a admin \"A\"\nuser a standard \"B\""), Err((Error::Repeated, 2)));
        let many: String = (0..65).map(|i| format!("user u{i} standard \"U\"\n")).collect();
        assert_eq!(p(&many), Err((Error::TooMany, 65)));
        assert_eq!(p("user a admin \"A"), Err((Error::Read(ReadError::BadQuote), 0)));
    }

    #[test]
    fn random_damage_never_panics() {
        crate::damage(USERS.as_bytes(), 20_000, |b| {
            if let Ok(users) = parse(b) {
                assert!(users.iter().all(User::is_valid));
            }
        });
    }
}
