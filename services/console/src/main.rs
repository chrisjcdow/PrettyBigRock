// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The console service (docs/design/0018-supervision-shell-and-m5-tests.md, section 5): a text
//! terminal on the screen, for the shell. It finds the screen through the device manager and
//! takes the seat from the input service for its keys (`devices.rs`; note 0021, section 6), draws a grid of text on the screen with the kernel's own font
//! and colours (`screen.rs`), and serves `pbr.terminal` on its endpoint to whoever holds a
//! capability for it: `write`, `read_line`, `size` and `clear`.
//!
//! Everything about a terminal that is not hardware (the grid, wrapping, scrolling, editing a
//! line, what the keys mean, what is logged) is in `lib/terminal`, tested on the host. What is
//! left here is plumbing: which thread does what.
//!
//! - **This thread** (the main one) does everything the terminal needs: it waits for a call on
//!   the endpoint *or* a signal, handles it, and draws what changed. Nothing else touches the
//!   terminal, so it needs no locks.
//! - **A seat thread** and a **watcher** (`devices.rs`) sleep until they have something to
//!   say, put it in a queue and signal a notification that is *bound* to the endpoint. A bound
//!   notification makes `Endpoint::receive` return `Event::Signal` as if a call had come in, so
//!   waiting for calls and waiting for keys is one wait, with no polling (note 0008).
//!
//! A `read_line` call is not answered when it arrives: its `Reply` (the one-time capability for
//! answering a call) is kept until the user presses Enter, and the call's sender waits for that.
//! Only one can wait at a time: a second takes over, and the first is answered `Busy`.
//!
//! In self-test runs (`selftest` among the arguments) everything shown is also written to the
//! kernel log in the format of `lib/terminal`'s `mirror` module, because the test runner reads
//! the serial port, not the screen.

#![no_std]
#![no_main]

extern crate alloc;

mod devices;
mod screen;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use devices::{Devices, Event as Queued, Shared};
use granite::endpoint::{Endpoint, Event};
use granite::notification::Notification;
use granite::process::role;
use granite::thread;
use granite::time;
use pbr_interfaces::pbr_supervisor as supervisor;
use pbr_interfaces::pbr_terminal::{self as terminal_if, TerminalError};
use pbr_terminal::Terminal;
use screen::Screen;

granite::main!(main);

/// The size of the terminal while there is no screen to give it one.
const DEFAULT_COLUMNS: usize = 80;
const DEFAULT_ROWS: usize = 25;

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("console: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let selftest = start.args().split_whitespace().any(|w| w == "selftest");
    // Role `OWN`: the endpoint this service answers on. Role `PARENT`: `pbr.supervisor`, the
    // root program, which finds other services for this one (note 0018, section 2).
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let mut parent = supervisor::Client::new(Endpoint(
        start.take(role::PARENT).ok_or("started without a supervisor")?,
    ));
    parent.set_timeout_ms(10_000);

    // Bind before anything can signal, so that a signal is never missed (a notification keeps
    // its bits until they are received, but there is no reason to rely on that).
    let shared = Shared::new()?;
    endpoint.bind(shared.wake()).map_err(|e| format!("binding a notification: {e}"))?;

    // The watcher finds the devices (see `devices.rs` for why it is a thread of its own from the
    // start) and goes on watching for as long as the program runs. Until it has had its first
    // look, there is nothing to draw on and nobody could type, so nothing is served.
    let for_watcher = shared.clone();
    // Made here because making it can fail and a new thread has nobody to tell; the watcher
    // itself is made on its own thread, because it will hold the handles of threads it
    // starts, and those can't be passed from one thread to another.
    let seat_gone = Notification::create().map_err(|e| format!("making a notification: {e}"))?;
    let watcher = thread::spawn(move || Devices::new(parent, for_watcher, seat_gone).watch())
        .map_err(|e| format!("starting a thread: {e}"))?;
    if !shared.wait_settled() {
        granite::println!("console: the first look for devices took too long; carrying on.");
    }
    let mut console = Console::new(selftest, shared)?;
    console.process_events();
    console.announce(selftest);
    console.present();
    // (The watcher never ends, so it is never joined.)
    let _watcher = watcher;
    serve(&endpoint, &mut console)
}

/// Everything the service keeps. Only the main thread touches it.
struct Console {
    terminal: Terminal,
    screen: Option<Screen>,
    shared: Arc<Shared>,
    /// The `read_line` waiting for its line. It is there exactly while the terminal is reading.
    waiting: Option<terminal_if::read_line::Reply>,
    /// Where the cursor was when the screen was last drawn, to find the rows to redraw when it
    /// moves.
    drawn_caret: Option<(usize, usize)>,
}

impl Console {
    fn new(selftest: bool, shared: Arc<Shared>) -> Result<Console, String> {
        let terminal = Terminal::new(DEFAULT_COLUMNS, DEFAULT_ROWS, selftest)
            .ok_or("can't make a terminal")?;
        Ok(Console { terminal, screen: None, shared, waiting: None, drawn_caret: None })
    }

    /// Says what was found.
    fn announce(&self, selftest: bool) {
        let grid = self.terminal.grid();
        let keys = if self.shared.has_seat() { "keys from the input service" } else { "no keys" };
        match &self.screen {
            Some(screen) => {
                let l = screen.layout();
                granite::println!(
                    "console: {}x{}, {}x{} characters, {keys}.",
                    l.width,
                    l.height,
                    grid.columns(),
                    grid.rows(),
                );
            }
            None => granite::println!(
                "console: no screen, {}x{} characters, {keys}.",
                grid.columns(),
                grid.rows(),
            ),
        }
        if selftest {
            granite::println!("console: mirroring to the kernel log.");
        }
    }

    /// Handles what the other threads queued: keys, and a screen found.
    fn process_events(&mut self) {
        for event in self.shared.take() {
            match event {
                Queued::Key(key) => {
                    // The terminal drops keys when no line is being read.
                    if let Some(line) = self.terminal.key(key)
                        && let Some(reply) = self.waiting.take()
                    {
                        answered(reply.ok(&line));
                    }
                }
                Queued::Screen(screen) => self.set_screen(*screen),
            }
        }
    }

    /// Starts using a screen, a new one or the old one found again. The whole grid is drawn on
    /// it. If it has another size the terminal changes size and starts blank.
    fn set_screen(&mut self, screen: Screen) {
        let layout = screen.layout();
        let grid = self.terminal.grid();
        if (grid.columns(), grid.rows()) != (layout.columns, layout.rows)
            && !self.terminal.resize(layout.columns, layout.rows)
        {
            granite::println!(
                "console: a screen of {}x{} characters is no use.",
                layout.columns,
                layout.rows
            );
            return;
        }
        self.terminal.mark_all_dirty();
        self.drawn_caret = None;
        self.screen = Some(screen);
    }

    /// Draws what has changed since the last time: the rows that changed, or all of them if the
    /// grid scrolled (a scroll could move the pixels, but reading the screen's memory is slow,
    /// so the rows are drawn again), and the rows the cursor left and arrived on.
    fn present(&mut self) {
        let changes = self.terminal.take_changes();
        let caret = self.terminal.caret();
        if changes.is_empty() && caret == self.drawn_caret {
            return;
        }
        let Some(screen) = &mut self.screen else { return };
        let grid = self.terminal.grid();

        let mut redraw = vec![false; grid.rows()];
        if changes.scrolled > 0 {
            redraw.fill(true);
        } else {
            for row in changes.dirty_rows() {
                redraw[row] = true;
            }
        }
        if caret != self.drawn_caret {
            for (_, row) in self.drawn_caret.iter().chain(caret.iter()) {
                if let Some(r) = redraw.get_mut(*row) {
                    *r = true;
                }
            }
        }
        for row in (0..grid.rows()).filter(|r| redraw[*r]) {
            let caret_column = caret.filter(|c| c.1 == row).map(|c| c.0);
            screen.draw_row(row, grid.row(row), caret_column);
        }
        // Nothing drawn is seen until the buffer is shown.
        screen.show();
        self.drawn_caret = caret;
    }

    /// Writes the mirror's records to the kernel log (there are none outside self-test runs).
    fn flush_mirror(&mut self) {
        for line in self.terminal.take_mirror() {
            granite::println!("{line}");
        }
    }
}

/// Answers calls and handles keys, for ever.
fn serve(endpoint: &Endpoint, console: &mut Console) -> ! {
    let mut buffer = vec![0u8; terminal_if::REQUEST_BUFFER];
    loop {
        match endpoint.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(msg, reply)) => {
                // Keys typed before this call, while no line was being read, are dropped by
                // handling them first.
                console.process_events();
                let unknown = terminal_if::dispatch(&mut Calls(console), msg, &buffer, reply);
                if let Some((_, reply)) = unknown {
                    granite::interface::unknown(reply);
                }
            }
            Ok(Event::Signal(_)) => console.process_events(),
            Ok(Event::Gone(_)) | Err(granite::Error::TimedOut) => {}
            Err(e) => {
                granite::println!("console: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
        console.present();
        console.flush_mirror();
    }
}

/// Reports a failed answer (a caller that has gone is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("console: answer failed: {e}");
    }
}

/// `pbr.terminal` for the console. (`'a` is a *lifetime*: this borrows the console for as long
/// as it exists, so the compiler can check nothing else uses it meanwhile.)
struct Calls<'a>(&'a mut Console);

impl terminal_if::Server for Calls<'_> {
    fn write(&mut self, text: String, reply: terminal_if::write::Reply) {
        self.0.terminal.write(&text);
        answered(reply.ok());
    }

    fn read_line(&mut self, prompt: String, reply: terminal_if::read_line::Reply) {
        let console = &mut *self.0;
        // Without the seat no key can arrive, so nobody could ever answer. (Without a screen,
        // the line is still read, and mirrored.)
        if !console.shared.has_seat() {
            answered(reply.err(TerminalError::Closed));
            return;
        }
        // A newer `read_line` takes over from a waiting one, which is answered `Busy`. The
        // terminal has one user, the shell, which waits for each line before asking for the
        // next; so a second one means the first caller has most likely gone (it ended and was
        // started again), and there is no way to tell that from its reply until it is answered.
        if let Some(old) = console.waiting.take() {
            console.terminal.abandon_read();
            answered(old.err(TerminalError::Busy));
        }
        if console.terminal.begin_read(&prompt).is_err() {
            answered(reply.err(TerminalError::Busy));
        } else {
            // Not answered yet: the caller waits until Enter is pressed.
            console.waiting = Some(reply);
        }
    }

    fn size(&mut self, reply: terminal_if::size::Reply) {
        let grid = self.0.terminal.grid();
        answered(reply.ok(grid.columns() as u32, grid.rows() as u32));
    }

    fn clear(&mut self, reply: terminal_if::clear::Reply) {
        self.0.terminal.clear();
        answered(reply.ok());
    }
}
