// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `echo`: a small service for the program self-test (docs/design/0009-programs-and-granite.md,
//! section 9). It answers calls on the endpoint it's given (role 1000), with as many threads
//! as its arguments say (`threads=N`), all receiving on the same endpoint.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use granite::endpoint::{Endpoint, Event};
use granite::message::{CapSend, Outgoing};
use granite::{thread, time};
use pbr_test_protocol::{op, role};

granite::main!(main);

static CALLS: AtomicU64 = AtomicU64::new(0);

fn main(mut start: granite::Start) -> i32 {
    let Some(ep) = start.take(role::ENDPOINT) else {
        granite::println!("echo: no endpoint to serve");
        return 1;
    };
    let threads: usize = start
        .args()
        .split_whitespace()
        .find_map(|w| w.strip_prefix("threads=")?.parse().ok())
        .unwrap_or(1)
        .clamp(1, 16);
    let raw = ep.into_raw();
    let mut helpers = Vec::new();
    for _ in 1..threads {
        match thread::spawn(move || serve(raw)) {
            Ok(h) => helpers.push(h),
            Err(e) => {
                granite::println!("echo: can't start a thread: {e}");
                return 2;
            }
        }
    }
    serve(raw);
    0
}

/// Answers calls for ever.
fn serve(ep: u32) {
    let ep = Endpoint(granite::Handle::from_raw(ep));
    let mut buffer = vec![0u8; 65536];
    loop {
        let (mut msg, reply) = match ep.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(m, r)) => (m, r),
            Ok(Event::Signal(_)) => continue,
            Err(e) => {
                granite::println!("echo: receive failed: {e}");
                time::sleep_ms(10);
                continue;
            }
        };
        CALLS.fetch_add(1, Ordering::SeqCst);
        let n = msg.buffer_len.min(buffer.len());
        let result = match msg.label {
            op::ECHO => {
                buffer[..n].reverse();
                let mut caps = [CapSend { handle: 0, mask: 0, moving: true }; 4];
                let count = msg.cap_count();
                for (i, c) in caps.iter_mut().enumerate().take(count) {
                    if let Some(h) = msg.take_cap(i) {
                        *c = CapSend::moved(h, u32::MAX);
                    }
                }
                let data: Vec<u8> = msg.data().to_vec();
                reply.reply(&Outgoing { label: 0, data: &data, caps: &caps[..count] }, &buffer[..n])
            }
            op::BADGE => reply.reply(&Outgoing::new(0, &msg.badge.to_le_bytes()), &[]),
            op::COUNT => {
                let c = CALLS.load(Ordering::SeqCst);
                reply.reply(&Outgoing::new(0, &c.to_le_bytes()), &[])
            }
            op::RIGHTS => {
                let mut out = Vec::new();
                for i in 0..msg.cap_count() {
                    out.extend_from_slice(&msg.cap_rights(i).to_le_bytes());
                }
                reply.reply(&Outgoing::new(0, &out), &[])
            }
            op::CRASH => {
                // A deliberate fault: this page is never mapped.
                // SAFETY: none; it's meant to fault, which ends this process.
                unsafe { core::ptr::write_volatile(0x20 as *mut u64, 1) };
                Ok(())
            }
            _ => reply.reply(&Outgoing::new(1, &[]), &[]),
        };
        if let Err(e) = result {
            granite::println!("echo: reply failed: {e}");
        }
    }
}
