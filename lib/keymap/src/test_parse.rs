// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The reader: every line kind, every refusal, the size limits, and hostile files.

use crate::test_layouts::{GB_FILE, INTL_FILE, US_FILE};
use crate::{Layout, MAX_LINE, MAX_SIZE, Mods, ParseError, Problem, Typed, parse};
use pbr_devices::keys::{ALL, key};

/// A smallest good file, one line (the title) before whatever a test adds.
const T: &str = "title \"T\"\n";

fn err(text: &str) -> (usize, Problem) {
    match parse(text.as_bytes()) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(ParseError { line, problem }) => (line, problem),
    }
}

fn ok(text: &str) -> Layout {
    match parse(text.as_bytes()) {
        Ok(layout) => layout,
        Err(e) => panic!("refused ({e}):\n{text}"),
    }
}

fn typed(layout: &Layout, code: u16, shift: bool, altgr: bool) -> Typed {
    layout.key(code, Mods { shift, altgr, caps_lock: false, num_lock: true })
}

#[test]
fn every_line_kind_is_accepted() {
    let layout = ok(concat!(
        "# a comment\n",
        "title \"My layout\"\n",
        "altgr\n",
        "\n",
        "key A a A á Á\n",
        "key 1 1\n",
        "key 2 2 @ none ²\n",
        "key SPACE space\n",
        "key APOSTROPHE dead:acute dead:acute\n",
        "dead acute a á space '\n",
    ));
    assert_eq!(layout.title(), "My layout");
    assert!(layout.has_altgr());
    let a = key::A;
    let at = |s, g| typed(&layout, a, s, g);
    assert_eq!(at(false, false), Typed::Char('a'));
    assert_eq!(at(true, false), Typed::Char('A'));
    assert_eq!(at(false, true), Typed::Char('á'));
    assert_eq!(at(true, true), Typed::Char('Á'));
    // Shift left out is the same as plain; the AltGr levels left out type nothing.
    assert_eq!(typed(&layout, key::D1, true, false), Typed::Char('1'));
    assert_eq!(typed(&layout, key::D1, false, true), Typed::Nothing);
    assert_eq!(typed(&layout, key::D1, true, true), Typed::Nothing);
    // `none` is how a later level is given when an earlier one is empty.
    assert_eq!(typed(&layout, key::D2, false, true), Typed::Nothing);
    assert_eq!(typed(&layout, key::D2, true, true), Typed::Char('²'));
    assert_eq!(typed(&layout, key::SPACE, true, false), Typed::Char(' '));
    // A key the file leaves out types nothing.
    assert_eq!(typed(&layout, key::B, false, false), Typed::Nothing);
    let Typed::Dead(d) = typed(&layout, key::APOSTROPHE, true, false) else { panic!("dead") };
    assert_eq!(layout.accent(d), '\'');
    assert_eq!(layout.combine(d, 'a'), Some('á'));
    assert_eq!(layout.combine(d, 'b'), None);
    assert_eq!(layout.combine(d, ' '), Some('\''));
}

#[test]
fn a_file_with_only_a_title_and_one_key_is_a_layout() {
    let layout = ok(&format!("{T}key A a\n"));
    assert!(!layout.has_altgr());
    assert_eq!(layout.title(), "T");
    // The last line needs no newline, and an empty file is refused for its missing title.
    ok("title \"T\"\nkey A a");
    assert_eq!(err(""), (0, Problem::TitleMissing));
}

#[test]
fn line_ends_tabs_and_indentation() {
    let layout =
        ok("title \"T\"\r\n\r\n\tkey\tA\t a \t A\t\r\n  # indented comment\r\n   altgr  \r\n");
    assert_eq!(typed(&layout, key::A, true, false), Typed::Char('A'));
    assert!(layout.has_altgr());
}

#[test]
fn comments_are_whole_lines_and_may_hold_anything() {
    ok("# a \" quote and ünïcödé and 日本語 and # more\ntitle \"T\"\nkey A a\n");
    // After the first word `#` is an ordinary character, so there are no end-of-line comments.
    assert_eq!(err(&format!("{T}key A a # comment\n")), (2, Problem::BadValue));
    assert_eq!(err(&format!("{T}key A a #comment\n")), (2, Problem::BadValue));
    // (`#` alone is a character, so this types a hash with Shift.)
    assert_eq!(typed(&ok(&format!("{T}key A a #\n")), key::A, true, false), Typed::Char('#'));
    assert_eq!(err("title \"T\" # note\n"), (1, Problem::BadTitle));
    assert_eq!(err(&format!("{T}altgr # note\n")), (2, Problem::AltgrWithWords));
}

#[test]
fn hash_and_quote_and_other_scripts_are_values() {
    let layout = ok(&format!(
        "{T}key 3 3 #\nkey APOSTROPHE ' \"\nkey A a Ж\nkey B # \"\nkey C é ÿ\nkey D 日 本\n"
    ));
    assert_eq!(typed(&layout, key::D3, true, false), Typed::Char('#'));
    assert_eq!(typed(&layout, key::APOSTROPHE, true, false), Typed::Char('"'));
    assert_eq!(typed(&layout, key::A, true, false), Typed::Char('Ж'));
    assert_eq!(typed(&layout, key::B, false, false), Typed::Char('#'));
    assert_eq!(typed(&layout, key::B, true, false), Typed::Char('"'));
    assert_eq!(typed(&layout, key::D, true, false), Typed::Char('本'));
    // A dead table can hold them too.
    let layout = ok(&format!("{T}key A dead:q\ndead q # \" \" # space \"\n"));
    let Typed::Dead(d) = typed(&layout, key::A, false, false) else { panic!("dead") };
    assert_eq!(layout.combine(d, '#'), Some('"'));
    assert_eq!(layout.combine(d, '"'), Some('#'));
}

#[test]
fn refuses_the_wrong_first_word() {
    assert_eq!(err(&format!("{T}keys A a\n")), (2, Problem::UnknownWord));
    assert_eq!(err(&format!("{T}KEY A a\n")), (2, Problem::UnknownWord));
    assert_eq!(err(&format!("{T}Title \"x\"\n")), (2, Problem::UnknownWord));
    assert_eq!(err(&format!("{T}altgrs\n")), (2, Problem::UnknownWord));
    assert_eq!(err(&format!("{T}\u{a0}key A a\n")), (2, Problem::UnknownWord));
}

#[test]
fn refuses_bad_titles() {
    assert_eq!(err("key A a\n"), (0, Problem::TitleMissing));
    assert_eq!(err("altgr\n"), (0, Problem::TitleMissing));
    assert_eq!(err(&format!("{T}title \"U\"\n")), (2, Problem::TitleTwice));
    assert_eq!(err("title\n"), (1, Problem::BadTitle));
    assert_eq!(err("title T\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"T\n"), (1, Problem::BadTitle));
    assert_eq!(err("title T\"\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"\"\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"   \"\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"a\"b\"\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"a\" \"b\"\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"a\"x\n"), (1, Problem::BadTitle));
    assert_eq!(err("title \"a\u{85}b\"\n"), (1, Problem::BadTitle));
    let long = format!("title \"{}\"\n", "x".repeat(65));
    assert_eq!(err(&long), (1, Problem::BadTitle));
    // 64 bytes is fine, and so are other scripts, counted in bytes.
    ok(&format!("title \"{}\"\nkey A a\n", "x".repeat(64)));
    ok(&format!("title \"{}\"\nkey A a\n", "é".repeat(32)));
    assert_eq!(err(&format!("title \"{}\"\n", "é".repeat(33))), (1, Problem::BadTitle));
    // Spaces and a # inside are part of the name.
    assert_eq!(ok("title \"  A # B  \"\nkey A a\n").title(), "  A # B  ");
}

#[test]
fn refuses_bad_altgr_lines() {
    assert_eq!(err(&format!("{T}altgr\naltgr\n")), (3, Problem::AltgrTwice));
    assert_eq!(err(&format!("{T}altgr now\n")), (2, Problem::AltgrWithWords));
    assert_eq!(err(&format!("{T}altgr \"\"\n")), (2, Problem::AltgrWithWords));
}

#[test]
fn refuses_bad_key_lines() {
    let p = |line: &str| err(&format!("{T}{line}\n"));
    assert_eq!(p("key"), (2, Problem::NoKeyName));
    assert_eq!(p("key \t "), (2, Problem::NoKeyName));
    assert_eq!(p("key A"), (2, Problem::WrongValueCount));
    assert_eq!(p("key A a b c d e"), (2, Problem::WrongValueCount));
    assert_eq!(p("key a a"), (2, Problem::UnknownKey));
    assert_eq!(p("key KP_77 7"), (2, Problem::UnknownKey));
    assert_eq!(p("key D1 1"), (2, Problem::UnknownKey));
    assert_eq!(p("key KEY_A a"), (2, Problem::UnknownKey));
    assert_eq!(p("key A a\nkey A b"), (3, Problem::KeyTwice));
    assert_eq!(p("key A ab"), (2, Problem::BadValue));
    assert_eq!(p("key A a ab"), (2, Problem::BadValue));
    assert_eq!(p("key A Space"), (2, Problem::BadValue));
    assert_eq!(p("key A NONE"), (2, Problem::BadValue));
    assert_eq!(p("key A \u{a0}"), (2, Problem::BadValue));
    assert_eq!(p("key A \u{85}"), (2, Problem::BadValue));
    assert_eq!(p("key A \u{2028}"), (2, Problem::BadValue));
    assert_eq!(p("key A dead:"), (2, Problem::BadValue));
    assert_eq!(p("key A dead:Acute"), (2, Problem::BadValue));
    assert_eq!(p("key A dead:a1"), (2, Problem::BadValue));
    assert_eq!(p("key A dead:abcdefghijklmnopq"), (2, Problem::BadValue));
    assert_eq!(p("key A dead :a"), (2, Problem::BadValue));
    assert_eq!(p("key A none"), (2, Problem::KeyTypesNothing));
    assert_eq!(p("key A none none"), (2, Problem::KeyTypesNothing));
    // Several values, all empty, is still nothing; the value count is checked first.
    assert_eq!(p("key A none none none none"), (2, Problem::KeyTypesNothing));
}

#[test]
fn refuses_the_keys_that_are_not_written_in_layout_files() {
    let refused = [
        "ENTER",
        "TAB",
        "BACKSPACE",
        "ESCAPE",
        "DELETE",
        "LEFT_SHIFT",
        "RIGHT_SHIFT",
        "LEFT_CTRL",
        "RIGHT_CTRL",
        "LEFT_ALT",
        "RIGHT_ALT",
        "LEFT_META",
        "RIGHT_META",
        "CAPS_LOCK",
        "NUM_LOCK",
        "SCROLL_LOCK",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "UP",
        "DOWN",
        "LEFT",
        "RIGHT",
        "HOME",
        "END",
        "PAGE_UP",
        "PAGE_DOWN",
        "INSERT",
        "PRINT_SCREEN",
        "PAUSE",
        "MENU",
        "KP_ENTER",
    ];
    for name in refused {
        assert_eq!(err(&format!("{T}key {name} x\n")), (2, Problem::ControlKey), "{name}");
    }
    // Every other key in the table is ordinary.
    let mut allowed = 0;
    for (name, _) in ALL {
        if !refused.contains(name) {
            let layout = ok(&format!("{T}key {name} x\n"));
            assert_eq!(layout.title(), "T");
            allowed += 1;
        }
    }
    assert_eq!(allowed + refused.len(), ALL.len());
    assert_eq!(allowed, 26 + 10 + 12 + 1 + 15);
}

#[test]
fn refuses_altgr_values_in_a_file_with_no_altgr_line() {
    assert_eq!(err(&format!("{T}key A a A á\n")), (2, Problem::AltgrWithoutLine));
    assert_eq!(err(&format!("{T}key A a A none Á\n")), (2, Problem::AltgrWithoutLine));
    assert_eq!(err(&format!("{T}key A a A none\n")), (2, Problem::AltgrWithoutLine));
    // The altgr line may come after the key.
    ok(&format!("{T}key A a A á\naltgr\n"));
    ok(&format!("{T}altgr\nkey A a A á\n"));
}

#[test]
fn refuses_bad_dead_lines() {
    let p = |lines: &str| err(&format!("{T}key A dead:acute\n{lines}\n"));
    assert_eq!(p("dead"), (3, Problem::BadDeadName));
    assert_eq!(p("dead Acute space '"), (3, Problem::BadDeadName));
    assert_eq!(p("dead acute1 space '"), (3, Problem::BadDeadName));
    assert_eq!(p("dead abcdefghijklmnopq space '"), (3, Problem::BadDeadName));
    assert_eq!(p("dead é space '"), (3, Problem::BadDeadName));
    assert_eq!(p("dead acute space"), (3, Problem::OddDeadWords));
    assert_eq!(p("dead acute space ' a"), (3, Problem::OddDeadWords));
    assert_eq!(p("dead acute"), (3, Problem::DeadNeedsSpace));
    assert_eq!(p("dead acute a á"), (3, Problem::DeadNeedsSpace));
    assert_eq!(p("dead acute space ' a á a Á"), (3, Problem::DeadBaseTwice));
    assert_eq!(p("dead acute space ' space \""), (3, Problem::DeadBaseTwice));
    assert_eq!(p("dead acute ab á space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(p("dead acute a áa space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(p("dead acute a space space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(p("dead acute a dead space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(p("dead acute none a space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(p("dead acute \u{a0} a space '"), (3, Problem::BadDeadCharacter));
    assert_eq!(
        err(&format!("{T}key A dead:acute\ndead acute space '\ndead acute space `\n")),
        (4, Problem::DeadTwice)
    );
    ok(&format!("{T}key A dead:acute\ndead acute space '\n"));
}

#[test]
fn refuses_dead_keys_without_tables_and_tables_without_keys() {
    assert_eq!(err(&format!("{T}key A dead:acute\n")), (2, Problem::UnknownDead));
    assert_eq!(
        err(&format!("{T}key A dead:acute\nkey B dead:grave\ndead acute space '\n")),
        (3, Problem::UnknownDead)
    );
    assert_eq!(err(&format!("{T}key A a\ndead acute space '\n")), (3, Problem::UnusedDead));
    assert_eq!(
        err(&format!("{T}key A dead:acute\ndead acute space '\ndead grave space `\n")),
        (4, Problem::UnusedDead)
    );
    // The table may come before or after the key that uses it, or between two keys.
    ok(&format!("{T}dead acute space '\nkey A dead:acute\n"));
    ok(&format!("{T}key A dead:acute\ndead acute space '\n"));
    ok(&format!("{T}key A dead:acute\ndead acute space '\nkey B dead:acute a\n"));
    // Any level may be a dead key, and a key may be dead on several.
    ok(&format!("{T}altgr\nkey A a A none dead:acute\ndead acute space '\n"));
}

#[test]
fn refuses_too_many_dead_keys_and_pairs() {
    let letters = "abcdefghijklmnopqrstuvwxyz";
    let name = |i: usize| letters[i..i + 1].repeat(2);
    let build = |n: usize| {
        let mut s = String::from(T);
        for (i, (key_name, _)) in ALL.iter().enumerate().take(n) {
            s += &format!("key {key_name} dead:{}\n", name(i));
        }
        for i in 0..n {
            s += &format!("dead {} space '\n", name(i));
        }
        s
    };
    ok(&build(16));
    assert_eq!(err(&build(17)), (1 + 17 + 17, Problem::TooManyDead));
    // 64 pairs (with the space pair) are fine; 65 are not.
    let pairs = |n: usize| {
        let bases: String = ('!'..='~').take(n - 1).collect();
        let mut line = String::from("dead x space '");
        for b in bases.chars() {
            line += &format!(" {b} {b}");
        }
        format!("{T}key A dead:x\n{line}\n")
    };
    ok(&pairs(64));
    assert_eq!(err(&pairs(65)), (3, Problem::TooManyPairs));
}

#[test]
fn the_earliest_problem_is_reported() {
    let text = format!("{T}key A dead:nope\nkey B b c d\n");
    assert_eq!(err(&text), (2, Problem::UnknownDead));
    assert_eq!(err(&format!("{T}key B b c d\nkey A dead:nope\n")), (2, Problem::AltgrWithoutLine));
    // A problem on a line beats the missing title.
    assert_eq!(err("key A dead:nope\n"), (1, Problem::UnknownDead));
}

#[test]
fn refuses_text_that_is_not_text() {
    assert_eq!(err(&format!("{T}key A a\u{0}\n")), (2, Problem::ControlCharacter));
    assert_eq!(err(&format!("{T}key A a\u{1b}\n")), (2, Problem::ControlCharacter));
    assert_eq!(err(&format!("{T}key A a\u{7f}\n")), (2, Problem::ControlCharacter));
    assert_eq!(err(&format!("{T}# comment \u{7}\n")), (2, Problem::ControlCharacter));
    // A carriage return anywhere but just before the newline.
    assert_eq!(err(&format!("{T}key A\ra\n")), (2, Problem::ControlCharacter));
    assert_eq!(err(&format!("{T}key A a\r\r\n")), (2, Problem::ControlCharacter));
    assert_eq!(err(&format!("{T}key A a\r")), (2, Problem::ControlCharacter));
    assert_eq!(err("title \"T\"\rkey A a\n"), (1, Problem::ControlCharacter));
    // Bytes that are not UTF-8.
    let bad: [&[u8]; 5] = [b"\xff", b"\xc3", b"\xc3\x28", b"\xe2\x82", b"\xed\xa0\x80"];
    for bytes in bad {
        let mut text = T.as_bytes().to_vec();
        text.extend_from_slice(b"# ");
        text.extend_from_slice(bytes);
        text.push(b'\n');
        assert_eq!(parse(&text).map(|_| ()).unwrap_err().problem, Problem::NotUtf8, "{bytes:?}");
        assert_eq!(parse(&text).map(|_| ()).unwrap_err().line, 2);
    }
    // The errors read as plain English.
    let e = parse(b"title \"T\"\nkey A a\x00\n").map(|_| ()).unwrap_err();
    assert_eq!(e.to_string(), "line 2: the line has a control character in it");
    let e = parse(b"key A a\n").map(|_| ()).unwrap_err();
    assert_eq!(e.to_string(), "there is no title line");
}

/// A file of exactly `size` bytes: the title and a key, then comment lines.
fn file_of(size: usize) -> String {
    let mut s = format!("{T}key A a\n");
    while size - s.len() > 100 {
        s += &format!("#{}\n", "x".repeat(98));
    }
    match size - s.len() {
        0 => {}
        1 => s += "#",
        n => s += &format!("#{}\n", "x".repeat(n - 2)),
    }
    assert_eq!(s.len(), size);
    s
}

#[test]
fn the_file_size_limit_is_exact() {
    ok(&file_of(MAX_SIZE));
    ok(&file_of(MAX_SIZE - 1));
    assert_eq!(err(&file_of(MAX_SIZE + 1)), (0, Problem::FileTooLong));
    assert_eq!(parse(&vec![b'x'; 1 << 20]).map(|_| ()).unwrap_err().problem, Problem::FileTooLong);
}

#[test]
fn the_line_length_limit_is_exact() {
    let line = |n: usize| format!("# {}", "x".repeat(n - 2));
    ok(&format!("{T}key A a\n{}\n", line(MAX_LINE)));
    assert_eq!(err(&format!("{T}key A a\n{}\n", line(MAX_LINE + 1))), (3, Problem::LineTooLong));
    // The carriage return of a Windows line end does not count.
    ok(&format!("{T}key A a\r\n{}\r\n", line(MAX_LINE)));
    assert_eq!(
        err(&format!("{T}key A a\r\n{}\r\n", line(MAX_LINE + 1))),
        (3, Problem::LineTooLong)
    );
    // Bytes are counted, not characters.
    let wide = format!("# {}", "é".repeat((MAX_LINE - 2) / 2));
    assert_eq!(wide.len(), MAX_LINE);
    ok(&format!("{T}key A a\n{wide}\n"));
    assert_eq!(err(&format!("{T}key A a\n{wide}x\n")), (3, Problem::LineTooLong));
    // A long dead key line is also refused, and a long line is refused even if it is a comment.
    let long_dead = format!("dead x space ' {}", "a a ".repeat(100));
    assert_eq!(err(&format!("{T}key A dead:x\n{long_dead}\n")), (3, Problem::LineTooLong));
}

#[test]
fn the_real_layouts_read() {
    for text in [US_FILE, GB_FILE, INTL_FILE] {
        assert!(text.len() < MAX_SIZE / 2);
        assert!(text.lines().all(|l| l.len() <= MAX_LINE));
        ok(text);
        // Windows line ends read the same.
        let crlf = text.replace('\n', "\r\n");
        let (a, b) = (ok(text), ok(&crlf));
        for code in 0..120 {
            for bits in 0..16u8 {
                let m = Mods {
                    shift: bits & 1 != 0,
                    altgr: bits & 2 != 0,
                    caps_lock: bits & 4 != 0,
                    num_lock: bits & 8 != 0,
                };
                assert_eq!(a.key(code, m), b.key(code, m));
            }
        }
    }
}

// ---- Damaged and hostile files ----

const NOISE: &[u8] = b"abcdefghijklmnopqrstuvwxyz \n\t\"#:KM0123456789\r\xc3\xa9";

/// A small deterministic random number generator (xorshift), so a failing run can be repeated.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Everything an accepted file must be: the title is 1 to 64 bytes, every key is a typing key
/// of the table and appears once, and every dead key works with `accent` and `combine`.
fn invariants(text: &[u8], layout: &Layout) {
    let title = layout.title();
    assert!((1..=64).contains(&title.len()), "{title:?}");
    assert!(!title.contains('"'));
    assert!(text.len() <= MAX_SIZE);
    // No key twice, and every key name is in the table (read from the text itself).
    let source = core::str::from_utf8(text).expect("accepted text is UTF-8");
    let mut seen = Vec::new();
    for line in source.lines() {
        let mut words = line.split([' ', '\t']).filter(|w| !w.is_empty());
        if words.next() == Some("key") {
            let name = words.next().expect("a key name");
            assert!(ALL.iter().any(|(n, _)| *n == name), "{name}");
            assert!(!seen.contains(&name), "{name} twice");
            seen.push(name);
        }
    }
    let highest = ALL.last().unwrap().1;
    assert_eq!(layout.slots.len(), usize::from(highest) + 1);
    for code in 0..=120 {
        for bits in 0..16u8 {
            let m = Mods {
                shift: bits & 1 != 0,
                altgr: bits & 2 != 0,
                caps_lock: bits & 4 != 0,
                num_lock: bits & 8 != 0,
            };
            match layout.key(code, m) {
                Typed::Nothing => {}
                Typed::Char(c) => {
                    assert!(!c.is_control() && !c.is_whitespace() || c == ' ');
                    assert!(ALL.iter().any(|&(_, k)| k == code), "code {code} types");
                }
                Typed::Dead(d) => {
                    let accent = layout.accent(d);
                    assert_eq!(layout.combine(d, ' '), Some(accent));
                    assert!(!accent.is_control());
                }
            }
        }
    }
    // Codes that are no key, or are keys that layouts never hold, type nothing.
    for code in [0, 106, 107, 1000, u16::MAX] {
        assert_eq!(layout.key(code, Mods::default()), Typed::Nothing);
    }
    assert!(layout.dead.len() <= 16);
    for table in &layout.dead {
        assert!(table.pairs.len() <= 64);
        assert!(table.pairs.iter().any(|&(b, _)| b == ' '));
        for (i, a) in table.pairs.iter().enumerate() {
            assert!(table.pairs.iter().skip(i + 1).all(|b| a.0 != b.0));
        }
    }
    // Every table is used by some key.
    for i in 0..layout.dead.len() {
        let used = layout
            .slots
            .iter()
            .flatten()
            .any(|t| matches!(t, Typed::Dead(d) if usize::from(d.0) == i));
        assert!(used, "table {i} unused");
    }
}

fn check_whatever(bytes: &[u8]) {
    if let Ok(layout) = parse(bytes) {
        invariants(bytes, &layout);
    }
}

#[test]
fn damaged_real_files_never_panic_and_obey_the_invariants() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut accepted = 0;
    for text in [US_FILE, GB_FILE, INTL_FILE] {
        for _ in 0..6000 {
            let mut b = text.as_bytes().to_vec();
            for _ in 0..1 + rng.below(6) {
                let i = rng.below(b.len());
                match rng.below(4) {
                    0 => b[i] = rng.next() as u8,
                    1 => {
                        b.remove(i);
                    }
                    2 => b.insert(i, NOISE[rng.below(NOISE.len())]),
                    _ => b[i] ^= 1 << rng.below(8),
                }
            }
            if parse(&b).is_ok() {
                accepted += 1;
            }
            check_whatever(&b);
        }
    }
    // Plenty of damage lands in comments and values, so some files are still good.
    assert!(accepted > 100, "{accepted}");
}

#[test]
fn damaged_real_files_cut_short_never_panic() {
    for text in [US_FILE, GB_FILE, INTL_FILE] {
        for cut in 0..text.len() {
            check_whatever(&text.as_bytes()[..cut]);
        }
    }
}

#[test]
fn random_files_made_of_the_format_s_words_never_panic() {
    let words = [
        "title",
        "altgr",
        "key",
        "dead",
        "\"T\"",
        "\"",
        "#",
        "space",
        "none",
        "dead:acute",
        "dead:grave",
        "dead:",
        "acute",
        "grave",
        "a",
        "A",
        "é",
        "\u{a0}",
        "'",
        "\\",
        "A",
        "B",
        "1",
        "KP_7",
        "KP_PERIOD",
        "102ND",
        "SPACE",
        "APOSTROPHE",
        "ENTER",
        "F1",
        "x",
        "xy",
        "7",
        "ÿ",
        "日",
        "\t",
        "  ",
    ];
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let mut accepted = 0;
    for _ in 0..30_000 {
        let mut text = String::new();
        if rng.below(4) != 0 {
            text += "title \"T\"\n";
        }
        for _ in 0..rng.below(8) {
            // Mostly well-shaped lines, with random words in them.
            match rng.below(6) {
                0 => text += "altgr",
                1 => text += "dead acute space '",
                2 => text += "key A dead:acute",
                _ => text += words[rng.below(words.len())],
            }
            for _ in 0..rng.below(6) {
                text += if rng.below(8) == 0 { "\t" } else { " " };
                text += words[rng.below(words.len())];
            }
            text += if rng.below(10) == 0 { "\r\n" } else { "\n" };
        }
        if parse(text.as_bytes()).is_ok() {
            accepted += 1;
        }
        check_whatever(text.as_bytes());
    }
    assert!(accepted > 20, "{accepted}");
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x0bad_cafe);
    for _ in 0..5000 {
        let n = rng.below(200);
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        check_whatever(&bytes);
    }
}
