// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The Welcome window on the host (note 0024, section 6a), against a pretend users service:
//! which page, the names made from what was typed, the disks offered, what each button asks
//! for, what a refusal says, and its look (`tests/pictures/welcome/`).

use super::*;
use pbr_sandstone::WindowId;
use pbr_sandstone::testing::Harness;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// What the pretend service was asked, and what it answers. `Rc<RefCell<…>>` lets the test
/// keep a handle on it while the window owns a copy (`Rc` shares, `RefCell` lets the shared
/// thing be changed).
#[derive(Default)]
struct State {
    me: Option<User>,
    system: Option<Result<Vec<sysinfo::VolumeInfo>, Failed>>,
    volumes: Vec<VolumeInfo>,
    /// What `add_first` and `setup` answer.
    refuse: Option<Failed>,
    asked: Vec<String>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<State>>);

impl Users for Fake {
    fn whoami(&mut self) -> Result<User, Failed> {
        self.0.borrow().me.clone().ok_or(None)
    }
    fn system_volumes(&mut self) -> Result<Vec<sysinfo::VolumeInfo>, Failed> {
        self.0.borrow().system.clone().unwrap_or(Err(None))
    }
    fn volumes(&mut self) -> Result<Vec<VolumeInfo>, Failed> {
        Ok(self.0.borrow().volumes.clone())
    }
    fn setup(&mut self, volume: u32, name: &str) -> Result<(), Failed> {
        let mut s = self.0.borrow_mut();
        s.asked.push(format!("setup {volume} {name}"));
        s.refuse.map_or(Ok(()), Err)
    }
    fn add_first(&mut self, user: &User) -> Result<(), Failed> {
        let mut s = self.0.borrow_mut();
        s.asked.push(format!("add_first {} {:?} {}", user.name, user.kind, user.display));
        s.refuse.map_or(Ok(()), Err)
    }
    fn answered(&mut self) -> Result<(), Failed> {
        self.0.borrow_mut().asked.push("answered".into());
        Ok(())
    }
    fn restart(&mut self) -> Result<(), Failed> {
        self.0.borrow_mut().asked.push("restart".into());
        Err(Some(UsersError::NotSupported))
    }
}

fn owner(kind: UserKind) -> User {
    User { name: "owner".into(), kind, display: "Owner".into() }
}

fn system(label: &str, in_memory: bool) -> sysinfo::VolumeInfo {
    sysinfo::VolumeInfo {
        label: label.into(),
        filesystem: "RockFS".into(),
        size: 256 << 20,
        free: 250 << 20,
        read_only: false,
        in_memory,
        system: true,
    }
}

fn volume(id: u32, label: &str, fs: Filesystem, state: VolumeState, in_memory: bool) -> VolumeInfo {
    VolumeInfo {
        id,
        disk: id,
        partition: 0,
        filesystem: fs,
        state,
        label: label.into(),
        size: 256 << 20,
        read_only: false,
        in_memory,
    }
}

/// On the ready-made disk, as `owner`.
fn on_disk() -> Fake {
    let f = Fake::default();
    {
        let mut s = f.0.borrow_mut();
        s.me = Some(owner(UserKind::Admin));
        s.system = Some(Ok(vec![system("Files", false)]));
    }
    f
}

/// In memory, with `volumes` beside the one in memory.
fn in_memory(volumes: Vec<VolumeInfo>) -> Fake {
    let f = Fake::default();
    {
        let mut s = f.0.borrow_mut();
        s.me = Some(owner(UserKind::Admin));
        s.system = Some(Ok(vec![system("Memory", true)]));
        s.volumes = volumes;
        s.volumes.insert(0, volume(9, "Memory", Filesystem::RockFs, VolumeState::Ready, true));
    }
    f
}

/// The window in sandstone's test harness, at the size for its page.
fn harness(w: Welcome<Fake>) -> Harness<Welcome<Fake>> {
    let s = settings(w.page());
    Harness::new(w, s)
}

fn asked(f: &Fake) -> Vec<String> {
    core::mem::take(&mut f.0.borrow_mut().asked)
}

#[test]
fn names_from_what_was_typed() {
    assert_eq!(names("Chris"), Some(("chris".into(), "Chris".into())));
    assert_eq!(names("  Mary Ann "), Some(("mary-ann".into(), "Mary Ann".into())));
    assert_eq!(names("r2_d2"), Some(("r2_d2".into(), "r2_d2".into())));
    for bad in ["", "   ", "Zoë", "a\"b", "x/y", "a.b", &"x".repeat(33)] {
        assert_eq!(names(bad), None, "{bad:?}");
    }
}

#[test]
fn sizes_in_words() {
    assert_eq!(size_text(256 << 20), "256 MB");
    assert_eq!(size_text(3 << 29), "1.5 GB");
    assert_eq!(size_text(20 << 30), "20 GB");
}

#[test]
fn on_a_disk_make_my_user() {
    let f = on_disk();
    let mut h = harness(Welcome::new(f.clone()));
    assert_eq!(h.app().page(), &Page::Disk { label: "Files".into() });
    let words = h.find("words").map(|(_, d)| d.what.label).unwrap_or_default();
    assert!(words.contains("\"Files\"") && words.contains("as Owner"), "{words}");
    assert!(h.find("Carry on as Owner").is_some());
    // The name box has the keyboard from the start.
    h.type_text("Dana");
    h.key(pbr_sandstone::Key::Enter);
    assert_eq!(asked(&f), ["add_first dana Admin Dana", "answered"]);
    assert_eq!(h.app().page(), &Page::Done { who: "Dana".into() });
    assert_eq!(h.app_mut().take_log(), ["page disk", "added dana", "answered", "page done"]);
    // Restart now asks; a computer that can't says so in the window.
    h.click_on("Restart now");
    assert_eq!(asked(&f), ["restart"]);
    assert!(h.app().problem().is_some_and(|p| p.starts_with("Restarting")));
    h.click_on("Later");
    assert!(h.finished());
}

#[test]
fn on_a_disk_carry_on() {
    let f = on_disk();
    let mut h = harness(Welcome::new(f.clone()));
    h.click_on("Carry on as Owner");
    assert_eq!(asked(&f), ["answered"]);
    assert!(h.finished());
}

#[test]
fn a_bad_name_or_a_refusal_is_said() {
    let f = on_disk();
    let mut h = harness(Welcome::new(f.clone()));
    h.click_on("Make my user");
    assert!(asked(&f).is_empty());
    assert!(h.app().problem().is_some_and(|p| p.starts_with("A name needs")));
    f.0.borrow_mut().refuse = Some(Some(UsersError::AlreadyExists));
    h.type_text("Owner");
    h.click_on("Make my user");
    assert_eq!(asked(&f), ["add_first owner Admin Owner"]);
    assert_eq!(h.app().problem(), Some("Making your user: that name or disk is taken already."));
    assert!(matches!(h.app().page(), Page::Disk { .. }));
    f.0.borrow_mut().refuse = Some(None);
    h.click_on("Make my user");
    assert_eq!(h.app().problem(), Some("Making your user: the users service didn't answer."));
}

#[test]
fn in_memory_the_disks_offered() {
    let list = vec![
        volume(1, "", Filesystem::Empty, VolumeState::Ready, false),
        volume(2, "Data", Filesystem::RockFs, VolumeState::Ready, false),
        volume(3, "USB", Filesystem::Fat32, VolumeState::Ready, false),
        volume(4, "Old", Filesystem::RockFs, VolumeState::Damaged, false),
        VolumeInfo {
            read_only: true,
            ..volume(5, "CD", Filesystem::RockFs, VolumeState::Ready, false)
        },
    ];
    let w = Welcome::new(in_memory(list));
    assert_eq!(
        w.page(),
        &Page::Memory {
            offers: vec![
                Offer { id: 1, text: "256 MB, empty".into() },
                Offer { id: 2, text: "Data, 256 MB, RockFS".into() },
            ]
        }
    );
}

#[test]
fn in_memory_set_up() {
    let f = in_memory(vec![volume(1, "", Filesystem::Empty, VolumeState::Ready, false)]);
    let mut h = harness(Welcome::new(f.clone()));
    let words = h.find("words").map(|(_, d)| d.what.label).unwrap_or_default();
    assert!(words.contains("kept in memory"), "{words}");
    // The only disk is picked already.
    h.type_text("Chris");
    h.click_on("Set up");
    assert_eq!(asked(&f), ["setup 1 chris", "answered"]);
    assert_eq!(h.app().page(), &Page::Done { who: "Chris".into() });
}

#[test]
fn in_memory_with_two_disks_one_is_picked_first() {
    let f = in_memory(vec![
        volume(1, "", Filesystem::Empty, VolumeState::Ready, false),
        volume(2, "Data", Filesystem::RockFs, VolumeState::Ready, false),
    ]);
    let mut h = harness(Welcome::new(f.clone()));
    h.type_text("Chris");
    h.click_on("Set up");
    assert!(asked(&f).is_empty(), "Set up can't be pressed with no disk picked");
    // Rows of a list: one pixel of edge, a heading of 21, rows of 20.
    let r = h.find("disks").expect("the disks").1.rect;
    h.click(r.x + 40, r.y + 1 + 21 + 20 + 10);
    h.click_on("Set up");
    assert_eq!(asked(&f), ["setup 2 chris", "answered"]);
}

#[test]
fn in_memory_with_no_disk() {
    let f = in_memory(Vec::new());
    let mut h = harness(Welcome::new(f.clone()));
    let words = h.find("words").map(|(_, d)| d.what.label).unwrap_or_default();
    assert!(words.contains("data.img"), "{words}");
    assert!(h.find("disks").is_none() && h.find("Set up").is_none());
    h.click_on("Not now");
    assert_eq!(asked(&f), ["answered"]);
    assert!(h.finished());
}

#[test]
fn nothing_for_a_standard_user_or_without_sysinfo() {
    let f = on_disk();
    f.0.borrow_mut().me = Some(owner(UserKind::Standard));
    let mut w = Welcome::new(f.clone());
    assert!(w.nothing_to_ask());
    w.answer_for_standard_user();
    assert_eq!(asked(&f), ["answered"]);
    let f = on_disk();
    f.0.borrow_mut().system = None;
    let w = Welcome::new(f);
    assert_eq!(w.page().name(), "problem");
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/welcome")
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "disk-1x"), (2, "disk-2x")] {
        let h = Harness::with_scale(
            Welcome::new(on_disk()),
            settings(&Page::Disk { label: String::new() }),
            factor,
        );
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    let f = in_memory(vec![volume(1, "", Filesystem::Empty, VolumeState::Ready, false)]);
    let h = harness(Welcome::new(f));
    h.check_window_picture_in(WindowId::MAIN, &dir(), "memory-1x");
}
