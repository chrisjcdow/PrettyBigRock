// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Paint (docs/design/0026-apps-m6-tests-and-stages.md, section 1): PrettyBigRock 1.3's drawing
//! program as a sandstone app, tested on the host.
//!
//! It does everything 1.3's Paint did (`paint_*` in `classic/kernel/apps.c`): a white picture
//! 600 x 362 pixels to draw on with a round brush, 13 colours and 4 brush sizes in a toolbar,
//! **Open**, **Save** and **Clear** (now **New**, in a File menu), saving and opening Windows
//! bitmaps (`lib/bmp`). On top of that it has what note 0026 adds: the file picker for opening
//! and saving anywhere the user chooses, Save as, copying and pasting the picture, a "•" in the
//! title while there are unsaved changes, a file to open when it is started, and the last colour
//! chosen remembered.
//!
//! **Files and the file picker are reached only through the [`Disk`] trait**, so that this
//! library has no files in it: the program (`main.rs`) implements it over `lib/files` and the
//! picker, and the tests over a pretend disk. Everything that matters here, including what
//! the dialogs say, is therefore tested on the host.
//!
//! A word on the window. 1.3's window was 600 x 400 with a 38-pixel toolbar and could not be
//! resized, so it did not fit a small screen. This one opens at the size that shows the whole
//! picture (a menu bar, the toolbar and the picture, in *logical pixels*, the units sandstone
//! lays out in), and can be made smaller, down to [`MIN_SIZE`]; the picture is then in a
//! scrolling area (sandstone's `Scroll`) with scroll bars. On a screen with a scale of 2 each
//! logical pixel is 2 x 2 real ones, and each pixel of the picture is shown that way too:
//! **the picture is always 600 x 362 pixels**, so a saved file is the same at any scale. The
//! buttons Open, Save and Clear of 1.3's toolbar are now the File and Edit menus (which show
//! the keyboard shortcuts); **New** is 1.3's Clear.
//!
//! A note on Rust for the newcomer. `#![no_std]` (below, except in tests) means this crate uses
//! only Rust's `core` and `alloc` libraries, not the operating system's `std`, so that it can
//! run as a PrettyBigRock program; `alloc` holds `String`, `Vec` and `format!`, which need a
//! heap. A *trait* is a set of methods a type promises to have: [`Disk`] is the set Paint asks
//! of whatever holds its files, and `Paint<D: Disk>` is "a Paint over any `D` that has them".

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_draw::{Canvas as DrawCanvas, Colour, Rect};
use pbr_sandstone::event::Button as MouseButton;
use pbr_sandstone::{
    App, Canvas, Column, Context, Dialog, Element, Identify, Length, Menu, MenuBar, MenuItem,
    Panel, Pointer, PointerKind, Row, Scroll, Settings, Shortcut, Space, Theme,
};
use pbr_window::{Cursor, Win};

// ---------------------------------------------------------------------------------------------
// The numbers that make 1.3's Paint
// ---------------------------------------------------------------------------------------------

/// The picture's width, in picture pixels (1.3's `PW`).
pub const PICTURE_WIDTH: usize = 600;
/// The picture's height (1.3's `PH - PTOOL`).
pub const PICTURE_HEIGHT: usize = 362;
/// The menu bar's height: sandstone's, the theme's control height.
const MENU_BAR_HEIGHT: i32 = Theme::STANDARD.control_height;
/// The window's height when it opens: the menu bar, the toolbar, the line under it, and the
/// whole picture.
const WINDOW_HEIGHT: u32 = (MENU_BAR_HEIGHT + TOOLBAR_HEIGHT + 1) as u32 + PICTURE_HEIGHT as u32;
/// The smallest the window can be made, in logical pixels: wide enough for the whole toolbar
/// ([`TOOLS_WIDTH`], 432) and so that, with the title bar, it fits a 512 x 384 screen (a 1024 x
/// 768 one at a scale of 2). Room is left for the picture's scroll bars.
pub const MIN_SIZE: (u32, u32) = (440, 220);
/// The toolbar, without the line under it (1.3's `PTOOL` is this and the line).
const TOOLBAR_HEIGHT: i32 = 37;

/// 1.3's 13 colours, `0xRRGGBB`.
pub const PALETTE: [u32; 13] = [
    0x1b1b1f, 0xffffff, 0x8a8f98, 0xe05252, 0xf29a3a, 0xf5d04a, 0x5cc46a, 0x2f7d43, 0x3aa6a0,
    0x4a8fe0, 0x8e5cc9, 0xe8678f, 0x8a5a35,
];
/// 1.3's four brush radii.
pub const BRUSHES: [i32; 4] = [2, 5, 10, 18];
/// The colour and brush Paint starts with when nothing is remembered (1.3's: the first colour
/// and the second size).
const FIRST_COLOUR: usize = 0;
const FIRST_BRUSH: usize = 1;

/// Where the swatches start and how far apart they are: 1.3's `8 + i * PSWATCH`.
const SWATCH_X: i32 = 8;
const SWATCH_STEP: i32 = 23;
/// A swatch's width, which is also how wide a click on it counts.
const SWATCH_WIDTH: i32 = 20;
/// Where the brush sizes start (after the last swatch and a gap) and how far apart they are.
const SIZES_X: i32 = SWATCH_X + PALETTE.len() as i32 * SWATCH_STEP + 8;
const SIZES_STEP: i32 = 27;
const SIZE_WIDTH: i32 = 26;
/// How wide the toolbar's drawn part is: up to where 1.3's first button started (436), less
/// the gap that was between it and the button.
const TOOLS_WIDTH: i32 = 436 - 4;

/// The ring round the chosen colour (1.3's `0x2f6b55`), a swatch's edge, the highlight behind
/// the chosen brush size, and the dots that show the sizes.
const RING: Colour = Colour::solid(0x2f6b55);
const SWATCH_EDGE: Colour = Colour::solid(0x6b717c);
const SIZE_HIGHLIGHT: Colour = Colour::solid(0xd5d0c6);
const SIZE_DOT: Colour = Colour::solid(0x2a2e38);

/// White, the picture's paper.
const WHITE: u32 = 0x00FF_FFFF;

/// The most a file may be to be opened, as in 1.3 (8 MiB).
pub const MAX_FILE: usize = 8 * 1024 * 1024;
/// Where pictures are kept and offered, and what "Drawing N.bmp" is saved as.
pub const PICTURES: &str = "/Home/Pictures";
/// The bit of the event loop's notification that the file picker's answers come through
/// (sandstone's `FilePicker` is given this number; bits 8 to 63 are the app's own).
pub const PICKED: u32 = 9;

/// What a stroke can't go beyond, in either direction: a drag that leaves the window still
/// reports where the pointer is, and a line to somewhere far off would only take longer to
/// walk. (The picture is 600 wide, so nothing visible changes.)
const REACH: i32 = 4096;

// ---------------------------------------------------------------------------------------------
// What Paint asks of the system
// ---------------------------------------------------------------------------------------------

/// Why a file couldn't be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskError {
    /// More than the limit given: not a picture Paint will try to open.
    TooBig,
    /// The file system said no, in words ("not found").
    Failed(String),
}

/// What Paint wants the file picker to ask the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A `.bmp` file to open, starting in Pictures.
    Open,
    /// Where to save, starting in Pictures and offering `name`.
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

/// Everything Paint does with files, and the file picker. Paths are the ones the user sees
/// (`/Home/Pictures/rock.bmp`).
///
/// Paint is a *sandboxed* app (note 0017): it can reach only the folders its manifest and the
/// user gave it, and a file the user picks anywhere else is handed over by the **file picker**,
/// a separate system app (note 0025), as a capability, a right to use that one file. The
/// picker waits for the user, so asking returns at once and the answer comes later: the
/// program tells the app with notification bit [`PICKED`], and [`Disk::answer`] gives it.
pub trait Disk {
    /// Whether a file or folder is at `path`.
    fn exists(&mut self, path: &str) -> bool;
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
// The toolbar's geometry, which the tests and the desktop boot's runner use too
// ---------------------------------------------------------------------------------------------

/// What a click on the toolbar's drawn part chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// A colour, by its number in [`PALETTE`].
    Colour(usize),
    /// A brush size, by its number in [`BRUSHES`].
    Size(usize),
}

/// What a click at (`x`, `y`), counted from the toolbar's top left corner, chooses (1.3's
/// `paint_mouse`): any height within the toolbar counts, as in 1.3.
pub fn tool_at(x: i32, y: i32) -> Option<Tool> {
    if !(0..TOOLBAR_HEIGHT).contains(&y) {
        return None;
    }
    let swatch = (0..PALETTE.len() as i32).find(|i| {
        let left = SWATCH_X + i * SWATCH_STEP;
        (left..left + SWATCH_WIDTH).contains(&x)
    });
    if let Some(i) = swatch {
        return Some(Tool::Colour(i as usize));
    }
    (0..BRUSHES.len() as i32)
        .find(|i| {
            let left = SIZES_X + i * SIZES_STEP;
            (left..left + SIZE_WIDTH).contains(&x)
        })
        .map(|i| Tool::Size(i as usize))
}

/// The middle of a tool, counted from the toolbar's top left corner: where to click.
pub fn tool_centre(tool: Tool) -> (i32, i32) {
    match tool {
        Tool::Colour(i) => (SWATCH_X + i as i32 * SWATCH_STEP + SWATCH_WIDTH / 2, 19),
        Tool::Size(i) => (SIZES_X + i as i32 * SIZES_STEP + SIZE_WIDTH / 2, 19),
    }
}

/// The toolbar's drawn part: the colours, the ring round the chosen one, the sizes as dots,
/// the chosen one highlighted. `area` is its rectangle in the window.
fn draw_tools(c: &mut DrawCanvas<'_>, area: Rect, colour: usize, size: usize) {
    for (i, rgb) in PALETTE.iter().enumerate() {
        let x = area.x + SWATCH_X + i as i32 * SWATCH_STEP;
        if i == colour {
            c.fill_round_rect(Rect::new(x - 3, area.y + 6, 26, 26), 5, RING);
        }
        c.fill_round_rect(Rect::new(x, area.y + 9, 20, 20), 4, SWATCH_EDGE);
        c.fill_round_rect(Rect::new(x + 1, area.y + 10, 18, 18), 3, Colour::solid(*rgb));
    }
    for i in 0..BRUSHES.len() {
        let x = area.x + SIZES_X + i as i32 * SIZES_STEP;
        if i == size {
            c.fill_round_rect(Rect::new(x, area.y + 6, 26, 26), 5, SIZE_HIGHLIGHT);
        }
        // The dots are 1.3's: radius 1, 3, 5, 7, whatever the brush's own radius is.
        let r = 1 + 2 * i as i32;
        c.fill_ellipse(Rect::new(x + 13 - r, area.y + 19 - r, 2 * r + 1, 2 * r + 1), SIZE_DOT);
    }
}

/// A file's name: what is after the last `/`.
fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// An error from the disk in words (for logs and dialogs).
fn why_text(e: &DiskError) -> String {
    match e {
        DiskError::TooBig => String::from("it is too big"),
        DiskError::Failed(why) => why.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------

/// What the window's widgets send.
#[derive(Clone, Debug)]
pub enum Message {
    /// A colour or a brush size was clicked.
    Tool(Tool),
    /// The pointer drew on the picture.
    Stroke(Stroke),
    /// File > Open (Ctrl+O): the file picker.
    Open,
    /// File > Save (Ctrl+S): to the file the picture came from, or the picker if none.
    Save,
    /// File > Save as (Ctrl+Shift+S): the picker.
    SaveAs,
    /// File > New (Ctrl+N), 1.3's Clear: a white picture, from no file.
    New,
    /// Edit > Copy (Ctrl+C): the picture to the clipboard.
    Copy,
    /// Edit > Paste (Ctrl+V): the clipboard's picture onto the picture's top left.
    Paste,
    /// The file picker has answered ([`Disk::answer`]).
    Picked,
}

/// What the pointer did on the picture, in picture pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    /// The left button went down here: a dab of the brush.
    Down(i32, i32),
    /// The pointer moved here with the button down: a line from where it was.
    To(i32, i32),
    /// The button came up.
    Up,
}

/// What a file picker that is open is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Open,
    Save,
}

/// Paint, over a [`Disk`].
pub struct Paint<D: Disk> {
    disk: D,
    /// The picture, `0x00RRGGBB`, row after row.
    pixels: Vec<u32>,
    /// Changes whenever the pixels do, so that sandstone draws the picture's canvas again
    /// (it can't see what the drawing function would draw).
    version: u64,
    /// The colour's and brush size's numbers.
    colour: usize,
    size: usize,
    /// The left button is down on the picture, and where the brush was last.
    drawing: bool,
    last: (i32, i32),
    /// The file the picture came from or was saved to, and whether it has changed since.
    path: Option<String>,
    dirty: bool,
    /// The file to open when the window opens (the program's argument).
    initial: Option<String>,
    /// What the file picker, if one is open, is for.
    asked: Option<Asked>,
    /// Where the last colour is kept.
    colour_path: String,
    /// Lines for the program's log, and whether the picture changed (a stroke ended, a file
    /// opened, a paste, New) since the program last looked.
    log: Vec<String>,
    picture_changed: bool,
}

impl<D: Disk> Paint<D> {
    /// Paint with a white picture. `data_folder` is the app's own folder (`/Home/Apps/paint`),
    /// where the last colour is kept in `colour.txt`; if it holds a colour's number, that
    /// colour is the one chosen to start with.
    pub fn new(mut disk: D, data_folder: &str) -> Paint<D> {
        let colour_path = format!("{data_folder}/colour.txt");
        // `.ok()` turns a `Result` into an `Option` (an error becomes `None`); `and_then`
        // carries on only while there is a value: no file, or text that isn't a colour's
        // number, simply means the first colour.
        let remembered = disk
            .read(&colour_path, 64)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| text.trim().parse::<usize>().ok())
            .filter(|n| *n < PALETTE.len());
        let mut paint = Paint {
            disk,
            pixels: vec![WHITE; PICTURE_WIDTH * PICTURE_HEIGHT],
            version: 1,
            colour: remembered.unwrap_or(FIRST_COLOUR),
            size: FIRST_BRUSH,
            drawing: false,
            last: (0, 0),
            path: None,
            dirty: false,
            initial: None,
            asked: None,
            colour_path,
            log: Vec::new(),
            picture_changed: false,
        };
        paint.log_tools();
        paint
    }

    /// Opens `path` when the window opens (the file the program was started with).
    pub fn with_file(mut self, path: &str) -> Paint<D> {
        self.initial = Some(String::from(path));
        self
    }

    // ---- What the program and the tests look at. ----

    /// The picture's pixels, `0x00RRGGBB`, row after row.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The chosen colour's number in [`PALETTE`].
    pub fn colour(&self) -> usize {
        self.colour
    }

    /// The chosen brush's number in [`BRUSHES`].
    pub fn size(&self) -> usize {
        self.size
    }

    /// The file the picture came from or was last saved to.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Whether the picture has changed since it was opened or saved.
    pub fn is_dirty(&self) -> bool {
        self.dirty
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
    /// chosen (`colour 3 size 1`), saved (`saved /Home/Pictures/Drawing 1.bmp (651654 bytes)`),
    /// opened, `new`, and what went wrong (`couldn't ...`).
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Whether the picture changed (a stroke ended, a file was opened, a picture pasted, New)
    /// since the last call. The program then writes [`Paint::picture_line`], in test boots.
    pub fn take_picture_changed(&mut self) -> bool {
        core::mem::take(&mut self.picture_changed)
    }

    /// `picture 600x362 crc 1a2b3c4d`: the CRC-32 (`lib/crc`) of the picture's pixels, each as
    /// its four bytes `BB GG RR 00` (the number's least significant byte first), row after
    /// row. It is what the desktop boot's runner compares with what the picture should be.
    pub fn picture_line(&self) -> String {
        let crc = self.pixels.iter().fold(0, |crc, p| pbr_crc::update(crc, &p.to_le_bytes()));
        format!("picture {PICTURE_WIDTH}x{PICTURE_HEIGHT} crc {crc:08x}")
    }

    // ---- Drawing. ----

    /// Stamps the brush at (`x`, `y`): 1.3's `p_stamp`, a filled circle of the chosen radius
    /// in the chosen colour, cut off at the picture's edges.
    fn stamp(&mut self, x: i32, y: i32) {
        let r = BRUSHES[self.size];
        let colour = PALETTE[self.colour];
        for dy in -r..=r {
            let row = y + dy;
            if !(0..PICTURE_HEIGHT as i32).contains(&row) {
                continue;
            }
            // How far the circle reaches along this row: the whole number at or under the
            // square root, as 1.3's `isqrt` gave.
            let dx = ((r * r - dy * dy) as u32).isqrt() as i32;
            let from = (x - dx).max(0);
            let to = (x + dx).min(PICTURE_WIDTH as i32 - 1);
            if from <= to {
                let start = row as usize * PICTURE_WIDTH;
                self.pixels[start + from as usize..=start + to as usize].fill(colour);
            }
        }
    }

    /// Stamps the brush all along the line from `from` to (`x1`, `y1`): Bresenham's line,
    /// step for step as 1.3's `p_line` took it, so a stroke has exactly 1.3's shape.
    fn line(&mut self, (mut x0, mut y0): (i32, i32), (x1, y1): (i32, i32)) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.stamp(x0, y0);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// The pointer on the picture.
    fn stroke(&mut self, stroke: Stroke) {
        let near = |x: i32, y: i32| (x.clamp(-REACH, REACH), y.clamp(-REACH, REACH));
        match stroke {
            Stroke::Down(x, y) => {
                let at = near(x, y);
                self.drawing = true;
                self.last = at;
                self.stamp(at.0, at.1);
                self.changed();
            }
            Stroke::To(x, y) if self.drawing => {
                let at = near(x, y);
                self.line(self.last, at);
                self.last = at;
                self.changed();
            }
            Stroke::To(..) => {}
            Stroke::Up => {
                if self.drawing {
                    self.drawing = false;
                    self.picture_changed = true;
                }
            }
        }
    }

    /// The picture's pixels changed by the user's doing: draw it again, and it needs saving.
    fn changed(&mut self) {
        self.version += 1;
        self.dirty = true;
    }

    /// Chooses a colour or a brush size, and keeps the colour for next time.
    fn choose(&mut self, tool: Tool) {
        match tool {
            Tool::Colour(i) if i < PALETTE.len() && i != self.colour => {
                self.colour = i;
                self.log_tools();
                let text = format!("{i}\n");
                if let Err(why) = self.disk.write(&self.colour_path, text.as_bytes()) {
                    self.log.push(format!("couldn't keep the colour: {}", why_text(&why)));
                }
            }
            Tool::Size(i) if i < BRUSHES.len() && i != self.size => {
                self.size = i;
                self.log_tools();
            }
            _ => {}
        }
    }

    fn log_tools(&mut self) {
        self.log.push(format!("colour {} size {}", self.colour, self.size));
    }

    // ---- Files. ----

    /// The name the picker offers for saving: the file the picture came from, or the first
    /// "Drawing N.bmp" there is no file called yet in Pictures (1.3's).
    fn suggested_name(&mut self) -> String {
        if let Some(path) = &self.path {
            return String::from(base_name(path));
        }
        for n in 1..1000 {
            let name = format!("Drawing {n}.bmp");
            if !self.disk.exists(&format!("{PICTURES}/{name}")) {
                return name;
            }
        }
        String::from("Drawing.bmp")
    }

    /// Asks the file picker, unless one is open already. The picker opens only for a press
    /// that is recent, which is why this is called from `update` with the context that has the
    /// press's serial.
    fn ask(&mut self, what: Asked, pick: Pick, cx: &mut Context<'_, Message>) {
        if self.asked.is_some() {
            return;
        }
        match self.disk.pick(cx.display_window(), cx.serial(), pick) {
            Ok(()) => self.asked = Some(what),
            Err(why) => self.picker_failed(what, &why, cx),
        }
    }

    /// There is no picker, or it couldn't answer: say so.
    fn picker_failed(&mut self, what: Asked, why: &str, cx: &mut Context<'_, Message>) {
        self.log.push(format!("couldn't use the picker: {why}"));
        let message = match what {
            Asked::Open => format!("Couldn't open the file picker. {why}."),
            Asked::Save => format!("Couldn't save the picture. {why}."),
        };
        cx.show(Dialog::message("Paint", &message));
    }

    /// Save (`ask_always` false) or Save as.
    fn save(&mut self, ask_always: bool, cx: &mut Context<'_, Message>) {
        match self.path.clone() {
            Some(path) if !ask_always => self.write_to(&path, cx),
            _ => {
                let name = self.suggested_name();
                self.ask(Asked::Save, Pick::Save { name }, cx);
            }
        }
    }

    /// Writes the picture to `path` as a 24-bit bitmap, and remembers it as the file.
    fn write_to(&mut self, path: &str, cx: &mut Context<'_, Message>) {
        let picture =
            pbr_bmp::Picture::new(PICTURE_WIDTH as u32, PICTURE_HEIGHT as u32, self.pixels.clone());
        let Some(picture) = picture else { return };
        let bytes = pbr_bmp::encode(&picture);
        match self.disk.write(path, &bytes) {
            Ok(()) => {
                self.path = Some(String::from(path));
                self.dirty = false;
                self.log.push(format!("saved {path} ({} bytes)", bytes.len()));
            }
            Err(why) => {
                let why = why_text(&why);
                self.log.push(format!("couldn't save {path}: {why}"));
                cx.show(Dialog::message("Paint", &format!("Couldn't save the picture. {why}.")));
            }
        }
    }

    /// Opens the bitmap at `path`. One bigger than the picture is cut to it; a smaller one
    /// sits at the top left, on white (1.3's).
    fn open_file(&mut self, path: &str, cx: &mut Context<'_, Message>) {
        let name = base_name(path);
        let not_a_picture = format!(
            "{name} isn't a picture Paint can open. It opens uncompressed 24- or 32-bit .bmp files."
        );
        let bytes = match self.disk.read(path, MAX_FILE) {
            Ok(bytes) => bytes,
            Err(DiskError::TooBig) => {
                self.log.push(format!("couldn't open {path}: it is too big"));
                cx.show(Dialog::message("Paint", &not_a_picture));
                return;
            }
            Err(DiskError::Failed(why)) => {
                self.log.push(format!("couldn't open {path}: {why}"));
                cx.show(Dialog::message("Paint", &format!("Couldn't open {name}. {why}.")));
                return;
            }
        };
        let picture = match pbr_bmp::decode(&bytes) {
            Ok(picture) => picture,
            Err(e) => {
                self.log.push(format!("couldn't open {path}: {e}"));
                cx.show(Dialog::message("Paint", &not_a_picture));
                return;
            }
        };
        self.pixels.fill(WHITE);
        self.paste_pixels(picture.width(), picture.height(), picture.pixels());
        self.path = Some(String::from(path));
        self.dirty = false;
        self.version += 1;
        self.picture_changed = true;
        self.log.push(format!("opened {path}"));
    }

    /// Puts a picture of `width` by `height` pixels on the top left corner, cut to what fits.
    /// `pixels` must be exactly that many.
    fn paste_pixels(&mut self, width: u32, height: u32, pixels: &[u32]) {
        let width = (width as usize).max(1);
        let across = width.min(PICTURE_WIDTH);
        let rows = (height as usize).min(PICTURE_HEIGHT);
        for (y, row) in pixels.chunks_exact(width).take(rows).enumerate() {
            let start = y * PICTURE_WIDTH;
            for (dest, &p) in self.pixels[start..start + across].iter_mut().zip(row) {
                *dest = p & 0x00FF_FFFF;
            }
        }
    }

    /// The file picker's answer: open or save what was chosen.
    fn picked(&mut self, cx: &mut Context<'_, Message>) {
        let Some(answer) = self.disk.answer() else { return };
        let what = self.asked.take();
        match (answer, what) {
            (PickAnswer::Chosen(path), Some(Asked::Open)) => self.open_file(&path, cx),
            (PickAnswer::Chosen(path), Some(Asked::Save)) => self.write_to(&path, cx),
            (PickAnswer::Cancelled, _) | (PickAnswer::Chosen(_), None) => {}
            (PickAnswer::Failed(why), what) => {
                self.picker_failed(what.unwrap_or(Asked::Open), &why, cx);
            }
        }
    }

    // ---- The parts of the view. ----

    /// The menu bar: File and Edit. The rows' shortcuts work while the menus are shut, so
    /// they are given here and nowhere else.
    fn menu_bar(&self) -> Element<'_, Message> {
        let file = Menu::new(
            "File",
            vec![
                MenuItem::new("New", Message::New).shortcut(Shortcut::ctrl('n')),
                MenuItem::new("Open…", Message::Open).shortcut(Shortcut::ctrl('o')),
                MenuItem::new("Save", Message::Save).shortcut(Shortcut::ctrl('s')),
                MenuItem::new("Save as…", Message::SaveAs).shortcut(Shortcut::ctrl_shift('s')),
            ],
        );
        let edit = Menu::new(
            "Edit",
            vec![
                MenuItem::new("Copy", Message::Copy).shortcut(Shortcut::ctrl('c')),
                MenuItem::new("Paste", Message::Paste).shortcut(Shortcut::ctrl('v')),
            ],
        );
        MenuBar::new(vec![file, edit]).into()
    }

    /// The toolbar: the colours and sizes, on the bar's paper.
    fn toolbar(&self) -> Element<'_, Message> {
        let (colour, size) = (self.colour, self.size);
        // `move` gives the drawing function its own copy of the two numbers, so that it
        // doesn't borrow from `self`.
        let tools =
            Canvas::new(TOOLS_WIDTH, TOOLBAR_HEIGHT, move |c: &mut DrawCanvas<'_>, area: Rect| {
                draw_tools(c, area, colour, size)
            })
            // Everything the drawing depends on, as one number.
            .version(((colour as u64) << 8 | size as u64) + 1)
            .on_pointer(|p: Pointer| {
                p.pressed(MouseButton::Left)
                    .and_then(|_| tool_at(p.at.x, p.at.y))
                    .map(Message::Tool)
            })
            .id("tools");
        Panel::new(Row::new().push(tools))
            .width(Length::Fill)
            .height(Length::Fixed(TOOLBAR_HEIGHT))
            .background(Theme::STANDARD.bar)
            .into()
    }

    /// The picture, as a canvas of its own size.
    fn picture(&self) -> Element<'_, Message> {
        let pixels = &self.pixels;
        let drawing = self.drawing;
        Canvas::new(
            PICTURE_WIDTH as i32,
            PICTURE_HEIGHT as i32,
            move |c: &mut DrawCanvas<'_>, area: Rect| {
                // Each pixel of the picture is one logical pixel, so at a scale of 2 it is
                // shown 2 x 2: `picture_fit` draws whole-number sizes crisp.
                let all = pbr_draw::Picture::new(
                    pixels,
                    PICTURE_WIDTH as u32,
                    PICTURE_HEIGHT as u32,
                    PICTURE_WIDTH as u32,
                );
                if let Some(all) = all {
                    c.picture_fit(&all, area);
                }
            },
        )
        .version(self.version)
        .cursor(Cursor::Crosshair)
        // Moves and the release matter only to a stroke begun on the picture, so that moving
        // the pointer over it makes Paint do nothing.
        .on_pointer(move |p: Pointer| match p.kind {
            PointerKind::Pressed { button: MouseButton::Left, .. } => {
                Some(Message::Stroke(Stroke::Down(p.at.x, p.at.y)))
            }
            PointerKind::Moved if drawing => Some(Message::Stroke(Stroke::To(p.at.x, p.at.y))),
            PointerKind::Released { button: MouseButton::Left } if drawing => {
                Some(Message::Stroke(Stroke::Up))
            }
            _ => None,
        })
        .id("picture")
    }
}

/// The window: opens at the size that shows the whole picture (600 wide, 424 high in logical
/// pixels), and can be resized down to [`MIN_SIZE`].
pub fn settings() -> Settings {
    Settings {
        size: (PICTURE_WIDTH as u32, WINDOW_HEIGHT),
        resizable: true,
        min_size: MIN_SIZE,
        over: None,
        system: false,
    }
}

impl<D: Disk> App for Paint<D> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.menu_bar())
            .push(self.toolbar())
            // The line under the toolbar, one pixel.
            .push(
                Panel::new(Space::new(0, 0))
                    .width(Length::Fill)
                    .height(Length::Fixed(1))
                    .background(Theme::STANDARD.bar_edge),
            )
            // The picture keeps its own size: bigger than the room, it scrolls; smaller, it is
            // at the top left of the window's paper.
            .push(Scroll::new(self.picture()).both())
            .into()
    }

    /// "NAME" or "Untitled", with "• " before it while there are unsaved changes. Sandstone
    /// puts "Paint" before it.
    fn title(&self) -> String {
        let name = self.path.as_deref().map_or("Untitled", base_name);
        format!("{}{name}", if self.dirty { "• " } else { "" })
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        if let Some(path) = self.initial.take() {
            self.open_file(&path, cx);
        }
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Tool(tool) => self.choose(tool),
            Message::Stroke(stroke) => self.stroke(stroke),
            Message::Open => self.ask(Asked::Open, Pick::Open, cx),
            Message::Save => self.save(false, cx),
            Message::SaveAs => self.save(true, cx),
            Message::New => {
                self.pixels.fill(WHITE);
                self.path = None;
                self.dirty = false;
                self.version += 1;
                self.picture_changed = true;
                self.log.push(String::from("new"));
            }
            Message::Copy => {
                cx.copy_picture(PICTURE_WIDTH as u32, PICTURE_HEIGHT as u32, &self.pixels);
            }
            Message::Paste => {
                // The clipboard is another app's doing, so it is checked: the picture's size
                // must agree with its pixels (`checked_mul` gives `None` instead of
                // overflowing).
                if let Some((width, height, pixels)) = cx.paste_picture()
                    && (width as usize).checked_mul(height as usize) == Some(pixels.len())
                    && !pixels.is_empty()
                {
                    self.paste_pixels(width, height, &pixels);
                    self.changed();
                    self.picture_changed = true;
                }
            }
            Message::Picked => self.picked(cx),
        }
    }

    fn notified(&self, bits: u64) -> Option<Message> {
        (bits & (1 << PICKED) != 0).then_some(Message::Picked)
    }
}

#[cfg(test)]
mod tests;
