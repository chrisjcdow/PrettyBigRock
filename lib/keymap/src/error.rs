// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What can be wrong with a layout file, in words a person can act on.

use core::fmt;

/// What was wrong with one part of a layout file.
///
/// An `enum` is Rust's "one of these" type: a `Problem` is exactly one of the cases below, and a
/// `match` on it must deal with every case. A layout is refused whole for any of them, so nobody
/// ever types with half-understood keys (note 0021, section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The file is longer than [`crate::MAX_SIZE`].
    FileTooLong,
    /// A line is longer than [`crate::MAX_LINE`].
    LineTooLong,
    /// A control character (other than a tab) or a lone carriage return.
    ControlCharacter,
    /// Bytes that are not UTF-8 text.
    NotUtf8,
    /// A line that starts with none of `title`, `altgr`, `key` or `dead`.
    UnknownWord,
    /// A `title` line that is not one pair of double quotes round 1 to 64 bytes of text.
    BadTitle,
    /// A second `title` line.
    TitleTwice,
    /// No `title` line at all.
    TitleMissing,
    /// A second `altgr` line.
    AltgrTwice,
    /// An `altgr` line with words after it.
    AltgrWithWords,
    /// A `key` line with no key name.
    NoKeyName,
    /// A key name that is not in the table of keys.
    UnknownKey,
    /// A key that is not written in layout files (Enter, Shift, the arrows, the function keys).
    ControlKey,
    /// A second `key` line for one key.
    KeyTwice,
    /// A `key` line with no values, or with more than four.
    WrongValueCount,
    /// A value that is not one character, `space`, `none` or `dead:NAME`.
    BadValue,
    /// A `key` line that types nothing at any level.
    KeyTypesNothing,
    /// A value for the AltGr levels in a file with no `altgr` line.
    AltgrWithoutLine,
    /// A `dead` line whose name is not 1 to 16 lower-case letters.
    BadDeadName,
    /// A second `dead` line for one name.
    DeadTwice,
    /// More than 16 `dead` lines.
    TooManyDead,
    /// A `dead` line whose words after the name do not come in pairs.
    OddDeadWords,
    /// A `dead` line with more than 64 pairs.
    TooManyPairs,
    /// A base or result in a `dead` line that is not a single character (or, for a base, `space`).
    BadDeadCharacter,
    /// A base given twice in one `dead` line.
    DeadBaseTwice,
    /// A `dead` line without its `space` pair, which says what the accent looks like.
    DeadNeedsSpace,
    /// A `dead:NAME` value with no `dead NAME` line.
    UnknownDead,
    /// A `dead` line that no key uses.
    UnusedDead,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Problem::FileTooLong => "the file is too long (16 KiB at most)",
            Problem::LineTooLong => "the line is too long (400 bytes at most)",
            Problem::ControlCharacter => "the line has a control character in it",
            Problem::NotUtf8 => "the text is not valid UTF-8",
            Problem::UnknownWord => {
                "the line should start with title, altgr, key or dead, and does not"
            }
            Problem::BadTitle => {
                "the title must be 1 to 64 bytes of text between double quotes, with no \
                 double quote inside"
            }
            Problem::TitleTwice => "there is more than one title",
            Problem::TitleMissing => "there is no title line",
            Problem::AltgrTwice => "there is more than one altgr line",
            Problem::AltgrWithWords => "the altgr line must have nothing after it",
            Problem::NoKeyName => "the key line does not say which key",
            Problem::UnknownKey => "there is no key with that name",
            Problem::ControlKey => {
                "that key is the same on every layout, so it is not written in a layout file"
            }
            Problem::KeyTwice => "this key was already given",
            Problem::WrongValueCount => "a key needs from one to four values",
            Problem::BadValue => {
                "a value must be one character, or space, or none, or dead: and a name"
            }
            Problem::KeyTypesNothing => "this key types nothing at any level",
            Problem::AltgrWithoutLine => {
                "this key has an AltGr value, but the file has no altgr line"
            }
            Problem::BadDeadName => "a dead key's name must be 1 to 16 lower-case letters",
            Problem::DeadTwice => "this dead key was already given",
            Problem::TooManyDead => "there are too many dead keys (16 at most)",
            Problem::OddDeadWords => "a dead key needs pairs: a letter, then what it becomes",
            Problem::TooManyPairs => "this dead key has too many pairs (64 at most)",
            Problem::BadDeadCharacter => {
                "each letter and result of a dead key must be one character (a letter may also \
                 be space)"
            }
            Problem::DeadBaseTwice => "this dead key gives one letter twice",
            Problem::DeadNeedsSpace => {
                "a dead key needs a pair that starts with space, giving the accent itself"
            }
            Problem::UnknownDead => "this key uses a dead key that the file does not define",
            Problem::UnusedDead => "no key uses this dead key",
        };
        f.write_str(text)
    }
}

/// A layout file that was refused: which line, and what was wrong with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The line the problem is on, counting from 1; 0 for a problem with no one line (a missing
    /// title, a file that is too long).
    pub line: usize,
    /// What was wrong.
    pub problem: Problem,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.problem)
        } else {
            write!(f, "line {}: {}", self.line, self.problem)
        }
    }
}

// A trait is a set of abilities a type can promise; `Error` is the promise "I describe a
// failure". The `Display` above is what makes this one keepable.
impl core::error::Error for ParseError {}
