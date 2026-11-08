// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The driver's event loop, and telling the device manager the driver is ready.

use granite::endpoint::{Endpoint, Event, ReplyCap};
use granite::interface::CallError;
use granite::message::Incoming;
use granite::sys::rights;
use granite::{process, time};
use pbr_interfaces::pbr_driver_host::{self as host, HostError};

/// A driver's service.
pub trait Service {
    /// A request arrived. Answer it through `reply` (now or later); dropping `reply` tells the
    /// caller the service has gone.
    fn call(&mut self, msg: Incoming, buffer: &[u8], reply: ReplyCap);

    /// Interrupts fired: bit *i* is interrupt *i* (see `Device::bind_interrupts`).
    fn signal(&mut self, bits: u64);
}

/// How many receive failures in a row end the driver (it is restarted by the device manager).
const MAX_FAILURES: u32 = 50;

/// Serves requests and interrupts on `ep` for ever. `buffer_size` is the largest attached
/// buffer a request can have.
pub fn serve(ep: &Endpoint, buffer_size: usize, service: &mut impl Service) -> ! {
    let mut buffer = alloc::vec![0u8; buffer_size];
    let mut failures = 0;
    loop {
        match ep.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(msg, reply)) => {
                failures = 0;
                service.call(msg, &buffer, reply);
            }
            Ok(Event::Signal(bits)) => {
                failures = 0;
                service.signal(bits);
            }
            // Drivers don't mint watched badges, so this never comes.
            Ok(Event::Gone(_)) => failures = 0,
            Err(e) => {
                failures += 1;
                if failures >= MAX_FAILURES {
                    granite::println!("driver: receive keeps failing ({e}); ending");
                    process::exit(2);
                }
                time::sleep_ms(10);
            }
        }
    }
}

/// The badge on clients' capabilities for a driver's service. Drivers don't tell their clients
/// apart, but it isn't 0: a copyable capability without a badge would let whoever holds it mint
/// copies with any badge it likes (note 0006, section 5).
pub const CLIENT_BADGE: u64 = 1;

/// Tells the device manager (`parent`) that the driver is serving, and gives it an endpoint
/// clients may send to: a copy of `service` that can be sent, copied and passed on, but not
/// received from, carrying [`CLIENT_BADGE`].
pub fn host_ready(parent: Endpoint, service: &Endpoint) -> Result<(), CallError<HostError>> {
    let send = service
        .mint(CLIENT_BADGE, rights::SEND | rights::TRANSFER | rights::DUPLICATE)
        .map_err(CallError::Call)?;
    let mut client = host::Client::new(parent);
    client.set_deadline(time::after_ms(10_000));
    client.ready(send.0)
}
