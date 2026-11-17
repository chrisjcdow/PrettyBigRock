// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Terminal (docs/design/0026-apps-m6-tests-and-stages.md, section 1): PrettyBigRock 1.3's
//! Terminal as a sandstone app, tested on the host.
//!
//! It is the shell in a window. The shell itself (`lib/shell`'s `program` module) is the one the
//! text shell runs too; here it runs on a thread of its own, and talks to this window through a
//! small queue ([`link`]) so that a long command never freezes the window. This library is the
//! window's half: the grid of characters, the line being typed, the history, scrolling back,
//! copying and pasting. The program (`main.rs`) starts the shell's thread and the event loop.
//!
//! What it does that 1.3's Terminal did: the prompt and the line you type, the history with Up
//! and Down, scrolling back with Page Up and Page Down (and, new, the wheel), Ctrl+C to give up
//! the line being typed, Ctrl+L to clear the screen, and the dark paper and green cursor
//! (sandstone's `TerminalView` has that look). What is new: the window can be resized (the grid
//! becomes as many columns and rows as fit), the history is kept in the app's own folder, and
//! text can be copied and pasted (note 0025, section 2).
//!
//! **A few words on the ideas used here.** A *trait* is a set of methods a type promises to have;
//! [`Store`] is what Terminal asks of whatever keeps its history file, so that this library has
//! no files in it (the program implements it over `lib/files`, the tests over a variable).
//! `Arc` is a pointer to something shared between threads, which lives until the last pointer
//! goes. `#![no_std]` (below, except in tests) means the library uses only Rust's `core` and
//! `alloc`, so that it can run as a PrettyBigRock program.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod link;

use crate::link::{Output, WindowEnd};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_draw::Colour;
use pbr_sandstone::{
    App, Cell, Cells, Context, ContextMenu, Element, Identify, Key, KeyPress, MenuItem, Settings,
    TerminalView,
};
use pbr_terminal::{Key as EditKey, Terminal};

/// The bit of the event loop's notification that the shell's thread rings to say it has
/// something for the window (bits 8 to 63 are the app's own, note 0023, section 4).
pub const WAKE: u32 = 9;
/// How many rows that scroll off the top are kept to look back at.
pub const SCROLLBACK: usize = 1000;
/// How many lines of history are kept, in memory and in the file.
pub const HISTORY: usize = 100;
/// The history's file, in the app's own folder.
pub const HISTORY_FILE: &str = "history.txt";
/// The window opens at 480 x 320 logical pixels (77 columns by 26 rows of the view's 6 x 12
/// cells). 1.3's was 512 x 384, which with its frame doesn't fit a 2x screen of 1024 x 768
/// (512 x 384 logical); Notepad opens at the same size.
pub const SIZE: (u32, u32) = (480, 320);
/// The smallest the window can be made: 28 columns by 7 rows.
pub const MIN_SIZE: (u32, u32) = (190, 100);
/// What the grid is until the view says how many columns and rows fit.
const FIRST_GRID: (usize, usize) = (82, 31);
/// The text colour: 1.3's `K_NORMAL`.
const TEXT: Colour = Colour::solid(0xd8dce6);

/// The window: opens at [`SIZE`] and can be made smaller, down to [`MIN_SIZE`].
pub fn settings() -> Settings {
    Settings { size: SIZE, resizable: true, min_size: MIN_SIZE, over: None, system: false }
}

// ---------------------------------------------------------------------------------------------
// What Terminal asks of the system
// ---------------------------------------------------------------------------------------------

/// Where the history is kept: one line of text per command, oldest first. (The program
/// implements this over the file `history.txt` in the app's own folder, `/Home/Apps/terminal`.)
pub trait Store {
    /// What was saved last, if anything was.
    fn load(&mut self) -> Option<String>;
    /// Replaces what is kept, and makes that last. `Err` says in words why not.
    fn save(&mut self, text: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------------------------
// The screen: the grid, and where in its scrollback the window is looking
// ---------------------------------------------------------------------------------------------

/// What the window shows: `lib/terminal`'s [`Terminal`] (the grid, the line being edited, the
/// history), and how far back the user has scrolled.
struct Screen {
    terminal: Terminal,
    /// How many rows back from the bottom the window is looking: 0 is the live screen.
    offset: usize,
    /// Counts every change of `offset`, so that [`Cells::moves`] changes when the user scrolls.
    shifts: u64,
}

impl Screen {
    fn new(columns: usize, rows: usize, mirror: bool) -> Screen {
        // The size is never 0 (the view says at least 1 by 1; the start is a real size).
        let mut terminal = Terminal::new(columns.max(1), rows.max(1), mirror)
            .or_else(|| Terminal::new(1, 1, mirror))
            .expect("a 1 x 1 terminal");
        terminal.set_scrollback(SCROLLBACK);
        terminal.set_history_max(HISTORY);
        Screen { terminal, offset: 0, shifts: 0 }
    }

    /// Looks `offset` rows back (held to what is kept).
    fn look_back(&mut self, offset: usize) {
        let offset = offset.min(self.terminal.grid().scrollback_len());
        if offset != self.offset {
            self.offset = offset;
            self.shifts = self.shifts.wrapping_add(1);
        }
    }

    /// Back to the live screen.
    fn go_to_bottom(&mut self) {
        self.look_back(0);
    }

    /// Goes on looking at the same text while the screen scrolls: when `rows` rows scrolled off
    /// the top, what was `offset` rows back is now `offset + rows` back.
    fn follow(&mut self) {
        let scrolled = self.terminal.take_changes().scrolled;
        if self.offset > 0 {
            self.look_back(self.offset.saturating_add(scrolled));
        }
    }

    /// Shows text from the shell, above the line being typed.
    fn write(&mut self, text: &str) {
        self.terminal.write(text);
        self.follow();
    }

    fn clear(&mut self) {
        self.terminal.clear();
        self.go_to_bottom();
        self.terminal.take_changes();
    }

    fn resize(&mut self, columns: usize, rows: usize) {
        if self.terminal.resize(columns, rows) {
            self.go_to_bottom();
            self.terminal.take_changes();
        }
    }
}

impl Cells for Screen {
    fn size(&self) -> (usize, usize) {
        (self.terminal.grid().columns(), self.terminal.grid().rows())
    }

    fn cell(&self, column: usize, row: usize) -> Cell {
        Cell::new(self.terminal.grid().cell_back(self.offset, column, row), TEXT)
    }

    /// The cursor is where the line being typed is, on the live screen only.
    fn cursor(&self) -> Option<(usize, usize)> {
        if self.offset == 0 { self.terminal.caret() } else { None }
    }

    /// Changes when rows scroll, are cleared, or the user looks further back or forward:
    /// both counts only go up, so their sum changes whenever either does.
    fn moves(&self) -> u64 {
        self.terminal.grid().moves().wrapping_add(self.shifts)
    }
}

// ---------------------------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------------------------

/// What a key press does to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A key of the line editor: a character, Backspace, the arrows, Enter...
    Edit(EditKey),
    /// Page Up: back a page. (1.3's `KEY_PGUP`.)
    PageUp,
    /// Page Down: forward a page.
    PageDown,
    /// Ctrl+C: give up the line being typed and start a new prompt.
    Interrupt,
    /// Ctrl+L: empty the screen (1.3's).
    ClearScreen,
    /// A key that does nothing here but must not move the focus (Tab).
    Nothing,
}

/// What `key` does, or `None` for a key Terminal doesn't use, which is left to the window
/// (Ctrl+W closes it, Alt+F4 and the Windows key are the desktop's). The view has already taken
/// its own keys (Ctrl+Shift+C and Ctrl+Shift+V) and presses of Shift or Ctrl alone.
pub fn action(key: &KeyPress) -> Option<Action> {
    let m = key.modifiers;
    if m.ctrl() {
        // With Ctrl held a letter is a command, never text: the letter on the key, whatever the
        // layout (note 0021).
        return match key.ch.and_then(|c| c.to_lowercase().next()) {
            Some('c') if !m.shift() => Some(Action::Interrupt),
            Some('l') if !m.shift() => Some(Action::ClearScreen),
            _ => None,
        };
    }
    match key.key {
        Key::Enter => Some(Action::Edit(EditKey::Enter)),
        Key::Backspace => Some(Action::Edit(EditKey::Backspace)),
        Key::Delete => Some(Action::Edit(EditKey::Delete)),
        Key::Left => Some(Action::Edit(EditKey::Left)),
        Key::Right => Some(Action::Edit(EditKey::Right)),
        Key::Home => Some(Action::Edit(EditKey::Home)),
        Key::End => Some(Action::Edit(EditKey::End)),
        Key::Up => Some(Action::Edit(EditKey::Up)),
        Key::Down => Some(Action::Edit(EditKey::Down)),
        Key::PageUp => Some(Action::PageUp),
        Key::PageDown => Some(Action::PageDown),
        Key::Tab => Some(Action::Nothing),
        _ => match key.ch {
            Some(c) if key.typed => Some(Action::Edit(EditKey::Char(c))),
            _ => None,
        },
    }
}

/// Pasted text as it goes into the line being typed: every line break (and tab) is a space,
/// so that a paste never runs a command by itself, and a Windows line end (`\r\n`) is one
/// space, not two.
pub fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            '\n' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------

/// What the window's view and menu send.
#[derive(Clone, Debug)]
pub enum Message {
    /// A key Terminal uses ([`action`]).
    Act(Action),
    /// Text to put in the line being typed: Ctrl+Shift+V, or a middle click, in the view.
    Paste(String),
    /// The wheel: lines, positive down.
    Scroll(i32),
    /// The view's columns and rows changed.
    Resized(usize, usize),
    /// What is selected now (empty if nothing).
    Selected(String),
    /// Copy, in the right-click menu.
    Copy,
    /// Paste, in the right-click menu.
    PasteMenu,
    /// Whether the clipboard has text (to grey out Paste).
    Clipboard(bool),
    /// The shell's thread rang the window's bell: it has printed something or asked for a line.
    Wake,
}

/// Terminal's window.
pub struct TerminalApp<S: Store> {
    screen: Screen,
    store: S,
    link: WindowEnd,
    /// The prompt of the line being read, if the shell is waiting for one; Ctrl+C starts it
    /// again.
    prompt: Option<String>,
    /// The text selected in the view, kept to copy from the menu.
    selected: String,
    /// The clipboard has text.
    clipboard_text: bool,
    /// Lines for the program's log: the terminal's mirror (note 0018, section 7), in test boots.
    log: Vec<String>,
}

impl<S: Store> TerminalApp<S> {
    /// Terminal, shown on `store`'s history and fed by the shell through `link`'s other end.
    /// With `mirror` (in test boots) everything the screen shows and every line entered is
    /// also recorded for [`TerminalApp::take_log`], so the test runner can see what the screen
    /// shows.
    pub fn new(link: WindowEnd, store: S, mirror: bool) -> TerminalApp<S> {
        TerminalApp {
            screen: Screen::new(FIRST_GRID.0, FIRST_GRID.1, mirror),
            store,
            link,
            prompt: None,
            selected: String::new(),
            clipboard_text: false,
            log: Vec::new(),
        }
    }

    // ---- What the program and the tests look at. ----

    /// The lines for the log since the last call: `[console] text`, `[console:read] prompt`,
    /// `[console] prompt line` and `[console:clear]` (`lib/terminal`'s mirror), then what went
    /// wrong in words (`couldn't keep the history: ...`). Without the mirror, only the latter.
    pub fn take_log(&mut self) -> Vec<String> {
        self.collect_mirror();
        core::mem::take(&mut self.log)
    }

    /// The characters on the live screen, one string per row, trailing spaces left as they
    /// are.
    pub fn rows(&self) -> Vec<String> {
        let grid = self.screen.terminal.grid();
        (0..grid.rows()).map(|r| grid.row(r).iter().collect()).collect()
    }

    /// The characters the window shows now: [`TerminalApp::rows`] unless the user has scrolled
    /// back, then the rows from further up.
    pub fn shown_rows(&self) -> Vec<String> {
        let (columns, rows) = self.screen.size();
        let grid = self.screen.terminal.grid();
        (0..rows)
            .map(|r| (0..columns).map(|c| grid.cell_back(self.screen.offset, c, r)).collect())
            .collect()
    }

    /// The columns and rows of the grid.
    pub fn grid_size(&self) -> (usize, usize) {
        self.screen.size()
    }

    /// How many rows back from the live screen the window is looking (0: the live screen).
    pub fn scrolled_back(&self) -> usize {
        self.screen.offset
    }

    /// How many rows are kept to look back at.
    pub fn kept_rows(&self) -> usize {
        self.screen.terminal.grid().scrollback_len()
    }

    /// Whether the shell is waiting for a line.
    pub fn is_reading(&self) -> bool {
        self.screen.terminal.is_reading()
    }

    /// The lines Up and Down go through, oldest first.
    pub fn history(&self) -> Vec<String> {
        self.screen.terminal.history().map(String::from).collect()
    }

    /// The text selected in the view.
    pub fn selected(&self) -> &str {
        &self.selected
    }

    /// The history store (the tests' pretend one is looked at through this).
    pub fn store(&self) -> &S {
        &self.store
    }

    // ---- The shell's thread. ----

    fn collect_mirror(&mut self) {
        let lines = self.screen.terminal.take_mirror();
        self.log.extend(lines);
    }

    /// Takes what the shell has queued and shows it.
    fn pump(&mut self) {
        let work = self.link.take_work();
        for output in work.output {
            match output {
                Output::Text(text) => self.screen.write(&text),
                Output::Clear => self.screen.clear(),
            }
        }
        if let Some(prompt) = work.ask {
            self.begin_read(prompt);
        }
    }

    /// Starts reading the line the shell asked for.
    fn begin_read(&mut self, prompt: String) {
        if self.screen.terminal.begin_read(&prompt).is_err() {
            // The shell asks for one line at a time, so this can't happen; if it somehow does,
            // the new prompt replaces the old rather than leave the shell waiting for ever.
            self.screen.terminal.abandon_read();
            let _ = self.screen.terminal.begin_read(&prompt);
        }
        self.prompt = Some(prompt);
        self.screen.follow();
    }

    // ---- The keyboard. ----

    fn act(&mut self, action: Action) {
        match action {
            Action::PageUp => {
                let page = self.screen.size().1.saturating_sub(2).max(1);
                self.screen.look_back(self.screen.offset.saturating_add(page));
            }
            Action::PageDown => {
                let page = self.screen.size().1.saturating_sub(2).max(1);
                self.screen.look_back(self.screen.offset.saturating_sub(page));
            }
            Action::Nothing => {}
            Action::ClearScreen => {
                self.screen.clear();
            }
            Action::Interrupt => {
                self.screen.go_to_bottom();
                self.interrupt();
            }
            Action::Edit(key) => {
                // Typing jumps back to the bottom, where what is being typed is.
                self.screen.go_to_bottom();
                if let Some(line) = self.screen.terminal.key(key) {
                    self.entered(line);
                }
                self.screen.follow();
            }
        }
    }

    /// Ctrl+C: what was typed stays on the screen with `^C` after it, and a new prompt starts,
    /// as in most terminals. The shell isn't told: it is still waiting for its line, and
    /// there is no way to stop a command that is running (so while one runs, there is no line
    /// being read and Ctrl+C does nothing).
    fn interrupt(&mut self) {
        let Some(prompt) = self.prompt.clone() else { return };
        if !self.screen.terminal.is_reading() {
            return;
        }
        for c in "^C".chars() {
            self.screen.terminal.key(EditKey::Char(c));
        }
        self.screen.terminal.abandon_read();
        self.begin_read(prompt);
    }

    /// The user pressed Enter: the line goes to the shell, and the history is kept.
    fn entered(&mut self, line: String) {
        self.prompt = None;
        if !line.is_empty() {
            self.keep_history();
        }
        self.link.answer(line);
    }

    /// Writes the history to the store: the last [`HISTORY`] lines, one to a line.
    fn keep_history(&mut self) {
        let mut text = String::new();
        for line in self.screen.terminal.history() {
            text.push_str(line);
            text.push('\n');
        }
        if let Err(why) = self.store.save(&text) {
            self.log.push(format!("couldn't keep the history: {why}"));
        }
    }

    /// Reads the history the last run kept.
    fn load_history(&mut self) {
        if let Some(text) = self.store.load() {
            self.screen.terminal.load_history(text.lines());
        }
    }

    // ---- The clipboard. ----

    /// Puts text in the line being typed (nothing if no line is being read: the keys would be
    /// dropped too). Line breaks become spaces, so a paste can't run a command.
    fn paste(&mut self, text: &str) {
        if !self.screen.terminal.is_reading() {
            return;
        }
        self.screen.go_to_bottom();
        for c in flatten(text).chars() {
            // Enter is never among them: `flatten` has no line breaks left.
            self.screen.terminal.key(EditKey::Char(c));
        }
        self.screen.follow();
    }

    // ---- The view. ----

    /// The right-click menu: Copy (when something is selected) and Paste (when the clipboard has
    /// text). The same two are Ctrl+Shift+C and Ctrl+Shift+V, which the view handles.
    fn menu(&self) -> Vec<MenuItem<Message>> {
        vec![
            MenuItem::new("Copy", Message::Copy).enabled(!self.selected.is_empty()),
            MenuItem::new("Paste", Message::PasteMenu).enabled(self.clipboard_text),
        ]
    }
}

impl<S: Store> App for TerminalApp<S> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let view = TerminalView::new(&self.screen)
            .on_key(|k| action(&k).map(Message::Act))
            .on_paste(Message::Paste)
            .on_scroll(Message::Scroll)
            .on_resize(Message::Resized)
            .on_selection(Message::Selected);
        ContextMenu::new(view.id("terminal"), self.menu()).into()
    }

    /// Empty (just "Terminal") while looking at the live screen; while scrolled back, which way
    /// to return. (1.3 drew this on the screen over the text; here the cells are the text and
    /// nothing else, so that copying a selection never copies the notice.)
    fn title(&self) -> String {
        if self.screen.offset > 0 {
            String::from("scrolled back: Page Down returns")
        } else {
            String::new()
        }
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        self.clipboard_text = cx.clipboard().0;
        self.load_history();
        cx.focus("terminal");
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Act(action) => self.act(action),
            Message::Paste(text) => self.paste(&text),
            Message::Scroll(lines) => {
                // The wheel's lines are positive down, towards the live screen.
                let back = (self.screen.offset as i64).saturating_sub(i64::from(lines)).max(0);
                self.screen.look_back(usize::try_from(back).unwrap_or(0));
            }
            Message::Resized(columns, rows) => {
                if self.screen.size() != (columns, rows) {
                    self.screen.resize(columns, rows);
                }
            }
            Message::Selected(text) => self.selected = text,
            Message::Copy => {
                if !self.selected.is_empty() {
                    cx.copy_text(&self.selected);
                }
            }
            Message::PasteMenu => {
                if let Some(text) = cx.paste_text() {
                    self.paste(&text);
                }
            }
            Message::Clipboard(text) => self.clipboard_text = text,
            Message::Wake => self.pump(),
        }
        self.collect_mirror();
    }

    fn notified(&self, bits: u64) -> Option<Message> {
        (bits & (1 << WAKE) != 0).then_some(Message::Wake)
    }

    fn clipboard_changed(&self, text: bool, _picture: bool) -> Option<Message> {
        Some(Message::Clipboard(text))
    }
}

#[cfg(test)]
mod tests;
