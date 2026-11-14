// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The places at the side of the picker and Files.

use pbr_browse::places::{place_of, places};
use pbr_browse::{Place, PlaceKind};

#[test]
fn a_standard_user_sees_the_six() {
    let list = places(false, &["Data", "Backup"]);
    let seen: Vec<(&str, &str)> =
        list.iter().map(|p| (p.title.as_str(), p.path.as_str())).collect();
    assert_eq!(
        seen,
        [
            ("Home", "/Home"),
            ("Documents", "/Home/Documents"),
            ("Pictures", "/Home/Pictures"),
            ("Music", "/Home/Music"),
            ("Videos", "/Home/Videos"),
            ("Downloads", "/Home/Downloads"),
        ]
    );
    assert_eq!(list[0].kind, PlaceKind::Home);
    assert_eq!(list[5].kind, PlaceKind::Downloads);
}

#[test]
fn an_administrator_also_sees_volumes_in_order() {
    let list = places(true, &["Data", "Backup"]);
    assert_eq!(list.len(), 8);
    assert_eq!(
        list[6],
        Place { kind: PlaceKind::Volume, title: "Data".into(), path: "/Volumes/Data".into() }
    );
    assert_eq!(list[7].path, "/Volumes/Backup");
    assert_eq!(places(true, &[]).len(), 6);
}

#[test]
fn bad_volume_names_are_skipped() {
    let list = places(true, &["", "a/b", "..", " x", "ok", "tab\t", "fine name"]);
    let volumes: Vec<&str> = list[6..].iter().map(|p| p.title.as_str()).collect();
    assert_eq!(volumes, ["ok", "fine name"]);
}

#[test]
fn kinds_know_their_paths_titles_and_icons() {
    assert_eq!(PlaceKind::Home.path(), Some("/Home"));
    assert_eq!(PlaceKind::Documents.path(), Some("/Home/Documents"));
    assert_eq!(PlaceKind::Pictures.path(), Some("/Home/Pictures"));
    assert_eq!(PlaceKind::Music.path(), Some("/Home/Music"));
    assert_eq!(PlaceKind::Videos.path(), Some("/Home/Videos"));
    assert_eq!(PlaceKind::Downloads.path(), Some("/Home/Downloads"));
    assert_eq!(PlaceKind::Volume.path(), None);
    assert_eq!(PlaceKind::Volume.title(), "Volume");
    for kind in [
        PlaceKind::Home,
        PlaceKind::Documents,
        PlaceKind::Pictures,
        PlaceKind::Music,
        PlaceKind::Videos,
        PlaceKind::Downloads,
        PlaceKind::Volume,
    ] {
        let icon = kind.icon();
        assert!(icon.iter().any(|&d| d >> 24 == 0xFF), "{kind:?} draws something");
        assert!(icon.iter().all(|&d| d == 0 || d >> 24 == 0xFF), "{kind:?}: solid or clear");
    }
}

#[test]
fn place_of_finds_the_longest_container() {
    let list = places(true, &["Data"]);
    let at = |p: &str| place_of(p, &list).map(|i| list[i].title.as_str());
    assert_eq!(at("/Home"), Some("Home"));
    assert_eq!(at("/Home/Documents"), Some("Documents"));
    assert_eq!(at("/Home/Documents/a/b"), Some("Documents"));
    assert_eq!(at("/Home/Other"), Some("Home"));
    assert_eq!(at("/Home/DocumentsX"), Some("Home"));
    assert_eq!(at("/Volumes/Data/x"), Some("Data"));
    assert_eq!(at("/Volumes"), None);
    assert_eq!(at("/"), None);
    assert_eq!(at("/Volumes/Other"), None);
    assert_eq!(at(""), None);
    assert_eq!(place_of("/Home", &[]), None);
}

#[test]
fn place_of_prefers_the_first_of_two_equal_places() {
    let list = places(true, &["Data", "Data"]);
    assert_eq!(place_of("/Volumes/Data", &list), Some(6));
}
