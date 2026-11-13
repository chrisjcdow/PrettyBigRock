// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The start menu (1.3's `draw_menu`, `menu_item_y` and `menu_hit`), drawn in a window of its own
//! that is exactly the menu.
//!
//! Top to bottom, left to right: a banner down the left edge with the logo and the name running
//! down it; to its right a column with the user's name as a heading (new in 2.0: 1.3 had no
//! users), a row for each app, a thin line, then Restart and Shut down.

use crate::icons::{self, Icon};
use crate::look;
use pbr_draw::{Canvas, Rect, TextStyle};

/// The least wide a menu is (1.3's `MENU_W`), and the widest it grows to for a long label
/// (anything longer is cut off at the right edge).
const MIN_WIDTH: i32 = 236;
const MAX_WIDTH: i32 = 360;
/// A row's height, and the banner's width (1.3's `MENU_ITEM_H` and `MENU_BANNER`).
const ITEM_H: i32 = 30;
const BANNER: i32 = 30;
/// Space above the first row and below the last (1.3 had 6 each), and the line's room between
/// the apps and Restart.
const PAD: i32 = 6;
const SEPARATOR: i32 = 10;
/// Where a row's content starts: the banner and 4 pixels of margin (1.3's `ix`).
const COLUMN_X: i32 = BANNER + 4;
/// Room kept clear at the right edge of the column.
const RIGHT_MARGIN: i32 = 12;
/// What the banner says, down its length, one letter every 12 pixels (as 1.3).
const NAME: &str = "PRETTY BIG ROCK";

/// How the heading (the user's name) is drawn. All three are offered so that the person this
/// system is for can choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    /// The font at twice its size.
    Size2,
    /// The font at three times its size.
    Size3,
    /// The ordinary size, in 1.3's bold.
    Bold,
}

impl Heading {
    fn text_style(self) -> TextStyle {
        match self {
            Heading::Size2 => TextStyle::PLAIN.sized(2),
            Heading::Size3 => TextStyle::PLAIN.sized(3),
            Heading::Bold => TextStyle::BOLD,
        }
    }
}

/// What a click on the menu can choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// App number `n` of [`StartMenu::apps`].
    App(usize),
    /// Restart the computer.
    Restart,
    /// Turn the computer off.
    ShutDown,
}

/// One app's row.
#[derive(Clone, Copy, Debug)]
pub struct Item<'a> {
    /// The app's icon.
    pub icon: Icon,
    /// Its name.
    pub label: &'a str,
}

/// Everything the start menu shows.
#[derive(Clone, Copy, Debug)]
pub struct StartMenu<'a> {
    /// The user's display name, shown as a heading at the top of the item column (new in 2.0:
    /// 1.3 had no users). An empty one leaves the heading out.
    pub heading: &'a str,
    /// How the heading is drawn.
    pub style: Heading,
    /// The apps, in order; then the separator, Restart and Shut down (fixed, as 1.3).
    pub apps: &'a [Item<'a>],
    /// The row under the pointer or the keyboard's choice, drawn highlighted.
    pub hovered: Option<Choice>,
}

impl StartMenu<'_> {
    /// How tall the heading's block is: the name's capitals (7 dots), the room under them for
    /// tails (`g`, `y`: 2 dots) and 10 pixels over and 6 under; none if there is no heading.
    fn heading_height(&self) -> i32 {
        if self.heading.is_empty() {
            return 0;
        }
        let size = self.style.text_style().size as i32;
        10 + 9 * size + 6
    }

    /// The menu's width: 236, or more if a label or the heading needs it, up to 360.
    fn width(&self) -> i32 {
        // The room a row's label takes: it starts 30 pixels into the row's content.
        let label_x = COLUMN_X + 30;
        let mut needed = MIN_WIDTH;
        for item in self.apps {
            let wanted = label_x.saturating_add(TextStyle::PLAIN.width(item.label));
            needed = needed.max(wanted.saturating_add(RIGHT_MARGIN));
        }
        if !self.heading.is_empty() {
            let wanted = (COLUMN_X + 6).saturating_add(self.style.text_style().width(self.heading));
            needed = needed.max(wanted.saturating_add(RIGHT_MARGIN));
        }
        needed.min(MAX_WIDTH)
    }

    /// The height of the app rows together.
    fn apps_height(&self) -> i64 {
        i64::try_from(self.apps.len()).unwrap_or(i64::MAX).saturating_mul(i64::from(ITEM_H))
    }

    /// Its size in logical pixels: 1.3's width (wider if a label or the heading needs it, up to
    /// 360), and the height its rows need.
    pub fn size(&self) -> (i32, i32) {
        // 1.3's height: all the rows, the line, and 6 pixels above and below; plus the heading.
        let height = i64::from(self.heading_height())
            + i64::from(PAD)
            + self.apps_height().saturating_add(2 * i64::from(ITEM_H))
            + i64::from(SEPARATOR)
            + i64::from(PAD);
        (self.width(), height.min(i64::from(i32::MAX)) as i32)
    }

    /// The y of the first app's row (1.3's `menu_item_y(0)`).
    fn first_row_y(&self) -> i64 {
        i64::from(self.heading_height()) + i64::from(PAD)
    }

    /// The y of the row for `choice_index` counted over the apps, then Restart, then Shut down:
    /// the line's room comes between the apps and Restart.
    fn row_y(&self, index: usize) -> i64 {
        let index = i64::try_from(index).unwrap_or(i64::MAX);
        let mut y = self.first_row_y().saturating_add(index.saturating_mul(i64::from(ITEM_H)));
        if index >= self.apps.len() as i64 {
            y = y.saturating_add(i64::from(SEPARATOR));
        }
        y
    }

    /// What is at (`x`, `y`), in the menu's own logical pixels. The banner, the heading, the
    /// gaps and the line are nothing.
    pub fn choice_at(&self, x: i32, y: i32) -> Option<Choice> {
        let (width, _) = self.size();
        if x < BANNER || x >= width {
            return None;
        }
        let from_rows = i64::from(y) - self.first_row_y();
        if from_rows < 0 {
            return None;
        }
        let apps = self.apps_height();
        if from_rows < apps {
            return Some(Choice::App((from_rows / i64::from(ITEM_H)) as usize));
        }
        // Below the apps: first the line's room, then Restart and Shut down.
        let below = from_rows - apps - i64::from(SEPARATOR);
        match below {
            b if b < 0 => None,
            b if b < i64::from(ITEM_H) => Some(Choice::Restart),
            b if b < 2 * i64::from(ITEM_H) => Some(Choice::ShutDown),
            _ => None,
        }
    }

    /// Draws it into `canvas` (sized [`StartMenu::size`], any scale), at 0, 0. 1.3's banner with
    /// the name down it and the logo, the hover highlight, the icons and the line. The shadow
    /// 1.3 drew outside the menu is left out: the window is exactly the menu.
    ///
    /// The four corners outside the rounded shape are left as they were.
    pub fn draw(&self, canvas: &mut Canvas<'_>) {
        let (width, height) = self.size();
        let body = Rect::new(0, 0, width, height);
        canvas.fill_round_rect(body, 6, look::C_MENU);

        // The banner: a gradient from the accent to a dark plum, with caps top and bottom
        // and round corners, so that it follows the menu's rounded corners.
        canvas.gradient_down(
            Rect::new(0, 4, BANNER, height.saturating_sub(8)),
            look::C_BANNER_A,
            look::C_BANNER_B,
        );
        canvas.fill_rect(Rect::new(4, 0, BANNER - 4, 4), look::C_BANNER_A);
        canvas.fill_rect(Rect::new(4, height.saturating_sub(4), BANNER - 4, 4), look::C_BANNER_B);
        canvas.fill_round_rect(Rect::new(0, 0, 10, 10), 5, look::C_BANNER_A);
        canvas.fill_round_rect(
            Rect::new(0, height.saturating_sub(10), 10, 10),
            5,
            look::C_BANNER_B,
        );

        // The name, a letter to a line, ending 14 pixels above the bottom. A short menu has
        // no room for all of it; the letters that would run into the logo are left out.
        let letters = NAME.len() as i32;
        for (i, letter) in NAME.char_indices() {
            let y = height.saturating_sub(14).saturating_sub((letters - i as i32) * 12);
            if y >= 28 && letter != ' ' {
                let text = &NAME[i..i + letter.len_utf8()];
                crate::text(canvas, 12, y, text, look::C_WHITE, TextStyle::PLAIN);
            }
        }
        icons::draw(canvas, Icon::Logo, 7, 8, 1);

        // The user's name, and a line under it.
        if !self.heading.is_empty() {
            let column = Rect::new(COLUMN_X, 0, width - COLUMN_X - 8, self.heading_height());
            let mut inside = canvas.clipped(column);
            crate::text(
                &mut inside,
                COLUMN_X + 6,
                10,
                self.heading,
                look::C_MENU_TEXT,
                self.style.text_style(),
            );
            let line_y = self.heading_height() - 1;
            canvas.fill_rect(
                Rect::new(BANNER + 10, line_y, width - BANNER - 20, 1),
                look::C_MENU_SEPARATOR,
            );
        }

        // The rows: the apps, then Restart and Shut down. Only those that show are drawn.
        let visible = canvas.clip_bounds();
        let count = self.apps.len().saturating_add(2);
        for index in 0..count {
            let y = self.row_y(index);
            if y >= visible.bottom() {
                break;
            }
            if y + i64::from(ITEM_H) <= i64::from(visible.y) {
                continue;
            }
            let y = y as i32;
            let (icon, label, choice) = match self.apps.get(index) {
                Some(item) => (item.icon, item.label, Choice::App(index)),
                None if index == self.apps.len() => (Icon::Restart, "Restart", Choice::Restart),
                None => (Icon::ShutDown, "Shut down", Choice::ShutDown),
            };
            if self.hovered == Some(choice) {
                canvas.fill_round_rect(
                    Rect::new(COLUMN_X, y + 1, width - BANNER - 8, ITEM_H - 2),
                    4,
                    look::C_MENU_HOVER,
                );
            }
            icons::draw(canvas, icon, COLUMN_X + 6, y + 7, 1);
            let room = Rect::new(COLUMN_X + 30, y, width - COLUMN_X - 30 - 8, ITEM_H);
            crate::text(
                &mut canvas.clipped(room),
                COLUMN_X + 30,
                y + 11,
                label,
                look::C_MENU_TEXT,
                TextStyle::PLAIN,
            );
        }
        let line_y = self.row_y(self.apps.len()) - 6;
        let line_y = line_y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        canvas.fill_rect(
            Rect::new(BANNER + 10, line_y, width - BANNER - 20, 1),
            look::C_MENU_SEPARATOR,
        );
    }
}
