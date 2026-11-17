// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Terminal on the host: a pretend shell on the other side of the queue, typing lines and
//! getting output, clearing, history (kept in a pretend folder), scrolling back, copying and
//! pasting, Ctrl+C, resizing, the shell going away, the log the test boots read, and the
//! look (`tests/pictures/terminal/`).

use super::*;
use crate::link::tests::{Bell, pair};
use crate::link::{ShellEnd, WindowEnd};
use pbr_interfaces::pbr_input as input;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Modifiers, Shortcut, WindowId};
use pbr_shell::{Console, Gone};
use pbr_window::Event;
use std::path::PathBuf;
use std::sync::Arc;

/// A pretend folder: the history file's text, and whether saving works.
#[derive(Default)]
struct Fake {
    kept: Option<String>,
    saves: usize,
    full: bool,
}

impl Store for Fake {
    fn load(&mut self) -> Option<String> {
        self.kept.clone()
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        if self.full {
            return Err("the disk is full".into());
        }
        self.kept = Some(text.into());
        self.saves += 1;
        Ok(())
    }
}

/// The window, and the shell's end of the queue.
struct Rig {
    h: Harness<TerminalApp<Fake>>,
    shell: ShellEnd,
    window: WindowEnd,
    _window_bell: Arc<Bell>,
}

const PROMPT: &str = "chris:/Home> ";

fn rig_with(store: Fake, mirror: bool, scale: u32) -> Rig {
    let (window, shell, window_bell, _) = pair();
    let app = TerminalApp::new(window.clone(), store, mirror);
    let h = Harness::with_scale(app, settings(), scale);
    Rig { h, shell, window, _window_bell: window_bell }
}

fn rig() -> Rig {
    rig_with(Fake::default(), false, 1)
}

impl Rig {
    /// The shell's thread rang the window's bell.
    fn wake(&mut self) {
        self.h.send(Event::Notified(1 << WAKE));
    }

    /// The shell prints `text`.
    fn says(&mut self, text: &str) {
        self.shell.write(text).unwrap();
        self.wake();
    }

    /// The shell asks for a line.
    fn asks(&mut self) {
        self.shell.ask(PROMPT).unwrap();
        self.wake();
    }

    /// The user types `text` and presses Enter.
    fn types(&mut self, text: &str) {
        self.h.type_text(text);
        self.h.key(Key::Enter);
    }

    /// What the window shows, one string per row, without trailing spaces, and without the
    /// empty rows at the end.
    fn shown(&self) -> Vec<String> {
        let mut rows: Vec<String> =
            self.h.app().shown_rows().iter().map(|r| r.trim_end().to_string()).collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    /// The shell's whole turn: the user types `text`; the shell is given the line.
    fn line(&mut self, text: &str) -> String {
        self.types(text);
        self.shell.try_line().unwrap().expect("the line was handed to the shell")
    }
}

/// The middle of cell (`c`, `r`) of the view, which fills the window.
fn at(c: i32, r: i32) -> (i32, i32) {
    (8 + 6 * c + 3, 6 + 12 * r + 6)
}

fn fresh() -> Rig {
    let mut r = rig();
    r.says("PrettyBigRock 2.0.0-alpha.1 terminal\nType 'help' to see what I can do.\n\n");
    r.asks();
    r
}

// ---------------------------------------------------------------------------------------------
// Typing and printing
// ---------------------------------------------------------------------------------------------

#[test]
fn it_opens_on_the_shells_welcome_and_prompt() {
    let r = fresh();
    assert_eq!(
        r.shown(),
        [
            "PrettyBigRock 2.0.0-alpha.1 terminal",
            "Type 'help' to see what I can do.",
            "",
            "chris:/Home>"
        ]
    );
    assert!(r.h.app().is_reading());
    // 480 x 320 logical pixels is 77 columns by 25 rows of 6 x 12, inside the view's margins.
    assert_eq!(r.h.app().grid_size(), (77, 25));
    // The view has the keyboard, and the cursor is after the prompt.
    assert_eq!(r.h.focused().unwrap().what.role, "terminal");
    let d = r.h.find("terminal").unwrap().1;
    assert_eq!(d.what.value, "chris:/Home>");
}

#[test]
fn a_typed_line_is_handed_to_the_shell_and_its_output_appears() {
    let mut r = fresh();
    r.h.type_text("echo hi");
    assert_eq!(r.shell.try_line(), Ok(None), "nothing goes before Enter");
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> echo hi");
    r.h.key(Key::Enter);
    assert_eq!(r.shell.try_line(), Ok(Some("echo hi".to_string())));
    assert!(!r.h.app().is_reading(), "nobody is reading until the shell asks again");
    // The shell answers, then asks for the next line.
    r.says("hi\n");
    r.asks();
    assert_eq!(r.shown()[3..], ["chris:/Home> echo hi", "hi", "chris:/Home>"]);
    assert!(r.h.app().is_reading());
}

#[test]
fn keys_typed_while_a_command_runs_are_dropped() {
    let mut r = fresh();
    r.line("slow");
    r.h.type_text("abc");
    r.h.key(Key::Enter);
    assert_eq!(r.shell.try_line(), Ok(None), "no line is being read, so nothing is handed over");
    r.asks();
    assert_eq!(r.shown().last().unwrap(), "chris:/Home>", "and nothing is waiting in the line");
}

#[test]
fn an_empty_line_is_handed_over_too() {
    let mut r = fresh();
    assert_eq!(r.line(""), "");
    assert!(r.h.app().history().is_empty(), "but it is not remembered");
}

#[test]
fn output_that_arrives_while_a_line_is_typed_goes_above_it() {
    let mut r = fresh();
    r.h.type_text("ab");
    r.says("news\n");
    assert_eq!(r.shown()[3..], ["news", "chris:/Home> ab"]);
    r.h.type_text("c");
    r.h.key(Key::Enter);
    assert_eq!(r.shell.try_line(), Ok(Some("abc".to_string())));
}

#[test]
fn the_editing_keys_edit_the_line() {
    let mut r = fresh();
    r.h.type_text("helo");
    r.h.key(Key::Left);
    r.h.type_text("l");
    r.h.key(Key::Home);
    r.h.type_text(">");
    r.h.key(Key::End);
    r.h.key(Key::Backspace);
    r.h.key(Key::Left);
    r.h.key(Key::Delete);
    assert_eq!(r.line(""), ">hel");
    // Spaces and capitals, and what a layout types that ASCII doesn't.
    r.asks();
    r.h.type_text("Echo A b");
    r.h.key_raw(0, Some('£'), Modifiers::NONE);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> Echo A b£");
}

#[test]
fn clearing_empties_the_screen_and_the_prompt_goes_to_the_top() {
    let mut r = fresh();
    r.h.type_text("cls");
    r.shell.clear().unwrap();
    r.wake();
    assert_eq!(r.shown(), ["chris:/Home> cls"]);
    assert_eq!(r.h.app().scrolled_back(), 0);
    assert_eq!(r.h.app().kept_rows(), 0, "clear forgets what scrolled away too");
    // Output after a clear goes on from there.
    r.says("after\n");
    assert_eq!(r.shown(), ["after", "chris:/Home> cls"]);
}

#[test]
fn ctrl_l_clears_the_screen_too() {
    let mut r = fresh();
    r.h.type_text("keep");
    r.h.shortcut(Shortcut::ctrl('l'));
    assert_eq!(r.shown(), ["chris:/Home> keep"]);
    // The line goes on being edited.
    assert_eq!(r.line("!"), "keep!");
}

// ---------------------------------------------------------------------------------------------
// Ctrl+C
// ---------------------------------------------------------------------------------------------

#[test]
fn ctrl_c_gives_up_the_line_and_starts_a_new_prompt() {
    let mut r = fresh();
    r.h.type_text("rm -r everything");
    r.h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(r.shown()[3..], ["chris:/Home> rm -r everything^C", "chris:/Home>"]);
    // The shell isn't told and is still waiting; the new line is what it gets.
    assert_eq!(r.shell.try_line(), Ok(None));
    assert!(r.h.app().is_reading());
    assert_eq!(r.line("ls"), "ls");
    assert_eq!(r.h.app().history(), ["ls"], "the abandoned line is not remembered");
}

#[test]
fn ctrl_c_with_no_line_being_read_does_nothing() {
    let mut r = fresh();
    r.line("long command");
    let before = r.shown();
    r.h.shortcut(Shortcut::ctrl('c'));
    assert_eq!(r.shown(), before);
    // Ctrl+Shift+C is the view's Copy, not this.
    r.asks();
    r.h.type_text("x");
    r.h.shortcut(Shortcut::ctrl_shift('c'));
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> x");
}

// ---------------------------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------------------------

#[test]
fn up_and_down_go_through_the_lines_typed() {
    let mut r = fresh();
    for line in ["first", "second"] {
        r.line(line);
        r.asks();
    }
    r.h.key(Key::Up);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> second");
    r.h.key(Key::Up);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> first");
    r.h.key(Key::Down);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> second");
    r.h.key(Key::Down);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home>");
    r.h.key(Key::Up);
    assert_eq!(r.line(""), "second", "Enter runs what was recalled");
}

#[test]
fn the_history_is_kept_in_the_apps_folder_after_each_line() {
    let mut r = fresh();
    assert_eq!(r.h.app().store().saves, 0);
    r.line("one");
    assert_eq!(r.h.app().store().kept.as_deref(), Some("one\n"));
    r.asks();
    r.line("two");
    assert_eq!(r.h.app().store().kept.as_deref(), Some("one\ntwo\n"));
    assert_eq!(r.h.app().store().saves, 2);
    // An empty line, or the same as the last, changes nothing in the history (and isn't saved
    // again).
    r.asks();
    r.line("");
    assert_eq!(r.h.app().store().saves, 2);
}

#[test]
fn the_history_is_read_when_it_starts() {
    let fake = Fake { kept: Some("old one\nold two\n\nold two\n".into()), ..Fake::default() };
    let mut r = rig_with(fake, false, 1);
    r.asks();
    assert_eq!(r.h.app().history(), ["old one", "old two"]);
    r.h.key(Key::Up);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> old two");
    r.h.key(Key::Up);
    r.h.key(Key::Up);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> old one");
}

#[test]
fn only_the_last_hundred_lines_are_kept() {
    let fake =
        Fake { kept: Some((0..150).map(|i| format!("old {i}\n")).collect()), ..Fake::default() };
    let mut r = rig_with(fake, false, 1);
    r.asks();
    let history = r.h.app().history();
    assert_eq!(history.len(), 100);
    assert_eq!(history[0], "old 50");
    r.line("new");
    r.asks();
    let kept = r.h.app().store().kept.clone().unwrap();
    assert_eq!(kept.lines().count(), 100);
    assert!(kept.starts_with("old 51\n") && kept.ends_with("old 149\nnew\n"));
    // A hundred lines typed in this window do the same.
    for i in 0..120 {
        r.line(&format!("n{i}"));
        r.asks();
    }
    let kept = r.h.app().store().kept.clone().unwrap();
    assert_eq!(kept.lines().count(), 100);
    assert!(kept.starts_with("n20\n") && kept.ends_with("n119\n"));
}

#[test]
fn a_history_that_cannot_be_kept_is_logged_and_the_terminal_goes_on() {
    let fake = Fake { full: true, ..Fake::default() };
    let mut r = rig_with(fake, false, 1);
    r.asks();
    assert_eq!(r.line("ls"), "ls");
    assert_eq!(r.h.app_mut().take_log(), ["couldn't keep the history: the disk is full"]);
    // The line was run and is in the history in memory.
    assert_eq!(r.h.app().history(), ["ls"]);
}

// ---------------------------------------------------------------------------------------------
// Scrollback
// ---------------------------------------------------------------------------------------------

/// A shell that has printed `n` numbered lines and is waiting for a line.
fn busy(n: usize) -> Rig {
    let mut r = fresh();
    let text: String = (1..=n).map(|i| format!("line {i}\n")).collect();
    // The shell is still waiting for its line: the output goes above it.
    r.says(&text);
    r
}

#[test]
fn page_up_and_page_down_look_back_and_forward() {
    let mut r = busy(100);
    // 25 rows: the last is the prompt, before it line 100.
    let live = r.shown();
    assert_eq!(live.last().unwrap(), "chris:/Home>");
    assert_eq!(live[live.len() - 2], "line 100");
    assert!(r.h.app().kept_rows() > 70);
    r.h.key(Key::PageUp);
    // A page is the rows less 2, so two rows are on both screens.
    assert_eq!(r.h.app().scrolled_back(), 23);
    let back = r.shown();
    assert_eq!(back.last().unwrap(), "line 78", "23 rows further back");
    assert_eq!(back[0], "line 54");
    r.h.key(Key::PageUp);
    assert_eq!(r.h.app().scrolled_back(), 46);
    assert_eq!(r.shown()[0], "line 31");
    r.h.key(Key::PageDown);
    assert_eq!(r.h.app().scrolled_back(), 23);
    r.h.key(Key::PageDown);
    r.h.key(Key::PageDown);
    assert_eq!(r.h.app().scrolled_back(), 0, "never past the live screen");
    assert_eq!(r.shown(), live);
}

#[test]
fn the_title_says_how_to_get_back_while_scrolled() {
    let mut r = busy(100);
    let title = |r: &Rig| r.h.window(WindowId::MAIN).unwrap().title.clone();
    assert_eq!(title(&r), "");
    r.h.key(Key::PageUp);
    assert_eq!(title(&r), "scrolled back: Page Down returns");
    r.h.key(Key::PageDown);
    assert_eq!(title(&r), "");
}

#[test]
fn it_cannot_scroll_back_past_what_is_kept() {
    let mut r = busy(100);
    let kept = r.h.app().kept_rows();
    for _ in 0..20 {
        r.h.key(Key::PageUp);
    }
    assert_eq!(r.h.app().scrolled_back(), kept);
    // At the top of what is kept the first line shown is the oldest kept (the welcome's
    // first row has gone: only so many are kept).
    assert!(r.shown().iter().any(|row| row.starts_with("line ")));
    // A window that has printed little has nothing to look back at.
    let mut small = fresh();
    small.h.key(Key::PageUp);
    assert_eq!(small.h.app().scrolled_back(), 0);
}

#[test]
fn only_a_thousand_rows_are_kept() {
    let mut r = busy(1500);
    assert_eq!(r.h.app().kept_rows(), SCROLLBACK);
    for _ in 0..100 {
        r.h.key(Key::PageUp);
    }
    assert_eq!(r.h.app().scrolled_back(), SCROLLBACK);
    // 3 rows of welcome, 1500 lines (line k is row k + 2, counting from 0) and the prompt:
    // 1504 rows, of which 25 are on the screen, so the 1000 kept are the last 1000 of the
    // 1479 above it, from row 479.
    assert_eq!(r.shown()[0], "line 477");
}

#[test]
fn the_wheel_scrolls_by_three_lines() {
    let mut r = busy(100);
    let (x, y) = at(10, 10);
    r.h.scroll(x, y, -2);
    assert_eq!(r.h.app().scrolled_back(), 6, "wheel up looks back");
    r.h.scroll(x, y, 1);
    assert_eq!(r.h.app().scrolled_back(), 3);
    r.h.scroll(x, y, 5);
    assert_eq!(r.h.app().scrolled_back(), 0);
}

#[test]
fn typing_jumps_back_to_the_bottom() {
    let mut r = busy(100);
    r.h.key(Key::PageUp);
    r.h.key(Key::PageUp);
    assert!(r.h.app().scrolled_back() > 0);
    r.h.type_text("x");
    assert_eq!(r.h.app().scrolled_back(), 0);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> x");
    // Keys that are not typing leave the place alone: a page key, and one the window keeps.
    r.h.key(Key::PageUp);
    r.h.key(Key::Escape);
    assert_eq!(r.h.app().scrolled_back(), 23);
    // Backspace is editing, so it goes back to the bottom.
    r.h.key(Key::Backspace);
    assert_eq!(r.h.app().scrolled_back(), 0);
}

#[test]
fn scrolled_back_text_stays_put_while_output_arrives() {
    let mut r = busy(100);
    r.h.key(Key::PageUp);
    let before = r.shown();
    // More lines scroll in at the bottom; what is being looked at doesn't move.
    r.says("more 1\nmore 2\nmore 3\n");
    assert_eq!(r.shown(), before);
    assert_eq!(r.h.app().scrolled_back(), 26);
    r.h.key(Key::PageDown);
    r.h.key(Key::PageDown);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home>");
}

#[test]
fn the_cursor_is_not_shown_while_looking_back() {
    let mut r = busy(100);
    let live = r.h.find("terminal").unwrap().1;
    assert!(live.what.enabled);
    r.h.key(Key::PageUp);
    // The view is told there is no cursor (`Cells::cursor`); the app's own check:
    assert_eq!(Cells::cursor(&r.h.app().screen), None);
    r.h.key(Key::PageDown);
    assert!(Cells::cursor(&r.h.app().screen).is_some());
}

// ---------------------------------------------------------------------------------------------
// Copy and paste
// ---------------------------------------------------------------------------------------------

#[test]
fn ctrl_shift_c_copies_the_selection() {
    let mut r = fresh();
    r.h.drag(at(0, 0), at(12, 0));
    assert_eq!(r.h.app().selected(), "PrettyBigRock");
    r.h.shortcut(Shortcut::ctrl_shift('c'));
    assert_eq!(r.h.clipboard_text().as_deref(), Some("PrettyBigRock"));
    // Several rows.
    r.h.drag(at(0, 0), at(6, 1));
    assert_eq!(r.h.app().selected(), "PrettyBigRock 2.0.0-alpha.1 terminal\nType 'h");
}

#[test]
fn ctrl_shift_v_pastes_into_the_line() {
    let mut r = fresh();
    r.h.type_text("echo ");
    r.h.set_clipboard_text("hello world");
    r.h.shortcut(Shortcut::ctrl_shift('v'));
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> echo hello world");
    assert_eq!(r.line(""), "echo hello world");
}

#[test]
fn a_paste_never_runs_a_command_by_itself() {
    let mut r = fresh();
    r.h.set_clipboard_text("rm everything\nrm more\r\nrm last\tstill\r\n");
    r.h.shortcut(Shortcut::ctrl_shift('v'));
    // The line breaks are spaces: one line, still being typed, nothing handed to the shell.
    assert_eq!(r.shell.try_line(), Ok(None));
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> rm everything rm more rm last still");
    assert!(r.h.app().is_reading());
    assert_eq!(r.line(""), "rm everything rm more rm last still ");
}

#[test]
fn pasting_when_no_line_is_being_read_does_nothing() {
    let mut r = fresh();
    r.line("slow");
    let before = r.shown();
    r.h.set_clipboard_text("abc");
    r.h.shortcut(Shortcut::ctrl_shift('v'));
    assert_eq!(r.shown(), before);
    r.asks();
    assert_eq!(r.shown().last().unwrap(), "chris:/Home>", "and it isn't waiting for the next");
}

#[test]
fn a_middle_click_pastes() {
    let mut r = fresh();
    r.h.set_clipboard_text("mid");
    let (x, y) = at(5, 5);
    r.h.move_to(x, y);
    r.h.press(input::BUTTON_MIDDLE as u16);
    r.h.release(input::BUTTON_MIDDLE as u16);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> mid");
}

#[test]
fn a_paste_goes_to_the_bottom_where_the_line_is() {
    let mut r = busy(100);
    r.h.key(Key::PageUp);
    r.h.set_clipboard_text("pasted");
    r.h.shortcut(Shortcut::ctrl_shift('v'));
    assert_eq!(r.h.app().scrolled_back(), 0);
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> pasted");
}

#[test]
fn the_right_click_menu_has_copy_and_paste() {
    let mut r = fresh();
    // Nothing selected, nothing on the clipboard: both are greyed out.
    let (x, y) = at(20, 10);
    r.h.right_click_in(WindowId::MAIN, x, y);
    assert!(r.h.popup().is_some(), "the menu opened");
    assert!(!r.h.find("Copy").unwrap().1.what.enabled);
    assert!(!r.h.find("Paste").unwrap().1.what.enabled);
    r.h.key(Key::Escape);
    assert!(r.h.popup().is_none());
    // Select, and copy from the menu.
    r.h.drag(at(0, 0), at(12, 0));
    r.h.right_click_in(WindowId::MAIN, x, y);
    assert!(r.h.find("Copy").unwrap().1.what.enabled);
    r.h.click_on("Copy");
    assert_eq!(r.h.clipboard_text().as_deref(), Some("PrettyBigRock"));
    // Now the clipboard has text: Paste is available, and puts it in the line.
    r.h.right_click_in(WindowId::MAIN, x, y);
    assert!(r.h.find("Paste").unwrap().1.what.enabled);
    r.h.click_on("Paste");
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> PrettyBigRock");
}

#[test]
fn paste_follows_what_another_app_puts_on_the_clipboard() {
    let mut r = fresh();
    let (x, y) = at(20, 10);
    r.h.set_clipboard_text("from elsewhere");
    r.h.right_click_in(WindowId::MAIN, x, y);
    assert!(r.h.find("Paste").unwrap().1.what.enabled);
    r.h.click_on("Paste");
    assert_eq!(r.shown().last().unwrap(), "chris:/Home> from elsewhere");
}

#[test]
fn a_selection_is_dropped_when_the_rows_move() {
    let mut r = busy(100);
    r.h.drag(at(0, 5), at(6, 5));
    assert!(!r.h.app().selected().is_empty());
    r.h.key(Key::PageUp);
    // The next event is what lets the view notice that the rows moved.
    r.h.move_to(at(30, 20).0, at(30, 20).1);
    assert_eq!(r.h.app().selected(), "");
    r.h.drag(at(0, 5), at(3, 5));
    assert_eq!(r.h.app().selected(), "line");
    r.says("output\n");
    r.h.move_to(at(30, 20).0, at(30, 20).1);
    assert_eq!(r.h.app().selected(), "", "output that scrolls moves the rows too");
}

// ---------------------------------------------------------------------------------------------
// Resizing
// ---------------------------------------------------------------------------------------------

#[test]
fn the_grid_follows_the_windows_size() {
    let mut r = fresh();
    assert_eq!(r.h.app().grid_size(), (77, 25));
    r.h.resize(300, 200);
    // (300 - 16) / 6 = 47 columns; (200 - 12) / 12 = 15 rows.
    assert_eq!(r.h.app().grid_size(), (47, 15));
    // The line being read is shown again, at the top, and editing goes on.
    assert!(r.h.app().is_reading());
    assert_eq!(r.shown(), ["chris:/Home>"]);
    assert_eq!(r.line("ls"), "ls");
    // Bigger again.
    r.h.resize(600, 400);
    assert_eq!(r.h.app().grid_size(), (97, 32));
}

#[test]
fn what_was_on_the_screen_can_still_be_looked_back_at_after_a_resize() {
    let mut r = fresh();
    r.h.resize(300, 200);
    r.h.key(Key::PageUp);
    assert_eq!(r.shown()[0], "PrettyBigRock 2.0.0-alpha.1 terminal");
    assert!(r.h.app().kept_rows() >= 2);
    // Typing is at the bottom again.
    r.h.type_text("x");
    assert_eq!(r.h.app().scrolled_back(), 0);
    assert_eq!(r.shown(), ["chris:/Home> x"]);
}

#[test]
fn a_window_as_small_as_it_can_be_still_works() {
    let mut r = fresh();
    let (w, h) = MIN_SIZE;
    r.h.resize(w, h);
    let (columns, rows) = r.h.app().grid_size();
    assert!(columns >= 28 && rows >= 7, "{columns} x {rows}");
    assert_eq!(r.line("ls"), "ls");
    r.says(&"x\n".repeat(50));
    r.asks();
    assert_eq!(r.shown().last().unwrap(), "chris:/Home>");
}

// ---------------------------------------------------------------------------------------------
// The shell going away, the keys, the log
// ---------------------------------------------------------------------------------------------

#[test]
fn closing_tells_the_shell_it_is_gone() {
    let r = fresh();
    r.window.close();
    assert_eq!(r.shell.write("x"), Err(Gone));
    assert_eq!(r.shell.read_line("> "), Err(Gone));
}

#[test]
fn the_output_a_wake_up_brings_is_shown_even_if_several_bells_came_as_one() {
    let mut r = rig();
    r.shell.write("a\n").unwrap();
    r.shell.write("b\n").unwrap();
    r.shell.clear().unwrap();
    r.shell.write("c\n").unwrap();
    r.shell.ask(PROMPT).unwrap();
    // One wake-up for all of it.
    r.wake();
    assert_eq!(r.shown(), ["c", "chris:/Home>"]);
    // A wake-up with nothing to take is harmless.
    r.wake();
    assert_eq!(r.shown(), ["c", "chris:/Home>"]);
}

#[test]
fn a_bit_that_is_not_the_apps_wake_up_does_nothing() {
    let mut r = rig();
    r.shell.write("a\n").unwrap();
    r.h.send(Event::Notified(1 << 40));
    assert_eq!(r.shown(), Vec::<String>::new());
    r.wake();
    assert_eq!(r.shown(), ["a"]);
}

#[test]
fn the_keys_terminal_uses_and_the_ones_it_leaves_to_the_window() {
    fn press(key: Key, ch: Option<char>, mods: u16) -> KeyPress {
        let modifiers = Modifiers(mods);
        let typed =
            ch.is_some() && mods & (Modifiers::CTRL | Modifiers::ALT | Modifiers::META) == 0;
        KeyPress { key, code: 0, ch, typed, modifiers, repeat: false }
    }
    let none = 0;
    assert_eq!(
        action(&press(Key::Character, Some('a'), none)),
        Some(Action::Edit(EditKey::Char('a')))
    );
    assert_eq!(
        action(&press(Key::Character, Some('A'), Modifiers::SHIFT)),
        Some(Action::Edit(EditKey::Char('A')))
    );
    assert_eq!(action(&press(Key::Space, Some(' '), none)), Some(Action::Edit(EditKey::Char(' '))));
    // AltGr types a character.
    assert_eq!(
        action(&press(Key::Character, Some('€'), Modifiers::ALTGR)),
        Some(Action::Edit(EditKey::Char('€')))
    );
    assert_eq!(action(&press(Key::Enter, None, none)), Some(Action::Edit(EditKey::Enter)));
    assert_eq!(action(&press(Key::Up, None, none)), Some(Action::Edit(EditKey::Up)));
    assert_eq!(action(&press(Key::PageUp, None, none)), Some(Action::PageUp));
    assert_eq!(action(&press(Key::PageDown, None, Modifiers::SHIFT)), Some(Action::PageDown));
    assert_eq!(action(&press(Key::Tab, None, none)), Some(Action::Nothing));
    assert_eq!(action(&press(Key::Character, Some('c'), Modifiers::CTRL)), Some(Action::Interrupt));
    assert_eq!(
        action(&press(Key::Character, Some('l'), Modifiers::CTRL)),
        Some(Action::ClearScreen)
    );
    // What belongs to the window: Ctrl+W closes it, other Ctrl letters, Escape, F keys; and
    // Ctrl+Shift+C is the view's own.
    assert_eq!(action(&press(Key::Character, Some('w'), Modifiers::CTRL)), None);
    assert_eq!(action(&press(Key::Character, Some('x'), Modifiers::CTRL)), None);
    assert_eq!(action(&press(Key::Character, Some('c'), Modifiers::CTRL | Modifiers::SHIFT)), None);
    assert_eq!(action(&press(Key::Escape, None, none)), None);
    assert_eq!(action(&press(Key::Function(4), None, Modifiers::ALT)), None);
    assert_eq!(action(&press(Key::Insert, None, none)), None);
}

#[test]
fn pasted_text_is_flattened() {
    assert_eq!(flatten("one line"), "one line");
    assert_eq!(flatten("a\nb"), "a b");
    assert_eq!(flatten("a\r\nb"), "a b");
    assert_eq!(flatten("a\rb"), "a b");
    assert_eq!(flatten("a\n\nb"), "a  b");
    assert_eq!(flatten("a\tb"), "a b");
    assert_eq!(flatten("\n"), " ");
    assert_eq!(flatten(""), "");
    assert_eq!(flatten("é\n£"), "é £");
}

#[test]
fn the_log_is_what_the_screen_shows_and_the_lines_entered() {
    let mut r = rig_with(Fake::default(), true, 1);
    r.says("Welcome\n");
    r.asks();
    r.line("echo hi");
    r.says("hi\n");
    r.asks();
    r.h.type_text("clear");
    r.h.key(Key::Enter);
    r.shell.clear().unwrap();
    r.wake();
    assert_eq!(
        r.h.app_mut().take_log(),
        [
            "[console] Welcome",
            "[console:read] chris:/Home> ",
            "[console] chris:/Home> echo hi",
            "[console] hi",
            "[console:read] chris:/Home> ",
            "[console] chris:/Home> clear",
            "[console:clear]",
        ]
    );
    assert!(r.h.app_mut().take_log().is_empty(), "taking forgets");
    // Without the mirror (a normal start) there is nothing to log.
    let mut quiet = fresh();
    quiet.line("ls");
    assert!(quiet.h.app_mut().take_log().is_empty());
}

#[test]
fn the_terminal_describes_itself_with_the_row_the_cursor_is_on() {
    let mut r = fresh();
    r.h.type_text("ls -l");
    let lines = r.h.describe_lines(WindowId::MAIN);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("terminal"), "{lines:?}");
    assert!(lines[0].contains("chris:/Home> ls -l"), "{lines:?}");
}

// ---------------------------------------------------------------------------------------------
// The look
// ---------------------------------------------------------------------------------------------

fn pictures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/terminal")
}

fn check(r: &Rig, name: &str) {
    r.h.check_window_picture_in(WindowId::MAIN, &pictures(), name);
}

/// The window as it opens: the welcome and the prompt.
fn just_opened(scale: u32) -> Rig {
    let (window, shell, window_bell, _) = pair();
    let app = TerminalApp::new(window.clone(), Fake::default(), false);
    let h = Harness::with_scale(app, settings(), scale);
    let mut r = Rig { h, shell, window, _window_bell: window_bell };
    r.says("PrettyBigRock 2.0.0-alpha.1 terminal\nType 'help' to see what I can do.\n\n");
    r.asks();
    r
}

#[test]
fn the_window_just_opened_at_1x() {
    check(&just_opened(1), "terminal-1x");
}

#[test]
fn the_window_just_opened_at_2x() {
    check(&just_opened(2), "terminal-2x");
}

#[test]
fn after_a_few_commands_with_a_selection() {
    let mut r = just_opened(1);
    r.line("fetch");
    r.says(
        "            \\|/   PrettyBigRock 2.0.0-alpha.1\n\
         \x20    ~~~~~~~ -o-  ---------------------------\n\
         \x20 /\\/_______\\     Kernel:  x86-64, written in Rust\n\
         \x20/            \\   Display: 1024 x 768\n\
         /              \\^ Memory:  2048 MB, 1800 MB free\n\
         ~~~~~~~~~~~~~~~~~ Uptime:  2 min 5 s\n\
         \x20 ~   ~   ~   ~   Disk:    Home (64 MB, 50 MB free)\n",
    );
    r.asks();
    r.line("ls");
    r.says("Documents/\nMusic/\nPictures/\nnotes.txt  3.4 KB\n");
    r.asks();
    r.line("calc (2+3)*4^2");
    r.says("= 80\n");
    r.asks();
    r.h.type_text("echo \"half a line");
    // Select part of the listing.
    r.h.drag(at(0, 12), at(11, 14));
    assert_eq!(r.h.app().selected(), "Documents/\nMusic/\nPictures/");
    check(&r, "terminal-commands-1x");
}

#[test]
fn scrolled_back() {
    let mut r = just_opened(1);
    let text: String = (1..=60).map(|i| format!("This is line {i} of some output\n")).collect();
    r.says(&text);
    r.h.key(Key::PageUp);
    check(&r, "terminal-scrolled-1x");
}
