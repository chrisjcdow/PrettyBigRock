// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Turning the machine off and restarting it: `machine_power` (docs/design/0019-kernel-1.3.md,
//! section 1).
//!
//! The kernel saves nothing: whoever calls this must have saved everything first. Every
//! refusal is decided before anything is stopped; once the other cores are stopped there is no
//! way back, and if the machine then won't go down the kernel stops with a message, as it
//! does for a bug of its own.

use crate::obj::Error;
use crate::{arch, kprintln, percpu, time};
use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// What `machine_power` can be asked to do. The numbers are part of the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Turn the machine off.
    Off = 1,
    /// Restart it.
    Restart = 2,
}

/// How long to wait for the other cores to stop, and for each way of restarting to work.
pub const PATIENCE_NS: u64 = 100_000_000;

/// Set by the first thread to get as far as stopping the machine. An atomic is a value that
/// several cores can read and change safely without a lock.
static GOING_DOWN: AtomicBool = AtomicBool::new(false);
/// How many cores have stopped because they were told to.
static STOPPED: AtomicUsize = AtomicUsize::new(0);

/// Called by a core that has been told to stop (the "stop" interrupt from another core),
/// just before it halts for good.
pub fn core_stopped() {
    STOPPED.fetch_add(1, Ordering::SeqCst);
}

/// Waits (spinning: interrupts are off and nothing else runs) until `done` says so or
/// [`PATIENCE_NS`] has passed.
pub fn wait_until(done: impl Fn() -> bool) {
    let until = time::now_ns().saturating_add(PATIENCE_NS);
    while !done() && time::now_ns() < until {
        core::hint::spin_loop();
    }
}

/// Turns the machine off or restarts it. Comes back only with an error; `Infallible` is a
/// type that has no values, so `Result<Infallible, Error>` can only ever be an `Err`.
pub fn power(action: u64) -> Result<Infallible, Error> {
    let action = match action {
        1 => Action::Off,
        2 => Action::Restart,
        _ => return Err(Error::InvalidArgument),
    };
    // Whether this machine can do it at all, found out before anything is stopped.
    let plan = arch::power_prepare(action).ok_or(Error::NotSupported)?;
    kprintln!(
        "{}",
        match action {
            Action::Off => "Turning off",
            Action::Restart => "Restarting",
        }
    );
    arch::irq_disable();
    if GOING_DOWN.swap(true, Ordering::SeqCst) {
        // Another core got here first and is about to stop this one; its "stop" interrupt
        // can't arrive now that interrupts are off, so do what it would have done.
        core_stopped();
        arch::halt_forever();
    }
    // Stop the other cores, so nothing is running when the power goes or the reset comes.
    let me = percpu::this().index;
    let mut others = 0;
    for c in percpu::all() {
        if c.index != me && c.online.load(Ordering::Relaxed) {
            arch::send_ipi(c, arch::Ipi::Stop);
            others += 1;
        }
    }
    wait_until(|| STOPPED.load(Ordering::SeqCst) >= others);
    arch::power_now(plan)
}
