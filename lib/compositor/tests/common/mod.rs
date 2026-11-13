// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the drawing tests share: a [`Sheet`] to compare with the reference pictures kept in
//! `tests/pictures/compositor/` (as `lib/draw/tests/common` does for `tests/pictures/draw`,
//! docs/design/0023-draw-window-and-sandstone.md, section 6), apps' pictures made of a
//! checkerboard ([`Patterns`]), and a [`Rig`]: a real `Desk` with screens and windows, driven
//! the way the display server drives it.
//!
//! After a change to the look that is meant: `PBR_UPDATE_PICTURES=1 cargo test -p
//! pbr-compositor`, then look at each picture that changed before committing it.

// Each test file uses its own part of this.
#![allow(dead_code)]

use pbr_compositor::draw::{Frames, Target, compose};
use pbr_compositor::event::kind;
use pbr_compositor::{ClientId, Desk, Out, Output, Place, Shown, WindowId};
use pbr_draw::{Picture, Rect};
use pbr_interfaces::pbr_window::{WindowFlags, WindowKind};
use pbr_seat::Event as SeatEvent;
use pbr_seat::event::{kind as seat, value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const SEC: u64 = 1_000_000_000;

/// Pixels of a screen, owned by the test.
#[derive(Clone)]
pub struct Sheet {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

impl Sheet {
    /// A sheet of one colour.
    pub fn new(width: u32, height: u32, fill: u32) -> Sheet {
        Sheet { pixels: vec![fill; (width * height) as usize], width, height }
    }

    /// The pixel at (`x`, `y`).
    pub fn at(&self, x: u32, y: u32) -> u32 {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Copies `other` onto this sheet with its top left corner at (`x`, `y`).
    pub fn paste(&mut self, other: &Sheet, x: u32, y: u32) {
        for row in 0..other.height {
            let from = (row * other.width) as usize;
            let to = ((y + row) * self.width + x) as usize;
            self.pixels[to..to + other.width as usize]
                .copy_from_slice(&other.pixels[from..from + other.width as usize]);
        }
    }

    /// Compares the sheet with the reference picture `name`, and fails the test with what
    /// differs if it isn't the same, pixel for pixel.
    pub fn check(&self, name: &str) {
        // A composed screen is always solid.
        if let Some(i) = self.pixels.iter().position(|p| p >> 24 != 0xFF) {
            let (x, y) = (i as u32 % self.width, i as u32 / self.width);
            panic!("{name}: the pixel at {x},{y} isn't solid: {:#010x}", self.pixels[i]);
        }
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/pictures/compositor")
            .join(format!("{name}.png"));
        let got = pbr_testpng::Picture {
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
        };
        if let Err(why) = pbr_testpng::check(&file, &got) {
            panic!("{name}: {why}");
        }
    }
}

/// Apps' pictures: a checkerboard of two colours for each window, so that stretching and
/// halving show.
#[derive(Default)]
pub struct Patterns {
    frames: BTreeMap<WindowId, Vec<u32>>,
}

impl Patterns {
    /// Makes window `id`'s picture for `shown`'s buffers: squares of 8 device pixels.
    pub fn add(&mut self, id: WindowId, shown: &Shown, a: u32, b: u32) {
        let (w, h) = (shown.buffers.width, shown.buffers.height);
        let mut v = vec![0u32; (shown.buffers.stride / 4 * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let on = (x / 8 + y / 8) % 2 == 0;
                v[(y * shown.buffers.stride / 4 + x) as usize] = if on { a } else { b };
            }
        }
        self.frames.insert(id, v);
    }
}

impl Frames for Patterns {
    fn picture(&self, id: WindowId, shown: &Shown) -> Option<Picture<'_>> {
        let v = self.frames.get(&id)?;
        Picture::new(v, shown.buffers.width, shown.buffers.height, shown.buffers.stride / 4)
    }
}

/// A screen of `w` by `h` device pixels, `mm` millimetres wide (so 2x if `mm` is small).
pub fn output(index: u32, w: u32, h: u32, mm: u32) -> Output {
    Output {
        device: 7,
        index,
        connector: format!("Virtual-{}", index + 1),
        identity: String::from("QEMU 1234"),
        width: w,
        height: h,
        width_mm: mm,
        height_mm: mm * h / w.max(1),
    }
}

/// A desk with screens and an active session, and apps' pictures.
pub struct Rig {
    pub desk: Desk,
    pub session: u32,
    pub now: u64,
    pub patterns: Patterns,
    pub scratch: Vec<u32>,
    /// What the desk asked the display server to do, kept until used.
    sent: Vec<Out>,
    desktop: Option<ClientId>,
}

impl Rig {
    pub fn new(screens: &[Output]) -> Rig {
        let mut desk = Desk::new();
        desk.set_outputs(screens, 0);
        let session = desk.start_session("chris").unwrap();
        assert!(desk.activate(session));
        desk.take_out();
        Rig {
            desk,
            session,
            now: 10 * SEC,
            patterns: Patterns::default(),
            scratch: Vec::new(),
            sent: Vec::new(),
            desktop: None,
        }
    }

    /// An app the user launched just now (so its first window takes the focus).
    pub fn client(&mut self, title: &str, flags: u32) -> ClientId {
        self.desk.add_client(self.session, 1, title, flags, self.now).unwrap()
    }

    /// A window of `kind`, with buffers, committed and drawn as a checkerboard.
    pub fn open(&mut self, c: ClientId, kind: WindowKind, title: &str, size: (u32, u32)) -> u32 {
        let n =
            self.desk.create(c, kind, 0, title, size, WindowFlags(0), (0, 0), 0, self.now).unwrap();
        self.draw(c, n);
        n
    }

    /// Answers the window's latest configure with buffers and a committed frame.
    pub fn draw(&mut self, c: ClientId, n: u32) {
        self.sent.extend(self.desk.take_out());
        let serial = self
            .sent
            .iter()
            .rev()
            .find_map(|o| match o {
                Out::Event(to, e) if *to == c && e.window == n && e.kind == kind::CONFIGURE => {
                    Some(e.serial)
                }
                _ => None,
            })
            .expect("a configure event");
        self.desk.buffers(c, n, serial).unwrap();
        self.sent.clear();
        self.desk.commit(c, n, 0, &[], self.now).unwrap();
        self.sent.extend(self.desk.take_out());
        let id = WindowId { client: c, number: n };
        let shown =
            self.desk.scene().windows.iter().find(|w| w.id == id).and_then(|w| w.shown).unwrap();
        // Different windows get different colours.
        let colours = [(0xe8b04b, 0xfbe3b0), (0x4b8fe8, 0xc4dcfb), (0xe86a8f, 0xfbc4d4)];
        let (a, b) = colours[(c as usize + n as usize) % colours.len()];
        self.patterns.add(id, &shown, a, b);
    }

    /// Changes screens' places (as the desktop does).
    pub fn arrange(&mut self, places: &[(u32, Place)]) {
        let d = match self.desktop {
            Some(d) => d,
            None => {
                let d = self
                    .desk
                    .add_client(
                        self.session,
                        99,
                        "Desktop",
                        pbr_compositor::desk::flags::DESKTOP,
                        0,
                    )
                    .unwrap();
                self.desktop = Some(d);
                d
            }
        };
        self.desk.arrange(d, places, self.now).unwrap();
        self.sent.extend(self.desk.take_out());
    }

    fn input(&mut self, kind: u16, code: u16, value: i32) {
        let e = SeatEvent {
            kind,
            device: 1,
            code,
            mods: 0,
            value,
            first: 0,
            second: 0,
            time: self.now,
        };
        self.desk.seat(&e, self.now);
        self.sent.extend(self.desk.take_out());
    }

    /// Where the pointer is, in the space.
    fn pointer(&self) -> (i32, i32) {
        let lines = self.desk.describe(0);
        let Some(l) = lines.iter().find(|l| l.starts_with("pointer at ")) else { return (0, 0) };
        let (a, b) = l.split_whitespace().nth(2).unwrap().split_once(',').unwrap();
        (a.parse().unwrap(), b.parse().unwrap())
    }

    /// Moves the pointer to (`x`, `y`) with a mouse.
    pub fn point(&mut self, x: i32, y: i32) {
        let (cx, cy) = self.pointer();
        self.input(seat::MOVE, 1, x - cx);
        self.input(seat::MOVE, 2, y - cy);
    }

    pub fn button(&mut self, code: u16, down: bool) {
        self.input(seat::BUTTON, code, if down { value::PRESSED } else { value::RELEASED });
    }

    /// Drags a window by its title bar so that its content's corner is at (`x`, `y`).
    pub fn drag(&mut self, c: ClientId, n: u32, x: i32, y: i32) {
        let id = WindowId { client: c, number: n };
        let content = self.desk.scene().windows.iter().find(|w| w.id == id).unwrap().content;
        self.point(content.x + 30, content.y - 13);
        self.button(1, true);
        self.point(x + 30, y - 13);
        self.button(1, false);
    }

    /// Composes screen `screen` whole.
    pub fn render(&mut self, screen: u32) -> Sheet {
        let o = &self.desk.space().screen(screen).unwrap().output;
        let mut sheet = Sheet::new(o.width, o.height, 0xFF00_00FF);
        self.render_into(screen, &mut sheet, None);
        sheet
    }

    /// Composes screen `screen` into `sheet`, inside `areas`; the rectangles it says changed.
    pub fn render_into(
        &mut self,
        screen: u32,
        sheet: &mut Sheet,
        areas: Option<&[Rect]>,
    ) -> Vec<Rect> {
        let target = Target {
            pixels: &mut sheet.pixels,
            width: sheet.width,
            height: sheet.height,
            stride: sheet.width,
        };
        compose(
            &self.desk.scene(),
            self.desk.space(),
            screen,
            target,
            areas,
            &self.patterns,
            &mut self.scratch,
        )
    }
}
