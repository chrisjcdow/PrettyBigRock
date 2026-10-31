// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `probe`: a test program for the program self-test (docs/design/0009-programs-and-granite.md,
//! section 9). Its first argument says what to try; it exits with 0 if everything behaved as
//! it should, or the number of the first check that didn't. Some modes are meant to end in a
//! fault, which the root program checks for.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::memory::Memory;
use granite::message::{CapSend, Outgoing};
use granite::notification::Notification;
use granite::sys::{self, Error, num, perm, rights};
use granite::{Handle, process, thread, time};
use pbr_test_protocol::{PASSED, op, role};

granite::main!(main);

fn main(mut start: granite::Start) -> i32 {
    let args: Vec<alloc::string::String> =
        start.args().split_whitespace().map(alloc::string::String::from).collect();
    let mode = args.first().map(|s| s.as_str()).unwrap_or("");
    let num = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    let r = match mode {
        "messages" => messages(&mut start, num(1), num(2)),
        "isolation" => isolation(&mut start),
        "fault-readonly" => fault_readonly(),
        "fault-kernel" => fault_kernel(),
        "fault-instruction" => fault_instruction(),
        "fault-null" => fault_null(),
        "revoke-endpoint" => revoke_endpoint(&mut start),
        "revoke-memory" => revoke_memory(&mut start),
        "sender" => sender(&mut start),
        "crash-service" => crash_service(&mut start),
        "budget" => budget(num(1)),
        "load" => load(&mut start, num(1), num(2)),
        _ => Err(99),
    };
    match r {
        Ok(()) => PASSED,
        Err(n) => {
            granite::println!("probe {mode}: check {n} failed");
            n
        }
    }
}

/// Fails with check number `n` unless `ok`.
fn check(n: i32, ok: bool) -> Result<(), i32> {
    if ok { Ok(()) } else { Err(n) }
}

fn endpoint(start: &mut granite::Start, r: u32) -> Result<Endpoint, i32> {
    start.take(r).map(Endpoint).ok_or(98)
}

fn call(ep: &Endpoint, label: u32, data: &[u8]) -> Result<granite::message::Incoming, Error> {
    ep.call(&Outgoing::new(label, data), &mut [], 0, time::after_ms(10_000))
}

// ------------------------------------------------------------------------------------------
// Messages and capabilities

fn messages(start: &mut granite::Start, badge1: u64, badge2: u64) -> Result<(), i32> {
    let ep = endpoint(start, role::ENDPOINT)?;
    let ep2 = endpoint(start, role::ENDPOINT2)?;
    // 1: an empty message.
    let r = call(&ep, op::ECHO, &[]).map_err(|_| 1)?;
    check(1, r.label == 0 && r.data().is_empty())?;
    // 2: a full 240 bytes of data.
    let data: Vec<u8> = (0..240u32).map(|i| (i * 7 + 3) as u8).collect();
    let r = call(&ep, op::ECHO, &data).map_err(|_| 2)?;
    check(2, r.data() == &data[..])?;
    // 3: the largest attached buffer, which comes back reversed.
    let mut buf: Vec<u8> = (0..65536u32).map(|i| (i ^ (i >> 8)) as u8).collect();
    let expect: Vec<u8> = buf.iter().rev().copied().collect();
    let r = ep
        .call(&Outgoing::new(op::ECHO, &[]), &mut buf, 65536, time::after_ms(10_000))
        .map_err(|_| 3)?;
    check(3, r.buffer_len == 65536 && buf == expect)?;
    // 4: one byte more than a buffer can be is refused before it's sent.
    let mut big = vec![0u8; 65537];
    check(
        4,
        ep.call(&Outgoing::new(op::ECHO, &[]), &mut big, 65537, time::NOW).err()
            == Some(Error::TooBig),
    )?;
    // 5: a copied capability: the service sends it back, and both are the same notification.
    let n = Notification::create().map_err(|_| 5)?;
    let caps = [CapSend::copy(&n.0, rights::ALL)];
    let mut r = ep
        .call(
            &Outgoing { label: op::ECHO, data: &[], caps: &caps },
            &mut [],
            0,
            time::after_ms(10_000),
        )
        .map_err(|_| 5)?;
    check(5, r.cap_count() == 1)?;
    let back = Notification(r.take_cap(0).ok_or(5)?);
    check(5, back.0.raw() != n.0.raw())?;
    back.signal(0x40).map_err(|_| 5)?;
    check(5, n.wait(time::NOW) == Ok(0x40))?;
    // 6: a moved capability: the sender's handle is gone, the returned one works.
    let m = Notification::create().map_err(|_| 6)?;
    let old = m.0.raw();
    let caps = [CapSend::moved(m.0, rights::ALL)];
    let mut r = ep
        .call(
            &Outgoing { label: op::ECHO, data: &[], caps: &caps },
            &mut [],
            0,
            time::after_ms(10_000),
        )
        .map_err(|_| 6)?;
    let back = Notification(r.take_cap(0).ok_or(6)?);
    check(6, sys::call(num::HANDLE_INFO, [old.into(), 0, 0, 0, 0, 0]) == Err(Error::BadHandle))?;
    back.signal(1).map_err(|_| 6)?;
    check(6, back.wait(time::NOW) == Ok(1))?;
    // 7: rights can only shrink on the way.
    let caps = [CapSend::copy(&n.0, rights::SIGNAL | rights::TRANSFER)];
    let r = ep
        .call(
            &Outgoing { label: op::RIGHTS, data: &[], caps: &caps },
            &mut [],
            0,
            time::after_ms(10_000),
        )
        .map_err(|_| 7)?;
    check(7, r.data() == (rights::SIGNAL | rights::TRANSFER).to_le_bytes())?;
    // 8: badges tell clients apart.
    let b1 = call(&ep, op::BADGE, &[]).map_err(|_| 8)?;
    let b2 = call(&ep2, op::BADGE, &[]).map_err(|_| 8)?;
    check(8, b1.data() == badge1.to_le_bytes() && b2.data() == badge2.to_le_bytes())?;
    // 9: a capability that may not be transferred can't be sent.
    let fixed = n.0.duplicate(rights::SIGNAL | rights::WAIT).map_err(|_| 9)?;
    let caps = [CapSend::copy(&fixed, rights::ALL)];
    let r = ep.call(
        &Outgoing { label: op::ECHO, data: &[], caps: &caps },
        &mut [],
        0,
        time::after_ms(10_000),
    );
    check(9, r.err() == Some(Error::NoRight))?;
    Ok(())
}

// ------------------------------------------------------------------------------------------
// Isolation: things a program mustn't be able to do fail cleanly

fn isolation(start: &mut granite::Start) -> Result<(), i32> {
    // 1: handles it was never given.
    for h in [0u32, 0x7_FFFF, 12345, (5 << 20) | 900, 0xFFFF_FFFF, 0x8000_0001] {
        let r = sys::call(num::NOTIFICATION_SIGNAL, [h.into(), 1, 0, 0, 0, 0]);
        check(1, r == Err(Error::BadHandle))?;
    }
    // 2: a handle number past 32 bits.
    check(2, sys::call(num::HANDLE_INFO, [1 << 40, 0, 0, 0, 0, 0]) == Err(Error::BadHandle))?;
    // 3: memory it may read but not map.
    let ro = Memory(start.take(role::MEMORY_NO_MAP).ok_or(3)?);
    check(3, ro.map(perm::READ).err() == Some(Error::NoRight))?;
    let mut b = [0u8; 16];
    check(3, ro.read(0, &mut b).is_ok())?;
    check(3, ro.write(0, &b) == Err(Error::NoRight))?;
    // 4: an endpoint capability without the send right.
    let ns = endpoint(start, role::NO_SEND)?;
    check(4, call(&ns, op::ECHO, &[]).err() == Some(Error::NoRight))?;
    // 5: writable and executable at once is refused.
    let m = Memory::create(4096).map_err(|_| 5)?;
    let me = process::self_handle();
    check(5, m.map_into(me, 0, 4096, 0, 7).err() == Some(Error::InvalidArgument))?;
    check(5, m.map_into(me, 0, 4096, 0, 2).err() == Some(Error::InvalidArgument))?; // write-only
    // 6: bad addresses passed to the kernel.
    let log = granite::log::handle();
    for addr in [0x10u64, 0xFFFF_8000_0000_0000, 0x0000_7FFF_FFFF_F000, u64::MAX - 4] {
        let r = sys::call(num::LOG_WRITE, [log.into(), addr, 5, 0, 0, 0]);
        check(6, r == Err(Error::BadAddress))?;
    }
    // 7: reading memory into a read-only mapping.
    let page = m.map(perm::READ).map_err(|_| 7)?;
    let r = sys::call(num::MEMORY_READ, [m.0.raw().into(), 0, page.addr(), 16, 0, 0]);
    check(7, r == Err(Error::BadAddress))?;
    // 8: a system call that doesn't exist.
    check(8, sys::call(999, [0; 6]) == Err(Error::InvalidCall))?;
    // 9: mapping over the kernel's half, or the first 64 KiB.
    check(9, m.map_into(me, 0, 4096, 0xFFFF_8000_0000_0000, perm::READ).is_err())?;
    check(9, m.map_into(me, 0, 4096, 0x1000, perm::READ).is_err())?;
    // 10: copies can't gain rights.
    let weak = m.0.duplicate(rights::READ | rights::DUPLICATE).map_err(|_| 10)?;
    let again = weak.duplicate(rights::ALL).map_err(|_| 10)?;
    check(10, again.info().map(|(_, r)| r) == Ok(rights::READ | rights::DUPLICATE))?;
    // 11: a badged endpoint can't be given another badge.
    let e = Endpoint::create().map_err(|_| 11)?;
    let b = e.mint(5, rights::ALL).map_err(|_| 11)?;
    check(11, b.mint(6, rights::ALL).is_err())?;
    Ok(())
}

// These end in a fault, which the root program checks for. Returning means the fault didn't
// happen.

fn fault_readonly() -> Result<(), i32> {
    let m = Memory::create(4096).map_err(|_| 1)?;
    let page = m.map(perm::READ).map_err(|_| 1)?;
    // SAFETY: none: the page is read-only, so this must fault.
    unsafe { core::ptr::write_volatile(page.as_mut_ptr(), 1) };
    Err(2)
}

fn fault_kernel() -> Result<(), i32> {
    // SAFETY: none: kernel memory, so this must fault.
    let x = unsafe { core::ptr::read_volatile(0xFFFF_8000_0000_1000 as *const u64) };
    core::hint::black_box(x);
    Err(2)
}

fn fault_null() -> Result<(), i32> {
    // SAFETY: none: the first 64 KiB are never mapped, so this must fault.
    let x = unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<u64>(8)) };
    core::hint::black_box(x);
    Err(2)
}

fn fault_instruction() -> Result<(), i32> {
    // SAFETY: none: an undefined instruction, which must fault.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("ud2");
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("udf #0");
    }
    Err(2)
}

// ------------------------------------------------------------------------------------------
// Revocation

/// Calls the echo service until the capability stops working (the root program revokes it).
fn revoke_endpoint(start: &mut granite::Start) -> Result<(), i32> {
    let ep = endpoint(start, role::ENDPOINT)?;
    let ready = Notification(start.take(role::READY).ok_or(1)?);
    let mut ok = 0u32;
    let until = time::after_ms(20_000);
    while time::now() < until {
        match call(&ep, op::ECHO, b"x") {
            Ok(_) => {
                ok += 1;
                if ok == 1 {
                    ready.signal(1).map_err(|_| 2)?;
                }
            }
            Err(Error::BadHandle | Error::Revoked) => return check(3, ok > 0),
            Err(_) => return Err(4),
        }
        thread::yield_now();
    }
    Err(5)
}

/// Maps the memory it was given and keeps reading it: once the root program revokes it, the
/// mapping is gone and the read faults.
fn revoke_memory(start: &mut granite::Start) -> Result<(), i32> {
    let m = Memory(start.take(role::MEMORY).ok_or(1)?);
    let ready = Notification(start.take(role::READY).ok_or(1)?);
    let page = m.map(perm::READ_WRITE).map_err(|_| 2)?;
    for i in 0..4096u64 {
        // SAFETY: inside the mapping just made.
        unsafe { core::ptr::write_volatile(page.as_mut_ptr().add(i as usize), (i * 13) as u8) };
    }
    ready.signal(1).map_err(|_| 3)?;
    let until = time::after_ms(20_000);
    let mut sum = 0u64;
    while time::now() < until {
        // SAFETY: mapped until it's revoked; then this faults, which is the point.
        sum = sum.wrapping_add(unsafe { core::ptr::read_volatile(page.as_ptr()) }.into());
        time::sleep_ms(1);
    }
    core::hint::black_box(sum);
    Err(4)
}

/// Holds an endpoint capability, signals it's ready, and exits.
fn sender(start: &mut granite::Start) -> Result<(), i32> {
    let _ep = endpoint(start, role::ENDPOINT)?;
    let ready = Notification(start.take(role::READY).ok_or(1)?);
    ready.signal(1).map_err(|_| 2)?;
    time::sleep_ms(20);
    Ok(())
}

/// Crashes the service it calls, and expects to be told it's gone.
fn crash_service(start: &mut granite::Start) -> Result<(), i32> {
    let ep = endpoint(start, role::ENDPOINT)?;
    check(1, call(&ep, op::CRASH, &[]).err() == Some(Error::PeerGone))?;
    // And it stays gone.
    check(2, call(&ep, op::ECHO, &[]).err() == Some(Error::PeerGone))
}

// ------------------------------------------------------------------------------------------
// Budgets and load

/// Asks for more memory than its budget of `budget` bytes, then for a little.
fn budget(budget: u64) -> Result<(), i32> {
    check(1, Memory::create(budget).err() == Some(Error::OutOfMemory))?;
    let small = Memory::create(16 * 1024).map_err(|_| 2)?;
    check(3, small.size() == Ok(16 * 1024))?;
    let info = process::info(process::self_handle()).map_err(|_| 4)?;
    check(4, info.budget == budget && info.used <= budget)
}

/// `threads` threads each make `calls` calls to the echo service, checking every reply.
fn load(start: &mut granite::Start, threads: u64, calls: u64) -> Result<(), i32> {
    let ep = endpoint(start, role::ENDPOINT)?;
    let raw = ep.0.into_raw();
    let failures = alloc::sync::Arc::new(core::sync::atomic::AtomicU64::new(0));
    let mut joins = Vec::new();
    for t in 0..threads {
        let f = failures.clone();
        let j = thread::spawn(move || {
            let ep = Endpoint(Handle::from_raw(raw));
            for i in 0..calls {
                let data = [(t as u8).wrapping_mul(31) ^ i as u8; 64];
                match call(&ep, op::ECHO, &data) {
                    Ok(r) if r.data() == data => {}
                    _ => {
                        f.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            let _ = ep.0.into_raw(); // shared: closed once, below
        })
        .map_err(|_| 1)?;
        joins.push(j);
    }
    for j in joins {
        j.join().map_err(|_| 2)?;
    }
    drop(Handle::from_raw(raw));
    check(3, failures.load(core::sync::atomic::Ordering::SeqCst) == 0)
}
