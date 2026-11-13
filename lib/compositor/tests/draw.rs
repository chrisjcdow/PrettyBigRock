// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Composing screens (docs/design/0022-display-server.md, section 7): pictures of the desktop
//! compared with the reference pictures in `tests/pictures/compositor/`, and checks that
//! drawing only part of a screen changes only that part and that nothing a scene holds can
//! make composing panic.

mod common;

use common::{Patterns, Rig, Sheet, output};
use pbr_compositor::decor::Button;
use pbr_compositor::draw::{Frames, Target, compose};
use pbr_compositor::{Buffers, Desk, Menu, MenuItem, Place, Scene, SceneWindow, Shown, WindowId};
use pbr_draw::{Picture, Rect};
use pbr_interfaces::pbr_window::{Cursor, WindowKind};

/// A screen's place with nothing special about it.
fn place(x: i32, scale: u32) -> Place {
    Place { x, y: 0, scale, rotation: 0, mirrors: 0, main: x == 0 }
}

#[test]
fn two_overlapping_windows_the_front_one_focused() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340)]);
    let back = rig.client("Notepad", 0);
    rig.open(back, WindowKind::Normal, "letter.txt", (150, 110));
    let front = rig.client("Calculator", 0);
    rig.open(front, WindowKind::Normal, "", (140, 100));
    rig.point(200, 160);
    rig.render(1).check("two-windows");
}

#[test]
fn a_window_at_2x_on_a_2x_screen() {
    // 400 by 300 dots in 50 millimetres is dense enough to be 2x by itself.
    let mut rig = Rig::new(&[output(0, 400, 300, 50)]);
    assert_eq!(rig.desk.space().screen(1).unwrap().place.scale, 200);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (120, 80));
    rig.point(110, 90);
    rig.render(1).check("scale-2x");
}

/// A window across a 1x screen (on the left) and a 2x screen (on the right), mostly on the
/// right one, so that it was told 2x and drew 2x pixels.
fn across() -> Rig {
    let mut rig = Rig::new(&[output(0, 240, 180, 340), output(1, 480, 360, 60)]);
    assert_eq!(rig.desk.space().screen(2).unwrap().place.scale, 200);
    let c = rig.client("Notepad", 0);
    let n = rig.open(c, WindowKind::Normal, "letter.txt", (120, 90));
    rig.drag(c, n, 190, 50);
    rig.draw(c, n);
    let w = rig.desk.scene().windows[0].clone();
    assert_eq!(w.shown.unwrap().buffers.scale, 200, "drawn for the 2x screen");
    assert!(w.content.x < 240 && w.content.right() > 240, "across the join: {:?}", w.content);
    rig
}

#[test]
fn a_window_across_two_screens_halved_on_the_1x_one() {
    let mut rig = across();
    rig.render(1).check("across-1x");
}

#[test]
fn a_window_across_two_screens_sharp_on_the_2x_one() {
    let mut rig = across();
    rig.render(2).check("across-2x");
}

#[test]
fn a_turned_screen_shows_the_space_turned() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (150, 110));
    rig.arrange(&[(1, Place { rotation: 90, ..place(0, 100) })]);
    rig.point(100, 100);
    assert_eq!(rig.desk.space().screen(1).unwrap().logical_size(), (240, 320));
    rig.render(1).check("rotated-90");
}

#[test]
fn half_and_three_quarter_turns_are_the_flat_picture_turned() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (150, 110));
    let flat = rig.render(1);
    rig.arrange(&[(1, Place { rotation: 180, ..place(0, 100) })]);
    let half = rig.render(1);
    for (x, y) in [(0, 0), (40, 30), (319, 239), (100, 10)] {
        assert_eq!(half.at(319 - x, 239 - y), flat.at(x, y));
    }
    // Turned a quarter clockwise, the space's top left corner is at the screen's top right.
    // (The space is 240 by 320 then, and the window is at the same place in it.)
    rig.arrange(&[(1, Place { rotation: 270, ..place(0, 100) })]);
    let left = rig.render(1);
    assert_eq!(left.width, 320);
    assert!(left.pixels.iter().all(|p| p >> 24 == 0xFF));
}

#[test]
fn a_mirroring_screen_shows_the_other_scaled_to_fit_with_black_round_it() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340), output(1, 240, 120, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (150, 110));
    rig.arrange(&[(1, place(0, 100)), (2, Place { mirrors: 1, ..place(0, 100) })]);
    rig.point(100, 100);
    let sheet = rig.render(2);
    // 320 by 240 shrunk to fit 240 by 120 is 160 by 120, in the middle.
    assert_eq!(sheet.at(0, 60), 0xFF00_0000);
    assert_eq!(sheet.at(239, 60), 0xFF00_0000);
    assert_ne!(sheet.at(120, 60), 0xFF00_0000);
    sheet.check("mirror");
}

/// A window of the scene by hand, with its content at (`x`, `y`).
fn hand(number: u32, x: i32, y: i32, title: &str) -> SceneWindow {
    SceneWindow {
        id: WindowId { client: 1, number },
        kind: WindowKind::Normal,
        content: Rect::new(x, y, 240, 18),
        decorated: true,
        focused: false,
        title: title.into(),
        trusted: false,
        system: false,
        maximised: false,
        shown: None,
        pressed: None,
    }
}

/// A desk with one screen, for scenes made by hand.
fn desk(w: u32, h: u32, mm: u32) -> Desk {
    let mut desk = Desk::new();
    desk.set_outputs(&[output(0, w, h, mm)], 0);
    desk
}

fn render(desk: &Desk, scene: &Scene, frames: &dyn Frames) -> Sheet {
    let o = &desk.space().screen(1).unwrap().output;
    let mut sheet = Sheet::new(o.width, o.height, 0xFF00_00FF);
    let target = Target {
        pixels: &mut sheet.pixels,
        width: sheet.width,
        height: sheet.height,
        stride: sheet.width,
    };
    compose(scene, desk.space(), 1, target, None, frames, &mut Vec::new());
    sheet
}

#[test]
fn every_state_of_a_windows_decorations() {
    let desk = desk(560, 270, 600);
    let long = "A very long title that has to be cut short by the buttons";
    let mut windows = vec![
        SceneWindow { focused: true, ..hand(1, 14, 40, "Notepad \u{2014} letter.txt") },
        hand(2, 284, 40, "Notepad \u{2014} letter.txt"),
        SceneWindow {
            focused: true,
            system: true,
            kind: WindowKind::System,
            ..hand(3, 14, 102, "File picker")
        },
        SceneWindow { system: true, kind: WindowKind::System, ..hand(4, 284, 102, "Permissions") },
        SceneWindow {
            focused: true,
            trusted: true,
            ..hand(5, 14, 164, "Terminal \u{2014} ~/Documents")
        },
        SceneWindow { trusted: true, ..hand(6, 284, 164, "Files \u{2014} ~/Documents") },
        SceneWindow {
            focused: true,
            maximised: true,
            pressed: Some(Button::Close),
            ..hand(7, 14, 226, "Maximised")
        },
        SceneWindow { title: format!("{long} (not responding)"), ..hand(8, 284, 226, "") },
    ];
    // One with the app's picture inside, so the content shows against its frame.
    let mut patterns = Patterns::default();
    let buffers = Buffers {
        generation: 1,
        width: 240,
        height: 18,
        stride: 960,
        scale: 100,
        logical: (240, 18),
    };
    let shown = Shown { buffers, frame: 0 };
    patterns.add(windows[0].id, &shown, 0xe8b04b, 0xfbe3b0);
    windows[0].shown = Some(shown);
    // A window above the last two, undecorated like a popup, to show it is just its content.
    windows.push(SceneWindow {
        kind: WindowKind::Popup,
        decorated: false,
        content: Rect::new(470, 10, 80, 20),
        title: String::new(),
        ..hand(9, 0, 0, "")
    });
    let scene = Scene { windows, ..Scene::default() };
    // The unfocused trusted and the pressed maximise buttons need no more states than these.
    render(&desk, &scene, &patterns).check("decorations");
}

#[test]
fn the_window_menu_with_a_row_hovered() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (200, 130));
    let mut scene = rig.desk.scene();
    scene.menu = Some(Menu {
        window: scene.windows[0].id,
        rect: Rect::new(60, 40, 220, 4 + 22 * 5),
        items: vec![
            MenuItem::Minimise,
            MenuItem::Restore,
            MenuItem::MoveTo(2),
            MenuItem::Permissions,
            MenuItem::Close,
        ],
        hover: Some(3),
    });
    let o = &rig.desk.space().screen(1).unwrap().output;
    let mut sheet = Sheet::new(o.width, o.height, 0);
    let target =
        Target { pixels: &mut sheet.pixels, width: o.width, height: o.height, stride: o.width };
    compose(&scene, rig.desk.space(), 1, target, None, &rig.patterns, &mut rig.scratch);
    sheet.check("menu");
}

#[test]
fn every_pointer_shape_at_1x_and_2x() {
    let shapes = [
        Cursor::Arrow,
        Cursor::Text,
        Cursor::Hand,
        Cursor::Move,
        Cursor::ResizeNs,
        Cursor::ResizeEw,
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
        Cursor::Wait,
        Cursor::Crosshair,
        Cursor::None,
    ];
    let mut out = Sheet::new(528, 276, 0xFF40_4040);
    for (row, (scale, mm)) in [(1u32, 340u32), (2, 60)].into_iter().enumerate() {
        let desk = desk(264 * scale, 90 * scale, mm);
        assert_eq!(desk.space().screen(1).unwrap().factor() as u32, scale);
        // A light panel across the lower half, so the shapes are seen on light and dark.
        let panel = SceneWindow {
            kind: WindowKind::Background,
            decorated: false,
            content: Rect::new(0, 45, 264, 45),
            ..hand(1, 0, 0, "")
        };
        let backdrop = Scene { windows: vec![panel.clone()], ..Scene::default() };
        let mut sheet = render(&desk, &backdrop, &Patterns::default());
        for lower in [0, 1] {
            for (i, cursor) in shapes.iter().enumerate() {
                let scene = Scene {
                    windows: vec![panel.clone()],
                    pointer: Some((24 + 22 * i as i32, 20 + 45 * lower, *cursor)),
                    ..Scene::default()
                };
                // Each shape is composed alone, then its neighbourhood is kept.
                let one = render(&desk, &scene, &Patterns::default());
                let (x0, x1) = ((12 + 22 * i as u32) * scale, (36 + 22 * i as u32) * scale);
                let (y0, y1) = ((45 * lower as u32) * scale, (45 * lower as u32 + 45) * scale);
                for y in y0..y1 {
                    for x in x0..x1.min(264 * scale) {
                        sheet.pixels[(y * 264 * scale + x) as usize] = one.at(x, y);
                    }
                }
            }
        }
        out.paste(&sheet, 0, if row == 0 { 0 } else { 90 });
    }
    out.check("pointers");
}

#[test]
fn the_farewell_is_all_there_is() {
    let mut rig = Rig::new(&[output(0, 320, 100, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "letter.txt", (150, 40));
    rig.desk.farewell(true);
    let restarting = rig.render(1);
    rig.desk.farewell(false);
    let shutting = rig.render(1);
    let mut both = Sheet::new(320, 200, 0);
    both.paste(&restarting, 0, 0);
    both.paste(&shutting, 0, 100);
    assert_eq!(restarting.at(0, 0), 0xFF10_1418);
    both.check("farewell");
}

#[test]
fn composing_a_part_changes_only_that_part_and_says_so() {
    for mm in [340, 60] {
        let mut rig = Rig::new(&[output(0, 320, 240, mm)]);
        let factor = rig.desk.space().screen(1).unwrap().factor();
        let a = rig.client("Notepad", 0);
        let na = rig.open(a, WindowKind::Normal, "a", (150, 110));
        let before = rig.render(1);
        // The window moves: then the picture differs in two places.
        rig.drag(a, na, 90, 80);
        let after = rig.render(1);
        assert_ne!(before.pixels, after.pixels);
        let areas = [
            Rect::new(20, 20, 100, 60),
            Rect::new(80, 70, 90, 80),
            Rect::new(-50, -50, 70, 70),
            Rect::new(1000, 0, 5, 5),
        ];
        let mut part = before.clone();
        let done = rig.render_into(1, &mut part, Some(&areas));
        let f = factor as u32;
        assert_eq!(done.len(), 3, "the one off the screen is left out");
        // Every pixel is either as before or as the whole picture says; and every pixel that
        // changed is in a rectangle returned; and the rectangles are inside the areas.
        let inside = |x: u32, y: u32, r: &Rect| {
            (x as i32) >= r.x
                && (y as i32) >= r.y
                && (x as i32) < r.x + r.width
                && (y as i32) < r.y + r.height
        };
        let mut changed = 0;
        for y in 0..part.height {
            for x in 0..part.width {
                let (p, b, w) = (part.at(x, y), before.at(x, y), after.at(x, y));
                if p != b {
                    changed += 1;
                    assert!(
                        done.iter().any(|r| inside(x, y, r)),
                        "{x},{y} changed outside the rectangles"
                    );
                    assert_eq!(p, w, "{x},{y}");
                }
                let in_area = areas.iter().any(|a| {
                    let d = Rect::new(
                        a.x * f as i32,
                        a.y * f as i32,
                        a.width * f as i32,
                        a.height * f as i32,
                    );
                    inside(x, y, &d)
                });
                if !in_area {
                    assert_eq!(p, b, "{x},{y} is outside every area");
                } else {
                    assert_eq!(p, w, "{x},{y} is inside an area");
                }
            }
        }
        assert!(changed > 0);
        // All of it, and nothing, are the two ends.
        let mut all = before.clone();
        assert_eq!(rig.render_into(1, &mut all, None), vec![Rect::new(0, 0, 320, 240)]);
        assert_eq!(all.pixels, after.pixels);
        let mut none = before.clone();
        assert!(rig.render_into(1, &mut none, Some(&[])).is_empty());
        assert_eq!(none.pixels, before.pixels);
    }
}

#[test]
fn more_than_64_areas_are_joined() {
    let mut rig = Rig::new(&[output(0, 320, 240, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "a", (150, 110));
    let areas: Vec<Rect> =
        (0..200).map(|i| Rect::new((i * 7) % 300, (i * 13) % 220, 3, 3)).collect();
    let mut sheet = rig.render(1);
    let done = rig.render_into(1, &mut sheet, Some(&areas));
    assert!(!done.is_empty() && done.len() <= 64);
    assert_eq!(sheet.pixels, rig.render(1).pixels);
}

#[test]
fn spare_pixels_at_the_end_of_rows_are_left_alone() {
    let mut rig = Rig::new(&[output(0, 200, 50, 340)]);
    let c = rig.client("Notepad", 0);
    rig.open(c, WindowKind::Normal, "a", (160, 10));
    let mut pixels = vec![0x1234_5678u32; 220 * 50];
    let target = Target { pixels: &mut pixels, width: 200, height: 50, stride: 220 };
    compose(&rig.desk.scene(), rig.desk.space(), 1, target, None, &rig.patterns, &mut rig.scratch);
    for row in pixels.chunks(220) {
        assert!(row[200..].iter().all(|&p| p == 0x1234_5678));
        assert!(row[..200].iter().all(|&p| p >> 24 == 0xFF));
    }
    // A buffer too short for what it says, or a screen that isn't there: nothing, no panic.
    let mut short = vec![0u32; 10];
    let target = Target { pixels: &mut short, width: 100, height: 50, stride: 100 };
    assert!(
        compose(
            &rig.desk.scene(),
            rig.desk.space(),
            1,
            target,
            None,
            &rig.patterns,
            &mut rig.scratch
        )
        .is_empty()
    );
    let target = Target { pixels: &mut pixels, width: 100, height: 50, stride: 120 };
    assert!(
        compose(
            &rig.desk.scene(),
            rig.desk.space(),
            9,
            target,
            None,
            &rig.patterns,
            &mut rig.scratch
        )
        .is_empty()
    );
}

/// Pictures of the wrong size for their buffers.
struct Wrong(Vec<u32>);

impl Frames for Wrong {
    fn picture(&self, _: WindowId, _: &Shown) -> Option<Picture<'_>> {
        Picture::new(&self.0, 3, 3, 3)
    }
}

/// Gives nothing.
struct Nothing;

impl Frames for Nothing {
    fn picture(&self, _: WindowId, _: &Shown) -> Option<Picture<'_>> {
        None
    }
}

#[test]
fn a_hostile_scene_draws_what_is_inside_and_never_panics() {
    let (max, min) = (i32::MAX, i32::MIN);
    let mut windows = Vec::new();
    let contents = [
        Rect::new(max / 2, max / 2, 100, 100),
        Rect::new(min / 2, min / 2, 100, 100),
        Rect::new(10, 10, 16384, 16384),
        Rect::new(10, 10, max, max),
        Rect::new(-5, -5, -10, 40),
        Rect::new(max, max, max, max),
        Rect::new(min, min, 50, 50),
        Rect::new(0, 0, 0, 0),
        Rect::new(20, 20, 100, 80),
    ];
    let sizes = [(max, max), (min, min), (0, 0), (16384, 16384), (-1, 5), (2, 2), (100, 80)];
    for (i, content) in contents.iter().enumerate() {
        for (j, size) in sizes.iter().enumerate() {
            for decorated in [true, false] {
                let buffers = Buffers {
                    generation: 1,
                    width: 8,
                    height: 8,
                    stride: 32,
                    scale: 100,
                    logical: *size,
                };
                windows.push(SceneWindow {
                    content: *content,
                    decorated,
                    focused: j % 2 == 0,
                    trusted: i % 2 == 0,
                    system: i % 3 == 0,
                    maximised: j % 3 == 0,
                    shown: if j == 6 { None } else { Some(Shown { buffers, frame: 1 }) },
                    pressed: Some(Button::Maximise),
                    title: "\u{1f600} caf\u{e9} ".repeat(i * 50),
                    ..hand((i * 10 + j) as u32, 0, 0, "")
                });
            }
        }
    }
    let scene = Scene {
        windows,
        pointer: Some((min, max, Cursor::Arrow)),
        menu: Some(Menu {
            window: WindowId { client: 1, number: 1 },
            rect: Rect::new(max - 5, min, max, max),
            items: vec![MenuItem::Close; 10_000],
            hover: Some(usize::MAX),
        }),
        farewell: None,
    };
    let mut other = scene.clone();
    other.pointer = Some((max / 2, 3, Cursor::Wait));
    let huge_areas =
        [Rect::new(min, min, max, max), Rect::new(0, 0, max, max), Rect::new(max, max, 5, 5)];
    let many: Vec<Rect> = (0..500).map(|i| Rect::new(i, i, 20, 20)).collect();
    let wrong = Wrong(vec![0xFF00_FF00; 9]);
    let frames: [&dyn Frames; 3] = [&Nothing, &wrong, &Patterns::default()];
    for (w, h, mm) in [(160, 120, 340), (200, 150, 50)] {
        for rotation in [0, 90, 180, 270] {
            let mut rig = Rig::new(&[output(0, w, h, mm), output(1, 90, 70, 340)]);
            let scale = if mm == 340 { 100 } else { 200 };
            // The second screen mirrors the first and has no rotation of its own.
            let arrangement = [
                (1, Place { rotation, ..place(0, scale) }),
                (2, Place { mirrors: 1, main: false, ..place(0, 100) }),
            ];
            rig.arrange(&arrangement);
            for screen in [1, 2] {
                for scene in [&scene, &other, &Scene { farewell: Some(true), ..scene.clone() }] {
                    for areas in [None, Some(&huge_areas[..]), Some(&many[..])] {
                        for f in frames {
                            let o = &rig.desk.space().screen(screen).unwrap().output;
                            let mut sheet = Sheet::new(o.width, o.height, 0xFF00_0000);
                            let target = Target {
                                pixels: &mut sheet.pixels,
                                width: o.width,
                                height: o.height,
                                stride: o.width,
                            };
                            let done = compose(
                                scene,
                                rig.desk.space(),
                                screen,
                                target,
                                areas,
                                f,
                                &mut rig.scratch,
                            );
                            let whole = Rect::new(0, 0, o.width as i32, o.height as i32);
                            assert!(done.len() <= 64);
                            assert!(done.iter().all(|r| whole.contains_rect(r)));
                            assert!(sheet.pixels.iter().all(|p| p >> 24 == 0xFF));
                        }
                    }
                }
            }
        }
    }
}
