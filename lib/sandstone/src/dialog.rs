// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Dialogs: a message, a question with buttons, or a line of text to type, each in a `Dialog`
//! window over the app's main window (note 0023, section 5; 1.3's `wm_dialog` and
//! `wm_message` in `classic/kernel/dialog.c`).
//!
//! An app makes one with [`Dialog`] and shows it with [`Context::show`](crate::Context::show);
//! sandstone owns its window and widgets, and sends the app the message of the button the user
//! chose. Enter chooses the default button, Escape (or the window's close button) the cancel
//! button.

use crate::Length;
use crate::element::Element;
use crate::event::Role;
use crate::theme::Theme;
use crate::widget::{Button, Column, Label, Row, Space, Stretch};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A button of a dialog.
pub(crate) struct DialogButton<M> {
    pub label: String,
    /// What it sends (none: it only closes the dialog).
    pub message: Option<M>,
    pub role: Option<Role>,
    /// For a dialog with a line to type: this button sends the text.
    pub submits: bool,
}

/// A dialog's line of text to type, and what sends it.
pub(crate) struct DialogInput<M> {
    pub value: String,
    pub submit: Box<dyn Fn(String) -> M>,
}

/// A dialog, as the app describes it.
pub struct Dialog<M> {
    pub(crate) title: String,
    pub(crate) heading: Option<String>,
    pub(crate) message: String,
    pub(crate) buttons: Vec<DialogButton<M>>,
    pub(crate) input: Option<DialogInput<M>>,
}

/// What the user did in a dialog: the dialog's own messages, which sandstone turns into the
/// app's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Button `n` was chosen.
    Button(usize),
    /// The line to type changed.
    #[expect(dead_code, reason = "sent by the text field, which comes with its own brief")]
    Text(String),
}

impl<M> Dialog<M> {
    fn new(title: &str, message: &str) -> Dialog<M> {
        Dialog {
            title: title.to_string(),
            heading: None,
            message: message.to_string(),
            buttons: Vec::new(),
            input: None,
        }
    }

    /// A message with an OK button, which only closes it.
    pub fn message(title: &str, message: &str) -> Dialog<M> {
        let mut d = Dialog::new(title, message);
        d.buttons.push(DialogButton {
            label: "OK".to_string(),
            message: None,
            role: Some(Role::Default),
            submits: false,
        });
        d
    }

    /// A question: give it buttons with [`Dialog::button`], [`Dialog::default_button`] and
    /// [`Dialog::cancel_button`].
    pub fn question(title: &str, message: &str) -> Dialog<M> {
        Dialog::new(title, message)
    }

    /// A line of text to type, starting as `initial` (selected, so typing replaces it), with
    /// OK, which sends `submit(text)`, and Cancel, which only closes it. `'static`: the
    /// function may not borrow anything, since the dialog outlives the `update` that made it.
    pub fn input(
        title: &str,
        prompt: &str,
        initial: &str,
        submit: impl Fn(String) -> M + 'static,
    ) -> Dialog<M> {
        let mut d = Dialog::new(title, prompt);
        d.input = Some(DialogInput { value: initial.to_string(), submit: Box::new(submit) });
        d.buttons.push(DialogButton {
            label: "Cancel".to_string(),
            message: None,
            role: Some(Role::Cancel),
            submits: false,
        });
        d.buttons.push(DialogButton {
            label: "OK".to_string(),
            message: None,
            role: Some(Role::Default),
            submits: true,
        });
        d
    }

    /// A heading over the message, in the theme's heading style.
    pub fn heading(mut self, heading: &str) -> Dialog<M> {
        self.heading = Some(heading.to_string());
        self
    }

    /// Adds a button that sends `message`. Buttons appear left to right in the order added.
    pub fn button(self, label: &str, message: M) -> Dialog<M> {
        self.add(label, Some(message), None)
    }

    /// Adds the button Enter chooses.
    pub fn default_button(self, label: &str, message: M) -> Dialog<M> {
        self.add(label, Some(message), Some(Role::Default))
    }

    /// Adds the button Escape (and closing the window) chooses.
    pub fn cancel_button(self, label: &str, message: M) -> Dialog<M> {
        self.add(label, Some(message), Some(Role::Cancel))
    }

    fn add(mut self, label: &str, message: Option<M>, role: Option<Role>) -> Dialog<M> {
        self.buttons.push(DialogButton { label: label.to_string(), message, role, submits: false });
        self
    }

    /// The window's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The button closing the window stands for: the cancel button, if there is one.
    pub(crate) fn cancel(&self) -> Option<usize> {
        self.buttons.iter().position(|b| b.role == Some(Role::Cancel))
    }

    /// What choosing button `n` sends the app, if anything.
    pub(crate) fn answer(&self, n: usize) -> Option<M>
    where
        M: Clone,
    {
        let b = self.buttons.get(n)?;
        if b.submits {
            return self.input.as_ref().map(|i| (i.submit)(i.value.clone()));
        }
        b.message.clone()
    }
}

/// How wide a dialog's text may be before it wraps.
const TEXT_WIDTH: i32 = 360;
/// A dialog's buttons, as 1.3's: 96 by 26.
const BUTTON_WIDTH: i32 = 96;

/// The dialog's window contents.
pub(crate) fn view<'a, M>(d: &'a Dialog<M>, theme: &Theme) -> Element<'a, Answer> {
    let mut column = Column::new().padding(16).spacing(10);
    if let Some(h) = &d.heading {
        column = column.push(Label::new(h).style(theme.heading).colour(theme.strong));
    }
    if !d.message.is_empty() {
        column = column.push(Label::new(&d.message).wrap().max_width(TEXT_WIDTH));
    }
    // The line to type: a text field once sandstone has one (M6 stage 7's text field brief);
    // until then the value is shown.
    if let Some(input) = &d.input {
        column = column.push(Label::new(&input.value));
    }
    let mut buttons = Row::new().spacing(8).push(Stretch::new());
    for (i, b) in d.buttons.iter().enumerate() {
        let mut button =
            Button::new(&b.label).on_press(Answer::Button(i)).width(Length::Fixed(BUTTON_WIDTH));
        button = match b.role {
            Some(Role::Default) => button.default(),
            Some(Role::Cancel) => button.cancel(),
            None => button,
        };
        buttons = buttons.push(button);
    }
    column.push(Space::new(0, 4)).push(buttons.width(Length::Fill)).into()
}
