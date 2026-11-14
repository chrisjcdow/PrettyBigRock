// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The permissions viewer (docs/design/0025-picker-clipboard-permissions.md, section 4), as a
//! sandstone app tested on the host.
//!
//! Every app runs in a *sandbox*: it holds only the capabilities (rights to use one thing,
//! kept by the kernel for it; note 0006) that its manifest, the user's rules and the user's own
//! choices gave it. The session's sandbox manager keeps a record of each, and why (note 0017,
//! section 7). This window shows those records for each running app, in words, and lets the
//! user take a file or folder back (**Take back**: the app loses it and everything it opened
//! through it, at once) or stop the app (**Stop app**).
//!
//! The viewer holds a `pbr.sandbox` capability that may only look (`list`, `details`,
//! `grants`) and take back (`revoke`, `stop`): it can't launch anything or give anything. Its
//! window is a *system* window, which only system apps may make, so no app can draw a fake one.
//!
//! The sandbox manager is reached through the [`Apps`] trait, so the host tests can give a
//! pretend one.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use pbr_interfaces::pbr_sandbox::{
    AppDetails, AppInfo, AppServices, FileRights, Grant, GrantReason,
};
use pbr_sandstone::{
    Align, App, Button, Column, Context, Element, Identify, Label, Length, List, ListColumn,
    ListRow, Panel, Row, Settings, Stretch, Theme,
};

/// What the viewer asks of the sandbox manager.
pub trait Apps {
    /// The running apps.
    fn list(&mut self) -> Result<Vec<AppInfo>, String>;
    /// More about one.
    fn details(&mut self, app: u32) -> Result<AppDetails, String>;
    /// Everything it holds, and why.
    fn grants(&mut self, app: u32) -> Result<Vec<Grant>, String>;
    /// Takes back what it holds at `path`.
    fn revoke(&mut self, app: u32, path: &str) -> Result<(), String>;
    /// Stops it.
    fn stop(&mut self, app: u32) -> Result<(), String>;
}

/// One row of what an app holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// What it says.
    pub text: String,
    /// The path a file or folder grant can be taken back by; `None` for services (a window,
    /// the picker), which can't be taken back one by one in M6: stopping the app is the way.
    pub path: Option<String>,
}

/// "read" or "read and change".
fn rights(r: FileRights) -> &'static str {
    if r.0 & FileRights::WRITE.0 != 0 { "read and change" } else { "read" }
}

/// One grant in words (note 0025, section 4's table).
pub fn describe(app: &str, g: &Grant) -> String {
    let r = rights(g.rights);
    // An app's own folder is a grant at `/Home/Apps/APP` (note 0024, section 4).
    if g.path == format!("/Home/Apps/{app}") && g.reason != GrantReason::Chosen {
        return format!("Its own folder ({}): {r}", g.path);
    }
    let why = match g.reason {
        GrantReason::Manifest => "asked for in its manifest",
        GrantReason::Rule => "allowed by your rules",
        GrantReason::Chosen => "you chose it",
        GrantReason::Trusted => "it is trusted with all your files",
    };
    format!("{}: {r}, {why}", g.path)
}

/// What it holds besides files, in words, in the order of `AppServices`' bits. Its own folder
/// is a grant of its own (described by [`describe`]), so it isn't repeated here.
pub fn services(s: AppServices) -> Vec<&'static str> {
    let names = [
        (AppServices::WINDOWS, "Windows"),
        (AppServices::PICKER, "The file picker"),
        (AppServices::SYSINFO, "System information"),
        (AppServices::TERMINAL, "The terminal"),
        (AppServices::USERS, "Your account and settings"),
        (AppServices::SANDBOX, "Launching and managing apps"),
    ];
    names.iter().filter(|(bit, _)| s.0 & bit.0 != 0).map(|(_, n)| *n).collect()
}

/// How long ago, in words: "just now", "5 seconds ago", "3 minutes ago", "2 hours ago".
pub fn ago(started_ns: u64, now_ns: u64) -> String {
    let s = now_ns.saturating_sub(started_ns) / 1_000_000_000;
    match s {
        0..=1 => "just now".to_string(),
        2..=119 => format!("{s} seconds ago"),
        120..=7199 => format!("{} minutes ago", s / 60),
        _ => format!("{} hours ago", s / 3600),
    }
}

/// What the window's widgets send.
#[derive(Clone, Debug)]
pub enum Message {
    /// An app picked in the list.
    Pick(usize),
    /// A row of what it holds picked.
    Row(usize),
    /// Take back the row picked.
    TakeBack,
    /// Stop the app picked.
    Stop,
    /// Look again (the timer, once a second, and after each change).
    Refresh,
    /// Close the window.
    Close,
}

/// One running app, as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    info: AppInfo,
    title: String,
}

/// The window's size.
pub fn settings() -> Settings {
    Settings { size: (640, 400), resizable: true, min_size: (480, 300), over: None, system: true }
}

/// The permissions viewer, over `A`.
pub struct Viewer<A: Apps> {
    apps_if: A,
    apps: Vec<Shown>,
    /// The app picked, by its number.
    app: Option<u32>,
    details: Option<AppDetails>,
    held: Vec<Held>,
    row: Option<usize>,
    /// What went wrong last.
    problem: Option<String>,
    /// The clock when the details were read (for "started … ago").
    now: u64,
    /// The changes it made, as lines for the log ("took /Home/x back from notepad").
    log: Vec<String>,
}

impl<A: Apps> Viewer<A> {
    /// The viewer, open on app `app` if given (the window menu's "What can this app reach?"),
    /// else on the first.
    pub fn new(apps_if: A, app: Option<u32>) -> Viewer<A> {
        let mut v = Viewer {
            apps_if,
            apps: Vec::new(),
            app,
            details: None,
            held: Vec::new(),
            row: None,
            problem: None,
            now: 0,
            log: Vec::new(),
        };
        v.refresh(0);
        v
    }

    /// The apps listed, by name.
    pub fn app_names(&self) -> Vec<&str> {
        self.apps.iter().map(|a| a.info.app.as_str()).collect()
    }

    /// The app shown.
    pub fn shown(&self) -> Option<u32> {
        self.app
    }

    /// What the app shown holds, in words.
    pub fn held(&self) -> &[Held] {
        &self.held
    }

    /// What went wrong last.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The changes made since the last call, for the log.
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Reads everything again. A different list of apps or grants is what tells it an app has
    /// started or ended, or a grant changed.
    fn refresh(&mut self, now: u64) {
        self.now = now;
        let list = match self.apps_if.list() {
            Ok(l) => l,
            Err(e) => {
                self.problem = Some(format!("The sandbox manager didn't answer: {e}"));
                return;
            }
        };
        let mut apps: Vec<Shown> = list
            .into_iter()
            .map(|info| {
                let title = self
                    .apps_if
                    .details(info.id)
                    .map(|d| d.title)
                    .unwrap_or_else(|_| info.app.clone());
                Shown { info, title }
            })
            .collect();
        apps.sort_by(|a, b| a.title.cmp(&b.title).then(a.info.id.cmp(&b.info.id)));
        self.apps = apps;
        // The app shown stays shown while it runs; one that has ended gives way to the first.
        if !self.app.is_some_and(|id| self.apps.iter().any(|a| a.info.id == id)) {
            self.app = self.apps.first().map(|a| a.info.id);
            self.row = None;
        }
        self.read_app();
    }

    /// Reads what the app shown holds.
    fn read_app(&mut self) {
        let Some(id) = self.app else {
            self.details = None;
            self.held.clear();
            return;
        };
        let details = self.apps_if.details(id).ok();
        let grants = self.apps_if.grants(id).unwrap_or_default();
        let name = details.as_ref().map(|d| d.app.clone()).unwrap_or_default();
        let mut held: Vec<Held> = grants
            .iter()
            .map(|g| Held { text: describe(&name, g), path: Some(g.path.clone()) })
            .collect();
        if let Some(d) = &details {
            held.extend(
                services(d.services).into_iter().map(|s| Held { text: s.to_string(), path: None }),
            );
        }
        // The row picked stays picked if it is still there.
        let picked = self.row.and_then(|r| self.held.get(r)).cloned();
        self.row = picked.and_then(|p| held.iter().position(|h| *h == p));
        self.held = held;
        self.details = details;
    }

    fn picked_index(&self) -> Option<usize> {
        let id = self.app?;
        self.apps.iter().position(|a| a.info.id == id)
    }

    // ---- The parts of the view. ----

    fn app_list(&self) -> Element<'_, Message> {
        let rows = self
            .apps
            .iter()
            .map(|a| {
                let mark = if a.info.trusted { "trusted" } else { "" };
                ListRow::new().cell(a.title.as_str()).cell(mark)
            })
            .collect();
        List::new(
            vec![ListColumn::new("App", Length::Fill), ListColumn::new("", Length::Fixed(64))],
            rows,
        )
        .selected(self.picked_index())
        .on_select(Message::Pick)
        .width(Length::Fixed(200))
        .height(Length::Fill)
        .id("apps")
    }

    fn details_view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut column = Column::new().spacing(t.spacing).width(Length::Fill).height(Length::Fill);
        match &self.details {
            None => {
                column = column.push(Label::new("No app is running."));
            }
            Some(d) => {
                column = column.push(Label::new(&d.title).heading().colour(t.strong).id("title"));
                let what = if d.trusted { "Trusted" } else { "Sandboxed" };
                let line = format!("{what} · started {} · app {}", ago(d.started, self.now), d.id);
                column = column.push(Label::new(&line).id("about"));
                let rows = self.held.iter().map(|h| ListRow::new().cell(h.text.as_str())).collect();
                column = column.push(
                    List::new(vec![ListColumn::new("What it can reach", Length::Fill)], rows)
                        .selected(self.row)
                        .on_select(Message::Row)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .id("held"),
                );
            }
        }
        column.into()
    }

    fn buttons(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let take = self
            .row
            .and_then(|r| self.held.get(r))
            .and_then(|h| h.path.as_ref())
            .map(|_| Message::TakeBack);
        let stop = self.details.as_ref().map(|_| Message::Stop);
        let mut row = Row::new().spacing(t.spacing).align(Align::Center).width(Length::Fill);
        row = match &self.problem {
            Some(p) => row.push(Label::new(p).id("problem")),
            None => row.push(Stretch::new()),
        };
        row.push(Button::new("Take back").on_press_maybe(take).id("takeback"))
            .push(Button::new("Stop app").on_press_maybe(stop).id("stop"))
            .push(Button::new("Close").on_press(Message::Close).cancel().id("close"))
            .into()
    }
}

impl<A: Apps> App for Viewer<A> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let body = Row::new()
            .spacing(t.padding)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(self.app_list())
            .push(self.details_view());
        Panel::new(
            Column::new()
                .padding(t.padding)
                .spacing(t.spacing)
                .width(Length::Fill)
                .height(Length::Fill)
                .push(body)
                .push(self.buttons()),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        self.refresh(cx.now_ms() * 1_000_000);
        cx.after(1000, Message::Refresh);
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        let now = cx.now_ms() * 1_000_000;
        match message {
            Message::Pick(i) => {
                if let Some(a) = self.apps.get(i) {
                    self.app = Some(a.info.id);
                    self.row = None;
                    self.problem = None;
                    self.read_app();
                }
            }
            Message::Row(i) => self.row = (i < self.held.len()).then_some(i),
            Message::TakeBack => {
                let (Some(id), Some(path)) = (
                    self.app,
                    self.row.and_then(|r| self.held.get(r)).and_then(|h| h.path.clone()),
                ) else {
                    return;
                };
                let name = self.details.as_ref().map(|d| d.app.clone()).unwrap_or_default();
                match self.apps_if.revoke(id, &path) {
                    Ok(()) => {
                        self.log.push(format!("took {path} back from {name}"));
                        self.problem = None;
                        self.row = None;
                    }
                    Err(e) => self.problem = Some(format!("Couldn't take it back: {e}")),
                }
                self.refresh(now);
            }
            Message::Stop => {
                let Some(id) = self.app else { return };
                let name = self.details.as_ref().map(|d| d.app.clone()).unwrap_or_default();
                match self.apps_if.stop(id) {
                    Ok(()) => {
                        self.log.push(format!("stopped {name}"));
                        self.problem = None;
                    }
                    Err(e) => self.problem = Some(format!("Couldn't stop it: {e}")),
                }
                self.refresh(now);
            }
            Message::Refresh => {
                self.refresh(now);
                cx.after(1000, Message::Refresh);
            }
            Message::Close => cx.quit(),
        }
    }
}

#[cfg(test)]
mod tests;
