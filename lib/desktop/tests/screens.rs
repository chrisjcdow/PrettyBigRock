// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The Screens window (`pbr_desktop::screens_window`) with sandstone's harness: what the
//! choices make Apply ask for, OK, Cancel, and its look (`tests/pictures/desktop/screens-*.png`).
//! What the choices mean and which are allowed is tested in `src/arrange.rs`.

use pbr_desktop::screens_window::{self, SETTLED, ScreensWindow, settings};
use pbr_interfaces::pbr_window::{ScreenPlace, Size};
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{App, Key, WindowId};
use pbr_window::Event;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/desktop")
}

/// Screen `n`, `connector`, at `x`, with the size it has in the space.
fn place(n: u32, connector: &str, x: i32, scale: u32, w: u32, h: u32) -> ScreenPlace {
    ScreenPlace {
        screen: n,
        identity: format!("RHT-1234-00000000 {connector}"),
        x,
        y: 0,
        scale,
        rotation: 0,
        mirrors: 0,
        main: n == 1,
        size: Size { width: w, height: h },
    }
}

/// Two screens of 1280 x 800 dots, the second at 2x.
fn two_places() -> Vec<ScreenPlace> {
    vec![place(1, "Virtual-1", 0, 100, 1280, 800), place(2, "Virtual-2", 1280, 200, 640, 400)]
}

fn open(tablets: &[(String, String)]) -> Harness<ScreensWindow> {
    Harness::new(ScreensWindow::new(&two_places(), None, tablets), settings())
}

/// Picks row `row` of the list of screens.
fn pick(h: &mut Harness<ScreensWindow>, row: i32) {
    let (id, list) = h.find("screens").expect("the list");
    // The header is 21 high and each row 20.
    h.click_in(id, list.rect.x + 20, list.rect.y + 21 + 20 * row + 10);
}

fn is_disabled(h: &Harness<ScreensWindow>, label: &str) -> bool {
    !h.find(label).expect(label).1.what.enabled
}

/// What Apply asks for, as (screen, x, scale, rotation, mirrors, main).
fn asked(h: &mut Harness<ScreensWindow>) -> Vec<(u32, i32, u32, u32, u32, bool)> {
    h.click_on("Apply");
    let pending = h.app_mut().take_request().expect("Apply asks for something");
    assert!(!pending.close);
    let mut places: Vec<_> = pending
        .request
        .places
        .iter()
        .map(|p| (p.screen, p.x, p.scale, p.rotation, p.mirrors, p.main))
        .collect();
    // By screen, whatever their order in the space.
    places.sort();
    places
}

#[test]
fn opens_on_the_screens_as_they_are() {
    let mut h = open(&[]);
    assert_eq!(h.app().title(), "Screens");
    assert_eq!(h.app().picked(), 1);
    // Apply with nothing changed asks for what is there (side by side, the second at 2x).
    assert_eq!(asked(&mut h), [(1, 0, 100, 0, 0, true), (2, 1280, 200, 0, 0, false)]);
    let lines = h.app().plan_lines();
    assert_eq!(lines[1], "screen 2 Virtual-2 at 1280,0 scale 200 rotation 0 mirrors 0");
    assert_eq!(lines[0], "screen 1 Virtual-1 at 0,0 scale 100 rotation 0 mirrors 0 main");
}

#[test]
fn the_choices_change_what_apply_asks_for() {
    let mut h = open(&[]);
    // Screen 1 to 2x: screen 2 moves up to meet it (it is 640 wide now).
    h.click_on("2x");
    assert_eq!(asked(&mut h)[1].1, 640);
    // Screen 2 turned a quarter, back at 1x: it is then 800 wide, and screen 1 is 640.
    pick(&mut h, 1);
    h.click_on("1x");
    h.click_on("90");
    let places = asked(&mut h);
    assert_eq!((places[1].2, places[1].3), (100, 90));
    // The order.
    h.click_on("Move left");
    let places = asked(&mut h);
    assert_eq!((places[1].1, places[0].1), (0, 800));
    h.click_on("Move right");
    assert_eq!(asked(&mut h)[1].1, 640);
}

#[test]
fn the_ends_of_the_order_cannot_move_further() {
    let mut h = open(&[]);
    assert!(is_disabled(&h, "Move left"));
    assert!(!is_disabled(&h, "Move right"));
    pick(&mut h, 1);
    assert!(!is_disabled(&h, "Move left"));
    assert!(is_disabled(&h, "Move right"));
}

#[test]
fn one_main_screen() {
    let mut h = open(&[]);
    pick(&mut h, 1);
    h.click_on("Main screen");
    let places = asked(&mut h);
    assert_eq!((places[0].5, places[1].5), (false, true));
    // Pressing it again leaves it main.
    h.click_on("Main screen");
    assert_eq!(asked(&mut h).iter().filter(|p| p.5).count(), 1);
}

#[test]
fn mirroring_and_its_limits() {
    let mut h = open(&[]);
    // The main screen can't show another, and says why.
    assert!(is_disabled(&h, "Its own picture"));
    assert!(h.find("(it is the main screen)").is_some());
    pick(&mut h, 1);
    h.click_on("Its own picture");
    assert_eq!(h.app().plan().screen(2).unwrap().mirrors, 1);
    assert!(h.find("Same picture as Virtual-1").is_some());
    assert_eq!(asked(&mut h)[1].4, 1);
    // And the one shown can't show another either, nor be moved.
    assert!(is_disabled(&h, "Move left") && is_disabled(&h, "Move right"));
    pick(&mut h, 0);
    assert!(h.find("(it is the main screen)").is_some());
    // Round again to its own picture.
    pick(&mut h, 1);
    h.click_on("Same picture as Virtual-1");
    assert_eq!(h.app().plan().screen(2).unwrap().mirrors, 0);
}

#[test]
fn refusing_to_make_the_main_screen_mirror_says_why() {
    let mut h = open(&[]);
    // The only way to make no main screen is to make the main screen show another, which the
    // button doesn't offer; choosing "main" on a screen that shows another is refused too.
    pick(&mut h, 1);
    h.click_on("Its own picture");
    h.click_on("Main screen");
    let text = h.find("status").expect("status").1.what.label;
    assert!(text.contains("no place") || text.contains("main"), "{text}");
    assert_eq!(h.app().plan().screens().iter().filter(|s| s.main).count(), 1);
}

#[test]
fn the_pointer_and_the_tablets() {
    let tablets = [("QEMU Virtio Tablet".to_string(), "RHT-1234-00000000 Virtual-2".to_string())];
    let mut h = open(&tablets);
    h.click_on("Faster");
    h.click_on("Faster");
    assert_eq!(h.app().plan().speed(), 150);
    h.click_on("Slower");
    assert_eq!(h.app().plan().speed(), 125);
    assert!(h.find("125%").is_some());
    // Tied to screen 2; the next choice is the whole space.
    assert!(h.find("Virtual-2").is_some());
    h.click_on("Virtual-2");
    h.click_on("Apply");
    let pending = h.app_mut().take_request().unwrap();
    assert_eq!(pending.request.speed, 125);
    assert_eq!(pending.request.ties, [("QEMU Virtio Tablet".to_string(), None)]);
}

#[test]
fn speed_stops_at_its_limits() {
    let mut h = open(&[]);
    for _ in 0..12 {
        h.click_on("Slower");
    }
    assert_eq!(h.app().plan().speed(), 50);
    assert!(is_disabled(&h, "Slower"));
    for _ in 0..12 {
        h.click_on("Faster");
    }
    assert_eq!(h.app().plan().speed(), 300);
    assert!(is_disabled(&h, "Faster"));
}

#[test]
fn ok_asks_and_closes_once_it_has_gone_through() {
    let mut h = open(&[]);
    h.click_on("OK");
    let pending = h.app_mut().take_request().expect("OK asks");
    assert!(pending.close);
    // The window waits for the answer.
    assert!(!h.finished());
    h.app_mut().accepted(true, 2, Ok(()));
    h.send(Event::Notified(SETTLED));
    assert!(h.app().is_done());
    assert!(h.finished());
}

#[test]
fn a_refusal_is_said_and_changes_nothing() {
    let mut h = open(&[]);
    h.click_on("OK");
    let pending = h.app_mut().take_request().unwrap();
    assert!(pending.close);
    h.app_mut().refused("that arrangement can't be used");
    h.send(Event::Notified(SETTLED));
    assert!(!h.finished() && !h.app().is_done() && !h.app().has_applied());
    let text = h.find("status").unwrap().1.what.label;
    assert_eq!(text, "The display server refused it: that arrangement can't be used");
    // The user can go on, and Apply asks again.
    h.click_on("Apply");
    assert!(h.app_mut().take_request().is_some());
}

#[test]
fn it_goes_through_but_cannot_be_kept() {
    let mut h = open(&[]);
    h.click_on("OK");
    h.app_mut().take_request().unwrap();
    h.app_mut().accepted(true, 2, Err("the disk is full".into()));
    h.send(Event::Notified(SETTLED));
    // Applied, so Cancel has something to put back; the window stays so that this is read.
    assert!(h.app().has_applied() && !h.finished());
    assert!(h.find("status").unwrap().1.what.label.contains("the disk is full"));
}

#[test]
fn apply_leaves_the_window_open_and_says_so() {
    let mut h = open(&[]);
    h.click_on("Apply");
    let pending = h.app_mut().take_request().unwrap();
    h.app_mut().accepted(pending.close, 2, Ok(()));
    h.send(Event::Notified(SETTLED));
    assert!(h.app().has_applied() && !h.finished());
    assert_eq!(h.find("status").unwrap().1.what.label, "Applied to 2 screens.");
    // A change since then takes the message away.
    h.click_on("2x");
    assert_eq!(h.find("status").unwrap().1.what.label, "");
}

#[test]
fn cancel_escape_and_closing_ask_for_nothing() {
    let mut h = open(&[]);
    h.click_on("Cancel");
    assert!(h.finished() && h.app_mut().take_request().is_none());
    assert!(!h.app().is_done());

    let mut h = open(&[]);
    h.key(Key::Escape);
    assert!(h.finished() && h.app_mut().take_request().is_none());

    let mut h = open(&[]);
    h.close(WindowId::MAIN);
    assert!(h.finished() && h.app_mut().take_request().is_none());
}

#[test]
fn enter_is_ok() {
    let mut h = open(&[]);
    h.key(Key::Enter);
    assert!(h.app_mut().take_request().is_some_and(|p| p.close));
}

#[test]
fn what_cancel_puts_back_is_how_it_was() {
    let mut h = open(&[]);
    h.click_on("2x");
    h.click_on("Move right");
    h.click_on("Apply");
    assert!(h.app_mut().take_request().is_some());
    assert_eq!(h.app().plan().opened().places, two_places());
    assert_eq!(h.app().plan().opened().speed, 100);
}

#[test]
fn it_fits_a_small_window() {
    // Made as small as it may be, everything is still reachable (it scrolls).
    let tablets = [("QEMU Virtio Tablet".to_string(), "RHT-1234-00000000 Virtual-2".to_string())];
    let mut h = open(&tablets);
    let (w, hgt) = settings().min_size;
    h.resize(w, hgt);
    assert!(h.find("OK").is_some() && h.find("Apply").is_some());
    let ok = h.find("OK").unwrap().1.rect;
    assert!(ok.y + ok.height <= hgt as i32 && ok.x + ok.width <= w as i32);
}

#[test]
fn it_fits_a_2x_screen_of_1024_x_768() {
    // The window and the title bar and taskbar around it go in 512 x 384 logical pixels.
    let (w, h) = settings().size;
    assert!(w <= 512 && h + 38 + 28 <= 384);
    let _ = screens_window::settings();
}

#[test]
fn how_it_looks() {
    let tablets = [("QEMU Virtio Tablet".to_string(), "RHT-1234-00000000 Virtual-2".to_string())];
    for (factor, name) in [(1, "screens-1x"), (2, "screens-2x")] {
        let h = Harness::with_scale(
            ScreensWindow::new(&two_places(), Some(125), &tablets),
            settings(),
            factor,
        );
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}
