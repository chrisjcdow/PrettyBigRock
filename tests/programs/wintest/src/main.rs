// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `wintest`: a well-behaved windowed program for the desktop boot tests (note 0026). It logs
//! what happens to its window, one line each, so that a boot test can check what the display
//! server did (focus, input routing, the clipboard, scales) by reading the kernel log.
//!
//! Arguments, in any order: `name=NAME` (default `test`), `colour=RRGGBB` (default `2f6b55`),
//! `size=WxH` in logical pixels (default `240x160`), and the flags `hang` (stop answering
//! pings after the first picture), `desktop` (act as the session's desktop: no window, arrange
//! the screens, at the start and whenever they change and aren't arranged yet) and
//! `resizable`. Every line starts `wintest NAME: `.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use pbr_draw::{Colour, Damage};
use pbr_interfaces::pbr_window::ScreenPlace;
use pbr_window::{Connection, Event, PointerKind, WindowFlags, WindowKind};

granite::main!(main);

/// `pbr.input`'s key code for C and V (the letters are numbered from A = 1), and the Ctrl bit
/// of `pbr.seat`'s modifiers.
const KEY_C: u16 = 3;
const KEY_V: u16 = 22;
const MOD_CTRL: u16 = 2;

/// What the arguments said.
struct Config {
    name: String,
    colour: u32,
    size: (u32, u32),
    hang: bool,
    desktop: bool,
    resizable: bool,
}

impl Config {
    fn parse(args: &str) -> Config {
        let mut c = Config {
            name: String::from("test"),
            colour: 0x2f6b55,
            size: (240, 160),
            hang: false,
            desktop: false,
            resizable: false,
        };
        for word in args.split_whitespace() {
            if let Some(n) = word.strip_prefix("name=") {
                c.name = String::from(n);
            } else if let Some(h) = word.strip_prefix("colour=") {
                if let Ok(v) = u32::from_str_radix(h, 16) {
                    c.colour = v & 0xFF_FFFF;
                }
            } else if let Some(s) = word.strip_prefix("size=") {
                if let Some((w, h)) = s.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    c.size = (w, h);
                }
            } else {
                match word {
                    "hang" => c.hang = true,
                    "desktop" => c.desktop = true,
                    "resizable" => c.resizable = true,
                    _ => {}
                }
            }
        }
        c
    }
}

/// Logs a line with the program's prefix.
macro_rules! say {
    ($name:expr, $($arg:tt)*) => {
        granite::println!("wintest {}: {}", $name, format_args!($($arg)*))
    };
}

fn main(mut start: granite::Start) -> i32 {
    let config = Config::parse(start.args());
    let mut conn = match Connection::from_start(&mut start) {
        Ok(c) => c,
        Err(e) => {
            say!(config.name, "can't connect: {e:?}");
            return 2;
        }
    };
    if config.desktop { run_desktop(&mut conn, &config) } else { run_window(&mut conn, &config) }
}

// ---- an ordinary window ----

fn run_window(conn: &mut Connection, config: &Config) -> i32 {
    let name = config.name.as_str();
    let flags = if config.resizable { WindowFlags::RESIZABLE } else { WindowFlags::default() };
    let win = match conn.create(WindowKind::Normal, name, config.size, flags) {
        Ok(w) => w,
        Err(e) => {
            say!(name, "can't make a window: {e:?}");
            return 2;
        }
    };
    say!(name, "window {}", win.number());

    let mut code = 0;
    let mut pointer_in = false;
    let mut committed = false;
    conn.run(|conn, event| {
        match event {
            Event::Configure { window, size, scale, focused, .. } => {
                if focused {
                    say!(name, "configure {}x{} scale {} focused", size.0, size.1, scale);
                } else {
                    say!(name, "configure {}x{} scale {}", size.0, size.1, scale);
                }
                // (Re)draw all of it, once the window has buffers.
                if conn.has_buffers(window) {
                    let ink = Colour::solid(config.colour);
                    match conn.draw(window, |canvas| {
                        canvas.fill(ink);
                        Damage::new()
                    }) {
                        Ok(true) => committed = true,
                        Ok(false) => {}
                        Err(e) => say!(name, "draw failed {e:?}"),
                    }
                    if committed && config.hang {
                        say!(name, "hanging");
                        loop {
                            granite::time::sleep_ms(60_000);
                        }
                    }
                }
            }
            Event::Key(k) => {
                if k.value == 1 || k.value == 2 {
                    match k.ch {
                        Some(c) => say!(name, "key {} '{}'", k.code, c),
                        None => say!(name, "key {}", k.code),
                    }
                    if k.value == 1 && k.modifiers & MOD_CTRL != 0 {
                        if k.code == KEY_C {
                            match conn.copy_text(k.serial, name) {
                                Ok(()) => say!(name, "copied"),
                                Err(e) => say!(name, "copy refused {e:?}"),
                            }
                        } else if k.code == KEY_V {
                            match conn.paste_text(k.serial) {
                                Ok(text) => say!(name, "pasted {}", text),
                                Err(e) => say!(name, "paste refused {e:?}"),
                            }
                        }
                    }
                }
            }
            Event::Pointer(p) => {
                if p.what == PointerKind::Left {
                    say!(name, "pointer out");
                    pointer_in = false;
                } else {
                    if !pointer_in {
                        say!(name, "pointer in");
                        pointer_in = true;
                    }
                    match p.what {
                        PointerKind::Button { button, pressed: true } => {
                            say!(name, "button {} pressed at {},{}", button, p.x, p.y)
                        }
                        PointerKind::Button { button, pressed: false } => {
                            say!(name, "button {} released at {},{}", button, p.x, p.y)
                        }
                        PointerKind::Scroll { axis, steps } => {
                            say!(name, "scroll {} {}", axis, steps)
                        }
                        _ => {}
                    }
                }
            }
            Event::Clipboard { text, picture } => {
                if text {
                    say!(name, "clipboard text");
                } else if picture {
                    say!(name, "clipboard picture");
                } else {
                    say!(name, "clipboard empty");
                }
            }
            Event::Lost(n) => say!(name, "lost {}", n),
            Event::CloseRequested { window } => {
                say!(name, "closing");
                let _ = conn.destroy(window);
                return false;
            }
            Event::Disconnected => {
                say!(name, "disconnected");
                code = 3;
                return false;
            }
            _ => {}
        }
        true
    });
    code
}

// ---- the desktop ----

/// The logical width of a screen at its current scale: its `size` is already logical, but is
/// about to be re-measured if the screen's scale changes, so it is worked back to pixels.
fn logical_width(s: &ScreenPlace) -> i32 {
    let scale = if s.scale == 0 { 100 } else { s.scale };
    (u64::from(s.size.width) * u64::from(scale) / 100).min(i32::MAX as u64) as i32
}

/// Asks for the screens, lowest number first.
fn screens(conn: &Connection, name: &str) -> Option<alloc::vec::Vec<ScreenPlace>> {
    match conn.client().screens() {
        Ok(mut list) => {
            list.sort_by_key(|s| s.screen);
            Some(list)
        }
        Err(e) => {
            say!(name, "screens failed {e:?}");
            None
        }
    }
}

/// Sends the arrangement, logging `done` if it was accepted.
fn arrange(conn: &Connection, name: &str, list: &[ScreenPlace], done: &str) {
    match conn.client().arrange(list) {
        Ok(()) => say!(name, "{}", done),
        Err(e) => say!(name, "arrange failed {e:?}"),
    }
}

/// The first screen at 0,0 at 1x and main, the second to its right at 2x, not mirroring.
fn arrange_side_by_side(list: &mut [ScreenPlace]) {
    let width = logical_width(&list[0]);
    list[0].x = 0;
    list[0].y = 0;
    list[0].scale = 100;
    list[0].main = true;
    list[1].x = width;
    list[1].y = 0;
    list[1].scale = 200;
    list[1].mirrors = 0;
}

/// Arranges the screens side by side if there are two or more and the second is neither
/// there already (at 2x) nor mirroring; says `one screen` once if there is only one.
fn arrange_if_needed(conn: &Connection, name: &str, said_one: &mut bool) {
    match screens(conn, name) {
        Some(mut list) if list.len() >= 2 => {
            if list[1].scale == 200 || list[1].mirrors != 0 {
                return;
            }
            arrange_side_by_side(&mut list);
            let n = list.len();
            arrange(conn, name, &list, &alloc::format!("arranged {n} screens"));
        }
        Some(list) if list.len() == 1 && !*said_one => {
            *said_one = true;
            say!(name, "one screen");
        }
        _ => {}
    }
}

fn run_desktop(conn: &mut Connection, config: &Config) -> i32 {
    let name = config.name.as_str();
    let mut said_one = false;
    arrange_if_needed(conn, name, &mut said_one);

    let mut code = 0;
    conn.run(|conn, event| {
        match event {
            Event::StartMenu => {
                say!(name, "start menu");
                if let Some(mut list) = screens(conn, name)
                    && list.len() >= 2
                {
                    if list[1].mirrors == list[0].screen {
                        arrange_side_by_side(&mut list);
                        let n = list.len();
                        arrange(conn, name, &list, &alloc::format!("arranged {n} screens"));
                    } else {
                        list[1].mirrors = list[0].screen;
                        list[1].scale = 100;
                        arrange(conn, name, &list, "mirroring");
                    }
                }
            }
            Event::WindowMenu { window, choice } => {
                say!(name, "window menu {:x} {}", window, choice)
            }
            Event::Screens => {
                say!(name, "screens changed");
                arrange_if_needed(conn, name, &mut said_one);
            }
            Event::Disconnected => {
                say!(name, "disconnected");
                code = 3;
                return false;
            }
            _ => {}
        }
        true
    });
    code
}
