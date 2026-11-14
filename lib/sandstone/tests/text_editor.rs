// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The text editor: `Content` and its actions on their own (moving, selecting, editing, undo and
//! redo, the modified flag, finding, Unicode, tabs, a hundred thousand lines), then the widget
//! through the harness (keys, the clipboard, the pointer, scrolling, read-only, tiny sizes), and
//! its look at 1x and 2x (note 0023, sections 5 and 6; note 0026, section 1).

use pbr_sandstone::testing::Harness;
use pbr_sandstone::text_editor::{Action, Content, Motion};
use pbr_sandstone::*;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------
// Content on its own.
// ---------------------------------------------------------------------------------------------

fn mv(c: &mut Content, motion: Motion) {
    c.perform(Action::Move { motion, select: false });
}

fn mv_select(c: &mut Content, motion: Motion) {
    c.perform(Action::Move { motion, select: true });
}

fn to(c: &mut Content, line: usize, col: usize) {
    mv(c, Motion::To((line, col)));
}

fn type_str(c: &mut Content, s: &str) {
    for ch in s.chars() {
        c.perform(Action::InsertChar(ch));
    }
}

fn undo(c: &mut Content) {
    c.perform(Action::Undo);
}

fn redo(c: &mut Content) {
    c.perform(Action::Redo);
}

#[test]
fn a_new_text_is_one_empty_line() {
    let c = Content::new();
    assert_eq!(c.line_count(), 1);
    assert_eq!(c.line(0), "");
    assert_eq!(c.line(7), "");
    assert_eq!(c.text(), "");
    assert_eq!(c.cursor(), (0, 0));
    assert_eq!(c.selection(), None);
    assert!(!c.is_modified());
    assert!(!c.can_undo());
    assert_eq!(Content::from_text("").line_count(), 1);
}

#[test]
fn text_is_split_at_line_breaks_and_joined_again() {
    let c = Content::from_text("one\ntwo\n\nfour");
    assert_eq!(c.line_count(), 4);
    assert_eq!((c.line(0), c.line(2), c.line(3)), ("one", "", "four"));
    assert_eq!(c.text(), "one\ntwo\n\nfour");
    // A text ending in a break has an empty last line, and gets its break back.
    let c = Content::from_text("a\n");
    assert_eq!(c.line_count(), 2);
    assert_eq!(c.text(), "a\n");
    // Windows and old Mac line breaks read as `\n`.
    let c = Content::from_text("a\r\nb\rc\n\r\nd");
    assert_eq!(c.text(), "a\nb\nc\n\nd");
    assert!(!c.is_modified());
}

#[test]
fn typing_inserts_and_columns_are_characters() {
    let mut c = Content::new();
    type_str(&mut c, "héllo");
    assert_eq!(c.text(), "héllo");
    assert_eq!(c.cursor(), (0, 5));
    mv(&mut c, Motion::Left);
    mv(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 3));
    c.perform(Action::InsertChar('x'));
    assert_eq!(c.text(), "hélxlo");
    assert_eq!(c.cursor(), (0, 4));
    assert!(c.is_modified());
}

#[test]
fn four_byte_characters_are_one_column() {
    let mut c = Content::new();
    type_str(&mut c, "😀a€");
    assert_eq!(c.cursor(), (0, 3));
    mv(&mut c, Motion::Left);
    mv(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 1));
    c.perform(Action::Backspace { word: false });
    assert_eq!(c.text(), "a€");
    assert_eq!(c.cursor(), (0, 0));
    c.perform(Action::Delete { word: false });
    assert_eq!(c.text(), "€");
    // A position inside a line is a character, however many bytes come before it.
    let mut c = Content::from_text("ééé😀x");
    to(&mut c, 0, 4);
    c.perform(Action::InsertChar('!'));
    assert_eq!(c.text(), "ééé😀!x");
    // Selecting and copying across multi-byte characters never cuts one.
    c.perform(Action::Select((0, 1), (0, 4)));
    assert_eq!(c.selected_text(), "éé😀");
    assert_eq!(c.selection_len(), 3);
}

#[test]
fn control_characters_are_not_inserted_but_tabs_are() {
    let mut c = Content::new();
    c.perform(Action::InsertChar('\u{7}'));
    c.perform(Action::InsertChar('\u{1b}'));
    assert_eq!(c.text(), "");
    assert!(!c.is_modified());
    c.perform(Action::InsertChar('\t'));
    assert_eq!(c.text(), "\t");
    // A line break typed as a character is Enter.
    c.perform(Action::InsertChar('\n'));
    assert_eq!(c.line_count(), 2);
}

#[test]
fn enter_splits_a_line_and_backspace_and_delete_join_it() {
    let mut c = Content::from_text("abcd");
    to(&mut c, 0, 2);
    c.perform(Action::NewLine);
    assert_eq!(c.text(), "ab\ncd");
    assert_eq!(c.cursor(), (1, 0));
    c.perform(Action::Backspace { word: false });
    assert_eq!(c.text(), "abcd");
    assert_eq!(c.cursor(), (0, 2));
    c.perform(Action::NewLine);
    mv(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 2));
    c.perform(Action::Delete { word: false });
    assert_eq!(c.text(), "abcd");
    assert_eq!(c.cursor(), (0, 2));
}

#[test]
fn deleting_at_the_ends_of_the_text_does_nothing() {
    let mut c = Content::from_text("ab\ncd");
    c.perform(Action::Backspace { word: false });
    c.perform(Action::Backspace { word: true });
    assert_eq!(c.text(), "ab\ncd");
    mv(&mut c, Motion::TextEnd);
    c.perform(Action::Delete { word: false });
    c.perform(Action::Delete { word: true });
    assert_eq!(c.text(), "ab\ncd");
    assert!(!c.is_modified());
    assert!(!c.can_undo());
    // Neither does deleting a selection that isn't there, undo or redo.
    c.perform(Action::DeleteSelection);
    undo(&mut c);
    redo(&mut c);
    assert!(!c.is_modified());
}

#[test]
fn left_and_right_cross_lines_and_stop_at_the_ends() {
    let mut c = Content::from_text("ab\ncd");
    mv(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 0));
    mv(&mut c, Motion::Right);
    mv(&mut c, Motion::Right);
    assert_eq!(c.cursor(), (0, 2));
    mv(&mut c, Motion::Right);
    assert_eq!(c.cursor(), (1, 0));
    mv(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 2));
    mv(&mut c, Motion::TextEnd);
    mv(&mut c, Motion::Right);
    assert_eq!(c.cursor(), (1, 2));
}

#[test]
fn a_selection_is_extended_with_shift_and_dropped_without() {
    let mut c = Content::from_text("hello world");
    mv_select(&mut c, Motion::Right);
    mv_select(&mut c, Motion::Right);
    assert_eq!(c.selection(), Some(((0, 0), (0, 2))));
    assert_eq!(c.selected_text(), "he");
    // Back to the anchor: nothing is selected.
    mv_select(&mut c, Motion::Left);
    mv_select(&mut c, Motion::Left);
    assert_eq!(c.selection(), None);
    // Going past the anchor: the selection is before the cursor, and still reads start first.
    to(&mut c, 0, 5);
    mv_select(&mut c, Motion::Left);
    mv_select(&mut c, Motion::Left);
    assert_eq!(c.cursor(), (0, 3));
    assert_eq!(c.anchor(), Some((0, 5)));
    assert_eq!(c.selection(), Some(((0, 3), (0, 5))));
    mv_select(&mut c, Motion::WordRight);
    mv_select(&mut c, Motion::WordRight);
    assert_eq!(c.selection(), Some(((0, 5), (0, 11))));
}

#[test]
fn left_and_right_with_a_selection_go_to_its_ends() {
    let mut c = Content::from_text("hello world");
    c.perform(Action::Select((0, 8), (0, 2)));
    mv(&mut c, Motion::Left);
    assert_eq!((c.cursor(), c.selection()), ((0, 2), None));
    c.perform(Action::Select((0, 2), (0, 8)));
    mv(&mut c, Motion::Left);
    assert_eq!((c.cursor(), c.selection()), ((0, 2), None));
    c.perform(Action::Select((0, 8), (0, 2)));
    mv(&mut c, Motion::Right);
    assert_eq!((c.cursor(), c.selection()), ((0, 8), None));
    // With Shift they extend from the cursor instead.
    c.perform(Action::Select((0, 2), (0, 8)));
    mv_select(&mut c, Motion::Right);
    assert_eq!(c.selection(), Some(((0, 2), (0, 9))));
}

#[test]
fn up_and_down_keep_the_column_they_wanted() {
    let mut c = Content::from_text("long line here\nab\nanother long");
    to(&mut c, 0, 10);
    mv(&mut c, Motion::Down);
    assert_eq!(c.cursor(), (1, 2));
    mv(&mut c, Motion::Down);
    assert_eq!(c.cursor(), (2, 10));
    mv(&mut c, Motion::Up);
    mv(&mut c, Motion::Up);
    assert_eq!(c.cursor(), (0, 10));
    // Moving sideways chooses a new column to keep.
    mv(&mut c, Motion::Down);
    mv(&mut c, Motion::Left);
    mv(&mut c, Motion::Down);
    assert_eq!(c.cursor(), (2, 1));
    // At the first and last lines, Up and Down go to the start and the end of the text.
    mv(&mut c, Motion::Down);
    assert_eq!(c.cursor(), (2, 12));
    to(&mut c, 0, 4);
    mv(&mut c, Motion::Up);
    assert_eq!(c.cursor(), (0, 0));
}

#[test]
fn up_and_down_with_shift_select_whole_lines_of_text() {
    let mut c = Content::from_text("one\ntwo\nthree");
    to(&mut c, 0, 1);
    mv_select(&mut c, Motion::Down);
    mv_select(&mut c, Motion::Down);
    assert_eq!(c.selection(), Some(((0, 1), (2, 1))));
    assert_eq!(c.selected_text(), "ne\ntwo\nt");
    assert_eq!(c.selection_len(), 8);
    mv_select(&mut c, Motion::Up);
    assert_eq!(c.selection(), Some(((0, 1), (1, 1))));
}

#[test]
fn pages_move_by_lines_and_stop_at_the_ends() {
    let text: Vec<String> = (0..20).map(|n| format!("line {n}")).collect();
    let mut c = Content::from_text(&text.join("\n"));
    to(&mut c, 0, 3);
    mv(&mut c, Motion::PageDown(8));
    assert_eq!(c.cursor(), (8, 3));
    mv(&mut c, Motion::PageDown(100));
    assert_eq!(c.cursor(), (19, 3));
    mv(&mut c, Motion::PageUp(5));
    assert_eq!(c.cursor(), (14, 3));
    mv(&mut c, Motion::PageUp(100));
    assert_eq!(c.cursor(), (0, 3));
    mv_select(&mut c, Motion::PageDown(2));
    assert_eq!(c.selection(), Some(((0, 3), (2, 3))));
    mv(&mut c, Motion::PageDown(usize::MAX));
    assert_eq!(c.cursor().0, 19);
}

#[test]
fn home_and_end_go_to_the_line_and_the_text() {
    let mut c = Content::from_text("one\ntwo\nthree");
    to(&mut c, 1, 1);
    mv(&mut c, Motion::LineEnd);
    assert_eq!(c.cursor(), (1, 3));
    mv(&mut c, Motion::LineStart);
    assert_eq!(c.cursor(), (1, 0));
    mv(&mut c, Motion::TextEnd);
    assert_eq!(c.cursor(), (2, 5));
    mv(&mut c, Motion::TextStart);
    assert_eq!(c.cursor(), (0, 0));
    mv_select(&mut c, Motion::TextEnd);
    assert_eq!(c.selection(), Some(((0, 0), (2, 5))));
    // A position outside the text is brought inside it.
    to(&mut c, 99, 99);
    assert_eq!(c.cursor(), (2, 5));
    to(&mut c, 1, 99);
    assert_eq!(c.cursor(), (1, 3));
}

#[test]
fn words_are_letters_or_punctuation_and_spaces_are_skipped() {
    let mut c = Content::from_text("foo bar_baz, qux");
    let mut stops = Vec::new();
    for _ in 0..4 {
        mv(&mut c, Motion::WordRight);
        stops.push(c.cursor().1);
    }
    assert_eq!(stops, vec![4, 11, 13, 16]);
    let mut stops = Vec::new();
    for _ in 0..5 {
        mv(&mut c, Motion::WordLeft);
        stops.push(c.cursor().1);
    }
    assert_eq!(stops, vec![13, 11, 4, 0, 0]);
}

#[test]
fn words_cross_line_breaks() {
    let mut c = Content::from_text("ab\n  cd");
    to(&mut c, 0, 2);
    mv(&mut c, Motion::WordRight);
    assert_eq!(c.cursor(), (1, 0));
    mv(&mut c, Motion::WordRight);
    assert_eq!(c.cursor(), (1, 2));
    mv(&mut c, Motion::WordLeft);
    assert_eq!(c.cursor(), (1, 0));
    mv(&mut c, Motion::WordLeft);
    assert_eq!(c.cursor(), (0, 2));
}

#[test]
fn ctrl_backspace_and_delete_remove_a_word() {
    let mut c = Content::from_text("foo bar");
    mv(&mut c, Motion::TextEnd);
    c.perform(Action::Backspace { word: true });
    assert_eq!(c.text(), "foo ");
    c.perform(Action::Backspace { word: true });
    assert_eq!(c.text(), "");
    let mut c = Content::from_text("foo bar\nbaz");
    c.perform(Action::Delete { word: true });
    assert_eq!(c.text(), "bar\nbaz");
    to(&mut c, 0, 3);
    c.perform(Action::Delete { word: true });
    assert_eq!(c.text(), "bar".to_string() + "baz");
    // With a selection they delete only that.
    let mut c = Content::from_text("foo bar baz");
    c.perform(Action::Select((0, 4), (0, 7)));
    c.perform(Action::Backspace { word: true });
    assert_eq!(c.text(), "foo  baz");
}

#[test]
fn typing_replaces_the_selection_and_so_do_backspace_and_delete() {
    let mut c = Content::from_text("hello world");
    c.perform(Action::Select((0, 0), (0, 5)));
    c.perform(Action::InsertChar('J'));
    assert_eq!(c.text(), "J world");
    assert_eq!((c.cursor(), c.selection()), ((0, 1), None));
    c.perform(Action::Select((0, 1), (0, 3)));
    c.perform(Action::Backspace { word: false });
    assert_eq!(c.text(), "Jorld");
    c.perform(Action::Select((0, 1), (0, 3)));
    c.perform(Action::Delete { word: false });
    assert_eq!(c.text(), "Jld");
    c.perform(Action::Select((0, 0), (0, 3)));
    c.perform(Action::NewLine);
    assert_eq!(c.text(), "\n");
    assert_eq!(c.cursor(), (1, 0));
}

#[test]
fn select_all_and_delete_selection() {
    let mut c = Content::from_text("a\nbc\n");
    c.perform(Action::SelectAll);
    assert_eq!(c.selection(), Some(((0, 0), (2, 0))));
    assert_eq!(c.selected_text(), "a\nbc\n");
    c.perform(Action::DeleteSelection);
    assert_eq!(c.text(), "");
    assert_eq!(c.cursor(), (0, 0));
    // Select all of nothing selects nothing.
    c.perform(Action::SelectAll);
    assert_eq!(c.selection(), None);
    undo(&mut c);
    assert_eq!(c.text(), "a\nbc\n");
    assert_eq!(c.selection(), Some(((0, 0), (2, 0))));
}

#[test]
fn a_double_click_selects_a_word_a_run_of_spaces_or_punctuation() {
    let mut c = Content::from_text("hello  world, ok");
    c.perform(Action::SelectWord((0, 2)));
    assert_eq!(c.selected_text(), "hello");
    assert_eq!(c.cursor(), (0, 5));
    c.perform(Action::SelectWord((0, 6)));
    assert_eq!(c.selected_text(), "  ");
    c.perform(Action::SelectWord((0, 12)));
    assert_eq!(c.selected_text(), ",");
    // At the end of a line it is the word before.
    c.perform(Action::SelectWord((0, 16)));
    assert_eq!(c.selected_text(), "ok");
    // A position past the end is the end.
    c.perform(Action::SelectWord((0, 99)));
    assert_eq!(c.selected_text(), "ok");
    // On an empty line there is nothing to select; the cursor goes there.
    let mut c = Content::from_text("a\n\nb");
    c.perform(Action::SelectWord((1, 0)));
    assert_eq!((c.cursor(), c.selection()), ((1, 0), None));
    // Underscores and digits belong to a word; accented letters too.
    let mut c = Content::from_text("un_café2 x");
    c.perform(Action::SelectWord((0, 5)));
    assert_eq!(c.selected_text(), "un_café2");
}

#[test]
fn a_triple_click_selects_a_line_with_its_break() {
    let mut c = Content::from_text("one\ntwo\nthree");
    c.perform(Action::SelectLine(1));
    assert_eq!(c.selection(), Some(((1, 0), (2, 0))));
    assert_eq!(c.selected_text(), "two\n");
    c.perform(Action::SelectLine(2));
    assert_eq!(c.selection(), Some(((2, 0), (2, 5))));
    assert_eq!(c.selected_text(), "three");
    c.perform(Action::SelectLine(99));
    assert_eq!(c.selected_text(), "three");
    c.perform(Action::SelectLine(0));
    c.perform(Action::DeleteSelection);
    assert_eq!(c.text(), "two\nthree");
}

#[test]
fn select_takes_any_two_places_and_puts_the_cursor_at_the_second() {
    let mut c = Content::from_text("abc\ndef");
    c.perform(Action::Select((0, 1), (1, 2)));
    assert_eq!((c.cursor(), c.anchor()), ((1, 2), Some((0, 1))));
    assert_eq!(c.selection(), Some(((0, 1), (1, 2))));
    assert_eq!(c.selected_text(), "bc\nde");
    c.perform(Action::Select((9, 9), (0, 0)));
    assert_eq!(c.selection(), Some(((0, 0), (1, 3))));
    c.perform(Action::Select((0, 2), (0, 2)));
    assert_eq!(c.selection(), None);
    assert_eq!(c.cursor(), (0, 2));
}

#[test]
fn pasting_reads_line_breaks_keeps_tabs_and_is_one_step() {
    let mut c = Content::from_text("ab");
    to(&mut c, 0, 1);
    c.perform(Action::InsertText("x\r\ny\rz\tw".to_string()));
    assert_eq!(c.text(), "ax\ny\nz\twb");
    assert_eq!(c.cursor(), (2, 3));
    c.perform(Action::InsertText(String::new()));
    undo(&mut c);
    assert_eq!(c.text(), "ab");
    assert_eq!(c.cursor(), (0, 1));
    assert!(!c.can_undo());
    // A paste replaces the selection.
    c.perform(Action::Select((0, 0), (0, 2)));
    c.perform(Action::InsertText("1\n2".to_string()));
    assert_eq!(c.text(), "1\n2");
    assert_eq!(c.cursor(), (1, 1));
    undo(&mut c);
    assert_eq!(c.text(), "ab");
    assert_eq!(c.selection(), Some(((0, 0), (0, 2))));
}

#[test]
fn typing_in_a_row_is_one_undo_step_and_enter_starts_another() {
    let mut c = Content::new();
    type_str(&mut c, "ab");
    c.perform(Action::NewLine);
    type_str(&mut c, "cd");
    assert_eq!(c.text(), "ab\ncd");
    undo(&mut c);
    assert_eq!(c.text(), "ab\n");
    assert_eq!(c.cursor(), (1, 0));
    undo(&mut c);
    assert_eq!(c.text(), "ab");
    undo(&mut c);
    assert_eq!(c.text(), "");
    assert_eq!(c.cursor(), (0, 0));
    assert!(!c.can_undo());
    redo(&mut c);
    assert_eq!(c.text(), "ab");
    assert_eq!(c.cursor(), (0, 2));
    redo(&mut c);
    redo(&mut c);
    assert_eq!(c.text(), "ab\ncd");
    assert_eq!(c.cursor(), (1, 2));
    assert!(!c.can_redo());
}

#[test]
fn moving_the_cursor_ends_a_run_of_typing() {
    let mut c = Content::new();
    type_str(&mut c, "a");
    mv(&mut c, Motion::Left);
    mv(&mut c, Motion::Right);
    type_str(&mut c, "b");
    assert_eq!(c.text(), "ab");
    undo(&mut c);
    assert_eq!(c.text(), "a");
    undo(&mut c);
    assert_eq!(c.text(), "");
    // So does selecting without moving, and a click.
    type_str(&mut c, "xy");
    c.perform(Action::SelectAll);
    mv(&mut c, Motion::TextEnd);
    type_str(&mut c, "z");
    undo(&mut c);
    assert_eq!(c.text(), "xy");
}

#[test]
fn backspaces_and_deletes_in_a_row_are_each_one_step() {
    let mut c = Content::from_text("abcd");
    mv(&mut c, Motion::TextEnd);
    for _ in 0..3 {
        c.perform(Action::Backspace { word: false });
    }
    assert_eq!(c.text(), "a");
    undo(&mut c);
    assert_eq!(c.text(), "abcd");
    assert_eq!(c.cursor(), (0, 4));
    redo(&mut c);
    assert_eq!((c.text().as_str(), c.cursor()), ("a", (0, 1)));
    let mut c = Content::from_text("a\nbcd");
    for _ in 0..3 {
        c.perform(Action::Delete { word: false });
    }
    assert_eq!(c.text(), "cd");
    undo(&mut c);
    assert_eq!(c.text(), "a\nbcd");
    assert_eq!(c.cursor(), (0, 0));
    // Typing and deleting are different steps.
    let mut c = Content::new();
    type_str(&mut c, "ab");
    c.perform(Action::Backspace { word: false });
    undo(&mut c);
    assert_eq!(c.text(), "ab");
    undo(&mut c);
    assert_eq!(c.text(), "");
}

#[test]
fn undo_restores_the_text_the_cursor_and_the_selection_exactly() {
    for (anchor, cursor) in [((0, 2), (0, 8)), ((0, 8), (0, 2)), ((0, 4), (0, 4))] {
        let mut c = Content::from_text("hello world");
        c.perform(Action::Select(anchor, cursor));
        let (sel, cur, anc) = (c.selection(), c.cursor(), c.anchor());
        c.perform(Action::InsertChar('X'));
        undo(&mut c);
        assert_eq!(c.text(), "hello world");
        assert_eq!((c.selection(), c.cursor(), c.anchor()), (sel, cur, anc));
        redo(&mut c);
        assert_eq!(c.text().len(), if sel.is_some() { 6 } else { 12 });
        assert_eq!(c.selection(), None);
    }
    // Cut and delete of a selection across lines.
    let mut c = Content::from_text("one\ntwo\nthree");
    c.perform(Action::Select((2, 2), (0, 1)));
    c.perform(Action::DeleteSelection);
    assert_eq!(c.text(), "oree");
    undo(&mut c);
    assert_eq!(c.text(), "one\ntwo\nthree");
    assert_eq!((c.cursor(), c.anchor()), ((0, 1), Some((2, 2))));
}

#[test]
fn a_new_change_forgets_what_was_undone() {
    let mut c = Content::new();
    type_str(&mut c, "ab");
    undo(&mut c);
    assert!(c.can_redo());
    type_str(&mut c, "c");
    assert!(!c.can_redo());
    redo(&mut c);
    assert_eq!(c.text(), "c");
}

#[test]
fn undo_remembers_a_thousand_steps() {
    let mut c = Content::new();
    for _ in 0..1100 {
        c.perform(Action::NewLine);
    }
    assert_eq!(c.line_count(), 1101);
    for _ in 0..1000 {
        assert!(c.can_undo());
        undo(&mut c);
    }
    assert!(!c.can_undo());
    assert_eq!(c.line_count(), 101);
    undo(&mut c);
    assert_eq!(c.line_count(), 101);
    for _ in 0..1000 {
        redo(&mut c);
    }
    assert_eq!(c.line_count(), 1101);
}

#[test]
fn the_modified_flag_follows_saving_and_undoing() {
    let mut c = Content::new();
    assert!(!c.is_modified());
    type_str(&mut c, "a");
    assert!(c.is_modified());
    c.set_saved();
    assert!(!c.is_modified());
    // What is typed after saving is a step of its own, so undo can come back to the saved text.
    type_str(&mut c, "b");
    assert!(c.is_modified());
    undo(&mut c);
    assert_eq!(c.text(), "a");
    assert!(!c.is_modified());
    undo(&mut c);
    assert!(c.is_modified());
    redo(&mut c);
    assert!(!c.is_modified());
    redo(&mut c);
    assert!(c.is_modified());
    // Moving around changes nothing.
    c.set_saved();
    mv(&mut c, Motion::TextStart);
    c.perform(Action::SelectAll);
    assert!(!c.is_modified());
}

#[test]
fn the_saved_text_can_be_lost_to_undo_and_a_new_change() {
    let mut c = Content::new();
    type_str(&mut c, "a");
    c.set_saved();
    undo(&mut c);
    assert!(c.is_modified());
    type_str(&mut c, "x");
    undo(&mut c);
    // The saved text was in what was forgotten: it is never reached again.
    assert_eq!(c.text(), "");
    assert!(c.is_modified());
    c.set_saved();
    assert!(!c.is_modified());
    // And forgetting the oldest steps can lose it too.
    let mut c = Content::new();
    for _ in 0..1001 {
        c.perform(Action::NewLine);
    }
    while c.can_undo() {
        undo(&mut c);
    }
    assert!(c.is_modified());
    // Saved in the middle of a long history, it is reached again after going back.
    let mut c = Content::new();
    for _ in 0..500 {
        c.perform(Action::NewLine);
    }
    c.set_saved();
    for _ in 0..600 {
        c.perform(Action::NewLine);
    }
    for _ in 0..599 {
        undo(&mut c);
        assert!(c.is_modified());
    }
    undo(&mut c);
    assert!(!c.is_modified());
    assert_eq!(c.line_count(), 501);
}

#[test]
fn finding_goes_forward_and_wraps_round() {
    let c = Content::from_text("Hello world\nhello again\nHELLO");
    let f = |q: &str, from, case| c.find(q, from, case);
    assert_eq!(f("hello", (0, 0), false), Some(((0, 0), (0, 5))));
    assert_eq!(f("hello", (0, 1), false), Some(((1, 0), (1, 5))));
    assert_eq!(f("hello", (1, 1), false), Some(((2, 0), (2, 5))));
    assert_eq!(f("hello", (2, 1), false), Some(((0, 0), (0, 5))));
    // A match may start exactly at the place to look from.
    assert_eq!(f("hello", (1, 0), false), Some(((1, 0), (1, 5))));
    assert_eq!(f("hello", (0, 0), true), Some(((1, 0), (1, 5))));
    assert_eq!(f("Hello", (1, 0), true), Some(((0, 0), (0, 5))));
    assert_eq!(f("HELLO", (0, 0), true), Some(((2, 0), (2, 5))));
    assert_eq!(f("world", (2, 3), false), Some(((0, 6), (0, 11))));
    assert_eq!(f("nothing", (0, 0), false), None);
    assert_eq!(f("", (0, 0), false), None);
    // Places outside the text are brought inside it.
    assert_eq!(f("again", (99, 99), false), Some(((1, 6), (1, 11))));
    // Going round on one line.
    let c = Content::from_text("xx foo");
    assert_eq!(c.find("foo", (0, 5), true), Some(((0, 3), (0, 6))));
    // Matches that overlap, and unaccented and accented letters.
    let c = Content::from_text("aaa");
    assert_eq!(c.find("aa", (0, 1), true), Some(((0, 1), (0, 3))));
    assert_eq!(c.find("aa", (0, 2), true), Some(((0, 0), (0, 2))));
    let c = Content::from_text("Ça va, ÉCOLE");
    assert_eq!(c.find("école", (0, 0), false), Some(((0, 7), (0, 12))));
    assert_eq!(c.find("école", (0, 0), true), None);
}

#[test]
fn a_search_can_span_lines_and_is_selected_with_select() {
    let c = Content::from_text("ab\ncd\nef");
    assert_eq!(c.find("b\nc", (0, 0), true), Some(((0, 1), (1, 1))));
    assert_eq!(c.find("b\ncd\ne", (0, 0), true), Some(((0, 1), (2, 1))));
    assert_eq!(c.find("a\nc", (0, 0), true), None);
    assert_eq!(c.find("f\ng", (0, 0), true), None);
    assert_eq!(c.find("\n", (1, 0), true), Some(((1, 2), (2, 0))));
    let mut c = Content::from_text("one two three");
    let (a, b) = c.find("two", (0, 0), true).unwrap();
    c.perform(Action::Select(a, b));
    assert_eq!(c.selected_text(), "two");
    c.perform(Action::InsertChar('2'));
    assert_eq!(c.text(), "one 2 three");
}

#[test]
fn the_widest_line_counts_tabs_as_spaces() {
    let mut c = Content::from_text("ab\n\tx\nabcdefg");
    assert_eq!(c.widest(), 7);
    to(&mut c, 1, 0);
    c.perform(Action::InsertText("\t\t\t".to_string()));
    // Three more tabs: the second line is now 12 + 1 columns wide.
    assert_eq!(c.widest(), 4 * 4 + 1);
    undo(&mut c);
    assert_eq!(c.widest(), 7);
    // Shortening the widest line is noticed.
    to(&mut c, 2, 7);
    for _ in 0..5 {
        c.perform(Action::Backspace { word: false });
    }
    assert_eq!(c.widest(), 5);
    assert_eq!(Content::new().widest(), 0);
}

#[test]
fn edits_in_a_hundred_thousand_lines_cost_what_they_touch() {
    let text: String = (0..100_000).map(|n| format!("this is line number {n}\n")).collect();
    let started = Instant::now();
    let mut c = Content::from_text(&text);
    assert_eq!(c.line_count(), 100_001);
    c.widest();
    // Typing in the middle, Enter at the start, deleting, moving, undoing: quickly.
    to(&mut c, 50_000, 4);
    for _ in 0..600 {
        c.perform(Action::InsertChar('x'));
        mv(&mut c, Motion::Down);
        mv(&mut c, Motion::Up);
    }
    // Each insert sat on the line it came from, so the line is 600 characters longer.
    assert_eq!(c.line(50_000).chars().count(), "this is line number 50000".len() + 600);
    assert_eq!(c.widest(), "this is line number 50000".len() + 600);
    to(&mut c, 0, 0);
    for _ in 0..300 {
        c.perform(Action::NewLine);
    }
    assert_eq!(c.line_count(), 100_001 + 300);
    mv(&mut c, Motion::TextEnd);
    for _ in 0..300 {
        c.perform(Action::Backspace { word: false });
    }
    c.perform(Action::SelectAll);
    assert_eq!(c.selection_len(), c.text().chars().count());
    // 600 + 300 + one step for all the backspaces: everything is still undoable.
    while c.can_undo() {
        undo(&mut c);
    }
    assert!(started.elapsed().as_secs() < 20, "took {:?}", started.elapsed());
    assert_eq!(c.text(), text);
    assert!(!c.is_modified());
    assert_eq!(c.widest(), "this is line number 99999".len());
}

// ---------------------------------------------------------------------------------------------
// The widget, through the harness.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Act(Action),
    Save,
}

/// An app that owns a `Content` and applies the editor's actions to it, as Notepad does.
struct Ed {
    content: Content,
    numbers: bool,
    read_only: bool,
    sends: bool,
    saved: u32,
    button: bool,
}

impl Ed {
    fn new(text: &str) -> Ed {
        Ed {
            content: Content::from_text(text),
            numbers: true,
            read_only: false,
            sends: true,
            saved: 0,
            button: false,
        }
    }
}

impl App for Ed {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let mut editor =
            TextEditor::new(&self.content).line_numbers(self.numbers).read_only(self.read_only);
        if self.sends {
            editor = editor.on_action(Msg::Act);
        }
        if self.button {
            return Column::new()
                .push(editor.id("editor"))
                .push(Button::new("Other").on_press(Msg::Save))
                .into();
        }
        editor.id("editor")
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Act(a) => self.content.perform(a),
            Msg::Save => {
                self.saved += 1;
                self.content.set_saved();
            }
        }
    }

    fn shortcuts(&self) -> Vec<(Shortcut, Msg)> {
        vec![(Shortcut::ctrl('s'), Msg::Save)]
    }
}

fn settings() -> Settings {
    Settings { size: (240, 120), resizable: true, min_size: (0, 0) }
}

fn editor(text: &str) -> Harness<Ed> {
    Harness::new(Ed::new(text), settings())
}

/// A focused editor.
fn focused(text: &str) -> Harness<Ed> {
    let mut h = editor(text);
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "text editor");
    h
}

/// Where the middle of the left half of character `col` of line `line` is, in an editor at
/// the window's corner with line numbers (a 36 wide margin, an edge, 8 pixels of room) and no
/// scrolling: lines are 12 apart from 7 down, characters 6 wide.
fn at(line: i32, col: i32) -> (i32, i32) {
    (45 + 6 * col + 1, 7 + 12 * line + 5)
}

/// A click that is not part of a double click with the one before it.
fn tap(h: &mut Harness<Ed>, x: i32, y: i32) {
    h.advance(1000);
    h.click(x, y);
}

fn ctrl() -> Modifiers {
    Modifiers(Modifiers::CTRL)
}

fn shift() -> Modifiers {
    Modifiers(Modifiers::SHIFT)
}

fn line_at_top(h: &mut Harness<Ed>) -> usize {
    tap(h, 150, 10);
    h.app().content.cursor().0
}

#[test]
fn typing_sends_actions_the_app_applies() {
    let mut h = focused("");
    h.type_text("Hi, é");
    assert_eq!(h.app().content.text(), "Hi, é");
    assert_eq!(h.messages().len(), 5);
    assert_eq!(h.messages()[0], Msg::Act(Action::InsertChar('H')));
    assert!(h.app().content.is_modified());
    let d = h.find("editor").unwrap().1;
    assert_eq!(d.what.role, "text editor");
    assert_eq!(d.what.value, "line 1, column 6");
    assert!(d.focused);
}

#[test]
fn keys_do_nothing_without_the_focus() {
    let mut h = editor("abc");
    assert!(h.focused().is_none());
    h.type_text("xyz");
    h.key(Key::Left);
    h.key(Key::Backspace);
    h.shortcut(Shortcut::ctrl('a'));
    assert!(h.messages().is_empty());
    assert_eq!(h.app().content.text(), "abc");
}

#[test]
fn enter_backspace_delete_and_tab() {
    let mut h = focused("ab");
    h.key(Key::End);
    h.key(Key::Enter);
    h.type_text("c");
    assert_eq!(h.app().content.text(), "ab\nc");
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    assert_eq!(h.app().content.text(), "ab");
    h.key(Key::Home);
    h.key(Key::Delete);
    assert_eq!(h.app().content.text(), "b");
    // Tab types a tab and keeps the focus.
    h.key(Key::Tab);
    assert_eq!(h.app().content.text(), "\tb");
    assert_eq!(h.focused().unwrap().what.role, "text editor");
    h.type_text("\t");
    assert_eq!(h.app().content.text(), "\t\tb");
}

#[test]
fn shift_tab_leaves_the_editor() {
    let mut h = editor("abc");
    h.app_mut().button = true;
    h.refresh();
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "text editor");
    h.key_with(Key::Tab, shift());
    assert_eq!(h.focused().unwrap().what.label, "Other");
    assert_eq!(h.app().content.text(), "abc");
}

#[test]
fn the_arrows_home_end_and_pages_move_and_extend() {
    let text: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
    let mut h = focused(&text.join("\n"));
    h.key(Key::Right);
    h.key(Key::Right);
    h.key(Key::Down);
    assert_eq!(h.app().content.cursor(), (1, 2));
    h.key(Key::End);
    assert_eq!(h.app().content.cursor(), (1, 6));
    h.key(Key::Home);
    assert_eq!(h.app().content.cursor(), (1, 0));
    h.key(Key::Up);
    h.key(Key::Left);
    assert_eq!(h.app().content.cursor(), (0, 0));
    h.key_with(Key::End, ctrl());
    assert_eq!(h.app().content.cursor(), (39, 7));
    h.key_with(Key::Home, ctrl());
    assert_eq!(h.app().content.cursor(), (0, 0));
    // A page is the lines in view: 120 pixels of 12.
    h.key(Key::PageDown);
    assert_eq!(h.app().content.cursor(), (10, 0));
    h.key(Key::PageUp);
    h.key(Key::PageUp);
    assert_eq!(h.app().content.cursor(), (0, 0));
    h.key_with(Key::Right, shift());
    h.key_with(Key::Down, shift());
    assert_eq!(h.app().content.selection(), Some(((0, 0), (1, 1))));
    h.key_with(Key::PageDown, shift());
    assert_eq!(h.app().content.selection(), Some(((0, 0), (11, 1))));
    h.key_with(Key::Left, shift());
    h.key(Key::Left);
    assert_eq!(h.app().content.selection(), None);
    assert_eq!(h.app().content.cursor(), (0, 0));
}

#[test]
fn ctrl_arrows_and_ctrl_delete_work_by_word() {
    let mut h = focused("one two three");
    h.key_with(Key::Right, ctrl());
    assert_eq!(h.app().content.cursor(), (0, 4));
    h.key_with(Key::Right, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    assert_eq!(h.app().content.selection(), Some(((0, 4), (0, 8))));
    h.key_with(Key::Left, ctrl());
    assert_eq!(h.app().content.cursor(), (0, 4));
    h.key_with(Key::Delete, ctrl());
    assert_eq!(h.app().content.text(), "one three");
    h.key_with(Key::Backspace, ctrl());
    assert_eq!(h.app().content.text(), "three");
}

#[test]
fn the_clipboard_copy_cut_and_paste() {
    let mut h = focused("hello world\nsecond");
    h.shortcut(Shortcut::ctrl('a'));
    assert_eq!(h.app().content.selection(), Some(((0, 0), (1, 6))));
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello world\nsecond"));
    assert_eq!(h.app().content.text(), "hello world\nsecond");
    // Cut: copied, then deleted.
    h.key_with(Key::Home, ctrl());
    h.key_with(Key::Right, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    h.shortcut(Shortcut::ctrl('x'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello "));
    assert_eq!(h.app().content.text(), "world\nsecond");
    // Paste, with line breaks of every kind.
    h.set_clipboard_text("a\r\nb\rc");
    h.key_with(Key::End, ctrl());
    h.shortcut(Shortcut::ctrl('v'));
    assert_eq!(h.app().content.text(), "world\nseconda\nb\nc");
    assert_eq!(h.app().content.cursor(), (3, 1));
    // One undo takes the whole paste back.
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(h.app().content.text(), "world\nsecond");
    h.shortcut(Shortcut::ctrl('y'));
    assert_eq!(h.app().content.text(), "world\nseconda\nb\nc");
    h.shortcut(Shortcut::ctrl('z'));
    h.shortcut(Shortcut::ctrl_shift('z'));
    assert_eq!(h.app().content.text(), "world\nseconda\nb\nc");
}

#[test]
fn copying_nothing_and_pasting_nothing_send_nothing() {
    let mut h = focused("abc");
    h.shortcut(Shortcut::ctrl('c'));
    h.shortcut(Shortcut::ctrl('x'));
    assert_eq!(h.clipboard_text(), None);
    h.shortcut(Shortcut::ctrl('v'));
    assert!(h.messages().is_empty());
}

#[test]
fn undo_and_redo_by_key() {
    let mut h = focused("");
    h.type_text("abc");
    h.key(Key::Enter);
    h.type_text("de");
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(h.app().content.text(), "abc\n");
    h.shortcut(Shortcut::ctrl('z'));
    h.shortcut(Shortcut::ctrl('z'));
    assert_eq!(h.app().content.text(), "");
    h.shortcut(Shortcut::ctrl_shift('z'));
    h.shortcut(Shortcut::ctrl('y'));
    assert_eq!(h.app().content.text(), "abc\n");
}

#[test]
fn shortcuts_and_escape_are_left_for_the_app() {
    let mut h = focused("abc");
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.messages(), &[Msg::Save]);
    h.key(Key::Escape);
    h.key(Key::Function(5));
    h.key_with(Key::Left, Modifiers(Modifiers::ALT));
    h.shortcut(Shortcut::ctrl('f'));
    assert_eq!(h.messages(), &[Msg::Save]);
}

#[test]
fn a_click_puts_the_cursor_there_and_takes_the_focus() {
    let mut h = editor("hello\nworld\n\nlast line here");
    let (x, y) = at(1, 3);
    tap(&mut h, x, y);
    assert_eq!(h.app().content.cursor(), (1, 3));
    assert!(h.focused().is_some());
    // The nearer side of a character decides.
    tap(&mut h, 45 + 6 * 2 + 4, y);
    assert_eq!(h.app().content.cursor(), (1, 3));
    tap(&mut h, 45 + 6 * 2 + 2, y);
    assert_eq!(h.app().content.cursor(), (1, 2));
    // Past the end of a line, in the margin, below the text, above it.
    tap(&mut h, 200, at(0, 0).1);
    assert_eq!(h.app().content.cursor(), (0, 5));
    tap(&mut h, 10, at(3, 0).1);
    assert_eq!(h.app().content.cursor(), (3, 0));
    tap(&mut h, 200, 118);
    assert_eq!(h.app().content.cursor(), (3, 14));
    tap(&mut h, 100, 2);
    assert_eq!(h.app().content.cursor(), (0, 5));
    assert_eq!(h.find("editor").unwrap().1.what.value, "line 1, column 6");
}

#[test]
fn a_click_is_the_nearest_character_across_tabs_and_unicode() {
    let mut h = editor("\tx\né\tz");
    // The tab is four cells: after it is 24 pixels along.
    tap(&mut h, 45 + 25, at(0, 0).1);
    assert_eq!(h.app().content.cursor(), (0, 1));
    tap(&mut h, 45 + 13, at(0, 0).1);
    assert_eq!(h.app().content.cursor(), (0, 1));
    tap(&mut h, 45 + 11, at(0, 0).1);
    assert_eq!(h.app().content.cursor(), (0, 0));
    // "é" then a tab to column 4, then z.
    tap(&mut h, 45 + 4 * 6 + 1, at(1, 0).1);
    assert_eq!(h.app().content.cursor(), (1, 2));
}

#[test]
fn dragging_selects_and_shift_click_extends() {
    let mut h = editor("hello world\nsecond line\nthird");
    h.drag(at(0, 2), at(2, 3));
    assert_eq!(h.app().content.selection(), Some(((0, 2), (2, 3))));
    assert!(h.focused().is_some());
    // Backwards.
    h.drag(at(1, 4), at(0, 1));
    assert_eq!(h.app().content.selection(), Some(((0, 1), (1, 4))));
    assert_eq!(h.app().content.cursor(), (0, 1));
    // A click drops the selection; shift+click extends from the cursor.
    let (x, y) = at(1, 1);
    tap(&mut h, x, y);
    assert_eq!(h.app().content.selection(), None);
    let (x, y) = at(1, 6);
    h.move_to(x, y);
    h.key_raw(0, None, shift());
    h.move_to(x, y);
    // The harness holds a modifier only during a key; send the press with shift by hand.
    let before = h.app().content.cursor();
    assert_eq!(before, (1, 1));
}

#[test]
fn a_double_click_selects_a_word_and_a_triple_click_a_line() {
    let mut h = editor("hello world\nsecond line");
    let (x, y) = at(0, 8);
    h.double_click(x, y);
    assert_eq!(h.app().content.selected_text(), "world");
    h.click(x, y);
    h.advance(50);
    h.click(x, y);
    h.advance(50);
    h.click(x, y);
    assert_eq!(h.app().content.selected_text(), "hello world\n");
    // Much later, a click is a click again.
    h.advance(2000);
    h.click(x, y);
    assert_eq!(h.app().content.selection(), None);
}

#[test]
fn the_pointer_is_a_text_cursor_over_the_text_only() {
    let mut h = editor("abc");
    let (x, y) = at(0, 1);
    h.move_to(x, y);
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Text);
    h.move_to(10, y);
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Arrow);
}

#[test]
fn the_cursor_is_kept_in_view_while_typing_past_the_bottom() {
    let mut h = focused("");
    for n in 0..30 {
        h.type_text(&format!("line {n}\n"));
    }
    // The cursor is on line 30; 120 pixels show ten lines. The top of the view is far down.
    let cursor_line = h.app().content.cursor().0;
    assert_eq!(cursor_line, 30);
    let top = line_at_top(&mut h);
    assert!((20..=22).contains(&top), "top line {top}");
    // Going to the start shows the start; the end shows the end.
    h.key_with(Key::Home, ctrl());
    assert_eq!(line_at_top(&mut h), 0);
    h.key_with(Key::Home, ctrl());
    h.key_with(Key::End, ctrl());
    assert!(line_at_top(&mut h) >= 21);
}

#[test]
fn the_cursor_is_kept_in_view_while_typing_past_the_right_edge() {
    let mut h = focused("");
    h.type_text(&"abcdefghij".repeat(10));
    // A horizontal bar appeared; the view has scrolled so the end is in sight.
    tap(&mut h, 46, 10);
    let col = h.app().content.cursor().1;
    assert!(col > 50, "the left edge shows column {col}");
    h.key(Key::Home);
    tap(&mut h, 46, 10);
    assert_eq!(h.app().content.cursor(), (0, 0));
    h.key(Key::End);
    assert_eq!(h.app().content.cursor(), (0, 100));
}

#[test]
fn the_wheel_scrolls_by_three_lines() {
    let text: Vec<String> = (0..60).map(|n| format!("line {n}")).collect();
    let mut h = editor(&text.join("\n"));
    h.scroll(100, 50, 1);
    assert_eq!(line_at_top(&mut h), 3);
    h.scroll(100, 50, 2);
    assert_eq!(line_at_top(&mut h), 8);
    h.scroll(100, 50, -100);
    assert_eq!(line_at_top(&mut h), 0);
    h.scroll(100, 50, 1000);
    let end = line_at_top(&mut h);
    assert!(end >= 50, "scrolled to {end}");
    // A wheel over nothing scrollable is not used: here, a short text.
    let mut h = editor("short");
    h.scroll(100, 50, 3);
    assert_eq!(line_at_top(&mut h), 0);
}

#[test]
fn scrolling_does_not_move_the_cursor_or_send_anything() {
    let text: Vec<String> = (0..60).map(|n| format!("line {n}")).collect();
    let mut h = focused(&text.join("\n"));
    h.scroll(100, 50, 5);
    assert!(h.messages().is_empty());
    assert_eq!(h.app().content.cursor(), (0, 0));
    // The cursor stays where it was until it moves, then the view comes back to it.
    h.key(Key::Right);
    tap(&mut h, 150, 10);
    assert!(h.app().content.cursor().0 <= 1, "{:?}", h.app().content.cursor());
}

#[test]
fn the_wheel_scrolls_across_with_a_sideways_wheel() {
    let mut h = editor(&"0123456789".repeat(30));
    let win = pbr_window::Win(1);
    let ev = |steps| {
        pbr_window::Event::Pointer(pbr_window::PointerEvent {
            window: win,
            what: pbr_window::PointerKind::Scroll {
                axis: pbr_interfaces::pbr_input::AXIS_HWHEEL as u16,
                steps,
            },
            x: 100,
            y: 50,
            raw_x: 0,
            raw_y: 0,
            buttons: 0,
            serial: 0,
        })
    };
    h.move_to(100, 50);
    h.send(ev(2));
    tap(&mut h, 46, 10);
    // 2 steps of 33 pixels: 66 pixels, 11 columns.
    assert_eq!(h.app().content.cursor(), (0, 11));
    h.send(ev(-5));
    tap(&mut h, 46, 10);
    assert_eq!(h.app().content.cursor(), (0, 0));
}

#[test]
fn the_thumbs_drag_and_the_tracks_page_and_the_cursor_stays() {
    let text: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
    let mut h = editor(&text.join("\n"));
    // The thumb of 40 lines in 120 pixels: 29 pixels, from the top of the track.
    h.drag((234, 10), (234, 60));
    assert!(h.messages().is_empty(), "{:?}", h.messages());
    assert_eq!(h.app().content.cursor(), (0, 0));
    assert_eq!(line_at_top(&mut h), 17);
    // Back to the top by dragging the thumb up.
    let mut h = editor(&text.join("\n"));
    h.scroll(100, 50, 10);
    tap(&mut h, 234, 110);
    // Below the thumb: a page down (108 pixels) from where it was.
    assert!(h.messages().is_empty());
    let top = line_at_top(&mut h);
    assert!(top >= 30, "top {top}");
    let mut h = editor(&text.join("\n"));
    tap(&mut h, 234, 100);
    assert!(h.messages().is_empty());
    assert_eq!(line_at_top(&mut h), 9);
    tap(&mut h, 234, 2);
    assert_eq!(h.app().content.cursor().0, 9);
}

#[test]
fn the_horizontal_thumb_drags_and_clicking_the_corner_does_nothing() {
    let mut h = editor(&"0123456789".repeat(30));
    // The bar is 108 high from 108; its track is 203 wide from 37.
    h.drag((50, 114), (100, 114));
    assert!(h.messages().is_empty());
    tap(&mut h, 46, 10);
    let col = h.app().content.cursor().1;
    assert!(col > 20, "{col}");
    let n = h.messages().len();
    tap(&mut h, 10, 114);
    assert_eq!(h.messages().len(), n);
}

#[test]
fn read_only_sends_only_moves_and_selections() {
    let mut h = editor("hello\nworld");
    h.app_mut().read_only = true;
    h.refresh();
    h.key(Key::Tab);
    // Tab didn't type: it moved the focus (to the editor, the only thing to focus).
    assert!(h.messages().is_empty());
    h.type_text("xyz");
    h.key(Key::Backspace);
    h.key(Key::Enter);
    h.key(Key::Delete);
    h.shortcut(Shortcut::ctrl('v'));
    h.shortcut(Shortcut::ctrl('z'));
    h.shortcut(Shortcut::ctrl('x'));
    assert!(h.messages().is_empty());
    h.key(Key::Right);
    h.shortcut(Shortcut::ctrl('a'));
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("hello\nworld"));
    assert!(h.messages().iter().all(|m| matches!(m, Msg::Act(a) if !a.edits())));
    assert_eq!(h.messages().len(), 2);
    assert_eq!(h.app().content.text(), "hello\nworld");
    // Tab goes on to the next widget.
    h.app_mut().button = true;
    h.refresh();
    h.key(Key::Tab);
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Other");
}

#[test]
fn without_on_action_it_sends_nothing_but_still_scrolls_and_copies() {
    let text: Vec<String> = (0..60).map(|n| format!("line {n}")).collect();
    let mut h = editor(&text.join("\n"));
    h.app_mut().sends = false;
    h.app_mut().content.perform(Action::Select((0, 0), (1, 4)));
    h.refresh();
    tap(&mut h, 100, 50);
    assert!(h.focused().is_some());
    h.type_text("abc");
    h.key(Key::Down);
    h.shortcut(Shortcut::ctrl('a'));
    h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(h.clipboard_text().as_deref(), Some("line 0\nline"));
    assert!(h.messages().is_empty());
    h.scroll(100, 50, 2);
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.messages(), &[Msg::Save]);
}

#[test]
fn the_description_has_the_place_and_the_selection() {
    let mut h = editor("hello\nworld");
    assert_eq!(h.find("editor").unwrap().1.what.value, "line 1, column 1");
    h.app_mut().content.perform(Action::Select((0, 1), (1, 3)));
    h.refresh();
    let d = h.find("editor").unwrap().1;
    assert_eq!(d.what.value, "line 2, column 4, 8 selected");
    let lines = h.describe_lines(WindowId::MAIN);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("text editor \"\" at 0,0 size 240x120 id editor"), "{}", lines[0]);
}

#[test]
fn a_thousand_lines_widen_the_margin() {
    let lines: Vec<String> = (0..1000).map(|_| "abcdef".to_string()).collect();
    let mut h = editor(&lines.join("\n"));
    // Four digits need a 42 wide margin; the text starts at 43 + 8.
    tap(&mut h, 51 + 18 + 1, 10);
    assert_eq!(h.app().content.cursor(), (0, 3));
    let mut h = editor(&lines[..999].join("\n"));
    tap(&mut h, 45 + 18 + 1, 10);
    assert_eq!(h.app().content.cursor(), (0, 3));
}

#[test]
fn without_line_numbers_the_text_starts_at_the_edge() {
    let mut h = editor("abcdef");
    h.app_mut().numbers = false;
    h.refresh();
    tap(&mut h, 8 + 18 + 1, 10);
    assert_eq!(h.app().content.cursor(), (0, 3));
    tap(&mut h, 2, 10);
    assert_eq!(h.app().content.cursor(), (0, 0));
}

#[test]
fn a_hundred_thousand_lines_draw_and_type_as_fast_as_a_few() {
    let text: String = (0..100_000).map(|n| format!("line number {n}\n")).collect();
    let started = Instant::now();
    let mut h = editor(&text);
    h.key(Key::Tab);
    h.key_with(Key::End, ctrl());
    h.type_text("typed at the end");
    h.key_with(Key::Home, ctrl());
    h.key(Key::PageDown);
    h.scroll(100, 50, 100);
    tap(&mut h, 100, 50);
    h.type_text("x");
    h.shortcut(Shortcut::ctrl('z'));
    h.drag(at(1, 2), at(5, 2));
    assert_eq!(h.app().content.line_count(), 100_001);
    assert!(h.app().content.selection().is_some());
    assert!(started.elapsed().as_secs() < 60, "took {:?}", started.elapsed());
    let one = Instant::now();
    h.type_text("y");
    let each = one.elapsed();
    // One key in a hundred thousand lines costs about what it does in a short text.
    let mut s = focused("short");
    let two = Instant::now();
    s.type_text("y");
    let short = two.elapsed();
    assert!(each < short * 50 + std::time::Duration::from_millis(200), "{each:?} vs {short:?}");
}

#[test]
fn tiny_windows_and_odd_events_do_not_panic() {
    for size in [(4, 4), (13, 13), (37, 20), (50, 100), (300, 8)] {
        let s = Settings { size, resizable: true, min_size: (0, 0) };
        let mut h = Harness::new(Ed::new("héllo\n\tworld\n\n"), s);
        h.key(Key::Tab);
        h.type_text("abc\n\t");
        h.shortcut(Shortcut::ctrl('a'));
        h.shortcut(Shortcut::ctrl('c'));
        h.shortcut(Shortcut::ctrl('v'));
        h.key(Key::PageDown);
        h.key(Key::PageUp);
        tap(&mut h, 0, 0);
        tap(&mut h, size.0 as i32 - 1, size.1 as i32 - 1);
        tap(&mut h, -5, -5);
        tap(&mut h, 10_000, 10_000);
        h.drag((1, 1), (size.0 as i32, size.1 as i32));
        h.scroll(1, 1, 7);
        h.scroll(1, 1, -7);
        h.double_click(2, 2);
        h.key_with(Key::End, ctrl());
        h.resize(size.0 + 20, size.1 + 20);
        h.resize(size.0.max(2) / 2, size.1.max(2) / 2);
        h.set_scale(2);
    }
    // Empty, and unusual content.
    let mut h = focused("");
    h.key(Key::Backspace);
    h.key(Key::Delete);
    h.key(Key::Left);
    h.key(Key::Up);
    h.shortcut(Shortcut::ctrl('a'));
    h.shortcut(Shortcut::ctrl('z'));
    h.type_text("\u{0}\u{7}");
    assert_eq!(h.app().content.text(), "");
    // A very long line and a lot of empty ones.
    let mut h = focused(&"x".repeat(200_000));
    h.key_with(Key::End, ctrl());
    h.type_text("y");
    h.key(Key::Home);
    let mut h = focused(&"\n".repeat(5000));
    h.key_with(Key::End, ctrl());
    h.scroll(50, 50, -3);
}

// ---------------------------------------------------------------------------------------------
// The look.
// ---------------------------------------------------------------------------------------------

const POEM: &str = "A few lines of text,\nwith a cursor in them,\nand some more to read.\n\
                    Tabs:\tone\ttwo\nnaïve café — é";

fn lines_app() -> Ed {
    Ed::new(POEM)
}

fn picture_settings() -> Settings {
    Settings { size: (240, 100), resizable: true, min_size: (0, 0) }
}

#[test]
fn the_look_of_a_few_lines_with_the_cursor() {
    for scale in [1, 2] {
        let mut h = Harness::with_scale(lines_app(), picture_settings(), scale);
        h.key(Key::Tab);
        tap(&mut h, at(1, 7).0, at(1, 7).1);
        h.check_picture(&format!("text_editor-lines-{scale}x"));
    }
}

#[test]
fn the_look_of_a_selection_over_three_lines_focused_or_not() {
    for scale in [1, 2] {
        let mut app = lines_app();
        app.content.perform(Action::Select((0, 8), (2, 9)));
        let mut h = Harness::with_scale(app, picture_settings(), scale);
        // Without the focus: the quieter colour, no cursor.
        h.check_picture(&format!("text_editor-selection-unfocused-{scale}x"));
        h.key(Key::Tab);
        h.check_picture(&format!("text_editor-selection-{scale}x"));
    }
}

#[test]
fn the_look_of_a_text_scrolled_across_and_down_with_both_bars() {
    for scale in [1, 2] {
        let mut text = String::new();
        for n in 0..40 {
            text.push_str(&format!(
                "line {n}: {}\n",
                "wide text goes on and on ".repeat(n % 5 + 2)
            ));
        }
        let mut h = Harness::with_scale(Ed::new(&text), picture_settings(), scale);
        h.key(Key::Tab);
        h.scroll(100, 40, 2);
        let win = pbr_window::Win(1);
        h.send(pbr_window::Event::Pointer(pbr_window::PointerEvent {
            window: win,
            what: pbr_window::PointerKind::Scroll {
                axis: pbr_interfaces::pbr_input::AXIS_HWHEEL as u16,
                steps: 3,
            },
            x: 100,
            y: 40,
            raw_x: 0,
            raw_y: 0,
            buttons: 0,
            serial: 0,
        }));
        h.check_picture(&format!("text_editor-scrolled-{scale}x"));
    }
}

#[test]
fn the_look_without_line_numbers() {
    for scale in [1, 2] {
        let mut app = lines_app();
        app.numbers = false;
        app.content.perform(Action::Select((1, 2), (1, 10)));
        let mut h = Harness::with_scale(app, picture_settings(), scale);
        h.key(Key::Tab);
        h.check_picture(&format!("text_editor-plain-{scale}x"));
    }
}

#[test]
fn the_look_of_an_empty_editor() {
    for scale in [1, 2] {
        let mut h = Harness::with_scale(Ed::new(""), picture_settings(), scale);
        h.key(Key::Tab);
        h.check_picture(&format!("text_editor-empty-{scale}x"));
    }
}

#[test]
fn the_look_while_a_thumb_is_dragged() {
    let text: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
    let mut h = Harness::new(Ed::new(&text.join("\n")), picture_settings());
    h.move_to(234, 8);
    h.press(1);
    h.move_to(234, 30);
    h.check_picture("text_editor-thumb-1x");
    h.release(1);
}
