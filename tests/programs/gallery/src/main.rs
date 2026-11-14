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
    use pbr_files::{FileRights, Files};
    use pbr_sandstone::Runtime;
    use pbr_sandstone::picker::{Ask, FilePicker, PickError};
    use pbr_test_gallery::Gallery;
    use pbr_window::{Connection, Event};

    /// The bit of the event loop's notification the file picker's answers come through.
    const PICKED: u32 = 9;

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
        // Its file tree (nothing of the user's: the gallery's manifest asks for no place), where
        // what the file picker gives is put, and the file picker (note 0025).
        let mut files = Files::from_start(&mut start);
        let mut picker =
            conn.notifier().ok().and_then(|n| FilePicker::from_start(&mut start, n, PICKED));
        say!("{} the file picker", if picker.is_some() { "has" } else { "hasn't" });
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
            // The picker's answer comes as the event loop's own bit.
            let picked = matches!(event, Event::Notified(bits) if bits & (1 << PICKED) != 0);
            let go = rt.handle(&mut *c, event);
            for m in rt.app_mut().take_log() {
                say!("message {m}");
            }
            if let Some(p) = &mut picker {
                if let Some(wish) = rt.app_mut().take_pick() {
                    let ask = if wish.save {
                        Ask::save("", "Gallery.txt").filter("Text", "txt")
                    } else {
                        Ask::open("").filter("Text", "txt md")
                    };
                    match p.open(wish.window, wish.serial, &ask) {
                        Ok(()) => {
                            say!("asked the picker to {}", if wish.save { "save" } else { "open" })
                        }
                        Err(e) => say!("can't ask the picker: {e:?}"),
                    }
                }
                if picked && let Some(answer) = p.take() {
                    let save = rt.app().text().as_bytes().to_vec();
                    use_answer(&mut files, &mut rt, answer, save);
                }
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

    /// What the picker answered: what was chosen is put in the gallery's tree; a file to save
    /// gets the field's text, and one opened puts its first line in the field.
    fn use_answer(
        files: &mut Files,
        rt: &mut Runtime<Gallery>,
        answer: Result<pbr_sandstone::picker::Chosen, PickError>,
        text: Vec<u8>,
    ) {
        let chosen = match answer {
            Ok(c) => c,
            Err(e) => return say!("the picker answered {e:?}"),
        };
        let path = match FilePicker::mount(files, chosen) {
            Ok(p) => p,
            Err(e) => {
                return say!("can't put what was chosen in the tree: {}", pbr_files::describe(e));
            }
        };
        say!("picked {path}");
        let rw = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
        // Saving: the file was given read and write; opening: read only, so it is read.
        match files.open(&path, rw) {
            Ok(f) => match f.save(&text) {
                Ok(()) => say!("saved {} bytes to {path}", text.len()),
                Err(e) => say!("can't save {path}: {}", pbr_files::describe(e)),
            },
            Err(_) => match files.open(&path, FileRights::READ).and_then(|f| f.read_all()) {
                Ok(data) => {
                    let text = alloc::string::String::from_utf8_lossy(&data);
                    rt.app_mut().set_text(&text);
                    say!("read {} bytes from {path}", data.len());
                }
                Err(e) => say!("can't read {path}: {}", pbr_files::describe(e)),
            },
        }
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
