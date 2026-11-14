// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `MemTree`, the tree in memory.

use pbr_browse::{BrowseError, Folders, Item, MemTree};

fn tree() -> MemTree {
    let mut t = MemTree::new();
    t.add_folder("/Home/Documents")
        .add_file("/Home/Documents/letter.txt", 120, 1000)
        .add_file("/Home/.profile", 5, 2000)
        .hide("/Home/.profile")
        .add_folder("/Locked")
        .read_only("/Locked")
        .add_folder("/Locked/Sub");
    t
}

fn names(mut items: Vec<Item>) -> Vec<String> {
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items.into_iter().map(|i| i.name).collect()
}

#[test]
fn a_new_tree_is_just_the_top() {
    let mut t = MemTree::new();
    assert!(t.exists("/"));
    assert!(!t.exists("/a"));
    assert_eq!(t.list("/"), Ok(vec![]));
    assert!(MemTree::default().exists("/"));
}

#[test]
fn stat_of_the_top_and_of_things() {
    let mut t = tree();
    let top = t.stat("/").unwrap();
    assert_eq!(top, Item { name: "/".into(), folder: true, size: 0, modified: 0, hidden: false });
    let file = t.stat("/Home/Documents/letter.txt").unwrap();
    assert_eq!(
        file,
        Item { name: "letter.txt".into(), folder: false, size: 120, modified: 1000, hidden: false }
    );
    assert!(t.stat("/Home/.profile").unwrap().hidden);
    assert!(t.stat("/Home/Documents").unwrap().folder);
    assert_eq!(t.stat("/nope"), Err(BrowseError::NotFound));
    assert_eq!(t.stat("/Home/Documents/letter.txt/x"), Err(BrowseError::NotFound));
}

#[test]
fn list_answers() {
    let mut t = tree();
    assert_eq!(names(t.list("/").unwrap()), ["Home", "Locked"]);
    assert_eq!(names(t.list("/Home").unwrap()), [".profile", "Documents"]);
    // Not the grandchildren, and not a folder whose name only starts the same.
    t.add_folder("/Home2/x");
    assert_eq!(names(t.list("/Home").unwrap()), [".profile", "Documents"]);
    assert_eq!(names(t.list("/Home/Documents").unwrap()), ["letter.txt"]);
    assert_eq!(t.list("/Home/Documents/letter.txt"), Err(BrowseError::NotAFolder));
    assert_eq!(t.list("/missing"), Err(BrowseError::NotFound));
    assert_eq!(t.list("/Home/"), Err(BrowseError::BadName));
    assert_eq!(t.list("Home"), Err(BrowseError::BadName));
    assert_eq!(t.list(""), Err(BrowseError::BadName));
    assert_eq!(t.list("/Home/../Locked"), Err(BrowseError::BadName));
}

#[test]
fn parents_are_made_and_files_replaced() {
    let mut t = MemTree::new();
    t.add_file("/a/b/c.txt", 1, 2);
    assert!(t.exists("/a") && t.exists("/a/b") && t.exists("/a/b/c.txt"));
    t.add_file("/a/b/c.txt", 9, 8);
    let item = t.stat("/a/b/c.txt").unwrap();
    assert_eq!((item.size, item.modified), (9, 8));
    // A folder is not replaced by a file, nor a file used as a folder.
    t.add_file("/a/b", 1, 1);
    assert!(t.stat("/a/b").unwrap().folder);
    t.add_folder("/a/b/c.txt/deeper").add_file("/a/b/c.txt/f", 1, 1);
    assert!(!t.exists("/a/b/c.txt/deeper") && !t.exists("/a/b/c.txt/f"));
    t.add_folder("/a/b/c.txt");
    assert!(!t.stat("/a/b/c.txt").unwrap().folder);
}

#[test]
fn bad_paths_are_ignored() {
    let mut t = MemTree::new();
    t.add_folder("").add_folder("rel").add_folder("/a//b").add_folder("/a/../b").add_folder("/a/");
    t.add_file("/", 1, 1).add_file("", 1, 1).add_file("/x/", 1, 1).add_file("/ok/bad\nname", 1, 1);
    t.add_folder("/ok/ bad").add_folder("/good/bad /x");
    t.hide("/missing").read_only("/missing").hide("/").read_only("/");
    assert_eq!(t.list("/"), Ok(vec![]));
    assert!(!t.exists("/ok"), "a bad name halfway makes nothing");
    assert!(!t.exists("/good"));
}

#[test]
fn make_folder_answers() {
    let mut t = tree();
    assert_eq!(t.make_folder("/Home/New"), Ok(()));
    assert!(t.stat("/Home/New").unwrap().folder);
    assert_eq!(t.make_folder("/Home/New"), Err(BrowseError::AlreadyExists));
    assert_eq!(t.make_folder("/Home/Documents/letter.txt"), Err(BrowseError::AlreadyExists));
    assert_eq!(t.make_folder("/"), Err(BrowseError::AlreadyExists));
    assert_eq!(t.make_folder("/Nowhere/New"), Err(BrowseError::NotFound));
    assert_eq!(t.make_folder("/Home/Documents/letter.txt/New"), Err(BrowseError::NotAFolder));
    assert_eq!(t.make_folder("/Home/bad name "), Err(BrowseError::BadName));
    assert_eq!(t.make_folder("/Home/a\u{7}b"), Err(BrowseError::BadName));
    assert_eq!(t.make_folder("/Home/"), Err(BrowseError::BadName));
    assert_eq!(t.make_folder("relative"), Err(BrowseError::BadName));
    assert_eq!(t.make_folder("/Home/../X"), Err(BrowseError::BadName));
    assert_eq!(t.make_folder(&format!("/Home/{}", "x".repeat(256))), Err(BrowseError::BadName));
}

#[test]
fn read_only_folders_refuse_everything_below() {
    let mut t = tree();
    assert_eq!(t.make_folder("/Locked/New"), Err(BrowseError::NoRight));
    assert_eq!(t.make_folder("/Locked/Sub/New"), Err(BrowseError::NoRight));
    assert_eq!(t.make_folder("/Locked/Sub"), Err(BrowseError::NoRight));
    // The folder itself may be made inside a writable one, and what is outside is writable.
    assert_eq!(t.make_folder("/Other"), Ok(()));
    // A folder made inside a writable one can be made read-only afterwards.
    t.read_only("/Other");
    assert_eq!(t.make_folder("/Other/x"), Err(BrowseError::NoRight));
    // Read-only does nothing to a file.
    t.read_only("/Home/Documents/letter.txt");
    assert_eq!(t.make_folder("/Home/Documents/letter.txt/x"), Err(BrowseError::NotAFolder));
}

#[test]
fn hostile_paths() {
    let mut t = tree();
    for path in [
        "\0",
        "/\0",
        "//",
        "/ /",
        "/\u{202e}x",
        "/é/ü",
        &"/a".repeat(5000),
        &format!("/{}", "z".repeat(100_000)),
    ] {
        let _ = t.stat(path);
        let _ = t.list(path);
        let _ = t.make_folder(path);
        t.add_folder(path).add_file(path, u64::MAX, u64::MAX).hide(path).read_only(path);
        let _ = t.exists(path);
    }
    assert_eq!(t.stat("/Home").map(|i| i.name), Ok("Home".to_string()));
}
