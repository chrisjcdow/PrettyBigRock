// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! File-type filters.

mod common;

use common::{file, folder};
use pbr_browse::Filter;
use pbr_browse::filter::with_all;

#[test]
fn new_splits_cleans_and_lowers() {
    let f = Filter::new("Pictures", "bmp png");
    assert_eq!(f.title, "Pictures");
    assert_eq!(f.extensions, ["bmp", "png"]);
    assert_eq!(Filter::new("x", "bmp, .PNG").extensions, ["bmp", "png"]);
    assert_eq!(Filter::new("x", " ,, .JPG\t.jpg  Jpg,.,").extensions, ["jpg"]);
    assert_eq!(Filter::new("x", "").extensions, Vec::<String>::new());
    assert_eq!(Filter::new("x", "...").extensions, Vec::<String>::new());
}

#[test]
fn all_matches_everything() {
    let f = Filter::all();
    assert_eq!(f.title, "All files");
    assert!(f.extensions.is_empty());
    assert_eq!(f.first_extension(), None);
    assert!(f.matches(&file("a.txt", 0, 0)));
    assert!(f.matches(&file("noextension", 0, 0)));
    assert!(f.matches(&file(".profile", 0, 0)));
    assert!(f.matches(&folder("d")));
}

#[test]
fn matching_ignores_case_and_always_shows_folders() {
    let f = Filter::new("Pictures", "bmp png");
    assert!(f.matches(&file("a.BMP", 0, 0)));
    assert!(f.matches(&file("a.b.Png", 0, 0)));
    assert!(!f.matches(&file("a.txt", 0, 0)));
    assert!(!f.matches(&file("png", 0, 0)));
    assert!(!f.matches(&file(".png", 0, 0)));
    assert!(!f.matches(&file("a.", 0, 0)));
    assert!(f.matches(&folder("holiday.txt")));
}

#[test]
fn first_extension() {
    assert_eq!(Filter::new("x", "TXT md").first_extension(), Some("txt"));
}

#[test]
fn with_all_adds_all_files_last() {
    let list = with_all(&[Filter::new("Pictures", "bmp"), Filter::new("Text", "txt")]);
    assert_eq!(list.len(), 3);
    assert_eq!(list[2], Filter::all());
    assert_eq!(with_all(&[]), [Filter::all()]);
}

#[test]
fn with_all_does_not_add_a_second_one() {
    let mine = Filter::new("Everything", "");
    let list = with_all(&[Filter::new("Pictures", "bmp"), mine.clone()]);
    assert_eq!(list.len(), 2);
    assert_eq!(list[1], mine);
}

#[test]
fn with_all_keeps_at_most_eight() {
    let many: Vec<Filter> = (0..12).map(|i| Filter::new(&format!("T{i}"), "x")).collect();
    let list = with_all(&many);
    assert_eq!(list.len(), 9);
    assert_eq!(list[7].title, "T7");
    assert_eq!(list[8], Filter::all());
    // A match-everything filter that was cut off doesn't count.
    let mut many = many;
    many[10] = Filter::all();
    assert_eq!(with_all(&many).len(), 9);
}
