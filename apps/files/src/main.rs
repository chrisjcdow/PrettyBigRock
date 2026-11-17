// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Files as a program (docs/design/0026-apps-m6-tests-and-stages.md, section 1): the window in
//! `lib.rs`, run by sandstone, over the session's whole file tree and its sandbox manager.
//!
//! Files is *trusted* (note 0026, section 8), so its manifest gives it the whole tree the
//! session's user may see, the session's `pbr.sandbox` at start-up role 1000 (with a badge, a
//! number the service stamps on a capability to know who is calling, that lets it launch apps
//! and ask which can open a file) and the session's `pbr.users` at role 1001, which it asks for
//! the user's time zone. The dates in its window are shown in that zone.
//!
//! If the program is started with a path as its argument (`/Home/Documents`), it opens that
//! folder; a file's path opens the folder it is in, with it picked. With `describe` among its
//! arguments (test boots) it also writes what its window shows to the log, as the gallery does,
//! and at the start the file tree as `[tree] files ...` lines for the desktop boot's runner.
//!
//! The log (`files: ...`) says `folder PATH (N items)` whenever the folder shown, or what is in
//! it, changes; `made PATH`, `renamed PATH to PATH` and `deleted PATH`; `opening PATH with APP`
//! and `opened PATH with APP as app N`; and `couldn't ...` for what went wrong.

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
    use granite::endpoint::Endpoint;
    use pbr_browse::{BrowseError, Folders, Item};
    use pbr_files::{Error, FileError, Files, READ, READ_WRITE};
    use pbr_files_app::{FileManager, Opener, System};
    use pbr_interfaces::pbr_sandbox as sandbox;
    use pbr_interfaces::pbr_users as users;
    use pbr_sandstone::Runtime;
    use pbr_window::Connection;

    granite::main!(main);

    /// The start-up roles of the session's `pbr.sandbox` and `pbr.users` (the sandbox manager
    /// gives them to trusted apps, as to the text shell).
    const ROLE_SANDBOX: u32 = 1000;
    const ROLE_USERS: u32 = 1001;

    /// How long a call to the sandbox manager or the users service may take before Files gives
    /// up on it, so that a stuck service never freezes the window for long.
    const SERVICE_TIMEOUT_MS: u64 = 3_000;

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("files: {}", format_args!($($arg)*))
        };
    }

    /// The tree and the sandbox manager, for the library.
    struct Machine {
        files: Files,
        /// `None` if the app wasn't given the sandbox manager (it always is, when started by a
        /// session's sandbox manager).
        sandbox: Option<sandbox::Client>,
    }

    impl Machine {
        /// Starts `app` with the file at `path` (note 0025, section 3), as `run` does in the text
        /// shell, and gives back the new app's number.
        ///
        /// Choosing a file for an app *is* granting it (note 0017, section 6): Files opens the
        /// file, and hands the sandbox manager that capability with `choose`. The sandbox manager
        /// asks the file's server what it is, records it as the user's choice (or, if the app's
        /// own grants already reach it, gives nothing more), and the app gets that one file in its
        /// tree, at the same path, and the path as its argument. The grant can never be more than
        /// Files holds, and is read and write only if the user may change the file.
        fn launch(&self, app: &str, path: &str) -> Result<u32, String> {
            let sandbox = self.sandbox.as_ref().ok_or("Files wasn't given the sandbox manager")?;
            let file = match self.files.open(path, READ_WRITE) {
                Err(Error::File(FileError::NoRight | FileError::ReadOnly)) => {
                    self.files.open(path, READ)
                }
                other => other,
            }
            .map_err(pbr_files::describe)?;
            let launch = sandbox.prepare(app, path).map_err(|e| format!("{e}"))?;
            // Our copy of the capability goes with the call: `into_handle` gives it up.
            let node = Endpoint(file.into_handle());
            match sandbox.choose(launch, path, node).and_then(|()| sandbox.start(launch)) {
                // The sandbox manager keeps the app tied to itself; the process capability is
                // only for watching it, which Files doesn't do, so it is let go here.
                Ok(started) => Ok(started.app),
                Err(e) => {
                    // A launch left prepared would hold one of this app's few places for them
                    // until Files ends.
                    let _ = sandbox.cancel(launch);
                    Err(format!("{e}"))
                }
            }
        }
    }

    // The tree is `lib/files`' own, which `lib/browse` already browses; these only pass the
    // calls on. (`Files` has methods of the same names, so each is written out in full.)
    impl Folders for Machine {
        fn list(&mut self, path: &str) -> Result<Vec<Item>, BrowseError> {
            Folders::list(&mut self.files, path)
        }

        fn stat(&mut self, path: &str) -> Result<Item, BrowseError> {
            Folders::stat(&mut self.files, path)
        }

        fn make_folder(&mut self, path: &str) -> Result<(), BrowseError> {
            Folders::make_folder(&mut self.files, path)
        }

        fn make_file(&mut self, path: &str) -> Result<(), BrowseError> {
            Folders::make_file(&mut self.files, path)
        }

        fn rename(&mut self, from: &str, to: &str) -> Result<(), BrowseError> {
            Folders::rename(&mut self.files, from, to)
        }

        fn remove(&mut self, path: &str) -> Result<(), BrowseError> {
            Folders::remove(&mut self.files, path)
        }
    }

    impl System for Machine {
        fn openers(&mut self) -> Vec<Opener> {
            let Some(sandbox) = &self.sandbox else {
                say!("couldn't list the apps: it was not given the sandbox manager");
                return Vec::new();
            };
            match sandbox.openers() {
                Ok(list) => list
                    .into_iter()
                    .map(|o| Opener { app: o.app, title: o.title, opens: o.opens })
                    .collect(),
                Err(e) => {
                    say!("couldn't list the apps: {e}");
                    Vec::new()
                }
            }
        }

        fn open_with(&mut self, app: &str, path: &str) -> Result<u32, String> {
            self.launch(app, path)
        }
    }

    /// The user's time zone, in minutes east of UTC, as `pbr.users` says in its `timezone`
    /// setting (0 if it can't be told).
    fn time_zone(users: &users::Client) -> i32 {
        match users.settings() {
            Ok(list) => list
                .iter()
                .find(|s| s.key == "timezone")
                .and_then(|s| pbr_manifest::settings::timezone_minutes(&s.value))
                .unwrap_or(0),
            Err(e) => {
                say!("couldn't read the time zone: {e}");
                0
            }
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        // The arguments: `describe` is for the test boots; anything else is the path to show
        // (which may have spaces in it, so the rest is joined, not split).
        let args = String::from(start.args());
        let describe = args.split_whitespace().any(|a| a == "describe");
        let words: Vec<&str> = args.split_whitespace().filter(|a| *a != "describe").collect();
        let path = words.join(" ");
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let files = Files::from_start(&mut start);
        if describe {
            files.report("files");
        }
        let mut sandbox = start.take(ROLE_SANDBOX).map(|h| sandbox::Client::new(Endpoint(h)));
        if let Some(c) = &mut sandbox {
            c.set_timeout_ms(SERVICE_TIMEOUT_MS);
        }
        let mut users = start.take(ROLE_USERS).map(|h| users::Client::new(Endpoint(h)));
        if let Some(c) = &mut users {
            c.set_timeout_ms(SERVICE_TIMEOUT_MS);
        }
        let offset = users.as_ref().map_or(0, time_zone);
        let mut manager = FileManager::new(Machine { files, sandbox }, offset);
        if path.starts_with('/') {
            manager = manager.starting_at(&path);
        }
        let mut rt = Runtime::new(manager, pbr_files_app::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open.");
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        // What was logged while the window was being made comes out first.
        for line in rt.app_mut().take_log() {
            say!("{line}");
        }
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
        say!("closing");
        0
    }
}
