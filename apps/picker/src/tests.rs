// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The picker's window on the host (note 0025, section 5): choosing, filters, places for an
//! administrator and a standard user, replacing, the rights for each request, starting where
//! this app last picked, all against a tree in memory, through sandstone's harness; and its
//! look, against reference pictures in `tests/pictures/picker/`.

use super::*;
use pbr_browse::MemTree;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Description, Key, WindowId};
use std::path::PathBuf;

/// A home with a few things in it, and, for an administrator, two volumes.
fn tree(admin: bool) -> MemTree {
    let mut t = MemTree::new();
    t.add_file("/Home/Documents/letter.txt", 1200, 1_700_000_000)
        .add_file("/Home/Documents/notes.md", 300, 1_700_100_000)
        .add_file("/Home/Documents/budget.txt", 9000, 1_700_200_000)
        .add_file("/Home/Documents/.secret", 10, 0)
        .hide("/Home/Documents/.secret")
        .add_file("/Home/Documents/Recipes/bread.txt", 100, 0)
        .add_file("/Home/Pictures/dusk.bmp", 1_440_054, 1_700_000_000)
        .add_file("/Home/Pictures/protea.png", 210_000, 1_700_000_000)
        .read_only("/Home/Pictures")
        .add_folder("/Home/Downloads")
        .add_folder("/Home/Apps/notepad");
    if admin {
        t.add_file("/Volumes/PrettyBigRock/System/users.txt", 40, 0).add_file(
            "/Volumes/PBR13 DATA/README.TXT",
            500,
            0,
        );
    }
    t
}

fn request(ask: Ask) -> Request {
    Request {
        app: String::from("Notepad"),
        what: String::new(),
        ask,
        start: Start::Place(PlaceKind::Documents),
        filters: vec![Filter::new("Text", "txt md")],
        last: None,
        offset_minutes: 120,
    }
}

fn open_file() -> Ask {
    Ask::Open { folder: false, write: false }
}

fn harness(p: Picker<MemTree>) -> Harness<Picker<MemTree>> {
    Harness::new(p, settings((7, 1)))
}

fn picker(admin: bool, ask: Ask) -> Harness<Picker<MemTree>> {
    harness(Picker::new(tree(admin), request(ask)))
}

fn widget(h: &Harness<Picker<MemTree>>, name: &str) -> (WindowId, Description) {
    h.find(name).unwrap_or_else(|| panic!("no widget {name}"))
}

/// A point in row `n` of the list with id `list`, past its icon: one pixel of edge, the
/// heading's 21 rows, then rows of 20 (as the gallery's tests have it).
fn row_at(h: &Harness<Picker<MemTree>>, list: &str, n: i32) -> (i32, i32) {
    let r = widget(h, list).1.rect;
    (r.x + 40, r.y + 1 + 21 + 20 * n + 10)
}

fn chosen(h: &Harness<Picker<MemTree>>) -> Option<Outcome> {
    h.app().outcome().cloned()
}

#[test]
fn the_title_says_who_asks_and_for_what() {
    let mut r = request(open_file());
    assert_eq!(r.title(), "Notepad wants to open a file");
    assert_eq!(r.warning(), None);
    assert!(!r.write());
    r.ask = Ask::Open { folder: false, write: true };
    assert_eq!(r.warning().as_deref(), Some("Notepad will be able to change the file you choose."));
    assert!(r.write());
    r.ask = Ask::Open { folder: true, write: false };
    assert_eq!(r.title(), "Notepad wants to open a folder");
    r.ask = Ask::Save { name: String::from("x") };
    assert_eq!(r.title(), "Notepad wants to save a file");
    assert!(r.write());
    r.app = String::from("Paint");
    r.what = String::from("save a picture");
    assert_eq!(r.title(), "Paint wants to save a picture");
    // An app's words are cut to fit the title bar, at a character's edge.
    r.what = "é".repeat(60);
    assert!(r.title().len() <= 64 && r.title().starts_with("Paint wants to é"));
}

#[test]
fn it_opens_in_the_place_asked_with_the_filter_and_no_hidden_files() {
    let h = picker(false, open_file());
    let p = h.app();
    assert_eq!(p.folder(), "/Home/Documents");
    // Folders first, then the text files in natural order; `.secret` is hidden.
    assert_eq!(p.names(), ["Recipes", "budget.txt", "letter.txt", "notes.md"]);
    assert_eq!(p.place_titles(), ["Home", "Documents", "Pictures", "Music", "Videos", "Downloads"]);
}

#[test]
fn an_administrator_also_has_the_volumes() {
    let h = picker(true, open_file());
    assert_eq!(
        h.app().place_titles(),
        [
            "Home",
            "Documents",
            "Pictures",
            "Music",
            "Videos",
            "Downloads",
            "PBR13 DATA",
            "PrettyBigRock"
        ]
    );
    // And can browse one.
    let mut h = h;
    let (x, y) = row_at(&h, "places", 6);
    h.click(x, y);
    assert_eq!(h.app().folder(), "/Volumes/PBR13 DATA");
}

#[test]
fn a_place_the_home_lacks_falls_back_to_home_and_last_is_remembered() {
    let mut r = request(open_file());
    r.start = Start::Place(PlaceKind::Music);
    assert_eq!(Picker::new(tree(false), r.clone()).folder(), "/Home");
    r.start = Start::Last;
    assert_eq!(Picker::new(tree(false), r.clone()).folder(), "/Home/Documents");
    r.last = Some(String::from("/Home/Pictures"));
    assert_eq!(Picker::new(tree(false), r.clone()).folder(), "/Home/Pictures");
    r.last = Some(String::from("/Home/Gone"));
    assert_eq!(Picker::new(tree(false), r).folder(), "/Home");
}

#[test]
fn choosing_a_file_by_double_click_and_by_open() {
    let mut h = picker(false, open_file());
    let (x, y) = row_at(&h, "files", 2);
    h.double_click(x, y);
    assert_eq!(
        chosen(&h),
        Some(Outcome::Chosen(Choice {
            path: String::from("/Home/Documents/letter.txt"),
            folder: false,
            new: false
        }))
    );
    assert!(h.finished());

    let mut h = picker(false, open_file());
    // Open with nothing chosen says so, and stays open.
    h.click_on("Open");
    assert_eq!(h.app().problem(), Some("Choose a file to open."));
    // Open on a folder goes into it; then a file.
    let (x, y) = row_at(&h, "files", 0);
    h.click(x, y);
    h.click_on("Open");
    assert_eq!(h.app().folder(), "/Home/Documents/Recipes");
    let (x, y) = row_at(&h, "files", 0);
    h.click(x, y);
    h.key(Key::Enter);
    assert_eq!(
        chosen(&h),
        Some(Outcome::Chosen(Choice {
            path: String::from("/Home/Documents/Recipes/bread.txt"),
            folder: false,
            new: false
        }))
    );
}

#[test]
fn filters_up_and_the_path_bar() {
    let mut h = picker(false, open_file());
    // "All files" is offered after the app's own.
    h.click_on("All files");
    assert_eq!(h.app().names(), ["Recipes", "budget.txt", "letter.txt", "notes.md"]);
    let (x, y) = row_at(&h, "places", 2);
    h.click(x, y);
    assert_eq!(h.app().names(), ["dusk.bmp", "protea.png"]);
    h.click_on("Text");
    assert!(h.app().names().is_empty());
    h.click_on("Up");
    assert_eq!(h.app().folder(), "/Home");
    // The path bar's first part is the top of the tree, which a standard user's is only Home
    // in (it is shown, and can't be changed).
    h.click_on("crumb0");
    assert_eq!(h.app().folder(), "/");
    assert_eq!(h.app().names(), ["Home"]);
}

#[test]
fn sorting_by_a_column_and_back() {
    let mut h = picker(false, open_file());
    let heading = widget(&h, "files").1.rect;
    // The Size heading: right of the Name column, which fills what Size and Modified leave.
    let size_x = heading.x + heading.width - 1 - 124 - 36;
    h.click(size_x, heading.y + 10);
    assert_eq!(h.app().names(), ["Recipes", "notes.md", "letter.txt", "budget.txt"]);
    h.click(size_x, heading.y + 10);
    assert_eq!(h.app().names(), ["Recipes", "budget.txt", "letter.txt", "notes.md"]);
}

#[test]
fn choosing_a_folder() {
    let mut h = picker(false, Ask::Open { folder: true, write: true });
    // Only folders are shown.
    assert_eq!(h.app().names(), ["Recipes"]);
    // Open with nothing selected chooses the folder shown.
    h.click_on("Open");
    assert_eq!(
        chosen(&h),
        Some(Outcome::Chosen(Choice {
            path: String::from("/Home/Documents"),
            folder: true,
            new: false
        }))
    );
    // The top of the tree can't be chosen.
    let mut h = picker(false, Ask::Open { folder: true, write: false });
    h.click_on("crumb0");
    h.click_on("Open");
    assert_eq!(chosen(&h), None);
    assert_eq!(h.app().problem(), Some("Choose a folder inside it."));
}

#[test]
fn saving_a_new_file_adds_the_filter_s_extension() {
    let mut h = picker(false, Ask::Save { name: String::from("Untitled.txt") });
    // The name field has the focus, with the offered name: typing replaces nothing until it is
    // selected, so clear it first.
    let field = widget(&h, "name").1;
    assert!(field.focused);
    for _ in 0.."Untitled.txt".len() {
        h.key(Key::Backspace);
    }
    h.type_text("shopping");
    h.key(Key::Enter);
    assert_eq!(
        chosen(&h),
        Some(Outcome::Chosen(Choice {
            path: String::from("/Home/Documents/shopping.txt"),
            folder: false,
            new: true
        }))
    );
}

#[test]
fn saving_over_a_file_asks_first() {
    let mut h = picker(false, Ask::Save { name: String::from("letter.txt") });
    h.click_on("Save");
    let dialog = h.dialog().expect("Replace? asked");
    assert!(h.describe_lines(dialog).iter().any(|l| l.contains("Replace \\\"letter.txt\\\"?")));
    // Cancel keeps the picker open...
    h.click_on("Cancel");
    assert!(h.dialog().is_none());
    assert_eq!(chosen(&h), None);
    // ...and Replace chooses it, not new.
    h.click_on("Save");
    h.click_on("Replace");
    assert_eq!(
        chosen(&h),
        Some(Outcome::Chosen(Choice {
            path: String::from("/Home/Documents/letter.txt"),
            folder: false,
            new: false
        }))
    );
}

#[test]
fn saving_a_name_that_is_a_folder_opens_it_and_bad_names_are_refused() {
    let mut h = picker(false, Ask::Save { name: String::from("Recipes") });
    // A filter's extension is only added to a name without one, and "Recipes" has none, so
    // with All files chosen it is the folder.
    h.click_on("All files");
    h.click_on("Save");
    assert_eq!(h.app().folder(), "/Home/Documents/Recipes");
    assert_eq!(chosen(&h), None);
    let mut h = picker(false, Ask::Save { name: String::new() });
    h.click_on("Save");
    assert_eq!(h.app().problem(), Some("Type a name to save as."));
    h.type_text("a/b");
    h.click_on("Save");
    assert!(h.app().problem().unwrap().starts_with("A name can't"));
    assert_eq!(chosen(&h), None);
}

#[test]
fn picking_a_file_offers_its_name_for_saving() {
    let mut h = picker(false, Ask::Save { name: String::new() });
    let (x, y) = row_at(&h, "files", 3);
    h.click(x, y);
    assert_eq!(widget(&h, "name").1.what.value, "notes.md");
}

#[test]
fn new_folder() {
    let mut h = picker(false, open_file());
    h.click_on("New folder");
    let d = h.dialog().expect("asked for a name");
    // The dialog's line starts with "New folder" selected: typing replaces it.
    h.type_text("Letters");
    h.key(Key::Enter);
    assert!(h.window(d).is_none());
    assert_eq!(h.app().names(), ["Letters", "Recipes", "budget.txt", "letter.txt", "notes.md"]);
    // A second with the same name says so.
    h.click_on("New folder");
    h.type_text("Letters");
    h.key(Key::Enter);
    assert_eq!(h.app().problem(), Some("\"Letters\" is there already."));
    // Nor in a folder the picker may only read.
    let (x, y) = row_at(&h, "places", 2);
    h.click(x, y);
    h.click_on("New folder");
    h.type_text("Nope");
    h.key(Key::Enter);
    assert_eq!(h.app().problem(), Some("Nothing can be changed here."));
}

#[test]
fn cancel_escape_and_closing_all_cancel() {
    let mut h = picker(false, open_file());
    h.click_on("Cancel");
    assert_eq!(chosen(&h), Some(Outcome::Cancelled));
    let mut h = picker(false, open_file());
    h.key(Key::Escape);
    assert_eq!(chosen(&h), Some(Outcome::Cancelled));
    let mut h = picker(false, open_file());
    h.close(WindowId::MAIN);
    assert_eq!(chosen(&h), Some(Outcome::Cancelled));
    assert!(h.finished());
}

/// Where the reference pictures are kept.
fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/picker")
}

#[test]
fn how_it_looks() {
    let mut r = request(Ask::Open { folder: false, write: true });
    r.filters = vec![Filter::new("Text", "txt md")];
    for (factor, name) in [(1, "open-1x"), (2, "open-2x")] {
        let h = Harness::with_scale(Picker::new(tree(true), r.clone()), settings((7, 1)), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    let r = request(Ask::Save { name: String::from("letter.txt") });
    for (factor, name) in [(1, "save-1x"), (2, "save-2x")] {
        let h = Harness::with_scale(Picker::new(tree(false), r.clone()), settings((7, 1)), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}
