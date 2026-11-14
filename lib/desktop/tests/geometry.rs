// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Where things are, and that nothing panics: hit tests for the menu, the taskbar and the
//! shortcuts, and every drawing call on hostile sizes and strings.

mod common;

use common::Sheet;
use pbr_desktop::icons::{self, Icon};
use pbr_desktop::menu::{Choice, Item, StartMenu};
use pbr_desktop::shortcuts::{self, Shortcut};
use pbr_desktop::taskbar::{self, Part, TaskButton, Taskbar};
use pbr_desktop::wallpaper;
use pbr_draw::TextStyle;
use pbr_draw::{Canvas, Rect};

fn items(n: usize) -> Vec<Item<'static>> {
    (0..n).map(|_| Item { icon: Icon::App, label: "App" }).collect()
}

fn menu<'a>(apps: &'a [Item<'a>], style: TextStyle) -> StartMenu<'a> {
    StartMenu { heading: "Chris", style, apps, hovered: None }
}

#[test]
fn the_menus_size_follows_1_3() {
    let apps = items(7);
    // No heading: 1.3's width, and its height (9 rows of 30, the line's 10 and 12 of padding).
    let m = StartMenu { heading: "", style: TextStyle::BOLD, apps: &apps, hovered: None };
    assert_eq!(m.size(), (236, 9 * 30 + 10 + 12));
    // A heading adds its block: 10 + 9 x size + 6.
    assert_eq!(menu(&apps, TextStyle::BOLD).size(), (236, 292 + 25));
    assert_eq!(menu(&apps, TextStyle::PLAIN.sized(2)).size(), (236, 292 + 34));
    assert_eq!(menu(&apps, TextStyle::PLAIN.sized(3)).size(), (236, 292 + 43));
}

#[test]
fn the_menu_grows_for_a_long_label_up_to_a_limit() {
    let long = [Item { icon: Icon::App, label: "A rather long name for an app" }];
    let m = menu(&long, TextStyle::BOLD);
    // 64 for the label's start, 29 letters of 6, 12 of margin.
    assert_eq!(m.size().0, 64 + 29 * 6 + 12);
    let huge = String::from("x").repeat(10_000);
    let apps = [Item { icon: Icon::App, label: &huge }];
    assert_eq!(menu(&apps, TextStyle::BOLD).size().0, 360);
    let m =
        StartMenu { heading: &huge, style: TextStyle::PLAIN.sized(3), apps: &[], hovered: None };
    assert_eq!(m.size().0, 360);
}

#[test]
fn choice_at_finds_every_row_and_not_the_gaps() {
    for style in [TextStyle::PLAIN.sized(2), TextStyle::PLAIN.sized(3), TextStyle::BOLD] {
        let apps = items(7);
        let m = menu(&apps, style);
        let (w, h) = m.size();
        let head = h - (6 + 9 * 30 + 10 + 6);
        let x = 100;
        // The heading and the top padding are nothing.
        for y in 0..head + 6 {
            assert_eq!(m.choice_at(x, y), None, "{style:?} y {y}");
        }
        for i in 0..7 {
            let top = head + 6 + i * 30;
            assert_eq!(
                m.choice_at(x, top - 1),
                if i == 0 { None } else { Some(Choice::App(i as usize - 1)) }
            );
            assert_eq!(m.choice_at(x, top), Some(Choice::App(i as usize)));
            assert_eq!(m.choice_at(x, top + 29), Some(Choice::App(i as usize)));
        }
        let line = head + 6 + 7 * 30;
        // The line's room, 10 pixels, is nothing.
        assert_eq!(m.choice_at(x, line - 1), Some(Choice::App(6)));
        for y in line..line + 10 {
            assert_eq!(m.choice_at(x, y), None);
        }
        assert_eq!(m.choice_at(x, line + 10), Some(Choice::Restart));
        assert_eq!(m.choice_at(x, line + 39), Some(Choice::Restart));
        assert_eq!(m.choice_at(x, line + 40), Some(Choice::ShutDown));
        assert_eq!(m.choice_at(x, line + 69), Some(Choice::ShutDown));
        // The bottom padding.
        for y in line + 70..h + 5 {
            assert_eq!(m.choice_at(x, y), None);
        }
        // The banner (x under 30) and past the right edge are nothing; the column's edge is not.
        assert_eq!(m.choice_at(29, head + 10), None);
        assert_eq!(m.choice_at(30, head + 10), Some(Choice::App(0)));
        assert_eq!(m.choice_at(w - 1, head + 10), Some(Choice::App(0)));
        assert_eq!(m.choice_at(w, head + 10), None);
        assert_eq!(m.choice_at(-5, head + 10), None);
        assert_eq!(m.choice_at(x, -5), None);
        assert_eq!(m.choice_at(i32::MAX, i32::MAX), None);
        assert_eq!(m.choice_at(i32::MIN, i32::MIN), None);
    }
}

#[test]
fn a_menu_with_no_apps_and_no_heading_has_just_restart_and_shut_down() {
    let m = StartMenu { heading: "", style: TextStyle::BOLD, apps: &[], hovered: None };
    assert_eq!(m.size(), (236, 2 * 30 + 10 + 12));
    assert_eq!(m.choice_at(100, 5), None);
    assert_eq!(m.choice_at(100, 6), None);
    assert_eq!(m.choice_at(100, 16), Some(Choice::Restart));
    assert_eq!(m.choice_at(100, 46), Some(Choice::ShutDown));
}

fn buttons(n: usize) -> Vec<TaskButton<'static>> {
    (0..n)
        .map(|_| TaskButton {
            icon: Icon::App,
            title: "A window",
            focused: false,
            minimised: false,
            attention: false,
            not_responding: false,
        })
        .collect()
}

fn bar<'a>(width: i32, buttons: &'a [TaskButton<'a>]) -> Taskbar<'a> {
    Taskbar { width, start_open: false, buttons, time: "14:05", date: "13 Nov 2026" }
}

#[test]
fn part_at_finds_the_start_button_each_button_and_the_clock() {
    let list = buttons(3);
    let t = bar(800, &list);
    assert_eq!(t.start_rect(), Rect::new(6, 5, 132, 28));
    assert_eq!(t.clock_rect(), Rect::new(696, 0, 98, 38));
    let at = |x, y| t.part_at(x, y);
    assert_eq!(at(6, 20), Some(Part::Start));
    assert_eq!(at(137, 20), Some(Part::Start));
    assert_eq!(at(138, 20), None);
    assert_eq!(at(5, 20), None);
    // As 1.3: the start button reaches the bottom edge of the bar, but not its top 5 pixels.
    assert_eq!(at(10, 37), Some(Part::Start));
    assert_eq!(at(10, 4), None);
    assert_eq!(at(10, 38), None);
    assert_eq!(at(10, -1), None);
    for i in 0..3 {
        let r = t.button_rect(i).unwrap();
        assert_eq!(r, Rect::new(148 + i as i32 * 166, 5, 160, 28));
        assert_eq!(at(r.x, 20), Some(Part::Button(i)));
        assert_eq!(at(r.x + 159, 20), Some(Part::Button(i)));
        assert_eq!(at(r.x + 160, 20), None, "the gap");
    }
    assert_eq!(t.button_rect(3), None);
    assert_eq!(at(148 + 3 * 166, 20), None);
    assert_eq!(at(696, 0), Some(Part::Clock));
    assert_eq!(at(793, 37), Some(Part::Clock));
    assert_eq!(at(794, 20), None);
    assert_eq!(at(i32::MAX, 20), None);
    assert_eq!(at(i32::MIN, 20), None);
}

#[test]
fn buttons_shrink_when_there_are_many() {
    let list = buttons(8);
    let t = bar(800, &list);
    // 800 - 148 - 120 = 532 of room, shared by 8 with 6 between: 532 / 8 - 6 = 60.
    for i in 0..8 {
        let r = t.button_rect(i).unwrap();
        assert_eq!(r.width, 60);
        assert_eq!(r.x, 148 + i as i32 * 66);
        assert!(r.right() <= i64::from(t.clock_rect().x), "button {i} is clear of the clock");
        assert_eq!(t.part_at(r.x + 30, 20), Some(Part::Button(i)));
    }
}

#[test]
fn shortcut_cells_follow_1_3_and_wrap_into_a_second_column() {
    // A 640 x 480 screen has 442 above the taskbar: 5 cells fit (the fifth ends at 420).
    let height = 442;
    assert_eq!(shortcuts::cell(0, height), Rect::new(16, 16, 84, 76));
    assert_eq!(shortcuts::cell(4, height), Rect::new(16, 16 + 4 * 82, 84, 76));
    assert_eq!(shortcuts::cell(5, height), Rect::new(106, 16, 84, 76));
    assert_eq!(shortcuts::cell(6, height), Rect::new(106, 98, 84, 76));
    // A sixth would not fit in a screen 4 pixels shorter than the fifth needs: 16 + 328 + 76 = 420.
    assert_eq!(shortcuts::cell(4, 420), Rect::new(16, 344, 84, 76));
    assert_eq!(shortcuts::cell(4, 419), Rect::new(106, 16, 84, 76));
    // A screen too short for even one still has a column of one.
    assert_eq!(shortcuts::cell(1, 10), Rect::new(106, 16, 84, 76));
}

#[test]
fn shortcuts_at_finds_each_cell_and_not_the_gaps() {
    let height = 442;
    for i in 0..7 {
        let c = shortcuts::cell(i, height);
        for (x, y) in [(c.x, c.y), (c.x + 83, c.y + 75), (c.x + 40, c.y + 40)] {
            assert_eq!(shortcuts::at(7, height, x, y), Some(i), "cell {i} at {x},{y}");
        }
        // The gaps right of and below the cell are nothing.
        assert_eq!(shortcuts::at(7, height, c.x + 84, c.y + 10), None);
        assert_eq!(shortcuts::at(7, height, c.x + 10, c.y + 76), None);
    }
    // Beyond the last shortcut, left of and above the first, and off the screen.
    assert_eq!(shortcuts::at(7, height, 106 + 10, 98 + 82 + 10), None);
    assert_eq!(shortcuts::at(7, height, 15, 20), None);
    assert_eq!(shortcuts::at(7, height, 20, 15), None);
    assert_eq!(shortcuts::at(7, height, -100, -100), None);
    assert_eq!(shortcuts::at(7, height, i32::MAX, i32::MAX), None);
    assert_eq!(shortcuts::at(7, height, i32::MIN, 20), None);
    assert_eq!(shortcuts::at(0, height, 20, 20), None);
    // The sixth row of the first column doesn't exist on this screen.
    assert_eq!(shortcuts::at(100, height, 20, 16 + 5 * 82 + 10), None);
    // `at` and `cell` agree everywhere.
    for x in (0..400).step_by(7) {
        for y in (0..480).step_by(5) {
            let by_cell = (0..30).find(|&i| shortcuts::cell(i, height).contains(x, y));
            assert_eq!(shortcuts::at(30, height, x, y), by_cell, "{x},{y}");
        }
    }
}

// ---------------------------------------------------------------- nothing panics

const TITLES: [&str; 4] =
    ["", "x", "A title with, well, quite a lot of words in it", "é€ü \u{1F600} \u{0} \n"];

#[test]
fn the_wallpaper_survives_any_size() {
    for scale in [0, 1, 2, 3, 4, 5, u32::MAX] {
        for (w, h) in [(0, 0), (0, 10), (10, 0), (1, 1), (2, 3), (7, 5), (50, 20), (100, 37)] {
            let mut pixels = vec![0u32; (w * h) as usize];
            wallpaper::draw(&mut pixels, w, h, w, scale);
        }
    }
    // Too little memory for the size, a stride less than the width, a huge size: nothing drawn.
    let mut pixels = vec![7u32; 100];
    wallpaper::draw(&mut pixels, 640, 400, 640, 1);
    wallpaper::draw(&mut pixels, 10, 10, 5, 1);
    wallpaper::draw(&mut pixels, u32::MAX, u32::MAX, u32::MAX, 1);
    wallpaper::draw(&mut pixels, 100_000, 1, 100_000, 1);
    wallpaper::draw(&mut [], 1, 1, 1, 1);
    assert!(pixels.iter().all(|&p| p == 7));
    // A stride wider than the width leaves the spare pixels alone.
    let mut pixels = vec![0xDEAD_BEEFu32; 70 * 40];
    wallpaper::draw(&mut pixels, 64, 40, 70, 1);
    for row in 0..40 {
        assert!(pixels[row * 70 + 64..row * 70 + 70].iter().all(|&p| p == 0xDEAD_BEEF));
        assert!(pixels[row * 70..row * 70 + 64].iter().all(|&p| p != 0xDEAD_BEEF));
    }
}

#[test]
fn a_tall_thin_and_a_wide_flat_screen_work() {
    for (w, h) in [(3, 600), (600, 3), (1, 500), (500, 1)] {
        let mut pixels = vec![0u32; w * h];
        wallpaper::draw(&mut pixels, w as u32, h as u32, w as u32, 2);
    }
}

#[test]
fn icons_survive_any_position_and_size() {
    let mut sheet = Sheet::new(30, 30, 0);
    let mut canvas = sheet.canvas(1);
    for icon in icons::ALL {
        for &(x, y) in &[(0, 0), (-5, -5), (25, 25), (i32::MAX, i32::MAX), (i32::MIN, i32::MIN)] {
            for size in [i32::MIN, -1, 0, 1, 3, 1000, i32::MAX] {
                icons::draw(&mut canvas, icon, x, y, size);
            }
        }
    }
    // An icon drawn off the canvas changes nothing.
    let before = sheet.pixels.clone();
    let mut canvas = sheet.canvas(1);
    icons::draw(&mut canvas, Icon::Logo, 100, 100, 3);
    icons::draw(&mut canvas, Icon::Logo, 5, 5, 0);
    assert_eq!(sheet.pixels, before);
}

#[test]
fn the_taskbar_survives_anything() {
    for scale in [1, 2, 5] {
        for width in [i32::MIN, -1, 0, 1, 50, 300, 800, 100_000, i32::MAX] {
            for count in [0, 1, 3, 40, 2000] {
                for title in TITLES {
                    let mut list = buttons(count);
                    for (i, b) in list.iter_mut().enumerate() {
                        b.title = title;
                        b.focused = i == 1;
                        b.minimised = i % 3 == 0;
                        b.attention = i % 4 == 0;
                        b.not_responding = i % 5 == 0;
                    }
                    let t = Taskbar { time: title, date: title, ..bar(width, &list) };
                    let w = width.clamp(0, 400) as u32 * scale;
                    let mut sheet = Sheet::new(w, 38 * scale, 0);
                    t.draw(&mut sheet.canvas(scale));
                    // The hit tests answer for any position.
                    for &(x, y) in &[(0, 0), (-1, -1), (10, 10), (400, 20), (i32::MAX, 5)] {
                        let _ = t.part_at(x, y);
                    }
                    let _ = t.button_rect(usize::MAX);
                    let _ = t.button_rect(count / 2);
                    let _ = t.start_rect();
                    let _ = t.clock_rect();
                }
            }
        }
    }
    // A zero-sized canvas.
    let mut none: [u32; 0] = [];
    let mut canvas = Canvas::new(&mut none, 0, 0, 0).unwrap();
    bar(800, &buttons(3)).draw(&mut canvas);
}

#[test]
fn the_menu_survives_anything() {
    let huge = String::from("w").repeat(5000);
    for scale in [1, 2, 5] {
        for style in [TextStyle::PLAIN.sized(2), TextStyle::PLAIN.sized(3), TextStyle::BOLD] {
            for count in [0, 1, 7, 100] {
                for title in TITLES.iter().copied().chain([huge.as_str()]) {
                    let apps: Vec<Item> =
                        (0..count).map(|_| Item { icon: Icon::Snake, label: title }).collect();
                    for hovered in [
                        None,
                        Some(Choice::App(0)),
                        Some(Choice::App(1000)),
                        Some(Choice::Restart),
                        Some(Choice::ShutDown),
                    ] {
                        let m = StartMenu { heading: title, style, apps: &apps, hovered };
                        let (w, h) = m.size();
                        assert!((236..=360).contains(&w) && h > 0);
                        let mut sheet = Sheet::new(w as u32 * scale, h as u32 * scale, 0);
                        m.draw(&mut sheet.canvas(scale));
                        // Drawn into a canvas too small, or too big, for it.
                        let mut small = Sheet::new(40, 40, 0);
                        m.draw(&mut small.canvas(scale));
                        let mut big = Sheet::new(500, 500, 0);
                        m.draw(&mut big.canvas(scale));
                        for &(x, y) in &[(0, 0), (-1, -1), (100, 100), (i32::MAX, i32::MIN)] {
                            let _ = m.choice_at(x, y);
                        }
                    }
                }
            }
        }
    }
    // Millions of apps: the size and a hit test don't loop, and nothing wraps round.
    let many: Vec<Item> = (0..2_000_000).map(|_| Item { icon: Icon::App, label: "x" }).collect();
    let m = menu(&many, TextStyle::BOLD);
    assert!(m.size().1 > 0);
    // The heading's block is 25 tall and the padding 6: the last app's row starts at 31 + 30 x 1999999.
    assert_eq!(m.choice_at(100, 31 + 30 * 1_999_999), Some(Choice::App(1_999_999)));
}

#[test]
fn shortcuts_survive_anything() {
    for scale in [1, 2, 5] {
        for height in [i32::MIN, -1, 0, 1, 100, 442, i32::MAX] {
            for count in [0, 1, 7, 500] {
                for label in TITLES {
                    let list: Vec<Shortcut> =
                        (0..count).map(|_| Shortcut { icon: Icon::Paint, label }).collect();
                    let mut sheet = Sheet::new(200 * scale, 150 * scale, 0);
                    let mut canvas = sheet.canvas(scale);
                    for selected in [None, Some(0), Some(3), Some(usize::MAX)] {
                        shortcuts::draw(&mut canvas, &list, selected, height);
                    }
                    let _ = shortcuts::at(count, height, 20, 20);
                }
            }
        }
    }
    let _ = shortcuts::cell(usize::MAX, i32::MAX);
    let _ = shortcuts::cell(usize::MAX, i32::MIN);
    let _ = shortcuts::at(usize::MAX, i32::MAX, i32::MAX, i32::MAX);
    let _ = shortcuts::at(usize::MAX, 0, 16 + 90 * 1000, 16);
}

#[test]
fn a_whole_taskbar_height_matches_the_constant() {
    assert_eq!(taskbar::HEIGHT, 38);
}
