// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Notepad (docs/design/0026-apps-m6-tests-and-stages.md, section 1): PrettyBigRock 1.3's text
//! editor as a sandstone app, tested on the host.
//!
//! It does everything 1.3's Notepad did (`np_*` in `classic/kernel/notepad.c`): typing and
//! editing with the arrow keys, Home, End, Page Up and Down, Backspace, Delete, Enter and Tab,
//! clicking to place the cursor, line numbers in a margin, **New**, **Open**, **Save** and
//! **Save as** (now in a File menu, with 1.3's Ctrl+N, Ctrl+O and Ctrl+S), the cursor's line and
//! column, a short message after saving, and a limit on how big a text may be. On top of that it
//! has what note 0026 adds: the file picker for opening and saving anywhere the user chooses,
//! the clipboard (Cut, Copy and Paste), Undo and Redo, Select all, a "•" in the title while
//! there are unsaved changes, **asking before work is lost** ("Save changes to NAME?"), and a
//! file to open when it is started (what Files' "Open with…" and Terminal's `edit` do).
//!
//! **Files and the file picker are reached only through the [`Disk`] trait**, so that this
//! library has no files in it: the program (`main.rs`) implements it over `lib/files` and the
//! picker, and the tests over a pretend disk. Everything that matters here, including what
//! the dialogs say, is therefore tested on the host.
//!
//! **What a file is.** Files are UTF-8. A file that isn't valid UTF-8 (1.3's were all ASCII,
//! but a user may open anything) is read as Latin-1, where each byte is one character, and
//! written back the same way, so that opening and saving it unchanged gives the same bytes; the
//! status line says so. A file whose lines end in `\r\n` is shown without the `\r`s and saved
//! with them again, and a UTF-8 byte order mark is kept off the screen and put back. The text
//! is edited in one sandstone `Content` (lines, cursor, selection and undo), which is not copied
//! except to read or write a file.
//!
//! A note on Rust for the newcomer. `#![no_std]` (below, except in tests) means this crate uses
//! only Rust's `core` and `alloc` libraries, not the operating system's `std`, so that it can
//! run as a PrettyBigRock program; `alloc` holds `String`, `Vec` and `format!`, which need a
//! heap. A *trait* is a set of methods a type promises to have: [`Disk`] is the set Notepad asks
//! of whatever holds its files, and `Notepad<D: Disk>` is "a Notepad over any `D` that has them".

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_sandstone::{
    Action, App, Column, Content, Context, Dialog, Element, Identify, Length, Menu, MenuBar,
    MenuItem, Settings, Shortcut, StatusBar, TextEditor, WindowId,
};
use pbr_window::Win;

// ---------------------------------------------------------------------------------------------
// The numbers that make Notepad
// ---------------------------------------------------------------------------------------------

/// The most a text may be, in bytes: a bigger file isn't opened, and a paste that would take
/// the text past it is refused. 1.3's limit was 32000 bytes, which was a fixed buffer; this
/// one leaves plenty of room in Notepad's 8 MiB (the text is kept once, as lines, and read or
/// written through one more copy).
pub const MAX_FILE: usize = 1024 * 1024;
/// Where Notepad may read and change files without asking, and what is offered when saving.
pub const DOCUMENTS: &str = "/Home/Documents";
/// The bit of the event loop's notification that the file picker's answers come through
/// (sandstone's `FilePicker` is given this number; bits 8 to 63 are the app's own).
pub const PICKED: u32 = 10;
/// The name offered when saving a text that has no file yet.
const FIRST_NAME: &str = "Untitled.txt";
/// How long a message in the status line stays, in milliseconds (1.3's was 300 ticks, 3 s).
const STATUS_MS: u64 = 3000;
/// The window opens at this size, in logical pixels. 1.3's was 560 x 400, which doesn't fit a
/// screen of 1024 x 768 at a scale of 2 (512 x 384 logical pixels, less the title bar).
pub const WINDOW_SIZE: (u32, u32) = (480, 320);
/// The smallest the window can be made.
pub const MIN_SIZE: (u32, u32) = (280, 180);
/// The id of the editor, which the tests and the desktop boot's runner find it by.
pub const EDITOR_ID: &str = "editor";

/// A UTF-8 byte order mark: some programs put it at the start of a file.
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

// ---------------------------------------------------------------------------------------------
// What Notepad asks of the system
// ---------------------------------------------------------------------------------------------

/// Why a file couldn't be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskError {
    /// More than the limit given: not a text Notepad will try to open.
    TooBig,
    /// There is no such file.
    NotFound,
    /// The file system said no, in words ("the disk is full").
    Failed(String),
}

/// What Notepad wants the file picker to ask the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A text file to open, starting in Documents, with a filter for text files.
    Open,
    /// Where to save, starting in Documents and offering `name`.
    Save {
        /// The file name offered first.
        name: String,
    },
}

/// What the user chose, or why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickAnswer {
    /// The path of the file chosen. It can be opened by that path.
    Chosen(String),
    /// The user cancelled. Nothing to say about it.
    Cancelled,
    /// The picker couldn't answer, in words.
    Failed(String),
}

/// Everything Notepad does with files, and the file picker. Paths are the ones the user sees
/// (`/Home/Documents/letter.txt`).
///
/// Notepad is a *sandboxed* app (note 0017): it can reach only the folders its manifest and the
/// user gave it (here, Documents), and a file the user picks anywhere else is handed over by the
/// **file picker**, a separate system app (note 0025), as a capability, a right to use that one
/// file. The picker waits for the user, so asking returns at once and the answer comes later:
/// the program tells the app with notification bit [`PICKED`], and [`Disk::answer`] gives it.
pub trait Disk {
    /// The whole file, if it is at most `max` bytes.
    fn read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, DiskError>;
    /// Makes the file at `path` (made if it isn't there) hold exactly `data`, and makes that
    /// durable: write it all, set its size, then `sync` (note 0014), so that a power cut leaves
    /// the old file or the new one, never half of each.
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), DiskError>;
    /// Asks the file picker. `window` and `serial` are the app's window and the key or button
    /// press that led here (the picker opens only for a recent one). Returns at once; `Err`
    /// says in words why there is no picker to ask.
    fn pick(&mut self, window: Option<Win>, serial: u32, pick: Pick) -> Result<(), String>;
    /// The picker's answer, once it has come (`None` before).
    fn answer(&mut self) -> Option<PickAnswer>;
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

/// A file's name: what is after the last `/`.
fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// An error from the disk in words (for logs and dialogs).
fn why_text(e: &DiskError) -> String {
    match e {
        DiskError::TooBig => String::from("it is too big"),
        DiskError::NotFound => String::from("there is no such file"),
        DiskError::Failed(why) => why.clone(),
    }
}

/// Whether `path` is inside Documents, the one folder Notepad can make new files in without
/// asking.
fn in_documents(path: &str) -> bool {
    path.strip_prefix(DOCUMENTS).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether every line break in `text` is a `\r\n` (and there is at least one): such a file is
/// shown without the `\r`s and saved with them again. A file that mixes the two kinds is not
/// counted, because there is no one way to put its breaks back.
fn is_crlf(text: &str) -> bool {
    let breaks = text.matches('\n').count();
    breaks > 0 && text.matches("\r\n").count() == breaks
}

/// The limit in words, for the messages ("1 MB").
fn size_words() -> String {
    format!("{} MB", MAX_FILE / (1024 * 1024))
}

// ---------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------

/// What the window's widgets send.
#[derive(Clone, Debug)]
pub enum Message {
    /// The user did something to the text (typing, moving, selecting, undo, a paste with
    /// Ctrl+V...): [`Content::perform`] applies it.
    Edit(Action),
    /// File > New (Ctrl+N): an empty text, from no file.
    New,
    /// File > Open… (Ctrl+O): the file picker.
    Open,
    /// File > Save (Ctrl+S): to the file the text came from, or the picker if none.
    Save,
    /// File > Save as… (Ctrl+Shift+S): the picker.
    SaveAs,
    /// File > Close (Ctrl+W), or the window's close button.
    Close,
    /// Edit > Undo (Ctrl+Z).
    Undo,
    /// Edit > Redo (Ctrl+Y, Ctrl+Shift+Z).
    Redo,
    /// Edit > Cut (Ctrl+X).
    Cut,
    /// Edit > Copy (Ctrl+C).
    Copy,
    /// Edit > Paste (Ctrl+V).
    Paste,
    /// Edit > Select all (Ctrl+A).
    SelectAll,
    /// The file picker has answered ([`Disk::answer`]).
    Picked,
    /// The clipboard changed: whether it has text.
    Clipboard(bool),
    /// The status line's message with this number has been up long enough.
    ClearStatus(u32),
    /// "Save changes?": Save.
    SaveChanges,
    /// "Save changes?": Don't save.
    DiscardChanges,
    /// "Save changes?": Cancel.
    KeepEditing,
}

/// What a file picker that is open is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Open,
    Save,
}

/// What the user asked for that would lose the unsaved text, and is waiting on the question
/// "Save changes?".
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// File > New.
    New,
    /// The file chosen for Open, to be opened once the old text is saved or let go. (The
    /// picker is asked first, and the question after: the picker opens only for a press in the
    /// app's window, and the press that answers a question is in the question's.)
    OpenFile(String),
    /// Closing the window.
    Close,
}

/// Notepad, over a [`Disk`].
pub struct Notepad<D: Disk> {
    disk: D,
    /// The text, with the cursor, selection and undo.
    content: Content,
    /// The file the text came from or was saved to.
    path: Option<String>,
    /// The file's lines end in `\r\n`: they are saved that way again.
    crlf: bool,
    /// The file wasn't UTF-8 and was read as Latin-1: it is saved that way again.
    latin1: bool,
    /// The file began with a UTF-8 byte order mark: it is put back.
    bom: bool,
    /// The file to open when the window opens (the program's argument).
    initial: Option<String>,
    /// What the file picker, if one is open, is for.
    asked: Option<Asked>,
    /// What is waiting for the answer to "Save changes?".
    pending: Option<Pending>,
    /// Whether the clipboard has text (Paste is greyed out when it doesn't).
    paste_ok: bool,
    /// The status line's message, and its number, which makes each timer know whether it is
    /// still the one that is up.
    status: String,
    status_id: u32,
    /// Lines for the program's log, and whether the text changed since the program last looked.
    log: Vec<String>,
    text_changed: bool,
}

impl<D: Disk> Notepad<D> {
    /// Notepad with an empty text.
    pub fn new(disk: D) -> Notepad<D> {
        Notepad {
            disk,
            content: Content::new(),
            path: None,
            crlf: false,
            latin1: false,
            bom: false,
            initial: None,
            asked: None,
            pending: None,
            paste_ok: false,
            status: String::new(),
            status_id: 0,
            log: Vec::new(),
            text_changed: false,
        }
    }

    /// Opens `path` when the window opens (the file the program was started with). A file in
    /// Documents that isn't there yet is a new text that will be made by the first save, as
    /// 1.3's was.
    pub fn with_file(mut self, path: &str) -> Notepad<D> {
        self.initial = Some(String::from(path));
        self
    }

    // ---- What the program and the tests look at. ----

    /// The text.
    pub fn content(&self) -> &Content {
        &self.content
    }

    /// The text, to change (the tests make a text without typing it).
    pub fn content_mut(&mut self) -> &mut Content {
        &mut self.content
    }

    /// The file the text came from or was last saved to.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Whether the text has changed since it was opened or saved.
    pub fn is_dirty(&self) -> bool {
        self.content.is_modified()
    }

    /// The status line's message ("" when there is none).
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The disk (the tests' pretend one is looked at through this).
    pub fn disk(&self) -> &D {
        &self.disk
    }

    /// The disk, to change (the program mounts what the picker chose through this).
    pub fn disk_mut(&mut self) -> &mut D {
        &mut self.disk
    }

    /// The lines for the log since the last call, without the program's prefix: what was
    /// opened (`opened /Home/Documents/a.txt (12 bytes, crc 1a2b3c4d)`), saved, `new`,
    /// `asked save changes`, and what went wrong (`couldn't ...`).
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Whether the text may have changed (typed in, opened, pasted, New, undone...) since the
    /// last call. The program then writes [`Notepad::text_line`], in test boots.
    pub fn take_text_changed(&mut self) -> bool {
        core::mem::take(&mut self.text_changed)
    }

    /// `text 3 lines 17 bytes crc 1a2b3c4d`: how many lines the text has, how many bytes it is
    /// as UTF-8 with its lines joined by `\n`, and the CRC-32 (`lib/crc`) of those bytes. It is
    /// what the desktop boot's runner compares with what it typed and pasted.
    pub fn text_line(&self) -> String {
        let lines = self.content.line_count();
        let mut crc = 0;
        for n in 0..lines {
            if n > 0 {
                crc = pbr_crc::update(crc, b"\n");
            }
            crc = pbr_crc::update(crc, self.content.line(n).as_bytes());
        }
        format!("text {lines} lines {} bytes crc {crc:08x}", self.text_bytes())
    }

    /// How many bytes the text is as UTF-8 with `\n` between lines, without making a copy.
    fn text_bytes(&self) -> usize {
        let lines = self.content.line_count();
        (0..lines).map(|n| self.content.line(n).len()).sum::<usize>() + lines.saturating_sub(1)
    }

    // ---- The status line, and the log. ----

    /// Shows `text` in the status line for a few seconds.
    fn set_status(&mut self, text: &str, cx: &mut Context<'_, Message>) {
        self.status = String::from(text);
        self.status_id = self.status_id.wrapping_add(1);
        cx.after(STATUS_MS, Message::ClearStatus(self.status_id));
    }

    /// The text has a new content as a whole (opened, New): tell the program.
    fn replaced(&mut self) {
        self.text_changed = true;
    }

    // ---- Editing. ----

    /// Does what the user did to the text. A paste that would take the text over [`MAX_FILE`]
    /// is refused with a message; typing is not checked, since a key press adds a few bytes at
    /// most and the limit is checked where it matters, when a file is opened.
    fn edit(&mut self, action: Action, cx: &mut Context<'_, Message>) {
        if let Action::InsertText(text) = &action
            && self.text_bytes().saturating_add(text.len()) > MAX_FILE
        {
            self.log.push(String::from("couldn't paste: the text would be too big"));
            let words =
                format!("That is too much text for Notepad, which holds up to {}.", size_words());
            cx.show(Dialog::message("Notepad", &words));
            return;
        }
        if action.edits() {
            self.text_changed = true;
        }
        self.content.perform(action);
    }

    /// Edit > Copy.
    fn copy(&mut self, cx: &mut Context<'_, Message>) -> bool {
        let selected = self.content.selected_text();
        !selected.is_empty() && cx.copy_text(&selected)
    }

    /// Edit > Paste: the clipboard's text goes where the cursor is, over the selection.
    fn paste(&mut self, cx: &mut Context<'_, Message>) {
        if let Some(text) = cx.paste_text()
            && !text.is_empty()
        {
            self.edit(Action::InsertText(text), cx);
        }
    }

    // ---- Files. ----

    /// The name shown for the text: its file's, or "Untitled".
    fn name(&self) -> &str {
        self.path.as_deref().map_or("Untitled", base_name)
    }

    /// Asks the file picker, unless one is open already. The picker opens only for a press
    /// that is recent, which is why this is called from `update` with the context that has the
    /// press's serial. True if it was asked.
    fn ask(&mut self, what: Asked, pick: Pick, cx: &mut Context<'_, Message>) -> bool {
        if self.asked.is_some() {
            return false;
        }
        match self.disk.pick(cx.display_window(), cx.serial(), pick) {
            Ok(()) => {
                self.asked = Some(what);
                true
            }
            Err(why) => {
                self.picker_failed(what, &why, cx);
                false
            }
        }
    }

    /// There is no picker, or it couldn't answer: say so.
    fn picker_failed(&mut self, what: Asked, why: &str, cx: &mut Context<'_, Message>) {
        self.log.push(format!("couldn't use the picker: {why}"));
        let message = match what {
            Asked::Open => format!("Couldn't open the file picker. {why}."),
            Asked::Save => format!("Couldn't save the text. {why}."),
        };
        cx.show(Dialog::message("Notepad", &message));
    }

    /// Save (`ask_always` false) or Save as. True if the text was written to a file now; false
    /// if the picker was asked instead, or it failed.
    fn save(&mut self, ask_always: bool, cx: &mut Context<'_, Message>) -> bool {
        match self.path.clone() {
            Some(path) if !ask_always => self.write_to(&path, cx),
            _ => {
                let name = self.path.as_deref().map_or(FIRST_NAME, base_name);
                let name = String::from(name);
                self.ask(Asked::Save, Pick::Save { name }, cx);
                false
            }
        }
    }

    /// The bytes of the file for the text as it is: its lines joined, `\r\n` if the file had
    /// them, in Latin-1 if the file was, behind the byte order mark it had. A file read as
    /// Latin-1 that now has a character Latin-1 hasn't is saved as UTF-8 from now on: the
    /// second answer says so.
    fn encode(&mut self) -> (Vec<u8>, bool) {
        let mut text = self.content.text();
        if self.crlf {
            text = text.replace('\n', "\r\n");
        }
        let mut utf8_now = false;
        let mut bytes = Vec::with_capacity(text.len() + BOM.len());
        if self.bom {
            bytes.extend_from_slice(&BOM);
        }
        if self.latin1 && text.chars().all(|c| u32::from(c) <= 0xFF) {
            // Each character is one byte: the number is at most 255 (checked just above), so
            // the cast loses nothing.
            bytes.extend(text.chars().map(|c| u32::from(c) as u8));
        } else {
            utf8_now = self.latin1;
            self.latin1 = false;
            bytes.extend_from_slice(text.as_bytes());
        }
        (bytes, utf8_now)
    }

    /// Writes the text to `path`, and remembers it as the file. True if it was saved.
    fn write_to(&mut self, path: &str, cx: &mut Context<'_, Message>) -> bool {
        let (bytes, utf8_now) = self.encode();
        match self.disk.write(path, &bytes) {
            Ok(()) => {
                self.path = Some(String::from(path));
                self.content.set_saved();
                let crc = pbr_crc::crc32(&bytes);
                self.log.push(format!("saved {path} ({} bytes, crc {crc:08x})", bytes.len()));
                self.set_status(if utf8_now { "Saved as UTF-8" } else { "Saved" }, cx);
                true
            }
            Err(why) => {
                let why = why_text(&why);
                self.log.push(format!("couldn't save {path}: {why}"));
                let name = base_name(path);
                cx.show(Dialog::message("Notepad", &format!("Couldn't save {name}. {why}.")));
                false
            }
        }
    }

    /// Opens the file at `path`. `may_create` is for the file the program was started with: a
    /// file in Documents that is not there is then an empty text for a file to be made by the
    /// first save.
    fn open_file(&mut self, path: &str, may_create: bool, cx: &mut Context<'_, Message>) {
        let name = base_name(path);
        let (bytes, missing) = match self.disk.read(path, MAX_FILE) {
            Ok(bytes) => (bytes, false),
            Err(DiskError::NotFound) if may_create && in_documents(path) => (Vec::new(), true),
            Err(DiskError::TooBig) => {
                self.log.push(format!("couldn't open {path}: it is too big"));
                let words = format!(
                    "{name} is too big for Notepad, which opens files up to {}.",
                    size_words()
                );
                cx.show(Dialog::message("Notepad", &words));
                return;
            }
            Err(e) => {
                let why = why_text(&e);
                self.log.push(format!("couldn't open {path}: {why}"));
                cx.show(Dialog::message("Notepad", &format!("Couldn't open {name}. {why}.")));
                return;
            }
        };
        if bytes.contains(&0) {
            self.log.push(format!("couldn't open {path}: it has NUL bytes"));
            let words = format!("{name} doesn't look like a text file, so Notepad won't open it.");
            cx.show(Dialog::message("Notepad", &words));
            return;
        }
        let (crc, size) = (pbr_crc::crc32(&bytes), bytes.len());
        self.load(bytes);
        self.path = Some(String::from(path));
        self.replaced();
        self.log.push(format!("opened {path} ({size} bytes, crc {crc:08x})"));
        if self.latin1 {
            self.set_status("Opened as Latin-1 text: it isn't UTF-8", cx);
        } else if missing {
            self.set_status("New file: it is made when you save", cx);
        }
    }

    /// Makes `bytes` (a file with no NUL in it) the text.
    fn load(&mut self, bytes: Vec<u8>) {
        // A byte order mark counts only if what follows is UTF-8; otherwise the file is
        // Latin-1 from its first byte.
        let (rest, bom) = match bytes.strip_prefix(&BOM) {
            Some(rest) if core::str::from_utf8(rest).is_ok() => (rest, true),
            _ => (&bytes[..], false),
        };
        // `Ok` is a `Result`'s good answer, `Err` its bad one: a file that isn't UTF-8 is read
        // a byte per character instead of refused.
        let (text, latin1) = match core::str::from_utf8(rest) {
            Ok(text) => (String::from(text), false),
            Err(_) => (rest.iter().map(|&b| char::from(b)).collect(), true),
        };
        self.crlf = is_crlf(&text);
        self.latin1 = latin1;
        self.bom = bom;
        self.content = Content::from_text(&text);
    }

    /// File > New, once the old text is let go.
    fn new_text(&mut self) {
        self.content = Content::new();
        self.path = None;
        self.crlf = false;
        self.latin1 = false;
        self.bom = false;
        self.replaced();
        self.log.push(String::from("new"));
    }

    // ---- Asking before work is lost. ----

    /// Asks "Save changes to NAME?", to go on with `then` after Save or Don't save.
    fn ask_save_changes(&mut self, then: Pending, cx: &mut Context<'_, Message>) {
        // One question at a time (the picker's answer can come while one is up).
        if self.pending.is_some() {
            return;
        }
        self.log.push(String::from("asked save changes"));
        self.pending = Some(then);
        let question = format!("Save changes to {}?", self.name());
        cx.show(
            Dialog::question("Notepad", &question)
                .default_button("Save", Message::SaveChanges)
                .button("Don't save", Message::DiscardChanges)
                .cancel_button("Cancel", Message::KeepEditing),
        );
    }

    /// Goes on with what the user asked for, once the old text is saved or let go.
    fn carry_on(&mut self, then: Pending, cx: &mut Context<'_, Message>) {
        match then {
            Pending::New => self.new_text(),
            Pending::OpenFile(path) => self.open_file(&path, false, cx),
            Pending::Close => cx.quit(),
        }
    }

    /// File > New, Close and the like: straight away if nothing would be lost, else after the
    /// question.
    fn or_ask(&mut self, then: Pending, cx: &mut Context<'_, Message>) {
        if self.content.is_modified() {
            self.ask_save_changes(then, cx);
        } else {
            self.carry_on(then, cx);
        }
    }

    /// The answer Save to "Save changes?": write to the file there is, or ask the picker for
    /// one and carry on when it has answered.
    fn save_changes(&mut self, cx: &mut Context<'_, Message>) {
        if self.pending.is_none() {
            return;
        }
        if self.path.is_some() {
            if self.save(false, cx)
                && let Some(then) = self.pending.take()
            {
                self.carry_on(then, cx);
            } else {
                // The write failed (its message is up), so nothing is lost by stopping.
                self.pending = None;
            }
        } else if !self.save(true, cx) && self.asked != Some(Asked::Save) {
            // No picker to ask.
            self.pending = None;
        }
    }

    /// The file picker's answer: open or save what was chosen.
    fn picked(&mut self, cx: &mut Context<'_, Message>) {
        let Some(answer) = self.disk.answer() else { return };
        let what = self.asked.take();
        match (answer, what) {
            (PickAnswer::Chosen(path), Some(Asked::Open)) => {
                self.or_ask(Pending::OpenFile(path), cx);
            }
            (PickAnswer::Chosen(path), Some(Asked::Save)) => {
                let saved = self.write_to(&path, cx);
                match self.pending.take() {
                    Some(then) if saved => self.carry_on(then, cx),
                    _ => {}
                }
            }
            (PickAnswer::Cancelled, _) | (PickAnswer::Chosen(_), None) => self.pending = None,
            (PickAnswer::Failed(why), what) => {
                self.pending = None;
                self.picker_failed(what.unwrap_or(Asked::Open), &why, cx);
            }
        }
    }

    // ---- The parts of the view. ----

    /// The menu bar: File and Edit. The rows' shortcuts work while the menus are shut, so
    /// they are given here and nowhere else. (With the editor in focus its own keys for undo,
    /// redo, cut, copy, paste and select all answer first, with the same effect.)
    fn menu_bar(&self) -> Element<'_, Message> {
        let selected = self.content.selection().is_some();
        let file = Menu::new(
            "File",
            vec![
                MenuItem::new("New", Message::New).shortcut(Shortcut::ctrl('n')),
                MenuItem::new("Open…", Message::Open).shortcut(Shortcut::ctrl('o')),
                MenuItem::new("Save", Message::Save).shortcut(Shortcut::ctrl('s')),
                MenuItem::new("Save as…", Message::SaveAs).shortcut(Shortcut::ctrl_shift('s')),
                MenuItem::separator(),
                MenuItem::new("Close", Message::Close).shortcut(Shortcut::ctrl('w')),
            ],
        );
        let edit = Menu::new(
            "Edit",
            vec![
                MenuItem::new("Undo", Message::Undo)
                    .shortcut(Shortcut::ctrl('z'))
                    .enabled(self.content.can_undo()),
                MenuItem::new("Redo", Message::Redo)
                    .shortcut(Shortcut::ctrl('y'))
                    .enabled(self.content.can_redo()),
                MenuItem::separator(),
                MenuItem::new("Cut", Message::Cut).shortcut(Shortcut::ctrl('x')).enabled(selected),
                MenuItem::new("Copy", Message::Copy)
                    .shortcut(Shortcut::ctrl('c'))
                    .enabled(selected),
                MenuItem::new("Paste", Message::Paste)
                    .shortcut(Shortcut::ctrl('v'))
                    .enabled(self.paste_ok),
                MenuItem::separator(),
                MenuItem::new("Select all", Message::SelectAll).shortcut(Shortcut::ctrl('a')),
            ],
        );
        MenuBar::new(vec![file, edit]).into()
    }

    /// The status line: the message, if there is one, at the left; the cursor's line and
    /// column (1.3's "Ln 1, Col 1") at the right.
    fn status_line(&self) -> Element<'_, Message> {
        let (line, column) = self.content.cursor();
        let mut bar = StatusBar::new();
        if !self.status.is_empty() {
            bar = bar.push(&self.status);
        }
        bar.push_right(&format!("Ln {}, Col {}", line + 1, column + 1)).into()
    }
}

/// The window: opens at [`WINDOW_SIZE`], and can be resized down to [`MIN_SIZE`].
pub fn settings() -> Settings {
    Settings { size: WINDOW_SIZE, resizable: true, min_size: MIN_SIZE, over: None, system: false }
}

impl<D: Disk> App for Notepad<D> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.menu_bar())
            .push(TextEditor::new(&self.content).on_action(Message::Edit).id(EDITOR_ID))
            .push(self.status_line())
            .into()
    }

    /// "NAME" or "Untitled", with "• " before it while there are unsaved changes. Sandstone
    /// puts "Notepad" before it.
    fn title(&self) -> String {
        format!("{}{}", if self.content.is_modified() { "• " } else { "" }, self.name())
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        cx.focus(EDITOR_ID);
        self.paste_ok = cx.clipboard().0;
        if let Some(path) = self.initial.take() {
            self.open_file(&path, true, cx);
        }
    }

    /// The window's close button and Ctrl+W: the app decides, so it can ask first.
    fn close_requested(&self, _window: WindowId) -> Option<Message> {
        Some(Message::Close)
    }

    fn clipboard_changed(&self, text: bool, _picture: bool) -> Option<Message> {
        Some(Message::Clipboard(text))
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Edit(action) => self.edit(action, cx),
            Message::New => self.or_ask(Pending::New, cx),
            Message::Open => {
                self.ask(Asked::Open, Pick::Open, cx);
            }
            Message::Save => {
                self.save(false, cx);
            }
            Message::SaveAs => {
                self.save(true, cx);
            }
            Message::Close => self.or_ask(Pending::Close, cx),
            Message::Undo => self.edit(Action::Undo, cx),
            Message::Redo => self.edit(Action::Redo, cx),
            Message::SelectAll => self.edit(Action::SelectAll, cx),
            Message::Cut => {
                // Only cut what was copied: a copy the display server refuses must not lose
                // the text.
                if self.copy(cx) {
                    self.edit(Action::DeleteSelection, cx);
                }
            }
            Message::Copy => {
                self.copy(cx);
            }
            Message::Paste => self.paste(cx),
            Message::Picked => self.picked(cx),
            Message::Clipboard(text) => self.paste_ok = text,
            Message::ClearStatus(id) => {
                if id == self.status_id {
                    self.status.clear();
                }
            }
            Message::SaveChanges => self.save_changes(cx),
            Message::DiscardChanges => {
                if let Some(then) = self.pending.take() {
                    self.carry_on(then, cx);
                }
            }
            Message::KeepEditing => self.pending = None,
        }
    }

    fn notified(&self, bits: u64) -> Option<Message> {
        (bits & (1 << PICKED) != 0).then_some(Message::Picked)
    }
}

#[cfg(test)]
mod tests;
