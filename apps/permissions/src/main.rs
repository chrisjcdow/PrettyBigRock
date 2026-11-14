// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The permissions viewer as a program (note 0025, section 4): the window in `lib.rs`, run by
//! sandstone, over the `pbr.sandbox` capability the sandbox manager gave it at start-up role
//! 1000 (one that may only look, take back and stop). Launched from the start menu it opens on
//! the first running app; from a window's menu ("What can this app reach?") the desktop
//! launches it with that app's number as its argument, and it opens on that app.
//!
//! What it takes back or stops is written to the log (`permissions: took PATH back from APP`),
//! and in test boots (`describe` among its arguments) what its window shows, as the gallery
//! does, for the desktop boot's runner.

#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use granite::endpoint::Endpoint;
    use pbr_interfaces::pbr_sandbox::{self as sandbox, AppDetails, AppInfo, Grant};
    use pbr_permissions::{Apps, Viewer};
    use pbr_sandstone::Runtime;
    use pbr_window::Connection;

    granite::main!(main);

    /// The start-up role of its `pbr.sandbox`.
    const SANDBOX: u32 = 1000;

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("permissions: {}", format_args!($($arg)*))
        };
    }

    /// The sandbox manager, through the generated client.
    struct Manager(sandbox::Client);

    impl Apps for Manager {
        fn list(&mut self) -> Result<Vec<AppInfo>, String> {
            self.0.list().map_err(|e| format!("{e}"))
        }

        fn details(&mut self, app: u32) -> Result<AppDetails, String> {
            self.0.details(app).map_err(|e| format!("{e}"))
        }

        fn grants(&mut self, app: u32) -> Result<Vec<Grant>, String> {
            // Grants come a page at a time (a trusted app can hold 256).
            let mut out = Vec::new();
            let mut from = 0;
            loop {
                let page = self.0.grants(app, from).map_err(|e| format!("{e}"))?;
                out.extend(page.grants);
                if page.next == 0 || out.len() >= 256 {
                    return Ok(out);
                }
                from = page.next;
            }
        }

        fn revoke(&mut self, app: u32, path: &str) -> Result<(), String> {
            self.0.revoke(app, path).map_err(|e| format!("{e}"))
        }

        fn stop(&mut self, app: u32) -> Result<(), String> {
            self.0.stop(app).map_err(|e| format!("{e}"))
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        let app: Option<u32> = start.args().split_whitespace().find_map(|a| a.parse().ok());
        let Some(sb) = start.take(SANDBOX) else {
            say!("started without pbr.sandbox");
            return 1;
        };
        let mut client = sandbox::Client::new(Endpoint(sb));
        client.set_timeout_ms(5_000);
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let mut rt = Runtime::new(Viewer::new(Manager(client), app), pbr_permissions::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open{}.", app.map(|a| format!(" on app {a}")).unwrap_or_default());
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            for l in rt.app_mut().take_log() {
                say!("{l}");
            }
            if describe {
                let mut lines = Vec::new();
                // What the app shown holds, row by row (the list's description says only which
                // row is picked): the runner compares it with what the sandbox manager says.
                let v = rt.app();
                if let Some(app) = v.shown() {
                    for h in v.held() {
                        lines.push(format!("app {app} holds {}", h.text));
                    }
                }
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
