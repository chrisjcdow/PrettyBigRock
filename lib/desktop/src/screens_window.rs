// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The Screens window: arranging the screens, from the desktop's right-click menu
//! (docs/design/0024-the-session-desktop.md, section 6).
//!
//! It shows every screen the display server has with its size, and for the one picked: whether
//! it is the main screen, its scale (1x or 2x), its rotation, which screen it shows the same as
//! (if any), and where it is in the order from left to right. Below are the pointer's speed
//! and, for each tablet known, which screen it points at. **Apply** asks the display server for
//! all of it; **OK** does so and closes the window; **Cancel**, Escape and closing the window
//! change nothing: if Apply was used first, the desktop puts back what was there when the
//! window opened.
//!
//! This is a sandstone app (note 0023), run by the desktop in its own event loop while the
//! window is open, as `confirm` is. An *app* in sandstone is a type that describes its window
//! in [`App::view`] from its state, and changes that state in [`App::update`] when the user
//! does something. It can't call the display server (it only has a window), so Apply and OK
//! leave a [`Pending`] request, which the desktop takes with [`ScreensWindow::take_request`],
//! makes the calls for, and answers with [`ScreensWindow::accepted`] or
//! [`ScreensWindow::refused`]. The desktop then gives the window an event
//! ([`SETTLED`], which arrives at [`App::notified`]) so that it draws again and, after OK,
//! closes. What the choices mean and which are allowed is [`crate::arrange`]'s, tested there.

use crate::arrange::{Plan, Problem, ROTATIONS, Request, SCALES};
use alloc::string::String;
use alloc::vec::Vec;
use pbr_interfaces::pbr_window::ScreenPlace;
use pbr_sandstone::{
    Align, App, Button, Column, Context, Element, Identify, Label, Length, List, ListColumn,
    ListRow, Row, Scroll, Settings, Space, Stretch, Theme, ToggleButton,
};

/// What the desktop sends the window (as bit 8 of a notification, the first bit an app may use)
/// after it has answered a request, so the window draws what happened.
pub const SETTLED: u64 = 1 << 8;

/// The window's size, in logical pixels: it fits a 2x screen of 1024 x 768 (512 x 384 logical)
/// with the taskbar and the title bar. It is resizable, and scrolls if it is made too small to
/// show everything.
pub fn settings() -> Settings {
    Settings { size: (440, 316), resizable: true, min_size: (400, 260), over: None, system: false }
}

/// What the buttons and choices send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// Row `n` of the list of screens was picked.
    Pick(usize),
    /// The picked screen is to be the main one.
    Main,
    /// The picked screen's scale: 100 or 200.
    Scale(u32),
    /// The picked screen's rotation, in degrees.
    Turn(u32),
    /// The picked screen goes on to the next choice of what it shows.
    Mirror,
    /// The picked screen moves left, or right, in the order.
    Left,
    /// See [`Message::Left`].
    Right,
    /// The pointer a step slower, or faster.
    Slower,
    /// See [`Message::Slower`].
    Faster,
    /// Tablet `n` goes on to the next screen it can point at.
    Tie(usize),
    /// Ask for what is chosen.
    Apply,
    /// Ask for what is chosen, and close.
    Ok,
    /// Close, changing nothing more.
    Cancel,
    /// The desktop has answered: draw what it said, and close if the user chose OK.
    Settled,
}

/// What is shown under the choices.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    /// Nothing to say.
    Quiet,
    /// Something went through.
    Done(String),
    /// Something didn't, and why.
    Failed(String),
}

/// What Apply or OK asks the desktop to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// What to ask the display server for, and keep.
    pub request: Request,
    /// The user chose OK: close the window if it goes through.
    pub close: bool,
}

/// The Screens window's state.
pub struct ScreensWindow {
    plan: Plan,
    /// The number of the screen picked in the list.
    picked: u32,
    status: Status,
    pending: Option<Pending>,
    /// Apply or OK has gone through at least once, so Cancel has something to put back.
    applied: bool,
    /// OK went through: the window closes.
    done: bool,
    log: Vec<String>,
}

impl ScreensWindow {
    /// The window for the screens as they are now (`places`, from the display server), the
    /// pointer's speed and the tablets known as (name, identity of the screen it is tied to),
    /// as the desktop's `screens.txt` has them.
    pub fn new(places: &[ScreenPlace], speed: Option<u32>, tablets: &[(String, String)]) -> Self {
        let plan = Plan::new(places, speed, tablets);
        let picked = plan.screens().iter().find(|s| s.main).map_or(0, |s| s.number);
        ScreensWindow {
            plan,
            picked,
            status: Status::Quiet,
            pending: None,
            applied: false,
            done: false,
            log: Vec::new(),
        }
    }

    /// What has been chosen, and how things were when the window opened.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The number of the screen picked.
    pub fn picked(&self) -> u32 {
        self.picked
    }

    /// What Apply or OK asked for since this was last called.
    pub fn take_request(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// The display server took what was asked for (`screens` of them), and the desktop kept it
    /// (`saved` is why it couldn't, if so). After OK (`close`) the window closes, unless it
    /// couldn't be kept: then the user is told.
    pub fn accepted(&mut self, close: bool, screens: usize, saved: Result<(), String>) {
        self.applied = true;
        match saved {
            Ok(()) => {
                self.status = Status::Done(alloc::format!(
                    "Applied to {screens} {}.",
                    if screens == 1 { "screen" } else { "screens" }
                ));
                self.done = close;
            }
            Err(why) => {
                self.status = Status::Failed(alloc::format!(
                    "Applied, but it couldn't be kept for next time: {why}"
                ));
            }
        }
    }

    /// The display server refused what was asked for, with `why`: nothing changed.
    pub fn refused(&mut self, why: &str) {
        self.status = Status::Failed(alloc::format!("The display server refused it: {why}"));
    }

    /// Whether Apply or OK has gone through, so Cancel has something to put back.
    pub fn has_applied(&self) -> bool {
        self.applied
    }

    /// Whether OK went through: the window is to close.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// What the choices come to, one line for each screen, for the desktop's log (the test
    /// runner reads it): `screen 2 Virtual-2 at 1280,0 scale 200 rotation 0 mirrors 0`.
    pub fn plan_lines(&self) -> Vec<String> {
        self.plan
            .places()
            .iter()
            .map(|p| {
                alloc::format!(
                    "screen {} {} at {},{} scale {} rotation {} mirrors {}{}",
                    p.screen,
                    self.plan.name(p.screen),
                    p.x,
                    p.y,
                    p.scale,
                    p.rotation,
                    p.mirrors,
                    if p.main { " main" } else { "" }
                )
            })
            .collect()
    }

    /// What happened since last asked, for the desktop's log.
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Says why a choice wasn't made, or clears what was said before.
    fn settle_choice(&mut self, result: Result<(), Problem>) {
        self.status = match result {
            Ok(()) => Status::Quiet,
            Err(problem) => Status::Failed(String::from(problem.explain())),
        };
    }
}

impl ScreensWindow {
    /// The list of screens: one row each, in their order.
    fn screens_list(&self) -> Element<'_, Message> {
        let rows = self
            .plan
            .screens()
            .iter()
            .map(|s| {
                let role = if s.main {
                    String::from("Main")
                } else if s.mirrors != 0 {
                    alloc::format!("Shows {}", self.plan.name(s.mirrors))
                } else {
                    String::new()
                };
                ListRow::new()
                    .cell(s.label())
                    .cell(alloc::format!("{}x", s.scale / 100))
                    .cell(alloc::format!("{}", s.rotation))
                    .cell(role)
            })
            .collect();
        let picked = self.plan.screens().iter().position(|s| s.number == self.picked);
        List::new(
            alloc::vec![
                ListColumn::new("Screen", Length::Fill),
                ListColumn::new("Scale", Length::Fixed(48)),
                ListColumn::new("Turn", Length::Fixed(44)),
                ListColumn::new("Role", Length::Fixed(104)),
            ],
            rows,
        )
        .selected(picked)
        .on_select(Message::Pick)
        .height(Length::Fixed(86))
        .id("screens")
    }

    /// A row of choices with a label before them.
    fn labelled<'a>(label: &str, width: i32) -> Row<'a, Message> {
        Row::new()
            .spacing(Theme::STANDARD.spacing)
            .align(Align::Center)
            .push(Label::new(label).width(Length::Fixed(width)))
    }

    /// The choices for the screen picked.
    fn choices(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let Some(screen) = self.plan.screen(self.picked) else {
            return Column::new().into();
        };
        let number = screen.number;
        let narrow = Length::Fixed(44);

        let main = ToggleButton::new(screen.main)
            .label("Main screen")
            .on_toggle(|_| Message::Main)
            .width(Length::Fixed(104))
            .id("main");
        let left = Button::new("Move left")
            .on_press_maybe(self.plan.can_move_left(number).then_some(Message::Left))
            .width(Length::Fixed(88))
            .id("left");
        let right = Button::new("Move right")
            .on_press_maybe(self.plan.can_move_right(number).then_some(Message::Right))
            .width(Length::Fixed(88))
            .id("right");
        let first =
            Row::new().spacing(t.spacing).push(main).push(Space::new(8, 0)).push(left).push(right);

        let mut second = Self::labelled("Scale", 36);
        for scale in SCALES {
            second = second.push(
                ToggleButton::new(screen.scale == scale)
                    .label(&alloc::format!("{}x", scale / 100))
                    .on_toggle(move |_| Message::Scale(scale))
                    .width(narrow),
            );
        }
        second = second.push(Space::new(8, 0)).push(Label::new("Turn").width(Length::Fixed(32)));
        for turn in ROTATIONS {
            second = second.push(
                ToggleButton::new(screen.rotation == turn)
                    .label(&alloc::format!("{turn}"))
                    .on_toggle(move |_| Message::Turn(turn))
                    .width(narrow),
            );
        }

        let shows = match screen.mirrors {
            0 => String::from("Its own picture"),
            m => alloc::format!("Same picture as {}", self.plan.name(m)),
        };
        let blocker = self.plan.mirror_blocker(number);
        let third = Self::labelled("Shows", 36)
            .push(
                Button::new(&shows)
                    .on_press_maybe(blocker.is_none().then_some(Message::Mirror))
                    .width(Length::Fixed(176))
                    .id("mirror"),
            )
            .push_maybe(
                blocker.map(|b| Label::new(&alloc::format!("({})", b.short())).colour(t.text_dim)),
            );

        let speed = Self::labelled("Pointer", 52)
            .push(
                Button::new("Slower")
                    .on_press_maybe(
                        (self.plan.speed() > crate::arrange::SPEED_MIN).then_some(Message::Slower),
                    )
                    .width(Length::Fixed(72))
                    .id("slower"),
            )
            .push(
                Label::new(&alloc::format!("{}%", self.plan.speed()))
                    .width(Length::Fixed(40))
                    .align(Align::Center),
            )
            .push(
                Button::new("Faster")
                    .on_press_maybe(
                        (self.plan.speed() < crate::arrange::SPEED_MAX).then_some(Message::Faster),
                    )
                    .width(Length::Fixed(72))
                    .id("faster"),
            );

        let mut column =
            Column::new().spacing(t.spacing).push(first).push(second).push(third).push(speed);
        for (n, tablet) in self.plan.tablets().iter().enumerate() {
            column = column.push(
                Row::new()
                    .spacing(t.spacing)
                    .align(Align::Center)
                    .push(Label::new(&tablet.name).width(Length::Fill))
                    .push(Label::new("points at").colour(t.text_dim))
                    .push(
                        Button::new(&self.plan.tie_label(n))
                            .on_press(Message::Tie(n))
                            .width(Length::Fixed(136)),
                    ),
            );
        }
        column.into()
    }
}

impl App for ScreensWindow {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let (text, colour) = match &self.status {
            Status::Quiet => (String::new(), t.text_dim),
            Status::Done(text) => (text.clone(), t.text_dim),
            Status::Failed(text) => (text.clone(), t.error),
        };
        let top = Column::new()
            .spacing(t.spacing + 2)
            .width(Length::Fill)
            .push(self.screens_list())
            .push(self.choices());
        let buttons = Row::new()
            .spacing(8)
            .width(Length::Fill)
            .push(Stretch::new())
            .push(
                Button::new("Apply").on_press(Message::Apply).width(Length::Fixed(80)).id("apply"),
            )
            .push(Button::new("Cancel").on_press(Message::Cancel).cancel().width(Length::Fixed(80)))
            .push(
                Button::new("OK").on_press(Message::Ok).default().width(Length::Fixed(80)).id("ok"),
            );
        Column::new()
            .padding(12)
            .spacing(8)
            .push(Scroll::new(top).vertical().width(Length::Fill).height(Length::Fill))
            .push(Label::new(&text).colour(colour).wrap().id("status"))
            .push(buttons)
            .into()
    }

    fn title(&self) -> String {
        String::from("Screens")
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        // The default button has the keyboard, so Enter is OK as in any dialog.
        cx.focus("ok");
    }

    fn notified(&self, bits: u64) -> Option<Message> {
        (bits & SETTLED != 0).then_some(Message::Settled)
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        let number = self.picked;
        match message {
            Message::Pick(row) => {
                if let Some(s) = self.plan.screens().get(row) {
                    self.picked = s.number;
                }
            }
            Message::Main => {
                let r = self.plan.set_main(number);
                self.settle_choice(r);
            }
            Message::Scale(scale) => {
                let r = self.plan.set_scale(number, scale);
                self.settle_choice(r);
            }
            Message::Turn(turn) => {
                let r = self.plan.set_rotation(number, turn);
                self.settle_choice(r);
            }
            Message::Mirror => {
                let r = self.plan.cycle_mirror(number);
                self.settle_choice(r);
            }
            Message::Left => {
                let r = self.plan.move_left(number).map(|_| ());
                self.settle_choice(r);
            }
            Message::Right => {
                let r = self.plan.move_right(number).map(|_| ());
                self.settle_choice(r);
            }
            Message::Slower => {
                self.plan.slower();
                self.status = Status::Quiet;
            }
            Message::Faster => {
                self.plan.faster();
                self.status = Status::Quiet;
            }
            Message::Tie(n) => {
                let r = self.plan.cycle_tablet(n);
                self.settle_choice(r);
            }
            Message::Apply | Message::Ok => match self.plan.request() {
                Ok(request) => {
                    self.pending = Some(Pending { request, close: message == Message::Ok });
                }
                Err(problem) => self.settle_choice(Err(problem)),
            },
            Message::Cancel => {
                self.log.push(String::from("cancelled"));
                cx.quit();
            }
            Message::Settled => {
                if self.done {
                    cx.quit();
                }
            }
        }
    }
}
