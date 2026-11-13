// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop's screen arrangement, kept in its own folder as `screens.txt`
//! (docs/design/0024-the-session-desktop.md, section 6), in the same plain style as manifests:
//!
//! ```text
//! # Where the screens go (the desktop keeps this)
//! screen "Virtual-1" at 0,0 scale 100 main
//! screen "Virtual-2" at 1280,0 scale 200
//! screen "Virtual-3" mirrors "Virtual-1"
//! pointer 100
//! tablet "QEMU Virtio Tablet" "Virtual-1"
//! ```
//!
//! Each `screen` line is for one screen *identity* (what the display server calls a screen's
//! description of itself and its connector: the same screen plugged in again has the same
//! one), so a screen that isn't there is simply skipped, and comes back to its place when it
//! is. `pointer` is the relative pointer's speed in hundredths, and `tablet` ties a tablet (by
//! its name) to a screen (by its identity).
//!
//! The file is the user's own (in their app folder), so it is read strictly but nothing in it
//! is trusted: the display server checks every arrangement it is asked for. A file that can't
//! be read is ignored as a whole, with the line that was wrong.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_window::ScreenPlace;
use pbr_manifest::lines::{self, Limits, ReadError};

/// The largest file, and the longest line.
pub const MAX_SIZE: usize = 8192;
/// See [`MAX_SIZE`].
pub const MAX_LINE: usize = 300;
/// The most screens and tablets it keeps.
pub const MAX_SCREENS: usize = 16;
/// See [`MAX_SCREENS`].
pub const MAX_TABLETS: usize = 8;
/// The longest identity or tablet name (as `pbr.window` holds them).
pub const MAX_NAME: usize = 64;

const LIMITS: Limits = Limits { max_size: MAX_SIZE, max_line: MAX_LINE, quotes: true };

/// One screen's place, by identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The screen's identity (`ScreenPlace.identity`).
    pub identity: String,
    /// Its top-left corner in the space, in logical pixels.
    pub x: i32,
    /// See `x`.
    pub y: i32,
    /// Hundredths: 100 or 200.
    pub scale: u32,
    /// Degrees clockwise.
    pub rotation: u32,
    /// The identity of the screen it mirrors, if it does.
    pub mirrors: Option<String>,
    /// It has the taskbar, and new windows open on it.
    pub main: bool,
}

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// One for each screen identity, in the file's order.
    pub screens: Vec<Saved>,
    /// The pointer's speed, in hundredths, if set.
    pub speed: Option<u32>,
    /// (tablet name, screen identity).
    pub tablets: Vec<(String, String)>,
}

/// What was wrong with the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file itself: too long, not text, a broken quote.
    Read(ReadError),
    /// A line that isn't `screen`, `pointer` or `tablet`.
    UnknownKey,
    /// A value that is missing or malformed.
    BadValue,
    /// The same screen or tablet twice, or `pointer` twice.
    Repeated,
    /// More screens or tablets than it keeps.
    TooMany,
}

impl From<ReadError> for Error {
    fn from(e: ReadError) -> Error {
        Error::Read(e)
    }
}

/// A name in quotes, within [`MAX_NAME`].
fn name(w: Option<&lines::Word<'_>>) -> Result<String, Error> {
    match w {
        Some(w) if w.quoted && !w.text.is_empty() && w.text.len() <= MAX_NAME => {
            Ok(String::from(w.text))
        }
        _ => Err(Error::BadValue),
    }
}

fn number<T: core::str::FromStr>(w: Option<&lines::Word<'_>>) -> Result<T, Error> {
    match w {
        Some(w) if !w.quoted => w.text.parse().map_err(|_| Error::BadValue),
        _ => Err(Error::BadValue),
    }
}

impl Arrangement {
    /// Reads a `screens.txt`. On an error, also says which line (0 for the whole file).
    pub fn parse(text: &[u8]) -> Result<Arrangement, (Error, usize)> {
        let mut a = Arrangement::default();
        for line in lines::read(text, LIMITS).map_err(|e| (Error::Read(e), 0))? {
            let at = |e| (e, line.number);
            let words = line.words().map_err(|e| at(Error::Read(e)))?;
            match line.key {
                "screen" => {
                    let identity = name(words.first()).map_err(at)?;
                    if a.screens.iter().any(|s| s.identity == identity) {
                        return Err(at(Error::Repeated));
                    }
                    if a.screens.len() >= MAX_SCREENS {
                        return Err(at(Error::TooMany));
                    }
                    let mut s = Saved {
                        identity,
                        x: 0,
                        y: 0,
                        scale: 100,
                        rotation: 0,
                        mirrors: None,
                        main: false,
                    };
                    let mut rest = words[1..].iter();
                    while let Some(w) = rest.next() {
                        match (w.quoted, w.text) {
                            (false, "at") => {
                                let p: &str =
                                    rest.next().filter(|w| !w.quoted).map_or("", |w| w.text);
                                let (x, y) = p.split_once(',').ok_or(at(Error::BadValue))?;
                                s.x = x.parse().map_err(|_| at(Error::BadValue))?;
                                s.y = y.parse().map_err(|_| at(Error::BadValue))?;
                            }
                            (false, "scale") => s.scale = number(rest.next()).map_err(at)?,
                            (false, "rotation") => s.rotation = number(rest.next()).map_err(at)?,
                            (false, "mirrors") => s.mirrors = Some(name(rest.next()).map_err(at)?),
                            (false, "main") => s.main = true,
                            _ => return Err(at(Error::BadValue)),
                        }
                    }
                    a.screens.push(s);
                }
                "pointer" => {
                    let [w] = words.as_slice() else { return Err(at(Error::BadValue)) };
                    if a.speed.is_some() {
                        return Err(at(Error::Repeated));
                    }
                    a.speed = Some(number(Some(w)).map_err(at)?);
                }
                "tablet" => {
                    let [t, s] = words.as_slice() else { return Err(at(Error::BadValue)) };
                    let (t, s) = (name(Some(t)).map_err(at)?, name(Some(s)).map_err(at)?);
                    if a.tablets.iter().any(|(n, _)| *n == t) {
                        return Err(at(Error::Repeated));
                    }
                    if a.tablets.len() >= MAX_TABLETS {
                        return Err(at(Error::TooMany));
                    }
                    a.tablets.push((t, s));
                }
                _ => return Err(at(Error::UnknownKey)),
            }
        }
        Ok(a)
    }

    /// The file's text. (Names can't hold `"`: the display server's identities don't.)
    pub fn to_text(&self) -> String {
        let mut out = String::from("# Where the screens go (the desktop keeps this file)\n");
        for s in &self.screens {
            out.push_str(&format!("screen \"{}\"", s.identity));
            match &s.mirrors {
                Some(m) => out.push_str(&format!(" mirrors \"{m}\"")),
                None => {
                    out.push_str(&format!(" at {},{} scale {}", s.x, s.y, s.scale));
                    if s.rotation != 0 {
                        out.push_str(&format!(" rotation {}", s.rotation));
                    }
                    if s.main {
                        out.push_str(" main");
                    }
                }
            }
            out.push('\n');
        }
        if let Some(speed) = self.speed {
            out.push_str(&format!("pointer {speed}\n"));
        }
        for (t, s) in &self.tablets {
            out.push_str(&format!("tablet \"{t}\" \"{s}\"\n"));
        }
        out
    }

    /// The arrangement to ask for, given the screens there are now: each screen this file has
    /// a line for goes where the line says (a screen it mirrors is found by identity; one that
    /// isn't there leaves it in the space), and the rest stay where they are. `None` if the
    /// file says nothing about any of them, or nothing would change.
    pub fn apply(&self, now: &[ScreenPlace]) -> Option<Vec<ScreenPlace>> {
        let mut out: Vec<ScreenPlace> = now.to_vec();
        let mut any = false;
        for p in &mut out {
            let Some(s) = self.screens.iter().find(|s| s.identity == p.identity) else {
                continue;
            };
            any = true;
            let mirrors = s
                .mirrors
                .as_ref()
                .and_then(|m| now.iter().find(|o| o.identity == *m && o.screen != p.screen))
                .map_or(0, |o| o.screen);
            p.mirrors = mirrors;
            if mirrors == 0 {
                p.x = s.x;
                p.y = s.y;
                p.scale = s.scale;
                p.rotation = s.rotation;
                p.main = s.main;
            } else {
                p.main = false;
            }
        }
        (any && out != now).then_some(out)
    }

    /// The file for the arrangement there is now (what the Screens dialog saves).
    pub fn from_places(places: &[ScreenPlace]) -> Arrangement {
        let identity = |n: u32| places.iter().find(|p| p.screen == n).map(|p| p.identity.clone());
        Arrangement {
            screens: places
                .iter()
                .take(MAX_SCREENS)
                .map(|p| Saved {
                    identity: p.identity.clone(),
                    x: p.x,
                    y: p.y,
                    scale: p.scale,
                    rotation: p.rotation,
                    mirrors: if p.mirrors == 0 { None } else { identity(p.mirrors) },
                    main: p.main,
                })
                .collect(),
            speed: None,
            tablets: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pbr_interfaces::pbr_window::Size;

    fn place(screen: u32, identity: &str, x: i32, scale: u32) -> ScreenPlace {
        ScreenPlace {
            screen,
            identity: String::from(identity),
            x,
            y: 0,
            scale,
            rotation: 0,
            mirrors: 0,
            main: screen == 1,
            size: Size { width: 1280, height: 800 },
        }
    }

    const FILE: &str = "# test\nscreen \"Virtual-1\" at 0,0 scale 100 main\nscreen \"QEMU Virtual-2\" at 1280,0 scale 200\nscreen \"Virtual-3\" mirrors \"Virtual-1\"\npointer 150\ntablet \"QEMU Virtio Tablet\" \"Virtual-1\"\n";

    #[test]
    fn reads_and_writes() {
        let a = Arrangement::parse(FILE.as_bytes()).unwrap();
        assert_eq!(a.screens.len(), 3);
        assert_eq!(a.screens[1].identity, "QEMU Virtual-2");
        assert_eq!((a.screens[1].x, a.screens[1].scale, a.screens[1].main), (1280, 200, false));
        assert!(a.screens[0].main);
        assert_eq!(a.screens[2].mirrors.as_deref(), Some("Virtual-1"));
        assert_eq!(a.speed, Some(150));
        assert_eq!(a.tablets, alloc::vec![("QEMU Virtio Tablet".into(), "Virtual-1".into())]);
        // Written out and read back, the same.
        assert_eq!(Arrangement::parse(a.to_text().as_bytes()).unwrap(), a);
        assert_eq!(Arrangement::parse(b""), Ok(Arrangement::default()));
    }

    #[test]
    fn refuses_the_wrong() {
        let p = |s: &str| Arrangement::parse(s.as_bytes()).map(|_| ());
        assert_eq!(p("screens \"a\""), Err((Error::UnknownKey, 1)));
        assert_eq!(p("screen a at 0,0"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" at 0"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" at x,0"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" scale"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" scale -1"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" mirrors b"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\" upside-down"), Err((Error::BadValue, 1)));
        assert_eq!(p("screen \"a\"\nscreen \"a\""), Err((Error::Repeated, 2)));
        assert_eq!(p("pointer 1\npointer 2"), Err((Error::Repeated, 2)));
        assert_eq!(p("pointer"), Err((Error::BadValue, 1)));
        assert_eq!(p("tablet \"t\""), Err((Error::BadValue, 1)));
        assert_eq!(p("tablet \"t\" \"a\"\ntablet \"t\" \"b\""), Err((Error::Repeated, 2)));
        let long = alloc::format!("screen \"{}\"", "x".repeat(65));
        assert_eq!(p(&long), Err((Error::BadValue, 1)));
        let many: String = (0..17).map(|i| alloc::format!("screen \"s{i}\"\n")).collect();
        assert_eq!(p(&many), Err((Error::TooMany, 17)));
    }

    #[test]
    fn applies_to_the_screens_there_are() {
        let a = Arrangement::parse(FILE.as_bytes()).unwrap();
        // The first two are there (the second numbered 4 this time); the third isn't.
        let now = [place(1, "Virtual-1", 0, 100), place(4, "QEMU Virtual-2", 1280, 100)];
        let new = a.apply(&now).unwrap();
        assert_eq!((new[1].screen, new[1].x, new[1].scale), (4, 1280, 200));
        assert!(new[0].main && !new[1].main);
        // A screen the file doesn't know stays where it is; with nothing known, nothing to do.
        let other = [place(1, "Other", 0, 100)];
        assert_eq!(a.apply(&other), None);
        // Already arranged: nothing to do.
        assert_eq!(a.apply(&new), None);
        // Mirroring, by identity.
        let three = [
            place(1, "Virtual-1", 0, 100),
            place(2, "QEMU Virtual-2", 1280, 200),
            place(3, "Virtual-3", 1920, 100),
        ];
        let new = a.apply(&three).unwrap();
        assert_eq!((new[2].mirrors, new[2].main), (1, false));
        // The file the arrangement makes says the same.
        let again = Arrangement::from_places(&new);
        assert_eq!(again.apply(&new), None);
        assert_eq!(again.screens[2].mirrors.as_deref(), Some("Virtual-1"));
    }

    #[test]
    fn random_damage_never_panics() {
        let mut x = 7u32;
        for _ in 0..5000 {
            let mut b = FILE.as_bytes().to_vec();
            for _ in 0..3 {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                let i = (x >> 8) as usize % b.len();
                b[i] = (x >> 20) as u8;
            }
            if let Ok(a) = Arrangement::parse(&b) {
                assert!(a.screens.len() <= MAX_SCREENS);
            }
        }
    }
}
