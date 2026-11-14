// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Putting items in order.

mod common;

use common::{file, folder, names};
use pbr_browse::item::{sort, visible};
use pbr_browse::{Item, Sort, SortBy};

fn sorted(mut items: Vec<Item>, by: SortBy, ascending: bool) -> Vec<Item> {
    sort(&mut items, Sort { by, ascending });
    items
}

#[test]
fn the_default_is_by_name_ascending() {
    assert_eq!(Sort::default(), Sort { by: SortBy::Name, ascending: true });
}

#[test]
fn folders_come_first_in_both_directions() {
    let items = vec![file("a", 1, 1), folder("z"), file("b", 2, 2), folder("m")];
    let up = sorted(items.clone(), SortBy::Name, true);
    assert_eq!(names(&up), ["m", "z", "a", "b"]);
    let down = sorted(items, SortBy::Name, false);
    assert_eq!(names(&down), ["z", "m", "b", "a"]);
}

#[test]
fn names_are_in_natural_order() {
    let items = vec![file("a10", 0, 0), file("b1", 0, 0), file("A3", 0, 0), file("a2", 0, 0)];
    assert_eq!(names(&sorted(items, SortBy::Name, true)), ["a2", "A3", "a10", "b1"]);
}

#[test]
fn case_and_accents_do_not_matter_but_decide_ties() {
    let items = vec![file("é", 0, 0), file("É", 0, 0), file("d", 0, 0), file("f", 0, 0)];
    let out = sorted(items, SortBy::Name, true);
    // É and é are the same letter, so they are together (after plain letters, by code point);
    // the exact bytes settle which is first ("É" is U+00C9, before "é" U+00E9).
    assert_eq!(names(&out), ["d", "f", "É", "é"]);
    let items = vec![file("b", 0, 0), file("B", 0, 0), file("a", 0, 0)];
    assert_eq!(names(&sorted(items, SortBy::Name, true)), ["a", "B", "b"]);
}

#[test]
fn leading_zeros_and_equal_numbers() {
    let items = vec![file("a01", 0, 0), file("a1", 0, 0), file("a2", 0, 0), file("a001", 0, 0)];
    assert_eq!(names(&sorted(items, SortBy::Name, true)), ["a1", "a01", "a001", "a2"]);
    // A later difference beats the length of the zeros.
    let items = vec![file("a1c", 0, 0), file("a01b", 0, 0)];
    assert_eq!(names(&sorted(items, SortBy::Name, true)), ["a01b", "a1c"]);
}

#[test]
fn digit_runs_longer_than_any_number() {
    let big = "9".repeat(30);
    let bigger = format!("1{}", "0".repeat(30));
    let padded = format!("{}{}", "0".repeat(25), "7");
    let items = vec![
        file(&format!("f{bigger}"), 0, 0),
        file(&format!("f{big}"), 0, 0),
        file(&format!("f{padded}"), 0, 0),
        file("f8", 0, 0),
        file(&format!("f{big}x"), 0, 0),
    ];
    let out = sorted(items, SortBy::Name, true);
    assert_eq!(
        names(&out),
        [
            format!("f{padded}").as_str(),
            "f8",
            format!("f{big}").as_str(),
            format!("f{big}x").as_str(),
            format!("f{bigger}").as_str()
        ]
    );
}

#[test]
fn digits_sort_before_letters_and_prefixes_first() {
    let items = vec![file("ab", 0, 0), file("a", 0, 0), file("a1", 0, 0), file("a-", 0, 0)];
    assert_eq!(names(&sorted(items, SortBy::Name, true)), ["a", "a-", "a1", "ab"]);
}

#[test]
fn by_size() {
    let items = vec![
        file("big", 300, 0),
        folder("zz"),
        file("small", 1, 0),
        folder("aa"),
        file("mid", 20, 0),
    ];
    let up = sorted(items.clone(), SortBy::Size, true);
    assert_eq!(names(&up), ["aa", "zz", "small", "mid", "big"]);
    let down = sorted(items, SortBy::Size, false);
    assert_eq!(names(&down), ["zz", "aa", "big", "mid", "small"]);
}

#[test]
fn by_modified() {
    let items = vec![file("c", 0, 30), file("a", 0, 20), folder("d"), file("b", 0, 10)];
    assert_eq!(names(&sorted(items.clone(), SortBy::Modified, true)), ["d", "b", "a", "c"]);
    assert_eq!(names(&sorted(items, SortBy::Modified, false)), ["d", "c", "a", "b"]);
}

#[test]
fn by_kind_uses_the_title_then_the_name() {
    let items = vec![
        file("x.zip", 0, 0),
        file("b.png", 0, 0),
        file("a.png", 0, 0),
        file("n.txt", 0, 0),
        file("m.mp3", 0, 0),
        file("q.unknown", 0, 0),
        folder("f"),
    ];
    // Titles: Archive, File, Music, Picture, Text document.
    assert_eq!(
        names(&sorted(items.clone(), SortBy::Kind, true)),
        ["f", "x.zip", "q.unknown", "m.mp3", "a.png", "b.png", "n.txt"]
    );
    assert_eq!(
        names(&sorted(items, SortBy::Kind, false)),
        ["f", "n.txt", "b.png", "a.png", "m.mp3", "q.unknown", "x.zip"]
    );
}

#[test]
fn ties_are_broken_by_name_so_the_order_is_the_same_whatever_the_start() {
    for by in [SortBy::Name, SortBy::Size, SortBy::Modified, SortBy::Kind] {
        let a = vec![file("b", 5, 5), file("a", 5, 5), file("B", 5, 5), file("c", 5, 5)];
        let mut b = a.clone();
        b.reverse();
        let mut c = a.clone();
        c.rotate_left(2);
        let first = sorted(a, by, true);
        assert_eq!(first, sorted(b, by, true));
        assert_eq!(first, sorted(c, by, true));
        assert_eq!(names(&first), ["a", "B", "b", "c"]);
    }
}

#[test]
fn sorting_nothing_and_one_thing() {
    assert!(sorted(vec![], SortBy::Name, true).is_empty());
    assert_eq!(names(&sorted(vec![file("a", 1, 1)], SortBy::Kind, false)), ["a"]);
}

#[test]
fn visible_drops_hidden_unless_asked() {
    let mut hidden = file(".secret", 1, 1);
    hidden.hidden = true;
    let items = vec![file("a", 1, 1), hidden, folder("d")];
    let shown = visible(items.clone(), false);
    assert_eq!(names(&shown), ["a", "d"]);
    let all = visible(items, true);
    assert_eq!(names(&all), ["a", ".secret", "d"]);
}
