// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests for the font: the table against the drawings in `glyphs.txt`, the parser and
//! generator that connect the two, the set of characters, and the style rules (an accented
//! letter keeps its base letter's dots, no two characters look the same by accident).
//!
//! Rust notes for a newcomer: a function marked `#[test]` is run by `cargo test`, and fails if
//! it panics (which `assert!`, `assert_eq!` and `unwrap()` do when their check fails). `&str`
//! is a borrowed piece of text and `String` an owned, growable one. `Result<T, String>` is
//! either `Ok(value)` or `Err(message)`.

use super::*;
use crate::table::GLYPHS;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::process::Command;

// ---- Reading glyphs.txt and writing table.rs ----

/// The drawings, as text.
const DRAWINGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/glyphs.txt");
/// The generated table of numbers.
const TABLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/table.rs");

/// Characters that the drawings (and the table's comments) write as words, because the
/// character itself is blank or invisible.
const NAMED: [(u32, &str); 3] = [(0x20, "space"), (0xA0, "no-break space"), (0xAD, "soft hyphen")];

/// What the header line of an entry says after the code point: the character itself, or its
/// name for the characters in [`NAMED`].
fn label(code: u32) -> String {
    match NAMED.iter().find(|&&(c, _)| c == code) {
        Some(&(_, name)) => name.to_string(),
        None => char::from_u32(code).map(String::from).unwrap_or_default(),
    }
}

/// Reads the header line of an entry (`U+00E9 é`) and returns its code point.
fn parse_header(line: &str) -> Result<u32, String> {
    let rest = line.strip_prefix("U+").ok_or("the header must start with U+")?;
    let (hex, name) = rest
        .split_once(' ')
        .ok_or("the header must be U+, four hex digits, a space and the character")?;
    if hex.len() != 4 || !hex.chars().all(|c| matches!(c, '0'..='9' | 'A'..='F')) {
        return Err(format!("{hex:?} is not four upper-case hex digits"));
    }
    let code = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
    if let Some(&(named, word)) = NAMED.iter().find(|&&(_, word)| word == name) {
        if named != code {
            return Err(format!("U+{code:04X} is not the {word} (that is U+{named:04X})"));
        }
        return Ok(code);
    }
    let mut chars = name.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(format!("{name:?} is not one character (or one of the names for blank ones)"));
    };
    if NAMED.iter().any(|&(named, _)| named == u32::from(c)) {
        return Err(format!("U+{:04X} must be written as its name, not itself", u32::from(c)));
    }
    if u32::from(c) != code {
        return Err(format!(
            "the code point U+{code:04X} doesn't match the character {c:?} (U+{:04X})",
            u32::from(c)
        ));
    }
    Ok(code)
}

/// Reads the whole of `glyphs.txt`. Every complaint names the line (counting from 1).
fn parse(text: &str) -> Result<Vec<(u32, Glyph)>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    while at < lines.len() && lines[at].starts_with(';') {
        at += 1;
    }
    let mut entries: Vec<(u32, Glyph)> = Vec::new();
    loop {
        while at < lines.len() && lines[at].is_empty() {
            at += 1;
        }
        if at >= lines.len() {
            return Ok(entries);
        }
        let code = parse_header(lines[at]).map_err(|e| format!("line {}: {e}", at + 1))?;
        // A `match` arm can carry an `if` condition (a "guard"): it only matches when it holds.
        match entries.last() {
            Some(&(last, _)) if code <= last => {
                let what = if code == last { "repeated" } else { "out of order" };
                return Err(format!(
                    "line {}: U+{code:04X} is {what} (after U+{last:04X})",
                    at + 1
                ));
            }
            _ => {}
        }
        at += 1;
        let mut glyph = [0u8; HEIGHT];
        for (row, byte) in glyph.iter_mut().enumerate() {
            let number = at + 1;
            let line = lines.get(at).copied().unwrap_or("");
            if line.is_empty() || line.starts_with("U+") || line.starts_with(';') {
                return Err(format!(
                    "line {number}: U+{code:04X} has {row} rows, but needs {HEIGHT}"
                ));
            }
            if line.chars().count() != WIDTH {
                return Err(format!(
                    "line {number}: a row must be {WIDTH} characters, not {line:?}"
                ));
            }
            for ch in line.chars() {
                *byte <<= 1;
                match ch {
                    '#' => *byte |= 1,
                    '.' => {}
                    _ => return Err(format!("line {number}: {ch:?} is not # or .")),
                }
            }
            at += 1;
        }
        if at < lines.len() && !lines[at].is_empty() {
            return Err(format!(
                "line {}: U+{code:04X} has more than {HEIGHT} rows (or no blank line before the \
                 next entry)",
                at + 1
            ));
        }
        entries.push((code, glyph));
    }
}

/// The text of `src/table.rs` for these drawings.
fn generate(entries: &[(u32, Glyph)]) -> String {
    let mut out = String::new();
    out.push_str(
        "// SPDX-License-Identifier: MPL-2.0\n\
         // Copyright (C) 2026 Christopher Cameron-Dow\n\
         \n\
         // GENERATED from glyphs.txt by the test `the_table_matches_the_drawings` (src/tests.rs).\n\
         // DO NOT EDIT BY HAND: change glyphs.txt, then run\n\
         //     PBR_UPDATE_FONT=1 cargo test -p pbr-font\n\
         // to write this file again.\n\
         \n\
         use crate::HEIGHT;\n\
         \n\
         /// Every glyph, sorted by code point: the code point, then the rows (top first), bit 4\n\
         /// of each row byte being the leftmost dot. The first 95 entries are U+0020 to U+007E.\n\
         #[rustfmt::skip]\n",
    );
    out.push_str(&format!(
        "pub(crate) static GLYPHS: [(u32, [u8; HEIGHT]); {}] = [\n",
        entries.len()
    ));
    for (code, glyph) in entries {
        let rows: Vec<String> = glyph.iter().map(|row| format!("0x{row:02x}")).collect();
        out.push_str(&format!("    (0x{code:04x}, [{}]), // {}\n", rows.join(", "), label(*code)));
    }
    out.push_str("];\n");
    out
}

fn load_drawings() -> Vec<(u32, Glyph)> {
    let text = std::fs::read_to_string(DRAWINGS).expect("glyphs.txt can be read");
    parse(&text).unwrap_or_else(|e| panic!("glyphs.txt: {e}"))
}

#[test]
fn the_table_matches_the_drawings() {
    let generated = generate(&load_drawings());
    if std::env::var("PBR_UPDATE_FONT").as_deref() == Ok("1") {
        std::fs::write(TABLE, &generated).expect("src/table.rs can be written");
        return;
    }
    let on_disk = std::fs::read_to_string(TABLE).expect("src/table.rs can be read");
    assert!(
        on_disk == generated,
        "src/table.rs doesn't match glyphs.txt: run `PBR_UPDATE_FONT=1 cargo test -p pbr-font` \
         to write it again, then run the tests again"
    );
}

#[test]
fn the_generated_table_is_what_the_library_uses() {
    // Parsing the drawings must give exactly the compiled-in table, not only the same text.
    let drawn = load_drawings();
    assert_eq!(drawn.len(), GLYPHS.len());
    for (a, b) in drawn.iter().zip(GLYPHS.iter()) {
        assert_eq!(a, b);
    }
}

// ---- The parser's refusals ----

/// A well-formed entry for the given header, with a dot in the top-left of every row.
fn entry(header: &str) -> String {
    format!("{header}\n{}", "#....\n".repeat(HEIGHT))
}

/// Parses `text` and checks that it fails, with a message containing `line` and `what`.
fn refused(text: &str, line: usize, what: &str) {
    let message = parse(text).expect_err("this text should be refused");
    assert!(message.starts_with(&format!("line {line}: ")), "{message}");
    assert!(message.contains(what), "{message} (looking for {what:?})");
}

#[test]
fn the_parser_accepts_a_good_file() {
    let text = format!(
        "; a comment\n; another\n\n{}\n{}",
        entry("U+0041 A"),
        entry("U+00A0 no-break space")
    );
    let entries = parse(&text).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], (0x41, [0x10; HEIGHT]));
    assert_eq!(entries[1].0, 0xA0);
}

#[test]
fn the_parser_reads_dots_left_to_right() {
    let mut text = String::from("U+0041 A\n");
    for row in 0..HEIGHT {
        text.push_str(if row == 0 {
            "#...#\n"
        } else if row == 1 {
            ".###.\n"
        } else {
            ".....\n"
        });
    }
    let entries = parse(&text).unwrap();
    assert_eq!(entries[0].1[0], 0b10001);
    assert_eq!(entries[0].1[1], 0b01110);
    assert_eq!(entries[0].1[2], 0);
}

#[test]
fn the_parser_refuses_a_wrong_header() {
    refused("U+41 A\n", 1, "four upper-case hex digits");
    refused("U+004a J\n", 1, "four upper-case hex digits");
    refused("U+00ZZ J\n", 1, "four upper-case hex digits");
    refused("0041 A\n", 1, "must start with U+");
    refused("U+0041\n", 1, "must be U+");
    refused("U+0041 AB\n", 1, "not one character");
    refused("U+0041 \n", 1, "not one character");
    refused("; comment\n\nU+0041 A\n#....\n; stray comment\n", 5, "has 1 rows");
    // Blank characters must be written as their names.
    refused("U+0020  \n", 1, "must be written as its name");
    refused("U+0020 \u{a0}\n", 1, "must be written as its name");
    refused("U+00AD \u{ad}\n", 1, "must be written as its name");
    refused("U+0020 no-break space\n", 1, "is not the no-break space");
}

#[test]
fn the_parser_refuses_a_code_point_that_does_not_match_the_character() {
    refused("U+0042 A\n", 1, "doesn't match the character 'A'");
    refused(&format!("{}\nU+00E9 e\n", entry("U+0041 A")), 14, "doesn't match");
}

#[test]
fn the_parser_refuses_rows_of_the_wrong_length() {
    let mut text = entry("U+0041 A");
    text = text.replacen("#....", "#...", 1);
    refused(&text, 2, "must be 5 characters");
    let text = entry("U+0041 A").replacen("#....", "#.....", 1);
    refused(&text, 2, "must be 5 characters");
    // Counted in characters, not bytes: a 5-character row of wide characters is still refused,
    // but for its characters.
    let text = entry("U+0041 A").replacen("#....", "ééééé", 1);
    refused(&text, 2, "is not # or .");
}

#[test]
fn the_parser_refuses_the_wrong_number_of_rows() {
    // Too few: the next entry starts early, a blank line comes early, or the file ends.
    let mut few = String::from("U+0041 A\n");
    few.push_str(&"#....\n".repeat(HEIGHT - 1));
    refused(&format!("{few}\nU+0042 B\n"), HEIGHT + 1, "has 10 rows, but needs 11");
    refused(&format!("{few}U+0042 B\n"), HEIGHT + 1, "has 10 rows, but needs 11");
    refused(&few, HEIGHT + 1, "has 10 rows, but needs 11");
    // Too many.
    let many = format!("{}#....\n", entry("U+0041 A"));
    refused(&many, HEIGHT + 2, "more than 11 rows");
}

#[test]
fn the_parser_refuses_other_characters_in_rows() {
    let text = entry("U+0041 A").replacen("#....", "#.x..", 1);
    refused(&text, 2, "'x' is not # or .");
    let text = entry("U+0041 A").replacen("#....", "#. ..", 1);
    refused(&text, 2, "' ' is not # or .");
    let text = entry("U+0041 A").replacen("#....", "#.0..", 1);
    refused(&text, 2, "'0' is not # or .");
}

#[test]
fn the_parser_refuses_entries_out_of_order_or_repeated() {
    let second = HEIGHT + 3;
    refused(&format!("{}\n{}", entry("U+0042 B"), entry("U+0041 A")), second, "out of order");
    refused(&format!("{}\n{}", entry("U+0041 A"), entry("U+0041 A")), second, "repeated");
}

#[test]
fn the_generator_writes_one_commented_line_per_glyph() {
    let entries = parse(&format!("{}\n{}", entry("U+0020 space"), entry("U+005C \\"))).unwrap();
    let text = generate(&entries);
    assert!(text.contains("[(u32, [u8; HEIGHT]); 2]"));
    assert!(text.contains(
        "(0x0020, [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10]), // space\n"
    ));
    assert!(text.contains("(0x005c, ["));
    assert!(text.contains("]), // \\\n"));
}

// ---- Which characters the font has ----

/// The characters the font must have, in code point order.
fn expected_characters() -> Vec<char> {
    let mut all: Vec<char> = Vec::new();
    all.extend('\u{20}'..='\u{7E}'); // ASCII
    all.extend('\u{A0}'..='\u{FF}'); // Latin-1
    all.extend('\u{100}'..='\u{17F}'); // Latin Extended-A
    all.extend([
        '\u{2013}', '\u{2014}', // en dash, em dash
        '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', // single and double quotes
        '\u{2022}', '\u{2026}', '\u{20AC}', // bullet, ellipsis, euro
        '\u{2190}', '\u{2191}', '\u{2192}', '\u{2193}', // arrows
        '\u{2212}', '\u{2713}', // minus sign, check mark
    ]);
    all
}

#[test]
fn the_font_has_exactly_the_characters_it_should() {
    let expected = expected_characters();
    assert_eq!(expected.len(), 334);
    let have: Vec<char> = characters().collect();
    assert_eq!(have.len(), GLYPHS.len());
    assert_eq!(have, expected);
    assert!(have.windows(2).all(|pair| pair[0] < pair[1]), "sorted, none twice");
}

#[test]
fn lookup_finds_every_character_and_only_those() {
    for (code, glyph) in GLYPHS.iter() {
        let c = char::from_u32(*code).unwrap();
        assert_eq!(lookup(c), Some(glyph), "{c:?}");
        assert_eq!(super::glyph(c), glyph, "{c:?}");
    }
    let question = super::glyph('?');
    assert_eq!(lookup('?'), Some(question));
    for c in [
        '\0',
        '\n',
        '\t',
        '\u{1F}',
        '\u{7F}',
        '\u{80}',
        '\u{9F}',
        '\u{180}',
        '\u{2012}',
        '\u{2015}',
        '\u{2714}',
        '\u{1F600}',
        char::MAX,
    ] {
        assert_eq!(lookup(c), None, "{c:?}");
        assert_eq!(super::glyph(c), question, "{c:?}");
    }
    // Every character in the first few blocks is either in the list or not found.
    let expected = expected_characters();
    for code in 0..0x2800u32 {
        if let Some(c) = char::from_u32(code) {
            assert_eq!(lookup(c).is_some(), expected.contains(&c), "{c:?}");
        }
    }
}

#[test]
fn the_first_95_entries_are_ascii_in_order() {
    // `lookup` indexes these directly.
    for (i, (code, _)) in GLYPHS.iter().take(95).enumerate() {
        assert_eq!(*code, 0x20 + i as u32);
    }
    assert_eq!(GLYPHS[94].0, 0x7E);
    assert_eq!(GLYPHS[95].0, 0xA0);
}

// ---- What the glyphs look like ----

#[test]
fn no_glyph_uses_the_unused_bits() {
    for (code, glyph) in GLYPHS.iter() {
        assert!(glyph.iter().all(|row| row & 0xE0 == 0), "U+{code:04X}");
    }
}

#[test]
fn ascii_leaves_the_rows_above_the_capitals_empty() {
    for (code, glyph) in GLYPHS.iter().take(95) {
        assert!(glyph[..ABOVE].iter().all(|&row| row == 0), "U+{code:04X}");
    }
    assert_eq!(ABOVE, 2);
    assert_eq!((WIDTH, HEIGHT), (5, 11));
}

#[test]
fn only_the_spaces_are_blank() {
    for (code, glyph) in GLYPHS.iter() {
        let blank = glyph.iter().all(|&row| row == 0);
        assert_eq!(blank, *code == 0x20 || *code == 0xA0, "U+{code:04X}");
    }
}

/// Characters that look exactly the same, on purpose.
const SAME_ON_PURPOSE: [(char, char); 6] = [
    // These are meant to be identical.
    (' ', '\u{A0}'),       // space, no-break space
    ('-', '\u{AD}'),       // hyphen, soft hyphen (which shows as a hyphen when it shows)
    ('\u{D0}', '\u{110}'), // Eth and D with stroke are the same letter shape
    // With 5 dots of width there is no way to make an en dash wider than a minus sign, or an
    // em dash wider than an en dash; all three are a bar across the cell, at the height of the
    // bar of `+`.
    ('\u{2013}', '\u{2014}'),
    ('\u{2013}', '\u{2212}'),
    ('\u{2014}', '\u{2212}'),
];

#[test]
fn no_two_characters_look_the_same_by_accident() {
    let mut seen: HashMap<Glyph, Vec<char>> = HashMap::new();
    for (code, glyph) in GLYPHS.iter() {
        seen.entry(*glyph).or_default().push(char::from_u32(*code).unwrap());
    }
    let mut same: Vec<(char, char)> = Vec::new();
    for chars in seen.values() {
        for (i, &a) in chars.iter().enumerate() {
            for &b in &chars[i + 1..] {
                same.push((a, b));
            }
        }
    }
    same.sort();
    let mut allowed = SAME_ON_PURPOSE.to_vec();
    allowed.sort();
    assert_eq!(same, allowed);
}

// ---- An accented letter keeps its base letter ----

/// (accented character, base character, the rows that must be identical to the base's).
/// An accent goes above the base (rows 0 and 1 for capitals and letters with ascenders, rows
/// 1 to 3 for the others), or below it (rows 9 and 10), so the rows in between stay as they
/// were. Where the base was deliberately changed, the rows that are kept say how:
/// - `Å` (ring): the ring is closed by the top row of the `A`, so only rows 3 to 10 are kept;
/// - `i` and `j` lose their dot, whatever the mark: rows 4 to 10;
/// - `ť ľ Ľ ŉ`: the tick or apostrophe sits beside the top of the letter, so rows 4 to 10.
///
/// `ď` isn't here: its tick needs a column to itself beside the stem, so the whole letter is
/// drawn a dot narrower (it is in [`DRAWN_WHOLE`]).
fn kept_bases() -> Vec<(char, char, RangeInclusive<usize>)> {
    vec![
        ('À', 'A', 2..=10),
        ('Á', 'A', 2..=10),
        ('Â', 'A', 2..=10),
        ('Ã', 'A', 2..=10),
        ('Ä', 'A', 2..=10),
        ('Å', 'A', 3..=10),
        ('Ç', 'C', 2..=8),
        ('È', 'E', 2..=10),
        ('É', 'E', 2..=10),
        ('Ê', 'E', 2..=10),
        ('Ë', 'E', 2..=10),
        ('Ì', 'I', 2..=10),
        ('Í', 'I', 2..=10),
        ('Î', 'I', 2..=10),
        ('Ï', 'I', 2..=10),
        ('Ñ', 'N', 2..=10),
        ('Ò', 'O', 2..=10),
        ('Ó', 'O', 2..=10),
        ('Ô', 'O', 2..=10),
        ('Õ', 'O', 2..=10),
        ('Ö', 'O', 2..=10),
        ('Ù', 'U', 2..=10),
        ('Ú', 'U', 2..=10),
        ('Û', 'U', 2..=10),
        ('Ü', 'U', 2..=10),
        ('Ý', 'Y', 2..=10),
        ('à', 'a', 4..=10),
        ('á', 'a', 4..=10),
        ('â', 'a', 4..=10),
        ('ã', 'a', 4..=10),
        ('ä', 'a', 4..=10),
        ('å', 'a', 4..=10),
        ('ç', 'c', 4..=8),
        ('è', 'e', 4..=10),
        ('é', 'e', 4..=10),
        ('ê', 'e', 4..=10),
        ('ë', 'e', 4..=10),
        ('ì', 'i', 4..=10),
        ('í', 'i', 4..=10),
        ('î', 'i', 4..=10),
        ('ï', 'i', 4..=10),
        ('ñ', 'n', 4..=10),
        ('ò', 'o', 4..=10),
        ('ó', 'o', 4..=10),
        ('ô', 'o', 4..=10),
        ('õ', 'o', 4..=10),
        ('ö', 'o', 4..=10),
        ('ù', 'u', 4..=10),
        ('ú', 'u', 4..=10),
        ('û', 'u', 4..=10),
        ('ü', 'u', 4..=10),
        ('ý', 'y', 4..=10),
        ('ÿ', 'y', 4..=10),
        ('Ā', 'A', 2..=10),
        ('ā', 'a', 4..=10),
        ('Ă', 'A', 2..=10),
        ('ă', 'a', 4..=10),
        ('Ą', 'A', 2..=8),
        ('ą', 'a', 4..=8),
        ('Ć', 'C', 2..=10),
        ('ć', 'c', 4..=10),
        ('Ĉ', 'C', 2..=10),
        ('ĉ', 'c', 4..=10),
        ('Ċ', 'C', 2..=10),
        ('ċ', 'c', 4..=10),
        ('Č', 'C', 2..=10),
        ('č', 'c', 4..=10),
        ('Ď', 'D', 2..=10),
        ('Ē', 'E', 2..=10),
        ('ē', 'e', 4..=10),
        ('Ĕ', 'E', 2..=10),
        ('ĕ', 'e', 4..=10),
        ('Ė', 'E', 2..=10),
        ('ė', 'e', 4..=10),
        ('Ę', 'E', 2..=8),
        ('ę', 'e', 4..=8),
        ('Ě', 'E', 2..=10),
        ('ě', 'e', 4..=10),
        ('Ĝ', 'G', 2..=10),
        ('ĝ', 'g', 4..=10),
        ('Ğ', 'G', 2..=10),
        ('ğ', 'g', 4..=10),
        ('Ġ', 'G', 2..=10),
        ('ġ', 'g', 4..=10),
        ('Ģ', 'G', 2..=8),
        ('ģ', 'g', 4..=10),
        ('Ĥ', 'H', 2..=10),
        ('ĥ', 'h', 2..=10),
        ('Ĩ', 'I', 2..=10),
        ('ĩ', 'i', 4..=10),
        ('Ī', 'I', 2..=10),
        ('ī', 'i', 4..=10),
        ('Ĭ', 'I', 2..=10),
        ('ĭ', 'i', 4..=10),
        ('Į', 'I', 2..=8),
        ('į', 'i', 4..=8),
        ('İ', 'I', 2..=10),
        ('ı', 'i', 4..=10),
        ('Ĵ', 'J', 2..=10),
        ('ĵ', 'j', 4..=10),
        ('Ķ', 'K', 2..=8),
        ('ķ', 'k', 2..=8),
        ('Ĺ', 'L', 2..=10),
        ('ĺ', 'l', 2..=10),
        ('Ļ', 'L', 2..=8),
        ('ļ', 'l', 2..=8),
        ('Ľ', 'L', 4..=10),
        ('ľ', 'l', 4..=10),
        ('Ń', 'N', 2..=10),
        ('ń', 'n', 4..=10),
        ('Ņ', 'N', 2..=8),
        ('ņ', 'n', 4..=8),
        ('Ň', 'N', 2..=10),
        ('ň', 'n', 4..=10),
        ('ŉ', 'n', 4..=10),
        ('Ō', 'O', 2..=10),
        ('ō', 'o', 4..=10),
        ('Ŏ', 'O', 2..=10),
        ('ŏ', 'o', 4..=10),
        ('Ő', 'O', 2..=10),
        ('ő', 'o', 4..=10),
        ('Ŕ', 'R', 2..=10),
        ('ŕ', 'r', 4..=10),
        ('Ŗ', 'R', 2..=8),
        ('ŗ', 'r', 4..=8),
        ('Ř', 'R', 2..=10),
        ('ř', 'r', 4..=10),
        ('Ś', 'S', 2..=10),
        ('ś', 's', 4..=10),
        ('Ŝ', 'S', 2..=10),
        ('ŝ', 's', 4..=10),
        ('Ş', 'S', 2..=8),
        ('ş', 's', 4..=8),
        ('Š', 'S', 2..=10),
        ('š', 's', 4..=10),
        ('Ţ', 'T', 2..=8),
        ('ţ', 't', 2..=8),
        ('Ť', 'T', 2..=10),
        ('ť', 't', 4..=10),
        ('Ũ', 'U', 2..=10),
        ('ũ', 'u', 4..=10),
        ('Ū', 'U', 2..=10),
        ('ū', 'u', 4..=10),
        ('Ŭ', 'U', 2..=10),
        ('ŭ', 'u', 4..=10),
        ('Ů', 'U', 2..=10),
        ('ů', 'u', 4..=10),
        ('Ű', 'U', 2..=10),
        ('ű', 'u', 4..=10),
        ('Ų', 'U', 2..=8),
        ('ų', 'u', 4..=8),
        ('Ŵ', 'W', 2..=10),
        ('ŵ', 'w', 4..=10),
        ('Ŷ', 'Y', 2..=10),
        ('ŷ', 'y', 4..=10),
        ('Ÿ', 'Y', 2..=10),
        ('Ź', 'Z', 2..=10),
        ('ź', 'z', 4..=10),
        ('Ż', 'Z', 2..=10),
        ('ż', 'z', 4..=10),
        ('Ž', 'Z', 2..=10),
        ('ž', 'z', 4..=10),
    ]
}

/// The characters in Latin-1 and Latin Extended-A that are drawn whole, not as a base letter
/// and a mark (apart from the symbols, which are not letters).
const DRAWN_WHOLE: [char; 30] = [
    'Æ', 'Ð', 'Ø', 'Þ', 'ß', 'æ', 'ð', 'ø', 'þ', 'ď', 'Đ', 'đ', 'Ħ', 'ħ', 'Ĳ', 'ĳ', 'ĸ', 'Ŀ', 'ŀ',
    'Ł', 'ł', 'Ŋ', 'ŋ', 'Œ', 'œ', 'Ŧ', 'ŧ', 'ſ', '×', '÷',
];

#[test]
fn an_accented_letter_keeps_its_base_letters_dots() {
    for (accented, base, rows) in kept_bases() {
        let (a, b) = (super::glyph(accented), super::glyph(base));
        assert_ne!(a, b, "{accented} must differ from {base}");
        for row in rows {
            assert_eq!(a[row], b[row], "{accented} and {base}, row {row}");
        }
    }
}

#[test]
fn every_letter_is_either_base_plus_mark_or_drawn_whole() {
    let kept: Vec<char> = kept_bases().iter().map(|&(accented, _, _)| accented).collect();
    for code in 0xC0..=0x17Fu32 {
        let c = char::from_u32(code).unwrap();
        let (in_kept, whole) = (kept.contains(&c), DRAWN_WHOLE.contains(&c));
        assert!(in_kept != whole, "U+{code:04X} {c} is in exactly one of the two lists");
    }
    assert_eq!(kept.len() + DRAWN_WHOLE.len(), 0x180 - 0xC0);
}

#[test]
fn letters_without_ascenders_never_reach_row_zero() {
    for (accented, base, _) in kept_bases() {
        if base.is_lowercase() && !"bdhklt".contains(base) {
            assert_eq!(super::glyph(accented)[0], 0, "{accented}");
        }
    }
}

// ---- The 1.x font ----

/// Every ASCII glyph is the one `classic/tools/mkassets.py` (the 1.x system's asset generator)
/// makes: its 9 rows are rows 2 to 10 of ours. The script writes a C file; this reads the font
/// table back out of it. Skipped if python3 is missing.
#[test]
fn every_ascii_glyph_matches_the_classic_font() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../classic/tools/mkassets.py");
    let dir = std::env::temp_dir().join(format!("pbr-font-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("assets.c");
    // `-I` stops Python reading modules from the current folder.
    let run = match Command::new("python3").arg("-I").arg(script).arg(&out).output() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipped: python3 is not available ({e})");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    };
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let c = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let table = c.split("font_data[95][9] = {").nth(1).expect("the font table");
    let table = table.split("};").next().unwrap();
    let rows: Vec<Vec<u8>> = table
        .lines()
        .filter_map(|l| l.trim().strip_prefix('{'))
        .map(|l| {
            let numbers = l.split('}').next().unwrap();
            numbers
                .split(',')
                .map(|n| u8::from_str_radix(n.trim().trim_start_matches("0x"), 16).unwrap())
                .collect()
        })
        .collect();
    assert_eq!(rows.len(), 95);
    for (i, row) in rows.iter().enumerate() {
        let c = char::from(32 + i as u8);
        assert_eq!(row.as_slice(), &super::glyph(c)[2..], "the glyph for {c:?}");
    }
}
