// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! About as a program (note 0026, section 1): the window in `lib.rs`, run by sandstone, over the
//! `pbr.sysinfo` capability the sandbox manager gave it at start-up role 1005. It holds no
//! files.
//!
//! In test boots (`describe` among its arguments) it writes what its window shows, as the
//! gallery and the permissions viewer do, and each fact (`about: FACT VALUE`) when it changes,
//! for the desktop boot's runner; and at start the file tree it was given (`[tree]` lines).

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
    use pbr_about::{About, Reporter, Sysinfo};
    use pbr_files::Files;
    use pbr_interfaces::pbr_sysinfo::{self as sysinfo, ScreenSummary, VolumeInfo};
    use pbr_sandstone::Runtime;
    use pbr_window::Connection;

    granite::main!(main);

    /// The start-up role of its `pbr.sysinfo`.
    const SYSINFO: u32 = 1005;

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("about: {}", format_args!($($arg)*))
        };
    }

    /// `pbr.sysinfo`, through the generated client. An app that was started without the
    /// capability has no client, and every question is "can't be had".
    struct Info(Option<sysinfo::Client>);

    impl Sysinfo for Info {
        fn version(&mut self) -> Option<(String, String)> {
            let r = self.0.as_ref()?.version().ok()?;
            Some((r.name, r.version))
        }

        fn memory(&mut self) -> Option<(u64, u64)> {
            let r = self.0.as_ref()?.memory().ok()?;
            Some((r.total, r.available))
        }

        fn screens(&mut self) -> Option<Vec<ScreenSummary>> {
            self.0.as_ref()?.screens().ok()
        }

        fn volumes(&mut self) -> Option<Vec<VolumeInfo>> {
            self.0.as_ref()?.volumes().ok()
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        let client = start.take(SYSINFO).map(|cap| {
            let mut c = sysinfo::Client::new(Endpoint(cap));
            c.set_timeout_ms(5_000);
            c
        });
        if client.is_none() {
            say!("started without pbr.sysinfo");
        }
        let files = Files::from_start(&mut start);
        if describe {
            files.report("about");
        }
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let mut rt = Runtime::new(About::new(Info(client)), pbr_about::settings());
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open.");
        let mut reporter = Reporter::new();
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            if describe {
                for line in reporter.report(rt.app().facts()) {
                    say!("{line}");
                }
                let mut lines = Vec::new();
                for (id, win) in rt.windows() {
                    lines.push(format!("window {} is {}", id.0, win.number()));
                    for d in rt.describe(id) {
                        // The uptime's label changes every second: it is left out here (the
                        // `about: uptime` line says it, at most every 10 seconds), so that
                        // the window isn't described again each second.
                        if d.key.as_deref() == Some("Uptime") {
                            lines.push(format!("widget {} uptime label", id.0));
                        } else {
                            lines.push(format!("widget {} {}", id.0, d.line()));
                        }
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
