// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The widget gallery: every sandstone widget in one window (note 0023, section 6).
//!
//! It is a test program. Launched as an app in the desktop boot (note 0026, section 6), it is
//! driven from outside the machine by the runner in `xtask/src/desktop.rs` (`fn gallery`), which
//! compares screenshots of it with the reference pictures this crate's own host tests draw
//! (`tests/pictures/sandstone/gallery-*.png`), so **the window as it opens is what those
//! pictures show**: nothing in it may depend on the time or on chance.
//!
//! The program itself is `main.rs`: it runs [`Gallery`] with sandstone's `Runtime` and writes
//! what the widgets are, and what the app was sent, to the log for the runner to read. Here is
//! the app: its state, its `view` (the window, built afresh after every change) and its
//! `update` (what each message does). Tests are at the end, run on the host with sandstone's
//! harness.
//!
//! A note on Rust for the newcomer. `#![no_std]` means this crate uses only Rust's `core` and
//! `alloc` libraries, not the operating system's `std`, so that it can run as a PrettyBigRock
//! program; `alloc` holds `String`, `Vec` and `format!`, which need a heap. (The tests, which
//! run on the host, may use `std`: that is the `cfg_attr` below.)

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::mem;
use pbr_draw::{Canvas as DrawCanvas, Colour, Rect};
use pbr_sandstone::event::Button as MouseButton;
use pbr_sandstone::{
    Action, Align, App, Button, Canvas, Cell, Cells, CheckBox, Column, Content, Context,
    ContextMenu, Dialog, Element, Grid, Icon, IconButton, Identify, Image, Key, Label, Length,
    List, ListColumn, ListRow, Menu, MenuBar, MenuItem, Padding, Panel, Row, Scroll, Settings,
    Shortcut, StatusBar, TerminalView, TextEditor, TextField, Theme, ToggleButton, Toolbar,
};

// ---------------------------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------------------------

/// What one character of an icon's drawing stands for, as `0xAARRGGBB` (alpha first: 0 is clear,
/// `0xFF` solid). Every icon below is drawn with these, so that they look like a family.
///
/// `const fn` is a function the compiler can run while it builds the program, so the icons are
/// finished arrays of numbers in the program itself and cost nothing to make. (`match` picks the
/// first arm that fits, like a `switch`; the compiler insists that every case is covered.)
const fn colour_of(c: u8) -> u32 {
    match c {
        b'k' => 0xFF5A_5F6A, // the outline of a page
        b'w' => 0xFFFF_FFFF, // paper
        b'g' => 0xFFC9_CCD3, // a line of text on paper
        b'y' => 0xFFF2_C230, // the folder's yellow, and the star's
        b'Y' => 0xFFB8_862B, // its edge
        b'h' => 0xFFFB_E38A, // its light top
        b'p' => 0xFFD2_577A, // the accent's pink
        b'P' => 0xFF9E_2F52, // its edge
        b'l' => 0xFFF6_B8CB, // its highlight
        b'b' => 0xFF9C_C4E8, // sky
        b'n' => 0xFF2F_6B55, // the hill (1.3's dark green)
        _ => 0,              // '.' is clear
    }
}

/// Turns a drawing, sixteen rows of sixteen characters, into sixteen by sixteen pixels. A row
/// of any other length stops the build with an error, since indexing past the end of a row is
/// caught by the compiler when it runs this function (and the tests check the other way).
const fn icon(art: [&str; 16]) -> [u32; 256] {
    let mut pixels = [0u32; 256];
    let mut y = 0;
    while y < 16 {
        let row = art[y].as_bytes();
        let mut x = 0;
        while x < 16 {
            pixels[y * 16 + x] = colour_of(row[x]);
            x += 1;
        }
        y += 1;
    }
    pixels
}

/// A folder, for the list's rows.
const FOLDER: [u32; 256] = icon(FOLDER_ART);
const FOLDER_ART: [&str; 16] = [
    "................",
    "................",
    ".YYYYY..........",
    ".YhhhhYYYYYYYY..",
    ".YhhhhhhhhhhhhY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YyyyyyyyyyyyyY.",
    ".YYYYYYYYYYYYYY.",
    "................",
    "................",
];

/// A page of text.
const FILE: [u32; 256] = icon(FILE_ART);
const FILE_ART: [&str; 16] = [
    "................",
    "...kkkkkkkk.....",
    "...kwwwwwwkk....",
    "...kwwwwwwkgk...",
    "...kwwwwwwkkk...",
    "...kwwwwwwwwk...",
    "...kwggggggwk...",
    "...kwwwwwwwwk...",
    "...kwggggggwk...",
    "...kwwwwwwwwk...",
    "...kwggggggwk...",
    "...kwwwwwwwwk...",
    "...kwggggwwwk...",
    "...kwwwwwwwwk...",
    "...kkkkkkkkkk...",
    "................",
];

/// A picture: a hill and a sun in a frame.
const PICTURE: [u32; 256] = icon(PICTURE_ART);
const PICTURE_ART: [&str; 16] = [
    "................",
    "..kkkkkkkkkkkk..",
    "..kbbbbbbbbbbk..",
    "..kbbbbbyybbbk..",
    "..kbbbbbyybbbk..",
    "..kbbbnbbbbbbk..",
    "..kbbnnnbbbnbk..",
    "..kbnnnnnbnnnk..",
    "..knnnnnnnnnnk..",
    "..knnnnnnnnnnk..",
    "..knnnnnnnnnnk..",
    "..knnnnnnnnnnk..",
    "..knnnnnnnnnnk..",
    "..kkkkkkkkkkkk..",
    "................",
    "................",
];

/// A heart: the toolbar's button.
const HEART: [u32; 256] = icon(HEART_ART);
const HEART_ART: [&str; 16] = [
    "................",
    "................",
    "..PPPP....PPPP..",
    ".PppppP..PppppP.",
    "PppllppPPppppppP",
    "PpllpppppppppppP",
    "PppppppppppppppP",
    ".PppppppppppppP.",
    "..PppppppppppP..",
    "...PppppppppP...",
    "....PppppppP....",
    ".....PppppP.....",
    "......PppP......",
    ".......PP.......",
    "................",
    "................",
];

/// A star: the heading's icon.
const STAR: [u32; 256] = icon(STAR_ART);
const STAR_ART: [&str; 16] = [
    "................",
    ".......YY.......",
    "......YyyY......",
    "......YyyY......",
    ".YYYYYYyyYYYYYY.",
    ".YyyyyyyyyyyyyY.",
    "..YyyyyyyyyyyY..",
    "...YyyyyyyyyY...",
    "....YyyyyyyY....",
    "....YyyyyyyY....",
    "...YyyyYYyyyY...",
    "...YyyY..YyyY...",
    "..YyyY....YyyY..",
    "..YYY......YYY..",
    "................",
    "................",
];

/// An icon's pixels as the [`Image`] widgets take.
///
/// `&'static [u32; 256]` is a reference that lives as long as the program (the arrays above are
/// part of it); a reference to a whole array turns into a slice (`&[u32]`) where one is wanted.
/// A 16 by 16 picture needs 256 numbers, which every array above has by its type, so `new`
/// never says no.
fn image(pixels: &'static [u32; 256]) -> Image<'static> {
    Image::new(pixels, 16, 16).expect("a 16 by 16 icon has 256 pixels")
}

// ---------------------------------------------------------------------------------------------
// The files in the list
// ---------------------------------------------------------------------------------------------

/// What a row of the list is, which decides its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Folder,
    Text,
    Picture,
}

/// One row of the list. The app keeps them (sorted as the user chose), and the list only shows
/// what it is given.
#[derive(Clone, Debug)]
struct Entry {
    name: &'static str,
    kind: Kind,
    /// Zero for a folder.
    bytes: u32,
}

/// A dozen rows, in the order of the Name column, ascending.
fn entries() -> Vec<Entry> {
    let e = |name, kind, kb: u32| Entry { name, kind, bytes: kb * 1024 };
    vec![
        e("budget.txt", Kind::Text, 12),
        e("dusk.png", Kind::Picture, 148),
        e("Documents", Kind::Folder, 0),
        e("fynbos.png", Kind::Picture, 96),
        e("Music", Kind::Folder, 0),
        e("notes.txt", Kind::Text, 2),
        e("Pictures", Kind::Folder, 0),
        e("Projects", Kind::Folder, 0),
        e("protea.png", Kind::Picture, 210),
        e("readme.txt", Kind::Text, 1),
        e("recipe.txt", Kind::Text, 3),
        e("todo.txt", Kind::Text, 1),
    ]
}

/// Which column the list is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortBy {
    Name,
    Size,
}

impl SortBy {
    /// The column's number.
    fn column(self) -> usize {
        match self {
            SortBy::Name => 0,
            SortBy::Size => 1,
        }
    }
}

/// "3 KB", or nothing for a folder.
fn size_text(bytes: u32) -> String {
    if bytes == 0 { String::new() } else { format!("{} KB", bytes / 1024) }
}

// ---------------------------------------------------------------------------------------------
// The terminal's text
// ---------------------------------------------------------------------------------------------

/// How many columns and rows of text the terminal keeps.
const TERM_COLUMNS: usize = 60;
const TERM_ROWS: usize = 4;
/// The prompt at the start of the last row, which typing doesn't delete.
const PROMPT: &str = "$ ";

/// What the terminal shows: a few rows of text of the app's own, the last of which is typed
/// into. `Cells` is a *trait* (a set of methods a type promises to have, like an interface in
/// Java); the terminal view asks whatever the app gives it for its cells through that.
struct Screen {
    lines: Vec<String>,
}

impl Screen {
    fn new() -> Screen {
        Screen {
            lines: vec![
                "Sandstone's terminal view".to_string(),
                "Click here and type: it goes in the last row.".to_string(),
                "/home/chris".to_string(),
                PROMPT.to_string(),
            ],
        }
    }

    /// The last row: where the cursor is.
    fn typing(&mut self) -> &mut String {
        let last = self.lines.len() - 1;
        &mut self.lines[last]
    }

    /// A character typed: it joins the last row, if there is room.
    fn type_char(&mut self, c: char) {
        let line = self.typing();
        if line.chars().count() < TERM_COLUMNS - 1 {
            line.push(c);
        }
    }

    /// Backspace: the last character typed goes, but never the prompt.
    fn backspace(&mut self) {
        let line = self.typing();
        if line.chars().count() > PROMPT.len() {
            line.pop();
        }
    }
}

impl Cells for Screen {
    fn size(&self) -> (usize, usize) {
        (TERM_COLUMNS, TERM_ROWS)
    }

    fn cell(&self, column: usize, row: usize) -> Cell {
        let ch = self.lines.get(row).and_then(|l| l.chars().nth(column)).unwrap_or(' ');
        // As 1.3's Terminal: a row of each colour, the prompt in green and what is typed white.
        let colour = match (row, column) {
            (0, _) => 0xd8dce6,
            (1, _) => 0x8fd18a,
            (2, _) => 0x6fa8e8,
            (_, c) if c < PROMPT.len() => 0x8fd18a,
            _ => 0xffffff,
        };
        Cell::new(ch, Colour::solid(colour))
    }

    fn cursor(&self) -> Option<(usize, usize)> {
        let row = self.lines.len() - 1;
        Some((self.lines[row].chars().count(), row))
    }
}

// ---------------------------------------------------------------------------------------------
// The canvas's picture
// ---------------------------------------------------------------------------------------------

/// The canvas's size.
const CANVAS: (i32, i32) = (160, 72);
/// How many dots the canvas keeps (the oldest goes first).
const MAX_DOTS: usize = 24;

/// A dot the user put on the picture: where (from the canvas's corner), and whether the pen
/// was down, which decides its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dot {
    x: i32,
    y: i32,
    pen: bool,
}

/// A hill: a triangle, drawn a row at a time because integer arithmetic is all the drawing
/// uses (so the pictures are the same on every processor). `centre` is the peak's distance from
/// the left of `area`, `base` the row where the hill ends, `half` its half-width there.
fn hill(
    c: &mut DrawCanvas<'_>,
    area: Rect,
    centre: i32,
    base: i32,
    height: i32,
    half: i32,
    rgb: u32,
) {
    for dy in 0..height {
        let reach = half * dy / height;
        let row = Rect::new(area.x + centre - reach, area.y + base - height + dy, 2 * reach + 1, 1);
        c.fill_rect(row, Colour::solid(rgb));
    }
}

/// Draws the picture in `area` (the canvas's place in the window): a little mountain at dusk,
/// then the dots. Everything is worked out from `area`'s size, so any size gives a picture.
fn draw_scene(c: &mut DrawCanvas<'_>, area: Rect, dots: &[Dot], accent: Colour) {
    let sky = Rect::new(area.x, area.y, area.width, area.height * 3 / 4);
    c.gradient_down(sky, Colour::solid(0x2b3a67), Colour::solid(0xe8997f));
    c.fill_rect(
        Rect::new(area.x, sky.y + sky.height, area.width, area.height - sky.height),
        Colour::solid(0x232a47),
    );
    // A few stars, then the sun going down behind the hills.
    for (sx, sy) in [(12, 6), (40, 14), (68, 5), (96, 10), (24, 22), (84, 22)] {
        c.fill_rect(
            Rect::new(area.x + sx * area.width / 160, area.y + sy * area.height / 72, 1, 1),
            Colour::solid(0xf5f2ec),
        );
    }
    let sun = area.width / 8;
    c.fill_ellipse(
        Rect::new(
            area.x + area.width * 3 / 4 - sun / 2,
            sky.y + sky.height - sun * 3 / 4,
            sun,
            sun,
        ),
        Colour::solid(0xf6d07a),
    );
    let base = sky.height + area.height / 10;
    hill(c, area, area.width * 2 / 5, base, area.height * 3 / 5, area.width / 3, 0x6b5b86);
    hill(c, area, area.width * 3 / 4, base, area.height * 2 / 5, area.width / 4, 0x4a4468);
    hill(c, area, area.width / 4, base, area.height * 2 / 5, area.width / 4, 0x2f3552);
    c.fill_rect(
        Rect::new(area.x, area.y + base, area.width, (area.height - base).max(0)),
        Colour::solid(0x232a47),
    );
    // The dots: white rings, filled with the accent (pen down) or the sky's cream.
    for d in dots {
        let fill = if d.pen { accent } else { Colour::solid(0xf5f2ec) };
        c.fill_ellipse(
            Rect::new(area.x + d.x - 4, area.y + d.y - 4, 9, 9),
            Colour::solid(0x22252c),
        );
        c.fill_ellipse(Rect::new(area.x + d.x - 3, area.y + d.y - 3, 7, 7), fill);
    }
}

// ---------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------

/// What the gallery's widgets send. The names `Clicked`, `Wrap`, `Copy` and `Answer` are what
/// the desktop boot's runner waits for in the log, so they stay as they are.
///
/// `#[derive(Clone, Debug)]` makes the compiler write two methods for the type: `clone` (sandstone
/// keeps a message in each widget and sends a copy every time it is used) and the `{:?}` text
/// of a message, which is what [`Gallery::take_log`] hands to the program's log.
#[derive(Clone, Debug)]
pub enum Message {
    // The menus.
    /// File, New: a new, empty text in the editor.
    New,
    /// File, Open…: the file picker, to read a file into the field (note 0025).
    OpenFile,
    /// File, Save as…: the file picker, to save the field's text.
    SaveFile,
    /// File, Close: ends the app.
    Close,
    /// Edit, Copy: the field's text goes to the clipboard.
    Copy,
    /// Edit, Paste: the clipboard's text replaces the field's.
    Paste,
    /// Edit, Clear: the field is emptied.
    Clear,
    /// Help, About: a message dialog.
    About,
    // The toolbar.
    /// The icon button.
    Tool,
    /// The Bold toggle, with its new value.
    Bold(bool),
    /// "Ask...": opens the question dialog.
    Ask,
    /// The question's answer: Yes is `true`, No is `false`.
    Answer(bool),
    /// "Name...": opens the input dialog.
    Name,
    /// The text the input dialog's OK button gave.
    Named(String),
    // The left column.
    /// The field's new text, after every change.
    Field(String),
    /// The Wrap check box, with its new value.
    Wrap(bool),
    /// The Pen toggle, with its new value.
    Pen(bool),
    /// The "Click me" button.
    Clicked,
    /// One of the grid's keys, 1 to 6.
    Digit(u8),
    /// A click on the canvas, from its corner.
    Dot(i32, i32),
    /// The context menu's Reset: everything as it opened.
    Reset,
    // The right column.
    /// A row of the list picked.
    Select(usize),
    /// A row of the list opened (double click, or Enter).
    Open(usize),
    /// A click on a heading of the list, with the column's number.
    Sort(usize),
    /// Something the user did in the text editor, which the app applies.
    Edit(Action),
    /// A character typed in the terminal.
    Type(char),
    /// Backspace typed in the terminal.
    Backspace,
}

/// The gallery's state: what the widgets show now. Sandstone keeps the widgets' own small
/// state (the cursor in the field, how far the list is scrolled), so this is only what the app
/// itself must know.
pub struct Gallery {
    /// The text field's text.
    text: String,
    /// The Wrap check box, the Bold toggle and the Pen toggle.
    wrap: bool,
    bold: bool,
    pen: bool,
    /// What the name dialog was last given.
    name: String,
    /// The list's rows, in the order shown, and which is selected.
    files: Vec<Entry>,
    selected: Option<usize>,
    sort: (SortBy, bool),
    /// The text editor's text.
    editor: Content,
    screen: Screen,
    dots: Vec<Dot>,
    /// Changes whenever the dots do, so that the canvas is drawn again (sandstone can't see
    /// what its drawing function would draw).
    version: u64,
    /// The last message, for the status bar.
    last: String,
    /// Every message since [`Gallery::take_log`] was last called.
    log: Vec<String>,
    /// A file picker asked for and not yet handed to the program ([`Gallery::take_pick`]).
    pick: Option<Pick>,
}

/// A file picker the app wants: the program asks the picker (only it holds the picker's
/// capability), and the window and the press that asked, which the picker checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    /// Save as, rather than Open.
    pub save: bool,
    /// The display server's number for the window it was asked from.
    pub window: Option<pbr_window::Win>,
    /// The key or button press that asked.
    pub serial: u32,
}

/// The window as the app wants it: 560 by 380 logical pixels, not resizable.
pub fn settings() -> Settings {
    Settings { size: (560, 380), resizable: false, min_size: (0, 0), over: None, system: false }
}

impl Default for Gallery {
    fn default() -> Gallery {
        Gallery::new()
    }
}

impl Gallery {
    /// The state the window opens with.
    pub fn new() -> Gallery {
        Gallery {
            text: String::new(),
            wrap: false,
            bold: false,
            pen: false,
            name: "Chris".to_string(),
            files: entries(),
            selected: None,
            sort: (SortBy::Name, true),
            editor: Content::from_text(
                "A text editor:\nline numbers, a cursor,\nselection and undo.\nTry typing here.",
            ),
            screen: Screen::new(),
            dots: Vec::new(),
            version: 0,
            last: String::new(),
            log: Vec::new(),
            pick: None,
        }
    }

    /// The file picker asked for since the last call, if one was.
    pub fn take_pick(&mut self) -> Option<Pick> {
        self.pick.take()
    }

    /// The field's text, which Save as saves.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Puts text read from a file in the field (its first line, as a field has one).
    pub fn set_text(&mut self, text: &str) {
        self.text = text.lines().next().unwrap_or("").chars().take(200).collect();
    }

    /// Each message given to `update` since the last call, as the `{:?}` text of the message
    /// (`Clicked`, `Wrap(true)`): what the program writes to the log, and the runner reads.
    pub fn take_log(&mut self) -> Vec<String> {
        mem::take(&mut self.log)
    }

    /// Sorts the list by `by`, keeping the same row selected (by name: its place changes).
    fn sort_files(&mut self, by: SortBy, ascending: bool) {
        let chosen = self.selected.and_then(|i| self.files.get(i)).map(|f| f.name);
        self.files.sort_by(|a, b| {
            let order = match by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Size => a.bytes.cmp(&b.bytes).then(a.name.cmp(b.name)),
            };
            if ascending { order } else { order.reverse() }
        });
        self.sort = (by, ascending);
        self.selected = chosen.and_then(|n| self.files.iter().position(|f| f.name == n));
    }

    // ---- The parts of the view. ----

    /// The menu bar: File, Edit and Help.
    fn menu_bar(&self) -> Element<'_, Message> {
        let file = Menu::new(
            "File",
            vec![
                MenuItem::new("New", Message::New).shortcut(Shortcut::ctrl('n')),
                MenuItem::new("Open…", Message::OpenFile).shortcut(Shortcut::ctrl('o')),
                MenuItem::new("Save as…", Message::SaveFile).shortcut(Shortcut::ctrl('s')),
                MenuItem::separator(),
                MenuItem::new("Close", Message::Close).shortcut(Shortcut::ctrl('w')),
            ],
        );
        let edit = Menu::new(
            "Edit",
            vec![
                MenuItem::new("Copy", Message::Copy).shortcut(Shortcut::ctrl('c')),
                MenuItem::new("Paste", Message::Paste).shortcut(Shortcut::ctrl('v')),
                MenuItem::separator(),
                MenuItem::new("Clear", Message::Clear),
            ],
        );
        let help = Menu::new("Help", vec![MenuItem::new("About…", Message::About)]);
        MenuBar::new(vec![file, edit, help]).into()
    }

    /// The toolbar: an icon, a toggle, a line, two buttons that open dialogs, and a label at
    /// the right.
    fn toolbar(&self) -> Element<'_, Message> {
        Toolbar::new()
            .push(IconButton::new(image(&HEART)).on_press(Message::Tool).id("tool"))
            .push(ToggleButton::new(self.bold).label("Bold").on_toggle(Message::Bold).id("bold"))
            .separator()
            .push(Button::new("Ask…").on_press(Message::Ask).id("ask"))
            .push(Button::new("Name…").on_press(Message::Name).id("name"))
            .push_right(Label::new("Sandstone"))
            .into()
    }

    /// The left column: the small widgets, with a context menu over all of it.
    fn left(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let heading = Row::new()
            .spacing(t.spacing)
            .align(Align::Center)
            .push(Icon::new(image(&STAR)).label("Star"))
            .push(tag(Label::new("Widgets").heading().colour(t.strong), "heading"));
        let mut keys = Grid::new(3).spacing(t.spacing).width(Length::Fill);
        for n in 1..=6u8 {
            keys = keys.push(
                Button::new(&n.to_string())
                    .on_press(Message::Digit(n))
                    .width(Length::Fill)
                    .id(&format!("key{n}")),
            );
        }
        let canvas = Canvas::new(CANVAS.0, CANVAS.1, {
            let dots = &self.dots;
            move |c: &mut DrawCanvas<'_>, area: Rect| draw_scene(c, area, dots, t.accent)
        })
        .height(Length::Fill)
        .version(self.version)
        .on_pointer(|p| p.pressed(MouseButton::Left).map(|_| Message::Dot(p.at.x, p.at.y)));
        let column = Column::new()
            .width(Length::Fixed(160))
            .height(Length::Fill)
            .spacing(t.spacing)
            .push(heading)
            // The runner presses Tab from the field to the check box and then the toggle, so
            // these three stay together, in this order, with nothing focusable between.
            .push(
                TextField::new(&self.text)
                    .placeholder("Type here")
                    .on_input(Message::Field)
                    .id("field"),
            )
            .push(
                Row::new()
                    .spacing(t.spacing)
                    .align(Align::Center)
                    .push(CheckBox::new("Wrap", self.wrap).on_toggle(Message::Wrap).id("wrap"))
                    .push(
                        ToggleButton::new(self.pen).label("Pen").on_toggle(Message::Pen).id("pen"),
                    ),
            )
            .push(
                Row::new()
                    .spacing(t.spacing)
                    .width(Length::Fill)
                    .push(
                        Button::new("Click me")
                            .on_press(Message::Clicked)
                            .width(Length::Fill)
                            .id("click"),
                    )
                    // No message: a button with nothing to send is greyed out.
                    .push(Button::<Message>::new("Disabled").width(Length::Fill)),
            )
            .push(keys)
            .push(canvas.id("canvas"));
        ContextMenu::new(column, vec![MenuItem::new("Reset", Message::Reset)]).into()
    }

    /// The right column: a list, a scroll area beside a text editor, and a terminal.
    fn right(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let icons = [(Kind::Folder, &FOLDER), (Kind::Text, &FILE), (Kind::Picture, &PICTURE)];
        let rows: Vec<ListRow<'_>> = self
            .files
            .iter()
            .map(|f| {
                let pixels = icons.iter().find(|(k, _)| *k == f.kind).map(|(_, p)| *p);
                ListRow::new().icon_maybe(pixels.map(image)).cell(f.name).cell(size_text(f.bytes))
            })
            .collect();
        let list = List::new(
            vec![ListColumn::new("Name", Length::Fill), ListColumn::new("Size", Length::Fixed(80))],
            rows,
        )
        .selected(self.selected)
        .sorted(self.sort.0.column(), self.sort.1)
        .on_select(Message::Select)
        .on_activate(Message::Open)
        .on_sort(Message::Sort)
        .height(Length::Fixed(143));

        // Twenty labels in a column, which a scroll area of 70 high shows a few of.
        let mut lines = Column::new().padding_each(Padding::symmetric(4, 6));
        for n in 1..=20 {
            lines = lines.push(Label::new(&format!("Line {n}")));
        }
        let scroll = Panel::boxed(tag(Scroll::new(lines), "scroll"))
            .padding(1)
            .width(Length::Fixed(110))
            .height(Length::Fill);
        // A box round it, as round the scroll area and the list: the editor draws its paper
        // but no edge of its own.
        let editor =
            Panel::boxed(TextEditor::new(&self.editor).on_action(Message::Edit).id("editor"))
                .padding(1)
                .width(Length::Fill)
                .height(Length::Fill);
        let middle = Row::new()
            .spacing(t.spacing)
            .width(Length::Fill)
            .height(Length::Fixed(70))
            .push(scroll)
            .push(editor);

        let terminal = TerminalView::new(&self.screen)
            .on_key(|k| match k.ch {
                _ if k.key == Key::Backspace => Some(Message::Backspace),
                Some(c) if k.typed => Some(Message::Type(c)),
                _ => None,
            })
            .id("terminal");

        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(t.spacing)
            .push(tag(list, "list"))
            .push(middle)
            .push(terminal)
            .into()
    }

    /// The status bar: "Ready", and the last message once there is one.
    fn status(&self) -> Element<'_, Message> {
        let bar = StatusBar::new().push("Ready");
        if self.last.is_empty() { bar.into() } else { bar.push(&self.last).into() }
    }
}

/// `widget` as an element with the id `id` (the runner and the tests find widgets by id).
/// Written out as a function because a widget with no message of its own, such as a label,
/// leaves Rust unable to tell which app it belongs to unless the types are spelled out.
fn tag<'a>(widget: impl Into<Element<'a, Message>>, id: &str) -> Element<'a, Message> {
    widget.into().with_id(id)
}

impl App for Gallery {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let body = Row::new()
            .padding(t.padding)
            .spacing(t.padding)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.left())
            .push(self.right());
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.menu_bar())
            .push(self.toolbar())
            .push(body)
            .push(self.status())
            .into()
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        self.log.push(format!("{message:?}"));
        // Closing ends the app: its window is going, so there is nothing to show of it.
        if let Message::Close = message {
            cx.quit();
            return;
        }
        // The status bar shows the message, cut short: a long text typed in would not fit.
        self.last = format!("{message:?}").chars().take(48).collect();
        match message {
            Message::New => self.editor = Content::new(),
            Message::OpenFile | Message::SaveFile => {
                let save = matches!(message, Message::SaveFile);
                self.pick = Some(Pick { save, window: cx.display_window(), serial: cx.serial() });
            }
            Message::Close => {}
            // The serial of the click or key that made this message lets the app use the
            // clipboard (note 0022): `copy_text` and `paste_text` ask the display server.
            Message::Copy => {
                cx.copy_text(&self.text);
            }
            Message::Paste => {
                if let Some(text) = cx.paste_text() {
                    self.text = text;
                }
            }
            Message::Clear => self.text.clear(),
            Message::About => {
                cx.show(
                    Dialog::message("About the gallery", "Every sandstone widget, in one window.")
                        .heading("Sandstone"),
                );
            }
            Message::Tool | Message::Digit(_) | Message::Clicked | Message::Open(_) => {}
            Message::Bold(on) => self.bold = on,
            Message::Ask => {
                cx.show(
                    Dialog::question("A question", "Do you like the gallery?")
                        .cancel_button("No", Message::Answer(false))
                        .default_button("Yes", Message::Answer(true)),
                );
            }
            Message::Answer(_) => {}
            Message::Name => {
                // `'static` closure: a dialog outlives this call, so it may not borrow.
                cx.show(Dialog::input(
                    "Name",
                    "What should I call you?",
                    &self.name,
                    Message::Named,
                ));
            }
            Message::Named(name) => self.name = name,
            Message::Field(text) => self.text = text,
            Message::Wrap(on) => self.wrap = on,
            Message::Pen(on) => self.pen = on,
            Message::Dot(x, y) => {
                if self.dots.len() >= MAX_DOTS {
                    self.dots.remove(0);
                }
                self.dots.push(Dot { x, y, pen: self.pen });
                self.version += 1;
            }
            Message::Reset => {
                // Everything back as it opened, except what the app keeps for the outside:
                // the log, and a version that only ever goes up.
                let (log, version, last) =
                    (mem::take(&mut self.log), self.version, self.last.clone());
                *self = Gallery::new();
                self.log = log;
                self.version = version + 1;
                self.last = last;
            }
            Message::Select(row) => self.selected = Some(row),
            Message::Sort(column) => {
                let by = if column == 0 { SortBy::Name } else { SortBy::Size };
                // A click on the column the list is sorted by turns it round.
                let ascending = if self.sort.0 == by { !self.sort.1 } else { true };
                self.sort_files(by, ascending);
            }
            Message::Edit(action) => self.editor.perform(action),
            Message::Type(c) => self.screen.type_char(c),
            Message::Backspace => self.screen.backspace(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

/// The gallery on the host, through sandstone's harness: how the window opens (the pictures the
/// desktop boot compares the real screen with), the runner's own steps in the runner's order,
/// then every widget used once. Messages are checked as they are logged, by their `{:?}` text,
/// which is what the runner reads.
#[cfg(test)]
mod tests {
    use super::*;
    use pbr_sandstone::testing::Harness;
    use pbr_sandstone::{Description, Modifiers, WindowId};
    use std::path::PathBuf;

    /// Where the reference pictures are kept (`tests/pictures/sandstone`).
    fn dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pictures/sandstone")
    }

    /// The gallery, open at 1x with nothing touched.
    fn open() -> Harness<Gallery> {
        Harness::new(Gallery::new(), settings())
    }

    /// The widget with this id or label, in the newest window it is in.
    fn widget(h: &Harness<Gallery>, name: &str) -> (WindowId, Description) {
        h.find(name).unwrap_or_else(|| panic!("no widget {name}"))
    }

    /// The middle of the widget, as the runner clicks it.
    fn middle(h: &Harness<Gallery>, name: &str) -> (i32, i32) {
        let r = widget(h, name).1.rect;
        (r.x + r.width / 2, r.y + r.height / 2)
    }

    /// Clicks the middle of a widget of the main window.
    fn click(h: &mut Harness<Gallery>, name: &str) {
        let (x, y) = middle(h, name);
        h.click(x, y);
    }

    /// The messages logged since the last call.
    fn log(h: &mut Harness<Gallery>) -> Vec<String> {
        h.app_mut().take_log()
    }

    /// A widget's value, as the description says it.
    fn value(h: &Harness<Gallery>, name: &str) -> String {
        widget(h, name).1.what.value
    }

    /// Where the list's row `n` (counting from the first, unscrolled) is: a point in it, past
    /// its icon.
    fn row_at(h: &Harness<Gallery>, n: i32) -> (i32, i32) {
        let r = widget(h, "list").1.rect;
        // One pixel of edge, the heading's 21 rows, then rows of 20.
        (r.x + 40, r.y + 1 + 21 + 20 * n + 10)
    }

    /// A point in the left column with no widget on it: right of the heading. (A right click
    /// anywhere on the column opens its context menu.)
    fn blank_spot(h: &Harness<Gallery>) -> (i32, i32) {
        let field = widget(h, "field").1.rect;
        let heading = widget(h, "heading").1.rect;
        (field.x + field.width - 3, heading.y + heading.height / 2)
    }

    /// Opens a menu with F10 and moves along the bar `along` menus.
    fn open_menu(h: &mut Harness<Gallery>, along: usize) {
        h.key(Key::Function(10));
        for _ in 0..along {
            h.key(Key::Right);
        }
        assert!(h.popup().is_some(), "no menu is open");
    }

    // ---- How it opens. ----

    #[test]
    fn it_opens_as_the_reference_picture_at_1x() {
        let h = open();
        h.check_window_picture_in(WindowId::MAIN, &dir(), "gallery-1x");
    }

    #[test]
    fn it_opens_as_the_reference_picture_at_2x() {
        let h = Harness::with_scale(Gallery::new(), settings(), 2);
        h.check_window_picture_in(WindowId::MAIN, &dir(), "gallery-2x");
    }

    #[test]
    fn opening_it_twice_gives_the_same_window() {
        // The runner compares pixels: nothing may depend on the time or on chance.
        let (a, b) = (open(), open());
        assert_eq!(a.picture(WindowId::MAIN).pixels, b.picture(WindowId::MAIN).pixels);
        assert_eq!(a.describe_lines(WindowId::MAIN), b.describe_lines(WindowId::MAIN));
    }

    #[test]
    fn the_window_is_560_by_380_and_everything_fits_in_it() {
        let h = open();
        assert_eq!(settings().size, (560, 380));
        assert!(!settings().resizable);
        for d in h.describe(WindowId::MAIN) {
            // The scroll area's lines are laid out past its edge, which is the point of it.
            if d.what.label.starts_with("Line ") {
                continue;
            }
            let r = d.rect;
            assert!(
                r.x >= 0 && r.y >= 0 && r.x + r.width <= 560 && r.y + r.height <= 380,
                "{} is outside the window",
                d.line()
            );
            assert!(r.width > 0 && r.height > 0, "{} has no size", d.line());
        }
    }

    #[test]
    fn every_widget_is_there_with_the_ids_the_runner_uses() {
        let h = open();
        let lines = h.describe_lines(WindowId::MAIN);
        let roles: Vec<String> =
            h.describe(WindowId::MAIN).iter().map(|d| d.what.role.to_string()).collect();
        for role in [
            "menu bar",
            "button",
            "toggle button",
            "check box",
            "text field",
            "label",
            "icon",
            "canvas",
            "list",
            "scroll",
            "text editor",
            "terminal",
            "status bar",
        ] {
            assert!(roles.iter().any(|r| r == role), "no {role} in {lines:#?}");
        }
        for id in [
            "tool", "bold", "ask", "name", "heading", "field", "wrap", "pen", "click", "canvas",
            "list", "scroll", "editor", "terminal", "key1", "key6",
        ] {
            assert!(h.find(id).is_some(), "no widget with the id {id}");
        }
        // The field takes the keyboard only when clicked: nothing has it as the window opens.
        assert!(h.focused().is_none());
        // "Disabled" is greyed out.
        assert!(!widget(&h, "Disabled").1.what.enabled);
        assert_eq!(widget(&h, "heading").1.what.label, "Widgets");
        assert_eq!(widget(&h, "scroll").1.what.value, "0,0");
        // The status bar says Ready and nothing else, as it opens.
        assert!(h.describe_lines(WindowId::MAIN).iter().any(|l| l.starts_with("status bar")));
    }

    #[test]
    fn the_field_check_box_and_toggle_are_together_in_tab_order() {
        let mut h = open();
        click(&mut h, "field");
        let order = ["wrap", "pen"];
        for id in order {
            h.key(Key::Tab);
            assert_eq!(h.focused().and_then(|d| d.key).as_deref(), Some(id));
        }
    }

    #[test]
    fn the_icons_are_sixteen_rows_of_sixteen() {
        for art in [FOLDER_ART, FILE_ART, PICTURE_ART, HEART_ART, STAR_ART] {
            for row in art {
                assert_eq!(row.len(), 16, "{row:?}");
            }
        }
        for pixels in [&FOLDER, &FILE, &PICTURE, &HEART, &STAR] {
            assert!(pixels.iter().any(|p| *p != 0), "an icon with nothing in it");
            assert!(pixels.contains(&0), "an icon with no clear corner");
        }
    }

    // ---- The runner's steps, in the runner's order (`fn gallery` in xtask/src/desktop.rs). ----

    #[test]
    fn the_runners_steps() {
        let mut h = open();
        // A click.
        click(&mut h, "click");
        assert_eq!(log(&mut h), ["Clicked"]);
        // Typing into the text field.
        click(&mut h, "field");
        h.type_text("hello");
        let typed = [
            "Field(\"h\")",
            "Field(\"he\")",
            "Field(\"hel\")",
            "Field(\"hell\")",
            "Field(\"hello\")",
        ];
        assert_eq!(log(&mut h), typed);
        let field = widget(&h, "field").1;
        assert_eq!(field.what.value, "hello");
        assert!(field.focused);
        // Tab and Shift+Tab move the focus in order; Space ticks the check box.
        h.key(Key::Tab);
        assert_eq!(h.focused().unwrap().key.as_deref(), Some("wrap"));
        h.key(Key::Tab);
        assert_eq!(h.focused().unwrap().key.as_deref(), Some("pen"));
        h.key_with(Key::Tab, Modifiers(Modifiers::SHIFT));
        assert_eq!(h.focused().unwrap().key.as_deref(), Some("wrap"));
        h.key(Key::Space);
        assert_eq!(log(&mut h), ["Wrap(true)"]);
        // A menu: F10 opens File (its New is in a window of its own), Right moves to Edit, and
        // Copy (clicked) copies the field.
        h.key(Key::Function(10));
        assert_ne!(widget(&h, "New").0, WindowId::MAIN);
        h.key(Key::Right);
        assert_ne!(widget(&h, "Copy").0, WindowId::MAIN);
        h.click_on("Copy");
        assert_eq!(log(&mut h), ["Copy"]);
        assert_eq!(h.clipboard_text().as_deref(), Some("hello"));
        assert!(h.popup().is_none());
        // The clipboard, the other way: another window's text replaces the field's.
        h.set_clipboard_text("Other");
        click(&mut h, "field");
        h.shortcut(Shortcut::ctrl('a'));
        h.shortcut(Shortcut::ctrl('v'));
        assert_eq!(value(&h, "field"), "Other");
        // A dialog: a question, answered with a click.
        click(&mut h, "ask");
        assert_ne!(widget(&h, "Yes").0, WindowId::MAIN);
        h.click_on("Yes");
        // (The Ctrl+A and Ctrl+V were the field's own; the app was told of the new text.)
        assert_eq!(log(&mut h), ["Field(\"Other\")", "Ask", "Answer(true)"]);
        // The wheel over the scrolled column.
        let (x, y) = middle(&h, "scroll");
        h.scroll(x, y, 3);
        assert_ne!(value(&h, "scroll"), "0,0");
        // Close the main window (the display server's Ctrl+W): the app finishes.
        h.close(WindowId::MAIN);
        assert!(h.finished());
    }

    // ---- Every widget used once. ----

    #[test]
    fn the_menus_do_what_they_say() {
        let mut h = open();
        // File, New: the editor's text goes.
        assert_eq!(h.app().editor.line_count(), 4);
        open_menu(&mut h, 0);
        h.click_on("New");
        assert_eq!(h.app().editor.text(), "");
        // Edit, Paste: the clipboard's text goes into the field, by the menu and by its key.
        h.set_clipboard_text("pasted");
        open_menu(&mut h, 1);
        h.click_on("Paste");
        assert_eq!(h.app().text, "pasted");
        h.app_mut().text.clear();
        h.refresh();
        h.shortcut(Shortcut::ctrl('v'));
        assert_eq!(h.app().text, "pasted");
        // Edit, Clear.
        open_menu(&mut h, 1);
        h.click_on("Clear");
        assert_eq!(h.app().text, "");
        // Help, About: a message with an OK button, which only closes it.
        open_menu(&mut h, 2);
        h.click_on("About…");
        let about = h.dialog().expect("no About dialog");
        assert_ne!(about, WindowId::MAIN);
        h.click_on("OK");
        assert!(h.dialog().is_none());
        assert!(!h.finished());
        // File, Close ends the app: here by its shortcut, which the menu shows. (Chosen from
        // the open menu it would end the app with the menu still lit: see the report.)
        open_menu(&mut h, 0);
        assert_eq!(widget(&h, "Close").1.what.value, "Ctrl+W");
        h.key(Key::Escape);
        assert!(!h.finished());
        h.shortcut(Shortcut::ctrl('w'));
        assert!(h.finished());
        assert_eq!(
            log(&mut h),
            ["New", "Paste", "Paste", "Clear", "About", "Close"].map(String::from)
        );
    }

    #[test]
    fn the_shortcuts_in_the_menus_work_with_the_menus_shut() {
        let mut h = open();
        h.app_mut().text = "kept".to_string();
        h.refresh();
        h.shortcut(Shortcut::ctrl('c'));
        assert_eq!(h.clipboard_text().as_deref(), Some("kept"));
        h.shortcut(Shortcut::ctrl('n'));
        assert_eq!(log(&mut h), ["Copy", "New"]);
    }

    #[test]
    fn the_toolbar_buttons() {
        let mut h = open();
        click(&mut h, "tool");
        click(&mut h, "bold");
        assert_eq!(widget(&h, "bold").1.what.checked, Some(true));
        click(&mut h, "bold");
        assert_eq!(widget(&h, "bold").1.what.checked, Some(false));
        assert_eq!(log(&mut h), ["Tool", "Bold(true)", "Bold(false)"]);
    }

    #[test]
    fn the_question_has_a_default_yes_and_a_cancel_no() {
        let mut h = open();
        click(&mut h, "ask");
        h.key(Key::Enter);
        click(&mut h, "ask");
        h.key(Key::Escape);
        click(&mut h, "ask");
        h.click_on("No");
        assert_eq!(
            log(&mut h),
            ["Ask", "Answer(true)", "Ask", "Answer(false)", "Ask", "Answer(false)"]
        );
    }

    #[test]
    fn the_input_dialog_gives_back_the_name_typed() {
        let mut h = open();
        click(&mut h, "name");
        // The line starts with "Chris" selected, so typing replaces it.
        assert_eq!(h.focused().unwrap().what.value, "Chris");
        h.type_text("Ann");
        h.click_on("OK");
        assert_eq!(log(&mut h), ["Name", "Named(\"Ann\")"]);
        assert_eq!(h.app().name, "Ann");
        // Cancel gives nothing back, and the next dialog starts from the last name.
        click(&mut h, "name");
        assert_eq!(h.focused().unwrap().what.value, "Ann");
        h.type_text("Bob");
        h.click_on("Cancel");
        assert_eq!(log(&mut h), ["Name"]);
        assert_eq!(h.app().name, "Ann");
    }

    #[test]
    fn the_check_box_toggle_and_buttons_of_the_left_column() {
        let mut h = open();
        click(&mut h, "wrap");
        click(&mut h, "pen");
        click(&mut h, "click");
        click(&mut h, "key3");
        // A greyed-out button sends nothing.
        click(&mut h, "Disabled");
        click(&mut h, "wrap");
        assert_eq!(log(&mut h), ["Wrap(true)", "Pen(true)", "Clicked", "Digit(3)", "Wrap(false)"]);
        let keys: Vec<String> =
            (1..=6).map(|n| widget(&h, &format!("key{n}")).1.what.label).collect();
        assert_eq!(keys, ["1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn the_field_keeps_what_is_typed_and_shows_its_hint_when_empty() {
        let mut h = open();
        assert_eq!(value(&h, "field"), "");
        click(&mut h, "field");
        h.type_text("Hi");
        assert_eq!(h.app().text, "Hi");
        assert_eq!(log(&mut h), ["Field(\"H\")", "Field(\"Hi\")"]);
    }

    #[test]
    fn the_list_selects_sorts_and_opens() {
        let mut h = open();
        assert_eq!(widget(&h, "list").1.what.label, "Name|Size");
        // The first row, then the third.
        let (x, y) = row_at(&h, 0);
        h.click(x, y);
        assert_eq!(value(&h, "list"), "budget.txt|12 KB");
        let (x, y) = row_at(&h, 2);
        h.click(x, y);
        assert_eq!(value(&h, "list"), "Documents|");
        // A double click opens the row (and selects it first).
        let (x, y) = row_at(&h, 1);
        h.double_click(x, y);
        assert_eq!(value(&h, "list"), "dusk.png|148 KB");
        assert_eq!(log(&mut h), ["Select(0)", "Select(2)", "Select(1)", "Open(1)"]);
        // A click on the Name heading turns the order round, and the same row stays selected.
        let r = widget(&h, "list").1.rect;
        h.click(r.x + 40, r.y + 10);
        assert_eq!(log(&mut h), ["Sort(0)"]);
        assert_eq!(h.app().files[0].name, "todo.txt");
        assert_eq!(value(&h, "list"), "dusk.png|148 KB");
        // A click on Size sorts by it, smallest first; a second click turns it round.
        h.click(r.x + r.width - 30, r.y + 10);
        let sizes: Vec<u32> = h.app().files.iter().map(|f| f.bytes).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{sizes:?}");
        assert_eq!(value(&h, "list"), "dusk.png|148 KB");
        h.click(r.x + r.width - 30, r.y + 10);
        let sizes: Vec<u32> = h.app().files.iter().map(|f| f.bytes).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
        assert_eq!(h.app().files[0].name, "protea.png");
        // And back to the names, going up.
        h.click(r.x + 40, r.y + 10);
        assert_eq!(h.app().files[0].name, "budget.txt");
        assert_eq!(log(&mut h), ["Sort(1)", "Sort(1)", "Sort(0)"]);
    }

    #[test]
    fn the_list_has_a_dozen_rows_with_icons_and_scrolls_to_the_rest() {
        let mut h = open();
        assert_eq!(h.app().files.len(), 12);
        // Six rows show; the wheel brings the rest into view and a click then picks one that
        // was out of sight.
        let r = widget(&h, "list").1.rect;
        h.scroll(r.x + 40, r.y + 60, 10);
        h.click(r.x + 40, r.y + r.height - 12);
        assert_eq!(value(&h, "list"), "todo.txt|1 KB");
    }

    #[test]
    fn the_scroll_area_scrolls_with_the_wheel() {
        let mut h = open();
        let (x, y) = middle(&h, "scroll");
        h.scroll(x, y, 1);
        assert_eq!(value(&h, "scroll"), "0,33");
        h.scroll(x, y, -5);
        assert_eq!(value(&h, "scroll"), "0,0");
        h.scroll(x, y, 100);
        // Twenty lines of 11, with 4 above and below: 228, of which 68 show.
        assert_eq!(value(&h, "scroll"), "0,160");
    }

    #[test]
    fn the_canvas_puts_a_dot_where_it_is_clicked() {
        let mut h = open();
        assert_eq!(value(&h, "canvas"), "0");
        let r = widget(&h, "canvas").1.rect;
        // It is as wide as the column, and takes the height the column has left.
        assert_eq!(r.width, 160);
        assert!(r.height >= 50, "{r:?}");
        let before = h.picture(WindowId::MAIN).pixels;
        h.click(r.x + 30, r.y + 20);
        assert_eq!(log(&mut h), ["Dot(30, 20)"]);
        assert_eq!(value(&h, "canvas"), "1");
        assert_ne!(h.picture(WindowId::MAIN).pixels, before);
        // With the pen down the dot is the accent's.
        click(&mut h, "pen");
        h.click(r.x + 100, r.y + 50);
        assert_eq!(value(&h, "canvas"), "2");
        assert_eq!(
            h.app().dots,
            [Dot { x: 30, y: 20, pen: false }, Dot { x: 100, y: 50, pen: true }]
        );
        // Only so many dots are kept, the oldest going first.
        for i in 0..MAX_DOTS as i32 {
            h.click(r.x + 5 + i, r.y + 5);
        }
        assert_eq!(h.app().dots.len(), MAX_DOTS);
        assert_eq!(h.app().dots[0].x, 5);
    }

    #[test]
    fn the_context_menu_resets_everything() {
        let mut h = open();
        click(&mut h, "field");
        h.type_text("hello");
        click(&mut h, "wrap");
        let (x, y) = row_at(&h, 3);
        h.click(x, y);
        let r = widget(&h, "canvas").1.rect;
        h.click(r.x + 30, r.y + 20);
        click(&mut h, "terminal");
        h.type_text("ls");
        let _ = log(&mut h);
        // A right click on the column, on the empty room right of the heading.
        let (x, y) = blank_spot(&h);
        h.right_click_in(WindowId::MAIN, x, y);
        assert!(h.popup().is_some(), "the context menu didn't open");
        h.click_on("Reset");
        assert_eq!(log(&mut h), ["Reset"]);
        assert_eq!(value(&h, "field"), "");
        assert_eq!(widget(&h, "wrap").1.what.checked, Some(false));
        assert_eq!(value(&h, "list"), "");
        assert_eq!(value(&h, "terminal"), "$");
        // The canvas is drawn again, empty: its version only ever goes up.
        assert_eq!(value(&h, "canvas"), "2");
        assert!(h.app().dots.is_empty());
        // As it opened (apart from the focus and the pointer), so the reset window is the same
        // window.
        assert_eq!(h.app().files[0].name, "budget.txt");
    }

    #[test]
    fn the_text_editor_takes_typing_and_the_app_applies_it() {
        let mut h = open();
        assert!(value(&h, "editor").starts_with("line 1, column 1"));
        click(&mut h, "editor");
        h.shortcut(Shortcut::ctrl('a'));
        h.type_text("Hi");
        h.key(Key::Enter);
        h.type_text("there");
        assert_eq!(h.app().editor.text(), "Hi\nthere");
        assert_eq!(value(&h, "editor"), "line 2, column 6");
        h.shortcut(Shortcut::ctrl('z'));
        assert_eq!(h.app().editor.text(), "Hi\n");
        let entries = log(&mut h);
        assert!(entries.iter().any(|m| m == "Edit(InsertChar('H'))"), "{entries:?}");
        assert!(entries.iter().any(|m| m == "Edit(NewLine)"), "{entries:?}");
        assert!(entries.iter().any(|m| m == "Edit(Undo)"), "{entries:?}");
    }

    #[test]
    fn the_terminal_takes_typing_in_its_last_row() {
        let mut h = open();
        // (The description's text has no trailing spaces.)
        assert_eq!(value(&h, "terminal"), "$");
        click(&mut h, "terminal");
        h.type_text("ls -l");
        assert_eq!(value(&h, "terminal"), "$ ls -l");
        h.key(Key::Backspace);
        assert_eq!(value(&h, "terminal"), "$ ls -");
        // Backspace stops at the prompt.
        for _ in 0..20 {
            h.key(Key::Backspace);
        }
        assert_eq!(value(&h, "terminal"), "$");
        // Keys the terminal doesn't use still move the focus (Tab) and are not typed.
        h.key(Key::Tab);
        assert_eq!(value(&h, "terminal"), "$");
        let typed = log(&mut h);
        assert_eq!(typed[0], "Type('l')");
        assert_eq!(typed[1], "Type('s')");
        assert_eq!(typed[2], "Type(' ')");
        assert!(typed.iter().any(|m| m == "Backspace"));
        assert!(!typed.iter().any(|m| m.contains("Tab")));
    }

    #[test]
    fn the_terminal_holds_what_fits_in_its_row() {
        let mut h = open();
        click(&mut h, "terminal");
        h.type_text(&"x".repeat(200));
        assert_eq!(h.app().screen.lines[3].chars().count(), TERM_COLUMNS - 1);
    }

    #[test]
    fn the_status_bar_shows_the_last_message() {
        let mut h = open();
        click(&mut h, "click");
        let lines = h.describe_lines(WindowId::MAIN);
        let status = lines.iter().find(|l| l.starts_with("status bar")).unwrap();
        assert_eq!(h.app().last, "Clicked", "{status}");
        // A long message is cut short.
        click(&mut h, "field");
        h.type_text(&"y".repeat(100));
        assert_eq!(h.app().last.chars().count(), 48);
    }

    #[test]
    fn the_messages_log_in_their_debug_form_and_the_log_is_taken_once() {
        let mut h = open();
        click(&mut h, "click");
        click(&mut h, "wrap");
        click(&mut h, "ask");
        h.click_on("No");
        assert_eq!(log(&mut h), ["Clicked", "Wrap(true)", "Ask", "Answer(false)"]);
        assert!(log(&mut h).is_empty());
    }

    // ---- Pictures of states worth seeing, at 1x. ----

    #[test]
    fn the_file_menu_open() {
        let mut h = open();
        open_menu(&mut h, 0);
        let menu = h.popup().unwrap();
        h.check_window_picture_in(menu, &dir(), "gallery-menu-1x");
        h.check_window_picture_in(WindowId::MAIN, &dir(), "gallery-menu-window-1x");
    }

    #[test]
    fn the_context_menu_open() {
        let mut h = open();
        let (x, y) = blank_spot(&h);
        h.right_click_in(WindowId::MAIN, x, y);
        let menu = h.popup().expect("the context menu didn't open");
        h.check_window_picture_in(menu, &dir(), "gallery-context-1x");
    }

    #[test]
    fn the_question_dialog() {
        let mut h = open();
        click(&mut h, "ask");
        let dialog = h.dialog().unwrap();
        h.check_window_picture_in(dialog, &dir(), "gallery-question-1x");
    }

    #[test]
    fn the_input_dialog() {
        let mut h = open();
        click(&mut h, "name");
        let dialog = h.dialog().unwrap();
        h.check_window_picture_in(dialog, &dir(), "gallery-name-1x");
    }

    #[test]
    fn the_window_after_use() {
        let mut h = open();
        click(&mut h, "field");
        h.type_text("hello");
        click(&mut h, "wrap");
        click(&mut h, "bold");
        let (x, y) = row_at(&h, 3);
        h.click(x, y);
        let r = widget(&h, "canvas").1.rect;
        h.click(r.x + 30, r.y + 40);
        click(&mut h, "pen");
        h.click(r.x + 100, r.y + 30);
        let (x, y) = middle(&h, "scroll");
        h.scroll(x, y, 1);
        click(&mut h, "terminal");
        h.type_text("ls -l");
        h.check_window_picture_in(WindowId::MAIN, &dir(), "gallery-used-1x");
    }

    #[test]
    fn the_window_after_use_at_2x() {
        let mut h = Harness::with_scale(Gallery::new(), settings(), 2);
        click(&mut h, "field");
        h.type_text("hello");
        click(&mut h, "editor");
        h.type_text("Hi ");
        h.check_window_picture_in(WindowId::MAIN, &dir(), "gallery-used-2x");
    }

    #[test]
    fn shift_and_ctrl_clicks_reach_the_list() {
        // The core's `click_with` works through the gallery too: a Ctrl+click on a row picks it
        // as an ordinary click does (the list has no multiple selection).
        let mut h = open();
        let (x, y) = row_at(&h, 4);
        h.click_with(x, y, Modifiers(Modifiers::CTRL));
        assert_eq!(value(&h, "list"), "Music|");
    }
}
