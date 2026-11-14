// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The widget gallery as a program (note 0023, section 6; note 0026, section 6): the app in
//! `lib.rs`, run by sandstone, with everything the desktop boot's runner needs written to the
//! log. The runner can't see inside the window, so after every event that changed anything
//! the program writes a description of every window's widgets, a block from `gallery: view N`
//! to `gallery: view N done`:
//!
//! ```text
//! gallery: window 0 is 3                        (sandstone's window 0 is the display server's 3)
//! gallery: widget 0 button "Ask…" at 8,40 size 64x24 id ask
//! gallery: widget 0 text field "" at 8,70 size 200x24 id field focused value "hello"
//! ```
//!
//! Positions are the window's logical pixels; the display server's own description says where
//! the window is. What the app was sent is written too (`gallery: message Clicked`), and its
//! configure events (`gallery: configure 600x420 scale 200 focused`).

// The program is built for PrettyBigRock only; the crate's host build (its library's tests and
// Clippy) gets an empty `main` instead. `cfg_attr` applies an attribute only when the
// condition holds.
#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use pbr_sandstone::Runtime;
    use pbr_test_gallery::Gallery;
    use pbr_window::{Connection, Event};

    granite::main!(main);

    /// Logs a line with the program's prefix.
    macro_rules! say {
    ($($arg:tt)*) => {
        granite::println!("gallery: {}", format_args!($($arg)*))
    };
}

    fn main(mut start: granite::Start) -> i32 {
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let mut rt = Runtime::new(Gallery::new(), pbr_test_gallery::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            if let Event::Configure { size, scale, focused, .. } = event {
                let f = if focused { " focused" } else { "" };
                say!("configure {}x{} scale {scale}{f}", size.0, size.1);
            }
            let go = rt.handle(&mut *c, event);
            for m in rt.app_mut().take_log() {
                say!("message {m}");
            }
            let lines = describe(&rt);
            if lines != last {
                views += 1;
                say!("view {views}");
                for l in &lines {
                    say!("{l}");
                }
                say!("view {views} done");
                last = lines;
            }
            go
        });
        say!("closing");
        0
    }

    /// Every window of the app and its widgets, as lines.
    fn describe(rt: &Runtime<Gallery>) -> Vec<String> {
        let mut lines = Vec::new();
        for (id, win) in rt.windows() {
            lines.push(format!("window {} is {}", id.0, win.number()));
            for d in rt.describe(id) {
                lines.push(format!("widget {} {}", id.0, d.line()));
            }
        }
        lines
    }
}
