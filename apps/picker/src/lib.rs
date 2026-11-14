// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The file picker's window (docs/design/0025-picker-clipboard-permissions.md, section 1), as a
//! sandstone app tested on the host.
//!
//! **Choosing is granting.** An app asks the picker (through `pbr.picker`) for "a file to
//! open" or "where to save"; the user chooses in this window, which the picker draws over the
//! app's own as a *system* window (one only system apps may make, with the system's frame, so
//! that no app can draw a fake one); and the app is then given a capability for what was chosen
//! and nothing else. A *capability* is the right to use something, held by the kernel on a
//! program's behalf (note 0006). This file is only the choosing: `main.rs` opens what was
//! chosen, has the sandbox manager record the grant, and answers the app.
//!
//! The window browses a [`Folders`] tree: on PrettyBigRock, the picker's own file tree (the
//! session's whole tree: the picker is trusted, so that it can show every folder); in the
//! tests, `pbr_browse::MemTree`. *Generics* (`Picker<F: Folders>`) let the same code use
//! either: `F` is any type with the `Folders` trait's methods.
//!
//! What the window has: the places on the left (Home, Documents, Pictures, Music, Videos,
//! Downloads, and for an administrator the volumes); the folder's contents as a list (name,
//! size, modified), filtered by the chosen filter ("All files" is always offered); a path bar
//! and Up; New folder; for saving, a name; Open or Save, and Cancel (Escape). Saving over a
//! file asks first.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use pbr_browse::filter::with_all;
use pbr_browse::kind::kind_of;
use pbr_browse::places::{place_of, places};
use pbr_browse::{
    BrowseError, Filter, Folders, Item, Place, PlaceKind, Sort, SortBy, format, path,
};
use pbr_sandstone::{
    Align, App, Button, Context, Dialog, Element, Identify, Image, Label, Length, List, ListColumn,
    ListRow, Panel, Row, Settings, Theme, ToggleButton,
};

/// Where the picker starts (`pbr.picker`'s `Place`): `Last` is where this app's last pick was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// Where this app's last pick was (or Documents, the first time).
    Last,
    /// One of the user's places.
    Place(PlaceKind),
}

/// What the app asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// An existing file, or a folder, to read (and change, with `write`).
    Open {
        /// A folder, not a file.
        folder: bool,
        /// The app may change what is chosen.
        write: bool,
    },
    /// Where to save, and a name.
    Save {
        /// The name offered first.
        name: String,
    },
}

/// One request, as the picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The asking app's title ("Notepad"), from the sandbox manager: the app can't choose it.
    pub app: String,
    /// The app's own words for what it wants, after "Notepad wants to": "open a picture".
    /// Empty for the usual words.
    pub what: String,
    /// What it asked for.
    pub ask: Ask,
    /// Where to start.
    pub start: Start,
    /// The app's filters ("Pictures", "bmp png"); "All files" is added.
    pub filters: Vec<Filter>,
    /// Where this app's last pick was, if the picker remembers one.
    pub last: Option<String>,
    /// The user's time zone, in minutes from UTC, for the Modified column.
    pub offset_minutes: i32,
}

impl Request {
    /// The window's title: who is asking, and for what ("Notepad wants to open a file"). It
    /// is in the system's frame, which an app can't draw, so the user can believe it.
    pub fn title(&self) -> String {
        let what = match (&self.ask, self.what.trim()) {
            (_, w) if !w.is_empty() => w.to_string(),
            (Ask::Open { folder: true, .. }, _) => "open a folder".to_string(),
            (Ask::Open { .. }, _) => "open a file".to_string(),
            (Ask::Save { .. }, _) => "save a file".to_string(),
        };
        let mut title = format!("{} wants to {what}", self.app);
        // The display server keeps at most 64 bytes of a title: cut at a character's edge.
        while title.len() > 64 {
            title.pop();
        }
        title
    }

    /// The line above the path bar when the app may change what is opened. (Saving says it
    /// already.)
    pub fn warning(&self) -> Option<String> {
        let what = match self.ask {
            Ask::Open { write: false, .. } | Ask::Save { .. } => return None,
            Ask::Open { folder: true, .. } => "the folder you choose and everything in it",
            Ask::Open { .. } => "the file you choose",
        };
        Some(format!("{} will be able to change {what}.", self.app))
    }

    /// The rights the app is to be given (note 0025, section 1): read, and write if it asked
    /// to change what is chosen, or to save.
    pub fn write(&self) -> bool {
        match self.ask {
            Ask::Open { write, .. } => write,
            Ask::Save { .. } => true,
        }
    }
}

/// What the user chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Its path in the session's tree, which is also where it will be in the app's.
    pub path: String,
    /// It is a folder.
    pub folder: bool,
    /// A name to save under that isn't there yet: the file is to be made.
    pub new: bool,
}

/// How the window ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user chose.
    Chosen(Choice),
    /// The user cancelled (Cancel, Escape, or closing the window).
    Cancelled,
}

/// What the window's widgets send.
#[derive(Clone, Debug)]
pub enum Message {
    /// One of the places on the left.
    Place(usize),
    /// One of the path bar's parts.
    Crumb(usize),
    /// Up to the folder above.
    Up,
    /// A row of the list picked.
    Select(usize),
    /// A row of the list opened (double click, Enter).
    Activate(usize),
    /// A click on a heading.
    Sort(usize),
    /// One of the filters.
    Filter(usize),
    /// The name field's new text.
    Name(String),
    /// New folder: asks for a name.
    NewFolder,
    /// The name the New folder dialog gave (or `None`, cancelled).
    NewFolderNamed(Option<String>),
    /// Open or Save.
    Accept,
    /// Saving over a file: the user's answer to "Replace?".
    Replace(bool),
    /// Cancel, Escape, or closing the window.
    Cancel,
}

/// The window's size: big enough for the places, a list, and the buttons at 1x on a 1024 x 768
/// screen with room around it.
pub fn settings(over: (u32, u32)) -> Settings {
    Settings {
        size: (600, 400),
        resizable: true,
        min_size: (440, 300),
        over: Some(over),
        system: true,
    }
}

/// The picker's window, browsing `F`.
pub struct Picker<F: Folders> {
    tree: F,
    request: Request,
    places: Vec<Place>,
    filters: Vec<Filter>,
    /// The folder shown.
    folder: String,
    /// What is in it that the filter lets through, in the order shown.
    items: Vec<Item>,
    selected: Option<usize>,
    sort: Sort,
    filter: usize,
    /// The name to save under.
    name: String,
    /// What went wrong last, shown under the list until the next change.
    problem: Option<String>,
    /// How it ended, once it has.
    outcome: Option<Outcome>,
    /// The new file a Replace question is about.
    replacing: Option<String>,
}

impl<F: Folders> Picker<F> {
    /// A picker for `request`, browsing `tree`.
    pub fn new(mut tree: F, request: Request) -> Picker<F> {
        // An administrator's tree has every volume under `/Volumes` (note 0017, section 3); a
        // standard user's has none, so is shown none.
        let volumes: Vec<String> = match tree.list("/Volumes") {
            Ok(mut v) => {
                pbr_browse::item::sort(&mut v, Sort::default());
                v.into_iter().filter(|i| i.folder).map(|i| i.name).collect()
            }
            Err(_) => Vec::new(),
        };
        let names: Vec<&str> = volumes.iter().map(String::as_str).collect();
        let admin = !names.is_empty();
        let filters = with_all(&request.filters);
        let name = match &request.ask {
            Ask::Save { name } if path::check_name(name).is_ok() => name.clone(),
            _ => String::new(),
        };
        let mut p = Picker {
            tree,
            places: places(admin, &names),
            filters,
            folder: String::from("/Home"),
            items: Vec::new(),
            selected: None,
            sort: Sort::default(),
            filter: 0,
            name,
            problem: None,
            outcome: None,
            replacing: None,
            request,
        };
        let start = p.start_folder();
        p.go(&start);
        p
    }

    /// Where to start: the place asked for, or this app's last folder; a place this user's
    /// home doesn't have falls back to Home.
    fn start_folder(&mut self) -> String {
        let wanted = match self.request.start {
            Start::Last => match &self.request.last {
                Some(last) => last.clone(),
                None => String::from("/Home/Documents"),
            },
            Start::Place(kind) => String::from(kind.path().unwrap_or("/Home")),
        };
        match self.tree.stat(&wanted) {
            Ok(item) if item.folder => wanted,
            _ => String::from("/Home"),
        }
    }

    /// How it ended, once it has: what `main.rs` acts on after the window closes.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// The tree it browses: lent to the window while it is open, and taken back afterwards.
    pub fn tree_mut(&mut self) -> &mut F {
        &mut self.tree
    }

    /// The request it answers.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The folder shown.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// The names shown, in order.
    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|i| i.name.as_str()).collect()
    }

    /// The places offered, by title.
    pub fn place_titles(&self) -> Vec<&str> {
        self.places.iter().map(|p| p.title.as_str()).collect()
    }

    /// What went wrong last, if anything.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Shows the folder at `path` (or says why it can't).
    fn go(&mut self, to: &str) {
        match self.tree.list(to) {
            Ok(_) => {
                self.folder = String::from(to);
                self.selected = None;
                self.problem = None;
                self.refresh();
            }
            Err(e) => self.problem = Some(describe(&e, to)),
        }
    }

    /// Lists the folder shown again: hidden files and those the filter leaves out aren't shown.
    fn refresh(&mut self) {
        let chosen = self.selected.and_then(|i| self.items.get(i)).map(|i| i.name.clone());
        let filter = self.filters.get(self.filter).cloned().unwrap_or_else(Filter::all);
        let only_folders = matches!(self.request.ask, Ask::Open { folder: true, .. });
        let mut items = match self.tree.list(&self.folder) {
            Ok(items) => items,
            Err(e) => {
                self.problem = Some(describe(&e, &self.folder));
                Vec::new()
            }
        };
        items.retain(|i| !i.hidden && filter.matches(i) && (i.folder || !only_folders));
        pbr_browse::item::sort(&mut items, self.sort);
        self.selected = chosen.and_then(|n| items.iter().position(|i| i.name == n));
        self.items = items;
    }

    /// The name to save under, with the filter's extension added if it has none ("letter"
    /// saved as a text document is "letter.txt").
    fn save_name(&self) -> String {
        let name = self.name.trim().to_string();
        match self.filters.get(self.filter).and_then(Filter::first_extension) {
            Some(ext) if !name.is_empty() && path::extension(&name).is_none() => {
                format!("{name}.{ext}")
            }
            _ => name,
        }
    }

    /// Ends with `outcome`; the window closes.
    fn finish(&mut self, outcome: Outcome, cx: &mut Context<'_, Message>) {
        self.outcome = Some(outcome);
        cx.quit();
    }

    /// Chooses `path` (a file or folder that is there).
    fn choose(&mut self, path: String, folder: bool, cx: &mut Context<'_, Message>) {
        self.finish(Outcome::Chosen(Choice { path, folder, new: false }), cx);
    }

    /// Open or Save, or Enter.
    fn accept(&mut self, cx: &mut Context<'_, Message>) {
        let picked = self.selected.and_then(|i| self.items.get(i)).cloned();
        match self.request.ask.clone() {
            Ask::Open { folder: true, .. } => {
                // The folder selected, or the one shown if none is.
                let path = match picked {
                    Some(item) => path::join(&self.folder, &item.name),
                    None => self.folder.clone(),
                };
                if path == "/" || path == "/Volumes" {
                    self.problem = Some(String::from("Choose a folder inside it."));
                    return;
                }
                self.choose(path, true, cx);
            }
            Ask::Open { folder: false, .. } => match picked {
                Some(item) if item.folder => self.go(&path::join(&self.folder, &item.name)),
                Some(item) => self.choose(path::join(&self.folder, &item.name), false, cx),
                None => self.problem = Some(String::from("Choose a file to open.")),
            },
            Ask::Save { .. } => self.save(cx),
        }
    }

    /// Save: a folder of that name is opened; a file of that name is replaced if the user says
    /// so; a new name is chosen at once.
    fn save(&mut self, cx: &mut Context<'_, Message>) {
        let name = self.save_name();
        if let Err(e) = path::check_name(&name) {
            self.problem = Some(match name.is_empty() {
                true => String::from("Type a name to save as."),
                false => describe(&e, &name),
            });
            return;
        }
        let at = path::join(&self.folder, &name);
        match self.tree.stat(&at) {
            Ok(item) if item.folder => self.go(&at),
            Ok(_) => {
                cx.show(
                    Dialog::question("Replace", &format!("Replace \"{name}\"?"))
                        .heading("It is there already")
                        .cancel_button("Cancel", Message::Replace(false))
                        .default_button("Replace", Message::Replace(true)),
                );
                self.replacing = Some(at);
            }
            Err(BrowseError::NotFound) => {
                self.finish(Outcome::Chosen(Choice { path: at, folder: false, new: true }), cx)
            }
            Err(e) => self.problem = Some(describe(&e, &at)),
        }
    }

    // ---- The parts of the view. ----

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
            .width(Length::Fixed(132))
            .height(Length::Fill)
            .id("places")
    }

    /// The folder's contents.
    fn contents(&self) -> Element<'_, Message> {
        let offset = self.request.offset_minutes;
        let rows = self
            .items
            .iter()
            .map(|i| {
                let size = if i.folder { String::new() } else { format::size(i.size) };
                ListRow::new()
                    .icon_maybe(image(kind_of(&i.name, i.folder).icon()))
                    .cell(i.name.as_str())
                    .cell(size)
                    .cell(format::date(i.modified, offset))
            })
            .collect();
        let column = match self.sort.by {
            SortBy::Name | SortBy::Kind => 0,
            SortBy::Size => 1,
            SortBy::Modified => 2,
        };
        List::new(
            vec![
                ListColumn::new("Name", Length::Fill),
                ListColumn::new("Size", Length::Fixed(72)),
                ListColumn::new("Modified", Length::Fixed(124)),
            ],
            rows,
        )
        .selected(self.selected)
        .sorted(column, self.sort.ascending)
        .on_select(Message::Select)
        .on_activate(Message::Activate)
        .on_sort(Message::Sort)
        .empty("Nothing here")
        .width(Length::Fill)
        .height(Length::Fill)
        .id("files")
    }

    /// Up, the path bar's parts, and New folder.
    fn path_bar(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut row = Row::new().spacing(t.spacing).align(Align::Center).width(Length::Fill);
        let up = (self.folder != "/").then_some(Message::Up);
        row = row.push(Button::new("Up").on_press_maybe(up).id("up"));
        for (i, (name, _)) in path::crumbs(&self.folder).iter().enumerate() {
            row = row.push(Button::new(name).on_press(Message::Crumb(i)).id(&format!("crumb{i}")));
        }
        row.push(pbr_sandstone::Stretch::new())
            .push(Button::new("New folder").on_press(Message::NewFolder).id("newfolder"))
            .into()
    }

    /// The filters, the name (saving), and the buttons.
    fn bottom(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut row = Row::new().spacing(t.spacing).align(Align::Center).width(Length::Fill);
        if self.filters.len() > 1 {
            for (i, f) in self.filters.iter().enumerate() {
                row = row.push(
                    ToggleButton::new(i == self.filter)
                        .label(&f.title)
                        .on_toggle(move |_| Message::Filter(i))
                        .id(&format!("filter{i}")),
                );
            }
        }
        if let Ask::Save { .. } = self.request.ask {
            row = row.push(Label::new("Name:")).push(
                pbr_sandstone::TextField::new(&self.name)
                    .on_input(Message::Name)
                    .width(Length::Fill)
                    .id("name"),
            );
        } else {
            row = row.push(pbr_sandstone::Stretch::new());
        }
        let accept = match self.request.ask {
            Ask::Save { .. } => "Save",
            Ask::Open { .. } => "Open",
        };
        row.push(Button::new("Cancel").on_press(Message::Cancel).cancel().id("cancel"))
            .push(Button::new(accept).on_press(Message::Accept).default().id("accept"))
            .into()
    }
}

/// A 16 by 16 icon as sandstone's [`Image`].
fn image(pixels: &'static [u32; 256]) -> Option<Image<'static>> {
    Image::new(pixels, 16, 16)
}

/// What went wrong, in words.
fn describe(e: &BrowseError, what: &str) -> String {
    match e {
        BrowseError::NotFound => format!("\"{}\" isn't there.", path::name(what)),
        BrowseError::NotAFolder => format!("\"{}\" isn't a folder.", path::name(what)),
        BrowseError::AlreadyExists => format!("\"{}\" is there already.", path::name(what)),
        BrowseError::NoRight => String::from("Nothing can be changed here."),
        BrowseError::BadName => {
            String::from("A name can't be empty, contain / or begin or end with a space.")
        }
        BrowseError::Other(text) => text.clone(),
    }
}

impl<F: Folders> App for Picker<F> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut column = pbr_sandstone::Column::new()
            .padding(t.padding)
            .spacing(t.spacing)
            .width(Length::Fill)
            .height(Length::Fill);
        if let Some(w) = self.request.warning() {
            column = column.push(Element::from(Label::new(&w).colour(t.strong)).with_id("warning"));
        }
        column = column.push(self.path_bar()).push(
            Row::new()
                .spacing(t.spacing)
                .width(Length::Fill)
                .height(Length::Fill)
                .push(self.places_list())
                .push(self.contents()),
        );
        if let Some(p) = &self.problem {
            column = column.push(Element::from(Label::new(p)).with_id("problem"));
        }
        Panel::new(column.push(self.bottom())).width(Length::Fill).height(Length::Fill).into()
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        // Saving starts in the name, so the user can type it at once; opening, in the list.
        match self.request.ask {
            Ask::Save { .. } => cx.focus("name"),
            Ask::Open { .. } => cx.focus("files"),
        }
    }

    fn close_requested(&self, _window: pbr_sandstone::WindowId) -> Option<Message> {
        // Closing the window is Cancel: the app must still be answered.
        Some(Message::Cancel)
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
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
            Message::Select(i) => {
                self.selected = (i < self.items.len()).then_some(i);
                // Saving: picking a file offers its name.
                if let (Ask::Save { .. }, Some(item)) =
                    (&self.request.ask, self.selected.and_then(|i| self.items.get(i)))
                    && !item.folder
                {
                    self.name = item.name.clone();
                }
            }
            Message::Activate(i) => {
                let Some(item) = self.items.get(i).cloned() else { return };
                self.selected = Some(i);
                let at = path::join(&self.folder, &item.name);
                match (&self.request.ask, item.folder) {
                    (_, true) => self.go(&at),
                    (Ask::Open { .. }, false) => self.choose(at, false, cx),
                    (Ask::Save { .. }, false) => {
                        self.name = item.name;
                        self.save(cx);
                    }
                }
            }
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
            Message::Filter(i) => {
                if i < self.filters.len() {
                    self.filter = i;
                    self.refresh();
                }
            }
            Message::Name(text) => {
                self.name = text;
                self.problem = None;
            }
            Message::NewFolder => {
                cx.show(Dialog::input("New folder", "The new folder's name:", "New folder", |n| {
                    Message::NewFolderNamed(Some(n))
                }));
            }
            Message::NewFolderNamed(None) => {}
            Message::NewFolderNamed(Some(name)) => {
                let name = name.trim().to_string();
                let at = path::join(&self.folder, &name);
                match path::check_name(&name).and_then(|()| self.tree.make_folder(&at)) {
                    Ok(()) => {
                        self.problem = None;
                        self.refresh();
                        self.selected = self.items.iter().position(|i| i.name == name);
                    }
                    Err(e) => self.problem = Some(describe(&e, &at)),
                }
            }
            Message::Accept => self.accept(cx),
            Message::Replace(yes) => {
                if let Some(at) = self.replacing.take()
                    && yes
                {
                    self.finish(
                        Outcome::Chosen(Choice { path: at, folder: false, new: false }),
                        cx,
                    );
                }
            }
            Message::Cancel => self.finish(Outcome::Cancelled, cx),
        }
    }
}

#[cfg(test)]
mod tests;
