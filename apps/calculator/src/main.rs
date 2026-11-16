// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Calculator as a program (note 0026, section 1): the window in `lib.rs`, run by sandstone.
//! It needs only a window: the display server's connection is all it is given.
//!
//! In test boots (`describe` among its arguments) it writes what its window shows to the log
//! each time that changes, as the gallery does, and `calculator: display TEXT` each time the
//! number on the display changes, which the desktop boot's runner checks sums with.

#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use pbr_calculator::Calculator;
    use pbr_sandstone::Runtime;
    use pbr_window::Connection;

    granite::main!(main);

    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("calculator: {}", format_args!($($arg)*))
        };
    }

    fn main(mut start: granite::Start) -> i32 {
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        if describe {
            // Its file tree for the runner (note 0026, section 6): it should have none.
            pbr_files::Files::from_start(&mut start).report("calculator");
        }
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let mut rt = Runtime::new(Calculator::new(), pbr_calculator::settings());
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
            if describe {
                let display = rt.app().display();
                if display != shown {
                    shown = String::from(display);
                    say!("display {display}");
                }
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
