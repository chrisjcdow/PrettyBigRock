// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `echo`: a small service for the program self-test (docs/design/0009-programs-and-granite.md,
//! section 9). It answers calls on the endpoint it's given (role 1000), with as many threads
//! as its arguments say (`threads=N`), all receiving on the same endpoint.
//!
//! It speaks two protocols on the endpoint: the raw messages of `pbr-test-protocol` (labels
//! with version 0), and the `pbr.test.echo` interface (version 1) through its generated
//! dispatcher.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use granite::endpoint::{Endpoint, Event};
use granite::memory::Memory;
use granite::message::{CapSend, Outgoing};
use granite::notification::Notification;
use granite::{thread, time};
use pbr_interfaces::pbr_test_echo::{self as echo, EchoError, Flags, Item, Pair, Shape};
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
        if msg.label >> 16 != 0 {
            if let Some((_, reply)) = echo::dispatch(&mut Service, msg, &buffer, reply) {
                granite::interface::unknown(reply);
            }
            continue;
        }
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

/// The `pbr.test.echo` interface.
struct Service;

/// Reports a failed answer (the caller may have gone, which is no fault of ours).
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("echo: answer failed: {e}");
    }
}

impl echo::Server for Service {
    fn reverse(&mut self, text: String, reply: echo::reverse::Reply) {
        let back: String = text.chars().rev().collect();
        answered(reply.ok(&back));
    }

    fn add(&mut self, a: u32, b: u32, reply: echo::add::Reply) {
        answered(match a.checked_add(b) {
            Some(sum) => reply.ok(sum),
            None => reply.err(EchoError::Overflow),
        });
    }

    fn swap(&mut self, pair: Pair, flags: Flags, reply: echo::swap::Reply) {
        let shape = if pair.right.is_some() { Shape::Whole } else { Shape::LeftOnly };
        let out = Pair {
            left: pair.right.as_ref().map_or(-1, |r| r.len() as i16),
            right: Some(pair.left.to_string()),
        };
        answered(reply.ok(&out, flags, shape));
    }

    fn pass(
        &mut self,
        bits: u64,
        note: Notification,
        memory: Option<Memory>,
        reply: echo::pass::Reply,
    ) {
        if note.signal(bits).is_err() {
            answered(reply.fail());
            return;
        }
        let size = memory.and_then(|m| m.size().ok());
        answered(reply.ok(note, size));
    }

    fn total(&mut self, values: Vec<i64>, items: Vec<Item>, min: u16, reply: echo::total::Reply) {
        let sum = values.iter().fold(0i64, |a, v| a.wrapping_add(*v));
        let long: Vec<String> =
            items.into_iter().filter(|i| i.name.len() > usize::from(min)).map(|i| i.name).collect();
        let base = values.len() as u8;
        let grid = [[base, base + 1, base + 2], [base + 3, base + 4, base + 5]];
        answered(reply.ok(sum, &long, &grid));
    }

    fn fail(&mut self, reply: echo::fail::Reply) {
        answered(reply.fail());
    }
}
