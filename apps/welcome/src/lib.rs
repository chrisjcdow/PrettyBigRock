// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The Welcome window (docs/design/0024-the-session-desktop.md, section 6a), as a sandstone app
//! tested on the host.
//!
//! The desktop opens it at start until the user has answered it. It says where the user's
//! files are kept, and offers what fits:
//!
//! - **On a disk** (the *system volume*, the disk with `System/users.txt` that the system
//!   started from; note 0017, section 2): a user of one's own. **Make my user** adds an
//!   administrator who is logged in from the next start (`pbr.users.add_first`); **Carry on**
//!   keeps the user there is.
//! - **In memory** (no disk was set up, so files are gone after a restart): setting a disk up
//!   (`pbr.users.setup`), from the disks that could be used.
//!
//! Either answer sets the user's `welcome` setting to `done`, so the desktop doesn't open the
//! window again. On a volume in memory the settings are gone at the next start too, so there
//! the window comes back every start until a disk is set up: the reminder.
//!
//! It reaches the users service through the [`Users`] trait (a *trait* is a list of functions a
//! type promises to have), which the program implements over its `pbr.users` capability and
//! the tests with a pretend one. That capability may do only what this window needs (note
//! 0024, section 6a): it can't list users, format, check, or turn the computer off.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use pbr_interfaces::pbr_sysinfo as sysinfo;
use pbr_interfaces::pbr_users::{Filesystem, User, UserKind, UsersError, VolumeInfo, VolumeState};
use pbr_sandstone::{
    Align, App, Button, Column, Context, Element, Identify, Label, Length, List, ListColumn,
    ListRow, Panel, Row, Settings, Stretch, TextField, Theme,
};

/// Why a call failed: the users service refused (`Some`), or didn't answer (`None`).
pub type Failed = Option<UsersError>;

/// What the window asks of the users service and of `pbr.sysinfo`.
pub trait Users {
    /// Who the user is.
    fn whoami(&mut self) -> Result<User, Failed>;
    /// The volumes as `pbr.sysinfo` lists them, which says which is the system volume.
    fn system_volumes(&mut self) -> Result<Vec<sysinfo::VolumeInfo>, Failed>;
    /// The storage service's volumes, with the numbers `setup` takes.
    fn volumes(&mut self) -> Result<Vec<VolumeInfo>, Failed>;
    /// Makes `volume` the system volume from the next start, with `name` its administrator.
    fn setup(&mut self, volume: u32, name: &str) -> Result<(), Failed>;
    /// Adds an administrator who is logged in from the next start.
    fn add_first(&mut self, user: &User) -> Result<(), Failed>;
    /// Sets `welcome` to `done`.
    fn answered(&mut self) -> Result<(), Failed>;
    /// Restarts the computer. Returns only if it can't.
    fn restart(&mut self) -> Result<(), Failed>;
}

/// A size in words, in the units people see on disks: "256 MB", "2.5 GB".
pub fn size_text(bytes: u64) -> String {
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    if bytes >= 10 * GB {
        format!("{} GB", bytes / GB)
    } else if bytes >= GB {
        format!("{}.{} GB", bytes / GB, (bytes % GB) * 10 / GB)
    } else {
        format!("{} MB", bytes / MB)
    }
}

/// The user name and display name for what was typed in the name box: the display name is
/// what was typed, without spaces at either end; the user name is that in lower case, with
/// spaces as `-` ("Chris" is `chris`, "Mary Ann" is `mary-ann`). `None` if either breaks the
/// rules (note 0017, section 3: user names are 1 to 32 lower-case letters, digits, `-` and `_`;
/// display names have no `"`).
pub fn names(typed: &str) -> Option<(String, String)> {
    let display = typed.trim();
    let name: String =
        display.chars().map(|c| if c == ' ' { '-' } else { c.to_ascii_lowercase() }).collect();
    let user = pbr_manifest::users::User {
        name: name.clone(),
        kind: pbr_manifest::users::Kind::Admin,
        display: display.to_string(),
    };
    user.is_valid().then(|| (name, display.to_string()))
}

/// A disk `setup` could use, as offered on the in-memory page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The volume's number.
    pub id: u32,
    /// How it is shown: "Files, 256 MB, RockFS" or "256 MB, empty".
    pub text: String,
}

/// The disks `setup` could use: empty ones, and RockFS ones that are ready, that aren't in
/// memory or read-only. (A RockFS volume that is already a system volume is refused by `setup`
/// itself, which says so.) An empty whole disk and an empty partition on it would both be
/// listed; a disk with partitions lists only them.
pub fn offers(volumes: &[VolumeInfo]) -> Vec<Offer> {
    volumes
        .iter()
        .filter(|v| !v.in_memory && !v.read_only)
        .filter_map(|v| {
            let kind = match (v.filesystem, v.state) {
                (Filesystem::Empty, _) => "empty",
                (Filesystem::RockFs, VolumeState::Ready) => "RockFS",
                _ => return None,
            };
            let size = size_text(v.size);
            let text = if v.label.is_empty() {
                format!("{size}, {kind}")
            } else {
                format!("{}, {size}, {kind}", v.label)
            };
            Some(Offer { id: v.id, text })
        })
        .collect()
}

/// What the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    /// Files are on a disk: a user of one's own, or carry on.
    Disk {
        /// The disk's label.
        label: String,
    },
    /// Files are in memory: set a disk up, or not now.
    Memory {
        /// The disks that could be used.
        offers: Vec<Offer>,
    },
    /// Done: who is logged in from the next start, and Restart now.
    Done {
        /// Their display name.
        who: String,
    },
    /// The system volume couldn't be found out: what went wrong, and Close.
    Problem(String),
}

impl Page {
    /// The page's name in the log (`welcome: page NAME`).
    pub fn name(&self) -> &'static str {
        match self {
            Page::Disk { .. } => "disk",
            Page::Memory { .. } => "memory",
            Page::Done { .. } => "done",
            Page::Problem(_) => "problem",
        }
    }
}

/// What the window's widgets send.
#[derive(Clone, Debug)]
pub enum Message {
    /// The name box changed.
    Name(String),
    /// A disk picked in the list.
    Pick(usize),
    /// Make my user.
    Make,
    /// Set up the disk picked.
    SetUp,
    /// Carry on as the user there is, or not now: answered, and closed.
    CarryOn,
    /// Restart now.
    Restart,
    /// Close without restarting.
    Close,
}

/// The window's width, and the width its words wrap at (inside the padding).
const WIDTH: u32 = 520;
const TEXT_WIDTH: i32 = WIDTH as i32 - 2 * Theme::STANDARD.padding;

/// The window's size for the page it opens on: room for the list of disks on the in-memory
/// page, and only for the words and the name otherwise.
pub fn settings(page: &Page) -> Settings {
    let height = if matches!(page, Page::Memory { .. }) { 300 } else { 180 };
    Settings { size: (WIDTH, height), resizable: false, min_size: (0, 0), over: None, system: true }
}

/// Why `what` failed, in words, for the window.
fn failed_words(what: &str, e: &Failed) -> String {
    match e {
        Some(UsersError::AlreadyExists) => format!("{what}: that name or disk is taken already."),
        Some(UsersError::BadName) => format!("{what}: that name can't be used."),
        Some(UsersError::NotAllowed) => format!("{what}: only an administrator can."),
        Some(UsersError::Unsuitable) => format!("{what}: that disk can't be used."),
        Some(UsersError::NotReady) => format!("{what}: the disk isn't ready yet. Try again."),
        Some(UsersError::TooMany) => format!("{what}: there are as many users as there can be."),
        Some(e) => format!("{what}: it failed ({e:?})."),
        None => format!("{what}: the users service didn't answer."),
    }
}

/// The Welcome window, over `U`.
pub struct Welcome<U: Users> {
    users: U,
    /// Who is using it.
    me: Option<User>,
    page: Page,
    /// What is in the name box.
    name: String,
    /// The disk picked on the in-memory page.
    picked: Option<usize>,
    /// What went wrong last.
    problem: Option<String>,
    /// What it did, as lines for the log ("added dana").
    log: Vec<String>,
}

impl<U: Users> Welcome<U> {
    /// The window, with the page found from what the users service says.
    pub fn new(mut users: U) -> Welcome<U> {
        let me = users.whoami().ok();
        let page = match users.system_volumes() {
            Err(e) => Page::Problem(failed_words("Finding where your files are", &e)),
            Ok(list) => match list.iter().find(|v| v.system) {
                Some(v) if !v.in_memory => Page::Disk { label: v.label.clone() },
                // No system volume (sysinfo lists only what it may: none is the same as one
                // in memory here, since the users service always has one or the other).
                _ => {
                    Page::Memory { offers: users.volumes().map(|l| offers(&l)).unwrap_or_default() }
                }
            },
        };
        let mut w = Welcome {
            users,
            me,
            page,
            name: String::new(),
            picked: None,
            problem: None,
            log: Vec::new(),
        };
        if let Page::Memory { offers } = &w.page {
            w.picked = (offers.len() == 1).then_some(0);
        }
        w.log.push(format!("page {}", w.page.name()));
        w
    }

    /// Whether there's nothing to ask: a standard user can't add users or set up disks.
    pub fn nothing_to_ask(&self) -> bool {
        self.me.as_ref().is_some_and(|u| u.kind == UserKind::Standard)
    }

    /// Answers for a standard user, to whom the program shows no window
    /// ([`Welcome::nothing_to_ask`]).
    pub fn answer_for_standard_user(&mut self) {
        self.log.push("nothing to ask a standard user".to_string());
        self.answer();
    }

    /// The page shown.
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// What went wrong last.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The lines for the log since the last call.
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// The display name of the user there is ("Owner").
    fn display(&self) -> &str {
        self.me.as_ref().map_or("this user", |u| u.display.as_str())
    }

    /// Records that the window has been answered. A failure is only logged: the window then
    /// comes back at the next start, which is no harm.
    fn answer(&mut self) {
        match self.users.answered() {
            Ok(()) => self.log.push("answered".to_string()),
            Err(e) => self.log.push(failed_words("saving the answer", &e)),
        }
    }

    fn make(&mut self) {
        let Some((name, display)) = names(&self.name) else {
            self.problem = Some(
                "A name needs letters or digits, and may have spaces, - and _ (no \").".to_string(),
            );
            return;
        };
        let user = User { name: name.clone(), kind: UserKind::Admin, display: display.clone() };
        match self.users.add_first(&user) {
            Ok(()) => {
                self.log.push(format!("added {name}"));
                self.answer();
                self.problem = None;
                self.page = Page::Done { who: display };
                self.log.push(format!("page {}", self.page.name()));
            }
            Err(e) => self.problem = Some(failed_words("Making your user", &e)),
        }
    }

    fn set_up(&mut self) {
        let Page::Memory { offers } = &self.page else { return };
        let Some(offer) = self.picked.and_then(|i| offers.get(i)).cloned() else {
            self.problem = Some("Pick a disk first.".to_string());
            return;
        };
        let Some((name, display)) = names(&self.name) else {
            self.problem = Some(
                "A name needs letters or digits, and may have spaces, - and _ (no \").".to_string(),
            );
            return;
        };
        match self.users.setup(offer.id, &name) {
            Ok(()) => {
                self.log.push(format!("set up volume {} for {name}", offer.id));
                self.answer();
                self.problem = None;
                self.page = Page::Done { who: display };
                self.log.push(format!("page {}", self.page.name()));
            }
            Err(e) => self.problem = Some(failed_words("Setting the disk up", &e)),
        }
    }

    // ---- The parts of the view. ----

    fn name_row(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        Row::new()
            .spacing(t.spacing)
            .align(Align::Center)
            .width(Length::Fill)
            .push(Label::new("Your name:"))
            .push(
                TextField::new(&self.name)
                    .on_input(Message::Name)
                    .on_submit(match self.page {
                        Page::Memory { .. } => Message::SetUp,
                        _ => Message::Make,
                    })
                    .width(Length::Fill)
                    .id("name"),
            )
            .into()
    }

    fn words(&self) -> String {
        match &self.page {
            Page::Disk { label } => format!(
                "Your files are saved on the disk \"{label}\", so they'll still be here after a \
                 restart. You're using it as {}. Would you like a user of your own?",
                self.display()
            ),
            Page::Memory { offers } if offers.is_empty() => "PrettyBigRock isn't set up on a \
                 disk yet, so your files are kept in memory and will be gone after a restart. \
                 There's no disk it could use: run-prettybigrock.bat attaches one, data.img."
                .to_string(),
            Page::Memory { .. } => "PrettyBigRock isn't set up on a disk yet, so your files are \
                 kept in memory and will be gone after a restart. Set it up on a disk now?"
                .to_string(),
            Page::Done { who } => format!(
                "Done. From the next start you're {who}. Restart now, or carry on and restart \
                 later from the start menu."
            ),
            Page::Problem(p) => p.clone(),
        }
    }

    fn buttons(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut row = Row::new().spacing(t.spacing).align(Align::Center).width(Length::Fill);
        row = match &self.problem {
            Some(p) => row
                .push(Label::new(p).wrap().max_width(TEXT_WIDTH / 2).colour(t.error).id("problem")),
            None => row.push(Stretch::new()),
        };
        match &self.page {
            Page::Disk { .. } => {
                let carry = format!("Carry on as {}", self.display());
                row.push(Button::new(&carry).on_press(Message::CarryOn).cancel().id("carry"))
                    .push(Button::new("Make my user").on_press(Message::Make).default().id("make"))
            }
            Page::Memory { offers } => {
                let row = row
                    .push(Button::new("Not now").on_press(Message::CarryOn).cancel().id("notnow"));
                // With no disk to set up, there is nothing to press but Not now.
                if offers.is_empty() {
                    row
                } else {
                    let set_up = self.picked.map(|_| Message::SetUp);
                    row.push(Button::new("Set up").on_press_maybe(set_up).default().id("setup"))
                }
            }
            Page::Done { .. } => {
                row.push(Button::new("Later").on_press(Message::Close).cancel().id("later")).push(
                    Button::new("Restart now").on_press(Message::Restart).default().id("restart"),
                )
            }
            Page::Problem(_) => {
                row.push(Button::new("Close").on_press(Message::Close).cancel().id("close"))
            }
        }
        .into()
    }
}

impl<U: Users> App for Welcome<U> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let mut column = Column::new()
            .padding(t.padding)
            .spacing(t.spacing)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(Label::new("Welcome to PrettyBigRock").heading().colour(t.strong).id("heading"))
            .push(Label::new(&self.words()).wrap().max_width(TEXT_WIDTH).id("words"));
        match &self.page {
            Page::Disk { .. } => column = column.push(self.name_row()),
            Page::Memory { offers } if !offers.is_empty() => {
                let rows = offers.iter().map(|o| ListRow::new().cell(o.text.as_str())).collect();
                column = column
                    .push(
                        List::new(vec![ListColumn::new("Disk", Length::Fill)], rows)
                            .selected(self.picked)
                            .on_select(Message::Pick)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .id("disks"),
                    )
                    .push(self.name_row());
            }
            _ => {}
        }
        column = column.push(Stretch::new()).push(self.buttons());
        Panel::new(column).width(Length::Fill).height(Length::Fill).into()
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        if matches!(self.page, Page::Disk { .. } | Page::Memory { .. }) {
            cx.focus("name");
        }
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Name(n) => self.name = n,
            Message::Pick(i) => {
                if let Page::Memory { offers } = &self.page {
                    self.picked = (i < offers.len()).then_some(i);
                }
            }
            Message::Make => {
                if matches!(self.page, Page::Disk { .. }) {
                    self.make();
                }
            }
            Message::SetUp => self.set_up(),
            Message::CarryOn => {
                self.answer();
                cx.quit();
            }
            Message::Restart => {
                self.log.push("restarting".to_string());
                if let Err(e) = self.users.restart() {
                    self.problem = Some(failed_words("Restarting", &e));
                }
            }
            Message::Close => cx.quit(),
        }
    }
}

#[cfg(test)]
mod tests;
