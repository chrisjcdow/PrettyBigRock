// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Files (docs/design/0026-apps-m6-tests-and-stages.md, section 1): PrettyBigRock 1.3's file
//! manager as a sandstone app, tested on the host.
//!
//! It does everything 1.3's Files did (`fl_*` in `classic/kernel/notepad.c`): shows a folder's
//! contents, folders first and then by name; Up; Open; New file; New folder; Rename; and Delete,
//! which asks first, all with 1.3's dialogs' words and the answers 1.3 gave when a name was bad
//! or taken or a folder wasn't empty. It looks at the folder again every two seconds, as 1.3
//! did when the disk changed, and keeps the selection on the same name. On top of that it has
//! what note 0026 adds: **places** down the left side (as the file picker has them), sizes and
//! dates in columns you can sort by, the dates in the user's time zone, a path bar,
//! double-click to open, a right-click menu, icons by kind of file, and **Open with…**
//! (note 0025, section 3).
//!
//! **Everything outside this library is reached through the [`System`] trait**: the file tree
//! (the trait [`Folders`] of `lib/browse`, which the file picker shares) and starting other
//! apps. `main.rs` implements them over `lib/files` and the sandbox manager; the tests, over
//! `pbr_browse::MemTree` and a pretend launcher. So what the dialogs say, how a file is opened
//! and what each menu offers are all tested on the host.
//!
//! A word on Rust for a newcomer. `#![no_std]` (below, except in tests) means this crate uses
//! only Rust's `core` and `alloc` libraries, not the operating system's `std`, so that it can
//! run as a PrettyBigRock program; `alloc` holds `String`, `Vec` and `format!`, which need a
//! heap. A *trait* is a set of methods a type promises to have: `FileManager<S: System>` is "a
//! file manager over any `S` that has them". An `enum` is a value that is exactly one of several
//! kinds, and `match` takes it apart; sandstone's apps use one for every message their widgets
//! can send.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_browse::kind::kind_of;
use pbr_browse::places::{place_of, places};
use pbr_browse::{
    BrowseError, Folders, Item, Place, Sort, SortBy, format as show, item as order, path,
};
use pbr_sandstone::{
    Align, App, Button, Column, Context, ContextMenu, Dialog, Element, Identify, Image, Key, Label,
    Length, List, ListColumn, ListRow, MenuItem, Panel, Row, Settings, Shortcut, Space, StatusBar,
    Stretch, Theme, Toolbar, WindowId, WindowSpec,
};

/// How often the folder is listed again to see what changed behind the window's back, in
/// milliseconds (1.3 looked every time the disk's generation number moved; 2.0's tree has no
/// such number to read, so it asks).
pub const REFRESH_MS: u64 = 2000;
/// The app that opens a file no app lists the extension of, as 1.3 sent every file that wasn't
/// a `.bmp` to Notepad.
pub const FALLBACK_APP: &str = "notepad";
/// The folder Files opens in when it is not told another.
pub const HOME: &str = "/Home";
/// The name offered by New file and New folder (1.3's).
const NEW_FILE_NAME: &str = "Untitled.txt";
const NEW_FOLDER_NAME: &str = "New folder";

/// The window: 560 x 360 logical pixels, resizable down to [`MIN_SIZE`]. On a screen too small
/// for it (a 2x screen of 1024 x 768 is 512 x 384 logical pixels) the display server opens a
/// resizable window at the size of the screen's work area instead (note 0022, Changes).
pub fn settings() -> Settings {
    Settings { size: (560, 360), resizable: true, min_size: MIN_SIZE, over: None, system: false }
}

/// The smallest the window can be made: wide enough for the toolbar's six buttons, and so that
/// with the title bar it fits a 512 x 384 screen.
pub const MIN_SIZE: (u32, u32) = (400, 220);

// ---------------------------------------------------------------------------------------------
// What Files asks of the system
// ---------------------------------------------------------------------------------------------

/// An app that can open files, as the sandbox manager's `openers` says (note 0025, section 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opener {
    /// The app's name, as the sandbox manager launches it (`notepad`).
    pub app: String,
    /// Its manifest's title (`Notepad`), which is what the user reads.
    pub title: String,
    /// The extensions it says it opens, without the dot (`txt`, `md`), in its manifest's order.
    pub opens: Vec<String>,
}

/// Everything Files does besides showing and changing the tree: the file tree itself, as the
/// file picker browses it, and the sandbox manager.
///
/// Files is *trusted* (note 0026, section 8): it holds the session's whole tree, and a
/// `pbr.sandbox` capability that may launch other apps. Launching an app with a file hands that
/// app a capability for the one file (note 0017, section 6), which is why the launching is done
/// by the program and not by this library.
pub trait System: Folders {
    /// The apps that can open a file, in name order: every app with windows that isn't hidden.
    /// Empty if the sandbox manager couldn't say.
    fn openers(&mut self) -> Vec<Opener>;
    /// Starts `app` with the file at `path` (read and write if the user may write it) and gives
    /// back the new app's number, or says in words why not.
    fn open_with(&mut self, app: &str, path: &str) -> Result<u32, String>;
}

// ---------------------------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------------------------

/// What the window's widgets send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Nothing: a dialog's button that only closes it.
    Nothing,
    /// One of the places on the left.
    Place(usize),
    /// One of the path bar's parts.
    Crumb(usize),
    /// Up to the folder above.
    Up,
    /// A row picked.
    Select(usize),
    /// The empty part of the list pressed: nothing is picked.
    Clear,
    /// A row opened (double click, Enter).
    Activate(usize),
    /// A click on a heading.
    Sort(usize),
    /// The open button or menu row: the selected item.
    Open,
    /// Open with… (a row of the right-click menu): asks which app.
    OpenWith,
    /// New file: asks for a name.
    NewFile,
    /// The name the New file dialog gave.
    NewFileNamed(String),
    /// New folder: asks for a name.
    NewFolder,
    /// The name the New folder dialog gave.
    NewFolderNamed(String),
    /// Rename: asks for the new name.
    Rename,
    /// The new name the Rename dialog gave.
    RenamedTo(String),
    /// Delete: asks first.
    Delete,
    /// The Delete question answered "Delete".
    DeleteConfirmed,
    /// F5: list the folder again.
    Refresh,
    /// The timer: list the folder again, and set the timer again.
    Tick,
    /// In the Open with… window: an app picked.
    WithSelect(usize),
    /// In the Open with… window: an app opened (double click, Enter).
    WithActivate(usize),
    /// In the Open with… window: the Open button.
    WithOpen,
    /// In the Open with… window: Cancel, Escape or the close button.
    WithCancel,
}

/// One app offered by Open with….
#[derive(Clone, Debug, PartialEq, Eq)]
struct Choice {
    /// What is launched.
    app: String,
    /// What the user reads.
    title: String,
    /// It says it opens this file's extension (such apps are listed first).
    suggested: bool,
}

/// The Open with… window, while it is open.
#[derive(Clone, Debug)]
struct WithWindow {
    window: WindowId,
    /// The file's path and its name.
    path: String,
    name: String,
    /// The extension the suggestions are for (as the file has it), if it has one.
    ext: Option<String>,
    choices: Vec<Choice>,
    selected: usize,
}

/// The Files window, over a [`System`].
pub struct FileManager<S: System> {
    sys: S,
    /// The user's time zone, minutes east of UTC, for the Modified column.
    offset_minutes: i32,
    places: Vec<Place>,
    /// The folder shown.
    folder: String,
    /// What is in it (hidden things left out), in the order shown.
    items: Vec<Item>,
    selected: Option<usize>,
    sort: Sort,
    /// Why the folder couldn't be listed, shown where its contents would be.
    problem: Option<String>,
    /// The apps that open files, once the sandbox manager has said (empty before, and if it
    /// couldn't).
    openers: Vec<Opener>,
    /// The name Rename was asked about, and the name Delete is asking about.
    renaming: Option<String>,
    deleting: Option<String>,
    /// The Open with… window.
    with: Option<WithWindow>,
    /// Lines for the program's log, without its prefix.
    log: Vec<String>,
    /// What the last `folder` line said, so that it is written again only when it changes.
    logged: Option<(String, Vec<Item>)>,
}

impl<S: System> FileManager<S> {
    /// A file manager over `sys`, showing Home. `offset_minutes` is the user's time zone.
    pub fn new(mut sys: S, offset_minutes: i32) -> FileManager<S> {
        // An administrator's tree has every volume under `/Volumes` (note 0017, section 3); a
        // standard user's has none, so is shown none (as the file picker decides).
        let volumes: Vec<String> = match sys.list("/Volumes") {
            Ok(mut v) => {
                order::sort(&mut v, Sort::default());
                v.into_iter().filter(|i| i.folder).map(|i| i.name).collect()
            }
            Err(_) => Vec::new(),
        };
        let names: Vec<&str> = volumes.iter().map(String::as_str).collect();
        let places = places(!names.is_empty(), &names);
        let mut m = FileManager {
            sys,
            offset_minutes,
            places,
            folder: String::from("/"),
            items: Vec::new(),
            selected: None,
            sort: Sort::default(),
            problem: None,
            openers: Vec::new(),
            renaming: None,
            deleting: None,
            with: None,
            log: Vec::new(),
            logged: None,
        };
        // Home if the tree has one, else the top.
        let home = if m.sys.list(HOME).is_ok() { HOME } else { "/" };
        m.go(home);
        m
    }

    /// Starts at `path`: a folder is shown; a file shows the folder it is in, with it selected;
    /// anything else (or nothing the program can reach) leaves Home shown and says so.
    pub fn starting_at(mut self, path: &str) -> FileManager<S> {
        match self.sys.stat(path) {
            Ok(item) if item.folder => self.go(path),
            Ok(item) => {
                if let Some(parent) = path::parent(path).map(String::from) {
                    self.go(&parent);
                    self.selected = self.items.iter().position(|i| i.name == item.name);
                }
            }
            Err(e) => {
                self.problem = Some(describe(&e));
                self.log.push(format!("couldn't show {path}: {}", describe(&e)));
            }
        }
        self
    }

    /// The system Files runs over (the tests look at what it was asked).
    pub fn system(&self) -> &S {
        &self.sys
    }

    /// The same, to change it behind the window's back, as another program would.
    pub fn system_mut(&mut self) -> &mut S {
        &mut self.sys
    }

    /// The folder shown.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// The names shown, in order.
    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|i| i.name.as_str()).collect()
    }

    /// The name selected, if any.
    pub fn selected_name(&self) -> Option<&str> {
        self.selected.and_then(|i| self.items.get(i)).map(|i| i.name.as_str())
    }

    /// The places offered, by title.
    pub fn place_titles(&self) -> Vec<&str> {
        self.places.iter().map(|p| p.title.as_str()).collect()
    }

    /// Why the folder couldn't be shown, if it couldn't.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The apps the Open with… window offers now, by title, in order (empty when it is shut).
    pub fn with_titles(&self) -> Vec<&str> {
        self.with.iter().flat_map(|w| w.choices.iter().map(|c| c.title.as_str())).collect()
    }

    /// The lines for the program's log since the last call, without its prefix (`files:`): what
    /// was made, renamed, deleted and opened, what went wrong, and the folder shown whenever it
    /// or what is in it changes.
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    // ---- The folder. ----

    /// Shows the folder at `to` (or says why it can't be).
    fn go(&mut self, to: &str) {
        match self.sys.list(to) {
            Ok(_) => {
                self.folder = String::from(to);
                self.selected = None;
                self.refresh();
            }
            Err(e) => {
                self.problem = Some(describe(&e));
                self.log.push(format!("couldn't show {to}: {}", describe(&e)));
            }
        }
    }

    /// Lists the folder shown again, keeping the selection on the same name. A folder that has
    /// gone (another program removed it) leaves the nearest one above it that is still there,
    /// as 1.3 went to the top.
    fn refresh(&mut self) {
        let keep = self.selected_name().map(String::from);
        let mut listed = self.sys.list(&self.folder);
        while matches!(listed, Err(BrowseError::NotFound | BrowseError::NotAFolder)) {
            let Some(above) = path::parent(&self.folder).map(String::from) else { break };
            self.folder = above;
            self.selected = None;
            listed = self.sys.list(&self.folder);
        }
        let mut items = match listed {
            Ok(items) => {
                self.problem = None;
                items
            }
            Err(e) => {
                self.problem = Some(describe(&e));
                Vec::new()
            }
        };
        items.retain(|i| !i.hidden);
        order::sort(&mut items, self.sort);
        self.selected = keep.and_then(|n| items.iter().position(|i| i.name == n));
        self.items = items;
        // What is in the folder, whatever order it is shown in.
        let mut contents = self.items.clone();
        contents.sort_by(|a, b| a.name.cmp(&b.name));
        let now = (self.folder.clone(), contents);
        if self.logged.as_ref() != Some(&now) {
            self.log.push(format!("folder {} ({} items)", self.folder, self.items.len()));
            self.logged = Some(now);
        }
    }

    /// Lists again and selects `name` (after something was made or renamed).
    fn refresh_selecting(&mut self, name: &str) {
        self.selected = None;
        self.refresh();
        self.selected = self.items.iter().position(|i| i.name == name);
    }

    /// The selected item, and its path.
    fn picked(&self) -> Option<(Item, String)> {
        let item = self.selected.and_then(|i| self.items.get(i))?.clone();
        let at = path::join(&self.folder, &item.name);
        Some((item, at))
    }

    /// A problem with something the user asked for, in a dialog, in 1.3's words:
    /// "Couldn't make the file. Something with that name already exists."
    fn report(&mut self, what: &str, e: &BrowseError, cx: &mut Context<'_, Message>) {
        let why = describe(e);
        self.log.push(format!("couldn't {what}: {why}"));
        let why = why.trim_end_matches('.');
        cx.show(Dialog::message("Files", &format!("Couldn't {what}. {why}.")));
    }

    // ---- Opening. ----

    /// Opens the item at `index`: a folder in this window, a file in the app that opens it.
    fn open_item(&mut self, index: usize, cx: &mut Context<'_, Message>) {
        let Some(item) = self.items.get(index).cloned() else { return };
        self.selected = Some(index);
        let at = path::join(&self.folder, &item.name);
        if item.folder {
            self.go(&at);
        } else {
            self.open_file(&at, cx);
        }
    }

    /// Opens the file at `at` in the app whose manifest `opens` its extension, or in Notepad if
    /// none does (note 0025, section 3).
    fn open_file(&mut self, at: &str, cx: &mut Context<'_, Message>) {
        let ext = path::extension(path::name(at)).map(str::to_ascii_lowercase);
        let app = ext
            .as_deref()
            .and_then(|ext| {
                self.load_openers()
                    .into_iter()
                    .find(|o| o.opens.iter().any(|e| e.eq_ignore_ascii_case(ext)))
            })
            .map_or_else(|| String::from(FALLBACK_APP), |o| o.app);
        self.launch(&app, at, cx);
    }

    /// Starts `app` with the file at `at`, and tells the user if it didn't start.
    fn launch(&mut self, app: &str, at: &str, cx: &mut Context<'_, Message>) {
        self.log.push(format!("opening {at} with {app}"));
        match self.sys.open_with(app, at) {
            Ok(number) => self.log.push(format!("opened {at} with {app} as app {number}")),
            Err(why) => {
                self.log.push(format!("couldn't open {at} with {app}: {why}"));
                let why = why.trim_end_matches('.');
                let name = path::name(at);
                cx.show(Dialog::message("Files", &format!("Couldn't open {name}. {why}.")));
            }
        }
    }

    /// The apps that open files, asked of the sandbox manager the first time they are needed
    /// (and again if it couldn't say: the program waits only a few seconds for it).
    fn load_openers(&mut self) -> Vec<Opener> {
        if self.openers.is_empty() {
            self.openers = self.sys.openers();
        }
        self.openers.clone()
    }

    /// Opens the Open with… window for the selected file: every app that opens its extension
    /// first, then every other app, each group by title.
    fn ask_open_with(&mut self, cx: &mut Context<'_, Message>) {
        let Some((item, at)) = self.picked().filter(|(item, _)| !item.folder) else { return };
        let ext = path::extension(&item.name).map(str::to_ascii_lowercase);
        let mut choices: Vec<Choice> = self
            .load_openers()
            .into_iter()
            .map(|o| Choice {
                suggested: ext
                    .as_deref()
                    .is_some_and(|ext| o.opens.iter().any(|e| e.eq_ignore_ascii_case(ext))),
                app: o.app,
                title: o.title,
            })
            .collect();
        if choices.is_empty() {
            self.log.push(String::from("couldn't list the apps to open with"));
            cx.show(Dialog::message("Files", "Couldn't find the apps to open it with."));
            return;
        }
        // `sort_by_key` is stable: apps with the same title stay in the sandbox manager's order.
        choices.sort_by_key(|c| (!c.suggested, c.title.to_lowercase()));
        let window = cx.open_window(WindowSpec {
            dialog: true,
            modal: true,
            resizable: false,
            size: (WITH_SIZE.0, WITH_SIZE.1),
        });
        cx.focus_in(window, "apps");
        self.with =
            Some(WithWindow { window, path: at, name: item.name, ext, choices, selected: 0 });
    }

    /// Opens the file in the Open with… window with the app picked there, and closes it.
    fn open_chosen(&mut self, cx: &mut Context<'_, Message>) {
        let Some(with) = self.with.take() else { return };
        cx.close_window(with.window);
        if let Some(choice) = with.choices.get(with.selected) {
            self.launch(&choice.app, &with.path, cx);
        }
    }

    // ---- Making, renaming and deleting. ----

    /// The name a dialog gave, ready to use: nothing for an empty one, as 1.3 did. A name that
    /// can't be one (it has a `/` in it, say) is reported with the words of `what`, as 1.3's
    /// file system reported it, and is nothing too.
    fn typed(&mut self, name: &str, what: &str, cx: &mut Context<'_, Message>) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        match path::check_name(name) {
            Ok(()) => Some(String::from(name)),
            Err(e) => {
                self.report(what, &e, cx);
                None
            }
        }
    }

    fn new_file(&mut self, name: &str, cx: &mut Context<'_, Message>) {
        let Some(name) = self.typed(name, "make the file", cx) else { return };
        let at = path::join(&self.folder, &name);
        match self.sys.make_file(&at) {
            Ok(()) => {
                self.log.push(format!("made {at}"));
                self.refresh_selecting(&name);
                // 1.3 opened the new file in Notepad at once.
                self.open_file(&at, cx);
            }
            Err(e) => self.report("make the file", &e, cx),
        }
    }

    fn new_folder(&mut self, name: &str, cx: &mut Context<'_, Message>) {
        let Some(name) = self.typed(name, "make the folder", cx) else { return };
        let at = path::join(&self.folder, &name);
        match self.sys.make_folder(&at) {
            Ok(()) => {
                self.log.push(format!("made {at}"));
                self.refresh_selecting(&name);
            }
            Err(e) => self.report("make the folder", &e, cx),
        }
    }

    fn rename(&mut self, to: &str, cx: &mut Context<'_, Message>) {
        let Some(from) = self.renaming.take() else { return };
        let Some(to) = self.typed(to, "rename it", cx).filter(|to| *to != from) else { return };
        let (a, b) = (path::join(&self.folder, &from), path::join(&self.folder, &to));
        match self.sys.rename(&a, &b) {
            Ok(()) => {
                self.log.push(format!("renamed {a} to {b}"));
                self.refresh_selecting(&to);
            }
            Err(e) => self.report("rename it", &e, cx),
        }
    }

    fn delete(&mut self, cx: &mut Context<'_, Message>) {
        let Some(name) = self.deleting.take() else { return };
        let at = path::join(&self.folder, &name);
        match self.sys.remove(&at) {
            Ok(()) => {
                self.log.push(format!("deleted {at}"));
                self.refresh();
            }
            Err(e) => self.report("delete it", &e, cx),
        }
    }

    // ---- The parts of the view. ----

    /// The toolbar, with 1.3's buttons in 1.3's order: Up, Open, New file, New folder, Rename
    /// and Delete. Those that act on the selected item are greyed out without one.
    fn toolbar(&self) -> Element<'_, Message> {
        let picked = self.selected_name().is_some();
        let tool = |label: &str, width: i32, message: Message, enabled: bool| {
            Button::new(label)
                .on_press_maybe(enabled.then_some(message))
                .width(Length::Fixed(width))
                .id(&label.to_lowercase().replace(' ', ""))
        };
        Toolbar::new()
            .push(tool("Up", 40, Message::Up, self.folder != "/"))
            .push(tool("Open", 50, Message::Open, picked))
            .separator()
            .push(tool("New file", 70, Message::NewFile, true))
            .push(tool("New folder", 80, Message::NewFolder, true))
            .separator()
            .push(tool("Rename", 64, Message::Rename, picked))
            .push(tool("Delete", 58, Message::Delete, picked))
            .into()
    }

    /// The path bar: the parts of the folder's path, each a button that goes there.
    fn path_bar(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let crumbs = path::crumbs(&self.folder);
        // A deep path keeps only its last parts, so that the bar stays on one line.
        let first = crumbs.len().saturating_sub(MAX_CRUMBS);
        let mut row = Row::new().spacing(t.spacing).align(Align::Center).width(Length::Fill);
        if first > 0 {
            row = row.push(Label::new("…"));
        }
        for (i, (name, _)) in crumbs.iter().enumerate().skip(first) {
            row = row.push(Button::new(name).on_press(Message::Crumb(i)).id(&format!("crumb{i}")));
        }
        row.into()
    }

    /// The places on the left.
    fn places_list(&self) -> Element<'_, Message> {
        let rows = self
            .places
            .iter()
            .map(|p| ListRow::new().icon_maybe(image(p.kind.icon())).cell(p.title.as_str()))
            .collect();
        let here = place_of(&self.folder, &self.places);
        List::new(vec![ListColumn::new("Places", Length::Fill)], rows)
            .selected(here)
            .on_select(Message::Place)
            .width(Length::Fixed(PLACES_WIDTH))
            .height(Length::Fill)
            .id("places")
    }

    /// The folder's contents, with the menu a right click opens. The same menu serves a row and
    /// the empty part below the rows: the popup is made before the click has picked the row, so
    /// it has the same rows either way, and those that act on a selected item are greyed out
    /// when nothing is.
    fn contents(&self) -> Element<'_, Message> {
        let rows = self
            .items
            .iter()
            .map(|i| {
                let size = if i.folder { String::new() } else { show::size(i.size) };
                ListRow::new()
                    .icon_maybe(image(kind_of(&i.name, i.folder).icon()))
                    .cell(i.name.as_str())
                    .cell(size)
                    .cell(show::date(i.modified, self.offset_minutes))
            })
            .collect();
        let column = match self.sort.by {
            SortBy::Name | SortBy::Kind => 0,
            SortBy::Size => 1,
            SortBy::Modified => 2,
        };
        let empty = self.problem.as_deref().unwrap_or("This folder is empty.");
        let list = List::new(
            vec![
                ListColumn::new("Name", Length::Fill),
                ListColumn::new("Size", Length::Fixed(SIZE_WIDTH)),
                ListColumn::new("Modified", Length::Fixed(DATE_WIDTH)),
            ],
            rows,
        )
        .selected(self.selected)
        .sorted(column, self.sort.ascending)
        .on_select(Message::Select)
        .on_activate(Message::Activate)
        .on_sort(Message::Sort)
        .on_empty_press(|| Message::Clear)
        .empty(empty)
        .width(Length::Fill)
        .height(Length::Fill)
        .id("files");
        ContextMenu::new(list, self.menu_items()).into()
    }

    /// The right-click menu's rows.
    fn menu_items(&self) -> Vec<MenuItem<Message>> {
        let picked = self.picked();
        let any = picked.is_some();
        let file = picked.is_some_and(|(item, _)| !item.folder);
        vec![
            MenuItem::new("Open", Message::Open).enabled(any),
            MenuItem::new("Open with…", Message::OpenWith).enabled(file),
            MenuItem::new("Rename", Message::Rename)
                .shortcut(Shortcut::key(Key::Function(2)))
                .enabled(any),
            MenuItem::new("Delete", Message::Delete)
                .shortcut(Shortcut::key(Key::Delete))
                .enabled(any),
            MenuItem::separator(),
            MenuItem::new("New file", Message::NewFile),
            MenuItem::new("New folder", Message::NewFolder),
        ]
    }

    /// The line along the bottom: the problem with the folder, or how many items it has.
    fn status(&self) -> Element<'_, Message> {
        let n = self.items.len();
        let text = match &self.problem {
            Some(p) => p.clone(),
            None => format!("{n} item{}", if n == 1 { "" } else { "s" }),
        };
        let bar = StatusBar::new().push(&text);
        match self.picked() {
            Some((item, _)) => bar.push_right(kind_of(&item.name, item.folder).title()),
            None => bar,
        }
        .into()
    }

    /// The Open with… window's contents.
    fn with_view<'a>(&'a self, w: &'a WithWindow) -> Element<'a, Message> {
        let t = Theme::STANDARD;
        let rows = w
            .choices
            .iter()
            .map(|c| {
                // Those that open this kind of file say so, which is why they come first.
                let opens = match (&w.ext, c.suggested) {
                    (Some(ext), true) => format!(".{ext} files"),
                    _ => String::new(),
                };
                ListRow::new().cell(c.title.as_str()).cell(opens)
            })
            .collect();
        let list = List::new(
            vec![ListColumn::new("App", Length::Fill), ListColumn::new("Opens", Length::Fixed(90))],
            rows,
        )
        .selected(Some(w.selected))
        .on_select(Message::WithSelect)
        .on_activate(Message::WithActivate)
        .width(Length::Fill)
        .height(Length::Fill)
        .id("apps");
        let buttons = Row::new()
            .spacing(t.spacing)
            .push(Stretch::new())
            .push(Button::new("Cancel").on_press(Message::WithCancel).cancel().id("withcancel"))
            .push(Button::new("Open").on_press(Message::WithOpen).default().id("withopen"));
        Panel::new(
            Column::new()
                .padding(t.padding)
                .spacing(t.spacing)
                .width(Length::Fill)
                .height(Length::Fill)
                .push(Label::new(&format!("Open \"{}\" with:", w.name)).wrap())
                .push(list)
                .push(buttons.width(Length::Fill)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// The most parts of the path the bar shows.
const MAX_CRUMBS: usize = 4;
/// The places' width, and the Size and Modified columns' ("999 bytes" and "14 Nov 2026 21:09"
/// at the theme's font).
const PLACES_WIDTH: i32 = 110;
const SIZE_WIDTH: i32 = 70;
const DATE_WIDTH: i32 = 124;
/// The Open with… window's size.
const WITH_SIZE: (u32, u32) = (320, 260);

/// A 16 by 16 icon as sandstone's [`Image`].
fn image(pixels: &'static [u32; 256]) -> Option<Image<'static>> {
    Image::new(pixels, 16, 16)
}

/// What went wrong, in 1.3's words where it had them (`rfs_strerror`), for "Couldn't make the
/// file. Something with that name already exists."
pub fn describe(e: &BrowseError) -> String {
    match e {
        BrowseError::NotFound => String::from("Not found"),
        BrowseError::NotAFolder => String::from("That isn't a folder"),
        BrowseError::AlreadyExists => String::from("Something with that name already exists"),
        BrowseError::NotEmpty => String::from("The folder isn't empty"),
        BrowseError::NoRight => String::from("Nothing can be changed here"),
        BrowseError::BadName => {
            String::from("A name can't be empty, contain / or begin or end with a space")
        }
        BrowseError::Other(text) => text.clone(),
    }
}

impl<S: System> App for FileManager<S> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let body = Column::new()
            .padding(t.padding)
            .spacing(t.spacing)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.path_bar())
            .push(
                Row::new()
                    .spacing(t.spacing)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .push(self.places_list())
                    .push(self.contents()),
            );
        Panel::new(
            Column::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .push(self.toolbar())
                .push(body)
                .push(Space::new(0, 0))
                .push(self.status()),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// The folder's path: sandstone puts "Files — " before it.
    fn title(&self) -> String {
        self.folder.clone()
    }

    fn view_window(&self, window: WindowId) -> Option<Element<'_, Message>> {
        self.with.as_ref().filter(|w| w.window == window).map(|w| self.with_view(w))
    }

    fn window_title(&self, window: WindowId) -> String {
        match &self.with {
            Some(w) if w.window == window => String::from("Open with"),
            _ => String::new(),
        }
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        cx.focus("files");
        cx.after(REFRESH_MS, Message::Tick);
    }

    fn shortcuts(&self) -> Vec<(Shortcut, Message)> {
        vec![
            (Shortcut::key(Key::Delete), Message::Delete),
            (Shortcut::key(Key::Function(2)), Message::Rename),
            (Shortcut::key(Key::Function(5)), Message::Refresh),
            (Shortcut::key(Key::Backspace), Message::Up),
        ]
    }

    /// Closing the Open with… window is Cancel; closing the main window ends the app.
    fn close_requested(&self, window: WindowId) -> Option<Message> {
        (window != WindowId::MAIN).then_some(Message::WithCancel)
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Nothing => {}
            Message::Place(i) => {
                if let Some(p) = self.places.get(i) {
                    let to = p.path.clone();
                    self.go(&to);
                }
            }
            Message::Crumb(i) => {
                if let Some((_, to)) = path::crumbs(&self.folder).get(i).cloned() {
                    self.go(&to);
                }
            }
            Message::Up => {
                if let Some(up) = path::parent(&self.folder).map(String::from) {
                    self.go(&up);
                }
            }
            Message::Select(i) => self.selected = (i < self.items.len()).then_some(i),
            Message::Clear => self.selected = None,
            Message::Activate(i) => self.open_item(i, cx),
            Message::Sort(column) => {
                let by = match column {
                    1 => SortBy::Size,
                    2 => SortBy::Modified,
                    _ => SortBy::Name,
                };
                // A click on the column already used turns the order round.
                let ascending = if self.sort.by == by { !self.sort.ascending } else { true };
                self.sort = Sort { by, ascending };
                self.refresh();
            }
            Message::Open => {
                if let Some(i) = self.selected {
                    self.open_item(i, cx);
                }
            }
            Message::OpenWith => self.ask_open_with(cx),
            Message::NewFile => {
                cx.show(
                    Dialog::input("New file", "Name for the new file:", NEW_FILE_NAME, |n| {
                        Message::NewFileNamed(n)
                    })
                    .button_labels(Some("Make it"), None),
                );
            }
            Message::NewFileNamed(name) => self.new_file(&name, cx),
            Message::NewFolder => {
                cx.show(
                    Dialog::input("New folder", "Name for the new folder:", NEW_FOLDER_NAME, |n| {
                        Message::NewFolderNamed(n)
                    })
                    .button_labels(Some("Make it"), None),
                );
            }
            Message::NewFolderNamed(name) => self.new_folder(&name, cx),
            Message::Rename => {
                if let Some((item, _)) = self.picked() {
                    cx.show(
                        Dialog::input(
                            "Rename",
                            &format!("New name for \"{}\":", item.name),
                            &item.name,
                            Message::RenamedTo,
                        )
                        .button_labels(Some("Rename"), None),
                    );
                    self.renaming = Some(item.name);
                }
            }
            Message::RenamedTo(name) => self.rename(&name, cx),
            Message::Delete => {
                if let Some((item, _)) = self.picked() {
                    let question = if item.folder {
                        format!("Delete the folder \"{}\"? It has to be empty.", item.name)
                    } else {
                        format!("Delete \"{}\"? This can't be undone.", item.name)
                    };
                    // "Keep it" is the button Enter chooses, so that a stray Enter can't delete
                    // anything; Escape and the close button also keep it.
                    cx.show(
                        Dialog::question("Delete", &question)
                            .button("Delete", Message::DeleteConfirmed)
                            .default_button("Keep it", Message::Nothing),
                    );
                    self.deleting = Some(item.name);
                }
            }
            Message::DeleteConfirmed => self.delete(cx),
            Message::Refresh => {
                self.refresh();
            }
            Message::Tick => {
                self.refresh();
                cx.after(REFRESH_MS, Message::Tick);
            }
            Message::WithSelect(i) => {
                if let Some(w) = self.with.as_mut().filter(|w| i < w.choices.len()) {
                    w.selected = i;
                }
            }
            Message::WithActivate(i) => {
                if let Some(w) = self.with.as_mut().filter(|w| i < w.choices.len()) {
                    w.selected = i;
                    self.open_chosen(cx);
                }
            }
            Message::WithOpen => self.open_chosen(cx),
            Message::WithCancel => {
                if let Some(w) = self.with.take() {
                    cx.close_window(w.window);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
