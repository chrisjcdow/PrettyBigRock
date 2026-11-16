// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Asking before Shut down and Restart (`pbr_desktop::confirm`): only its button goes on;
//! Escape, Cancel and closing the window don't; and its look
//! (`tests/pictures/desktop/confirm-*.png`).

use pbr_desktop::confirm::{Confirm, Power, settings};
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{App, Key, WindowId};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/desktop")
}

#[test]
fn enter_goes_on_and_says_so() {
    let mut h = Harness::new(Confirm::new(Power::ShutDown), settings());
    assert_eq!(h.app().title(), "Shut down");
    assert_eq!(h.app().answer(), None);
    h.key(Key::Enter);
    assert_eq!(h.app().answer(), Some(true));
    assert!(h.finished());
}

#[test]
fn the_button_goes_on() {
    let mut h = Harness::new(Confirm::new(Power::Restart), settings());
    h.click_on("Restart");
    assert_eq!(h.app().answer(), Some(true));
    assert!(h.finished());
}

#[test]
fn escape_cancel_and_closing_do_not() {
    let mut h = Harness::new(Confirm::new(Power::ShutDown), settings());
    h.key(Key::Escape);
    assert_eq!(h.app().answer(), Some(false));
    assert!(h.finished());

    let mut h = Harness::new(Confirm::new(Power::ShutDown), settings());
    h.click_on("Cancel");
    assert_eq!(h.app().answer(), Some(false));
    assert!(h.finished());

    let mut h = Harness::new(Confirm::new(Power::Restart), settings());
    h.close(WindowId::MAIN);
    assert_eq!(h.app().answer(), None);
    assert!(h.finished());
}

#[test]
fn how_it_looks() {
    for (power, factor, name) in [
        (Power::ShutDown, 1, "confirm-shut-down-1x"),
        (Power::ShutDown, 2, "confirm-shut-down-2x"),
        (Power::Restart, 1, "confirm-restart-1x"),
    ] {
        let h = Harness::with_scale(Confirm::new(power), settings(), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}
