// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The icons: 1.3's pictures of 16 x 16 dots (`classic/tools/mkassets.py` drew them), and one
//! plain picture of a window for apps that have none of their own.
//!
//! Each picture is 16 lines of 16 letters. A `.` is a dot that is left clear (whatever is
//! behind shows through); any other letter is a colour from [`colour_of`]. Keeping the pictures
//! as text means they can be read, and changed, by eye.
//!
//! An icon is drawn by filling a small square for each dot. `size` says how many logical
//! pixels a dot is: 1 in the taskbar and the menu (a 16 x 16 icon), 3 on the desktop (48 x 48).

use pbr_draw::{Canvas, Colour, Rect};

/// Which picture to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// The PrettyBigRock logo: a rock with a sun behind it.
    Logo,
    /// A screen with a green prompt.
    Terminal,
    /// A page of text.
    Notepad,
    /// A folder.
    Files,
    /// A calculator.
    Calculator,
    /// A palette and a brush.
    Paint,
    /// A green snake and its apple.
    Snake,
    /// A blue disc with an "i".
    About,
    /// A plain window, for an app whose manifest names no icon (or one we have no picture for).
    App,
    /// A circular arrow.
    Restart,
    /// The power symbol.
    ShutDown,
}

impl Icon {
    /// The icon a manifest's `icon NAME` names ("terminal", "notepad", "files", "calculator",
    /// "paint", "snake", "about"); anything else is [`Icon::App`].
    ///
    /// (`&str` is a borrowed piece of text. The names are matched exactly: a manifest is
    /// written by a program's author, and a near miss is better shown as the plain window than
    /// guessed at.)
    pub fn from_name(name: &str) -> Icon {
        match name {
            "terminal" => Icon::Terminal,
            "notepad" => Icon::Notepad,
            "files" => Icon::Files,
            "calculator" => Icon::Calculator,
            "paint" => Icon::Paint,
            "snake" => Icon::Snake,
            "about" => Icon::About,
            _ => Icon::App,
        }
    }

    /// The icon's 16 lines of 16 dots.
    fn picture(self) -> &'static Picture {
        match self {
            Icon::Logo => &LOGO,
            Icon::Terminal => &TERMINAL,
            Icon::Notepad => &NOTEPAD,
            Icon::Files => &FILES,
            Icon::Calculator => &CALCULATOR,
            Icon::Paint => &PAINT,
            Icon::Snake => &SNAKE,
            Icon::About => &ABOUT,
            Icon::App => &APP,
            Icon::Restart => &RESTART,
            Icon::ShutDown => &POWER,
        }
    }
}

/// Every icon, for tests and for sheets of them.
pub const ALL: [Icon; 11] = [
    Icon::Logo,
    Icon::Terminal,
    Icon::Notepad,
    Icon::Files,
    Icon::Calculator,
    Icon::Paint,
    Icon::Snake,
    Icon::About,
    Icon::App,
    Icon::Restart,
    Icon::ShutDown,
];

/// The number of dots along each side of an icon.
pub const DOTS: i32 = 16;

/// An icon's picture: 16 lines, each of 16 letters (see the top of this file).
type Picture = [&'static str; 16];

/// The colour a letter of a picture stands for (1.3's `icon_palette`), or `None` for a clear
/// dot. `Option` is Rust's "maybe a value": `Some(colour)` or `None`.
fn colour_of(letter: u8) -> Option<Colour> {
    let rgb = match letter {
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
    };
    Some(Colour::solid(rgb))
}

/// Draws `icon` with its top left corner at (`x`, `y`), each dot a square of `size` x `size`
/// logical pixels. A `size` of zero or less draws nothing; any position is fine (what falls
/// outside the canvas is not drawn).
pub fn draw(canvas: &mut Canvas<'_>, icon: Icon, x: i32, y: i32, size: i32) {
    if size <= 0 {
        return;
    }
    // The work is bounded by the picture (256 dots), whatever `size` or the position is; the
    // canvas cuts what is outside it. `saturating_*` stops at the biggest number instead of
    // wrapping round, so a silly position cannot put a dot somewhere it should not be.
    for (row, line) in icon.picture().iter().enumerate() {
        let top = y.saturating_add((row as i32).saturating_mul(size));
        let bytes = line.as_bytes();
        let mut column = 0;
        while column < bytes.len() {
            let Some(colour) = colour_of(bytes[column]) else {
                column += 1;
                continue;
            };
            // A run of dots of one colour is one rectangle: fewer, longer fills.
            let start = column;
            while column < bytes.len() && bytes[column] == bytes[start] {
                column += 1;
            }
            let left = x.saturating_add((start as i32).saturating_mul(size));
            let width = ((column - start) as i32).saturating_mul(size);
            canvas.fill_rect(Rect::new(left, top, width, size), colour);
        }
    }
}

/// The plain window used for apps with no icon of their own (new in 2.0, drawn in 1.3's style:
/// a dark outline, a blue title bar, a white page with a few grey lines of text).
const APP: Picture = [
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

// ---------------------------------------------------------------- 1.3's pictures
//
// Copied from `classic/tools/mkassets.py` letter for letter.

/// 1.3's `terminal` picture.
const TERMINAL: Picture = [
    "................",
    ".ssssssssssssss.",
    ".sGGGGGGGGGGGGs.",
    ".skkkkkkkkkkkks.",
    ".skekkkkkkkkkks.",
    ".skkekkkkkkkkks.",
    ".skkkekkkkkkkks.",
    ".skkekkkkkkkkks.",
    ".skekkkkeeeekks.",
    ".skkkkkkkkkkkks.",
    ".skkkkkkkkkkkks.",
    ".skkkkkkkkkkkks.",
    ".ssssssssssssss.",
    "......sGGs......",
    "....ssssssss....",
    "................",
];

/// 1.3's `notepad` picture.
const NOTEPAD: Picture = [
    "..gggggggggg....",
    "..gwwwwwwwwgg...",
    "..gwwwwwwwwgwg..",
    "..gwBBBBBwwgggg.",
    "..gwwwwwwwwwwwg.",
    "..gwBBBBBBBBBwg.",
    "..gwwwwwwwwwwwg.",
    "..gwBBBBBBBBBwg.",
    "..gwwwwwwwwwwwg.",
    "..gwBBBBBBBBBwg.",
    "..gwwwwwwwwwwwg.",
    "..gwBBBBBBwwwwg.",
    "..gwwwwwwwwwwwg.",
    "..gwwwwwwwwwwwg.",
    "..ggggggggggggg.",
    "................",
];

/// 1.3's `files` picture.
const FILES: Picture = [
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

/// 1.3's `calculator` picture.
const CALCULATOR: Picture = [
    "..ssssssssssss..",
    "..sGGGGGGGGGGs..",
    "..sGeeeeeeeeGs..",
    "..sGeeeeeeeeGs..",
    "..sGGGGGGGGGGs..",
    "..sGggGggGooGs..",
    "..sGggGggGooGs..",
    "..sGGGGGGGGGGs..",
    "..sGggGggGooGs..",
    "..sGggGggGooGs..",
    "..sGGGGGGGGGGs..",
    "..sGggGggGooGs..",
    "..sGggGggGooGs..",
    "..sGGGGGGGGGGs..",
    "..ssssssssssss..",
    "................",
];

/// 1.3's `paint` picture.
const PAINT: Picture = [
    "................",
    "....cccccccc....",
    "..cccccccccccc..",
    ".ccrrcccccbbccc.",
    ".ccrrcccccbbccc.",
    ".ccccccccccccc..",
    ".cyyccccccccc...",
    ".cyycccc........",
    ".ccccccc......n.",
    ".cccccccccc..n..",
    ".ceecccccccnn...",
    ".ceeccppccnnc...",
    "..cccppccnnc....",
    "...cccccckc.....",
    "........k.......",
    "................",
];

/// 1.3's `snake` picture.
const SNAKE: Picture = [
    "................",
    ".....eeeeeeek...",
    "....eEEEEEEEee..",
    "....eEeeeeeee...",
    "....eEe.........",
    "....eEeeeeeee...",
    ".....eEEEEEEEe..",
    "......eeeeeeEe..",
    "............eEe.",
    "....rr......eEe.",
    "...rrrr..eeeeEe.",
    "...rrrr.eEEEEe..",
    "....rr...eeee...",
    "................",
    "................",
    "................",
];

/// 1.3's `about` picture.
const ABOUT: Picture = [
    "................",
    ".....bbbbbb.....",
    "...bbbbbbbbbb...",
    "..bbbbbwwbbbbb..",
    "..bbbbbwwbbbbb..",
    ".bbbbbbbbbbbbbb.",
    ".bbbbbwwwbbbbbb.",
    ".bbbbbbwwbbbbbb.",
    ".bbbbbbwwbbbbbb.",
    ".bbbbbbwwbbbbbb.",
    ".bbbbbbwwbbbbbb.",
    "..bbbbwwwwbbbb..",
    "..bbbbbbbbbbbb..",
    "...bbbbbbbbbb...",
    ".....bbbbbb.....",
    "................",
];

/// 1.3's `logo` picture.
const LOGO: Picture = [
    "................",
    "............OO..",
    "...........OooO.",
    "...........OooO.",
    "............OO..",
    "................",
    "....wwwwwwwww...",
    "..l.lmmmmmmmml..",
    ".lmmmmmmmmmmmm..",
    ".mmmmmmmmmmmmml.",
    "mmmmmmmmmmmmmmlm",
    "mmmmmmmmmmmmmmmm",
    "MMMMMMMMMMMMMMMM",
    "bbbbbbbbbOObbbbb",
    "................",
    "................",
];

/// 1.3's `power` picture.
const POWER: Picture = [
    "................",
    "................",
    ".......rr.......",
    "....r..rr..r....",
    "...rr..rr..rr...",
    "..rr...rr...rr..",
    "..r....rr....r..",
    "..r..........r..",
    "..r..........r..",
    "..rr........rr..",
    "...rr......rr...",
    "....rrrrrrrr....",
    "......rrrr......",
    "................",
    "................",
    "................",
];

/// 1.3's `restart` picture.
const RESTART: Picture = [
    "................",
    "................",
    "......bbbbb.....",
    "....bb.....bbb..",
    "...b.........b..",
    "..b.......bbbb..",
    "..b.............",
    "..b.............",
    "..b..........b..",
    "..b..........b..",
    "...b........b...",
    "....bb....bb....",
    "......bbbb......",
    "................",
    "................",
    "................",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_picture_is_16_by_16_of_known_letters() {
        for icon in ALL {
            let picture = icon.picture();
            for line in picture {
                assert_eq!(line.len(), 16, "{icon:?}");
                for letter in line.bytes() {
                    assert!(letter == b'.' || colour_of(letter).is_some(), "{icon:?}: {letter}");
                }
            }
        }
    }

    #[test]
    fn names() {
        assert_eq!(Icon::from_name("terminal"), Icon::Terminal);
        assert_eq!(Icon::from_name("calculator"), Icon::Calculator);
        assert_eq!(Icon::from_name("about"), Icon::About);
        assert_eq!(Icon::from_name("Terminal"), Icon::App);
        assert_eq!(Icon::from_name(""), Icon::App);
        assert_eq!(Icon::from_name("logo"), Icon::App);
    }
}
