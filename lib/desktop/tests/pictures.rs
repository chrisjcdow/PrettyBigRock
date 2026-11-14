// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop's drawing compared with the reference pictures in `tests/pictures/desktop/`
//! (docs/design/0023-draw-window-and-sandstone.md, section 6): the wallpaper, the taskbar, the
//! start menu in its three headings, the icons, and the desktop put together.

mod common;

use common::Sheet;
use pbr_desktop::icons::{self, Icon};
use pbr_desktop::menu::{Choice, Item, StartMenu};
use pbr_desktop::shortcuts::{self, Shortcut};
use pbr_desktop::taskbar::{self, TaskButton, Taskbar};
use pbr_desktop::wallpaper;
use pbr_draw::Rect;
use pbr_draw::TextStyle;

fn wall(width: u32, height: u32, scale: u32) -> Sheet {
    let mut sheet = Sheet::new(width, height, 0);
    wallpaper::draw(&mut sheet.pixels, width, height, width, scale);
    sheet
}

#[test]
fn wallpaper_640x400() {
    wall(640, 400, 1).check("wallpaper-640x400");
}

#[test]
fn wallpaper_at_2x_is_the_same_picture_but_for_the_sea() {
    let big = wall(640, 400, 2);
    big.check("wallpaper-320x200-2x");
    // Above the sea's reflections, which stop 76 pixels (the taskbar at 2x) up instead of 38,
    // the two are the same, pixel for pixel: the sky, stars, sun, mountain and the sea's
    // colour don't depend on the scale. The first reflection is below the horizon, so compare
    // the rows to the horizon and the sea's plain rows.
    let small = wall(640, 400, 1);
    let horizon = 400 * 76 / 100;
    assert_eq!(&big.pixels[..640 * horizon], &small.pixels[..640 * horizon]);
}

#[test]
fn the_wallpaper_is_the_same_every_time() {
    assert_eq!(wall(640, 400, 1).pixels, wall(640, 400, 1).pixels);
    // And any size draws the same picture, scaled: the sky's rows at one size are the sky's rows
    // at another (the first row is the sky's first colour at every size).
    assert_eq!(wall(320, 200, 1).pixels[0], 0x161a36);
    assert_eq!(wall(1920, 1080, 1).pixels[0], 0x161a36);
}

/// Three buttons: a focused one, a minimised one, and one that wants attention and is not
/// responding.
fn buttons() -> [TaskButton<'static>; 3] {
    let base = TaskButton {
        icon: Icon::Notepad,
        title: "letter.txt",
        focused: false,
        minimised: false,
        attention: false,
        not_responding: false,
    };
    [
        TaskButton { focused: true, ..base },
        TaskButton { icon: Icon::Files, title: "Documents", minimised: true, ..base },
        TaskButton {
            icon: Icon::Terminal,
            title: "Terminal",
            attention: true,
            not_responding: true,
            ..base
        },
    ]
}

fn taskbar_picture(scale: u32, open: bool) -> Sheet {
    let list = buttons();
    let bar = Taskbar {
        width: 800,
        start_open: open,
        buttons: &list,
        time: "14:05",
        date: "13 Nov 2026",
    };
    let mut sheet = Sheet::new(800 * scale, taskbar::HEIGHT as u32 * scale, 0x000000);
    bar.draw(&mut sheet.canvas(scale));
    sheet
}

#[test]
fn taskbar() {
    taskbar_picture(1, false).check("taskbar");
}

#[test]
fn taskbar_2x_with_the_start_button_pressed() {
    taskbar_picture(2, true).check("taskbar-2x-open");
}

const APPS: [(Icon, &str); 7] = [
    (Icon::Terminal, "Terminal"),
    (Icon::Notepad, "Notepad"),
    (Icon::Files, "Files"),
    (Icon::Calculator, "Calculator"),
    (Icon::Paint, "Paint"),
    (Icon::Snake, "Snake"),
    (Icon::About, "About"),
];

fn menu_picture(style: TextStyle, scale: u32) -> Sheet {
    let items: Vec<Item> = APPS.iter().map(|&(icon, label)| Item { icon, label }).collect();
    let menu = StartMenu { heading: "Chris", style, apps: &items, hovered: Some(Choice::App(1)) };
    let (w, h) = menu.size();
    // Grey behind, so that the rounded corners can be seen.
    let mut sheet = Sheet::new(w as u32 * scale, h as u32 * scale, 0x6f7480);
    menu.draw(&mut sheet.canvas(scale));
    sheet
}

#[test]
fn start_menu_with_a_size_2_heading() {
    menu_picture(TextStyle::PLAIN.sized(2), 1).check("start-menu-size2");
}

#[test]
fn start_menu_with_a_size_3_heading() {
    menu_picture(TextStyle::PLAIN.sized(3), 1).check("start-menu-size3");
}

#[test]
fn start_menu_with_a_bold_heading() {
    menu_picture(TextStyle::BOLD, 1).check("start-menu-bold");
}

#[test]
fn start_menu_at_2x() {
    menu_picture(TextStyle::PLAIN.sized(2), 2).check("start-menu-2x");
}

#[test]
fn every_icon_at_size_1_and_size_3() {
    let mut sheet = Sheet::new(11 * 56 + 8, 48 + 16 + 8 + 16 + 16, 0x6f7480);
    let mut canvas = sheet.canvas(1);
    for (i, icon) in icons::ALL.into_iter().enumerate() {
        let x = 8 + i as i32 * 56;
        icons::draw(&mut canvas, icon, x, 8, 3);
        icons::draw(&mut canvas, icon, x + 16, 72, 1);
    }
    sheet.check("icons");
}

#[test]
fn the_desktop_with_its_shortcuts_and_a_taskbar() {
    let mut sheet = wall(640, 480, 1);
    let height = 480 - taskbar::HEIGHT;
    let list: Vec<Shortcut> = APPS.iter().map(|&(icon, label)| Shortcut { icon, label }).collect();
    let bar =
        Taskbar { width: 640, start_open: false, buttons: &[], time: "14:05", date: "13 Nov 2026" };
    let mut canvas = sheet.canvas(1);
    shortcuts::draw(&mut canvas, &list, Some(2), height);
    bar.draw(&mut canvas.sub(Rect::new(0, height, 640, taskbar::HEIGHT)));
    sheet.check("desktop-empty");
}
