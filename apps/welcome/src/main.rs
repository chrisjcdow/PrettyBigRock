// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The Welcome window as a program (note 0024, section 6a): the window in `lib.rs`, run by
//! sandstone, over the `pbr.users` capability the sandbox manager gave it at start-up role 1001
//! (the one for `users welcome`, which can do only what this window needs) and `pbr.sysinfo`
//! at role 1005. It holds no files.
//!
//! It writes what it does to the log (`welcome: page disk`, `welcome: added dana`), and in test
//! boots (`describe` among its arguments) what its window shows, as the permissions viewer
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
    use granite::interface::CallError;
    use pbr_interfaces::pbr_sysinfo as sysinfo;
    use pbr_interfaces::pbr_users::{self as users, User, UsersError, VolumeInfo};
    use pbr_sandstone::Runtime;
    use pbr_welcome::{Failed, Users, Welcome};
    use pbr_window::Connection;

    granite::main!(main);

    /// The start-up roles of its `pbr.users` and `pbr.sysinfo`.
    const USERS: u32 = 1001;
    const SYSINFO: u32 = 1005;

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("welcome: {}", format_args!($($arg)*))
        };
    }

    /// The users service and system information, through the generated clients.
    struct Service {
        users: users::Client,
        sysinfo: Option<sysinfo::Client>,
    }

    /// A refusal is the service's answer; anything else (it has gone, it timed out) is no
    /// answer.
    fn failed(e: CallError<UsersError>) -> Failed {
        match e {
            CallError::Service(e) => Some(e),
            _ => None,
        }
    }

    impl Users for Service {
        fn whoami(&mut self) -> Result<User, Failed> {
            self.users.whoami().map_err(failed)
        }

        fn system_volumes(&mut self) -> Result<Vec<sysinfo::VolumeInfo>, Failed> {
            let s = self.sysinfo.as_ref().ok_or(None)?;
            s.volumes().map_err(|_| None)
        }

        fn volumes(&mut self) -> Result<Vec<VolumeInfo>, Failed> {
            self.users.volumes().map_err(failed)
        }

        fn setup(&mut self, volume: u32, name: &str) -> Result<(), Failed> {
            self.users.setup(volume, name).map_err(failed)
        }

        fn add_first(&mut self, user: &User) -> Result<(), Failed> {
            self.users.add_first(user).map_err(failed)
        }

        fn answered(&mut self) -> Result<(), Failed> {
            let (key, value) =
                (pbr_manifest::settings::WELCOME, pbr_manifest::settings::WELCOME_DONE);
            self.users.set(key, value).map_err(failed)
        }

        fn restart(&mut self) -> Result<(), Failed> {
            self.users.restart().map_err(failed)
        }
    }

    fn main(mut start: granite::Start) -> i32 {
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        let Some(cap) = start.take(USERS) else {
            say!("started without pbr.users");
            return 1;
        };
        let mut client = users::Client::new(Endpoint(cap));
        // Setting a disk up formats it and waits for its filesystem server: allow for that.
        client.set_timeout_ms(60_000);
        let info = start.take(SYSINFO).map(|cap| {
            let mut c = sysinfo::Client::new(Endpoint(cap));
            c.set_timeout_ms(5_000);
            c
        });
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let window = Welcome::new(Service { users: client, sysinfo: info });
        let settings = pbr_welcome::settings(window.page());
        let mut rt = Runtime::new(window, settings);
        // A standard user has nothing to be asked: no window at all.
        if rt.app().nothing_to_ask() {
            let w = rt.app_mut();
            w.answer_for_standard_user();
            for l in w.take_log() {
                say!("{l}");
            }
            return 0;
        }
        if !rt.open(&mut conn) {
            say!("the display server refused the window");
            return 3;
        }
        say!("open.");
        let mut last: Vec<String> = Vec::new();
        let mut views = 0u32;
        conn.run(|c, event| {
            let go = rt.handle(&mut *c, event);
            for l in rt.app_mut().take_log() {
                say!("{l}");
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
