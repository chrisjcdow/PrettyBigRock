// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Notepad as a program (docs/design/0026-apps-m6-tests-and-stages.md, section 1): the window in
//! `lib.rs`, run by sandstone, over the files the sandbox manager gave it and the file picker.
//!
//! Notepad's manifest gives it Documents (read and change) and the picker. Anywhere else, the
//! user hands it one file at a time through the picker; what the picker chooses is put in
//! Notepad's file tree (`FilePicker::mount`), at the path the user saw, so that Notepad opens it
//! like any other file.
//!
//! If the program is started with a path as its argument (`/Home/Documents/letter.txt`, which
//! is what Files' "Open with…" and Terminal's `edit` do), it opens that file first. With
//! `describe` among its arguments (test boots) it also writes what its window shows to the log,
//! as the gallery does, and the lines the desktop boot's runner checks: `notepad: text N lines
//! N bytes crc XXXXXXXX` whenever the text changes, `notepad: opened PATH (N bytes, crc
//! XXXXXXXX)` and `notepad: saved PATH (N bytes, crc XXXXXXXX)`, `notepad: new`, `notepad:
//! asked save changes`, `notepad: couldn't ...` for what went wrong; and at the start, the file
//! tree as `[tree] notepad ...` lines.

#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

// The program is built for PrettyBigRock only; the crate's host build (its library's tests and
// Clippy) gets an empty `main` instead. `cfg_attr` applies an attribute only when the
// condition holds.
#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use pbr_files::{Error, FileError, Files, READ, READ_WRITE};
    use pbr_interfaces::pbr_picker::{PickerError, Place};
    use pbr_notepad::{Disk, DiskError, Notepad, PICKED, Pick, PickAnswer};
    use pbr_sandstone::Runtime;
    use pbr_sandstone::picker::{Ask, Chosen, FilePicker, PickError};
    use pbr_window::{Connection, Win};

    granite::main!(main);

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("notepad: {}", format_args!($($arg)*))
        };
    }

    /// Notepad's files and the file picker, for the library.
    struct Disks {
        files: Files,
        /// `None` if the app has no picker.
        picker: Option<FilePicker>,
    }

    impl Disks {
        /// Puts what the user chose in the tree, unless it can be reached already: a file in
        /// Documents, say, is inside the folder Notepad holds, and the tree refuses an entry
        /// inside another.
        fn reach(&mut self, chosen: Chosen) -> Result<String, String> {
            if self.files.open(&chosen.path, READ).is_ok() {
                return Ok(chosen.path);
            }
            FilePicker::mount(&mut self.files, chosen).map_err(pbr_files::describe)
        }
    }

    impl Disk for Disks {
        fn read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, DiskError> {
            let failed = |e: Error| match e {
                Error::NotFound | Error::File(FileError::NotFound) => DiskError::NotFound,
                e => DiskError::Failed(pbr_files::describe(e)),
            };
            let file = self.files.open(path, READ).map_err(failed)?;
            if file.info().map_err(failed)?.size > max as u64 {
                return Err(DiskError::TooBig);
            }
            // The size can change between asking and reading: stop at the limit anyway.
            let mut data = Vec::new();
            loop {
                let chunk = file.read_at(data.len() as u64, 65536).map_err(failed)?;
                if chunk.is_empty() {
                    return Ok(data);
                }
                data.extend_from_slice(&chunk);
                if data.len() > max {
                    return Err(DiskError::TooBig);
                }
            }
        }

        fn write(&mut self, path: &str, data: &[u8]) -> Result<(), DiskError> {
            let failed = |e: Error| DiskError::Failed(pbr_files::describe(e));
            let file = match self.files.open(path, READ_WRITE) {
                Ok(f) => f,
                // Not there yet (a new file started from the command line): make it.
                Err(Error::NotFound | Error::File(FileError::NotFound)) => {
                    self.files.create(path, false, READ_WRITE).map_err(failed)?
                }
                Err(e) => return Err(failed(e)),
            };
            // All of it, then its size (a shorter text than the file held), then the sync that
            // makes it last (note 0014).
            file.write_at(0, data).map_err(failed)?;
            file.set_size(data.len() as u64).map_err(failed)?;
            file.sync().map_err(failed)
        }

        fn pick(&mut self, window: Option<Win>, serial: u32, pick: Pick) -> Result<(), String> {
            let Some(picker) = &mut self.picker else {
                return Err(String::from("Notepad has no file picker"));
            };
            let ask = match pick {
                Pick::Open => Ask::open("open a text file"),
                Pick::Save { name } => Ask::save("save the text", &name),
            };
            // Both start in Documents, with text files first as the picker's filter (it offers
            // "All files" too, and adds the filter's first extension, `.txt`, to a name typed
            // without one).
            let ask = ask.start(Place::Documents).filter("Text files", "txt md");
            picker.open(window, serial, &ask).map_err(|e| format!("{e:?}"))
        }

        fn answer(&mut self) -> Option<PickAnswer> {
            let answer = self.picker.as_mut()?.take()?;
            Some(match answer {
                Ok(chosen) => match self.reach(chosen) {
                    Ok(path) => PickAnswer::Chosen(path),
                    Err(why) => PickAnswer::Failed(why),
                },
                Err(PickError::Picker(PickerError::Cancelled)) => PickAnswer::Cancelled,
                Err(e) => PickAnswer::Failed(format!("{e:?}")),
            })
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        // The arguments: `describe` is for the test boots; anything else is the path of a file
        // to open (which may have spaces in it, so the rest is joined, not split).
        let args = String::from(start.args());
        let describe = args.split_whitespace().any(|a| a == "describe");
        let words: Vec<&str> = args.split_whitespace().filter(|a| *a != "describe").collect();
        let file = words.join(" ");
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let files = Files::from_start(&mut start);
        if describe {
            files.report("notepad");
        }
        let picker =
            conn.notifier().ok().and_then(|n| FilePicker::from_start(&mut start, n, PICKED));
        let mut notepad = Notepad::new(Disks { files, picker });
        if file.starts_with('/') {
            notepad = notepad.with_file(&file);
        }
        let mut rt = Runtime::new(notepad, pbr_notepad::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open.");
        let mut last: Vec<String> = Vec::new();
        let mut last_text = String::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            for line in rt.app_mut().take_log() {
                say!("{line}");
            }
            if rt.app_mut().take_text_changed() && describe {
                // Only when the text really is different: a key that changes nothing says
                // nothing.
                let line = rt.app().text_line();
                if line != last_text {
                    say!("{line}");
                    last_text = line;
                }
            }
            if describe {
                let mut lines = Vec::new();
                for (id, win) in rt.windows() {
                    lines.push(format!("window {} is {}", id.0, win.number()));
                    for d in rt.describe(id) {
                        lines.push(format!("widget {} {}", id.0, d.line()));
                    }
                }
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
