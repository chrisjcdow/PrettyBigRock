// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Snake as a program (note 0026, section 1): the window in `lib.rs`, run by sandstone. It
//! keeps the best score in `/Home/Apps/snake/best.txt` (its own folder, which the sandbox
//! manager made because the manifest says `data`) and starts the game's random numbers from
//! the clock.
//!
//! It writes what happens to the log (`snake: ...`). In test boots (`describe` among its
//! arguments) it also writes what its window shows, as the gallery does, and a line
//! `snake: STATE score S best B head X,Y food X,Y heading DIR length L` whenever the game
//! changes, which the desktop boot's runner steers by. Those boots start the game from a
//! fixed seed, so the runner plays a known game.

#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use granite::time;
    use pbr_files::{FileRights, Files};
    use pbr_sandstone::Runtime;
    use pbr_snake_app::{BEST_FILE, Snake, Store};
    use pbr_window::Connection;

    granite::main!(main);

    /// The seed of test boots, so that the runner plays a known game.
    const TEST_SEED: u64 = 0x5eed_5eed;

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("snake: {}", format_args!($($arg)*))
        };
    }

    /// The best score's file, in the app's own folder, reached through the program's file
    /// tree.
    struct Folder {
        files: Files,
        path: String,
    }

    impl Store for Folder {
        fn read(&mut self) -> Option<String> {
            let bytes = self.files.open(&self.path, FileRights::READ).ok()?.read_all().ok()?;
            String::from_utf8(bytes).ok()
        }

        fn write(&mut self, text: &str) -> Result<(), String> {
            let rw = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
            // The file isn't there the first time: make it.
            let file = match self.files.open(&self.path, rw) {
                Ok(f) => Ok(f),
                Err(_) => self.files.create(&self.path, false, rw),
            };
            file.and_then(|f| f.save(text.as_bytes())).map_err(pbr_files::describe)
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        let files = Files::from_start(&mut start);
        if describe {
            // Its file tree for the runner (note 0026, section 6): its own folder only.
            files.report("snake");
        }
        let path = format!("{}/{BEST_FILE}", Files::data_folder("snake"));
        let seed = if describe {
            TEST_SEED
        } else {
            time::now() ^ time::wall_now().unwrap_or(0).rotate_left(17)
        };
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let mut rt =
            Runtime::new(Snake::new(Folder { files, path }, seed), pbr_snake_app::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open.");
        let mut last: Vec<String> = Vec::new();
        let mut shown = String::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            for l in rt.app_mut().take_log() {
                say!("{l}");
            }
            if describe {
                let status = rt.app().status_line();
                if status != shown {
                    say!("{status}");
                    shown = status;
                }
                let mut lines = Vec::new();
                for (id, win) in rt.windows() {
                    lines.push(format!("window {} is {}", id.0, win.number()));
                    for d in rt.describe(id) {
                        lines.push(format!("widget {} {}", id.0, d.line()));
                    }
                }
                // Framed as the gallery's and the picker's are, so the runner reads a whole view.
                if lines != last {
                    views += 1;
                    say!("view {views}");
                    for l in &lines {
                        say!("{l}");
                    }
                    say!("view {views} done");
                    last = lines;
                }
            }
            go
        });
        say!("closing");
        0
    }
}
