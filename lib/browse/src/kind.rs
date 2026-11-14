// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Kinds of file, told by their names, and the icon for each.
//!
//! Each icon is drawn here as a *text picture*: 16 lines of 16 letters, the same way
//! `lib/desktop/src/icons.rs` draws the desktop's icons, with the same palette. A `.` is a dot
//! left clear; any other letter is a colour (see [`rgb_of`]). Keeping the pictures as text means
//! they can be read, and changed, by eye. A `const fn` ([`build`]) turns each picture into 256
//! numbers while the library is *compiled*, so using an icon later needs no memory to be asked
//! for and no work to be done.

use crate::path;

/// What sort of thing an item is, for its icon and its "Kind" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A folder.
    Folder,
    /// Text a person can read: notes, settings, source code.
    Text,
    /// A picture.
    Picture,
    /// Sound or music.
    Music,
    /// A film.
    Video,
    /// Files packed together.
    Archive,
    /// Something to run.
    Program,
    /// Anything else.
    Other,
}

/// Every kind, in the order of the enum.
pub const ALL: [Kind; 8] = [
    Kind::Folder,
    Kind::Text,
    Kind::Picture,
    Kind::Music,
    Kind::Video,
    Kind::Archive,
    Kind::Program,
    Kind::Other,
];

/// The extensions of each kind of file, in lower case.
const TEXT: &[&str] = &[
    "txt", "md", "log", "ini", "cfg", "conf", "csv", "json", "toml", "rs", "c", "h", "py", "sh",
    "bat",
];
const PICTURE: &[&str] = &["bmp", "png", "jpg", "jpeg", "gif"];
const MUSIC: &[&str] = &["wav", "mp3", "ogg", "flac", "mid", "midi"];
const VIDEO: &[&str] = &["avi", "mp4", "mkv", "webm", "mov"];
const ARCHIVE: &[&str] = &["zip", "tar", "gz", "tgz", "7z"];
const PROGRAM: &[&str] = &["app", "elf", "drv"];

/// The kind of an item from its name (case does not matter: `PHOTO.JPG` is a picture).
/// A name that only starts with a dot (`.profile`) has no extension.
pub fn kind_of(name: &str, folder: bool) -> Kind {
    if folder {
        return Kind::Folder;
    }
    let Some(ext) = path::extension(name) else {
        return Kind::Other;
    };
    let is = |list: &[&str]| list.iter().any(|known| known.eq_ignore_ascii_case(ext));
    if is(TEXT) {
        Kind::Text
    } else if is(PICTURE) {
        Kind::Picture
    } else if is(MUSIC) {
        Kind::Music
    } else if is(VIDEO) {
        Kind::Video
    } else if is(ARCHIVE) {
        Kind::Archive
    } else if is(PROGRAM) {
        Kind::Program
    } else {
        Kind::Other
    }
}

impl Kind {
    /// What the "Kind" column says.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Text => "Text document",
            Kind::Picture => "Picture",
            Kind::Music => "Music",
            Kind::Video => "Video",
            Kind::Archive => "Archive",
            Kind::Program => "Program",
            Kind::Other => "File",
        }
    }

    /// The icon: 16 x 16 dots, row after row, each `0xAARRGGBB` (alpha `0xFF` solid, `0`
    /// clear). The reference is `'static`: the icons live as long as the program does.
    pub fn icon(self) -> &'static [u32; 256] {
        match self {
            Kind::Folder => &FOLDER_ICON,
            Kind::Text => &TEXT_ICON,
            Kind::Picture => &PICTURE_ICON,
            Kind::Music => &MUSIC_ICON,
            Kind::Video => &VIDEO_ICON,
            Kind::Archive => &ARCHIVE_ICON,
            Kind::Program => &PROGRAM_ICON,
            Kind::Other => &OTHER_ICON,
        }
    }
}

/// A picture: 16 lines of 16 letters.
pub(crate) type Picture = [&'static str; 16];

/// The colour a letter stands for (the palette of `lib/desktop/src/icons.rs`, which is 1.3's),
/// as `0xRRGGBB`, or `None` for a clear dot.
const fn rgb_of(letter: u8) -> Option<u32> {
    Some(match letter {
        b'k' => 0x1b1b1f,
        b'w' => 0xffffff,
        b'g' => 0xc8ccd2,
        b'G' => 0x6b717c,
        b'r' => 0xe05252,
        b'o' => 0xf29a3a,
        b'y' => 0xf5d04a,
        b'e' => 0x5cc46a,
        b'E' => 0x2f7d43,
        b'b' => 0x4a8fe0,
        b'B' => 0x2b4c7e,
        b'p' => 0xe8678f,
        b'P' => 0xa8325a,
        b'n' => 0x8a5a35,
        b't' => 0x3aa6a0,
        b'c' => 0xf4efe2,
        b's' => 0x2a2f3a,
        b'm' => 0x5a4680,
        b'M' => 0x382c58,
        b'l' => 0xe0a090,
        b'O' => 0xffc070,
        _ => return None,
    })
}

/// Turns a picture into 256 dots. `const fn` means the compiler can run it while building, so
/// `static ICON: [u32; 256] = build(&PICTURE)` is worked out once, before the program ever runs.
/// (A `while` loop is used because `for` loops are not allowed in a `const fn` yet.) A line that
/// is too short, or a letter that is not in the palette, is simply a clear dot.
pub(crate) const fn build(picture: &Picture) -> [u32; 256] {
    let mut dots = [0u32; 256];
    let mut row = 0;
    while row < 16 {
        let line = picture[row].as_bytes();
        let mut column = 0;
        while column < 16 && column < line.len() {
            if let Some(rgb) = rgb_of(line[column]) {
                dots[row * 16 + column] = 0xFF00_0000 | rgb;
            }
            column += 1;
        }
        row += 1;
    }
    dots
}

/// 1.3's folder (`lib/desktop`'s Files picture): a manila folder with a brown outline.
pub(crate) const FOLDER: Picture = [
    "................",
    "................",
    ".nnnnnn.........",
    ".noooooon.......",
    ".nooooooonnnnnn.",
    ".noooooooooooon.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nyyyyyyyyyyyyn.",
    ".nnnnnnnnnnnnnn.",
    "................",
    "................",
];

/// A page with grey lines of text.
pub(crate) const TEXT_PAGE: Picture = [
    "................",
    "...sssssss......",
    "...swwwwwss.....",
    "...swwwwwsgs....",
    "...swwwwwssss...",
    "...swwwwwwwws...",
    "...swGGGGGGws...",
    "...swwwwwwwws...",
    "...swGGGGGGws...",
    "...swwwwwwwws...",
    "...swGGGGGGws...",
    "...swwwwwwwws...",
    "...swGGGGwwws...",
    "...swwwwwwwws...",
    "...ssssssssss...",
    "................",
];

/// A page with a small landscape: blue sky, a yellow sun, a green hill.
pub(crate) const PICTURE_PAGE: Picture = [
    "................",
    "...sssssss......",
    "...swwwwwss.....",
    "...swwwwwsgs....",
    "...swwwwwssss...",
    "...swwwwwwwws...",
    "...swbbbbbbws...",
    "...swbbbbybws...",
    "...swbbbbbbws...",
    "...swbbeebbws...",
    "...sweeeeeews...",
    "...sweeeeeews...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...ssssssssss...",
    "................",
];

/// A page with a note (an eighth note, in purple).
pub(crate) const MUSIC_PAGE: Picture = [
    "................",
    "...sssssss......",
    "...swwwwwss.....",
    "...swwwwwsgs....",
    "...swwwwwssss...",
    "...swwwwwwwws...",
    "...swwwwmmmws...",
    "...swwwwmmwws...",
    "...swwwwmwwws...",
    "...swwwwmwwws...",
    "...swwwwmwwws...",
    "...swmmmmwwws...",
    "...swmmmmwwws...",
    "...swwwwwwwws...",
    "...ssssssssss...",
    "................",
];

/// A page with a dark screen and a play triangle.
pub(crate) const VIDEO_PAGE: Picture = [
    "................",
    "...sssssss......",
    "...swwwwwss.....",
    "...swwwwwsgs....",
    "...swwwwwssss...",
    "...swwwwwwwws...",
    "...swBBBBBBws...",
    "...swBwBBBBws...",
    "...swBwwBBBws...",
    "...swBwwBBBws...",
    "...swBwBBBBws...",
    "...swBBBBBBws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...ssssssssss...",
    "................",
];

/// A box with a zip down the middle.
pub(crate) const ARCHIVE_BOX: Picture = [
    "................",
    "................",
    "..nnnnnnnnnnnn..",
    "..nOOOOkgOOOOn..",
    "..nOOOOgkOOOOn..",
    "..nOOOOkgOOOOn..",
    "..nOOOOgkOOOOn..",
    "..nOOOOkgOOOOn..",
    "..nOOOOgkOOOOn..",
    "..nOOOkkkkOOOn..",
    "..nOOOkOOkOOOn..",
    "..nOOOkkkkOOOn..",
    "..nOOOOOOOOOOn..",
    "..nOOOOOOOOOOn..",
    "..nnnnnnnnnnnn..",
    "................",
];

/// A small window with a blue title bar.
pub(crate) const WINDOW: Picture = [
    "................",
    "................",
    ".ssssssssssssss.",
    ".sbbbbbbbbbbbbs.",
    ".sbbbbbbbbbwwbs.",
    ".ssssssssssssss.",
    ".swwwwwwwwwwwws.",
    ".swggggggggwwws.",
    ".swwwwwwwwwwwws.",
    ".swggggggwwwwws.",
    ".swwwwwwwwwwwws.",
    ".swgggggggggwws.",
    ".swwwwwwwwwwwws.",
    ".ssssssssssssss.",
    "................",
    "................",
];

/// A plain page with a folded corner.
pub(crate) const PLAIN_PAGE: Picture = [
    "................",
    "...sssssss......",
    "...swwwwwss.....",
    "...swwwwwsgs....",
    "...swwwwwssss...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...swwwwwwwws...",
    "...ssssssssss...",
    "................",
];

pub(crate) static FOLDER_ICON: [u32; 256] = build(&FOLDER);
pub(crate) static TEXT_ICON: [u32; 256] = build(&TEXT_PAGE);
pub(crate) static PICTURE_ICON: [u32; 256] = build(&PICTURE_PAGE);
pub(crate) static MUSIC_ICON: [u32; 256] = build(&MUSIC_PAGE);
pub(crate) static VIDEO_ICON: [u32; 256] = build(&VIDEO_PAGE);
static ARCHIVE_ICON: [u32; 256] = build(&ARCHIVE_BOX);
static PROGRAM_ICON: [u32; 256] = build(&WINDOW);
static OTHER_ICON: [u32; 256] = build(&PLAIN_PAGE);
