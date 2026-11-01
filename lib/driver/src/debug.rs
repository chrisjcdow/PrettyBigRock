// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Crashing on purpose, for the tests (docs/design/0012, section 6). A driver started with
//! `selftest` among its arguments also answers `pbr.driver.debug`.

use granite::endpoint::ReplyCap;
use granite::message::Incoming;
use pbr_interfaces::pbr_driver_debug::{self as debug, CrashKind};

/// True if the arguments ask for the debug interface.
pub fn enabled(args: &str) -> bool {
    args.split_whitespace().any(|w| w == "selftest")
}

/// True if `msg` is for `pbr.driver.debug`. (Interfaces share one endpoint by having different
/// operation numbers; the class interface would answer an unknown one as unknown, so the
/// caller asks first.)
pub fn is_debug(msg: &Incoming) -> bool {
    msg.label >> 16 == u32::from(debug::VERSION)
        && (msg.label & 0xFFFF) == u32::from(debug::crash::OP)
}

/// Handles a request for `pbr.driver.debug`.
pub fn dispatch(msg: Incoming, buffer: &[u8], reply: ReplyCap) {
    if let Some((_, reply)) = debug::dispatch(&mut Crasher, msg, buffer, reply) {
        granite::interface::unknown(reply);
    }
}

struct Crasher;

impl debug::Server for Crasher {
    fn crash(&mut self, kind: CrashKind, _reply: debug::crash::Reply) {
        granite::println!("driver: crashing on purpose ({kind:?})");
        match kind {
            CrashKind::NullWrite => {
                // SAFETY: none: this is the point. The write faults and the process ends.
                unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u8>(), 1) };
            }
            CrashKind::IllegalInstruction => undefined_instruction(),
            CrashKind::DivideByZero => divide_by_zero(),
            _ => granite::process::exit(7),
        }
        // Still here (a fault that was not fatal?): end anyway.
        granite::process::exit(8)
    }
}

#[cfg(target_arch = "x86_64")]
fn undefined_instruction() {
    // SAFETY: `ud2` is defined to raise an invalid-opcode fault, which ends the process.
    unsafe { core::arch::asm!("ud2") };
}

#[cfg(target_arch = "aarch64")]
fn undefined_instruction() {
    // SAFETY: `udf` is permanently undefined, and raises an exception that ends the process.
    unsafe { core::arch::asm!("udf #0") };
}

#[cfg(target_arch = "x86_64")]
fn divide_by_zero() {
    let zero = core::hint::black_box(0u64);
    // SAFETY: dividing by zero raises a divide error, which ends the process.
    unsafe {
        core::arch::asm!("div {z}", z = in(reg) zero, inout("rax") 1u64 => _, inout("rdx") 0u64 => _)
    };
}

#[cfg(target_arch = "aarch64")]
fn divide_by_zero() {
    // Integer division by zero doesn't fault on ARM (it gives 0), so the nearest thing is a
    // breakpoint instruction.
    // SAFETY: `brk` raises a debug exception, which ends the process.
    unsafe { core::arch::asm!("brk #0") };
}
