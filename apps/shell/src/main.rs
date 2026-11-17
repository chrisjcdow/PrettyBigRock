// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `shell`: the text shell (docs/design/0018-supervision-shell-and-m5-tests.md, section 6).
//!
//! An app you type commands into, on the terminal the console serves. It is *trusted*
//! (note 0017, section 4): its manifest asks for the session's whole tree, and it is given
//! the session's sandbox manager and users service, so it can launch apps, show what they
//! hold and take it back. It has no powers beyond its session's: whatever its file tree holds
//! is all it can reach.
//!
//! The program is glue. The shell itself, its commands and everything they print, is in
//! `lib/shell` (`pbr-shell`, its `program` module), which Terminal (`apps/terminal`) runs too;
//! the parts that can be tested on the host are tested there. What is here is the part that is
//! only the text shell's: the terminal. [`Screen`] is the shell's [`Console`] over
//! `pbr.terminal`, the interface the console service serves (note 0018, section 5).
//!
//! What it is started with (a *start-up role* is a number a launcher gives each capability it
//! hands to a new program, so the program can find it; note 0009): its file tree, the services
//! `lib/shell` takes (roles 1000, 1001 and 1005), and role 1002, `pbr.terminal`.
//!
//! If the terminal is missing, or any call on it fails (the console has gone, or it is busy or
//! closed), the shell logs why and ends with 1. Its session starts it again (note 0018,
//! section 3).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use pbr_files::Files;
use pbr_interfaces::pbr_terminal::{self as terminal, TerminalError};
use pbr_shell::program::Shell;
use pbr_shell::{Console, Gone};

granite::main!(main);

/// The start-up role of the terminal (the sandbox manager hands it to apps granted `terminal`).
const ROLE_TERMINAL: u32 = 1002;

/// What the shell says first.
const WELCOME: &str = "PrettyBigRock shell. Type help for the commands.\n";

/// The shell's terminal: the console's, through `pbr.terminal`.
struct Screen {
    terminal: terminal::Client,
}

impl Screen {
    /// Logs why the terminal can't be used, for the person reading the log: the user can't be
    /// told, since the terminal is what shows them anything.
    fn lost(what: &str, e: CallError<TerminalError>) -> Gone {
        granite::println!("shell: the terminal failed while {what}: {e}");
        Gone
    }
}

impl Console for Screen {
    fn read_line(&self, prompt: &str) -> Result<String, Gone> {
        self.terminal.read_line(prompt).map_err(|e| Screen::lost("reading a line", e))
    }

    fn write(&self, text: &str) -> Result<(), Gone> {
        self.terminal.write(text).map_err(|e| Screen::lost("writing", e))
    }

    fn clear(&self) -> Result<(), Gone> {
        self.terminal.clear().map_err(|e| Screen::lost("clearing", e))
    }
}

fn main(mut start: granite::Start) -> i32 {
    let files = Files::from_start(&mut start);
    let Some(terminal) = start.take(ROLE_TERMINAL) else {
        granite::println!("shell: it was not given a terminal, so there is nowhere to type.");
        return 1;
    };
    let screen = Screen { terminal: terminal::Client::new(Endpoint(terminal)) };
    let mut shell = Shell::from_start(&mut start, files, screen);
    let ended = shell.say(WELCOME).and_then(|()| shell.run());
    match ended {
        Ok(()) => 0,
        Err(Gone) => 1,
    }
}
