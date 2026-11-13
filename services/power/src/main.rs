// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The power service (docs/design/0024-the-session-desktop.md, section 7): it turns the
//! computer off and restarts it, for the users service alone, which has saved everything first.
//!
//! The root program starts it with:
//! - the machine capability ([`role::POWER`]; note 0019), which lets the kernel restart the
//!   computer on both architectures, and turn it off on ARM64 (through the firmware, PSCI);
//! - on x86-64 only: the two I/O ports where the sleep type is written to turn a PC off (the
//!   FADT's PM1a and PM1b control registers), and the ACPI tables that say which sleep type that
//!   is (the DSDT and SSDTs), read only. An I/O port is a numbered register a device answers
//!   on, separate from memory; these slices are just the few it needs.
//!
//! Turning a PC off properly needs an ACPI interpreter (M7). Until then the sleep type is found
//! by scanning the tables' bytes, as 1.3 did: `pbr_platform::s5`, **a stopgap** in a file of
//! its own, to be removed whole.
//!
//! It serves `pbr.power` on one endpoint; only calls with the users service's badge (the root
//! program's lookup table gives a capability for it to nobody else) are answered.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use granite::endpoint::{Endpoint, Event};
use granite::machine::{Machine, Power};
use granite::process::role;
use granite::time;
use pbr_interfaces::pbr_power::{self as power_if, PowerError};

granite::main!(main);

/// The badge of the users service's capability (the root program's `POWER_USERS`).
const USERS: u64 = 1;

/// Where the root program gives the x86 parts (`services/root` uses the same numbers): the
/// PM1a and PM1b control ports, and the ACPI tables from this role on (their offsets and
/// lengths within each slice are in the arguments, `table=ROLE,OFFSET,LENGTH`).
#[cfg(target_arch = "x86_64")]
mod x86 {
    pub const PM1A: u32 = 1001;
    pub const PM1B: u32 = 1002;

    /// The sleep-enable bit of a PM1 control register, and where the sleep type goes.
    pub const SLEEP_ENABLE: u64 = 1 << 13;
    pub const SLEEP_TYPE_SHIFT: u32 = 10;
}

struct Service {
    machine: Option<Machine>,
    /// x86-64: the PM1a and PM1b control ports, and the sleep types for them, if `_S5_` was
    /// found.
    #[cfg(target_arch = "x86_64")]
    off: Option<(granite::device::IoPorts, Option<granite::device::IoPorts>, (u16, u16))>,
}

fn main(mut start: granite::Start) -> i32 {
    match run(&mut start) {
        Ok(()) => 0,
        Err(e) => {
            granite::println!("power: {e}");
            1
        }
    }
}

fn run(start: &mut granite::Start) -> Result<(), String> {
    let endpoint = Endpoint(start.take(role::OWN).ok_or("started without an endpoint to serve")?);
    let machine = start.take(role::POWER).map(Machine);
    if machine.is_none() {
        granite::println!("power: started without the machine capability.");
    }
    let mut s = Service {
        machine,
        #[cfg(target_arch = "x86_64")]
        off: find_off(start),
    };
    let mut buffer = vec![0u8; 256];
    loop {
        match endpoint.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(msg, reply)) => {
                let unknown = if msg.badge == USERS {
                    power_if::dispatch(&mut s, msg, &buffer, reply)
                } else {
                    // Nobody else should hold a capability for this endpoint; if someone
                    // does, they are told no in so many words.
                    Some((msg, reply))
                };
                if let Some((msg, reply)) = unknown {
                    refuse(msg, reply);
                }
            }
            Ok(_) => {}
            Err(e) => {
                granite::println!("power: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
    }
}

/// Answers a call that may not be made: `NotAllowed` for `pbr.power`'s own operations, and
/// "unknown" for anything else.
fn refuse(msg: granite::message::Incoming, reply: granite::endpoint::ReplyCap) {
    struct No;
    impl power_if::Server for No {
        fn off(&mut self, reply: power_if::off::Reply) {
            answered(reply.err(PowerError::NotAllowed));
        }
        fn restart(&mut self, reply: power_if::restart::Reply) {
            answered(reply.err(PowerError::NotAllowed));
        }
    }
    if let Some((_, reply)) = power_if::dispatch(&mut No, msg, &[], reply) {
        granite::interface::unknown(reply);
    }
}

fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("power: answer failed: {e}");
    }
}

/// x86-64: the PM1 ports and the sleep types to write to them, from what the root program gave.
#[cfg(target_arch = "x86_64")]
fn find_off(
    start: &mut granite::Start,
) -> Option<(granite::device::IoPorts, Option<granite::device::IoPorts>, (u16, u16))> {
    use granite::device::{DeviceMemory, IoPorts};
    use granite::sys::perm;
    let pm1a = IoPorts(start.take(x86::PM1A)?);
    let pm1b = start.take(x86::PM1B).map(IoPorts);
    let args = String::from(start.args());
    for word in args.split_whitespace() {
        let Some(t) = word.strip_prefix("table=") else { continue };
        let mut n = t.split(',').map(|v| v.parse::<u64>().ok());
        let (Some(Some(r)), Some(Some(offset)), Some(Some(len))) = (n.next(), n.next(), n.next())
        else {
            continue;
        };
        let Some(h) = u32::try_from(r).ok().and_then(|r| start.take(r)) else { continue };
        let memory = DeviceMemory(h);
        let Some(end) = offset.checked_add(len) else { continue };
        let Ok(mapping) = memory.map(0, end.next_multiple_of(4096), perm::READ) else {
            granite::println!("power: can't map an ACPI table.");
            continue;
        };
        // SAFETY: the mapping is readable for `end` bytes (rounded up to a page), nothing writes
        // to it here, and the slice is used only while `mapping` is alive (this block).
        let bytes = unsafe {
            core::slice::from_raw_parts(mapping.as_ptr().add(offset as usize), len as usize)
        };
        if let Some(types) = pbr_platform::s5::sleep_types(bytes) {
            granite::println!("power: sleep type for off: {}, {}.", types.0, types.1);
            return Some((pm1a, pm1b, types));
        }
    }
    granite::println!("power: no _S5_ in the ACPI tables: this computer can't be turned off.");
    None
}

impl power_if::Server for Service {
    fn off(&mut self, reply: power_if::off::Reply) {
        granite::println!("power: turning the computer off.");
        #[cfg(target_arch = "x86_64")]
        {
            // Writing the sleep type with the sleep-enable bit turns the computer off at once
            // (1.3's `acpi_poweroff`). If it is still running a moment later, it can't.
            let Some((pm1a, pm1b, (a, b))) = &self.off else {
                return answered(reply.err(PowerError::NotSupported));
            };
            let value = |t: u16| (u64::from(t) << x86::SLEEP_TYPE_SHIFT) | x86::SLEEP_ENABLE;
            let _ = pm1a.write(0, 2, value(*a));
            if let Some(p) = pm1b {
                let _ = p.write(0, 2, value(*b));
            }
            time::sleep_ms(1_000);
            answered(reply.err(PowerError::NotSupported));
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let e = match &self.machine {
                Some(m) => m.power(Power::Off),
                None => granite::Error::NoRight,
            };
            granite::println!("power: the kernel couldn't turn it off: {e}");
            answered(reply.err(PowerError::NotSupported));
        }
    }

    fn restart(&mut self, reply: power_if::restart::Reply) {
        granite::println!("power: restarting the computer.");
        let e = match &self.machine {
            Some(m) => m.power(Power::Restart),
            None => granite::Error::NoRight,
        };
        granite::println!("power: the kernel couldn't restart it: {}", format!("{e}"));
        answered(reply.err(PowerError::NotSupported));
    }
}
