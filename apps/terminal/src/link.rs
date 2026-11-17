// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The queue between Terminal's window and the shell that runs on a thread of its own.
//!
//! Terminal has two threads. The **window** thread runs the event loop: it draws, and it
//! hears the keys. The **shell** thread runs `lib/shell`'s shell, which is written as if it were
//! the only thing in the program: it asks for a line and waits, runs the command, and prints.
//! A command may take a long time (a service that doesn't answer for a minute), and the window
//! must not freeze meanwhile, so the shell can't be run by the event loop. The two meet here.
//!
//! The shell's side is [`ShellEnd`], which is the shell's [`Console`]:
//! - `write` and `clear` add to a list of output and ring the window's bell; they never wait;
//! - `read_line` leaves the prompt for the window and rings its bell, then **waits** until the
//!   window hands back the line the user typed.
//!
//! The window's side is [`WindowEnd`]: when its bell rings it takes the output and the prompt
//! ([`WindowEnd::take_work`]), shows them, and when the user presses Enter gives the line to the
//! shell ([`WindowEnd::answer`]). When the window closes, [`WindowEnd::close`] makes everything
//! the shell tries from then on say that the terminal is gone ([`Gone`]), so it ends.
//!
//! **Waiting without losing a wake-up.** Both sides keep the shared facts (what is queued, the
//! answer, whether it is closed) in one [`Lock`], and change them *before* ringing the other
//! side's bell. A bell is *sticky*: ringing it when nobody is listening leaves it ringing, and
//! the next wait ends at once. So a side that looks at the facts, finds nothing to do, and goes
//! to wait can't miss a ring that came in between: the ring is still there when it waits. A
//! wait may also end with nothing to do (two rings were saved up into one), so the shell
//! always looks again after waiting, and never trusts the wait alone. Nothing here spins.
//!
//! What a bell *is* is not decided here. On PrettyBigRock it is a notification object's bits
//! (`granite::notification`): signalling sets a bit, waiting returns when any is set and
//! clears them. In the tests it is a flag and a condition variable.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use granite::lock::Lock;
use pbr_shell::{Console, Gone};

/// Something that can be rung to wake the other side. Sticky: a ring with nobody waiting is
/// kept until the next [`Park::park`]. `Send + Sync` because it is rung from another thread.
pub trait Ring: Send + Sync {
    /// Rings the bell. Never waits.
    fn ring(&self);
}

/// A bell that can be waited on, for the shell's thread.
pub trait Park: Ring {
    /// Waits until the bell has been rung (at once if it has been since the last wait), and
    /// forgets the rings. `Err` if waiting can't work at all (the object behind the bell is
    /// broken), which the shell takes as the terminal being gone: it never spins on a wait
    /// that fails at once.
    fn park(&self) -> Result<(), Broken>;
}

/// Waiting on a bell failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Broken;

/// One piece of what the shell printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Text to show after what is there.
    Text(String),
    /// Empty the screen.
    Clear,
}

/// What the window takes from the queue each time its bell rings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// What the shell printed since last time, in order, with text that came in pieces joined.
    pub output: Vec<Output>,
    /// The prompt of a line the shell is waiting for, if it has asked since last time.
    pub ask: Option<String>,
}

/// What the two sides share, under the lock.
#[derive(Default)]
struct State {
    output: Vec<Output>,
    /// A prompt the window hasn't started reading at yet.
    asking: Option<String>,
    /// The line the user typed, until the shell takes it.
    answer: Option<String>,
    /// The window has gone.
    closed: bool,
}

/// What both ends hold: the facts, and the two bells.
struct Shared {
    state: Lock<State>,
    /// Rung for the window: output, or a prompt, is waiting.
    window: Box<dyn Ring>,
    /// Rung for the shell: an answer is waiting, or the window closed.
    shell: Box<dyn Park>,
}

/// Makes the two ends of a queue: `window` is rung when the shell has something for the window
/// (on PrettyBigRock, a bit of the event loop's notification), and `shell` when the window has
/// something for the shell (a notification of the shell thread's own).
pub fn link(window: impl Ring + 'static, shell: impl Park + 'static) -> (WindowEnd, ShellEnd) {
    let shared = Arc::new(Shared {
        state: Lock::new(State::default()),
        window: Box::new(window),
        shell: Box::new(shell),
    });
    (WindowEnd(shared.clone()), ShellEnd(shared))
}

/// The window's end.
#[derive(Clone)]
pub struct WindowEnd(Arc<Shared>);

impl WindowEnd {
    /// Everything the shell has queued since the last call.
    pub fn take_work(&self) -> Work {
        let mut state = self.0.state.lock();
        Work { output: core::mem::take(&mut state.output), ask: state.asking.take() }
    }

    /// Gives the shell the line the user typed (the answer to its `read_line`).
    pub fn answer(&self, line: String) {
        self.0.state.lock().answer = Some(line);
        self.0.shell.ring();
    }

    /// The window is closing: the shell's `read_line` and every call after it say the terminal
    /// is gone.
    pub fn close(&self) {
        self.0.state.lock().closed = true;
        self.0.shell.ring();
    }
}

/// The shell's end: its [`Console`].
pub struct ShellEnd(Arc<Shared>);

impl ShellEnd {
    /// Leaves `prompt` for the window, and rings it. [`Gone`] if the window has closed. This
    /// is the first half of [`Console::read_line`], on its own so that a test can do the halves
    /// one at a time.
    pub fn ask(&self, prompt: &str) -> Result<(), Gone> {
        {
            let mut state = self.0.state.lock();
            if state.closed {
                return Err(Gone);
            }
            // An answer left over from a line that was abandoned must not answer this one.
            state.answer = None;
            state.asking = Some(String::from(prompt));
        }
        self.0.window.ring();
        Ok(())
    }

    /// The line the user has typed since [`ShellEnd::ask`], if they have; [`Gone`] if the
    /// window closed first. Doesn't wait.
    pub fn try_line(&self) -> Result<Option<String>, Gone> {
        let mut state = self.0.state.lock();
        if let Some(line) = state.answer.take() {
            return Ok(Some(line));
        }
        if state.closed { Err(Gone) } else { Ok(None) }
    }

    /// Adds `output` to the queue and rings the window.
    fn push(&self, output: Output) -> Result<(), Gone> {
        {
            let mut state = self.0.state.lock();
            if state.closed {
                return Err(Gone);
            }
            // Text that comes in pieces (the shell cuts its output to 4096 bytes at a time) is
            // joined, so the window shows it in one go.
            if let (Output::Text(more), Some(Output::Text(last))) =
                (&output, state.output.last_mut())
            {
                last.push_str(more);
            } else {
                state.output.push(output);
            }
        }
        self.0.window.ring();
        Ok(())
    }
}

impl Console for ShellEnd {
    fn read_line(&self, prompt: &str) -> Result<String, Gone> {
        self.ask(prompt)?;
        loop {
            // Look first, then wait: an answer that came after the look rang the bell, and the
            // bell stays rung (see the module's note), so the wait ends at once.
            if let Some(line) = self.try_line()? {
                return Ok(line);
            }
            self.0.shell.park().map_err(|Broken| Gone)?;
        }
    }

    fn write(&self, text: &str) -> Result<(), Gone> {
        self.push(Output::Text(String::from(text)))
    }

    fn clear(&self) -> Result<(), Gone> {
        self.push(Output::Clear)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    /// A bell for tests: a flag and a condition variable, as `Notification` is a set of bits
    /// and a wait. `rings` counts every ring, for tests that ask how often the window was
    /// woken.
    #[derive(Default)]
    pub(crate) struct Bell {
        rung: Mutex<bool>,
        woken: Condvar,
        pub(crate) rings: AtomicUsize,
    }

    impl Ring for std::sync::Arc<Bell> {
        fn ring(&self) {
            self.rings.fetch_add(1, Ordering::SeqCst);
            *self.rung.lock().unwrap() = true;
            self.woken.notify_all();
        }
    }

    impl Park for std::sync::Arc<Bell> {
        fn park(&self) -> Result<(), Broken> {
            let mut rung = self.rung.lock().unwrap();
            // A wait that nothing ever ends must fail the test, not hang it.
            let deadline = Instant::now() + Duration::from_secs(20);
            while !*rung {
                let left = deadline.checked_duration_since(Instant::now()).ok_or(Broken)?;
                rung = self.woken.wait_timeout(rung, left).unwrap().0;
            }
            *rung = false;
            Ok(())
        }
    }

    /// A queue and the bells behind it: (window's end, shell's end, window's bell, shell's
    /// bell).
    pub(crate) fn pair() -> (WindowEnd, ShellEnd, std::sync::Arc<Bell>, std::sync::Arc<Bell>) {
        let (window_bell, shell_bell) =
            (std::sync::Arc::new(Bell::default()), std::sync::Arc::new(Bell::default()));
        let (w, s) = link(window_bell.clone(), shell_bell.clone());
        (w, s, window_bell, shell_bell)
    }

    #[test]
    fn output_is_queued_in_order_and_rings_the_window() {
        let (window, shell, window_bell, _) = pair();
        assert_eq!(window.take_work(), Work::default());
        shell.write("one ").unwrap();
        shell.write("two\n").unwrap();
        shell.clear().unwrap();
        shell.write("three\n").unwrap();
        assert_eq!(window_bell.rings.load(Ordering::SeqCst), 4);
        // Text that came in pieces is one piece; a clear separates what is before it and after.
        assert_eq!(
            window.take_work(),
            Work {
                output: vec![
                    Output::Text("one two\n".into()),
                    Output::Clear,
                    Output::Text("three\n".into())
                ],
                ask: None
            }
        );
        assert_eq!(window.take_work(), Work::default(), "taking forgets");
    }

    #[test]
    fn a_line_is_asked_for_and_answered() {
        let (window, shell, window_bell, _) = pair();
        shell.write("hello\n").unwrap();
        shell.ask("> ").unwrap();
        assert_eq!(shell.try_line(), Ok(None), "nothing typed yet");
        let work = window.take_work();
        assert_eq!(work.output, [Output::Text("hello\n".into())]);
        assert_eq!(work.ask.as_deref(), Some("> "));
        assert_eq!(window_bell.rings.load(Ordering::SeqCst), 2);
        window.answer("echo hi".into());
        assert_eq!(shell.try_line(), Ok(Some("echo hi".into())));
        assert_eq!(shell.try_line(), Ok(None), "an answer is taken once");
    }

    #[test]
    fn an_old_answer_does_not_answer_a_new_question() {
        let (window, shell, ..) = pair();
        window.answer("stale".into());
        shell.ask("> ").unwrap();
        assert_eq!(shell.try_line(), Ok(None));
    }

    #[test]
    fn closing_ends_everything_the_shell_tries() {
        let (window, shell, ..) = pair();
        shell.write("before\n").unwrap();
        window.close();
        assert_eq!(shell.write("x"), Err(Gone));
        assert_eq!(shell.clear(), Err(Gone));
        assert_eq!(shell.ask("> "), Err(Gone));
        assert_eq!(shell.try_line(), Err(Gone));
        assert_eq!(shell.read_line("> "), Err(Gone));
        // What was queued before is still there for the window.
        assert_eq!(window.take_work().output, [Output::Text("before\n".into())]);
    }

    /// The shell's thread waits in `read_line` until the window answers.
    #[test]
    fn read_line_waits_for_the_answer() {
        let (window, shell, window_bell, _) = pair();
        let reader = thread::spawn(move || shell.read_line("> "));
        // The window learns of the question (its bell rings), and answers.
        let start = Instant::now();
        while window_bell.rings.load(Ordering::SeqCst) == 0 {
            assert!(start.elapsed() < Duration::from_secs(20), "the shell never asked");
            thread::yield_now();
        }
        assert_eq!(window.take_work().ask.as_deref(), Some("> "));
        window.answer("ls".into());
        assert_eq!(reader.join().unwrap(), Ok("ls".to_string()));
    }

    /// The answer comes before the shell has even begun to wait: the bell stays rung, so the
    /// wait ends at once (no lost wake-up).
    #[test]
    fn an_answer_that_comes_early_is_not_lost() {
        let (window, shell, ..) = pair();
        shell.ask("> ").unwrap();
        window.answer("early".into());
        // `read_line` asks again, which forgets the answer: use the halves the way
        // `read_line` does, looking and then waiting.
        assert_eq!(shell.try_line(), Ok(Some("early".into())));
        // And the ring the answer made is still there for the next wait, which ends at once
        // (the loop in `read_line` then looks again and finds nothing: harmless).
        assert_eq!(shell.0.shell.park(), Ok(()));
    }

    /// Closing the window wakes a shell that is waiting for a line, and it is told it is gone.
    #[test]
    fn closing_wakes_a_waiting_shell() {
        let (window, shell, window_bell, _) = pair();
        let reader = thread::spawn(move || shell.read_line("> "));
        let start = Instant::now();
        while window_bell.rings.load(Ordering::SeqCst) == 0 {
            assert!(start.elapsed() < Duration::from_secs(20), "the shell never asked");
            thread::yield_now();
        }
        window.close();
        assert_eq!(reader.join().unwrap(), Err(Gone));
    }

    /// A wait that can't work is the terminal gone, not a loop.
    #[test]
    fn a_broken_wait_ends_the_shell() {
        struct Broke;
        impl Ring for Broke {
            fn ring(&self) {}
        }
        impl Park for Broke {
            fn park(&self) -> Result<(), Broken> {
                Err(Broken)
            }
        }
        let (_window, shell) = link(Broke, Broke);
        assert_eq!(shell.read_line("> "), Err(Gone));
    }

    /// Many lines, answered one by one from the window's side as fast as they are asked, with
    /// the shell writing in between: nothing is lost or repeated, and nothing waits for ever.
    #[test]
    fn a_long_conversation_loses_nothing() {
        let (window, shell, window_bell, _) = pair();
        let shell_thread = thread::spawn(move || {
            let mut got = Vec::new();
            for i in 0..300 {
                shell.write(&format!("out {i}\n")).unwrap();
                got.push(shell.read_line("> ").unwrap());
            }
            got
        });
        let mut seen = Vec::new();
        let mut answered = 0;
        let start = Instant::now();
        while answered < 300 {
            assert!(start.elapsed() < Duration::from_secs(30), "stuck after {answered} lines");
            let work = window.take_work();
            for o in work.output {
                if let Output::Text(t) = o {
                    seen.push(t);
                }
            }
            if work.ask.is_some() {
                window.answer(format!("line {answered}"));
                answered += 1;
            } else {
                thread::yield_now();
            }
        }
        let got = shell_thread.join().unwrap();
        assert_eq!(got, (0..300).map(|i| format!("line {i}")).collect::<Vec<_>>());
        // The window saw every piece of output, once, in order (pieces may be joined).
        let last = window.take_work();
        let text: String = seen.concat()
            + &last
                .output
                .iter()
                .filter_map(|o| match o {
                    Output::Text(t) => Some(t.as_str()),
                    Output::Clear => None,
                })
                .collect::<String>();
        let want: String = (0..300).map(|i| format!("out {i}\n")).collect();
        assert_eq!(text, want);
        assert!(window_bell.rings.load(Ordering::SeqCst) >= 300);
    }
}
