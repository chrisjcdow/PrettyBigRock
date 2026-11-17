// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the Screens dialog works out, as plain logic with no window in it
//! (docs/design/0024-the-session-desktop.md, section 6, and note 0022 on `arrange` and
//! `pointer`): the user's choices become the list of `ScreenPlace`s that `arrange` takes.
//!
//! A [`Plan`] holds one [`Screen`] for each screen the display server has, in the order the user
//! wants them *left to right*, with what the user has chosen for each: which one is **main**,
//! its **scale** (1x or 2x), its **rotation**, and whether it **mirrors** another screen. It
//! also holds the pointer's speed and which screen each known tablet is tied to.
//!
//! The display server makes the final check of any arrangement (it refuses what it can't use),
//! but the dialog checks first, so that the user is told why in plain words and can't even
//! choose what could never work. The rules, which every change here keeps:
//!
//! - exactly one screen is the main screen, and it doesn't mirror another;
//! - at least one screen doesn't mirror (so, not every screen mirrors);
//! - a screen never mirrors itself, a screen that mirrors another, or a screen that something
//!   else mirrors (the display server allows no chains).
//!
//! Screens that don't mirror are placed side by side in their order, tops aligned, each as wide
//! as its *logical* size: the screen's own size in dots, turned a quarter if it is rotated 90
//! or 270 degrees, divided by its scale. A screen at 2x is half as wide in the space as one at
//! 1x, because each logical pixel is two device dots across.
//!
//! Like the rest of this crate it has nothing to do with the display server itself: the program
//! (`apps/desktop`) makes the calls with what [`Plan::request`] gives it.

use crate::screens::{Arrangement, MAX_SCREENS, MAX_TABLETS};
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_window::{ScreenPlace, Size, TabletTie, WindowError};

/// The slowest and fastest pointer, in hundredths (100 is the usual speed), and how much each
/// press of "slower" or "faster" changes it.
pub const SPEED_MIN: u32 = 50;
/// See [`SPEED_MIN`].
pub const SPEED_MAX: u32 = 300;
/// See [`SPEED_MIN`].
pub const SPEED_STEP: u32 = 25;
/// The speed when nothing has been saved.
pub const SPEED_DEFAULT: u32 = 100;

/// The scales the dialog offers, in hundredths (the display server accepts more).
pub const SCALES: [u32; 2] = [100, 200];
/// The rotations, in degrees clockwise.
pub const ROTATIONS: [u32; 4] = [0, 90, 180, 270];

/// Why a choice isn't allowed, or why an arrangement can't be asked for. `explain` says it in
/// words for the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// There is no such screen (or tablet).
    NoSuchScreen,
    /// A scale the dialog doesn't offer.
    BadScale,
    /// A rotation that isn't 0, 90, 180 or 270.
    BadRotation,
    /// A pointer speed outside [`SPEED_MIN`] to [`SPEED_MAX`].
    BadSpeed,
    /// A screen can't show itself.
    MirrorsItself,
    /// The screen it would show is itself showing another.
    TargetMirrors,
    /// Another screen shows this one, so this one can't show another.
    IsMirrored,
    /// The main screen can't show another.
    IsMain,
    /// A screen that shows another can't be the main screen or have a place in the order.
    Mirrors,
    /// There is no other screen to show.
    OnlyOne,
    /// No screens at all.
    NoScreens,
    /// Every screen shows another, so none is left to have the taskbar.
    EveryScreenMirrors,
    /// None of the screens is the main one.
    NoMain,
    /// More screens than `screens.txt` keeps.
    TooMany,
}

impl Problem {
    /// The reason, for the dialog's text.
    pub fn explain(self) -> &'static str {
        match self {
            Problem::NoSuchScreen => "There is no such screen.",
            Problem::BadScale => "A screen can be 1x or 2x.",
            Problem::BadRotation => "A screen can be turned 0, 90, 180 or 270 degrees.",
            Problem::BadSpeed => "The pointer's speed can be from 50 to 300.",
            Problem::MirrorsItself => "A screen can't show its own picture twice.",
            Problem::TargetMirrors => "That screen shows another screen itself.",
            Problem::IsMirrored => "Another screen shows this one, so this one can't show another.",
            Problem::IsMain => "The main screen can't show another: choose another main screen.",
            Problem::Mirrors => "A screen that shows another has no place in the order.",
            Problem::OnlyOne => "There is no other screen to show.",
            Problem::NoScreens => "There are no screens to arrange.",
            Problem::EveryScreenMirrors => "Not every screen can show another: one must be main.",
            Problem::NoMain => "One screen must be the main one.",
            Problem::TooMany => "There are more screens than can be kept.",
        }
    }
}

impl Problem {
    /// A few words for beside a choice that isn't offered (empty for a reason that wouldn't
    /// make sense there).
    pub fn short(self) -> &'static str {
        match self {
            Problem::IsMain => "it is the main screen",
            Problem::IsMirrored => "another screen shows it",
            Problem::OnlyOne => "there is no other screen",
            _ => "",
        }
    }
}

/// What a refusal from the display server means, for the dialog: it checks every arrangement
/// again, and says only which kind of fault it found.
pub fn refusal(e: &WindowError) -> &'static str {
    match e {
        WindowError::BadArgument => "it can't use that arrangement",
        WindowError::NoSuchWindow => "a screen in it isn't there any more",
        WindowError::NotAllowed => "the desktop isn't allowed to arrange the screens",
        _ => "it didn't say why",
    }
}

/// One screen and what has been chosen for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    /// The display server's number for it.
    pub number: u32,
    /// The device's description of itself, then its connector (`ScreenPlace.identity`).
    pub identity: String,
    /// The size of the picture it shows, in device dots, not turned: what its mode says.
    pub native: (u32, u32),
    /// Hundredths: 100 or 200.
    pub scale: u32,
    /// Degrees clockwise.
    pub rotation: u32,
    /// The screen it shows the same as, or 0 for none.
    pub mirrors: u32,
    /// It has the taskbar, and new windows open on it.
    pub main: bool,
}

impl Screen {
    /// Its connector's name (`Virtual-1`): the last word of its identity, which is how the
    /// user knows it ("maker and serial number, then connector").
    pub fn connector(&self) -> &str {
        self.identity.rsplit(' ').next().filter(|w| !w.is_empty()).unwrap_or(&self.identity)
    }

    /// How the dialog names it: `Virtual-1, 1280 x 800`.
    pub fn label(&self) -> String {
        alloc::format!("{}, {} x {}", self.connector(), self.native.0, self.native.1)
    }

    /// The whole number of dots to a logical pixel.
    fn factor(&self) -> u32 {
        (self.scale / 100).clamp(1, 8)
    }

    /// Its size in the space: its dots, turned if it is rotated a quarter, divided by its scale
    /// (as the display server works it out, rounding down).
    pub fn logical_size(&self) -> (u32, u32) {
        let (w, h) = self.native;
        let (w, h) = if matches!(self.rotation, 90 | 270) { (h, w) } else { (w, h) };
        let f = self.factor();
        (w / f, h / f)
    }
}

/// A tablet the dialog knows, and what it is tied to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tablet {
    /// The tablet's name, as the seat gives it.
    pub name: String,
    /// The identity of the screen it points at, or `None` for the whole space.
    pub screen: Option<String>,
}

/// What the display server is asked for, and what is kept: the places of every screen, the
/// pointer's speed, and the tablets' ties (by name, to a screen's identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// For `arrange`.
    pub places: Vec<ScreenPlace>,
    /// For `pointer`, in hundredths.
    pub speed: u32,
    /// For `pointer`: (tablet name, identity of its screen, or `None` for the whole space).
    pub ties: Vec<(String, Option<String>)>,
}

impl Request {
    /// The ties as `pointer` takes them: each screen by its number among `now`. A tie to a
    /// screen that isn't there is left out, so that tablet is tied as if it had none.
    pub fn wire_ties(&self, now: &[ScreenPlace]) -> Vec<TabletTie> {
        self.ties
            .iter()
            .take(MAX_TABLETS)
            .filter_map(|(name, identity)| {
                let screen = match identity {
                    None => 0,
                    Some(id) => now.iter().find(|p| p.identity == *id)?.screen,
                };
                Some(TabletTie { name: name.clone(), screen })
            })
            .collect()
    }

    /// The `screens.txt` to keep: this arrangement, with what `old` says about screens and
    /// tablets that aren't here now kept as it was (a screen that is unplugged comes back to
    /// its place when it is plugged in again).
    pub fn arrangement(&self, old: &Arrangement) -> Arrangement {
        let mut a = Arrangement::from_places(&self.places);
        a.speed = Some(self.speed);
        for (name, screen) in &self.ties {
            if let Some(identity) = screen
                && a.tablets.len() < MAX_TABLETS
            {
                a.tablets.push((name.clone(), identity.clone()));
            }
        }
        for s in &old.screens {
            if a.screens.len() < MAX_SCREENS && !a.screens.iter().any(|n| n.identity == s.identity)
            {
                a.screens.push(s.clone());
            }
        }
        for (name, identity) in &old.tablets {
            if a.tablets.len() < MAX_TABLETS && !self.ties.iter().any(|(n, _)| n == name) {
                a.tablets.push((name.clone(), identity.clone()));
            }
        }
        a
    }
}

/// The choices, as the user has made them so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Every screen, in the order the user wants them left to right (a screen that shows
    /// another is somewhere in the list, but it has no place in the order).
    screens: Vec<Screen>,
    speed: u32,
    tablets: Vec<Tablet>,
    /// How things were when the dialog opened: what Cancel puts back.
    opened: Request,
}

impl Plan {
    /// The plan for the screens as they are now (`places`, from `screens()`), the pointer's
    /// speed (the saved one, if any) and the tablets known, as (name, identity of the screen
    /// each is tied to). The order starts as the screens' places say: left to right, then top
    /// to bottom.
    pub fn new(places: &[ScreenPlace], speed: Option<u32>, tablets: &[(String, String)]) -> Plan {
        let mut sorted: Vec<&ScreenPlace> = places.iter().take(MAX_SCREENS).collect();
        sorted.sort_by_key(|p| (p.x, p.y, p.screen));
        let screens = sorted
            .into_iter()
            .map(|p| {
                let f = (p.scale / 100).clamp(1, 8);
                let (w, h) = (p.size.width.saturating_mul(f), p.size.height.saturating_mul(f));
                // The place's size is the screen's, turned and divided by its scale: undo both.
                let native = if matches!(p.rotation, 90 | 270) { (h, w) } else { (w, h) };
                Screen {
                    number: p.screen,
                    identity: p.identity.clone(),
                    native,
                    scale: p.scale,
                    rotation: p.rotation,
                    mirrors: p.mirrors,
                    main: p.main && p.mirrors == 0,
                }
            })
            .collect();
        let speed = speed.unwrap_or(SPEED_DEFAULT).clamp(SPEED_MIN, SPEED_MAX);
        let tablets: Vec<Tablet> = tablets
            .iter()
            .take(MAX_TABLETS)
            .map(|(name, identity)| Tablet { name: name.clone(), screen: Some(identity.clone()) })
            .collect();
        let opened = Request {
            places: places.to_vec(),
            speed,
            ties: tablets.iter().map(|t| (t.name.clone(), t.screen.clone())).collect(),
        };
        let mut plan = Plan { screens, speed, tablets, opened };
        plan.tidy_main();
        plan
    }

    /// Makes exactly one non-mirroring screen the main one: the first that says so, or else the
    /// first that doesn't mirror. (The display server says the same of a bad file.)
    fn tidy_main(&mut self) {
        let main = self
            .screens
            .iter()
            .find(|s| s.main && s.mirrors == 0)
            .or_else(|| self.screens.iter().find(|s| s.mirrors == 0))
            .map(|s| s.number);
        for s in &mut self.screens {
            s.main = Some(s.number) == main;
        }
    }

    /// The screens, in their order.
    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    /// The pointer's speed, in hundredths.
    pub fn speed(&self) -> u32 {
        self.speed
    }

    /// The tablets known.
    pub fn tablets(&self) -> &[Tablet] {
        &self.tablets
    }

    /// How things were when the dialog opened, for putting them back.
    pub fn opened(&self) -> &Request {
        &self.opened
    }

    fn index(&self, number: u32) -> Result<usize, Problem> {
        self.screens.iter().position(|s| s.number == number).ok_or(Problem::NoSuchScreen)
    }

    /// The screen numbered `number`.
    pub fn screen(&self, number: u32) -> Option<&Screen> {
        self.screens.iter().find(|s| s.number == number)
    }

    /// The name of the screen numbered `number` (`Virtual-1`), or `?`.
    pub fn name(&self, number: u32) -> &str {
        self.screen(number).map_or("?", Screen::connector)
    }

    // ---- The choices -----------------------------------------------------------------

    /// Makes `number` the main screen. A screen that shows another can't be.
    pub fn set_main(&mut self, number: u32) -> Result<(), Problem> {
        let i = self.index(number)?;
        if self.screens[i].mirrors != 0 {
            return Err(Problem::Mirrors);
        }
        for s in &mut self.screens {
            s.main = s.number == number;
        }
        Ok(())
    }

    /// Sets a screen's scale: 100 or 200.
    pub fn set_scale(&mut self, number: u32, scale: u32) -> Result<(), Problem> {
        let i = self.index(number)?;
        if !SCALES.contains(&scale) {
            return Err(Problem::BadScale);
        }
        self.screens[i].scale = scale;
        Ok(())
    }

    /// Sets a screen's rotation: 0, 90, 180 or 270 degrees.
    pub fn set_rotation(&mut self, number: u32, rotation: u32) -> Result<(), Problem> {
        let i = self.index(number)?;
        if !ROTATIONS.contains(&rotation) {
            return Err(Problem::BadRotation);
        }
        self.screens[i].rotation = rotation;
        Ok(())
    }

    /// Makes `number` show the same picture as `target`, or (with `None`) its own again.
    pub fn set_mirror(&mut self, number: u32, target: Option<u32>) -> Result<(), Problem> {
        let i = self.index(number)?;
        let Some(target) = target else {
            self.screens[i].mirrors = 0;
            return Ok(());
        };
        if target == number {
            return Err(Problem::MirrorsItself);
        }
        let t = self.screen(target).ok_or(Problem::NoSuchScreen)?;
        if t.mirrors != 0 {
            return Err(Problem::TargetMirrors);
        }
        if self.screens.iter().any(|s| s.mirrors == number) {
            return Err(Problem::IsMirrored);
        }
        if self.screens[i].main {
            return Err(Problem::IsMain);
        }
        self.screens[i].mirrors = target;
        Ok(())
    }

    /// The screens `number` could be set to show: every other screen that doesn't itself show
    /// one, unless `number` is main or is shown by another (then none).
    pub fn mirror_options(&self, number: u32) -> Vec<u32> {
        let Some(me) = self.screen(number) else { return Vec::new() };
        if me.main || self.screens.iter().any(|s| s.mirrors == number) {
            return Vec::new();
        }
        self.screens
            .iter()
            .filter(|s| s.number != number && s.mirrors == 0)
            .map(|s| s.number)
            .collect()
    }

    /// Why `number` can't be set to show another screen, if that is so (and it doesn't already
    /// show one: that can always be undone).
    pub fn mirror_blocker(&self, number: u32) -> Option<Problem> {
        let me = self.screen(number)?;
        if me.mirrors != 0 || !self.mirror_options(number).is_empty() {
            return None;
        }
        Some(if me.main {
            Problem::IsMain
        } else if self.screens.iter().any(|s| s.mirrors == number) {
            Problem::IsMirrored
        } else {
            Problem::OnlyOne
        })
    }

    /// Goes to the next choice for `number`'s mirroring: its own picture, then each screen it
    /// may show in turn, then its own again.
    pub fn cycle_mirror(&mut self, number: u32) -> Result<(), Problem> {
        let i = self.index(number)?;
        let options = self.mirror_options(number);
        let next = match self.screens[i].mirrors {
            0 => match options.first() {
                Some(first) => Some(*first),
                None => return Err(self.mirror_blocker(number).unwrap_or(Problem::OnlyOne)),
            },
            current => {
                options.iter().position(|o| *o == current).and_then(|k| options.get(k + 1)).copied()
            }
        };
        self.set_mirror(number, next)
    }

    // ---- The order -------------------------------------------------------------------

    /// The neighbour `number` would swap places with, one step left (`-1`) or right (`1`):
    /// the nearest screen that has a place in the order.
    fn neighbour(&self, number: u32, step: i32) -> Option<usize> {
        let i = self.index(number).ok()?;
        if self.screens[i].mirrors != 0 {
            return None;
        }
        let mut j = i;
        loop {
            j = j.checked_add_signed(step as isize)?;
            let s = self.screens.get(j)?;
            if s.mirrors == 0 {
                return Some(j);
            }
        }
    }

    /// Whether there is a screen to the left of `number` it could swap with.
    pub fn can_move_left(&self, number: u32) -> bool {
        self.neighbour(number, -1).is_some()
    }

    /// Whether there is a screen to the right of `number` it could swap with.
    pub fn can_move_right(&self, number: u32) -> bool {
        self.neighbour(number, 1).is_some()
    }

    /// Moves `number` one place left in the order. `Ok(false)` if it is already first.
    pub fn move_left(&mut self, number: u32) -> Result<bool, Problem> {
        self.shift(number, -1)
    }

    /// Moves `number` one place right in the order. `Ok(false)` if it is already last.
    pub fn move_right(&mut self, number: u32) -> Result<bool, Problem> {
        self.shift(number, 1)
    }

    fn shift(&mut self, number: u32, step: i32) -> Result<bool, Problem> {
        let i = self.index(number)?;
        if self.screens[i].mirrors != 0 {
            return Err(Problem::Mirrors);
        }
        match self.neighbour(number, step) {
            Some(j) => {
                self.screens.swap(i, j);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    // ---- The pointer and the tablets ---------------------------------------------------

    /// Sets the pointer's speed, in hundredths, from 50 to 300.
    pub fn set_speed(&mut self, speed: u32) -> Result<(), Problem> {
        if !(SPEED_MIN..=SPEED_MAX).contains(&speed) {
            return Err(Problem::BadSpeed);
        }
        self.speed = speed;
        Ok(())
    }

    /// One step faster. False if it is at the fastest already.
    pub fn faster(&mut self) -> bool {
        let new = (self.speed + SPEED_STEP).min(SPEED_MAX);
        let changed = new != self.speed;
        self.speed = new;
        changed
    }

    /// One step slower. False if it is at the slowest already.
    pub fn slower(&mut self) -> bool {
        let new = self.speed.saturating_sub(SPEED_STEP).max(SPEED_MIN);
        let changed = new != self.speed;
        self.speed = new;
        changed
    }

    /// Ties tablet number `tablet` to the next choice: the whole space, then each screen that is
    /// part of the space in its order, then the whole space again.
    pub fn cycle_tablet(&mut self, tablet: usize) -> Result<(), Problem> {
        let choices: Vec<Option<String>> = core::iter::once(None)
            .chain(self.screens.iter().filter(|s| s.mirrors == 0).map(|s| Some(s.identity.clone())))
            .collect();
        let t = self.tablets.get_mut(tablet).ok_or(Problem::NoSuchScreen)?;
        // A tie to a screen that isn't there isn't among the choices: the next one is the first.
        let at = choices.iter().position(|c| *c == t.screen);
        t.screen = choices[at.map_or(0, |k| (k + 1) % choices.len())].clone();
        Ok(())
    }

    /// What tablet number `tablet` is tied to, in words: `The whole space`, or a screen's name
    /// (with `(not connected)` for one that isn't there).
    pub fn tie_label(&self, tablet: usize) -> String {
        match self.tablets.get(tablet).and_then(|t| t.screen.as_ref()) {
            None => String::from("The whole space"),
            Some(id) => match self.screens.iter().find(|s| s.identity == *id) {
                Some(s) => String::from(s.connector()),
                None => alloc::format!("{} (not connected)", id.rsplit(' ').next().unwrap_or(id)),
            },
        }
    }

    // ---- What to ask for ---------------------------------------------------------------

    /// Whether the plan could be asked for: the rules at the top of this file.
    pub fn check(&self) -> Result<(), Problem> {
        if self.screens.is_empty() {
            return Err(Problem::NoScreens);
        }
        if self.screens.len() > MAX_SCREENS {
            return Err(Problem::TooMany);
        }
        let shown = || self.screens.iter().filter(|s| s.mirrors == 0);
        if shown().next().is_none() {
            return Err(Problem::EveryScreenMirrors);
        }
        if shown().filter(|s| s.main).count() != 1
            || self.screens.iter().any(|s| s.main && s.mirrors != 0)
        {
            return Err(Problem::NoMain);
        }
        for s in &self.screens {
            if s.mirrors == 0 {
                continue;
            }
            if s.mirrors == s.number {
                return Err(Problem::MirrorsItself);
            }
            match self.screen(s.mirrors) {
                None => return Err(Problem::NoSuchScreen),
                Some(t) if t.mirrors != 0 => return Err(Problem::TargetMirrors),
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The place of every screen: those that don't mirror side by side in their order, from 0,
    /// tops aligned at 0, each as wide as its logical size. A screen that shows another keeps
    /// its own scale and rotation and sits at 0, 0 (its place means nothing to the display
    /// server). Listed in the plan's order.
    pub fn places(&self) -> Vec<ScreenPlace> {
        let mut x: i32 = 0;
        self.screens
            .iter()
            .map(|s| {
                let (w, h) = s.logical_size();
                let size = Size { width: w, height: h };
                let at = if s.mirrors == 0 { x } else { 0 };
                if s.mirrors == 0 {
                    x = x.saturating_add(i32::try_from(w).unwrap_or(i32::MAX));
                }
                ScreenPlace {
                    screen: s.number,
                    identity: s.identity.clone(),
                    x: at,
                    y: 0,
                    scale: s.scale,
                    rotation: s.rotation,
                    mirrors: s.mirrors,
                    main: s.main && s.mirrors == 0,
                    size,
                }
            })
            .collect()
    }

    /// What to ask the display server for, if the plan is allowed.
    pub fn request(&self) -> Result<Request, Problem> {
        self.check()?;
        Ok(Request {
            places: self.places(),
            speed: self.speed,
            ties: self.tablets.iter().map(|t| (t.name.clone(), t.screen.clone())).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::Saved;

    fn place(screen: u32, connector: &str, x: i32, w: u32, h: u32) -> ScreenPlace {
        ScreenPlace {
            screen,
            identity: alloc::format!("RHT-1234-00000000 {connector}"),
            x,
            y: 0,
            scale: 100,
            rotation: 0,
            mirrors: 0,
            main: screen == 1,
            size: Size { width: w, height: h },
        }
    }

    fn two() -> Plan {
        Plan::new(
            &[place(1, "Virtual-1", 0, 1280, 800), place(2, "Virtual-2", 1280, 1280, 800)],
            None,
            &[],
        )
    }

    fn three() -> Plan {
        Plan::new(
            &[
                place(1, "Virtual-1", 0, 1280, 800),
                place(2, "Virtual-2", 1280, 1280, 800),
                place(3, "Virtual-3", 2560, 640, 480),
            ],
            None,
            &[],
        )
    }

    fn xs(p: &Plan) -> Vec<(u32, i32, i32)> {
        p.places().iter().map(|s| (s.screen, s.x, s.y)).collect()
    }

    #[test]
    fn refusals_have_words() {
        for e in [WindowError::BadArgument, WindowError::NoSuchWindow, WindowError::NotAllowed] {
            assert!(!refusal(&e).is_empty());
        }
        assert_eq!(refusal(&WindowError::Stale), "it didn't say why");
    }

    #[test]
    fn names_and_sizes() {
        let p = two();
        assert_eq!(p.screens()[0].connector(), "Virtual-1");
        assert_eq!(p.screens()[0].label(), "Virtual-1, 1280 x 800");
        assert_eq!(p.name(2), "Virtual-2");
        // An identity with no spaces is its own name.
        let q = Plan::new(
            &[ScreenPlace { identity: "HDMI-A-1".into(), ..place(1, "x", 0, 8, 8) }],
            None,
            &[],
        );
        assert_eq!(q.screens()[0].connector(), "HDMI-A-1");
    }

    #[test]
    fn side_by_side_in_either_order() {
        let mut p = two();
        assert_eq!(xs(&p), [(1, 0, 0), (2, 1280, 0)]);
        assert_eq!(p.move_right(1), Ok(true));
        // Screen 2 is now first; the places are listed in the new order.
        assert_eq!(xs(&p), [(2, 0, 0), (1, 1280, 0)]);
        assert_eq!(p.move_right(1), Ok(false));
        assert_eq!(p.move_left(1), Ok(true));
        assert_eq!(xs(&p), [(1, 0, 0), (2, 1280, 0)]);
        assert_eq!(p.move_left(1), Ok(false));
        assert!(!p.can_move_left(1) && p.can_move_right(1));
        assert!(p.can_move_left(2) && !p.can_move_right(2));
    }

    #[test]
    fn the_order_starts_from_where_the_screens_are() {
        // Given in the wrong order and the second at the left.
        let p = Plan::new(
            &[place(1, "Virtual-1", 1280, 1280, 800), place(2, "Virtual-2", 0, 640, 480)],
            None,
            &[],
        );
        let numbers: Vec<u32> = p.screens().iter().map(|s| s.number).collect();
        assert_eq!(numbers, [2, 1]);
        assert_eq!(xs(&p), [(2, 0, 0), (1, 640, 0)]);
    }

    #[test]
    fn scale_halves_and_rotation_swaps() {
        let mut p = two();
        p.set_scale(1, 200).unwrap();
        assert_eq!(p.screen(1).unwrap().logical_size(), (640, 400));
        assert_eq!(xs(&p), [(1, 0, 0), (2, 640, 0)]);
        p.set_rotation(1, 90).unwrap();
        assert_eq!(p.screen(1).unwrap().logical_size(), (400, 640));
        assert_eq!(xs(&p), [(1, 0, 0), (2, 400, 0)]);
        p.set_rotation(1, 180).unwrap();
        assert_eq!(p.screen(1).unwrap().logical_size(), (640, 400));
        p.set_rotation(2, 270).unwrap();
        assert_eq!(xs(&p), [(1, 0, 0), (2, 640, 0)]);
        let places = p.places();
        assert_eq!((places[0].scale, places[0].rotation), (200, 180));
        assert_eq!((places[0].size.width, places[0].size.height), (640, 400));
        assert_eq!((places[1].size.width, places[1].size.height), (800, 1280));
        // Only what is offered.
        assert_eq!(p.set_scale(1, 150), Err(Problem::BadScale));
        assert_eq!(p.set_scale(1, 300), Err(Problem::BadScale));
        assert_eq!(p.set_rotation(1, 45), Err(Problem::BadRotation));
        assert_eq!(p.set_scale(9, 100), Err(Problem::NoSuchScreen));
    }

    #[test]
    fn the_size_survives_a_scale_and_a_turn_the_screen_already_had() {
        // The server's places hold logical sizes: a 2x screen turned a quarter is 400 x 640.
        let mut turned = place(1, "Virtual-1", 0, 400, 640);
        turned.scale = 200;
        turned.rotation = 90;
        let p = Plan::new(&[turned], None, &[]);
        assert_eq!(p.screens()[0].native, (1280, 800));
        assert_eq!(p.places()[0].size, Size { width: 400, height: 640 });
    }

    #[test]
    fn exactly_one_main() {
        let mut p = three();
        assert_eq!(p.screens().iter().filter(|s| s.main).count(), 1);
        p.set_main(2).unwrap();
        assert!(!p.screen(1).unwrap().main && p.screen(2).unwrap().main);
        assert_eq!(p.request().unwrap().places.iter().filter(|s| s.main).count(), 1);
        assert_eq!(p.set_main(9), Err(Problem::NoSuchScreen));
        // Nobody said main: the first one that can be is.
        let none = Plan::new(
            &[
                ScreenPlace { main: false, ..place(1, "A", 0, 8, 8) },
                ScreenPlace { main: false, ..place(2, "B", 8, 8, 8) },
            ],
            None,
            &[],
        );
        assert!(none.screen(1).unwrap().main && !none.screen(2).unwrap().main);
    }

    #[test]
    fn mirroring_takes_a_screen_out_of_the_order() {
        let mut p = three();
        p.set_mirror(2, Some(1)).unwrap();
        // Screen 3 now sits right after screen 1.
        assert_eq!(xs(&p), [(1, 0, 0), (2, 0, 0), (3, 1280, 0)]);
        let places = p.places();
        assert_eq!((places[1].mirrors, places[1].main), (1, false));
        assert_eq!(p.request().unwrap().places, places);
        // It has no place to move to, and can't become the main screen.
        assert_eq!(p.move_left(2), Err(Problem::Mirrors));
        assert_eq!(p.set_main(2), Err(Problem::Mirrors));
        assert!(!p.can_move_left(2) && !p.can_move_right(2));
        // Moving skips it: 3 swaps with 1.
        assert_eq!(p.move_left(3), Ok(true));
        // Screen 3 is 640 wide, so screen 1 now starts at 640; screen 2 has no place of its own.
        assert_eq!(xs(&p), [(3, 0, 0), (2, 0, 0), (1, 640, 0)]);
        // Its own picture again.
        p.set_mirror(2, None).unwrap();
        assert_eq!(p.screen(2).unwrap().mirrors, 0);
    }

    #[test]
    fn refuses_what_could_not_work() {
        let mut p = three();
        // Itself.
        assert_eq!(p.set_mirror(2, Some(2)), Err(Problem::MirrorsItself));
        // A screen that isn't there.
        assert_eq!(p.set_mirror(2, Some(9)), Err(Problem::NoSuchScreen));
        // The main screen shows nothing else.
        assert_eq!(p.set_mirror(1, Some(2)), Err(Problem::IsMain));
        // Chains: 3 shows 2; then nothing can be made to show 3 (it shows another), and 2 can't
        // show 1 (another shows it).
        p.set_mirror(3, Some(2)).unwrap();
        assert_eq!(p.set_mirror(1, Some(3)), Err(Problem::TargetMirrors));
        assert_eq!(p.set_mirror(2, Some(1)), Err(Problem::IsMirrored));
        let mut r = three();
        r.set_mirror(2, Some(1)).unwrap();
        assert_eq!(r.set_mirror(3, Some(2)), Err(Problem::TargetMirrors));
        // Nothing was changed by the refusals.
        assert_eq!(r.screen(3).unwrap().mirrors, 0);
        assert_eq!(p.screen(2).unwrap().mirrors, 0);
        // With one screen there is nothing to show.
        let one = Plan::new(&[place(1, "A", 0, 8, 8)], None, &[]);
        assert_eq!(one.mirror_blocker(1), Some(Problem::IsMain));
    }

    #[test]
    fn cycling_the_mirror_choice() {
        let mut p = three();
        assert_eq!(p.mirror_options(2), [1, 3]);
        assert_eq!(p.mirror_options(1), Vec::<u32>::new());
        assert_eq!(p.mirror_blocker(1), Some(Problem::IsMain));
        assert_eq!(p.cycle_mirror(1), Err(Problem::IsMain));
        let seen: Vec<u32> = (0..4)
            .map(|_| {
                p.cycle_mirror(2).unwrap();
                p.screen(2).unwrap().mirrors
            })
            .collect();
        // First screen, second screen, its own picture, first screen again.
        assert_eq!(seen, [1, 3, 0, 1]);
        // A screen that is shown can't show another.
        assert_eq!(p.mirror_options(1), Vec::<u32>::new());
        p.set_main(3).unwrap();
        assert_eq!(p.mirror_blocker(1), Some(Problem::IsMirrored));
        assert_eq!(p.cycle_mirror(1), Err(Problem::IsMirrored));
        // Two screens, the other main: the one that isn't can show it.
        let mut t = two();
        assert_eq!(t.mirror_blocker(2), None);
        t.set_mirror(2, Some(1)).unwrap();
        assert_eq!(t.check(), Ok(()));
    }

    #[test]
    fn a_plan_that_leaves_no_main_or_no_screens_is_not_asked_for() {
        let mut p = two();
        assert!(p.request().is_ok());
        // Forced past the setters (they would not allow it): every screen mirrors.
        p.screens[0].mirrors = 2;
        p.screens[1].mirrors = 1;
        assert_eq!(p.check(), Err(Problem::EveryScreenMirrors));
        assert_eq!(p.request(), Err(Problem::EveryScreenMirrors));
        // No main.
        let mut q = two();
        q.screens[0].main = false;
        assert_eq!(q.check(), Err(Problem::NoMain));
        // Two mains.
        let mut r = two();
        r.screens[1].main = true;
        assert_eq!(r.check(), Err(Problem::NoMain));
        // A mirror that is itself shown by another, or shows itself.
        let mut s = three();
        s.screens[1].mirrors = 3;
        s.screens[2].mirrors = 1;
        assert_eq!(s.check(), Err(Problem::TargetMirrors));
        let mut u = two();
        u.screens[1].mirrors = 2;
        assert_eq!(u.check(), Err(Problem::MirrorsItself));
        // No screens.
        assert_eq!(Plan::new(&[], None, &[]).check(), Err(Problem::NoScreens));
        // Every refusal has words.
        for e in [Problem::NoMain, Problem::OnlyOne, Problem::TooMany, Problem::BadSpeed] {
            assert!(!e.explain().is_empty());
        }
    }

    #[test]
    fn pointer_speed_has_limits() {
        let mut p = two();
        assert_eq!(p.speed(), 100);
        assert!(p.faster());
        assert_eq!(p.speed(), 125);
        for _ in 0..20 {
            p.faster();
        }
        assert_eq!(p.speed(), SPEED_MAX);
        assert!(!p.faster());
        for _ in 0..20 {
            p.slower();
        }
        assert_eq!(p.speed(), SPEED_MIN);
        assert!(!p.slower());
        assert_eq!(p.set_speed(49), Err(Problem::BadSpeed));
        assert_eq!(p.set_speed(301), Err(Problem::BadSpeed));
        assert_eq!(p.set_speed(0), Err(Problem::BadSpeed));
        assert_eq!(p.speed(), SPEED_MIN);
        p.set_speed(300).unwrap();
        p.set_speed(50).unwrap();
        // A saved speed out of range is brought in.
        assert_eq!(Plan::new(&[], Some(5000), &[]).speed(), SPEED_MAX);
        assert_eq!(Plan::new(&[], Some(1), &[]).speed(), SPEED_MIN);
        assert_eq!(Plan::new(&[], Some(150), &[]).speed(), 150);
    }

    #[test]
    fn tablets_are_tied_in_turn() {
        let tablets =
            [("QEMU Virtio Tablet".to_string(), "RHT-1234-00000000 Virtual-2".to_string())];
        let mut p = Plan::new(
            &[place(1, "Virtual-1", 0, 1280, 800), place(2, "Virtual-2", 1280, 1280, 800)],
            None,
            &tablets,
        );
        assert_eq!(p.tie_label(0), "Virtual-2");
        // Whole space, screen 1, screen 2, whole space.
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "The whole space");
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "Virtual-1");
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "Virtual-2");
        assert_eq!(p.cycle_tablet(5), Err(Problem::NoSuchScreen));
        // A screen that mirrors another isn't one to tie to.
        p.set_mirror(2, Some(1)).unwrap();
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "The whole space");
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "Virtual-1");
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "The whole space");
        // The wire form names screens by number; the whole space is 0.
        let request = p.request().unwrap();
        let now = request.places.clone();
        assert_eq!(
            request.wire_ties(&now),
            [TabletTie { name: "QEMU Virtio Tablet".into(), screen: 0 }]
        );
        p.cycle_tablet(0).unwrap();
        let request = p.request().unwrap();
        assert_eq!(
            request.wire_ties(&now),
            [TabletTie { name: "QEMU Virtio Tablet".into(), screen: 1 }]
        );
        // A tie to a screen that isn't there is left out.
        assert_eq!(request.wire_ties(&now[1..]), []);
        // Known tablets are kept to what the file keeps.
        let many: Vec<(String, String)> =
            (0..20).map(|i| (alloc::format!("t{i}"), String::from("x"))).collect();
        assert_eq!(Plan::new(&[], None, &many).tablets().len(), MAX_TABLETS);
        // One tied to a screen that isn't there says so, and the next choice is the first.
        let gone = [("t".to_string(), "Other Virtual-9".to_string())];
        let mut g = Plan::new(&[place(1, "Virtual-1", 0, 8, 8)], None, &gone);
        assert_eq!(g.tie_label(0), "Virtual-9 (not connected)");
        g.cycle_tablet(0).unwrap();
        assert_eq!(g.tie_label(0), "The whole space");
    }

    #[test]
    fn what_is_kept_holds_what_was_there() {
        let mut p = two();
        p.set_scale(2, 200).unwrap();
        p.set_speed(150).unwrap();
        let old = Arrangement {
            screens: alloc::vec![
                Saved {
                    identity: "RHT-1234-00000000 Virtual-1".into(),
                    x: 5,
                    y: 5,
                    scale: 100,
                    rotation: 0,
                    mirrors: None,
                    main: true,
                },
                Saved {
                    identity: "Unplugged Screen".into(),
                    x: 0,
                    y: 0,
                    scale: 200,
                    rotation: 90,
                    mirrors: None,
                    main: false,
                },
            ],
            speed: Some(75),
            tablets: alloc::vec![("Away Tablet".into(), "Unplugged Screen".into())],
        };
        let kept = p.request().unwrap().arrangement(&old);
        // The screens that are here are as chosen; the one that isn't is as it was.
        assert_eq!(kept.screens.len(), 3);
        assert_eq!((kept.screens[1].x, kept.screens[1].scale), (1280, 200));
        assert_eq!(kept.screens[2].identity, "Unplugged Screen");
        assert_eq!(kept.speed, Some(150));
        assert_eq!(kept.tablets.len(), 1);
        // It reads back as the same.
        assert_eq!(Arrangement::parse(kept.to_text().as_bytes()).unwrap(), kept);
        // Applying it to the screens as they are is nothing to do.
        let request = p.request().unwrap();
        assert_eq!(kept.apply(&request.places), None);
    }

    #[test]
    fn a_tablet_tie_that_was_changed_replaces_the_old_one() {
        let tablets = [("T".to_string(), "RHT-1234-00000000 Virtual-2".to_string())];
        let mut p = Plan::new(
            &[place(1, "Virtual-1", 0, 8, 8), place(2, "Virtual-2", 8, 8, 8)],
            None,
            &tablets,
        );
        p.cycle_tablet(0).unwrap();
        p.cycle_tablet(0).unwrap();
        let old = Arrangement {
            screens: Vec::new(),
            speed: None,
            tablets: alloc::vec![("T".into(), "RHT-1234-00000000 Virtual-2".into())],
        };
        let kept = p.request().unwrap().arrangement(&old);
        assert_eq!(kept.tablets, [("T".to_string(), "RHT-1234-00000000 Virtual-1".to_string())]);
        // Tied to the whole space: not in the file at all.
        p.cycle_tablet(0).unwrap();
        p.cycle_tablet(0).unwrap();
        assert_eq!(p.tie_label(0), "The whole space");
        assert!(p.request().unwrap().arrangement(&old).tablets.is_empty());
    }

    #[test]
    fn cancel_has_what_was_there() {
        let mut p = three();
        let opened = p.opened().clone();
        p.set_scale(1, 200).unwrap();
        p.move_right(1).unwrap();
        p.set_speed(300).unwrap();
        assert_eq!(*p.opened(), opened);
        assert_eq!(opened.places[1].x, 1280);
        assert_eq!(opened.speed, 100);
    }
}
