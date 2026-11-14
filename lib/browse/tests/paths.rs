// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Paths and names.

use pbr_browse::BrowseError;
use pbr_browse::path::{check_name, crumbs, extension, inside, join, name, parent, tidy};

#[test]
fn join_makes_one_slash() {
    assert_eq!(join("/", "a"), "/a");
    assert_eq!(join("/a", "b"), "/a/b");
    assert_eq!(join("/a/", "b"), "/a/b");
    assert_eq!(join("", "b"), "/b");
}

#[test]
fn parent_walks_up() {
    assert_eq!(parent("/a/b"), Some("/a"));
    assert_eq!(parent("/a"), Some("/"));
    assert_eq!(parent("/"), None);
    assert_eq!(parent(""), None);
    assert_eq!(parent("a"), None);
    assert_eq!(parent("/a/b/c/"), Some("/a/b"));
    assert_eq!(parent("/é/ü"), Some("/é"));
}

#[test]
fn name_is_the_last_part() {
    assert_eq!(name("/a/b"), "b");
    assert_eq!(name("/a"), "a");
    assert_eq!(name("/"), "/");
    assert_eq!(name("/a/b/"), "b");
    assert_eq!(name("plain"), "plain");
    assert_eq!(name(""), "/");
    assert_eq!(name("//"), "/");
}

#[test]
fn crumbs_for_the_path_bar() {
    let want = |list: &[(&str, &str)]| -> Vec<(String, String)> {
        list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    };
    assert_eq!(crumbs("/"), want(&[("/", "/")]));
    assert_eq!(
        crumbs("/Home/Documents"),
        want(&[("/", "/"), ("Home", "/Home"), ("Documents", "/Home/Documents")])
    );
    assert_eq!(crumbs("/a//b/"), want(&[("/", "/"), ("a", "/a"), ("b", "/a/b")]));
    assert_eq!(crumbs(""), want(&[("/", "/")]));
}

#[test]
fn inside_means_whole_parts() {
    assert!(inside("/a", "/a"));
    assert!(inside("/", "/a"));
    assert!(inside("/", "/"));
    assert!(inside("/a", "/a/b/c"));
    assert!(!inside("/a", "/ab"));
    assert!(!inside("/a/b", "/a"));
    assert!(!inside("/a", "/"));
    assert!(!inside("/a", "a/b"));
    assert!(!inside("/", "relative"));
    assert!(inside("/a/", "/a/b"));
    assert!(!inside("/é", "/éa"));
}

#[test]
fn extensions() {
    assert_eq!(extension("a.txt"), Some("txt"));
    assert_eq!(extension("a.b.TAR"), Some("TAR"));
    assert_eq!(extension("noext"), None);
    assert_eq!(extension(".profile"), None);
    assert_eq!(extension("a."), None);
    assert_eq!(extension("."), None);
    assert_eq!(extension(".."), None);
    assert_eq!(extension(""), None);
    assert_eq!(extension(".tar.gz"), Some("gz"));
    assert_eq!(extension("é.ü"), Some("ü"));
}

#[test]
fn good_and_bad_names() {
    for good in ["a", "letter.txt", ".hidden", "a b", "é", "...", "a.", &"x".repeat(255)] {
        assert_eq!(check_name(good), Ok(()), "{good:?}");
    }
    assert_eq!(check_name(&"é".repeat(255)), Ok(()));
    for bad in [
        "",
        ".",
        "..",
        "a/b",
        "/",
        " a",
        "a ",
        " ",
        "a\tb",
        "a\nb",
        "a\0b",
        "a\x7fb",
        "\x1f",
        &"x".repeat(256),
        &"é".repeat(256),
    ] {
        assert_eq!(check_name(bad), Err(BrowseError::BadName), "{bad:?}");
    }
}

#[test]
fn tidy_paths() {
    for good in ["/", "/a", "/a/b", "/Home/Documents/letter.txt", "/.hidden", "/a/...", "/é"] {
        assert_eq!(tidy(good).as_deref(), Some(good));
    }
    for bad in
        ["", "a", "a/b", "//", "/a/", "/a//b", "/./a", "/a/.", "/a/..", "/..", "//a", "/a/../b"]
    {
        assert_eq!(tidy(bad), None, "{bad:?}");
    }
}
