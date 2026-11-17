// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Notepad on the host: typing and the editing keys, the menus and their keys, saving and
//! opening through a pretend disk and file picker (UTF-8, Latin-1 and Windows line endings, files
//! that are refused), the file given at start, the clipboard, undo and redo, the "•" in the
//! title, asking before work is lost, and the look (`tests/pictures/notepad/`).

use super::*;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Key, Modifiers};
use pbr_window::Event;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

/// The menu bar's height: the editor starts below it.
const MENU: i32 = 24;
/// The middle of the File title and of the Edit title in the menu bar.
const FILE: (i32, i32) = (24, 12);
const EDIT: (i32, i32) = (64, 12);
const LETTER: &str = "/Home/Documents/letter.txt";

/// A pretend disk and file picker: files in a map, the picker's requests written down and its
/// answers given by the test.
#[derive(Default)]
struct Fake {
    files: BTreeMap<String, Vec<u8>>,
    /// What the picker was asked: the window's number, the serial, and for what.
    asked: Vec<(Option<Win>, u32, Pick)>,
    answers: VecDeque<PickAnswer>,
    /// The paths written to, in order.
    writes: Vec<String>,
    /// There is no picker.
    no_picker: bool,
    /// Every write fails.
    full: bool,
}

impl Fake {
    fn with(path: &str, data: &[u8]) -> Fake {
        let mut fake = Fake::default();
        fake.files.insert(path.into(), data.to_vec());
        fake
    }
}

impl Disk for Fake {
    fn read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, DiskError> {
        let data = self.files.get(path).ok_or(DiskError::NotFound)?;
        if data.len() > max { Err(DiskError::TooBig) } else { Ok(data.clone()) }
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), DiskError> {
        if self.full {
            return Err(DiskError::Failed("the disk is full".into()));
        }
        self.writes.push(path.into());
        self.files.insert(path.into(), data.to_vec());
        Ok(())
    }

    fn pick(&mut self, window: Option<Win>, serial: u32, pick: Pick) -> Result<(), String> {
        if self.no_picker {
            return Err("Notepad has no file picker".into());
        }
        self.asked.push((window, serial, pick));
        Ok(())
    }

    fn answer(&mut self) -> Option<PickAnswer> {
        self.answers.pop_front()
    }
}

type Test = Harness<Notepad<Fake>>;

fn harness_scaled(fake: Fake, factor: u32) -> Test {
    Harness::with_scale(Notepad::new(fake), settings(), factor)
}

fn harness() -> Test {
    harness_scaled(Fake::default(), 1)
}

/// Notepad started with a file to open, as Files' "Open with…" does.
fn harness_with_file(fake: Fake, path: &str) -> Test {
    Harness::new(Notepad::new(fake).with_file(path), settings())
}

/// The picker answers: the answer is given to the app, and the notification the program would
/// send for it comes.
fn answer(h: &mut Test, a: PickAnswer) {
    h.app_mut().disk_mut().answers.push_back(a);
    h.send(Event::Notified(1 << PICKED));
}

/// Opens a menu with the pointer and clicks one of its rows.
fn menu(h: &mut Test, title: (i32, i32), row: &str) {
    h.click(title.0, title.1);
    assert!(h.popup().is_some(), "the menu opened");
    h.click_on(row);
}

fn ctrl(c: char) -> Shortcut {
    Shortcut::ctrl(c)
}

/// The text of the editor.
fn text(h: &Test) -> String {
    h.app().content().text()
}

/// The text of the dialog on show, if there is one.
fn dialog_text(h: &Test) -> Option<String> {
    let id = h.dialog()?;
    // Labels that wrap give a line each; join them back into words.
    let words: Vec<String> = h
        .describe(id)
        .iter()
        .filter(|d| d.what.role == "label")
        .map(|d| d.what.label.clone())
        .collect();
    Some(words.join(" "))
}

/// The buttons of the dialog on show, left to right.
fn dialog_buttons(h: &Test) -> Vec<String> {
    let id = h.dialog().expect("a dialog");
    h.describe(id)
        .iter()
        .filter(|d| d.what.role == "button")
        .map(|d| d.what.label.clone())
        .collect()
}

/// The rows of the menu that is open: label, shortcut and whether it can be chosen.
fn rows(h: &Test) -> Vec<(String, String, bool)> {
    h.describe(h.popup().expect("a menu"))
        .into_iter()
        .map(|d| (d.what.label, d.what.value, d.what.enabled))
        .collect()
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/notepad")
}

/// The status line's texts: its description is the label of the bar.
fn status_bar(h: &Test) -> String {
    h.describe(WindowId::MAIN)
        .into_iter()
        .find(|d| d.what.role == "status bar")
        .map(|d| d.what.label)
        .unwrap_or_default()
}

/// The pointer's click on line `line` (counted from 0) of an editor that isn't scrolled, at
/// `x` pixels from the editor's left edge.
fn click_line(h: &mut Test, line: i32, x: i32) {
    let r = h.find(EDITOR_ID).unwrap().1.rect;
    h.click(r.x + x, r.y + 8 + 12 * line);
}

// ---- Starting. ----

#[test]
fn it_opens_empty_with_the_editor_ready_for_typing() {
    let h = harness();
    assert_eq!(h.app().title(), "Untitled");
    assert_eq!(text(&h), "");
    assert!(!h.app().is_dirty());
    let focused = h.focused().expect("something has the keyboard");
    assert_eq!(focused.key.as_deref(), Some(EDITOR_ID));
    let editor = h.find(EDITOR_ID).unwrap().1.rect;
    assert_eq!((editor.x, editor.y, editor.width), (0, MENU, 480));
    // Only the cursor's place, 1.3's "Ln 1, Col 1", and no message.
    assert_eq!(status_bar(&h), "Ln 1, Col 1");
    assert_eq!(h.app().status(), "");
    assert_eq!(settings().size, (480, 320));
    assert!(settings().size.0 <= 512 && settings().size.1 + 24 <= 384, "fits a small screen");
    assert!(settings().resizable);
}

// ---- Typing and 1.3's editing keys. ----

#[test]
fn typing_makes_text_the_title_gets_a_bullet_and_the_cursor_is_shown() {
    let mut h = harness();
    h.type_text("Hello, world");
    assert_eq!(text(&h), "Hello, world");
    assert_eq!(h.app().title(), "• Untitled");
    assert!(h.app().is_dirty());
    assert!(status_bar(&h).contains("Ln 1, Col 13"), "{}", status_bar(&h));
    h.key(Key::Enter);
    h.type_text("second");
    assert_eq!(text(&h), "Hello, world\nsecond");
    assert!(status_bar(&h).contains("Ln 2, Col 7"));
}

#[test]
fn the_arrow_keys_home_end_and_the_page_keys_move_the_cursor() {
    let mut h = harness();
    h.type_text("one\ntwo three\nfour");
    assert_eq!(h.app().content().cursor(), (2, 4));
    h.key(Key::Up);
    assert_eq!(h.app().content().cursor(), (1, 4));
    h.key(Key::Home);
    assert_eq!(h.app().content().cursor(), (1, 0));
    h.key(Key::End);
    assert_eq!(h.app().content().cursor(), (1, 9));
    // Up from a long line to a shorter one keeps to the end of it; the wanted column comes back.
    h.key(Key::Up);
    assert_eq!(h.app().content().cursor(), (0, 3));
    h.key(Key::Down);
    assert_eq!(h.app().content().cursor(), (1, 9));
    h.key(Key::Left);
    h.key(Key::Left);
    assert_eq!(h.app().content().cursor(), (1, 7));
    h.key(Key::Right);
    assert_eq!(h.app().content().cursor(), (1, 8));
    // Left at the start of a line goes to the end of the one above; Right at the end, down.
    h.key(Key::Home);
    h.key(Key::Left);
    assert_eq!(h.app().content().cursor(), (0, 3));
    h.key(Key::Right);
    assert_eq!(h.app().content().cursor(), (1, 0));
    // Ctrl+Home and Ctrl+End: the start and the end of the text.
    h.key_with(Key::End, Modifiers(Modifiers::CTRL));
    assert_eq!(h.app().content().cursor(), (2, 4));
    h.key_with(Key::Home, Modifiers(Modifiers::CTRL));
    assert_eq!(h.app().content().cursor(), (0, 0));
    // Moving changes nothing in the text.
    assert!(h.app().is_dirty(), "typed, not yet saved");
    assert_eq!(text(&h), "one\ntwo three\nfour");
}

#[test]
fn page_up_and_down_move_by_a_window_of_lines() {
    let mut h = harness();
    let lines: Vec<String> = (1..=100).map(|n| format!("line {n}")).collect();
    h.app_mut().content_mut().perform(Action::InsertText(lines.join("\n")));
    h.refresh();
    h.key_with(Key::Home, Modifiers(Modifiers::CTRL));
    h.key(Key::PageDown);
    let (line, _) = h.app().content().cursor();
    // 1.3 moved by `rows`, the lines in view: a little under 20 here.
    assert!((10..=25).contains(&line), "{line}");
    h.key(Key::PageUp);
    assert_eq!(h.app().content().cursor().0, 0);
}

#[test]
fn backspace_delete_enter_and_tab_edit_the_text() {
    let mut h = harness();
    h.type_text("abcd");
    h.key(Key::Backspace);
    assert_eq!(text(&h), "abc");
    h.key(Key::Left);
    h.key(Key::Delete);
    assert_eq!(text(&h), "ab");
    h.key(Key::Enter);
    h.type_text("x");
    assert_eq!(text(&h), "ab\nx");
    // Backspace at the start of a line joins it to the one above.
    h.key(Key::Home);
    h.key(Key::Backspace);
    assert_eq!(text(&h), "abx");
    // Tab types a tab (the editor shows it as spaces up to the next multiple of four).
    h.key(Key::Home);
    h.key(Key::Tab);
    assert_eq!(text(&h), "\tabx");
    // Backspace and Delete at the ends do nothing.
    h.key_with(Key::Home, Modifiers(Modifiers::CTRL));
    h.key(Key::Left);
    h.key(Key::Backspace);
    h.key_with(Key::End, Modifiers(Modifiers::CTRL));
    h.key(Key::Delete);
    assert_eq!(text(&h), "\tabx");
}

#[test]
fn clicking_places_the_cursor() {
    let mut h = harness();
    h.type_text("alpha\nbeta\ngamma");
    // Far to the right on the second line: its end.
    click_line(&mut h, 1, 300);
    assert_eq!(h.app().content().cursor(), (1, 4));
    // At the left of the first line: its start (the margin's click goes to column 0).
    click_line(&mut h, 0, 2);
    assert_eq!(h.app().content().cursor(), (0, 0));
    // Below the last line: the last line's end.
    let r = h.find(EDITOR_ID).unwrap().1.rect;
    h.click(r.x + 300, r.y + r.height - 20);
    assert_eq!(h.app().content().cursor(), (2, 5));
    // Typing then goes there.
    h.type_text("!");
    assert_eq!(text(&h), "alpha\nbeta\ngamma!");
}

// ---- The menus. ----

#[test]
fn the_menus_show_their_shortcuts_and_grey_out_what_cannot_be_done() {
    let mut h = harness();
    h.click(FILE.0, FILE.1);
    let want = [
        ("New", "Ctrl+N"),
        ("Open…", "Ctrl+O"),
        ("Save", "Ctrl+S"),
        ("Save as…", "Ctrl+Shift+S"),
        ("Close", "Ctrl+W"),
    ];
    let file: Vec<(String, String)> =
        rows(&h).into_iter().filter(|r| !r.0.is_empty()).map(|r| (r.0, r.1)).collect();
    assert_eq!(file, want.map(|(a, b)| (a.to_string(), b.to_string())));
    assert!(rows(&h).iter().all(|r| r.2), "every File row can be chosen");
    h.key(Key::Escape);
    h.click(EDIT.0, EDIT.1);
    let edit = rows(&h);
    let labels: Vec<(&str, &str, bool)> =
        edit.iter().filter(|r| !r.0.is_empty()).map(|r| (&*r.0, &*r.1, r.2)).collect();
    // Nothing typed, nothing selected, nothing copied: only Select all can be done.
    assert_eq!(
        labels,
        [
            ("Undo", "Ctrl+Z", false),
            ("Redo", "Ctrl+Y", false),
            ("Cut", "Ctrl+X", false),
            ("Copy", "Ctrl+C", false),
            ("Paste", "Ctrl+V", false),
            ("Select all", "Ctrl+A", true),
        ]
    );
}

#[test]
fn edit_rows_come_alive_with_what_they_can_do() {
    let mut h = harness();
    h.type_text("some words");
    h.shortcut(ctrl('a'));
    h.set_clipboard_text("from elsewhere");
    h.click(EDIT.0, EDIT.1);
    let edit = rows(&h);
    let enabled: Vec<&str> =
        edit.iter().filter(|r| r.2 && !r.0.is_empty()).map(|r| &*r.0).collect();
    assert_eq!(enabled, ["Undo", "Cut", "Copy", "Paste", "Select all"]);
    h.key(Key::Escape);
}

#[test]
fn file_new_and_the_ctrl_keys_work_with_the_menus_shut() {
    let mut h = harness();
    h.shortcut(ctrl('o'));
    assert_eq!(h.app().disk().asked.len(), 1);
    assert_eq!(h.app().disk().asked[0].2, Pick::Open);
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().asked.len(), 1, "one picker at a time");
    answer(&mut h, PickAnswer::Cancelled);
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().asked[1].2, Pick::Save { name: "Untitled.txt".into() });
    answer(&mut h, PickAnswer::Cancelled);
    h.type_text("abc");
    h.advance(200);
    h.shortcut(Shortcut::ctrl_shift('s'));
    assert_eq!(h.app().disk().asked.len(), 3);
}

// ---- Saving. ----

#[test]
fn save_with_no_file_asks_the_picker_and_writes_the_bytes() {
    let mut h = harness();
    h.type_text("Dear Chris,\nHello.");
    menu(&mut h, FILE, "Save");
    let asked = &h.app().disk().asked;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2, Pick::Save { name: "Untitled.txt".into() });
    assert!(asked[0].0.is_some() && asked[0].1 != 0, "with the window and the press");
    // Nothing is written until the user has chosen.
    assert!(h.app().disk().writes.is_empty());
    assert!(h.app().is_dirty());
    h.app_mut().take_log();
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(h.app().disk().files[LETTER], b"Dear Chris,\nHello.");
    assert_eq!(h.app().path(), Some(LETTER));
    assert_eq!(h.app().title(), "letter.txt");
    assert!(!h.app().is_dirty());
    let crc = pbr_crc::crc32(b"Dear Chris,\nHello.");
    assert_eq!(h.app_mut().take_log(), [format!("saved {LETTER} (18 bytes, crc {crc:08x})")]);
    assert!(status_bar(&h).contains("Saved"));
    // The message goes away after a few seconds.
    h.advance(3500);
    assert!(!status_bar(&h).contains("Saved"));
}

#[test]
fn save_again_writes_without_asking_and_save_as_always_asks() {
    let mut h = harness();
    h.type_text("one");
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    h.type_text(" two");
    assert_eq!(h.app().title(), "• letter.txt");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().asked.len(), 1, "not asked again");
    assert_eq!(h.app().disk().files[LETTER], b"one two");
    assert_eq!(h.app().disk().writes, [LETTER, LETTER]);
    assert_eq!(h.app().title(), "letter.txt");
    // Save as offers the file's own name, and writes where the user says; the old file stays.
    menu(&mut h, FILE, "Save as…");
    assert_eq!(h.app().disk().asked[1].2, Pick::Save { name: "letter.txt".into() });
    answer(&mut h, PickAnswer::Chosen("/Volumes/usb/copy.md".into()));
    assert_eq!(h.app().path(), Some("/Volumes/usb/copy.md"));
    assert_eq!(h.app().disk().files["/Volumes/usb/copy.md"], b"one two");
    assert_eq!(h.app().disk().files[LETTER], b"one two");
    assert_eq!(h.app().title(), "copy.md");
    // A cancelled picker changes nothing.
    h.type_text("!");
    h.shortcut(Shortcut::ctrl_shift('s'));
    answer(&mut h, PickAnswer::Cancelled);
    assert_eq!(h.app().path(), Some("/Volumes/usb/copy.md"));
    assert!(h.app().is_dirty());
    assert!(h.dialog().is_none());
}

#[test]
fn saving_nothing_makes_an_empty_file() {
    let mut h = harness();
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(h.app().disk().files[LETTER], b"");
}

#[test]
fn a_save_that_fails_says_so_and_keeps_the_text_unsaved() {
    let mut h = harness_scaled(Fake { full: true, ..Fake::default() }, 1);
    h.type_text("precious");
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(dialog_text(&h).unwrap(), "Couldn't save letter.txt. the disk is full.");
    assert!(h.app().is_dirty());
    assert_eq!(h.app().path(), None);
    assert_eq!(
        h.app_mut().take_log().last().unwrap(),
        &format!("couldn't save {LETTER}: the disk is full")
    );
    h.click_on("OK");
    assert!(h.dialog().is_none());
    assert_eq!(text(&h), "precious");
}

#[test]
fn a_picker_that_is_missing_or_fails_says_so() {
    let mut h = harness_scaled(Fake { no_picker: true, ..Fake::default() }, 1);
    h.shortcut(ctrl('o'));
    assert_eq!(
        dialog_text(&h).unwrap(),
        "Couldn't open the file picker. Notepad has no file picker."
    );
    h.click_on("OK");
    h.type_text("a");
    h.shortcut(ctrl('s'));
    assert_eq!(dialog_text(&h).unwrap(), "Couldn't save the text. Notepad has no file picker.");
    h.click_on("OK");
    let mut h = harness();
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Failed("the picker has gone".into()));
    assert_eq!(dialog_text(&h).unwrap(), "Couldn't open the file picker. the picker has gone.");
}

// ---- Opening. ----

#[test]
fn opening_a_file_in_documents_shows_it() {
    let mut h = harness_scaled(Fake::with(LETTER, b"Hello\nthere\n"), 1);
    h.shortcut(ctrl('o'));
    assert_eq!(h.app().disk().asked[0].2, Pick::Open);
    h.app_mut().take_log();
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(text(&h), "Hello\nthere\n");
    assert_eq!(h.app().path(), Some(LETTER));
    assert_eq!(h.app().title(), "letter.txt");
    assert!(!h.app().is_dirty());
    let crc = pbr_crc::crc32(b"Hello\nthere\n");
    assert_eq!(h.app_mut().take_log(), [format!("opened {LETTER} (12 bytes, crc {crc:08x})")]);
    // Typing goes on at the start, and the file is saved back where it came from.
    h.type_text(">");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().files[LETTER], b">Hello\nthere\n");
    assert_eq!(h.app().disk().asked.len(), 1);
}

#[test]
fn opening_a_file_the_picker_mounted_elsewhere_works_the_same() {
    let path = "/Volumes/usb/notes/todo.md";
    let mut h = harness_scaled(Fake::with(path, b"# To do\n"), 1);
    menu(&mut h, FILE, "Open…");
    answer(&mut h, PickAnswer::Chosen(path.into()));
    assert_eq!(text(&h), "# To do\n");
    assert_eq!(h.app().title(), "todo.md");
    h.type_text("x");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().writes, [path]);
    assert_eq!(h.app().disk().files[path], b"x# To do\n");
}

#[test]
fn a_file_given_at_start_is_opened() {
    let mut h = harness_with_file(Fake::with(LETTER, b"hello"), LETTER);
    assert_eq!(text(&h), "hello");
    assert_eq!(h.app().title(), "letter.txt");
    assert!(!h.app().is_dirty());
    let crc = pbr_crc::crc32(b"hello");
    assert_eq!(h.app_mut().take_log(), [format!("opened {LETTER} (5 bytes, crc {crc:08x})")]);
    // A path with spaces in it.
    let spaced = "/Home/Documents/my letter.txt";
    let h = harness_with_file(Fake::with(spaced, b"x"), spaced);
    assert_eq!(h.app().title(), "my letter.txt");
}

#[test]
fn a_new_file_named_at_start_is_made_by_the_first_save() {
    let mut h = harness_with_file(Fake::default(), "/Home/Documents/new.txt");
    assert_eq!(text(&h), "");
    assert_eq!(h.app().path(), Some("/Home/Documents/new.txt"));
    assert_eq!(h.app().title(), "new.txt");
    assert!(!h.app().is_dirty());
    assert!(h.dialog().is_none());
    assert!(status_bar(&h).contains("New file"));
    h.type_text("made");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().files["/Home/Documents/new.txt"], b"made");
    assert!(h.app().disk().asked.is_empty());
}

#[test]
fn a_file_at_start_that_cannot_be_opened_says_so() {
    // Outside Documents, "not there" is a failure, not a new file.
    let h = harness_with_file(Fake::default(), "/Volumes/usb/x.txt");
    assert_eq!(dialog_text(&h).unwrap(), "Couldn't open x.txt. there is no such file.");
    assert_eq!(h.app().path(), None);
    assert_eq!(h.app().title(), "Untitled");
}

// ---- What a file is. ----

/// Opens `bytes` as the file, saves it unchanged (after a touch that is undone), and gives the
/// bytes written.
fn open_and_save(bytes: &[u8]) -> (Test, Vec<u8>) {
    let mut h = harness_with_file(Fake::with(LETTER, bytes), LETTER);
    // Type a character and delete it: the text is as it was, and Save writes it.
    h.type_text("x");
    h.key(Key::Backspace);
    h.shortcut(ctrl('s'));
    let written = h.app().disk().files[LETTER].clone();
    (h, written)
}

#[test]
fn utf8_files_round_trip() {
    let bytes = "héllo wörld\n日本語 😀\n".as_bytes();
    let (h, written) = open_and_save(bytes);
    assert_eq!(text(&h), "héllo wörld\n日本語 😀\n");
    assert_eq!(written, bytes);
    assert!(!status_bar(&h).contains("Latin"));
}

#[test]
fn a_file_that_is_not_utf8_is_read_as_latin_1_and_written_back_the_same() {
    // "café" with the é as the single byte E9, and a ÿ (FF).
    let bytes = b"caf\xe9 \xff\nok".to_vec();
    let h = harness_with_file(Fake::with(LETTER, &bytes), LETTER);
    assert_eq!(text(&h), "café ÿ\nok");
    assert!(h.dialog().is_none(), "not refused");
    assert!(status_bar(&h).contains("Latin-1"), "{}", status_bar(&h));
    assert!(!text(&h).contains('\u{fffd}'), "no replacement characters");
    let (_, written) = open_and_save(&bytes);
    assert_eq!(written, bytes);
    // Typing a character Latin-1 hasn't turns the file into UTF-8, and says so.
    let mut h = harness_with_file(Fake::with(LETTER, &bytes), LETTER);
    h.type_text("€");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().files[LETTER], "€café ÿ\nok".as_bytes());
    assert!(status_bar(&h).contains("Saved as UTF-8"));
    // And it stays UTF-8 afterwards.
    h.key(Key::Delete);
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().files[LETTER], "€afé ÿ\nok".as_bytes());
}

#[test]
fn windows_line_endings_are_hidden_and_written_back() {
    let bytes = b"one\r\ntwo\r\n\r\nfour";
    let h = harness_with_file(Fake::with(LETTER, bytes), LETTER);
    assert_eq!(text(&h), "one\ntwo\n\nfour");
    assert_eq!(h.app().content().line_count(), 4);
    assert!(!h.app().is_dirty());
    let (_, written) = open_and_save(bytes);
    assert_eq!(written, bytes);
    // New lines typed into such a file get the same endings.
    let mut h = harness_with_file(Fake::with(LETTER, bytes), LETTER);
    h.key_with(Key::End, Modifiers(Modifiers::CTRL));
    h.key(Key::Enter);
    h.type_text("five");
    h.shortcut(ctrl('s'));
    assert_eq!(h.app().disk().files[LETTER], b"one\r\ntwo\r\n\r\nfour\r\nfive");
    // The text line the runner checks counts `\n` as the break.
    assert_eq!(
        h.app().text_line(),
        format!("text 5 lines 18 bytes crc {:08x}", pbr_crc::crc32(b"one\ntwo\n\nfour\nfive"))
    );
}

#[test]
fn a_file_with_unix_line_endings_stays_that_way_and_mixed_ones_become_unix() {
    let (_, written) = open_and_save(b"a\nb\n");
    assert_eq!(written, b"a\nb\n");
    // Mixed endings have no one way to be put back: the text is saved with `\n`.
    let (_, written) = open_and_save(b"a\r\nb\nc");
    assert_eq!(written, b"a\nb\nc");
}

#[test]
fn a_byte_order_mark_is_kept_off_the_screen_and_put_back() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("héllo".as_bytes());
    let h = harness_with_file(Fake::with(LETTER, &bytes), LETTER);
    assert_eq!(text(&h), "héllo");
    let (_, written) = open_and_save(&bytes);
    assert_eq!(written, bytes);
}

#[test]
fn an_empty_file_and_a_file_of_only_line_breaks_open() {
    let h = harness_with_file(Fake::with(LETTER, b""), LETTER);
    assert_eq!(text(&h), "");
    assert!(h.dialog().is_none());
    assert!(!status_bar(&h).contains("New file"), "it is there, just empty");
    let h = harness_with_file(Fake::with(LETTER, b"\n\n\n"), LETTER);
    assert_eq!(h.app().content().line_count(), 4);
}

#[test]
fn a_file_with_nul_bytes_is_refused_with_a_message() {
    let mut h = harness_with_file(Fake::with(LETTER, b"abc\0def"), LETTER);
    assert_eq!(
        dialog_text(&h).unwrap(),
        "letter.txt doesn't look like a text file, so Notepad won't open it."
    );
    assert_eq!(text(&h), "");
    assert_eq!(h.app().path(), None);
    assert_eq!(h.app_mut().take_log(), [format!("couldn't open {LETTER}: it has NUL bytes")]);
    h.click_on("OK");
    assert!(h.dialog().is_none());
}

#[test]
fn a_file_over_the_limit_is_refused_with_1_3s_words() {
    let big = vec![b'a'; MAX_FILE + 1];
    let h = harness_with_file(Fake::with(LETTER, &big), LETTER);
    assert_eq!(
        dialog_text(&h).unwrap(),
        "letter.txt is too big for Notepad, which opens files up to 1 MB."
    );
    assert_eq!(text(&h), "");
    // One of exactly the limit is opened.
    let fits = vec![b'a'; MAX_FILE];
    let h = harness_with_file(Fake::with(LETTER, &fits), LETTER);
    assert!(h.dialog().is_none());
    assert_eq!(h.app().content().line(0).len(), MAX_FILE);
}

#[test]
fn a_refused_file_leaves_the_text_and_the_unsaved_mark_alone() {
    let mut h = harness_scaled(Fake::with(LETTER, b"x\0"), 1);
    h.type_text("mine");
    h.app_mut().disk_mut().files.insert("/Home/Documents/bad.txt".into(), b"a\0".to_vec());
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen("/Home/Documents/bad.txt".into()));
    // The question is asked first (the text is unsaved), and then the file is refused after it.
    assert_eq!(dialog_buttons(&h), ["Save", "Don't save", "Cancel"]);
    h.click_on("Cancel");
    assert_eq!(text(&h), "mine");
    assert!(h.app().is_dirty());
}

#[test]
fn a_missing_file_chosen_in_the_picker_says_so() {
    let mut h = harness();
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen("/Home/Documents/gone.txt".into()));
    assert_eq!(dialog_text(&h).unwrap(), "Couldn't open gone.txt. there is no such file.");
}

// ---- The clipboard. ----

#[test]
fn copy_cut_and_paste_with_the_menu_and_with_the_keys() {
    let mut h = harness();
    h.type_text("hello world");
    // Select "world": from the end, Shift+Ctrl+Left.
    h.key_with(Key::Left, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    assert_eq!(h.app().content().selected_text(), "world");
    // Copy with the menu.
    menu(&mut h, EDIT, "Copy");
    assert_eq!(h.clipboard_text().as_deref(), Some("world"));
    assert_eq!(text(&h), "hello world");
    // Cut with the keys: the editor's own, since it has the keyboard.
    h.advance(200);
    h.shortcut(ctrl('x'));
    assert_eq!(text(&h), "hello ");
    assert_eq!(h.clipboard_text().as_deref(), Some("world"));
    // Paste with the keys, then with the menu.
    h.shortcut(ctrl('v'));
    assert_eq!(text(&h), "hello world");
    h.advance(200);
    menu(&mut h, EDIT, "Paste");
    assert_eq!(text(&h), "hello worldworld");
    // Cut with the menu.
    for _ in 0..5 {
        h.key_with(Key::Left, Modifiers(Modifiers::SHIFT));
    }
    h.advance(200);
    menu(&mut h, EDIT, "Cut");
    assert_eq!(text(&h), "hello world");
    // Pasted text replaces the selection and may be many lines.
    h.set_clipboard_text("a\nb\nc");
    h.shortcut(ctrl('a'));
    h.shortcut(ctrl('v'));
    assert_eq!(text(&h), "a\nb\nc");
    assert_eq!(h.app().content().line_count(), 3);
}

#[test]
fn paste_is_greyed_out_until_the_clipboard_has_text() {
    let mut h = harness();
    h.click(EDIT.0, EDIT.1);
    assert!(!rows(&h).iter().find(|r| r.0 == "Paste").unwrap().2);
    h.key(Key::Escape);
    h.advance(200);
    // Another app copies something: Paste is available.
    h.set_clipboard_text("ready");
    h.click(EDIT.0, EDIT.1);
    assert!(rows(&h).iter().find(|r| r.0 == "Paste").unwrap().2);
    h.click_on("Paste");
    assert_eq!(text(&h), "ready");
    // The clipboard now holds a picture only (Paint copied): greyed again.
    h.advance(200);
    h.send(Event::Clipboard { text: false, picture: true });
    h.click(EDIT.0, EDIT.1);
    assert!(!rows(&h).iter().find(|r| r.0 == "Paste").unwrap().2);
    h.key(Key::Escape);
}

#[test]
fn copying_makes_paste_available_and_cut_and_copy_wait_for_a_selection() {
    let mut h = harness();
    h.type_text("abc");
    // Nothing selected: the rows are greyed, and the keys do nothing.
    h.shortcut(ctrl('c'));
    assert_eq!(h.clipboard_text(), None);
    h.shortcut(ctrl('x'));
    assert_eq!(text(&h), "abc");
    h.shortcut(ctrl('a'));
    h.shortcut(ctrl('c'));
    h.advance(200);
    h.click(EDIT.0, EDIT.1);
    let edit = rows(&h);
    assert!(edit.iter().find(|r| r.0 == "Paste").unwrap().2, "it copied, so there is text");
    assert!(edit.iter().find(|r| r.0 == "Cut").unwrap().2);
    h.key(Key::Escape);
}

#[test]
fn a_paste_that_would_make_the_text_too_big_is_refused() {
    let mut h = harness();
    h.type_text("mine");
    h.set_clipboard_text(&"x".repeat(MAX_FILE));
    h.shortcut(ctrl('v'));
    assert_eq!(text(&h), "mine");
    assert_eq!(
        dialog_text(&h).unwrap(),
        "That is too much text for Notepad, which holds up to 1 MB."
    );
    h.click_on("OK");
    // One that fits goes in.
    h.set_clipboard_text(&"x".repeat(MAX_FILE - 4));
    h.shortcut(ctrl('v'));
    assert_eq!(h.app().text_line().split(' ').nth(3), Some(&*MAX_FILE.to_string()));
}

// ---- Undo and redo. ----

#[test]
fn undo_and_redo_with_the_keys_and_the_menu() {
    let mut h = harness();
    h.type_text("one");
    h.key(Key::Enter);
    h.type_text("two");
    assert_eq!(text(&h), "one\ntwo");
    h.shortcut(ctrl('z'));
    assert_eq!(text(&h), "one\n");
    h.shortcut(ctrl('y'));
    assert_eq!(text(&h), "one\ntwo");
    h.shortcut(ctrl('z'));
    h.shortcut(Shortcut::ctrl_shift('z'));
    assert_eq!(text(&h), "one\ntwo", "Ctrl+Shift+Z redoes too");
    h.advance(200);
    menu(&mut h, EDIT, "Undo");
    assert_eq!(text(&h), "one\n");
    h.advance(200);
    menu(&mut h, EDIT, "Redo");
    assert_eq!(text(&h), "one\ntwo");
    // Undo right back to the start: nothing to undo, so the row is greyed out.
    for _ in 0..5 {
        h.shortcut(ctrl('z'));
    }
    assert_eq!(text(&h), "");
    h.advance(200);
    h.click(EDIT.0, EDIT.1);
    let edit = rows(&h);
    assert!(!edit.iter().find(|r| r.0 == "Undo").unwrap().2);
    assert!(edit.iter().find(|r| r.0 == "Redo").unwrap().2);
    h.key(Key::Escape);
}

#[test]
fn undoing_back_to_the_saved_text_takes_the_bullet_away() {
    let mut h = harness_with_file(Fake::with(LETTER, b"base"), LETTER);
    h.key_with(Key::End, Modifiers(Modifiers::CTRL));
    h.type_text("!");
    assert_eq!(h.app().title(), "• letter.txt");
    h.shortcut(ctrl('z'));
    assert_eq!(h.app().title(), "letter.txt");
    h.shortcut(ctrl('y'));
    assert_eq!(h.app().title(), "• letter.txt");
}

#[test]
fn select_all_selects_everything() {
    let mut h = harness();
    h.type_text("a\nbb\nccc");
    menu(&mut h, EDIT, "Select all");
    assert_eq!(h.app().content().selected_text(), "a\nbb\nccc");
    h.shortcut(ctrl('a'));
    assert_eq!(h.app().content().selected_text(), "a\nbb\nccc");
}

// ---- The title. ----

#[test]
fn the_title_says_untitled_or_the_file_and_the_bullet_for_unsaved_changes() {
    let mut h = harness();
    assert_eq!(h.app().title(), "Untitled");
    h.type_text("a");
    assert_eq!(h.app().title(), "• Untitled");
    assert_eq!(h.window(WindowId::MAIN).unwrap().title, "• Untitled");
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(h.window(WindowId::MAIN).unwrap().title, "letter.txt");
    h.type_text("b");
    assert_eq!(h.window(WindowId::MAIN).unwrap().title, "• letter.txt");
    // Moving the cursor and selecting are not changes.
    h.shortcut(ctrl('s'));
    h.key(Key::Left);
    h.shortcut(ctrl('a'));
    assert_eq!(h.app().title(), "letter.txt");
}

// ---- Asking before work is lost. ----

#[test]
fn closing_with_nothing_unsaved_just_closes() {
    let mut h = harness();
    h.close(WindowId::MAIN);
    assert!(h.finished());
    assert!(h.app().disk().writes.is_empty());
    let mut h = harness();
    h.type_text("a");
    h.shortcut(ctrl('z'));
    menu(&mut h, FILE, "Close");
    assert!(h.finished(), "back to the saved text, nothing to lose");
}

#[test]
fn closing_unsaved_work_asks_and_each_answer_does_what_it_says() {
    // The close button, with a file: Save writes it and closes.
    let mut h = harness_with_file(Fake::with(LETTER, b"x"), LETTER);
    h.type_text("y");
    h.app_mut().take_log();
    h.close(WindowId::MAIN);
    assert!(!h.finished());
    assert_eq!(dialog_text(&h).unwrap(), "Save changes to letter.txt?");
    assert_eq!(dialog_buttons(&h), ["Save", "Don't save", "Cancel"]);
    assert_eq!(h.app_mut().take_log(), ["asked save changes"]);
    h.click_on("Save");
    assert_eq!(h.app().disk().files[LETTER], b"yx");
    assert!(h.finished());

    // Don't save: closes, and nothing is written.
    let mut h = harness_with_file(Fake::with(LETTER, b"x"), LETTER);
    h.type_text("y");
    h.shortcut(ctrl('w'));
    assert_eq!(dialog_text(&h).unwrap(), "Save changes to letter.txt?");
    h.click_on("Don't save");
    assert!(h.finished());
    assert_eq!(h.app().disk().files[LETTER], b"x");
    assert!(h.app().disk().writes.is_empty());

    // Cancel: the window and the text stay, and typing carries on in the editor.
    let mut h = harness_with_file(Fake::with(LETTER, b"x"), LETTER);
    h.type_text("y");
    menu(&mut h, FILE, "Close");
    h.click_on("Cancel");
    assert!(!h.finished());
    assert!(h.dialog().is_none());
    assert_eq!(text(&h), "yx");
    assert!(h.app().is_dirty());
    h.type_text("z");
    assert_eq!(text(&h), "yzx");
    // Escape and the dialog's close button are Cancel too.
    h.close(WindowId::MAIN);
    h.key(Key::Escape);
    assert!(!h.finished());
    assert_eq!(text(&h), "yzx");
    h.close(WindowId::MAIN);
    let dialog = h.dialog().unwrap();
    h.close(dialog);
    assert!(!h.finished());
    assert!(h.dialog().is_none());
}

#[test]
fn closing_untitled_work_with_save_asks_the_picker_then_closes() {
    let mut h = harness();
    h.type_text("draft");
    h.close(WindowId::MAIN);
    assert_eq!(dialog_text(&h).unwrap(), "Save changes to Untitled?");
    h.click_on("Save");
    assert_eq!(h.app().disk().asked.len(), 1);
    assert_eq!(h.app().disk().asked[0].2, Pick::Save { name: "Untitled.txt".into() });
    assert!(!h.finished(), "not until the user has chosen");
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert_eq!(h.app().disk().files[LETTER], b"draft");
    assert!(h.finished());
    // Cancelling the picker keeps the window and the text.
    let mut h = harness();
    h.type_text("draft");
    h.close(WindowId::MAIN);
    h.click_on("Save");
    answer(&mut h, PickAnswer::Cancelled);
    assert!(!h.finished());
    assert_eq!(text(&h), "draft");
    // So does a save that fails, and the text can still be saved or thrown away after.
    let mut h = harness_scaled(Fake { full: true, ..Fake::default() }, 1);
    h.type_text("draft");
    h.close(WindowId::MAIN);
    h.click_on("Save");
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert!(!h.finished());
    h.click_on("OK");
    h.close(WindowId::MAIN);
    h.click_on("Don't save");
    assert!(h.finished());
}

#[test]
fn new_with_unsaved_work_asks_and_each_answer_does_what_it_says() {
    // Don't save: an empty text from no file.
    let mut h = harness_with_file(Fake::with(LETTER, b"x"), LETTER);
    h.type_text("y");
    menu(&mut h, FILE, "New");
    assert_eq!(dialog_text(&h).unwrap(), "Save changes to letter.txt?");
    h.click_on("Don't save");
    assert_eq!(text(&h), "");
    assert_eq!(h.app().path(), None);
    assert_eq!(h.app().title(), "Untitled");
    assert!(!h.app().is_dirty());
    assert_eq!(h.app().disk().files[LETTER], b"x");
    // Cancel: nothing changes.
    let mut h = harness_with_file(Fake::with(LETTER, b"x"), LETTER);
    h.type_text("y");
    h.shortcut(ctrl('n'));
    h.click_on("Cancel");
    assert_eq!(text(&h), "yx");
    assert_eq!(h.app().path(), Some(LETTER));
    // Save: written, then new.
    h.shortcut(ctrl('n'));
    h.click_on("Save");
    assert_eq!(h.app().disk().files[LETTER], b"yx");
    assert_eq!((text(&h), h.app().path()), (String::new(), None));
    // With nothing unsaved, New needs no question, and it logs `new`.
    h.type_text("a");
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen("/Home/Documents/a.txt".into()));
    h.app_mut().take_log();
    h.shortcut(ctrl('n'));
    assert!(h.dialog().is_none());
    assert_eq!(h.app_mut().take_log(), ["new"]);
    assert_eq!(h.app().title(), "Untitled");
}

#[test]
fn open_with_unsaved_work_asks_after_the_file_is_chosen_and_each_answer_does_what_it_says() {
    let other = "/Home/Documents/other.txt";
    let mut fake = Fake::with(LETTER, b"x");
    fake.files.insert(other.into(), b"other text".to_vec());
    let mut h = harness_with_file(fake, LETTER);
    h.type_text("y");
    h.shortcut(ctrl('o'));
    assert!(h.dialog().is_none(), "the picker comes first");
    answer(&mut h, PickAnswer::Chosen(other.into()));
    assert_eq!(dialog_text(&h).unwrap(), "Save changes to letter.txt?");
    // Cancel: the text stays.
    h.click_on("Cancel");
    assert_eq!((text(&h), h.app().path()), ("yx".to_string(), Some(LETTER)));
    // Don't save: the file chosen is opened.
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen(other.into()));
    h.click_on("Don't save");
    assert_eq!((text(&h), h.app().path()), ("other text".to_string(), Some(other)));
    assert_eq!(h.app().disk().files[LETTER], b"x", "not saved");
    // Save: the old text is written, then the file chosen is opened.
    h.type_text("!");
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    h.click_on("Save");
    assert_eq!(h.app().disk().files[other], b"!other text");
    assert_eq!((text(&h), h.app().path()), ("x".to_string(), Some(LETTER)));
    // Unsaved work with no file: Save asks the picker again, and opens after it.
    h.shortcut(ctrl('n'));
    h.type_text("draft");
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen(other.into()));
    h.click_on("Save");
    answer(&mut h, PickAnswer::Chosen("/Home/Documents/draft.txt".into()));
    assert_eq!(h.app().disk().files["/Home/Documents/draft.txt"], b"draft");
    assert_eq!(text(&h), "!other text");
}

#[test]
fn only_one_question_is_asked_at_a_time() {
    let mut h = harness();
    h.type_text("a");
    h.close(WindowId::MAIN);
    assert_eq!(h.app_mut().take_log(), ["asked save changes"]);
    h.close(WindowId::MAIN);
    assert!(h.app_mut().take_log().is_empty());
    assert_eq!(h.windows().len(), 2);
}

// ---- What the runner reads. ----

#[test]
fn the_text_line_says_how_many_lines_bytes_and_the_crc() {
    let mut h = harness();
    assert_eq!(h.app().text_line(), "text 1 lines 0 bytes crc 00000000");
    assert!(!h.app_mut().take_text_changed());
    h.type_text("héllo\nworld");
    assert!(h.app_mut().take_text_changed());
    assert!(!h.app_mut().take_text_changed());
    let want = pbr_crc::crc32("héllo\nworld".as_bytes());
    assert_eq!(h.app().text_line(), format!("text 2 lines 12 bytes crc {want:08x}"));
    // Moving is not a change; opening and New are.
    h.key(Key::Left);
    assert!(!h.app_mut().take_text_changed());
    h.shortcut(ctrl('s'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    h.shortcut(ctrl('n'));
    assert!(h.app_mut().take_text_changed());
    h.shortcut(ctrl('o'));
    answer(&mut h, PickAnswer::Chosen(LETTER.into()));
    assert!(h.app_mut().take_text_changed());
    assert_eq!(h.app().text_line(), format!("text 2 lines 12 bytes crc {want:08x}"));
}

#[test]
fn the_describe_lines_show_the_editor_and_its_cursor() {
    let mut h = harness();
    h.type_text("ab");
    let lines = h.describe_lines(WindowId::MAIN);
    assert!(lines.iter().any(|l| l.starts_with("menu bar \"File|Edit\"")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("text editor") && l.contains("line 1, column 3")),
        "{lines:?}"
    );
}

// ---- The window's size. ----

#[test]
fn the_window_can_be_made_smaller_and_bigger() {
    assert_eq!(settings().min_size, MIN_SIZE);
    for factor in [1, 2] {
        let mut h = harness_scaled(Fake::default(), factor);
        h.type_text("some text");
        h.resize(MIN_SIZE.0, MIN_SIZE.1);
        let r = h.find(EDITOR_ID).unwrap().1.rect;
        assert_eq!((r.width, r.y), (280, MENU));
        assert!(r.height > 100, "{}", r.height);
        h.type_text("!");
        assert_eq!(text(&h), "some text!");
        h.resize(900, 700);
        let r = h.find(EDITOR_ID).unwrap().1.rect;
        assert_eq!((r.width, r.height), (900, 700 - MENU - 20));
    }
}

#[test]
fn a_long_text_scrolls_to_the_cursor() {
    let mut h = harness();
    let lines: Vec<String> = (1..=200).map(|n| format!("line {n}")).collect();
    h.app_mut().content_mut().perform(Action::InsertText(lines.join("\n")));
    h.refresh();
    assert_eq!(h.app().content().cursor().0, 199);
    // The last line is in view, so a click on the editor's bottom row lands on it.
    let r = h.find(EDITOR_ID).unwrap().1.rect;
    h.click(r.x + 100, r.y + r.height - 6);
    assert_eq!(h.app().content().cursor().0, 199);
}

// ---- How it looks. ----

#[test]
fn how_it_looks() {
    // Just opened: empty, as the desktop boot's screenshot shows it.
    for (factor, name) in [(1, "notepad-1x"), (2, "notepad-2x")] {
        let h = harness_scaled(Fake::default(), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    // A few lines, a selection, and the Edit menu open.
    let mut h = harness();
    h.type_text("Dear Chris,\n\nThanks for the rock.\nIt is pretty big.\n\nBest wishes");
    h.key_with(Key::Home, Modifiers(Modifiers::CTRL));
    h.key(Key::Down);
    h.key(Key::Down);
    h.key_with(Key::Right, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    h.key_with(Key::Right, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    h.set_clipboard_text("something");
    h.click(EDIT.0, EDIT.1);
    h.check_window_picture_in(WindowId::MAIN, &dir(), "notepad-text-1x");
    let popup = h.popup().expect("the Edit menu");
    h.check_window_picture_in(popup, &dir(), "notepad-menu-1x");
    h.key(Key::Escape);
    // The question.
    let mut h = harness_with_file(Fake::with(LETTER, b"Hello,\nit is me.\n"), LETTER);
    h.type_text("A few words. ");
    h.close(WindowId::MAIN);
    let dialog = h.dialog().expect("the question");
    h.check_window_picture_in(dialog, &dir(), "notepad-question-1x");
}
