// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests on the host: tidying paths, the rules for trees, the format (and damage to it), and
//! finding where a path is.

extern crate std;

use super::*;
use alloc::vec;

fn m(path: &str, kind: Kind) -> Mount {
    Mount { path: String::from(path), kind }
}

#[test]
fn tidying_paths() {
    for (cur, p, want) in [
        ("/", "a/b", "/a/b"),
        ("/Home", "Documents", "/Home/Documents"),
        ("/Home", "/x", "/x"),
        ("/", "", "/"),
        ("/Home", ".", "/Home"),
        ("/Home", "..", "/"),
        ("/", "..", "/"),
        ("/", "../../..", "/"),
        ("/a/b", "../../../c", "/c"),
        ("/", "a//b/./c/", "/a/b/c"),
        ("/Home/Documents", "../Pictures/x.png", "/Home/Pictures/x.png"),
    ] {
        assert_eq!(normalize(cur, p).as_deref(), Ok(want), "{cur} + {p}");
    }
    assert_eq!(normalize("/", "a\0b"), Err(Error::BadPath));
    assert_eq!(normalize("/", &"x".repeat(256)), Err(Error::BadPath));
    assert_eq!(normalize("/", &"a/".repeat(3000)), Err(Error::BadPath));
}

#[test]
fn the_rules_for_trees() {
    let ok = [m("/Home/Documents", Kind::Folder), m("/Home/Notes/todo.txt", Kind::File)];
    assert_eq!(check(&ok), Ok(()));
    for (bad, why) in [
        (vec![m("/", Kind::Folder)], Error::BadPath),
        (vec![m("Home", Kind::Folder)], Error::BadPath),
        (vec![m("/Home/", Kind::Folder)], Error::BadPath),
        (vec![m("/Home/../x", Kind::Folder)], Error::BadPath),
        (vec![m("/a", Kind::Folder), m("/a", Kind::File)], Error::Duplicate),
        (vec![m("/a", Kind::Folder), m("/a/b", Kind::File)], Error::Nested),
        (vec![m("/a/b", Kind::File), m("/a", Kind::Folder)], Error::Nested),
    ] {
        assert_eq!(check(&bad), Err(why), "{bad:?}");
    }
    // `/ab` isn't inside `/a`.
    assert_eq!(check(&[m("/a", Kind::Folder), m("/ab", Kind::Folder)]), Ok(()));
    let many: Vec<Mount> = (0..257).map(|i| m(&alloc::format!("/m{i}"), Kind::Folder)).collect();
    assert_eq!(check(&many), Err(Error::TooBig));
}

#[test]
fn the_format() {
    let mounts = vec![m("/Home/Documents", Kind::Folder), m("/Home/Notes/tödö.txt", Kind::File)];
    let b = encode(&mounts).unwrap();
    assert_eq!(&b[..8], b"PBRT\x01\x00\x02\x00");
    assert_eq!(b.len() % 8, 0);
    assert_eq!(decode(&b), Ok(mounts.clone()));
    assert_eq!(decode(&encode(&[]).unwrap()), Ok(vec![]));
    // Every single-byte change is caught or still decodes to valid entries; nothing panics.
    for i in 0..b.len() {
        for v in [0u8, 1, 0x2F, 0x80, 0xFF] {
            let mut c = b.clone();
            c[i] = v;
            if let Ok(d) = decode(&c) {
                assert_eq!(check(&d), Ok(()));
            }
        }
    }
    // Every truncation is refused.
    for n in 0..b.len() {
        assert!(decode(&b[..n]).is_err(), "truncated to {n}");
    }
    // Zeros after the last entry (the rest of a page) are fine; anything else isn't.
    let mut padded = b.clone();
    padded.resize(4096, 0);
    assert_eq!(decode(&padded), Ok(mounts.clone()));
    padded[4000] = 1;
    assert_eq!(decode(&padded), Err(Error::Format));
    // A role out of order is refused.
    let mut c = b.clone();
    c[16] = 101;
    assert_eq!(decode(&c), Err(Error::Format));
}

#[test]
fn finding_paths() {
    let t = Tree::new(vec![
        m("/Home/Documents", Kind::Folder),
        m("/Home/Notes/todo.txt", Kind::File),
        m("/Volumes/Data", Kind::Folder),
    ])
    .unwrap();
    assert_eq!(t.find("/Home/Documents"), Found::Entry { index: 0, rest: "" });
    assert_eq!(t.find("/Home/Documents/a/b.txt"), Found::Entry { index: 0, rest: "a/b.txt" });
    assert_eq!(t.find("/Home/Notes/todo.txt"), Found::Entry { index: 1, rest: "" });
    assert_eq!(
        t.find("/"),
        Found::Above(vec![("Home".into(), Kind::Folder), ("Volumes".into(), Kind::Folder)])
    );
    assert_eq!(
        t.find("/Home"),
        Found::Above(vec![("Documents".into(), Kind::Folder), ("Notes".into(), Kind::Folder)])
    );
    assert_eq!(t.find("/Home/Notes"), Found::Above(vec![("todo.txt".into(), Kind::File)]));
    for nothing in ["/Home/Pictures", "/Home/Notes/other.txt", "/Homes", "/Home/Documentsx"] {
        assert_eq!(t.find(nothing), Found::Nothing, "{nothing}");
    }
    // An empty tree has an empty root, and nothing else.
    let e = Tree::default();
    assert_eq!(e.find("/"), Found::Above(vec![]));
    assert_eq!(e.find("/x"), Found::Nothing);
    // Adding keeps the rules.
    let mut t2 = t.clone();
    assert_eq!(t2.add(m("/Home/Documents/x", Kind::File)), Err(Error::Nested));
    assert_eq!(t2.add(m("/Home/Pictures", Kind::Folder)), Ok(3));
}
