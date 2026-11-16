// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! About on the host: the words for each fact against a pretend `pbr.sysinfo`, the uptime
//! ticking, what is written to the log, and its look (`tests/pictures/about/`).

use super::*;
use pbr_sandstone::WindowId;
use pbr_sandstone::testing::Harness;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// What the pretend `pbr.sysinfo` knows; `None` is a question it answers with `NotReady`.
#[derive(Default)]
struct World {
    version: Option<(String, String)>,
    memory: Option<(u64, u64)>,
    screens: Option<Vec<ScreenSummary>>,
    volumes: Option<Vec<VolumeInfo>>,
}

/// A pretend `pbr.sysinfo`. `Rc<RefCell<…>>` lets a test keep a handle on what the window
/// asks about, and change it (`Rc` shares, `RefCell` lets the shared thing be changed).
#[derive(Clone, Default)]
struct Fake(Rc<RefCell<World>>);

impl Sysinfo for Fake {
    fn version(&mut self) -> Option<(String, String)> {
        self.0.borrow().version.clone()
    }
    fn memory(&mut self) -> Option<(u64, u64)> {
        self.0.borrow().memory
    }
    fn screens(&mut self) -> Option<Vec<ScreenSummary>> {
        self.0.borrow().screens.clone()
    }
    fn volumes(&mut self) -> Option<Vec<VolumeInfo>> {
        self.0.borrow().volumes.clone()
    }
}

fn screen(width: u32, height: u32, scale: u32) -> ScreenSummary {
    ScreenSummary { connector: "Virtual-1".to_string(), width, height, scale }
}

fn volume(label: &str, size: u64, free: u64, in_memory: bool, system: bool) -> VolumeInfo {
    VolumeInfo {
        label: label.to_string(),
        filesystem: "RockFS".to_string(),
        size,
        free,
        read_only: false,
        in_memory,
        system,
    }
}

/// A computer that answers everything: one screen, 256 MB with 100 free, a 2 GB disk.
fn fake() -> Fake {
    let f = Fake::default();
    *f.0.borrow_mut() = World {
        version: Some(("PrettyBigRock".to_string(), "2.0.0-alpha.1".to_string())),
        memory: Some((256 * MB, 100 * MB)),
        screens: Some(vec![screen(1024, 768, 100)]),
        volumes: Some(vec![
            volume("Data", 8 * GB, GB, false, false),
            volume("System", 2 * GB, 3 * GB / 2, false, true),
        ]),
    };
    f
}

/// What the window shows for a fact: the value beside its label.
fn shown(h: &Harness<About<Fake>>, fact: &str) -> String {
    h.find(fact).map(|(_, d)| d.what.label).unwrap_or_else(|| panic!("no fact {fact}"))
}

#[test]
fn sizes_in_1_3s_units() {
    assert_eq!(fmt_size(0), "0 bytes");
    assert_eq!(fmt_size(1023), "1023 bytes");
    assert_eq!(fmt_size(1024), "1 KB");
    assert_eq!(fmt_size(1536), "1.5 KB");
    assert_eq!(fmt_size(100 * 1024), "100 KB");
    assert_eq!(fmt_size(MB), "1 MB");
    assert_eq!(fmt_size(MB + MB / 4), "1.2 MB");
    assert_eq!(fmt_size(20 * GB), "20 GB");
    assert_eq!(fmt_size(3 * GB / 2), "1.5 GB");
    assert_eq!(fmt_size(1024 * GB), "1 TB");
    // Past the biggest unit it stays in terabytes.
    assert_eq!(fmt_size(u64::MAX), "16777215 TB");
}

#[test]
fn display_in_words() {
    assert_eq!(display_text(&[screen(1024, 768, 100)]), "1024 x 768");
    assert_eq!(display_text(&[screen(1920, 1080, 200)]), "1920 x 1080, 2x");
    assert_eq!(
        display_text(&[screen(1024, 768, 100), screen(1280, 720, 200)]),
        "1024 x 768; 1280 x 720, 2x"
    );
    assert_eq!(display_text(&[screen(800, 600, 150)]), "800 x 600, 1.5x");
    assert_eq!(display_text(&[screen(800, 600, 125)]), "800 x 600, 1.25x");
    assert_eq!(display_text(&[screen(800, 600, 105)]), "800 x 600, 1.05x");
    assert_eq!(display_text(&[]), "No screen");
}

#[test]
fn memory_uptime_and_disk_in_words() {
    assert_eq!(memory_text(256 * MB, 100 * MB), "256 MB, 100 MB free");
    assert_eq!(memory_text(MB - 1, 0), "0 MB, 0 MB free");
    assert_eq!(uptime_text(0), "0 h 0 min 0 s");
    assert_eq!(uptime_text(59), "0 h 0 min 59 s");
    assert_eq!(uptime_text(3723), "1 h 2 min 3 s");
    assert_eq!(uptime_text(100 * 3600 + 61), "100 h 1 min 1 s");
    let disk = |v: &[VolumeInfo]| disk_text(v);
    // The system volume is the one told about, whatever else is there.
    assert_eq!(
        disk(&[
            volume("Data", 8 * GB, GB, false, false),
            volume("S", 2 * GB, 3 * GB / 2, false, true)
        ]),
        "1.5 GB free of 2 GB"
    );
    assert_eq!(disk(&[volume("S", 64 * MB, 40 * MB, true, true)]), "40 MB free (in memory)");
    assert_eq!(disk(&[volume("S", 512, 100, false, true)]), "100 bytes free of 512 bytes");
    assert_eq!(disk(&[volume("Data", GB, GB, false, false)]), "No disk");
    assert_eq!(disk(&[]), "No disk");
}

#[test]
fn the_window_shows_the_facts() {
    let h = Harness::new(About::new(fake()), settings());
    assert_eq!(shown(&h, "Display"), "1024 x 768");
    assert_eq!(shown(&h, "Memory"), "256 MB, 100 MB free");
    assert_eq!(shown(&h, "Uptime"), "0 h 0 min 1 s");
    assert_eq!(shown(&h, "Disk"), "1.5 GB free of 2 GB");
    let facts = h.app().facts();
    assert_eq!(facts.name.as_deref(), Some("PrettyBigRock"));
    assert_eq!(facts.version.as_deref(), Some("2.0.0-alpha.1"));
}

#[test]
fn two_screens_at_2x() {
    let f = fake();
    f.0.borrow_mut().screens = Some(vec![screen(1024, 768, 100), screen(1920, 1080, 200)]);
    let h = Harness::new(About::new(f), settings());
    assert_eq!(shown(&h, "Display"), "1024 x 768; 1920 x 1080, 2x");
}

#[test]
fn a_fact_not_ready_shows_a_dash_and_is_asked_again() {
    let f = Fake::default();
    let mut h = Harness::new(About::new(f.clone()), settings());
    for fact in ["Display", "Memory", "Disk"] {
        assert_eq!(shown(&h, fact), UNKNOWN);
    }
    assert_eq!(h.app().facts().name, None);
    // The service comes up: the next tick has everything.
    *f.0.borrow_mut() = fake().0.take();
    h.advance(1000);
    assert_eq!(shown(&h, "Display"), "1024 x 768");
    assert_eq!(shown(&h, "Memory"), "256 MB, 100 MB free");
    assert_eq!(shown(&h, "Disk"), "1.5 GB free of 2 GB");
    assert_eq!(h.app().facts().version.as_deref(), Some("2.0.0-alpha.1"));
}

#[test]
fn a_fact_that_goes_missing_keeps_its_last_value() {
    let f = fake();
    let mut h = Harness::new(About::new(f.clone()), settings());
    f.0.borrow_mut().memory = None;
    h.advance(1000);
    assert_eq!(shown(&h, "Memory"), "256 MB, 100 MB free");
}

#[test]
fn the_disk_is_asked_about_every_ten_seconds() {
    let f = fake();
    let mut h = Harness::new(About::new(f.clone()), settings());
    let free = |gb: u64| vec![volume("System", 2 * GB, gb * GB / 4, false, true)];
    f.0.borrow_mut().volumes = Some(free(4));
    // Within the ten seconds the free space shown stays as it was first asked.
    h.advance(5000);
    assert_eq!(shown(&h, "Disk"), "1.5 GB free of 2 GB");
    h.advance(5000);
    assert_eq!(shown(&h, "Disk"), "1 GB free of 2 GB");
}

#[test]
fn uptime_ticks_once_a_second_and_changes_nothing_else() {
    let mut h = Harness::new(About::new(fake()), settings());
    let before = h.app().facts().clone();
    h.advance(3000);
    let after = h.app().facts().clone();
    assert_eq!(before.uptime, Some(1));
    assert_eq!(after.uptime, Some(4));
    assert_eq!(Facts { uptime: None, ..before }, Facts { uptime: None, ..after });
    assert_eq!(shown(&h, "Uptime"), "0 h 0 min 4 s");
    // Not before the second is up.
    h.advance(500);
    assert_eq!(h.app().facts().uptime, Some(4));
}

#[test]
fn the_log_says_each_fact_when_it_changes() {
    let f = fake();
    let mut h = Harness::new(About::new(f.clone()), settings());
    let mut r = Reporter::new();
    assert_eq!(
        r.report(h.app().facts()),
        [
            "name PrettyBigRock",
            "version 2.0.0-alpha.1",
            "display 1024 x 768",
            "memory 256 MB, 100 MB free",
            "uptime 0 h 0 min 1 s",
            "disk 1.5 GB free of 2 GB",
        ]
    );
    // Nothing changed: nothing is said, and a few seconds of uptime are not worth a line.
    h.advance(5000);
    assert!(r.report(h.app().facts()).is_empty());
    // Memory changed: just that. The uptime waits until 10 seconds after its last line.
    f.0.borrow_mut().memory = Some((256 * MB, 90 * MB));
    h.advance(1000);
    assert_eq!(r.report(h.app().facts()), ["memory 256 MB, 90 MB free"]);
    h.advance(4000);
    assert_eq!(r.report(h.app().facts()), ["uptime 0 h 0 min 11 s"]);
    // A fact that never came is written once, as a dash.
    let mut r = Reporter::new();
    let h = Harness::new(About::new(Fake::default()), settings());
    let lines = r.report(h.app().facts());
    assert_eq!(lines.len(), 6);
    assert!(lines.contains(&format!("memory {UNKNOWN}")));
    assert!(lines.contains(&format!("uptime {UNKNOWN}")) || h.app().facts().uptime.is_some());
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/about")
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "about-1x"), (2, "about-2x")] {
        let mut h = Harness::with_scale(About::new(fake()), settings(), factor);
        // A fixed uptime: the harness's clock starts at 1 s, so this is 1 min 5 s.
        h.advance(64_000);
        assert_eq!(h.app().facts().uptime, Some(65));
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
}
