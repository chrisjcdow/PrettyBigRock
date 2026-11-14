// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The places a person can jump to from the side of the picker and Files: Home, its usual
//! folders, and (for an administrator) the volumes.

use crate::kind::{self, Picture};
use crate::path;
use alloc::string::String;
use alloc::vec::Vec;

/// What a place is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// The user's own folder.
    Home,
    /// Home's Documents.
    Documents,
    /// Home's Pictures.
    Pictures,
    /// Home's Music.
    Music,
    /// Home's Videos.
    Videos,
    /// Home's Downloads.
    Downloads,
    /// A disk or partition, which only an administrator sees (under `/Volumes`).
    Volume,
}

/// One place in the list at the side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// What kind it is (which decides its icon).
    pub kind: PlaceKind,
    /// What the person reads.
    pub title: String,
    /// Where it is.
    pub path: String,
}

impl PlaceKind {
    /// The fixed path of the place, or `None` for a volume (each has its own).
    pub fn path(self) -> Option<&'static str> {
        match self {
            PlaceKind::Home => Some("/Home"),
            PlaceKind::Documents => Some("/Home/Documents"),
            PlaceKind::Pictures => Some("/Home/Pictures"),
            PlaceKind::Music => Some("/Home/Music"),
            PlaceKind::Videos => Some("/Home/Videos"),
            PlaceKind::Downloads => Some("/Home/Downloads"),
            PlaceKind::Volume => None,
        }
    }

    /// The title (for a volume, the general word: a volume's own title is its name).
    pub fn title(self) -> &'static str {
        match self {
            PlaceKind::Home => "Home",
            PlaceKind::Documents => "Documents",
            PlaceKind::Pictures => "Pictures",
            PlaceKind::Music => "Music",
            PlaceKind::Videos => "Videos",
            PlaceKind::Downloads => "Downloads",
            PlaceKind::Volume => "Volume",
        }
    }

    /// The icon, 16 x 16 dots of `0xAARRGGBB` as for [`crate::Kind::icon`].
    pub fn icon(self) -> &'static [u32; 256] {
        match self {
            PlaceKind::Home => &HOME_ICON,
            PlaceKind::Documents => &kind::TEXT_ICON,
            PlaceKind::Pictures => &kind::PICTURE_ICON,
            PlaceKind::Music => &kind::MUSIC_ICON,
            PlaceKind::Videos => &kind::VIDEO_ICON,
            PlaceKind::Downloads => &DOWNLOADS_ICON,
            PlaceKind::Volume => &VOLUME_ICON,
        }
    }
}

/// The six usual places, in the order they are listed.
const USUAL: [PlaceKind; 6] = [
    PlaceKind::Home,
    PlaceKind::Documents,
    PlaceKind::Pictures,
    PlaceKind::Music,
    PlaceKind::Videos,
    PlaceKind::Downloads,
];

/// The places to list: the six usual ones, then, for an administrator only, a place for each
/// of the `volumes` (their names, in the order given; a name that is not a valid name is
/// skipped, because it could not be a folder under `/Volumes`).
pub fn places(admin: bool, volumes: &[&str]) -> Vec<Place> {
    let mut out = Vec::new();
    for kind in USUAL {
        if let Some(fixed) = kind.path() {
            out.push(Place { kind, title: String::from(kind.title()), path: String::from(fixed) });
        }
    }
    if admin {
        for name in volumes.iter().filter(|name| path::check_name(name).is_ok()) {
            out.push(Place {
                kind: PlaceKind::Volume,
                title: String::from(*name),
                path: path::join("/Volumes", name),
            });
        }
    }
    out
}

/// The place that holds `path` (or is it): the one with the longest path of those that contain
/// it, so `/Home/Documents/x` is in Documents rather than Home. When two places have the same
/// path, the first. `None` if no place holds it.
pub fn place_of(path: &str, places: &[Place]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (index, place) in places.iter().enumerate() {
        if !path::inside(&place.path, path) {
            continue;
        }
        let length = place.path.trim_end_matches('/').len();
        if best.is_none_or(|(_, longest)| length > longest) {
            best = Some((index, length));
        }
    }
    best.map(|(index, _)| index)
}

/// A house: a red roof, cream walls, a brown door and yellow windows.
const HOME: Picture = [
    "................",
    ".......rr.......",
    "......rrrr......",
    ".....rrrrrr.....",
    "....rrrrrrrr....",
    "...rrrrrrrrrr...",
    "..rrrrrrrrrrrr..",
    ".rrrrrrrrrrrrrr.",
    "...nccccccccn...",
    "...nyyccccyyn...",
    "...nyycnncyyn...",
    "...ncccnncccn...",
    "...ncccnncccn...",
    "...ncccnncccn...",
    "...nnnnnnnnnn...",
    "................",
];

/// A blue arrow pointing down into a grey tray.
const DOWNLOADS: Picture = [
    "................",
    ".......bb.......",
    ".......bb.......",
    ".......bb.......",
    ".......bb.......",
    ".......bb.......",
    ".......bb.......",
    "....bbbbbbbb....",
    ".....bbbbbb.....",
    "......bbbb......",
    ".......bb.......",
    "................",
    "..G..........G..",
    "..G..........G..",
    "..GGGGGGGGGGGG..",
    "................",
];

/// A grey disk box with a slot and a green light.
const VOLUME: Picture = [
    "................",
    "................",
    "................",
    "..ssssssssssss..",
    "..sggggggggggs..",
    "..sggggggggggs..",
    "..sggggggggggs..",
    "..sGGGGGGGGGGs..",
    "..sggggggggggs..",
    "..sggggggggegs..",
    "..sggggggggggs..",
    "..ssssssssssss..",
    "................",
    "................",
    "................",
    "................",
];

static HOME_ICON: [u32; 256] = kind::build(&HOME);
static DOWNLOADS_ICON: [u32; 256] = kind::build(&DOWNLOADS);
static VOLUME_ICON: [u32; 256] = kind::build(&VOLUME);
