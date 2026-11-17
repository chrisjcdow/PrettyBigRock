// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Terminal as a program (docs/design/0026-apps-m6-tests-and-stages.md, section 1): the window
//! in `lib.rs`, run by sandstone, and the shell of `lib/shell` on a thread of its own.
//!
//! Terminal is *trusted*, as the text shell is (note 0017, section 4): its manifest gives it the
//! session's whole tree, the session's sandbox manager (role 1000) and users service (role
//! 1001), and system information (role 1005). `Shell::from_start` takes those; this program
//! only adds the window and the queue between the two.
//!
//! **Two threads.** The event loop (this thread) draws the window and hears the keys. The shell
//! runs on a second thread, so that a command that takes a long time never freezes the window.
//! They meet in the queue of `link.rs`: the shell's output and its request for a line go to
//! the window, which rings bit [`WAKE`] of the event loop's notification (`App::notified`); the
//! user's line goes back to the shell, which waits for it on a notification of its own. When the
//! window closes the queue says so, the shell's next call fails, and the program ends (a shell
//! in the middle of a long command is ended with the rest of the program).
//!
//! The history (the last 100 lines, Up and Down) is kept in the file `history.txt` in the app's
//! own folder, `/Home/Apps/terminal`, which the sandbox manager made for it: read when the
//! window starts, written after each line.
//!
//! With `describe` among its arguments (test boots) it writes what its window shows to the log,
//! as the gallery does, and the terminal's mirror lines (what the screen shows and the lines
//! entered, as the console writes them in self-test boots), each after `terminal: `; and at the
//! start the file tree as `[tree] terminal ...` lines.

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
    use granite::notification::Notification;
    use granite::time;
    use pbr_files::{Error, FileError, Files, READ_WRITE};
    use pbr_sandstone::Runtime;
    use pbr_shell::program::Shell;
    use pbr_terminal_app::link::{Broken, Park, Ring};
    use pbr_terminal_app::{HISTORY_FILE, Store, TerminalApp, WAKE};
    use pbr_window::Connection;

    granite::main!(main);

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("terminal: {}", format_args!($($arg)*))
        };
    }

    /// The most the history file may be to be read: 100 lines of at most 4096 bytes, and a
    /// little over. A bigger file isn't one this program wrote.
    const MAX_HISTORY_FILE: u64 = 512 * 1024;

    /// The window's bell: the event loop's own notification (a copy that can only signal), and
    /// the bit that means "the shell has something for the window".
    struct WindowBell(Notification);

    impl Ring for WindowBell {
        fn ring(&self) {
            // If signalling fails the event loop is gone, and so is the window.
            let _ = self.0.signal(1 << WAKE);
        }
    }

    /// The shell thread's bell: a notification of its own, which it waits on and the window
    /// signals. Bit 0 is all it uses.
    struct ShellBell(Notification);

    impl Ring for ShellBell {
        fn ring(&self) {
            let _ = self.0.signal(1);
        }
    }

    impl Park for ShellBell {
        fn park(&self) -> Result<(), Broken> {
            // Waits for ever, until a bit is set; takes the bits, so the next wait is a new
            // one. An error means the notification is broken: the shell ends instead of
            // asking again at once.
            self.0.wait(time::FOREVER).map(|_| ()).map_err(|_| Broken)
        }
    }

    /// The history file: one capability for the file, opened (made if it isn't there) at start.
    /// `None` if that couldn't be done, and then nothing is kept.
    struct History {
        file: Option<pbr_files::File>,
        /// Why there is no file.
        why: String,
    }

    impl History {
        fn open(files: &Files) -> History {
            let path = format!("{}/{HISTORY_FILE}", Files::data_folder("terminal"));
            let opened = match files.open(&path, READ_WRITE) {
                // Not there yet (the first time): make it.
                Err(Error::NotFound | Error::File(FileError::NotFound)) => {
                    files.create(&path, false, READ_WRITE)
                }
                other => other,
            };
            match opened {
                Ok(file) => History { file: Some(file), why: String::new() },
                Err(e) => History {
                    file: None,
                    why: format!("can't open {path}: {}", pbr_files::describe(e)),
                },
            }
        }
    }

    impl Store for History {
        fn load(&mut self) -> Option<String> {
            let file = self.file.as_ref()?;
            if file.info().ok()?.size > MAX_HISTORY_FILE {
                return None;
            }
            Some(String::from_utf8_lossy(&file.read_all().ok()?).into_owned())
        }

        fn save(&mut self, text: &str) -> Result<(), String> {
            let Some(file) = &self.file else { return Err(self.why.clone()) };
            // Empties the file, writes, and makes it durable: on RockFS an atomic save, so a
            // power cut leaves the old history or the new one (note 0014, section 6).
            file.save(text.as_bytes()).map_err(pbr_files::describe)
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        let args = String::from(start.args());
        let describe = args.split_whitespace().any(|a| a == "describe");
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let files = Files::from_start(&mut start);
        if describe {
            files.report("terminal");
        }
        let history = History::open(&files);
        if history.file.is_none() {
            say!("{}", history.why);
        }

        // The queue, and the two bells on it.
        let (Ok(window_note), Ok(shell_note)) = (conn.notifier(), Notification::create()) else {
            say!("can't make the notifications the shell's thread needs");
            return 4;
        };
        let (window, console) =
            pbr_terminal_app::link::link(WindowBell(window_note), ShellBell(shell_note));

        // The shell, on a thread of its own. It says its welcome first, then reads and runs
        // lines until the window is gone.
        let mut shell = Shell::from_start(&mut start, files, console);
        let spawned = granite::thread::spawn(move || {
            let welcome = match shell.version() {
                Some((name, version)) => format!("{name} {version} terminal\n"),
                None => String::from("PrettyBigRock terminal\n"),
            };
            let ended = shell
                .say(&format!("{welcome}Type 'help' to see what I can do.\n\n"))
                .and_then(|()| shell.run());
            // `Err(Gone)` is the only way out, and it is how a closed window ends the shell.
            let _ = ended;
        });
        let _shell_thread = match spawned {
            Ok(t) => t,
            Err(e) => {
                say!("can't start the shell's thread: {e:?}");
                return 5;
            }
        };

        let mut rt = Runtime::new(
            TerminalApp::new(window.clone(), history, describe),
            pbr_terminal_app::settings(),
        );
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            window.close();
            return 3;
        }
        say!("open.");
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            for line in rt.app_mut().take_log() {
                say!("{line}");
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
        // The window is gone: the shell's next call (or the one it waits in) says so.
        window.close();
        say!("closing");
        0
    }
}
