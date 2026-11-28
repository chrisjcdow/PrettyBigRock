// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The power service (docs/design/0024-the-session-desktop.md, section 7): it turns the
//! computer off and restarts it, for the users service alone, which has saved everything first;
//! and it tells the users service when the power button is pressed
//! (docs/design/0028-acpi.md, section 7).
//!
//! The root program starts it with the machine capability ([`role::POWER`]; note 0019), which
//! lets the kernel restart the computer on both architectures, and turn it off on ARM64
//! (through the firmware, PSCI). A PC is turned off through ACPI: the firmware's `\_S5` says
//! what to write to the power-management registers, and only the ACPI service can run the
//! firmware's AML to find out, so on x86-64 this service asks it (`pbr.acpi.off`). The ACPI
//! service also watches the power button, and signals this service, which passes it on.
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
use granite::interface::CallError;
use granite::machine::{Machine, Power};
use granite::notification::Notification;
use granite::process::role;
use granite::sys::rights;
use granite::time;
use pbr_interfaces::pbr_acpi::{self as acpi_if, AcpiError};
use pbr_interfaces::pbr_power::{self as power_if, PowerError};
use pbr_interfaces::pbr_supervisor as supervisor;

granite::main!(main);

/// The badge of the users service's capability (the root program's `POWER_USERS`).
const USERS: u64 = 1;
/// The bit the ACPI service signals on this service's notification for the power button.
const BUTTON: u64 = 1;

struct Service {
    machine: Option<Machine>,
    /// The ACPI service, if it could be found.
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(dead_code, reason = "ARM64 is turned off through the kernel, not ACPI")
    )]
    acpi: Option<acpi_if::Client>,
    /// Whether the ACPI service watches the power button for this service.
    watching: bool,
    /// Whom to tell when the power button is pressed: the users service's notification.
    button: Option<(Notification, u64)>,
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
    let pressed = Notification::create().map_err(|e| format!("a notification: {e}"))?;
    endpoint.bind(&pressed).map_err(|e| format!("binding the endpoint: {e}"))?;
    let (acpi, watching) = match start.take(role::PARENT) {
        Some(h) => find_acpi(Endpoint(h), &pressed),
        None => (None, false),
    };
    let mut s = Service { machine, acpi, watching, button: None };
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
            Ok(Event::Signal(bits)) if bits & BUTTON != 0 => {
                granite::println!("power: the power button was pressed.");
                if let Some((n, b)) = &s.button {
                    let _ = n.signal(*b);
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

/// The ACPI service, through the supervisor (`None` if it isn't there); and it is asked to
/// signal `pressed` when the power button is pressed (true if it will).
fn find_acpi(parent: Endpoint, pressed: &Notification) -> (Option<acpi_if::Client>, bool) {
    let mut p = supervisor::Client::new(parent);
    p.set_timeout_ms(10_000);
    let ep = match p.lookup("acpi") {
        Ok(h) => Endpoint(h),
        Err(e) => {
            granite::println!("power: no ACPI service ({e}).");
            return (None, false);
        }
    };
    let mut acpi = acpi_if::Client::new(ep);
    // Turning off waits 3 seconds to see whether it worked.
    acpi.set_timeout_ms(10_000);
    let watching = match pressed.0.duplicate(rights::SIGNAL | rights::TRANSFER) {
        Ok(h) => match acpi.watch_button(Notification(h), BUTTON) {
            Ok(()) => true,
            Err(CallError::Service(AcpiError::NotSupported)) => false,
            Err(e) => {
                granite::println!("power: watching the power button: {e}");
                false
            }
        },
        Err(e) => {
            granite::println!("power: can't copy its notification: {e}");
            false
        }
    };
    (Some(acpi), watching)
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
        fn watch_button(
            &mut self,
            _note: Notification,
            _bits: u64,
            reply: power_if::watch_button::Reply,
        ) {
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

impl power_if::Server for Service {
    fn off(&mut self, reply: power_if::off::Reply) {
        granite::println!("power: turning the computer off.");
        #[cfg(target_arch = "x86_64")]
        {
            // The ACPI service turns it off; it answers only if it couldn't.
            let Some(acpi) = &self.acpi else {
                return answered(reply.err(PowerError::NotSupported));
            };
            match acpi.off() {
                Ok(()) => {}
                Err(e) => granite::println!("power: the ACPI service couldn't turn it off: {e}"),
            }
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

    fn watch_button(
        &mut self,
        note: Notification,
        bits: u64,
        reply: power_if::watch_button::Reply,
    ) {
        // The button is only seen through the ACPI service, on a PC whose button is a fixed one.
        if !self.watching {
            return answered(reply.err(PowerError::NotSupported));
        }
        self.button = Some((note, bits));
        answered(reply.ok());
    }
}
