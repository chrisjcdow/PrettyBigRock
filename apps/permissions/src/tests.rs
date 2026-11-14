// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The permissions viewer on the host (note 0025, section 5): the words for each grant reason
//! and each service, against a pretend sandbox manager; Take back and Stop; following apps as
//! they start and end; and its look (`tests/pictures/permissions/`).

use super::*;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Description, WindowId};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A pretend sandbox manager: its apps, each with details and grants. `Rc<RefCell<…>>` lets
/// the test keep a handle on what the viewer changes (`Rc` shares, `RefCell` lets the shared
/// thing be changed).
#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Vec<(AppDetails, Vec<Grant>)>>>);

impl Apps for Fake {
    fn list(&mut self) -> Result<Vec<AppInfo>, String> {
        Ok(self
            .0
            .borrow()
            .iter()
            .map(|(d, _)| AppInfo {
                id: d.id,
                app: d.app.clone(),
                trusted: d.trusted,
                running: true,
            })
            .collect())
    }
    fn details(&mut self, app: u32) -> Result<AppDetails, String> {
        self.0
            .borrow()
            .iter()
            .find(|(d, _)| d.id == app)
            .map(|(d, _)| d.clone())
            .ok_or("NotFound".into())
    }
    fn grants(&mut self, app: u32) -> Result<Vec<Grant>, String> {
        self.0
            .borrow()
            .iter()
            .find(|(d, _)| d.id == app)
            .map(|(_, g)| g.clone())
            .ok_or("NotFound".into())
    }
    fn revoke(&mut self, app: u32, path: &str) -> Result<(), String> {
        let mut apps = self.0.borrow_mut();
        let (_, grants) = apps.iter_mut().find(|(d, _)| d.id == app).ok_or("NotFound")?;
        let at = grants.iter().position(|g| g.path == path).ok_or("NotFound")?;
        grants.remove(at);
        Ok(())
    }
    fn stop(&mut self, app: u32) -> Result<(), String> {
        let mut apps = self.0.borrow_mut();
        let at = apps.iter().position(|(d, _)| d.id == app).ok_or("NotFound")?;
        apps.remove(at);
        Ok(())
    }
}

const RW: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

fn grant(path: &str, rights: FileRights, reason: GrantReason) -> Grant {
    Grant { path: path.to_string(), rights, reason }
}

fn details(id: u32, app: &str, title: &str, trusted: bool, services: u32) -> AppDetails {
    AppDetails {
        id,
        app: app.to_string(),
        title: title.to_string(),
        trusted,
        system: false,
        services: AppServices(services),
        started: 0,
    }
}

/// Notepad (with Documents, its own folder, a chosen file), Terminal (trusted), and Snake.
fn fake() -> Fake {
    let f = Fake::default();
    let w = AppServices::WINDOWS.0;
    f.0.borrow_mut().extend([
        (
            details(
                3,
                "notepad",
                "Notepad",
                false,
                w | AppServices::PICKER.0 | AppServices::DATA.0,
            ),
            vec![
                grant("/Home/Documents", RW, GrantReason::Manifest),
                grant("/Home/Apps/notepad", RW, GrantReason::Manifest),
                grant("/Volumes/PBR13/README.TXT", FileRights::READ, GrantReason::Chosen),
            ],
        ),
        (
            details(
                5,
                "terminal",
                "Terminal",
                true,
                w | AppServices::USERS.0 | AppServices::SANDBOX.0 | AppServices::SYSINFO.0,
            ),
            vec![
                grant("/Home", RW, GrantReason::Trusted),
                grant("/System/rules.txt", FileRights::READ, GrantReason::Trusted),
            ],
        ),
        (
            details(7, "snake", "Snake", false, w),
            vec![grant("/Home/Pictures", FileRights::READ, GrantReason::Rule)],
        ),
    ]);
    f
}

fn texts(v: &Viewer<Fake>) -> Vec<&str> {
    v.held().iter().map(|h| h.text.as_str()).collect()
}

fn widget(h: &Harness<Viewer<Fake>>, name: &str) -> Description {
    h.find(name).unwrap_or_else(|| panic!("no widget {name}")).1
}

/// A point in row `n` of a list (one pixel of edge, a heading of 21, rows of 20).
fn row_at(h: &Harness<Viewer<Fake>>, list: &str, n: i32) -> (i32, i32) {
    let r = widget(h, list).rect;
    (r.x + 40, r.y + 1 + 21 + 20 * n + 10)
}

#[test]
fn each_grant_reason_and_service_in_words() {
    let v = Viewer::new(fake(), Some(3));
    assert_eq!(
        texts(&v),
        [
            "/Home/Documents: read and change, asked for in its manifest",
            "Its own folder (/Home/Apps/notepad): read and change",
            "/Volumes/PBR13/README.TXT: read, you chose it",
            "Windows",
            "The file picker",
        ]
    );
    let v = Viewer::new(fake(), Some(5));
    assert_eq!(
        texts(&v),
        [
            "/Home: read and change, it is trusted with all your files",
            "/System/rules.txt: read, it is trusted with all your files",
            "Windows",
            "System information",
            "Your account and settings",
            "Launching and managing apps",
        ]
    );
    let v = Viewer::new(fake(), Some(7));
    assert_eq!(texts(&v), ["/Home/Pictures: read, allowed by your rules", "Windows"]);
    // Services can't be taken back one by one: they have no path.
    assert!(v.held()[1].path.is_none());
    // Opened with no app (the start menu), or an app that has gone, it shows the first by
    // title.
    assert_eq!(Viewer::new(fake(), None).shown(), Some(3));
    assert_eq!(Viewer::new(fake(), Some(99)).shown(), Some(3));
    assert_eq!(Viewer::new(fake(), None).app_names(), ["notepad", "snake", "terminal"]);
}

#[test]
fn ages() {
    let s = 1_000_000_000;
    assert_eq!(ago(0, 0), "just now");
    assert_eq!(ago(0, 30 * s), "30 seconds ago");
    assert_eq!(ago(0, 180 * s), "3 minutes ago");
    assert_eq!(ago(0, 3 * 3600 * s), "3 hours ago");
    assert_eq!(ago(10 * s, 0), "just now");
}

#[test]
fn take_back_and_stop() {
    let f = fake();
    let mut h = Harness::new(Viewer::new(f.clone(), Some(3)), settings());
    // Nothing picked: Take back does nothing.
    h.click_on("Take back");
    assert_eq!(f.0.borrow()[0].1.len(), 3);
    // A service can't be taken back.
    let (x, y) = row_at(&h, "held", 3);
    h.click(x, y);
    h.click_on("Take back");
    assert_eq!(f.0.borrow()[0].1.len(), 3);
    // The chosen file can.
    let (x, y) = row_at(&h, "held", 2);
    h.click(x, y);
    h.click_on("Take back");
    assert_eq!(f.0.borrow()[0].1.len(), 2);
    assert_eq!(h.app_mut().take_log(), ["took /Volumes/PBR13/README.TXT back from notepad"]);
    assert!(!texts(h.app()).iter().any(|t| t.contains("README")));
    // Stop: the app goes, and the next one is shown.
    h.click_on("Stop app");
    assert_eq!(h.app().app_names(), ["snake", "terminal"]);
    assert_eq!(h.app().shown(), Some(7));
    assert_eq!(h.app_mut().take_log(), ["stopped notepad"]);
}

#[test]
fn it_follows_apps_as_they_start_and_end() {
    let f = fake();
    let mut h = Harness::new(Viewer::new(f.clone(), Some(7)), settings());
    let w = AppServices::WINDOWS.0;
    f.0.borrow_mut().push((details(9, "about", "About", false, w), Vec::new()));
    h.advance(1100);
    assert_eq!(h.app().app_names(), ["about", "notepad", "snake", "terminal"]);
    // Snake ends: the viewer moves to the first.
    f.0.borrow_mut().retain(|(d, _)| d.id != 7);
    h.advance(1100);
    assert_eq!(h.app().shown(), Some(9));
    // Picking another in the list shows it.
    let (x, y) = row_at(&h, "apps", 2);
    h.click(x, y);
    assert_eq!(h.app().shown(), Some(5));
}

#[test]
fn a_sandbox_manager_that_says_no() {
    #[derive(Clone)]
    struct No;
    impl Apps for No {
        fn list(&mut self) -> Result<Vec<AppInfo>, String> {
            Err("NotAllowed".into())
        }
        fn details(&mut self, _: u32) -> Result<AppDetails, String> {
            Err("NotAllowed".into())
        }
        fn grants(&mut self, _: u32) -> Result<Vec<Grant>, String> {
            Err("NotAllowed".into())
        }
        fn revoke(&mut self, _: u32, _: &str) -> Result<(), String> {
            Err("NotAllowed".into())
        }
        fn stop(&mut self, _: u32) -> Result<(), String> {
            Err("NotAllowed".into())
        }
    }
    let h = Harness::new(Viewer::new(No, None), settings());
    assert_eq!(h.app().problem(), Some("The sandbox manager didn't answer: NotAllowed"));
    assert!(h.find("No app is running.").is_some());
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/permissions")
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "viewer-1x"), (2, "viewer-2x")] {
        let mut h = Harness::with_scale(Viewer::new(fake(), Some(3)), settings(), factor);
        let (x, y) = row_at(&h, "held", 2);
        h.click(x, y);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}
