// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Asking before turning the computer off or restarting it
//! (docs/design/0024-the-session-desktop.md, section 7, and its Changes).
//!
//! Both stop every app, so anything not saved in one is lost: the start menu's Shut down and
//! Restart open this small window first, and only its button goes on. It is a sandstone app
//! (note 0023), so it looks and works as every dialog does: Enter chooses the default button,
//! Escape or closing the window cancels, Tab moves between the two. The desktop isn't a
//! sandstone app itself (it draws its wallpaper, taskbar and menu with `lib/draw`), so it runs
//! this one in its own event loop while the window is open (`apps/desktop`).

use alloc::string::String;
use alloc::vec::Vec;
use pbr_sandstone::{
    App, Button, Column, Context, Element, Identify, Label, Length, Row, Settings, Space, Stretch,
    Theme,
};

/// What the user is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Turning the computer off.
    ShutDown,
    /// Starting it again.
    Restart,
}

impl Power {
    /// The start menu's word for it, which is also the button that goes on.
    pub fn verb(self) -> &'static str {
        match self {
            Power::ShutDown => "Shut down",
            Power::Restart => "Restart",
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Power::ShutDown => {
                "Every app will be closed, and anything not saved in them will be lost. \
                 Your files are saved, then the computer turns off."
            }
            Power::Restart => {
                "Every app will be closed, and anything not saved in them will be lost. \
                 Your files are saved, then the computer starts again."
            }
        }
    }
}

/// What the window's buttons send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// Go on: shut down or restart.
    Yes,
    /// Don't.
    No,
}

/// The window's size, in logical pixels (as sandstone's dialogs: text 360 wide, 16 round it).
pub fn settings() -> Settings {
    Settings { size: (392, 160), resizable: false, min_size: (0, 0), over: None, system: false }
}

/// The question, and the answer once given.
pub struct Confirm {
    power: Power,
    answer: Option<bool>,
    log: Vec<String>,
}

impl Confirm {
    /// Asks about `power`.
    pub fn new(power: Power) -> Confirm {
        Confirm { power, answer: None, log: Vec::new() }
    }

    /// What it asks about.
    pub fn power(&self) -> Power {
        self.power
    }

    /// `Some(true)` once the user chose to go on, `Some(false)` once they chose not to; `None`
    /// while the window is open (closing it is "no": sandstone then ends the app with no
    /// message, and the desktop takes that as no).
    pub fn answer(&self) -> Option<bool> {
        self.answer
    }
}

impl App for Confirm {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let t = Theme::STANDARD;
        let verb = self.power.verb();
        let buttons = Row::new()
            .spacing(8)
            .width(Length::Fill)
            .push(Stretch::new())
            .push(Button::new("Cancel").on_press(Message::No).cancel().width(Length::Fixed(96)))
            .push(
                Button::new(verb)
                    .on_press(Message::Yes)
                    .default()
                    .width(Length::Fixed(96))
                    .id("yes"),
            );
        Column::new()
            .padding(16)
            .spacing(10)
            .push(Label::new(&alloc::format!("{verb}?")).style(t.heading).colour(t.strong))
            .push(Label::new(self.power.explanation()).wrap().max_width(360))
            .push(Space::new(0, 4))
            .push(buttons)
            .into()
    }

    fn title(&self) -> String {
        String::from(self.power.verb())
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        // The button that goes on has the focus, so Enter (or Space) chooses it at once; the
        // user has already chosen it once in the start menu.
        cx.focus("yes");
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        self.answer = Some(message == Message::Yes);
        self.log.push(alloc::format!(
            "{} {}",
            self.power.verb().to_lowercase(),
            if message == Message::Yes { "confirmed" } else { "cancelled" }
        ));
        cx.quit();
    }
}

impl Confirm {
    /// What happened since last asked, for the desktop's log (`shut down confirmed`).
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }
}
