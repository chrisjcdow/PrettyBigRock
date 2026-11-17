// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Files on the host: listing and sorting, Up and Open, the places, making, renaming and
//! deleting with 1.3's dialogs and answers, double-click and Enter, which app a file opens in,
//! Open with…, the right-click menu, the list noticing changes made behind its back, dates in
//! the user's time zone, the log lines the desktop boot checks, and the look
//! (`tests/pictures/files/`), all against a tree in memory and a pretend launcher.

use super::*;
use pbr_browse::MemTree;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Description, Key};
use std::path::PathBuf;

/// The pretend system: a tree in memory, the apps the sandbox manager would list, and a note of
/// every app started.
struct Fake {
    tree: MemTree,
    openers: Vec<Opener>,
    /// What was launched: the app and the file's path.
    launched: Vec<(String, String)>,
    /// Launching answers this instead of starting anything.
    refuse: Option<String>,
    /// How many times the sandbox manager was asked for `openers`.
    asked: u32,
}

impl Folders for Fake {
    fn list(&mut self, path: &str) -> Result<Vec<Item>, BrowseError> {
        self.tree.list(path)
    }
    fn stat(&mut self, path: &str) -> Result<Item, BrowseError> {
        self.tree.stat(path)
    }
    fn make_folder(&mut self, path: &str) -> Result<(), BrowseError> {
        self.tree.make_folder(path)
    }
    fn make_file(&mut self, path: &str) -> Result<(), BrowseError> {
        self.tree.make_file(path)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), BrowseError> {
        self.tree.rename(from, to)
    }
    fn remove(&mut self, path: &str) -> Result<(), BrowseError> {
        self.tree.remove(path)
    }
}

impl System for Fake {
    fn openers(&mut self) -> Vec<Opener> {
        self.asked += 1;
        self.openers.clone()
    }

    fn open_with(&mut self, app: &str, path: &str) -> Result<u32, String> {
        if let Some(why) = &self.refuse {
            return Err(why.clone());
        }
        self.launched.push((app.into(), path.into()));
        Ok(10 + self.launched.len() as u32)
    }
}

fn opener(app: &str, title: &str, opens: &[&str]) -> Opener {
    Opener {
        app: app.into(),
        title: title.into(),
        opens: opens.iter().map(|e| e.to_string()).collect(),
    }
}

/// The apps of a session, in name order as the sandbox manager gives them.
fn openers() -> Vec<Opener> {
    vec![
        opener("about", "About", &[]),
        opener("calculator", "Calculator", &[]),
        opener("notepad", "Notepad", &["txt", "md"]),
        opener("paint", "Paint", &["bmp"]),
        opener("snake", "Snake", &[]),
    ]
}

/// A user's home with every kind of thing in it. 1_700_000_000 is 22:13:20 on 14 November 2023
/// (UTC).
fn tree() -> MemTree {
    let mut t = MemTree::new();
    t.add_folder("/Home/Work")
        .add_folder("/Home/Music")
        .add_folder("/Home/Videos")
        .add_folder("/Home/Downloads")
        .add_file("/Home/Documents/letter.txt", 120, 1_700_000_000)
        .add_file("/Home/Pictures/rock.bmp", 1_440_054, 1_700_000_000)
        .add_file("/Home/readme.txt", 1234, 1_700_100_000)
        .add_file("/Home/budget.md", 9000, 1_699_900_000)
        .add_file("/Home/photo.jpg", 2_500_000, 1_700_200_000)
        .add_file("/Home/old.zip", 500, 1_600_000_000)
        .add_file("/Home/Notes10.txt", 10, 1_700_000_500)
        .add_file("/Home/Notes2.txt", 20, 1_700_000_600)
        .add_file("/Home/.profile", 5, 1)
        .hide("/Home/.profile");
    t.set_time(1_700_300_000);
    t
}

fn fake() -> Fake {
    Fake { tree: tree(), openers: openers(), launched: Vec::new(), refuse: None, asked: 0 }
}

type Test = Harness<FileManager<Fake>>;

fn harness_with(fake: Fake, offset: i32, factor: u32) -> Test {
    Harness::with_scale(FileManager::new(fake, offset), settings(), factor)
}

fn harness() -> Test {
    harness_with(fake(), 0, 1)
}

fn widget(h: &Test, name: &str) -> (WindowId, Description) {
    h.find(name).unwrap_or_else(|| panic!("no widget {name}"))
}

/// A point in row `n` of the list with id `list`, past its icon: one pixel of edge, the
/// heading's 21 rows, then rows of 20.
fn row_at(h: &Test, list: &str, n: i32) -> (i32, i32) {
    let r = widget(h, list).1.rect;
    (r.x + 40, r.y + 1 + 21 + 20 * n + 10)
}

fn row_of(h: &Test, name: &str) -> i32 {
    h.app().names().iter().position(|n| *n == name).unwrap_or_else(|| panic!("no {name}")) as i32
}

/// Clicks the row of the file called `name`.
fn pick(h: &mut Test, name: &str) {
    let (x, y) = row_at(h, "files", row_of(h, name));
    h.advance(1000);
    h.click(x, y);
}

fn double_click_on(h: &mut Test, name: &str) {
    let (x, y) = row_at(h, "files", row_of(h, name));
    h.advance(1000);
    h.double_click(x, y);
}

fn right_click_on(h: &mut Test, name: &str) {
    let (x, y) = row_at(h, "files", row_of(h, name));
    h.advance(1000);
    h.right_click_in(WindowId::MAIN, x, y);
}

/// The lines of a window's widgets.
fn lines(h: &Test, id: WindowId) -> Vec<String> {
    h.describe(id).iter().map(Description::line).collect()
}

/// What the newest dialog says: its title, and the text of its labels.
fn dialog_text(h: &Test) -> (String, Vec<String>) {
    let d = h.dialog().expect("a dialog");
    let labels = h
        .describe(d)
        .iter()
        .filter(|w| w.what.role == "label")
        .map(|w| w.what.label.clone())
        .collect();
    (h.window(d).unwrap().title.clone(), labels)
}

/// The buttons of the newest dialog, left to right.
fn dialog_buttons(h: &Test) -> Vec<String> {
    let d = h.dialog().expect("a dialog");
    h.describe(d).iter().filter(|w| w.what.role == "button").map(|w| w.what.label.clone()).collect()
}

/// Types into the dialog's line (its text is selected, so this replaces it) and chooses the
/// default button with Enter.
fn answer(h: &mut Test, text: &str) {
    h.type_text(text);
    h.key(Key::Enter);
}

/// The text the dialog's line starts with.
fn dialog_line(h: &Test) -> String {
    let d = h.dialog().expect("a dialog");
    h.describe(d)
        .iter()
        .find(|w| w.what.role == "text field")
        .map(|w| w.what.value.clone())
        .expect("a line to type in")
}

fn button(h: &Test, label: &str) -> Description {
    h.describe(WindowId::MAIN)
        .into_iter()
        .find(|w| w.what.role == "button" && w.what.label == label)
        .unwrap_or_else(|| panic!("no button {label}"))
}

fn click_button(h: &mut Test, label: &str) {
    let r = button(h, label).rect;
    h.click(r.x + r.width / 2, r.y + r.height / 2);
}

fn launched(h: &Test) -> Vec<(&str, &str)> {
    h.app().system().launched.iter().map(|(a, p)| (a.as_str(), p.as_str())).collect()
}

fn log(h: &mut Test) -> Vec<String> {
    h.app_mut().take_log()
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/files")
}

// ---- Listing and sorting. ----

#[test]
fn it_opens_in_home_with_folders_first_then_by_name() {
    let h = harness();
    assert_eq!(h.app().folder(), "/Home");
    // Folders first; names in natural order, without regard to case (Notes2 before Notes10);
    // the hidden file left out.
    assert_eq!(
        h.app().names(),
        [
            "Documents",
            "Downloads",
            "Music",
            "Pictures",
            "Videos",
            "Work",
            "budget.md",
            "Notes2.txt",
            "Notes10.txt",
            "old.zip",
            "photo.jpg",
            "readme.txt"
        ]
    );
    assert_eq!(h.app().selected_name(), None);
    assert_eq!(h.app().title(), "/Home");
}

#[test]
fn a_tree_without_a_home_opens_at_the_top() {
    let mut t = MemTree::new();
    t.add_file("/a.txt", 1, 1);
    let h = harness_with(Fake { tree: t, ..fake() }, 0, 1);
    assert_eq!(h.app().folder(), "/");
    assert_eq!(h.app().names(), ["a.txt"]);
}

#[test]
fn the_list_shows_sizes_and_dates_in_columns() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    let list = widget(&h, "files").1;
    assert_eq!(list.what.label, "Name|Size|Modified");
    assert_eq!(list.what.value, "readme.txt|1.2 KB|16 Nov 2023 02:00");
    pick(&mut h, "Work");
    // A folder has no size.
    assert_eq!(widget(&h, "files").1.what.value, "Work||");
    pick(&mut h, "old.zip");
    assert_eq!(widget(&h, "files").1.what.value, "old.zip|500 bytes|13 Sep 2020 12:26");
}

#[test]
fn clicking_a_heading_sorts_and_clicking_it_again_turns_it_round() {
    let mut h = harness();
    let r = widget(&h, "files").1.rect;
    let heading = |h: &mut Test, x: i32| {
        h.advance(1000);
        h.click(r.x + x, r.y + 10);
    };
    // The Size column is the second from the right: 70 wide, then 124 for Modified, and the
    // vertical scroll bar's room at the end is only taken when there is one.
    let size_x = r.width - DATE_WIDTH - SIZE_WIDTH / 2;
    heading(&mut h, size_x);
    // Folders stay first; the files, smallest first.
    assert_eq!(
        h.app().names()[6..],
        ["Notes10.txt", "Notes2.txt", "old.zip", "readme.txt", "budget.md", "photo.jpg"]
    );
    heading(&mut h, size_x);
    assert_eq!(
        h.app().names()[6..],
        ["photo.jpg", "budget.md", "readme.txt", "old.zip", "Notes2.txt", "Notes10.txt"]
    );
    // Folders still first, whichever way it is turned (and turned with the rest).
    assert_eq!(
        h.app().names()[..6],
        ["Work", "Videos", "Pictures", "Music", "Downloads", "Documents"]
    );
    // Modified: oldest first.
    heading(&mut h, r.width - DATE_WIDTH / 2);
    assert_eq!(
        h.app().names()[6..],
        ["old.zip", "budget.md", "Notes10.txt", "Notes2.txt", "readme.txt", "photo.jpg"]
    );
    // And back to the name.
    heading(&mut h, 20);
    assert_eq!(h.app().names()[6], "budget.md");
}

#[test]
fn the_selection_stays_on_the_same_name_when_the_order_changes() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    let r = widget(&h, "files").1.rect;
    h.click(r.x + r.width - DATE_WIDTH / 2, r.y + 10);
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
}

// ---- Up, Open, the path bar and the places. ----

#[test]
fn up_open_and_the_path_bar_move_between_folders() {
    let mut h = harness();
    pick(&mut h, "Documents");
    click_button(&mut h, "Open");
    assert_eq!(h.app().folder(), "/Home/Documents");
    assert_eq!(h.app().names(), ["letter.txt"]);
    assert_eq!(h.app().selected_name(), None);
    click_button(&mut h, "Up");
    assert_eq!(h.app().folder(), "/Home");
    // The path bar's parts: the top, Home, and (inside) Documents.
    pick(&mut h, "Documents");
    click_button(&mut h, "Open");
    h.click_on("crumb1");
    assert_eq!(h.app().folder(), "/Home");
    h.click_on("crumb0");
    assert_eq!(h.app().folder(), "/");
    assert_eq!(h.app().names(), ["Home"]);
    // Up at the top does nothing: its button is greyed out.
    assert!(!button(&h, "Up").what.enabled);
    // Backspace goes up too.
    pick(&mut h, "Home");
    click_button(&mut h, "Open");
    h.key(Key::Backspace);
    assert_eq!(h.app().folder(), "/");
}

#[test]
fn open_up_and_the_menu_follow_the_selection() {
    let mut h = harness();
    for label in ["Open", "Rename", "Delete"] {
        assert!(!button(&h, label).what.enabled, "{label} needs something selected");
    }
    for label in ["Up", "New file", "New folder"] {
        assert!(button(&h, label).what.enabled, "{label}");
    }
    pick(&mut h, "readme.txt");
    for label in ["Open", "Rename", "Delete"] {
        assert!(button(&h, label).what.enabled, "{label}");
    }
}

#[test]
fn the_places_are_listed_and_go_to_their_folders() {
    let mut h = harness();
    assert_eq!(
        h.app().place_titles(),
        ["Home", "Documents", "Pictures", "Music", "Videos", "Downloads"]
    );
    // Home is the place shown first; Documents becomes it after a click.
    let list = widget(&h, "places").1;
    assert_eq!(list.what.value, "Home");
    let r = list.rect;
    h.click(r.x + 40, r.y + 1 + 21 + 20 + 10);
    assert_eq!(h.app().folder(), "/Home/Documents");
    assert_eq!(widget(&h, "places").1.what.value, "Documents");
    h.click(r.x + 40, r.y + 1 + 21 + 20 * 2 + 10);
    assert_eq!(h.app().folder(), "/Home/Pictures");
    assert_eq!(h.app().names(), ["rock.bmp"]);
    // A folder inside a place is shown as in that place.
    h.click(r.x + 40, r.y + 1 + 21 + 10);
    pick(&mut h, "Documents");
    click_button(&mut h, "Open");
    assert_eq!(widget(&h, "places").1.what.value, "Documents");
}

#[test]
fn an_administrator_also_has_the_volumes() {
    let mut t = tree();
    t.add_file("/Volumes/PrettyBigRock/System/users.txt", 40, 0).add_file(
        "/Volumes/PBR13 DATA/README.TXT",
        500,
        0,
    );
    let h = harness_with(Fake { tree: t, ..fake() }, 0, 1);
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
}

// ---- New file and New folder. ----

#[test]
fn new_file_asks_in_1_3s_words_makes_the_file_and_opens_it_in_notepad() {
    let mut h = harness();
    click_button(&mut h, "New file");
    let (title, labels) = dialog_text(&h);
    assert_eq!(title, "New file");
    assert_eq!(labels, ["Name for the new file:"]);
    assert_eq!(dialog_line(&h), "Untitled.txt");
    assert_eq!(dialog_buttons(&h), ["Cancel", "Make it"]);
    log(&mut h);
    answer(&mut h, "todo.txt");
    assert!(h.app().system().tree.exists("/Home/todo.txt"));
    assert_eq!(h.app().selected_name(), Some("todo.txt"));
    assert_eq!(launched(&h), [("notepad", "/Home/todo.txt")]);
    assert_eq!(
        log(&mut h),
        [
            "made /Home/todo.txt",
            "folder /Home (13 items)",
            "opening /Home/todo.txt with notepad",
            "opened /Home/todo.txt with notepad as app 11",
        ]
    );
}

#[test]
fn new_file_with_the_name_it_offered_makes_untitled_txt() {
    let mut h = harness();
    click_button(&mut h, "New file");
    h.key(Key::Enter);
    assert!(h.app().system().tree.exists("/Home/Untitled.txt"));
    assert_eq!(launched(&h), [("notepad", "/Home/Untitled.txt")]);
}

#[test]
fn new_file_with_a_name_that_is_taken_says_so_as_1_3_did() {
    let mut h = harness();
    click_button(&mut h, "New file");
    answer(&mut h, "readme.txt");
    let (title, labels) = dialog_text(&h);
    assert_eq!(title, "Files");
    assert_eq!(labels, ["Couldn't make the file. Something with that name already exists."]);
    assert_eq!(dialog_buttons(&h), ["OK"]);
    // The old file is as it was, and nothing was opened.
    assert_eq!(h.app_mut().system_mut().tree.stat("/Home/readme.txt").unwrap().size, 1234);
    assert!(launched(&h).is_empty());
    h.key(Key::Enter);
    assert!(h.dialog().is_none());
    // A folder of that name is taken too.
    click_button(&mut h, "New file");
    answer(&mut h, "Work");
    assert!(dialog_text(&h).1[0].starts_with("Couldn't make the file. Something with that name"));
}

#[test]
fn new_file_with_a_bad_name_says_why() {
    let mut h = harness();
    click_button(&mut h, "New file");
    answer(&mut h, "a/b.txt");
    assert_eq!(
        dialog_text(&h).1,
        ["Couldn't make the file. A name can't be empty, contain / or begin or end with a space."]
    );
    assert!(launched(&h).is_empty());
}

#[test]
fn new_file_with_no_name_or_cancelled_does_nothing() {
    let mut h = harness();
    log(&mut h);
    click_button(&mut h, "New file");
    // Only spaces: an empty name.
    h.type_text("   ");
    h.key(Key::Enter);
    assert!(h.dialog().is_none());
    click_button(&mut h, "New file");
    h.key(Key::Escape);
    assert!(h.dialog().is_none());
    assert_eq!(h.app().names().len(), 12);
    assert!(launched(&h).is_empty());
    assert!(log(&mut h).is_empty());
}

#[test]
fn new_folder_asks_makes_it_and_selects_it() {
    let mut h = harness();
    click_button(&mut h, "New folder");
    let (title, labels) = dialog_text(&h);
    assert_eq!(title, "New folder");
    assert_eq!(labels, ["Name for the new folder:"]);
    assert_eq!(dialog_line(&h), "New folder");
    assert_eq!(dialog_buttons(&h), ["Cancel", "Make it"]);
    log(&mut h);
    answer(&mut h, "Games");
    assert!(h.app_mut().system_mut().tree.stat("/Home/Games").unwrap().folder);
    assert_eq!(h.app().selected_name(), Some("Games"));
    // It is made in the folder shown, not entered, and nothing is launched.
    assert_eq!(h.app().folder(), "/Home");
    assert!(launched(&h).is_empty());
    assert_eq!(log(&mut h), ["made /Home/Games", "folder /Home (13 items)"]);
}

#[test]
fn new_folder_with_a_name_that_is_taken_or_bad_says_so() {
    let mut h = harness();
    click_button(&mut h, "New folder");
    answer(&mut h, "Work");
    assert_eq!(
        dialog_text(&h).1,
        ["Couldn't make the folder. Something with that name already exists."]
    );
    h.key(Key::Enter);
    click_button(&mut h, "New folder");
    answer(&mut h, "x/y");
    assert!(dialog_text(&h).1[0].starts_with("Couldn't make the folder. A name can't be empty"));
    h.key(Key::Enter);
    // A file's name is taken too.
    click_button(&mut h, "New folder");
    answer(&mut h, "readme.txt");
    assert!(dialog_text(&h).1[0].contains("already exists"));
}

#[test]
fn nothing_can_be_made_in_a_folder_that_can_only_be_read() {
    let mut t = tree();
    t.read_only("/Home/Documents");
    let mut h = harness_with(Fake { tree: t, ..fake() }, 0, 1);
    pick(&mut h, "Documents");
    click_button(&mut h, "Open");
    click_button(&mut h, "New file");
    answer(&mut h, "x.txt");
    assert_eq!(dialog_text(&h).1, ["Couldn't make the file. Nothing can be changed here."]);
}

// ---- Rename. ----

#[test]
fn rename_asks_in_1_3s_words_and_renames() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    click_button(&mut h, "Rename");
    let (title, labels) = dialog_text(&h);
    assert_eq!(title, "Rename");
    assert_eq!(labels, ["New name for \"readme.txt\":"]);
    assert_eq!(dialog_line(&h), "readme.txt");
    assert_eq!(dialog_buttons(&h), ["Cancel", "Rename"]);
    log(&mut h);
    answer(&mut h, "about.txt");
    assert!(!h.app().system().tree.exists("/Home/readme.txt"));
    assert_eq!(h.app_mut().system_mut().tree.stat("/Home/about.txt").unwrap().size, 1234);
    // The selection goes with the file, to where the new name sorts.
    assert_eq!(h.app().selected_name(), Some("about.txt"));
    assert_eq!(
        log(&mut h),
        ["renamed /Home/readme.txt to /Home/about.txt", "folder /Home (12 items)"]
    );
}

#[test]
fn rename_to_the_same_name_or_nothing_does_nothing() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    log(&mut h);
    click_button(&mut h, "Rename");
    h.key(Key::Enter);
    click_button(&mut h, "Rename");
    h.key(Key::Escape);
    click_button(&mut h, "Rename");
    answer(&mut h, "  ");
    assert!(h.dialog().is_none());
    assert!(h.app().system().tree.exists("/Home/readme.txt"));
    assert!(log(&mut h).is_empty());
}

#[test]
fn rename_to_a_taken_or_bad_name_says_so() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    click_button(&mut h, "Rename");
    answer(&mut h, "photo.jpg");
    assert_eq!(dialog_text(&h).1, ["Couldn't rename it. Something with that name already exists."]);
    assert_eq!(h.app_mut().system_mut().tree.stat("/Home/photo.jpg").unwrap().size, 2_500_000);
    h.key(Key::Enter);
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
    click_button(&mut h, "Rename");
    answer(&mut h, "a/b");
    assert!(dialog_text(&h).1[0].starts_with("Couldn't rename it. A name can't be empty"));
    assert!(h.app().system().tree.exists("/Home/readme.txt"));
}

#[test]
fn a_folder_is_renamed_with_what_is_in_it() {
    let mut h = harness();
    pick(&mut h, "Documents");
    click_button(&mut h, "Rename");
    answer(&mut h, "Papers");
    assert!(h.app().system().tree.exists("/Home/Papers/letter.txt"));
    assert!(!h.app().system().tree.exists("/Home/Documents"));
}

// ---- Delete. ----

#[test]
fn delete_asks_first_and_keep_it_deletes_nothing() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    click_button(&mut h, "Delete");
    let (title, labels) = dialog_text(&h);
    assert_eq!(title, "Delete");
    assert_eq!(labels, ["Delete \"readme.txt\"? This can't be undone."]);
    assert_eq!(dialog_buttons(&h), ["Delete", "Keep it"]);
    // Keep it, Enter (which chooses it) and Escape each keep the file.
    h.click_on("Keep it");
    assert!(h.dialog().is_none());
    click_button(&mut h, "Delete");
    h.key(Key::Enter);
    click_button(&mut h, "Delete");
    h.key(Key::Escape);
    click_button(&mut h, "Delete");
    h.close(h.dialog().unwrap());
    assert!(h.dialog().is_none());
    assert!(h.app().system().tree.exists("/Home/readme.txt"));
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
}

#[test]
fn delete_deletes_what_was_asked_about() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    click_button(&mut h, "Delete");
    log(&mut h);
    h.click_on("Delete");
    assert!(h.dialog().is_none());
    assert!(!h.app().system().tree.exists("/Home/readme.txt"));
    assert!(!h.app().names().contains(&"readme.txt"));
    assert_eq!(h.app().selected_name(), None);
    assert_eq!(log(&mut h), ["deleted /Home/readme.txt", "folder /Home (11 items)"]);
}

#[test]
fn deleting_a_folder_asks_in_1_3s_words_and_an_empty_one_goes() {
    let mut h = harness();
    pick(&mut h, "Work");
    click_button(&mut h, "Delete");
    assert_eq!(dialog_text(&h).1, ["Delete the folder \"Work\"? It has to be empty."]);
    h.click_on("Delete");
    assert!(!h.app().system().tree.exists("/Home/Work"));
}

#[test]
fn a_folder_with_things_in_it_stays_and_the_answer_says_why() {
    let mut h = harness();
    pick(&mut h, "Documents");
    click_button(&mut h, "Delete");
    h.click_on("Delete");
    assert_eq!(dialog_text(&h).1, ["Couldn't delete it. The folder isn't empty."]);
    assert!(h.app().system().tree.exists("/Home/Documents/letter.txt"));
    assert_eq!(h.app().selected_name(), Some("Documents"));
    assert_eq!(log(&mut h).last().unwrap(), "couldn't delete it: The folder isn't empty");
}

// ---- Double-click, Enter and the keys. ----

#[test]
fn a_double_click_opens_a_folder_in_the_window() {
    let mut h = harness();
    double_click_on(&mut h, "Documents");
    assert_eq!(h.app().folder(), "/Home/Documents");
    assert!(launched(&h).is_empty());
}

#[test]
fn enter_opens_the_selected_item() {
    let mut h = harness();
    pick(&mut h, "Pictures");
    h.key(Key::Enter);
    assert_eq!(h.app().folder(), "/Home/Pictures");
    pick(&mut h, "rock.bmp");
    h.key(Key::Enter);
    assert_eq!(launched(&h), [("paint", "/Home/Pictures/rock.bmp")]);
}

#[test]
fn the_arrow_keys_move_the_selection() {
    let mut h = harness();
    pick(&mut h, "Documents");
    h.key(Key::Down);
    assert_eq!(h.app().selected_name(), Some("Downloads"));
    h.key(Key::Up);
    assert_eq!(h.app().selected_name(), Some("Documents"));
}

#[test]
fn delete_f2_f5_and_backspace_do_what_they_do_in_1_3_and_more() {
    let mut h = harness();
    // Delete and F2 ask about the selected item, and do nothing with none.
    h.key(Key::Delete);
    assert!(h.dialog().is_none());
    h.key(Key::Function(2));
    assert!(h.dialog().is_none());
    pick(&mut h, "readme.txt");
    h.key(Key::Delete);
    assert_eq!(dialog_text(&h).0, "Delete");
    h.key(Key::Escape);
    h.key(Key::Function(2));
    assert_eq!(dialog_text(&h).0, "Rename");
    h.key(Key::Escape);
    // F5 lists again, noticing what changed behind the window's back.
    h.app_mut().system_mut().tree.add_file("/Home/new.txt", 1, 1);
    assert!(!h.app().names().contains(&"new.txt"));
    h.key(Key::Function(5));
    assert!(h.app().names().contains(&"new.txt"));
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
    h.key(Key::Backspace);
    assert_eq!(h.app().folder(), "/");
}

// ---- Which app opens a file. ----

#[test]
fn a_file_opens_in_the_app_that_lists_its_extension() {
    let mut h = harness();
    log(&mut h);
    double_click_on(&mut h, "readme.txt");
    double_click_on(&mut h, "budget.md");
    assert_eq!(launched(&h), [("notepad", "/Home/readme.txt"), ("notepad", "/Home/budget.md")]);
    pick(&mut h, "Pictures");
    h.key(Key::Enter);
    double_click_on(&mut h, "rock.bmp");
    assert_eq!(launched(&h)[2], ("paint", "/Home/Pictures/rock.bmp"));
    assert_eq!(
        log(&mut h)[..2],
        [
            "opening /Home/readme.txt with notepad".to_string(),
            "opened /Home/readme.txt with notepad as app 11".to_string()
        ]
    );
}

#[test]
fn the_extension_is_matched_without_regard_to_case() {
    let mut t = tree();
    t.add_file("/Home/Pictures/SHOUT.BMP", 5, 5);
    let mut h = harness_with(Fake { tree: t, ..fake() }, 0, 1);
    pick(&mut h, "Pictures");
    h.key(Key::Enter);
    double_click_on(&mut h, "SHOUT.BMP");
    assert_eq!(launched(&h), [("paint", "/Home/Pictures/SHOUT.BMP")]);
}

#[test]
fn a_file_no_app_opens_goes_to_notepad_as_in_1_3() {
    let mut h = harness();
    double_click_on(&mut h, "photo.jpg");
    double_click_on(&mut h, "old.zip");
    assert_eq!(launched(&h), [("notepad", "/Home/photo.jpg"), ("notepad", "/Home/old.zip")]);
}

#[test]
fn without_the_sandbox_managers_list_everything_still_goes_to_notepad() {
    let mut h = harness_with(Fake { openers: Vec::new(), ..fake() }, 0, 1);
    double_click_on(&mut h, "readme.txt");
    pick(&mut h, "Pictures");
    h.key(Key::Enter);
    double_click_on(&mut h, "rock.bmp");
    assert_eq!(
        launched(&h),
        [("notepad", "/Home/readme.txt"), ("notepad", "/Home/Pictures/rock.bmp")]
    );
    // It was asked again each time, since it had said nothing.
    assert_eq!(h.app().system().asked, 2);
}

#[test]
fn the_sandbox_manager_is_asked_once_when_it_answers() {
    let mut h = harness();
    assert_eq!(h.app().system().asked, 0);
    double_click_on(&mut h, "readme.txt");
    double_click_on(&mut h, "photo.jpg");
    assert_eq!(h.app().system().asked, 1);
}

#[test]
fn an_app_that_wont_start_is_reported() {
    let mut h =
        harness_with(Fake { refuse: Some("Open with isn't built yet".into()), ..fake() }, 0, 1);
    log(&mut h);
    double_click_on(&mut h, "readme.txt");
    assert_eq!(dialog_text(&h).0, "Files");
    assert_eq!(dialog_text(&h).1, ["Couldn't open readme.txt. Open with isn't built yet."]);
    assert_eq!(
        log(&mut h),
        [
            "opening /Home/readme.txt with notepad",
            "couldn't open /Home/readme.txt with notepad: Open with isn't built yet"
        ]
    );
}

// ---- Open with…. ----

/// Chooses Open with… for `name` from its right-click menu.
fn open_with_menu(h: &mut Test, name: &str) {
    right_click_on(h, name);
    assert!(h.popup().is_some(), "the menu opened");
    h.click_on("Open with…");
}

fn with_window(h: &Test) -> WindowId {
    h.windows()
        .into_iter()
        .find(|id| *id != WindowId::MAIN && h.window(*id).is_some_and(|w| w.title == "Open with"))
        .expect("the Open with window")
}

#[test]
fn open_with_lists_the_apps_that_open_the_kind_first_then_the_rest_by_title() {
    let mut h = harness();
    open_with_menu(&mut h, "readme.txt");
    assert_eq!(h.app().with_titles(), ["Notepad", "About", "Calculator", "Paint", "Snake"]);
    let w = with_window(&h);
    assert!(h.window(w).unwrap().flags.contains(pbr_window::WindowFlags::MODAL));
    assert_eq!(widget(&h, "apps").1.what.label, "App|Opens");
    // The first is picked, and says why it is first.
    assert_eq!(widget(&h, "apps").1.what.value, "Notepad|.txt files");
    h.key(Key::Escape);
    assert!(h.app().with_titles().is_empty());
    // A picture: Paint first. Kinds nobody opens: all by title.
    pick(&mut h, "Pictures");
    h.key(Key::Enter);
    open_with_menu(&mut h, "rock.bmp");
    assert_eq!(h.app().with_titles(), ["Paint", "About", "Calculator", "Notepad", "Snake"]);
    h.key(Key::Escape);
    click_button(&mut h, "Up");
    open_with_menu(&mut h, "photo.jpg");
    assert_eq!(h.app().with_titles(), ["About", "Calculator", "Notepad", "Paint", "Snake"]);
    assert_eq!(widget(&h, "apps").1.what.value, "About|");
}

#[test]
fn open_with_launches_the_app_picked() {
    let mut h = harness();
    log(&mut h);
    open_with_menu(&mut h, "readme.txt");
    // Pick Paint, the fourth, by clicking its row, then Open.
    let (x, y) = row_at(&h, "apps", 3);
    h.click_in(with_window(&h), x, y);
    assert_eq!(widget(&h, "apps").1.what.value, "Paint|");
    h.click_on("withopen");
    assert!(h.app().with_titles().is_empty());
    assert_eq!(launched(&h), [("paint", "/Home/readme.txt")]);
    assert_eq!(
        log(&mut h),
        [
            "opening /Home/readme.txt with paint".to_string(),
            "opened /Home/readme.txt with paint as app 11".to_string()
        ]
    );
    assert!(h.windows().iter().all(|id| *id == WindowId::MAIN || h.popup() == Some(*id)));
}

#[test]
fn open_with_launches_on_a_double_click_and_on_enter() {
    let mut h = harness();
    open_with_menu(&mut h, "photo.jpg");
    let w = with_window(&h);
    let (x, y) = row_at(&h, "apps", 4);
    h.click_in(w, x, y);
    h.advance(50);
    h.click_in(w, x, y);
    assert_eq!(launched(&h), [("snake", "/Home/photo.jpg")]);
    assert!(h.app().with_titles().is_empty());
    open_with_menu(&mut h, "photo.jpg");
    h.key(Key::Down);
    h.key(Key::Enter);
    assert_eq!(launched(&h)[1], ("calculator", "/Home/photo.jpg"));
}

#[test]
fn open_with_can_be_cancelled_three_ways() {
    let mut h = harness();
    open_with_menu(&mut h, "readme.txt");
    h.click_on("withcancel");
    assert!(h.app().with_titles().is_empty());
    open_with_menu(&mut h, "readme.txt");
    h.key(Key::Escape);
    assert!(h.app().with_titles().is_empty());
    open_with_menu(&mut h, "readme.txt");
    h.close(with_window(&h));
    assert!(h.app().with_titles().is_empty());
    assert!(launched(&h).is_empty());
    assert!(!h.finished());
    // The main window is as it was.
    assert_eq!(h.app().folder(), "/Home");
}

#[test]
fn open_with_is_for_files_not_folders() {
    let mut h = harness();
    right_click_on(&mut h, "Documents");
    let menu = h.popup().unwrap();
    let rows = lines(&h, menu);
    assert!(
        rows.iter().any(|l| l.contains("\"Open with…\"") && l.contains("disabled")),
        "{rows:?}"
    );
}

#[test]
fn open_with_without_the_sandbox_managers_list_says_so() {
    let mut h = harness_with(Fake { openers: Vec::new(), ..fake() }, 0, 1);
    open_with_menu(&mut h, "readme.txt");
    assert_eq!(dialog_text(&h).1, ["Couldn't find the apps to open it with."]);
    assert!(h.app().with_titles().is_empty());
}

// ---- The right-click menus. ----

#[test]
fn a_right_click_on_an_item_picks_it_and_opens_its_menu() {
    let mut h = harness();
    right_click_on(&mut h, "readme.txt");
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
    let menu = h.popup().expect("a menu");
    let rows = lines(&h, menu);
    for label in ["Open", "Open with…", "Rename", "Delete", "New file", "New folder"] {
        assert!(rows.iter().any(|l| l.contains(&format!("\"{label}\""))), "{label} in {rows:?}");
    }
    // Everything acts on the item: Rename asks about it.
    h.click_on("Rename");
    assert_eq!(dialog_text(&h).1, ["New name for \"readme.txt\":"]);
    h.key(Key::Escape);
    // Open opens it.
    right_click_on(&mut h, "readme.txt");
    h.click_on("Open");
    assert_eq!(launched(&h), [("notepad", "/Home/readme.txt")]);
    // Delete asks first.
    right_click_on(&mut h, "readme.txt");
    h.click_on("Delete");
    assert_eq!(dialog_text(&h).0, "Delete");
    h.key(Key::Escape);
    assert!(h.app().system().tree.exists("/Home/readme.txt"));
}

#[test]
fn a_right_click_on_empty_space_offers_new_file_and_new_folder() {
    let mut h = harness_with(
        Fake {
            tree: {
                let mut t = MemTree::new();
                t.add_file("/Home/a.txt", 1, 1);
                t
            },
            ..fake()
        },
        0,
        1,
    );
    pick(&mut h, "a.txt");
    // Below the one row there is empty room.
    let r = widget(&h, "files").1.rect;
    h.advance(1000);
    h.right_click_in(WindowId::MAIN, r.x + 40, r.y + r.height - 20);
    assert_eq!(h.app().selected_name(), None, "nothing is picked any more");
    let menu = h.popup().expect("a menu");
    let rows = lines(&h, menu);
    for label in ["Open", "Open with…", "Rename", "Delete"] {
        assert!(
            rows.iter().any(|l| l.contains(&format!("\"{label}\"")) && l.contains("disabled")),
            "{label} greyed out in {rows:?}"
        );
    }
    for label in ["New file", "New folder"] {
        assert!(
            rows.iter().any(|l| l.contains(&format!("\"{label}\"")) && !l.contains("disabled")),
            "{label} in {rows:?}"
        );
    }
    h.click_on("New folder");
    assert_eq!(dialog_text(&h).0, "New folder");
    answer(&mut h, "Fresh");
    assert!(h.app().system().tree.exists("/Home/Fresh"));
}

#[test]
fn the_menu_in_an_empty_folder_still_works() {
    let mut h = harness();
    pick(&mut h, "Work");
    h.key(Key::Enter);
    assert!(h.app().names().is_empty());
    let r = widget(&h, "files").1.rect;
    h.advance(1000);
    h.right_click_in(WindowId::MAIN, r.x + 40, r.y + 60);
    assert!(h.popup().is_some());
    h.click_on("New file");
    assert_eq!(dialog_text(&h).0, "New file");
}

// ---- Looking again. ----

#[test]
fn it_notices_a_file_made_behind_its_back_every_two_seconds() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    log(&mut h);
    h.app_mut().system_mut().tree.add_file("/Home/arrived.txt", 3, 3);
    // The clock is at 1000 ms (the click's wait); the timer first goes off at 2000.
    h.advance(500);
    assert!(!h.app().names().contains(&"arrived.txt"));
    h.advance(600);
    assert!(h.app().names().contains(&"arrived.txt"));
    // The selection stayed on the same name though its row moved.
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
    assert_eq!(log(&mut h), ["folder /Home (13 items)"]);
    // Nothing changed: nothing is logged, and it keeps looking.
    h.advance(2000);
    assert!(log(&mut h).is_empty());
    h.app_mut().system_mut().tree.remove("/Home/arrived.txt").unwrap();
    h.advance(2000);
    assert!(!h.app().names().contains(&"arrived.txt"));
    assert_eq!(h.app().selected_name(), Some("readme.txt"));
}

#[test]
fn a_selection_whose_file_went_is_dropped() {
    let mut h = harness();
    pick(&mut h, "readme.txt");
    h.app_mut().system_mut().tree.remove("/Home/readme.txt").unwrap();
    h.advance(2000);
    assert_eq!(h.app().selected_name(), None);
}

#[test]
fn a_folder_that_went_leaves_for_the_one_above() {
    let mut h = harness();
    double_click_on(&mut h, "Work");
    assert_eq!(h.app().folder(), "/Home/Work");
    h.app_mut().system_mut().tree.remove("/Home/Work").unwrap();
    h.advance(2000);
    assert_eq!(h.app().folder(), "/Home");
    assert!(h.app().problem().is_none());
    assert_eq!(h.app().title(), "/Home");
}

#[test]
fn a_change_in_a_file_is_noticed_too() {
    let mut h = harness();
    log(&mut h);
    h.app_mut().system_mut().tree.add_file("/Home/readme.txt", 9999, 1_700_100_001);
    h.advance(2000);
    assert_eq!(log(&mut h), ["folder /Home (12 items)"]);
}

// ---- Time zones. ----

#[test]
fn dates_are_shown_in_the_users_time_zone() {
    // 1_700_000_000 is 22:13 on 14 November 2023 in UTC.
    for (offset, want) in [
        (0, "14 Nov 2023 22:13"),
        (120, "15 Nov 2023 00:13"),
        (-480, "14 Nov 2023 14:13"),
        (330, "15 Nov 2023 03:43"),
    ] {
        let mut h = harness_with(fake(), offset, 1);
        pick(&mut h, "Documents");
        h.key(Key::Enter);
        pick(&mut h, "letter.txt");
        assert_eq!(widget(&h, "files").1.what.value, format!("letter.txt|120 bytes|{want}"));
    }
}

// ---- Starting. ----

#[test]
fn it_can_start_in_a_folder_or_at_a_file() {
    let h = FileManager::new(fake(), 0).starting_at("/Home/Pictures");
    assert_eq!(h.folder(), "/Home/Pictures");
    let h = FileManager::new(fake(), 0).starting_at("/Home/Documents/letter.txt");
    assert_eq!(h.folder(), "/Home/Documents");
    assert_eq!(h.selected_name(), Some("letter.txt"));
    let mut m = FileManager::new(fake(), 0).starting_at("/Nowhere");
    assert_eq!(m.folder(), "/Home");
    assert_eq!(m.problem(), Some("Not found"));
    assert!(m.take_log().iter().any(|l| l == "couldn't show /Nowhere: Not found"));
}

#[test]
fn the_folder_is_logged_when_it_changes() {
    let mut h = harness();
    // Made by `new`, before the harness took the log.
    assert_eq!(log(&mut h), ["folder /Home (12 items)"]);
    pick(&mut h, "Documents");
    h.key(Key::Enter);
    assert_eq!(log(&mut h), ["folder /Home/Documents (1 items)"]);
    click_button(&mut h, "Up");
    assert_eq!(log(&mut h), ["folder /Home (12 items)"]);
    // Sorting differently lists the same things: nothing new to say.
    let r = widget(&h, "files").1.rect;
    h.click(r.x + r.width - DATE_WIDTH / 2, r.y + 10);
    assert!(log(&mut h).is_empty());
}

#[test]
fn the_status_line_counts_the_items() {
    let mut h = harness();
    let status = |h: &Test| -> String {
        h.describe(WindowId::MAIN)
            .iter()
            .find(|w| w.what.role == "status bar")
            .map(|w| w.what.label.clone())
            .unwrap_or_default()
    };
    assert!(status(&h).contains("12 items"), "{}", status(&h));
    pick(&mut h, "Documents");
    h.key(Key::Enter);
    assert!(status(&h).contains("1 item") && !status(&h).contains("1 items"));
}

// ---- Odd input. ----

#[test]
fn odd_names_odd_sizes_and_odd_clicks_do_not_panic() {
    let mut t = tree();
    t.add_file(&format!("/Home/{}", "w".repeat(250)), u64::MAX, u64::MAX)
        .add_file("/Home/é ü ñ 日本語.txt", 1, 1)
        .add_file("/Home/a\u{202e}b.txt", 1, 1)
        .add_file("/Home/.", 1, 1);
    for (size, factor) in [((400, 220), 1), ((400, 220), 2), ((900, 700), 1), ((40, 30), 1)] {
        let mut h = Harness::with_scale(
            FileManager::new(Fake { tree: t.clone(), ..fake() }, 840),
            settings(),
            factor,
        );
        h.resize(size.0, size.1);
        for (x, y) in [(0, 0), (5, 5), (100, 100), (-5, -5), (100_000, 3), (200, 150)] {
            h.click(x, y);
            h.right_click_in(WindowId::MAIN, x, y);
            h.key(Key::Escape);
            h.double_click(x, y);
        }
        for k in [Key::Down, Key::Up, Key::Enter, Key::Backspace, Key::Delete, Key::Function(2)] {
            h.key(k);
            if h.dialog().is_some() {
                h.key(Key::Escape);
            }
        }
    }
}

#[test]
fn the_window_works_at_its_least_size() {
    for factor in [1, 2] {
        let mut h = harness_with(fake(), 0, factor);
        h.resize(MIN_SIZE.0, MIN_SIZE.1);
        pick(&mut h, "readme.txt");
        // Every toolbar button is within the window.
        for label in ["Up", "Open", "New file", "New folder", "Rename", "Delete"] {
            let r = button(&h, label).rect;
            assert!(r.x >= 0 && r.x + r.width <= MIN_SIZE.0 as i32, "{label} at {r:?}");
        }
        // The list has room for its columns.
        let list = widget(&h, "files").1.rect;
        assert!(list.width > SIZE_WIDTH + DATE_WIDTH + 60, "{list:?}");
    }
}

// ---- The look. ----

fn named(h: &mut Test, name: &str) {
    pick(h, name);
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "files-1x"), (2, "files-2x")] {
        let h = harness_with(fake(), 120, factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    // With a file picked, and the folder's own place chosen.
    let mut h = harness_with(fake(), 120, 1);
    named(&mut h, "readme.txt");
    h.check_window_picture_in(WindowId::MAIN, &dir(), "files-picked-1x");
    // The right-click menu.
    right_click_on(&mut h, "photo.jpg");
    let menu = h.popup().unwrap();
    h.check_window_picture_in(menu, &dir(), "files-menu-1x");
    h.key(Key::Escape);
    // The question before Delete.
    named(&mut h, "readme.txt");
    click_button(&mut h, "Delete");
    let d = h.dialog().unwrap();
    h.check_window_picture_in(d, &dir(), "files-delete-1x");
    h.key(Key::Escape);
    // Open with….
    open_with_menu(&mut h, "readme.txt");
    let w = with_window(&h);
    h.check_window_picture_in(w, &dir(), "files-open-with-1x");
    h.key(Key::Escape);
    // The smallest window, in a deep folder, at 2x.
    let mut t = tree();
    t.add_folder("/Home/Work/Projects/Rock/Docs");
    let files = FileManager::new(Fake { tree: t, ..fake() }, 120)
        .starting_at("/Home/Work/Projects/Rock/Docs");
    let mut h = Harness::with_scale(files, settings(), 2);
    h.resize(MIN_SIZE.0, MIN_SIZE.1);
    h.check_window_picture_in(WindowId::MAIN, &dir(), "files-small-deep-2x");
}
