// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `TextField` and the line to type in dialogs, through the harness: every key with and without
//! a selection, Unicode, the app changing the text under the cursor, undo and redo, the
//! clipboard, the pointer, scrolling across, Tab and Enter, input dialogs, and the look at 1x
//! and 2x (note 0023, section 5; note 0025, section 2).

use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Input(String),
    Submit,
    Ok,
    Ask,
    Renamed(String),
}

struct Form {
    text: String,
    editable: bool,
    submit: bool,
    ok: u32,
    submitted: u32,
    renamed: Vec<String>,
    select_all: bool,
    width: Length,
    /// Changes `text` to this when the app sees the next input (an app that rewrites text).
    force: Option<String>,
}

impl Form {
    fn new(text: &str) -> Form {
        Form {
            text: text.to_string(),
            editable: true,
            submit: false,
            ok: 0,
            submitted: 0,
            renamed: Vec::new(),
            select_all: false,
            width: Length::Fill,
            force: None,
        }
    }
}

impl App for Form {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let mut f = TextField::new(&self.text).placeholder("Name").width(self.width);
        if self.editable {
            f = f.on_input(Msg::Input);
        }
        if self.submit {
            f = f.on_submit(Msg::Submit);
        }
        if self.select_all {
            f = f.select_all();
        }
        Column::new()
            .padding(8)
            .spacing(6)
            .push(f.id("f"))
            .push(
                Row::new()
                    .spacing(6)
                    .push(Button::new("OK").on_press(Msg::Ok).default())
                    .push(Button::new("Rename").on_press(Msg::Ask)),
            )
            .into()
    }

    fn update(&mut self, m: Msg, cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Input(s) => self.text = self.force.take().unwrap_or(s),
            Msg::Submit => self.submitted += 1,
            Msg::Ok => self.ok += 1,
            Msg::Ask => {
                cx.show(Dialog::input("Rename", "New name:", "letter.txt", Msg::Renamed));
            }
            Msg::Renamed(s) => self.renamed.push(s),
        }
    }
}

fn settings() -> Settings {
    Settings { size: (200, 80), resizable: false, min_size: (0, 0) }
}

fn harness(text: &str) -> Harness<Form> {
    let mut h = Harness::new(Form::new(text), settings());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("f"));
    h
}

fn text(h: &Harness<Form>) -> String {
    h.app().text.clone()
}

const CTRL: Modifiers = Modifiers(Modifiers::CTRL);
const SHIFT: Modifiers = Modifiers(Modifiers::SHIFT);
const CTRL_SHIFT: Modifiers = Modifiers(Modifiers::CTRL | Modifiers::SHIFT);

/// The x of character boundary `i` in the form's field (at 8, 8; the text starts 5 in).
fn bx(i: i32) -> i32 {
    13 + 6 * i
}

#[test]
fn the_field_describes_itself() {
    let h = harness("hello");
    let d = h.find("f").unwrap().1;
    assert_eq!(d.what.role, "text field");
    assert_eq!(d.what.label, "Name");
    assert_eq!(d.what.value, "hello");
    assert!(d.what.enabled);
    assert_eq!((d.rect.height, d.rect.x), (24, 8));
    // Fill: the window's width less the padding.
    assert_eq!(d.rect.width, 184);
    assert!(h.describe_lines(WindowId::MAIN).iter().any(|l| l.starts_with("text field")));
}

#[test]
fn typing_inserts_at_the_cursor_including_unicode() {
    let mut h = harness("");
    h.type_text("ab");
    h.type_text("é€ß");
    assert_eq!(text(&h), "abé€ß");
    h.key(Key::Left);
    h.key(Key::Left);
    h.type_text("x");
    assert_eq!(text(&h), "abéx€ß");
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    assert_eq!(text(&h), "ab€ß");
    h.key(Key::Delete);
    assert_eq!(text(&h), "abß");
    h.key(Key::Delete);
    assert_eq!(text(&h), "ab");
    h.key(Key::Delete);
    assert_eq!(text(&h), "ab");
    h.key(Key::Home);
    h.key(Key::Backspace);
    assert_eq!(text(&h), "ab");
    h.key(Key::Right);
    h.type_text(" Q");
    assert_eq!(text(&h), "a Qb");
}

#[test]
fn typing_replaces_the_selection_and_so_do_delete_keys() {
    let mut h = harness("hello world");
    h.shortcut(Shortcut::ctrl('a'));
    h.type_text("x");
    assert_eq!(text(&h), "x");

    let mut h = harness("hello world");
    h.key(Key::Home);
    for _ in 0..5 {
        h.key_with(Key::Right, SHIFT);
    }
    h.key(Key::Backspace);
    assert_eq!(text(&h), " world");
    h.key(Key::Home);
    for _ in 0..3 {
        h.key_with(Key::Right, SHIFT);
    }
    h.key(Key::Delete);
    assert_eq!(text(&h), "rld");
}

#[test]
fn ctrl_backspace_and_delete_take_a_word() {
    let mut h = harness("one two  three");
    h.key_with(Key::Backspace, CTRL);
    assert_eq!(text(&h), "one two  ");
    h.key_with(Key::Backspace, CTRL);
    assert_eq!(text(&h), "one ");
    h.key(Key::Home);
    h.key_with(Key::Delete, CTRL);
    assert_eq!(text(&h), "");
    h.type_text("a.b");
    h.key(Key::Home);
    h.key_with(Key::Delete, CTRL);
    assert_eq!(text(&h), ".b");
}

#[test]
fn arrows_home_end_and_words_with_and_without_shift() {
    let mut h = harness("foo bar.baz");
    h.key(Key::Home);
    h.key_with(Key::Right, CTRL);
    h.type_text("|");
    assert_eq!(text(&h), "foo |bar.baz");
    h.key_with(Key::Right, CTRL);
    h.type_text("|");
    assert_eq!(text(&h), "foo |bar|.baz");
    h.key_with(Key::Left, CTRL);
    h.type_text("^");
    assert_eq!(text(&h), "foo |bar^|.baz");
    h.key(Key::End);
    h.key_with(Key::Left, CTRL_SHIFT);
    h.key(Key::Backspace);
    assert_eq!(text(&h), "foo |bar^|.");
    // Shift+Home selects to the start, and a plain Left goes to the selection's start.
    h.key_with(Key::Home, SHIFT);
    h.key(Key::Left);
    h.type_text("<");
    assert_eq!(text(&h), "<foo |bar^|.");
    // Shift+End selects to the end; a plain Right goes to its end.
    h.key_with(Key::Left, SHIFT);
    h.key(Key::Home);
    h.key_with(Key::End, SHIFT);
    h.key(Key::Right);
    h.type_text(">");
    assert_eq!(text(&h), "<foo |bar^|.>");
    // Left and Right stop at the ends.
    h.key(Key::End);
    h.key(Key::Right);
    h.type_text("!");
    h.key(Key::Home);
    h.key(Key::Left);
    h.type_text("!");
    assert_eq!(text(&h), "!<foo |bar^|.>!");
}

#[test]
fn enter_escape_tab_and_arrows_up_down_are_not_used() {
    let mut h = harness("abc");
    h.key(Key::Up);
    h.key(Key::Down);
    h.key(Key::PageUp);
    assert_eq!(text(&h), "abc");
    // Enter goes to the default button, since the field has no `on_submit`.
    h.key(Key::Enter);
    assert_eq!((h.app().ok, h.app().submitted), (1, 0));
    // Tab leaves the field.
    h.key(Key::Tab);
    assert_ne!(h.focused().unwrap().key.as_deref(), Some("f"));
    // Keys no longer reach it.
    h.type_text("zz");
    assert_eq!(text(&h), "abc");
    // Shortcuts with Ctrl, Alt or Meta that aren't the field's are not typed.
    h.key(Key::Tab);
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("f"));
    h.shortcut(Shortcut::ctrl('s'));
    h.shortcut(Shortcut::alt('q'));
    assert_eq!(text(&h), "abc");
}

#[test]
fn enter_sends_on_submit_when_there_is_one() {
    let mut h = Harness::new(Form { submit: true, ..Form::new("abc") }, settings());
    h.key(Key::Tab);
    h.key(Key::Enter);
    assert_eq!((h.app().ok, h.app().submitted), (0, 1));
    assert_eq!(h.messages().last(), Some(&Msg::Submit));
}

#[test]
fn nothing_focused_means_nothing_typed() {
    let mut h = Harness::new(Form::new("abc"), settings());
    h.type_text("xyz");
    h.key(Key::Backspace);
    assert_eq!(text(&h), "abc");
    assert!(h.messages().is_empty());
}

#[test]
fn the_clipboard_copies_cuts_and_pastes() {
    let mut h = harness("hello world");
    h.key(Key::Home);
    for _ in 0..5 {
        h.key_with(Key::Right, SHIFT);
    }
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello"));
    assert_eq!(text(&h), "hello world");
    // Copy with nothing selected leaves the clipboard alone.
    h.key(Key::End);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello"));
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(text(&h), "hello worldhello");
    // Cut the last word.
    h.key_with(Key::Left, CTRL_SHIFT);
    h.shortcut(Shortcut::ctrl('x'));
    assert_eq!(text(&h), "hello ");
    assert_eq!(h.clipboard_text().as_deref(), Some("worldhello"));
    // Paste replaces the selection; line breaks become spaces.
    h.set_clipboard_text("a\nb\r\nc\rd\te");
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(text(&h), "hello a b c d e");
    h.shortcut(Shortcut::ctrl('a'));
    h.set_clipboard_text("é€");
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(text(&h), "é€");
    // Cut with nothing selected does nothing.
    h.shortcut(Shortcut::ctrl('x'));
    assert_eq!(text(&h), "é€");
}

#[test]
fn a_read_only_field_selects_and_copies_but_does_not_change() {
    let mut h = Harness::new(Form { editable: false, ..Form::new("fixed") }, settings());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("f"));
    h.type_text("x");
    h.key(Key::Backspace);
    h.key(Key::Delete);
    h.shortcut(Shortcut::ctrl('a'));
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("fixed"));
    h.set_clipboard_text("other");
    h.shortcut(Shortcut::ctrl('v'));
    h.shortcut(Shortcut::ctrl('x'));
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(h.clipboard_text().as_deref(), Some("other"));
    assert_eq!(text(&h), "fixed");
    assert!(h.messages().is_empty());
}

#[test]
fn typing_in_a_row_is_one_undo_step() {
    let mut h = harness("");
    h.type_text("abc");
    h.key(Key::Left);
    h.type_text("x");
    h.type_text("y");
    assert_eq!(text(&h), "abxyc");
    h.shortcut(Shortcut::ctrl('z'));
    // The cursor moved, so "xy" was a step of its own; the cursor comes back with the text.
    assert_eq!(text(&h), "abc");
    h.type_text("Q");
    assert_eq!(text(&h), "abQc");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "abc");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "");
    h.shortcut(Shortcut::ctrl('y'));
    assert_eq!(text(&h), "abc");
    h.shortcut(Shortcut::ctrl_shift('z'));
    assert_eq!(text(&h), "abQc");
    h.shortcut(Shortcut::ctrl('y'));
    assert_eq!(text(&h), "abQc");
    // Redo's cursor is where the edit left it: after the Q.
    h.type_text("!");
    assert_eq!(text(&h), "abQ!c");
}

#[test]
fn every_other_change_is_a_step_and_a_new_change_ends_redo() {
    let mut h = harness("one two three");
    h.key_with(Key::Backspace, CTRL);
    h.key_with(Key::Backspace, CTRL);
    assert_eq!(text(&h), "one ");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "one two ");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "one two three");
    // The cursor came back with the text, at the end.
    h.type_text("!");
    assert_eq!(text(&h), "one two three!");
    h.shortcut(Shortcut::ctrl('y'));
    assert_eq!(text(&h), "one two three!");
    // A selection replaced by typing, then undone, brings the selection back.
    h.shortcut(Shortcut::ctrl('a'));
    h.type_text("x");
    h.type_text("y");
    assert_eq!(text(&h), "xy");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(text(&h), "one two three!");
    h.type_text("?");
    assert_eq!(text(&h), "?");
}

#[test]
fn undo_keeps_at_most_a_hundred_steps() {
    let mut h = harness("");
    for _ in 0..120 {
        h.type_text("a");
        h.key(Key::Left);
        h.key(Key::Right);
    }
    assert_eq!(text(&h).len(), 120);
    for _ in 0..150 {
        h.shortcut(Shortcut::ctrl('z'));
    }
    assert_eq!(text(&h).len(), 20);
}

#[test]
fn the_app_changing_the_text_brings_the_cursor_within_it() {
    let mut h = harness("hello world");
    h.app_mut().text = "hi".to_string();
    h.refresh();
    // The cursor was at 11; it is now at the end of "hi".
    h.type_text("!");
    assert_eq!(text(&h), "hi!");
    h.app_mut().text = String::new();
    h.refresh();
    h.type_text("é");
    assert_eq!(text(&h), "é");
    // A selection past the new end is brought within it too.
    h.app_mut().text = "abcdef".to_string();
    h.refresh();
    h.shortcut(Shortcut::ctrl('a'));
    h.app_mut().text = "ab".to_string();
    h.refresh();
    h.type_text("X");
    assert_eq!(text(&h), "X");
    // An app that rewrites what is typed.
    h.app_mut().force = Some("FIXED".to_string());
    h.type_text("a");
    assert_eq!(text(&h), "FIXED");
    h.key(Key::Backspace);
    // The cursor stayed where the typed letter was, after the F.
    assert_eq!(text(&h), "FXED");
}

#[test]
fn a_click_places_the_cursor_at_the_nearest_boundary_and_takes_the_focus() {
    let mut h = Harness::new(Form::new("abcdef"), settings());
    assert!(h.focused().is_none());
    h.click(bx(2) + 2, 20);
    assert_eq!(h.focused().unwrap().key.as_deref(), Some("f"));
    h.type_text("|");
    assert_eq!(text(&h), "ab|cdef");
    // Just over halfway to the next boundary goes to it.
    h.click(bx(4) + 4, 20);
    h.type_text("|");
    assert_eq!(text(&h), "ab|cd|ef");
    // Past either end.
    h.click(190, 20);
    h.type_text("$");
    h.click(9, 20);
    h.type_text("^");
    assert_eq!(text(&h), "^ab|cd|ef$");
    assert_eq!(h.find("f").unwrap().1.what.value, "^ab|cd|ef$");
}

#[test]
fn shift_click_extends_the_selection() {
    let mut h = Harness::new(Form::new("abcdef"), settings());
    h.click(bx(1), 20);
    h.advance(1000);
    h.click_with(bx(4), 20, SHIFT);
    h.type_text("X");
    assert_eq!(text(&h), "aXef");
}

#[test]
fn dragging_selects_even_outside_the_field() {
    let mut h = Harness::new(Form::new("hello world"), settings());
    h.drag((bx(1), 20), (bx(4), 20));
    h.type_text("_");
    assert_eq!(text(&h), "h_o world");
    // Dragging left out of the field and below it selects to the start.
    let mut h = Harness::new(Form::new("hello world"), settings());
    h.drag((bx(5), 20), (2, 70));
    h.type_text("_");
    assert_eq!(text(&h), "_ world");
    // Dragging right past the end selects to the end.
    let mut h = Harness::new(Form::new("hello world"), settings());
    h.drag((bx(5), 20), (199, 60));
    h.type_text("_");
    assert_eq!(text(&h), "hello_");
    // Letting go ends the drag: later moves don't select.
    h.move_to(bx(0), 20);
    h.type_text("-");
    assert_eq!(text(&h), "hello_-");
}

#[test]
fn double_click_takes_a_word_and_triple_click_all() {
    let mut h = Harness::new(Form::new("hello big world"), settings());
    h.double_click(bx(7), 20);
    h.type_text("_");
    assert_eq!(text(&h), "hello _ world");

    let mut h = Harness::new(Form::new("hello big world"), settings());
    h.double_click(bx(2), 20);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello"));
    // On a space, the run of spaces.
    let mut h = Harness::new(Form::new("a   b"), settings());
    h.double_click(bx(2) + 1, 20);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("   "));
    // Past the end: the last word.
    let mut h = Harness::new(Form::new("big dog"), settings());
    h.double_click(190, 20);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("dog"));

    let mut h = Harness::new(Form::new("hello big world"), settings());
    h.click(bx(3), 20);
    h.advance(50);
    h.click(bx(3), 20);
    h.advance(50);
    h.click(bx(3), 20);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello big world"));
    // A double click in an empty field is harmless.
    let mut h = Harness::new(Form::new(""), settings());
    h.double_click(30, 20);
    h.type_text("a");
    assert_eq!(text(&h), "a");
}

#[test]
fn a_long_text_scrolls_to_keep_the_cursor_in_view() {
    let long: String = (0..60).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    let mut h = harness("");
    h.type_text(&long);
    // The cursor is at the end, so the field is scrolled: a click at its left edge lands well
    // into the text.
    h.advance(1000);
    h.click(bx(0), 20);
    h.type_text("|");
    let t = text(&h);
    let at = t.find('|').unwrap();
    assert!(at > 25 && at < 40, "the click landed at {at}");
    // Moving the cursor to the start scrolls back to the start.
    h.key(Key::Home);
    h.advance(1000);
    h.click(bx(0), 20);
    h.type_text("^");
    assert!(text(&h).starts_with("^"), "{}", text(&h));
    // Moving right past the visible end scrolls: the click at the right edge is further in
    // than the 29 characters that fit.
    h.key(Key::Home);
    for _ in 0..45 {
        h.key(Key::Right);
    }
    h.advance(1000);
    h.click(bx(0), 20);
    h.type_text("@");
    let at = text(&h).find('@').unwrap();
    assert!(at > 10, "{at}");
    // The widths: keys move the cursor and keep it in view, whatever the text.
    h.key(Key::End);
    h.key_with(Key::Left, CTRL);
    h.type_text("#");
    assert!(text(&h).contains('#'));
}

#[test]
fn the_pointer_is_a_text_cursor_over_the_field() {
    let mut h = harness("abc");
    h.move_to(bx(1), 20);
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Text);
    h.move_to(5, 70);
    assert_ne!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Text);
}

#[test]
fn edge_cases_do_not_panic() {
    // Zero-width and tiny fields.
    for width in [0, 1, 10, 40, 5000] {
        let mut h = Harness::new(
            Form { width: Length::Fixed(width), ..Form::new("some text here") },
            settings(),
        );
        h.key(Key::Tab);
        h.type_text("é");
        h.key(Key::Home);
        h.key(Key::End);
        h.click(10, 20);
        h.double_click(20, 20);
        h.drag((10, 20), (180, 20));
        h.shortcut(Shortcut::ctrl('a'));
        h.shortcut(Shortcut::ctrl('z'));
    }
    // Enormous text.
    let big = "x".repeat(20_000);
    let mut h = Harness::new(Form::new(&big), settings());
    h.key(Key::Tab);
    h.key(Key::Home);
    h.type_text("y");
    h.key(Key::End);
    h.key_with(Key::Left, CTRL_SHIFT);
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().unwrap().len(), 20_001);
    // A window resized under the field.
    let mut h = harness("resize me please, this is long enough to scroll when small");
    h.resize(60, 80);
    h.key(Key::Left);
    h.resize(0, 0);
    h.resize(300, 80);
    h.type_text("!");
    // Clicks outside, and the wheel over it.
    h.click(2, 2);
    h.scroll(30, 20, 3);
    assert!(text(&h).contains('!'));
}

fn pictures(scale: u32) {
    let make = |text: &str| Harness::with_scale(Form::new(text), settings(), scale);
    let suffix = format!("{scale}x");
    // Empty, with the placeholder (not focused).
    make("").check_picture(&format!("text_field-empty-{suffix}"));
    // With text.
    make("letter.txt").check_picture(&format!("text_field-text-{suffix}"));
    // Focused, with the cursor.
    let mut h = make("letter.txt");
    h.key(Key::Tab);
    h.key(Key::Left);
    h.key(Key::Left);
    h.check_picture(&format!("text_field-focused-{suffix}"));
    // A selection, focused...
    let mut h = make("letter.txt");
    h.key(Key::Tab);
    h.key(Key::Home);
    for _ in 0..6 {
        h.key_with(Key::Right, SHIFT);
    }
    h.check_picture(&format!("text_field-selection-{suffix}"));
    // ... and without the keyboard.
    h.key(Key::Tab);
    h.check_picture(&format!("text_field-selection-inactive-{suffix}"));
    // Scrolled.
    let mut h = make("a long file name that does not fit in the field at all, truncated.txt");
    h.key(Key::Tab);
    h.check_picture(&format!("text_field-scrolled-{suffix}"));
    // Read-only.
    let h =
        Harness::with_scale(Form { editable: false, ..Form::new("read only") }, settings(), scale);
    h.check_picture(&format!("text_field-readonly-{suffix}"));
}

#[test]
fn the_look_at_1x() {
    pictures(1);
}

#[test]
fn the_look_at_2x() {
    pictures(2);
}

// ---- The line to type in dialogs. ----

fn dialog_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/sandstone")
}

#[test]
fn an_input_dialog_replaces_the_selected_name_and_ok_sends_it() {
    let mut h = Harness::new(Form::new("x"), settings());
    h.click_on("Rename");
    let d = h.dialog().expect("a dialog");
    assert_eq!(h.window(d).unwrap().title, "Rename");
    // The line has the focus, with the name selected.
    let f = h.focused().unwrap();
    assert_eq!(f.key.as_deref(), Some("dialog-input"));
    assert_eq!(f.what.value, "letter.txt");
    assert_eq!(f.rect.width, 300);
    h.type_text("memo");
    assert_eq!(h.focused().unwrap().what.value, "memo");
    h.type_text(".doc");
    h.click_on("OK");
    assert!(h.dialog().is_none());
    assert_eq!(h.app().renamed, vec!["memo.doc".to_string()]);
}

#[test]
fn enter_in_an_input_dialog_presses_ok_and_cancel_and_escape_send_nothing() {
    let mut h = Harness::new(Form::new("x"), settings());
    h.click_on("Rename");
    h.key_with(Key::Left, Modifiers::NONE);
    h.type_text("!");
    h.key(Key::Enter);
    assert!(h.dialog().is_none());
    // Left collapsed the selection to its start, so "!" went in front.
    assert_eq!(h.app().renamed, vec!["!letter.txt".to_string()]);

    h.click_on("Rename");
    h.type_text("zzz");
    h.click_on("Cancel");
    assert!(h.dialog().is_none());
    h.click_on("Rename");
    h.type_text("zzz");
    h.key(Key::Escape);
    assert!(h.dialog().is_none());
    h.click_on("Rename");
    h.close(h.dialog().unwrap());
    assert_eq!(h.app().renamed.len(), 1);
    // OK with the name untouched sends it as it was.
    h.click_on("Rename");
    h.click_on("OK");
    assert_eq!(h.app().renamed.last().unwrap(), "letter.txt");
    // Undo works in the dialog's line, and an emptied line is sent empty.
    h.click_on("Rename");
    h.key(Key::Backspace);
    assert_eq!(h.focused().unwrap().what.value, "");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(h.focused().unwrap().what.value, "letter.txt");
    h.key(Key::Backspace);
    h.key(Key::Enter);
    assert_eq!(h.app().renamed.last().unwrap(), "");
}

#[test]
fn the_input_dialogs_look() {
    for (scale, name) in [(1, "text_field-dialog-1x"), (2, "text_field-dialog-2x")] {
        let mut h = Harness::with_scale(Form::new("x"), settings(), scale);
        h.click_on("Rename");
        let d = h.dialog().unwrap();
        h.check_window_picture_in(d, &dialog_dir(), name);
    }
    let mut h = Harness::new(Form::new("x"), settings());
    h.click_on("Rename");
    h.type_text("new");
    let d = h.dialog().unwrap();
    h.check_window_picture_in(d, &dialog_dir(), "text_field-dialog-typed-1x");
}
