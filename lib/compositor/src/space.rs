// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The screens, and the one space they make together (docs/design/0022-display-server.md,
//! section 2).
//!
//! Every screen of every display device has a place in one coordinate space, measured in
//! *logical* pixels: a window 400 logical pixels wide is 400 device pixels on a screen at 1x
//! and 800 on one at 2x, so it looks the same size on both. Each screen has a top-left corner
//! in the space, a size (its mode divided by its scale, and turned if the screen is), a scale
//! in hundredths and a rotation.
//!
//! A screen that *mirrors* another isn't part of the space: it shows the other's part of it,
//! scaled to fit.
//!
//! The display server gets the screens from the drivers ([`Output`]); the desktop app decides
//! where they go ([`Space::arrange`]); until it does, or for a screen it doesn't know, they are
//! placed left to right in device and connector order, tops aligned, the first being the main
//! one, and a screen of at least 180 dots per inch is 2x.

use alloc::string::String;
use alloc::vec::Vec;
use pbr_draw::Rect;

/// The scales M6 accepts: 1x to 8x in whole steps (`lib/draw`'s canvas goes up to 8).
pub const SCALES: core::ops::RangeInclusive<u32> = 100..=800;
/// Dots per inch from which a screen is 2x by default.
pub const HIGH_DPI: u64 = 180;

/// A screen as its display driver describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The device manager's number for the display device.
    pub device: u32,
    /// Which of the device's screens.
    pub index: u32,
    /// The connector's name ("Virtual-1", "Firmware").
    pub connector: String,
    /// The screen's description of itself (maker and serial number), or empty.
    pub identity: String,
    /// The mode it shows: its width in device pixels.
    pub width: u32,
    /// Its height in device pixels.
    pub height: u32,
    /// Its picture's width in millimetres, 0 if unknown.
    pub width_mm: u32,
    /// Its picture's height in millimetres, 0 if unknown.
    pub height_mm: u32,
}

impl Output {
    /// What an arrangement remembers it by: its identity and connector together (QEMU's screens
    /// all have the same identity, so the connector tells them apart).
    pub fn key(&self) -> String {
        let mut k = String::from(self.identity.trim());
        if !k.is_empty() {
            k.push(' ');
        }
        k.push_str(&self.connector);
        // At most 64 bytes, as `ScreenPlace.identity` holds, cut at a character boundary.
        let mut end = k.len().min(64);
        while !k.is_char_boundary(end) {
            end -= 1;
        }
        k.truncate(end);
        k
    }

    /// The scale to use when nobody has said: 2x from [`HIGH_DPI`] dots per inch, from the
    /// width (25.4 millimetres to the inch).
    pub fn default_scale(&self) -> u32 {
        if self.width_mm == 0 {
            return 100;
        }
        let dpi = u64::from(self.width) * 254 / (u64::from(self.width_mm) * 10);
        if dpi >= HIGH_DPI { 200 } else { 100 }
    }
}

/// Where a screen is, and how it shows the space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// Its top left corner in the space, in logical pixels.
    pub x: i32,
    /// The same, down.
    pub y: i32,
    /// Hundredths: 100 is 1x.
    pub scale: u32,
    /// Degrees clockwise: 0, 90, 180 or 270.
    pub rotation: u32,
    /// The number of the screen this one shows the same as, or 0.
    pub mirrors: u32,
    /// Has the panel, and where new windows open.
    pub main: bool,
}

/// One screen in the space.
#[derive(Clone, Debug)]
pub struct Screen {
    /// The display server's number for it, from 1; the same while it is there.
    pub number: u32,
    /// What its driver says it is.
    pub output: Output,
    /// Where it is in the space.
    pub place: Place,
}

impl Screen {
    /// The scale as a whole number of device pixels to a logical pixel.
    pub fn factor(&self) -> i32 {
        (self.place.scale / 100).clamp(1, 8) as i32
    }

    /// The size of the picture the driver shows, in device pixels: its mode.
    pub fn device_size(&self) -> (i32, i32) {
        (
            self.output.width.min(i32::MAX as u32) as i32,
            self.output.height.min(i32::MAX as u32) as i32,
        )
    }

    /// Whether the screen is turned a quarter: its logical width is then its height.
    pub fn sideways(&self) -> bool {
        matches!(self.place.rotation, 90 | 270)
    }

    /// Its size in logical pixels: its mode, turned, divided by its scale.
    pub fn logical_size(&self) -> (i32, i32) {
        let (w, h) = self.device_size();
        let (w, h) = if self.sideways() { (h, w) } else { (w, h) };
        (w / self.factor(), h / self.factor())
    }

    /// Its part of the space. (A mirroring screen's own place means nothing: see
    /// [`Space::shows`].)
    pub fn rect(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect::new(self.place.x, self.place.y, w, h)
    }

    /// Whether it is part of the space (doesn't mirror another).
    pub fn in_space(&self) -> bool {
        self.place.mirrors == 0
    }
}

/// What [`Space::update`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Screens that went: (number, its part of the space).
    pub gone: Vec<(u32, Rect)>,
    /// Screens that came.
    pub came: Vec<u32>,
    /// Anything changed at all (including a screen's mode).
    pub any: bool,
}

/// A place remembered for a screen that went, by [`Output::key`].
#[derive(Clone, Debug)]
struct Remembered {
    key: String,
    place: Place,
}

/// Why an arrangement was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangeError {
    /// It names a screen that isn't there, or one twice.
    NoSuchScreen,
    /// A scale, rotation or mirror that can't be used.
    BadArgument,
}

/// Every screen there is.
#[derive(Clone, Debug, Default)]
pub struct Space {
    screens: Vec<Screen>,
    next: u32,
    remembered: Vec<Remembered>,
}

impl Space {
    /// No screens.
    pub fn new() -> Space {
        Space { screens: Vec::new(), next: 1, remembered: Vec::new() }
    }

    /// The screens, in their numbers' order.
    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    /// Screen `number`.
    pub fn screen(&self, number: u32) -> Option<&Screen> {
        self.screens.iter().find(|s| s.number == number)
    }

    /// The screens that are part of the space (not mirroring another).
    pub fn in_space(&self) -> impl Iterator<Item = &Screen> {
        self.screens.iter().filter(|s| s.in_space())
    }

    /// The main screen: the one with the panel, where new windows open.
    pub fn main(&self) -> Option<&Screen> {
        self.in_space().find(|s| s.place.main).or_else(|| self.in_space().next())
    }

    /// The screen whose part of the space `number` shows: itself, or the one it mirrors.
    pub fn shows(&self, number: u32) -> Option<&Screen> {
        let s = self.screen(number)?;
        if s.in_space() { Some(s) } else { self.screen(s.place.mirrors).filter(|m| m.in_space()) }
    }

    /// The largest width and height any screen has, in device pixels: no window's buffers may
    /// be bigger (note 0022, section 3).
    pub fn largest_device(&self) -> (i32, i32) {
        self.screens.iter().fold((0, 0), |(w, h), s| {
            let (sw, sh) = s.device_size();
            let (sw, sh) = if s.sideways() { (sh, sw) } else { (sw, sh) };
            (w.max(sw), h.max(sh))
        })
    }

    /// Takes in what the drivers say the screens are now. Screens are known by device and
    /// index; a new one gets the place it had before (if it went and came back) or the
    /// default one, and one that went has its place remembered.
    pub fn update(&mut self, outputs: &[Output]) -> Changes {
        let mut changes = Changes::default();
        // The screens that went.
        let mut i = 0;
        while i < self.screens.len() {
            let s = &self.screens[i];
            if outputs.iter().any(|o| (o.device, o.index) == (s.output.device, s.output.index)) {
                i += 1;
                continue;
            }
            let gone = self.screens.remove(i);
            changes.gone.push((gone.number, gone.rect()));
            self.remember(&gone);
            changes.any = true;
        }
        // Anything mirroring a screen that went shows the space again, next to the rest.
        let gone: Vec<u32> = changes.gone.iter().map(|g| g.0).collect();
        for k in 0..self.screens.len() {
            if gone.contains(&self.screens[k].place.mirrors) {
                self.screens[k].place.mirrors = 0;
                let x = self.right_edge(Some(self.screens[k].number));
                self.screens[k].place.x = x;
                self.screens[k].place.y = 0;
            }
        }
        // Changed modes, and the new screens in device and connector order.
        let mut sorted: Vec<&Output> = outputs.iter().collect();
        sorted.sort_by_key(|o| (o.device, o.index));
        for o in sorted {
            if let Some(s) = self
                .screens
                .iter_mut()
                .find(|s| (s.output.device, s.output.index) == (o.device, o.index))
            {
                if s.output != *o {
                    s.output = o.clone();
                    changes.any = true;
                }
                continue;
            }
            let place = match self.recall(&o.key()) {
                Some(p) => p,
                None => Place {
                    x: self.right_edge(None),
                    y: 0,
                    scale: o.default_scale(),
                    rotation: 0,
                    mirrors: 0,
                    main: false,
                },
            };
            let number = self.next;
            self.next = self.next.wrapping_add(1).max(1);
            self.screens.push(Screen { number, output: o.clone(), place });
            changes.came.push(number);
            changes.any = true;
        }
        self.tidy();
        changes
    }

    /// The right edge of the space's screens (0 if none), leaving out `except`.
    fn right_edge(&self, except: Option<u32>) -> i32 {
        self.in_space()
            .filter(|s| Some(s.number) != except)
            .map(|s| s.rect().right().min(i64::from(i32::MAX / 2)) as i32)
            .max()
            .unwrap_or(0)
    }

    fn remember(&mut self, s: &Screen) {
        let key = s.output.key();
        self.remembered.retain(|r| r.key != key);
        // A few are enough: screens come and go one or two at a time.
        if self.remembered.len() >= 16 {
            self.remembered.remove(0);
        }
        self.remembered.push(Remembered { key, place: s.place.clone() });
    }

    fn recall(&mut self, key: &str) -> Option<Place> {
        let i = self.remembered.iter().position(|r| r.key == key)?;
        let mut place = self.remembered.remove(i).place;
        // What it mirrored may have gone meanwhile.
        if place.mirrors != 0 && !self.screen(place.mirrors).is_some_and(|m| m.in_space()) {
            place.mirrors = 0;
            place.x = self.right_edge(None);
            place.y = 0;
        }
        Some(place)
    }

    /// Exactly one main screen, if any screen is in the space: the one marked, or the first.
    fn tidy(&mut self) {
        let main = self
            .screens
            .iter()
            .filter(|s| s.in_space())
            .find(|s| s.place.main)
            .or_else(|| self.screens.iter().find(|s| s.in_space()))
            .map(|s| s.number);
        for s in &mut self.screens {
            s.place.main = Some(s.number) == main;
        }
    }

    /// The desktop's arrangement: a place for each screen it names (screens it doesn't name
    /// keep theirs). Refused whole if any of it is wrong.
    pub fn arrange(&mut self, places: &[(u32, Place)]) -> Result<(), ArrangeError> {
        let mut new: Vec<Screen> = self.screens.clone();
        for (k, (number, place)) in places.iter().enumerate() {
            if places[..k].iter().any(|(n, _)| n == number) {
                return Err(ArrangeError::NoSuchScreen);
            }
            let s =
                new.iter_mut().find(|s| s.number == *number).ok_or(ArrangeError::NoSuchScreen)?;
            if !SCALES.contains(&place.scale)
                || place.scale % 100 != 0
                || !matches!(place.rotation, 0 | 90 | 180 | 270)
                || place.mirrors == *number
                // Places far out would only make arithmetic on them overflow.
                || place.x.unsigned_abs() > 1 << 24
                || place.y.unsigned_abs() > 1 << 24
            {
                return Err(ArrangeError::BadArgument);
            }
            s.place = place.clone();
        }
        // A screen mirrors one that is itself in the space.
        for s in &new {
            let m = s.place.mirrors;
            if m != 0 && !new.iter().any(|t| t.number == m && t.place.mirrors == 0) {
                return Err(ArrangeError::BadArgument);
            }
        }
        // If more than one says it is the main screen, the first does; if none, the first.
        let mut seen = false;
        for s in &mut new {
            if s.place.mirrors != 0 {
                s.place.main = false;
            } else if s.place.main {
                s.place.main = !seen;
                seen = true;
            }
        }
        self.screens = new;
        self.tidy();
        Ok(())
    }

    /// The screen in the space that holds the most of `r` (by area), if any holds some; for a
    /// rectangle on no screen, the nearest screen.
    pub fn holding_most(&self, r: Rect) -> Option<u32> {
        let mut best: Option<(i64, u32)> = None;
        for s in self.in_space() {
            let a = s.rect().intersect(&r).area();
            if a > 0 && best.is_none_or(|(b, _)| a > b) {
                best = Some((a, s.number));
            }
        }
        best.map(|b| b.1).or_else(|| self.nearest(r.x, r.y))
    }

    /// The screen in the space nearest a point (by the distance to its rectangle).
    pub fn nearest(&self, x: i32, y: i32) -> Option<u32> {
        self.in_space()
            .min_by_key(|s| {
                let r = s.rect();
                let dx = (i64::from(r.x) - i64::from(x)).max(i64::from(x) - (r.right() - 1)).max(0);
                let dy =
                    (i64::from(r.y) - i64::from(y)).max(i64::from(y) - (r.bottom() - 1)).max(0);
                dx * dx + dy * dy
            })
            .map(|s| s.number)
    }

    /// The point nearest (`x`, `y`) that is on some screen: the pointer can't leave the
    /// space. Both are in 1/256 logical pixels.
    pub fn clamp(&self, x: i64, y: i64) -> (i64, i64) {
        let Some(n) = self.nearest((x >> 8) as i32, (y >> 8) as i32) else { return (0, 0) };
        let Some(r) = self.screen(n).map(Screen::rect) else { return (0, 0) };
        if r.is_empty() {
            return (i64::from(r.x) << 8, i64::from(r.y) << 8);
        }
        (
            x.clamp(i64::from(r.x) << 8, (r.right() << 8) - 1),
            y.clamp(i64::from(r.y) << 8, (r.bottom() << 8) - 1),
        )
    }

    /// Where in screen `number`'s part of the space a place on its surface is, given as
    /// fractions 0 to 65535 across and down the device (as a tablet tied to it reports):
    /// in 1/256 logical pixels. The rotation is undone: the device's top right corner is the
    /// space's top left on a screen turned 90 degrees.
    pub fn surface_to_space(&self, number: u32, fx: u32, fy: u32) -> Option<(i64, i64)> {
        let shown = self.shows(number)?;
        let own = self.screen(number)?;
        let (fx, fy) = (i64::from(fx.min(65535)), i64::from(fy.min(65535)));
        // As fractions of the logical picture, across and down.
        let (u, v) = match own.place.rotation {
            90 => (fy, 65535 - fx),
            180 => (65535 - fx, 65535 - fy),
            270 => (65535 - fy, fx),
            _ => (fx, fy),
        };
        let r = shown.rect();
        let x = (i64::from(r.x) << 8) + u * (i64::from(r.width) << 8) / 65536;
        let y = (i64::from(r.y) << 8) + v * (i64::from(r.height) << 8) / 65536;
        Some((x, y))
    }
}
