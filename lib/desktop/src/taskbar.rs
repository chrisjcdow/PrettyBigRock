// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The taskbar along the bottom of the main screen (1.3's `draw_taskbar` and `taskbar_click`):
//! the start button on the left, a button for each window, and the clock on the right.
//!
//! The drawing and the hit tests share their numbers ([`Taskbar::start_rect`] and the other
//! `_rect` methods), so a click always lands on what was drawn.

use crate::icons::{self, Icon};
use crate::look;
use alloc::format;
use alloc::string::String;
use pbr_draw::{Canvas, Colour, Rect, TextStyle};

/// 1.3's `TASKBAR_H`: the taskbar's height in logical pixels.
pub const HEIGHT: i32 = 38;

/// The start button's size and place (1.3's `START_W`, and 6 pixels in from the left edge).
const START_X: i32 = 6;
const START_W: i32 = 132;
/// Where the first window button goes (1.3's `TB_X`).
const FIRST_BUTTON_X: i32 = START_W + 16;
/// A window button at its widest, and the gap between two.
const BUTTON_W: i32 = 160;
const GAP: i32 = 6;
/// The room the clock takes at the right edge, and the part of it the clock itself fills.
const CLOCK_ROOM: i32 = 120;
const CLOCK_X_FROM_RIGHT: i32 = 104;
const CLOCK_W: i32 = 98;

/// One window's button.
#[derive(Clone, Copy, Debug)]
pub struct TaskButton<'a> {
    /// The window's icon.
    pub icon: Icon,
    /// The window's title.
    pub title: &'a str,
    /// It is the window with the focus: drawn lightest, with a line under it.
    pub focused: bool,
    /// It is minimised: drawn dimmer.
    pub minimised: bool,
    /// Wants attention: drawn in the accent colour, so that it stands out (1.3 had no such
    /// state). The focused underline, if there is one, turns white to keep showing.
    pub attention: bool,
    /// Shown with " (not responding)" after its title.
    pub not_responding: bool,
}

/// A part of the taskbar a click can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The start button.
    Start,
    /// Window button number `n`.
    Button(usize),
    /// The clock.
    Clock,
}

/// Everything the taskbar shows.
#[derive(Clone, Copy, Debug)]
pub struct Taskbar<'a> {
    /// Logical width (the screen's).
    pub width: i32,
    /// The start menu is open (the start button drawn pressed, as 1.3's `menu_open`).
    pub start_open: bool,
    /// One for each window, in order.
    pub buttons: &'a [TaskButton<'a>],
    /// The time, "14:05", and the date, "13 Nov 2026", drawn as 1.3 draws its clock.
    pub time: &'a str,
    /// See `time`.
    pub date: &'a str,
}

impl Taskbar<'_> {
    /// The start button, in the taskbar's own logical pixels (0, 0 is its top left).
    pub fn start_rect(&self) -> Rect {
        Rect::new(START_X, 5, START_W, HEIGHT - 10)
    }

    /// How wide each window button is: 160, or less when there are so many that they would
    /// not fit between the start button and the clock (1.3's rule: share what is left, less
    /// the gaps). Never below nothing, on a screen too narrow for any.
    fn button_width(&self) -> i32 {
        let count = i64::try_from(self.buttons.len()).unwrap_or(i64::MAX);
        let room = i64::from(self.width) - i64::from(FIRST_BUTTON_X) - i64::from(CLOCK_ROOM);
        let width = i64::from(BUTTON_W);
        if count > 0 && count * (width + i64::from(GAP)) > room {
            (room / count - i64::from(GAP)).clamp(0, width) as i32
        } else {
            BUTTON_W
        }
    }

    /// Window button `i`, or `None` if there is no such window.
    pub fn button_rect(&self, i: usize) -> Option<Rect> {
        if i >= self.buttons.len() {
            return None;
        }
        let width = self.button_width();
        // 64-bit, so that a thousand buttons or more cannot wrap the position round.
        let x = i64::from(FIRST_BUTTON_X) + i as i64 * i64::from(width + GAP);
        Some(Rect::new(x.min(i64::from(i32::MAX)) as i32, 5, width, HEIGHT - 10))
    }

    /// The clock: 98 wide, 104 from the right edge.
    pub fn clock_rect(&self) -> Rect {
        Rect::new(self.width.saturating_sub(CLOCK_X_FROM_RIGHT), 0, CLOCK_W, HEIGHT)
    }

    /// What is at (`x`, `y`), in the taskbar's own logical pixels. As in 1.3, a part can be
    /// clicked anywhere from its top to the bottom edge of the bar, not only on the part
    /// that is drawn as a button: the strip under it is easy to hit with the pointer thrown
    /// to the bottom of the screen.
    pub fn part_at(&self, x: i32, y: i32) -> Option<Part> {
        if !(0..HEIGHT).contains(&y) {
            return None;
        }
        let start = self.start_rect();
        if y >= start.y && x >= start.x && i64::from(x) < start.right() {
            return Some(Part::Start);
        }
        // No search through the buttons: they are all the same width, in a row.
        let width = self.button_width();
        let from_first = i64::from(x) - i64::from(FIRST_BUTTON_X);
        if y >= 5 && from_first >= 0 && width > 0 {
            let pitch = i64::from(width + GAP);
            let (i, inside) = (from_first / pitch, from_first % pitch);
            if inside < i64::from(width)
                && let Ok(i) = usize::try_from(i)
                && i < self.buttons.len()
            {
                return Some(Part::Button(i));
            }
        }
        if self.clock_rect().contains(x, y) { Some(Part::Clock) } else { None }
    }

    /// Draws it into `canvas` (sized `width` x [`HEIGHT`] logical pixels, at any scale).
    pub fn draw(&self, canvas: &mut Canvas<'_>) {
        let bar = Rect::new(0, 0, self.width, HEIGHT);
        canvas.gradient_down(bar, look::C_BAR_A, look::C_BAR_B);
        canvas.fill_rect(Rect::new(0, 0, self.width, 1), look::C_BAR_EDGE);

        // The start button: pressed while its menu is open.
        let start = self.start_rect();
        let colour = if self.start_open { look::C_ACCENT_PRESSED } else { look::C_ACCENT };
        canvas.fill_round_rect(start, 6, colour);
        icons::draw(canvas, Icon::Logo, 12, 11, 1);
        crate::text(canvas, 34, 15, "PrettyBigRock", look::C_WHITE, TextStyle::BOLD);

        for (i, button) in self.buttons.iter().enumerate() {
            if let Some(rect) = self.button_rect(i) {
                // Buttons are laid out left to right: the first past the canvas's edge ends it.
                if i64::from(rect.x) >= canvas.clip_bounds().right() {
                    break;
                }
                draw_button(canvas, rect, button);
            }
        }

        // The clock: the time in bold above the date, each centred in the clock's room.
        let clock = self.clock_rect();
        let mut inside = canvas.clipped(clock);
        let time_width = TextStyle::BOLD.width(self.time);
        let date_width = TextStyle::PLAIN.width(self.date);
        let left = |text_width: i32| clock.x.saturating_add((CLOCK_W - text_width) / 2);
        crate::text(&mut inside, left(time_width), 8, self.time, look::C_WHITE, TextStyle::BOLD);
        crate::text(
            &mut inside,
            left(date_width),
            22,
            self.date,
            look::C_CLOCK_DATE,
            TextStyle::PLAIN,
        );
    }
}

/// Draws one window's button in `rect`.
fn draw_button(canvas: &mut Canvas<'_>, rect: Rect, button: &TaskButton<'_>) {
    if rect.width <= 0 {
        return;
    }
    // Which colours: the accent if it wants attention (a darker one if minimised), otherwise
    // 1.3's: lightest for the focused window, darkest for a minimised one.
    let (background, text) = if button.attention {
        let background = if button.minimised { look::C_ACCENT_PRESSED } else { look::C_ACCENT };
        (background, look::C_WHITE)
    } else if button.focused {
        (look::C_TASK_FOCUSED, look::C_TASK_TEXT)
    } else if button.minimised {
        (look::C_TASK_MINIMISED, look::C_TASK_TEXT_MINIMISED)
    } else {
        (look::C_TASK_NORMAL, look::C_TASK_TEXT)
    };
    canvas.fill_round_rect(rect, 5, background);
    if button.focused {
        let underline: Colour = if button.attention { look::C_WHITE } else { look::C_ACCENT };
        let line = Rect::new(rect.x + 8, HEIGHT - 7, rect.width - 16, 2);
        canvas.fill_rect(line, underline);
    }

    // The icon and the title stay inside the button (less the gap, as 1.3's clip did).
    let mut inside = canvas.clipped(Rect::new(rect.x, 0, rect.width - GAP, HEIGHT));
    icons::draw(&mut inside, button.icon, rect.x + 7, 11, 1);
    let title: String = if button.not_responding {
        format!("{} (not responding)", button.title)
    } else {
        String::from(button.title)
    };
    crate::text(&mut inside, rect.x + 29, 15, &title, text, TextStyle::PLAIN);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_width_shrinks_as_1_3s_does() {
        let mk = |n: usize, width: i32| {
            // A leaked slice is fine in a test.
            let buttons: &'static [TaskButton<'static>] = alloc::vec![
                TaskButton {
                    icon: Icon::App,
                    title: "",
                    focused: false,
                    minimised: false,
                    attention: false,
                    not_responding: false,
                };
                n
            ]
            .leak();
            Taskbar { width, start_open: false, buttons, time: "", date: "" }.button_width()
        };
        assert_eq!(mk(0, 800), 160);
        // 800 - 148 - 120 = 532 of room: three of 160 + 6 = 498 fit, four don't.
        assert_eq!(mk(3, 800), 160);
        assert_eq!(mk(4, 800), 532 / 4 - 6);
        assert_eq!(mk(5000, 800), 0);
        assert_eq!(mk(2, 10), 0);
    }
}
