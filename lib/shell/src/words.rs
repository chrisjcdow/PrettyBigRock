// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Splitting a command line into words.
//!
//! The rules are the ones 1.x's Terminal had, plus escapes:
//! - words are separated by spaces (or tabs);
//! - double quotes group: `write a.txt "two words"` has three words after `write`;
//! - inside quotes, `\"` is a quote and `\\` is a backslash; any other backslash is just a
//!   backslash. Outside quotes a backslash is an ordinary character, so Windows-style names
//!   don't need doubling;
//! - `""` is a word with nothing in it, and quotes can sit in the middle of a word: `ab"c d"e`
//!   is the one word `abc de`.
//!
//! Splitting never fails badly: an unclosed quote or a line that is too long is an error value
//! for the shell to print, never a panic. (In Rust, a `Result` is either `Ok(value)` or
//! `Err(error)`; the caller has to look at which.)

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// The longest line the terminal gives us, in bytes (`pbr.terminal`'s `read_line`).
pub const MAX_LINE: usize = 4096;

/// What was wrong with a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordsError {
    /// A `"` was opened and the line ended before it was closed.
    UnclosedQuote,
    /// More than [`MAX_LINE`] bytes.
    TooLong,
}

impl fmt::Display for WordsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WordsError::UnclosedQuote => f.write_str("a quote was opened but never closed"),
            WordsError::TooLong => write!(f, "that line is too long ({MAX_LINE} bytes at most)"),
        }
    }
}

/// Splits `line` into words (see the module's rules). An empty or all-space line gives no words.
pub fn split(line: &str) -> Result<Vec<String>, WordsError> {
    if line.len() > MAX_LINE {
        return Err(WordsError::TooLong);
    }
    let mut words = Vec::new();
    let mut word = String::new();
    // Whether a word has started. `""` starts a word without putting a character in it, so
    // `word.is_empty()` alone can't tell "no word yet" from "an empty word".
    let mut started = false;
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' => quoted = false,
                '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                    word.extend(chars.next());
                }
                _ => word.push(c),
            }
        } else {
            match c {
                '"' => {
                    quoted = true;
                    started = true;
                }
                ' ' | '\t' => {
                    if started {
                        words.push(core::mem::take(&mut word));
                        started = false;
                    }
                }
                _ => {
                    word.push(c);
                    started = true;
                }
            }
        }
    }
    if quoted {
        return Err(WordsError::UnclosedQuote);
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn words(line: &str) -> Vec<String> {
        split(line).unwrap()
    }

    #[test]
    fn plain_words() {
        assert_eq!(words("ls -a /Home"), vec!["ls", "-a", "/Home"]);
        assert_eq!(words("  a   b\tc  "), vec!["a", "b", "c"]);
        assert!(words("").is_empty());
        assert!(words("     \t ").is_empty());
    }

    #[test]
    fn quotes_group_words() {
        assert_eq!(words(r#"write a.txt "two words""#), vec!["write", "a.txt", "two words"]);
        assert_eq!(words(r#""a  b" c"#), vec!["a  b", "c"]);
        assert_eq!(words(r#"ab"c d"e f"#), vec!["abc de", "f"]);
    }

    #[test]
    fn empty_words_are_kept() {
        assert_eq!(words(r#"write a """#), vec!["write", "a", ""]);
        assert_eq!(words(r#""" """#), vec!["", ""]);
        assert_eq!(words(r#"a "" b"#), vec!["a", "", "b"]);
    }

    #[test]
    fn escapes_only_inside_quotes() {
        assert_eq!(words(r#""say \"hi\"""#), vec![r#"say "hi""#]);
        assert_eq!(words(r#""a\\b""#), vec![r"a\b"]);
        // Any other backslash stays as it is.
        assert_eq!(words(r#""a\nb""#), vec![r"a\nb"]);
        assert_eq!(words(r"C:\Users\x"), vec![r"C:\Users\x"]);
        // An escaped backslash before the closing quote doesn't swallow the quote.
        assert_eq!(words(r#""a\\" b"#), vec![r"a\", "b"]);
    }

    #[test]
    fn unclosed_quotes_are_errors() {
        assert_eq!(split(r#"echo "hi"#), Err(WordsError::UnclosedQuote));
        assert_eq!(split(r#"""#), Err(WordsError::UnclosedQuote));
        // The backslash escapes the last quote, so it never closes.
        assert_eq!(split(r#""\""#), Err(WordsError::UnclosedQuote));
        assert_eq!(split(r#"a "b c\"#), Err(WordsError::UnclosedQuote));
    }

    #[test]
    fn long_lines() {
        let ok = "a".repeat(MAX_LINE);
        assert_eq!(split(&ok).unwrap(), vec![ok.clone()]);
        let too_long = "a".repeat(MAX_LINE + 1);
        assert_eq!(split(&too_long), Err(WordsError::TooLong));
        // Many words fill a line without trouble.
        let many = "a ".repeat(MAX_LINE / 2);
        assert_eq!(words(&many).len(), MAX_LINE / 2);
        // And the length is in bytes, not characters.
        let wide = "é".repeat(MAX_LINE / 2 + 1);
        assert_eq!(split(&wide), Err(WordsError::TooLong));
    }

    #[test]
    fn other_characters_pass_through() {
        assert_eq!(words("héllo wörld ☃"), vec!["héllo", "wörld", "☃"]);
        assert_eq!(words("a\u{0}b"), vec!["a\u{0}b"]);
    }
}
