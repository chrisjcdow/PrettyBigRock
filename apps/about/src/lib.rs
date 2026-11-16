// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! About PrettyBigRock (docs/design/0026-apps-m6-tests-and-stages.md, sections 1 and 3), as a
//! sandstone app tested on the host.
//!
//! The window is 1.3's About: a green banner with the logo, the system's name, its version and
//! where the name comes from, and under it the facts about this computer: **Display**,
//! **Memory**, **Uptime** and **Disk**. Everything the window says about the computer comes
//! from `pbr.sysinfo`, a read-only *interface* (a list of questions a service answers; note
//! 0008) that the session's sandbox manager hands to apps whose manifest asks for `sysinfo`.
//! It says nothing about other apps or other users. The uptime comes from the monotonic clock
//! (a clock that only counts up from start-up, and is the same for every program).
//!
//! `pbr.sysinfo` is reached through the [`Sysinfo`] trait. A *trait* is a list of functions a
//! type promises to have; the program implements it over the real capability, and the host
//! tests implement it with a pretend one.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use pbr_desktop::icons::{self, Icon};
use pbr_draw::{Canvas as DrawCanvas, Colour, Rect, TextStyle};
use pbr_interfaces::pbr_sysinfo::{ScreenSummary, VolumeInfo};
use pbr_sandstone::{
    App, Canvas, Column, Context, Element, Identify, Label, Length, Padding, Panel, Row, Settings,
    Space, Theme,
};

/// What a fact says when it can't be had yet.
pub const UNKNOWN: &str = "\u{2014}";

/// What the window asks of `pbr.sysinfo`. Each answer is `None` when it can't be had: the
/// service isn't running yet (`NotReady`), or the app wasn't given the capability. (`Option` is
/// Rust's "maybe a value": `Some(value)` or `None`.) The window asks again a second later.
pub trait Sysinfo {
    /// The system's name and version: "PrettyBigRock", "2.0.0-alpha.1".
    fn version(&mut self) -> Option<(String, String)>;
    /// The computer's memory and how much is free, in bytes.
    fn memory(&mut self) -> Option<(u64, u64)>;
    /// The screens.
    fn screens(&mut self) -> Option<Vec<ScreenSummary>>;
    /// The volumes.
    fn volumes(&mut self) -> Option<Vec<VolumeInfo>>;
}

// ---------------------------------------------------------------------------------------------
// The words for each fact
// ---------------------------------------------------------------------------------------------

/// A size in bytes in words, as 1.3's `fmt_size` did: whole "bytes" below 1024, then KB, MB, GB
/// or TB with one decimal place when it is under 100 and not whole ("1.5 MB", "20 GB").
pub fn fmt_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut unit = 0;
    let (mut whole, mut rest) = (bytes / 1024, bytes % 1024);
    while whole >= 1024 && unit < 3 {
        rest = whole % 1024;
        whole /= 1024;
        unit += 1;
    }
    let tenth = rest * 10 / 1024;
    if whole >= 100 || tenth == 0 {
        format!("{whole} {}", UNITS[unit])
    } else {
        format!("{whole}.{tenth} {}", UNITS[unit])
    }
}

/// A screen's scale (a percentage: 100 is ordinary, 200 is "2x") as a multiple: "2x", "1.5x",
/// "1.25x".
fn scale_text(percent: u32) -> String {
    let (whole, hundredths) = (percent / 100, percent % 100);
    if hundredths == 0 {
        format!("{whole}x")
    } else if hundredths % 10 == 0 {
        format!("{whole}.{}x", hundredths / 10)
    } else {
        format!("{whole}.{hundredths:02}x")
    }
}

/// The screens in words: "1024 x 768", with ", 2x" after any whose scale isn't 100, and a "; "
/// between screens. No screens at all say "No screen".
pub fn display_text(screens: &[ScreenSummary]) -> String {
    if screens.is_empty() {
        return "No screen".to_string();
    }
    let parts: Vec<String> = screens
        .iter()
        .map(|s| {
            let size = format!("{} x {}", s.width, s.height);
            if s.scale == 100 { size } else { format!("{size}, {}", scale_text(s.scale)) }
        })
        .collect();
    parts.join("; ")
}

/// Memory in words, as 1.3 ("N MB" in whole megabytes) with what is free: "N MB, M MB free".
pub fn memory_text(total: u64, available: u64) -> String {
    const MB: u64 = 1024 * 1024;
    format!("{} MB, {} MB free", total / MB, available / MB)
}

/// Time since start-up as 1.3 wrote it: "1 h 2 min 3 s".
pub fn uptime_text(seconds: u64) -> String {
    format!("{} h {} min {} s", seconds / 3600, (seconds / 60) % 60, seconds % 60)
}

/// The system volume in 1.3's short form: "X free of Y", or "X free (in memory)" for the disk
/// that lives in memory and is lost at restart. If there is no system volume, "No disk".
pub fn disk_text(volumes: &[VolumeInfo]) -> String {
    let Some(v) = volumes.iter().find(|v| v.system) else {
        return "No disk".to_string();
    };
    let free = fmt_size(v.free);
    if v.in_memory {
        format!("{free} free (in memory)")
    } else {
        format!("{free} free of {}", fmt_size(v.size))
    }
}

// ---------------------------------------------------------------------------------------------
// The facts, and the window
// ---------------------------------------------------------------------------------------------

/// What the window knows about the computer. `None` is a fact that couldn't be had yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The system's name.
    pub name: Option<String>,
    /// Its version.
    pub version: Option<String>,
    /// The screens, in words.
    pub display: Option<String>,
    /// Memory, in words.
    pub memory: Option<String>,
    /// Seconds since start-up.
    pub uptime: Option<u64>,
    /// The system volume, in words.
    pub disk: Option<String>,
}

impl Facts {
    /// Each fact as the log says it (`about: FACT VALUE`): its name and its value, "—" for one
    /// not had. The uptime is in the 1.3 form.
    pub fn lines(&self) -> [(&'static str, String); 6] {
        let text = |t: &Option<String>| t.clone().unwrap_or_else(|| UNKNOWN.to_string());
        [
            ("name", text(&self.name)),
            ("version", text(&self.version)),
            ("display", text(&self.display)),
            ("memory", text(&self.memory)),
            ("uptime", self.uptime.map_or_else(|| UNKNOWN.to_string(), uptime_text)),
            ("disk", text(&self.disk)),
        ]
    }
}

/// Works out which lines to write to the log: each fact when it changes, except the uptime,
/// which changes every second and is written at most once every 10 seconds, to keep the log
/// short.
#[derive(Debug, Default)]
pub struct Reporter {
    /// What was last written, by fact.
    last: Vec<(&'static str, String)>,
    /// The uptime, in seconds, when it was last written.
    uptime_at: Option<u64>,
}

impl Reporter {
    /// How often the uptime is written, in seconds.
    const UPTIME_EVERY: u64 = 10;

    /// A reporter that has written nothing yet.
    pub fn new() -> Reporter {
        Reporter::default()
    }

    /// The lines to write now (each "FACT VALUE"), for `facts`.
    pub fn report(&mut self, facts: &Facts) -> Vec<String> {
        let mut out = Vec::new();
        let first = self.last.is_empty();
        for (name, value) in facts.lines() {
            if name == "uptime" {
                let due = match (facts.uptime, self.uptime_at) {
                    (None, _) => first,
                    (Some(_), None) => true,
                    (Some(now), Some(then)) => now.saturating_sub(then) >= Self::UPTIME_EVERY,
                };
                if !due {
                    continue;
                }
                self.uptime_at = facts.uptime.or(self.uptime_at);
            }
            match self.last.iter_mut().find(|(n, _)| *n == name) {
                Some((_, old)) if *old == value => continue,
                Some((_, old)) => *old = value.clone(),
                None => self.last.push((name, value.clone())),
            }
            out.push(format!("{name} {value}"));
        }
        out
    }
}

/// What the window's timer sends.
#[derive(Clone, Debug)]
pub enum Message {
    /// A second has passed: ask again, and move the uptime on.
    Tick,
}

/// The window's size: 1.3's 400 x 260, and not resizable.
pub fn settings() -> Settings {
    Settings {
        size: (WIDTH as u32, 260),
        resizable: false,
        min_size: (0, 0),
        over: None,
        system: false,
    }
}

/// The window's width, and the banner's.
/// How many ticks (seconds) apart the disk's free space is asked for.
const DISK_EVERY: u32 = 10;

const WIDTH: i32 = 400;
/// The banner's height.
const BANNER: i32 = 120;
/// How far in from the left the labels start.
const LEFT: i32 = 24;
/// How wide a label's column is: the values start 86 dots after it (1.3: 110 and 24).
const LABEL_WIDTH: i32 = 86;
/// The distance from one fact's top to the next, in 1.3.
const PITCH: i32 = 18;
/// The banner's colours, and its texts' (1.3's).
const BANNER_TOP: Colour = Colour::solid(0x2f6b55);
const BANNER_BOTTOM: Colour = Colour::solid(0x1d4234);
const NAME_COLOUR: Colour = Colour::solid(0xffffff);
const VERSION_COLOUR: Colour = Colour::solid(0xcfe8dc);
const TAGLINE_COLOUR: Colour = Colour::solid(0xa9cdbd);
/// 1.3's colour for a fact's value.
const VALUE_COLOUR: Colour = Colour::solid(0x2a2e38);
/// The banner's last line.
const TAGLINE: &str = "Named for Table Mountain, a pretty big rock.";

/// About, over `S`.
pub struct About<S: Sysinfo> {
    sys: S,
    facts: Facts,
    /// Changes whenever the banner would look different, so that sandstone draws it again
    /// (a canvas is only drawn again when it is told it has changed).
    banner: u64,
    /// Ticks since the disk was last asked about.
    since_disk: u32,
}

impl<S: Sysinfo> About<S> {
    /// The window, with what `sys` can say at once. The uptime is set when the window opens.
    pub fn new(sys: S) -> About<S> {
        let mut about = About { sys, facts: Facts::default(), banner: 1, since_disk: 0 };
        about.read();
        about
    }

    /// What it knows now.
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// Asks `pbr.sysinfo` again. The name and version are only asked for until they are had:
    /// they don't change. A fact that can't be had is left as it was, if it was had before
    /// (a service that goes away for a moment doesn't blank the window), or stays unknown.
    fn read(&mut self) {
        let had_version = self.facts.name.is_some() && self.facts.version.is_some();
        if !had_version && let Some((name, version)) = self.sys.version() {
            self.facts.name = Some(name);
            self.facts.version = Some(version);
            self.banner += 1;
        }
        if let Some(screens) = self.sys.screens() {
            self.facts.display = Some(display_text(&screens));
        }
        if let Some((total, available)) = self.sys.memory() {
            self.facts.memory = Some(memory_text(total, available));
        }
        // Free space is asked of each volume's filesystem server, which takes a moment (and
        // up to a time-out for one that doesn't answer), so it is asked every
        // [`DISK_EVERY`] ticks rather than every second.
        if self.facts.disk.is_none() || self.since_disk >= DISK_EVERY {
            self.since_disk = 0;
            if let Some(volumes) = self.sys.volumes() {
                self.facts.disk = Some(disk_text(&volumes));
            }
        }
        self.since_disk += 1;
    }

    /// One row of the facts: its bold green label, and its value.
    fn fact(&self, label: &str, value: Option<&str>) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        Row::new()
            .push(Label::new(label).bold().colour(t.strong).width(Length::Fixed(LABEL_WIDTH)))
            .push(Label::new(value.unwrap_or(UNKNOWN)).colour(VALUE_COLOUR).id(label))
            .into()
    }

    /// The banner: 1.3's green gradient, the logo, the name and the version.
    fn banner_view(&self) -> Element<'_, Message> {
        let name = self.facts.name.clone().unwrap_or_default();
        let version = match &self.facts.version {
            Some(v) => format!("Version {v}"),
            None => String::new(),
        };
        Canvas::new(WIDTH, BANNER, move |canvas: &mut DrawCanvas<'_>, area: Rect| {
            canvas.gradient_down(area, BANNER_TOP, BANNER_BOTTOM);
            icons::draw(canvas, Icon::Logo, area.x + 22, area.y + 12, 6);
            canvas.text(area.x + 130, area.y + 30, &name, NAME_COLOUR, TextStyle::PLAIN.sized(3));
            canvas.text(area.x + 132, area.y + 66, &version, VERSION_COLOUR, TextStyle::BOLD);
            canvas.text(area.x + 132, area.y + 84, TAGLINE, TAGLINE_COLOUR, TextStyle::PLAIN);
        })
        .version(self.banner)
        .width(Length::Fill)
        .into()
    }
}

impl<S: Sysinfo> App for About<S> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        // 1.3 put the facts 16 dots below the banner, 18 apart, and the closing lines 28 dots
        // below the top of the last fact, 13 apart. A line of text is 11 dots high, so the gap
        // between facts is 7, and 3 more (on top of two gaps of 7) lead into the closing lines.
        let uptime = self.facts.uptime.map(uptime_text);
        let facts = Column::new()
            .spacing(PITCH - t.line_height())
            .push(self.fact("Display", self.facts.display.as_deref()))
            .push(self.fact("Memory", self.facts.memory.as_deref()))
            .push(self.fact("Uptime", uptime.as_deref()))
            .push(self.fact("Disk", self.facts.disk.as_deref()))
            .push(Space::new(0, 3))
            .push(
                Column::new()
                    .spacing(13 - t.line_height())
                    .push(Label::new("A small hobby operating system, written").colour(t.text_dim))
                    .push(Label::new("from scratch in Rust.").colour(t.text_dim)),
            );
        Panel::new(Column::new().width(Length::Fill).push(self.banner_view()).push(
            Panel::new(facts).padding_each(Padding { top: 16, right: LEFT, bottom: 0, left: LEFT }),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        self.facts.uptime = Some(cx.now_ms() / 1000);
        cx.after(1000, Message::Tick);
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Tick => {
                self.facts.uptime = Some(cx.now_ms() / 1000);
                self.read();
                cx.after(1000, Message::Tick);
            }
        }
    }
}

#[cfg(test)]
mod tests;
