// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The line reader every plain-text file of settings shares: driver manifests (note 0011,
//! section 5), app manifests, `users.txt` and rules files (note 0017).
//!
//! A file is one `key value…` per line. `#` starts a comment that runs to the end of the line.
//! Lines end with `\n` (a `\r` just before it is ignored, so files saved on Windows read the
//! same). Everything is printable ASCII or tabs, except, where a format allows it, words in
//! double quotes, which may hold any UTF-8 but not a `"` (there are no escapes: nothing these
//! files hold needs one). Anything else, and anything too long, refuses the whole file: these
//! files decide what programs may do, so a damaged one must never be half understood.

use alloc::vec::Vec;

/// How big a file and its lines may be, and whether it has quoted words.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The largest file, in bytes.
    pub max_size: usize,
    /// The longest line, in bytes, comments included.
    pub max_line: usize,
    /// Whether `"…"` is a quoted word (which may hold UTF-8 and `#`). Without it, `"` is an
    /// ordinary character and the whole file must be ASCII.
    pub quotes: bool,
}

/// What was wrong with a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Longer than [`Limits::max_size`].
    TooLong,
    /// A line longer than [`Limits::max_line`].
    LineTooLong,
    /// Not text: control characters, a lone `\r`, bytes that aren't UTF-8, or (outside quotes)
    /// anything that isn't ASCII.
    NotText,
    /// A quote that isn't closed on its line, or doesn't stand as a word on its own.
    BadQuote,
}

/// One line with something on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line<'a> {
    /// Its line number, from 1, for messages.
    pub number: usize,
    /// The first word.
    pub key: &'a str,
    /// The rest, without the comment, with the space around it trimmed.
    pub rest: &'a str,
    /// Whether the file's format has quoted words.
    quotes: bool,
}

/// One word of a line's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Word<'a> {
    /// The word, without its quotes.
    pub text: &'a str,
    /// Whether it was in quotes.
    pub quoted: bool,
}

fn space(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Where the comment starts in `line` (or its length), checking quotes on the way.
fn comment_start(line: &str, quotes: bool) -> Result<usize, ReadError> {
    let mut in_quote = false;
    for (i, b) in line.bytes().enumerate() {
        match b {
            b'"' if quotes => in_quote = !in_quote,
            b'#' if !in_quote => return Ok(i),
            _ if !in_quote && !b.is_ascii() => return Err(ReadError::NotText),
            _ => {}
        }
    }
    if in_quote { Err(ReadError::BadQuote) } else { Ok(line.len()) }
}

/// Splits `text` into its lines, checking it against `limits`. Lines with nothing but space or
/// a comment are left out.
///
/// The `'a` in the signature is a *lifetime*: it says the lines returned point into `text`
/// (they borrow it, rather than copying), so they can't outlive it.
pub fn read(text: &[u8], limits: Limits) -> Result<Vec<Line<'_>>, ReadError> {
    if text.len() > limits.max_size {
        return Err(ReadError::TooLong);
    }
    // Control characters are refused everywhere; UTF-8 is checked as a whole, and then where
    // it may appear, line by line.
    if text.iter().any(|&b| b < 0x20 && b != b'\n' && b != b'\t' && b != b'\r' || b == 0x7F) {
        return Err(ReadError::NotText);
    }
    if !limits.quotes && !text.is_ascii() {
        return Err(ReadError::NotText);
    }
    let text = core::str::from_utf8(text).map_err(|_| ReadError::NotText)?;
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.contains('\r') {
            return Err(ReadError::NotText);
        }
        if line.len() > limits.max_line {
            return Err(ReadError::LineTooLong);
        }
        let line =
            line[..comment_start(line, limits.quotes)?].trim_matches(|c| c == ' ' || c == '\t');
        if line.is_empty() {
            continue;
        }
        let (key, rest) = match line.find([' ', '\t']) {
            Some(at) => (&line[..at], line[at..].trim_matches(|c| c == ' ' || c == '\t')),
            None => (line, ""),
        };
        out.push(Line { number: i + 1, key, rest, quotes: limits.quotes });
    }
    Ok(out)
}

impl<'a> Line<'a> {
    /// The value's words: separated by spaces or tabs, with a quoted word (in a format with
    /// quotes) taken whole. A quote must start and end a word: `a"b"` and `"a"b` are refused.
    pub fn words(&self) -> Result<Vec<Word<'a>>, ReadError> {
        let s = self.rest;
        let b = s.as_bytes();
        let mut out = Vec::new();
        if !self.quotes {
            let words = s.split([' ', '\t']).filter(|w| !w.is_empty());
            return Ok(words.map(|text| Word { text, quoted: false }).collect());
        }
        let mut i = 0;
        while i < b.len() {
            if space(b[i]) {
                i += 1;
                continue;
            }
            if b[i] == b'"' {
                let close = s[i + 1..].find('"').ok_or(ReadError::BadQuote)? + i + 1;
                if close + 1 < b.len() && !space(b[close + 1]) {
                    return Err(ReadError::BadQuote);
                }
                out.push(Word { text: &s[i + 1..close], quoted: true });
                i = close + 1;
            } else {
                let end = s[i..].find([' ', '\t']).map_or(b.len(), |e| e + i);
                if s[i..end].contains('"') {
                    return Err(ReadError::BadQuote);
                }
                out.push(Word { text: &s[i..end], quoted: false });
                i = end;
            }
        }
        Ok(out)
    }

    /// The value's words, none of them quoted.
    pub fn plain_words(&self) -> Result<Vec<&'a str>, ReadError> {
        let w = self.words()?;
        if w.iter().any(|w| w.quoted) {
            return Err(ReadError::BadQuote);
        }
        Ok(w.into_iter().map(|w| w.text).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const Q: Limits = Limits { max_size: 4096, max_line: 200, quotes: true };
    const NOQ: Limits = Limits { max_size: 4096, max_line: 200, quotes: false };

    fn keys(text: &str, l: Limits) -> Vec<(usize, &str, &str)> {
        read(text.as_bytes(), l).unwrap().into_iter().map(|l| (l.number, l.key, l.rest)).collect()
    }

    #[test]
    fn reads_lines() {
        let t = "# a comment\n\nkey value  words # comment\n\tsolo\r\nlast \"in # quotes\" x";
        assert_eq!(
            keys(t, Q),
            vec![(3, "key", "value  words"), (4, "solo", ""), (5, "last", "\"in # quotes\" x")]
        );
        // Without quotes, `#` always starts a comment and `"` is just a character.
        assert_eq!(keys("args \"a#b\"", NOQ), vec![(1, "args", "\"a")]);
        assert_eq!(keys("", Q), vec![]);
    }

    #[test]
    fn splits_words() {
        let lines =
            read("title \"Chris's  Notes\"  x\ty\nuser a \"\u{e9}l\u{e8}ve\"".as_bytes(), Q)
                .unwrap();
        assert_eq!(
            lines[0].words().unwrap(),
            vec![
                Word { text: "Chris's  Notes", quoted: true },
                Word { text: "x", quoted: false },
                Word { text: "y", quoted: false },
            ]
        );
        assert_eq!(lines[1].words().unwrap()[1].text, "\u{e9}l\u{e8}ve");
        assert_eq!(lines[1].plain_words(), Err(ReadError::BadQuote));
        let one = read(b"k \"\"", Q).unwrap();
        assert_eq!(one[0].words().unwrap(), vec![Word { text: "", quoted: true }]);
    }

    #[test]
    fn refuses_the_wrong() {
        let r = |s: &[u8], l| read(s, l).map(|_| ());
        assert_eq!(r(&[b'#'; 4097], Q), Err(ReadError::TooLong));
        assert_eq!(r(format!("k {}", "x".repeat(199)).as_bytes(), Q), Err(ReadError::LineTooLong));
        assert_eq!(r(b"k \x00", Q), Err(ReadError::NotText));
        assert_eq!(r(b"k \x7f", Q), Err(ReadError::NotText));
        assert_eq!(r(b"k\rv", Q), Err(ReadError::NotText));
        assert_eq!(r(b"k \xff", Q), Err(ReadError::NotText));
        assert_eq!(r("k \u{e9}".as_bytes(), Q), Err(ReadError::NotText), "UTF-8 outside quotes");
        assert_eq!(r("k \"\u{e9}\"".as_bytes(), NOQ), Err(ReadError::NotText));
        assert_eq!(r("# \u{e9}".as_bytes(), NOQ), Err(ReadError::NotText));
        assert_eq!(r(b"k \"open", Q), Err(ReadError::BadQuote));
        assert_eq!(r(b"k \"a\nb\"", Q), Err(ReadError::BadQuote));
        let w = |s: &str| read(s.as_bytes(), Q).unwrap()[0].words().map(|_| ());
        assert_eq!(w("k a\"b\""), Err(ReadError::BadQuote));
        assert_eq!(w("k \"a\"b"), Err(ReadError::BadQuote));
        assert_eq!(w("k \"a\" \"b\""), Ok(()));
    }
}
