// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The system calls (docs/design/0007-system-calls.md, section 6; docs/design/0010-device-access.md,
//! section 9; and what interface versions 1.2 and 1.3 added, notes 0013 and 0019).
//!
//! The architecture layer passes the call number and six arguments, and returns the status
//! (0, or an error code) and two results. Every argument is checked; a call that fails changes
//! nothing.

use crate::mem::object::MemoryObject;
use crate::mem::space::perm;
use crate::obj::endpoint::{CallerCap, Endpoint, Received};
use crate::obj::notification::Notification;
use crate::obj::process::{Process, Status};
use crate::obj::table::Entry;
use crate::obj::{Error, Object, caps, charge, require, rights};
use crate::sched::{self, Thread};
use crate::{kprintln, time, user};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;

/// The interface version (note 0007, section 7; 1.2 is note 0013, 1.3 note 0019).
pub const VERSION: (u64, u64) = (1, 3);

type R = Result<(u64, u64), Error>;

/// Handles one system call. Returns (status, result 1, result 2).
pub fn handle(num: u64, a: [u64; 6]) -> (u64, u64, u64) {
    // The calls that never return are handled first, holding no references: anything still
    // held on this stack when the thread exits would never be released.
    match num {
        25 => {
            if let Some(p) = sched::current().user.as_ref().map(|u| u.process.clone()) {
                p.end(Status::Exited(a[0]));
            }
            user::exit_thread()
        }
        42 => user::exit_thread(),
        _ => {}
    }
    let me = sched::current();
    let Some(u) = me.user.as_ref() else { return (Error::BadState as u64, 0, 0) };
    let p = u.process.clone();
    match dispatch(&me, &p, num, a) {
        Ok((r1, r2)) => (0, r1, r2),
        Err(e) => (e as u64, 0, 0),
    }
}

fn handle_arg(x: u64) -> Result<u32, Error> {
    u32::try_from(x).map_err(|_| Error::BadHandle)
}

fn get(p: &Process, h: u64) -> Result<Entry, Error> {
    caps::lookup(p, handle_arg(h)?)
}

fn process(p: &Process, h: u64, need: u32) -> Result<Arc<Process>, Error> {
    let e = get(p, h)?;
    let Object::Process(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn thread(p: &Process, h: u64, need: u32) -> Result<Arc<Thread>, Error> {
    let e = get(p, h)?;
    let Object::Thread(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn memory(p: &Process, h: u64, need: u32) -> Result<Arc<MemoryObject>, Error> {
    let e = get(p, h)?;
    let Object::Memory(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn device_memory(
    p: &Process,
    h: u64,
    need: u32,
) -> Result<Arc<crate::obj::device::DeviceMemory>, Error> {
    let e = get(p, h)?;
    let Object::DeviceMemory(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn interrupts(
    p: &Process,
    h: u64,
    need: u32,
) -> Result<Arc<crate::obj::device::Interrupts>, Error> {
    let e = get(p, h)?;
    let Object::Interrupts(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn io_ports(p: &Process, h: u64, need: u32) -> Result<Arc<crate::obj::device::IoPorts>, Error> {
    let e = get(p, h)?;
    let Object::IoPorts(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

fn notification(p: &Process, h: u64, need: u32) -> Result<Arc<Notification>, Error> {
    let e = get(p, h)?;
    let Object::Notification(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    Ok(x)
}

/// An endpoint capability: (the endpoint, its badge, a check that it's still held).
fn endpoint(
    p: &Arc<Process>,
    h: u64,
    need: u32,
) -> Result<(Arc<Endpoint>, u64, impl Fn() -> bool + use<>), Error> {
    let hv = handle_arg(h)?;
    let e = caps::lookup(p, hv)?;
    let Object::Endpoint(x) = e.object else { return Err(Error::WrongType) };
    require(e.rights, need)?;
    let (q, node) = (p.clone(), e.node);
    Ok((x, e.badge, move || q.table.lock().holds(hv, node)))
}

/// Splits `memory_map`'s permission word into the permissions and the cache mode (bits 8 and
/// 9; only device memory accepts a mode other than 0). Returns (permissions, write-combining).
fn map_perms(x: u64) -> Result<(u32, bool), Error> {
    let cache = (x >> 8) & 3;
    if x >> 10 != 0 || cache > 1 {
        return Err(Error::InvalidArgument);
    }
    Ok((perms(x & 0xFF)?, cache == 1))
}

/// Mapping permissions: readable, readable and writable, or readable and executable.
fn perms(x: u64) -> Result<u32, Error> {
    let p = u32::try_from(x).map_err(|_| Error::InvalidArgument)?;
    if p & !(perm::READ | perm::WRITE | perm::EXECUTE) != 0
        || p & perm::READ == 0
        || p & perm::WRITE != 0 && p & perm::EXECUTE != 0
    {
        return Err(Error::InvalidArgument);
    }
    Ok(p)
}

fn page_aligned(x: u64) -> Result<(), Error> {
    if x.is_multiple_of(user::PAGE) { Ok(()) } else { Err(Error::InvalidArgument) }
}

fn dispatch(me: &Arc<Thread>, p: &Arc<Process>, num: u64, a: [u64; 6]) -> R {
    match num {
        // Version and time.
        1 => Ok(VERSION),
        2 => Ok((time::now_ns(), 0)),

        // Handles.
        10 => caps::close(p, handle_arg(a[0])?).map(|()| (0, 0)),
        11 => {
            let mask = u32::try_from(a[1]).unwrap_or(u32::MAX);
            caps::duplicate(p, handle_arg(a[0])?, mask, None).map(|h| (u64::from(h), 0))
        }
        12 => caps::revoke(p, handle_arg(a[0])?).map(|()| (0, 0)),
        13 => {
            let e = get(p, a[0])?;
            Ok((u64::from(e.object.kind()), u64::from(e.rights)))
        }

        // Processes.
        20 => {
            // The second argument: bits 0 to 7 the maximum priority, bit 8 `TIED` (note
            // 0013, section 3), the rest 0.
            let (budget, name_addr, name_len) = (a[0], a[2], a[3]);
            let (max_prio, tied) = (a[1] & 0xFF, a[1] & 0x100 != 0);
            if name_len > 64 || a[1] >> 9 != 0 || max_prio > u64::from(p.max_priority) {
                return Err(Error::InvalidArgument);
            }
            let mut name = vec![0u8; name_len as usize];
            p.space.lock().copy_in(name_addr, &mut name)?;
            let name = String::from_utf8(name).map_err(|_| Error::InvalidArgument)?;
            let b = crate::obj::budget::Budget::child(&p.budget, budget)?;
            let child = Process::new(&name, b, max_prio as u8)?;
            if tied {
                p.adopt_tied(&child)?;
            }
            caps::create(p, Object::Process(child)).map(|h| (u64::from(h), 0))
        }
        21 => {
            let target = process(p, a[0], rights::MANAGE)?;
            let mask = u32::try_from(a[2]).unwrap_or(u32::MAX);
            let moving = match a[3] {
                0 => false,
                1 => true,
                _ => return Err(Error::InvalidArgument),
            };
            let mut tree = caps::lock();
            caps::give(&mut tree, p, handle_arg(a[1])?, &target, mask, moving)
                .map(|(h, _)| (u64::from(h), 0))
        }
        22 => {
            let target = process(p, a[0], rights::STOP)?;
            target.end(Status::Stopped(a[1]));
            Ok((0, 0))
        }
        23 => {
            let target = process(p, a[0], rights::INSPECT)?;
            let rec = target.info();
            let n = (a[2] as usize).min(rec.len());
            p.space.lock().copy_out(a[1], &rec[..n])?;
            Ok((n as u64, 0))
        }
        24 => {
            let target = process(p, a[0], rights::WATCH)?;
            let n = notification(p, a[1], rights::SIGNAL)?;
            target.watch(n, a[2]);
            Ok((0, 0))
        }

        // Memory.
        30 => {
            if a[1] != 0 {
                return Err(Error::InvalidArgument);
            }
            let m = MemoryObject::new(a[0], &p.budget)?;
            caps::create(p, Object::Memory(m)).map(|h| (u64::from(h), 0))
        }
        31 => {
            let target = process(p, a[0], rights::MANAGE)?;
            let (offset, len, addr) = (a[2], a[3], a[4]);
            let (pm, combining) = map_perms(a[5])?;
            page_aligned(offset)?;
            page_aligned(len)?;
            page_aligned(addr)?;
            caps::map(p, handle_arg(a[1])?, &target, offset, len, addr, pm, combining)
                .map(|at| (at, 0))
        }
        32 => {
            let target = process(p, a[0], rights::MANAGE)?;
            caps::unmap(&target, a[1], a[2]).map(|()| (0, 0))
        }
        33 => {
            let target = process(p, a[0], rights::MANAGE)?;
            let pm = perms(a[3])?;
            target.space.lock().protect(a[1], a[2], pm).map(|()| (0, 0))
        }
        34 | 35 => {
            let writing = num == 35;
            let m = memory(p, a[0], if writing { rights::WRITE } else { rights::READ })?;
            let (offset, buf, len) = (a[1], a[2], a[3]);
            let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
            if end > m.size() {
                return Err(Error::InvalidArgument);
            }
            let need = if writing { perm::READ } else { perm::WRITE };
            p.space.lock().check(buf, len, need)?;
            let mut chunk = vec![0u8; 4096];
            let mut done = 0;
            while done < len {
                let n = (len - done).min(4096) as usize;
                if writing {
                    p.space.lock().copy_in(buf + done, &mut chunk[..n])?;
                    m.write(offset + done, &chunk[..n])?;
                } else {
                    m.read(offset + done, &mut chunk[..n])?;
                    p.space.lock().copy_out(buf + done, &chunk[..n])?;
                }
                done += n as u64;
            }
            Ok((0, 0))
        }
        36 => Ok((memory(p, a[0], 0)?.size(), 0)),
        37 => {
            let d = device_memory(p, a[0], rights::DMA)?;
            if a[2] & !1 != 0 {
                return Err(Error::InvalidArgument);
            }
            let _ = d;
            let m = MemoryObject::contiguous(a[1], a[2] & 1 != 0, &p.budget)?;
            let r = Object::Memory(m.clone()).all_rights() | rights::BUS;
            let mut tree = caps::lock();
            caps::insert(&mut tree, p, Object::Memory(m), r, 0, None).map(|h| (u64::from(h), 0))
        }
        38 => {
            let m = memory(p, a[0], rights::BUS)?;
            let _ = device_memory(p, a[1], rights::DMA)?;
            let page = (a[2] / user::PAGE).min(u64::MAX / user::PAGE);
            m.page(page).map(|phys| (phys, 0)).ok_or(Error::InvalidArgument)
        }

        // Threads.
        40 => {
            let target = process(p, a[0], rights::MANAGE)?;
            let prio = u8::try_from(a[4]).map_err(|_| Error::InvalidArgument)?;
            let t = user::create_thread(&target, a[1], a[2], a[3], prio)?;
            caps::create(p, Object::Thread(t)).map(|h| (u64::from(h), 0))
        }
        41 => {
            let t = thread(p, a[0], rights::MANAGE)?;
            let u = t.user.as_ref().ok_or(Error::WrongType)?;
            u.process.start_thread(&t).map(|()| (0, 0))
        }
        43 => {
            let t = thread(p, a[0], rights::STOP)?;
            t.stop.store(true, core::sync::atomic::Ordering::SeqCst);
            sched::kick(&t);
            Ok((0, 0))
        }
        44 => {
            sched::yield_now();
            Ok((0, 0))
        }
        45 => match sched::wait_event(a[0], || false) {
            sched::Woken::Stopped => Err(Error::BadState),
            _ => Ok((0, 0)),
        },
        46 => {
            let t = thread(p, a[0], rights::WATCH)?;
            let n = notification(p, a[1], rights::SIGNAL)?;
            t.user.as_ref().ok_or(Error::WrongType)?.watch(n, a[2]);
            Ok((0, 0))
        }
        47 => {
            if a[0] >= 0x0000_8000_0000_0000 {
                return Err(Error::InvalidArgument);
            }
            me.user.as_ref().ok_or(Error::BadState)?.set_thread_pointer(a[0]);
            Ok((0, 0))
        }

        // Endpoints and messages.
        50 => {
            let ep = Endpoint::new(Some(p.budget.charge(charge::ENDPOINT)?));
            caps::create(p, Object::Endpoint(ep)).map(|h| (u64::from(h), 0))
        }
        51 => {
            let mask = u32::try_from(a[2]).unwrap_or(u32::MAX);
            caps::duplicate(p, handle_arg(a[0])?, mask, Some(a[1])).map(|h| (u64::from(h), 0))
        }
        52 => {
            let (ep, badge, held) = endpoint(p, a[0], rights::SEND)?;
            // Remember which capability the call came through, for a derived reply (note 0013).
            let hv = handle_arg(a[0])?;
            let node = caps::lookup(p, hv)?.node;
            let through = CallerCap { process: Arc::downgrade(p), handle: hv, node };
            ep.call(me, badge, a[1], through, held).map(|()| (0, 0))
        }
        53 => {
            let (ep, _, held) = endpoint(p, a[0], rights::RECEIVE)?;
            ep.receive(me, a[1], held).map(received)
        }
        54 => reply(me, p, a[0]).map(|()| (0, 0)),
        55 => {
            let (ep, _, held) = endpoint(p, a[1], rights::RECEIVE)?;
            let replied = reply(me, p, a[0]).err().map_or(0, |e| e as u64);
            ep.receive(me, a[2], held).map(|r| {
                let (kind, value) = received(r);
                (kind | (replied << 32), value)
            })
        }
        56 => {
            let (ep, _, _) = endpoint(p, a[0], rights::RECEIVE)?;
            let n = notification(p, a[1], rights::WAIT)?;
            ep.bind(n).map(|()| (0, 0))
        }
        57 => {
            let (ep, _, _) = endpoint(p, a[0], rights::ENDPOINT_WATCH)?;
            let n = notification(p, a[1], rights::SIGNAL)?;
            ep.watch(n, a[2]);
            Ok((0, 0))
        }
        58 => {
            let mask = u32::try_from(a[2]).map_err(|_| Error::InvalidArgument)?;
            caps::mint_watched(p, handle_arg(a[0])?, mask, a[1]).map(|h| (u64::from(h), 0))
        }
        59 => {
            let n = notification(p, a[1], rights::SIGNAL)?;
            caps::watch_receiver(p, handle_arg(a[0])?, n, a[2]).map(|()| (0, 0))
        }

        // Notifications.
        60 => {
            let n = Notification::new(Some(p.budget.charge(charge::NOTIFICATION)?));
            caps::create(p, Object::Notification(n)).map(|h| (u64::from(h), 0))
        }
        61 => {
            notification(p, a[0], rights::SIGNAL)?.signal(a[1]);
            Ok((0, 0))
        }
        62 => {
            let hv = handle_arg(a[0])?;
            let e = caps::lookup(p, hv)?;
            let Object::Notification(n) = e.object else { return Err(Error::WrongType) };
            require(e.rights, rights::WAIT)?;
            let q = p.clone();
            n.wait(a[1], move || q.table.lock().holds(hv, e.node)).map(|b| (b, 0))
        }

        // The kernel log.
        70 => {
            let e = get(p, a[0])?;
            if !matches!(e.object, Object::Log) {
                return Err(Error::WrongType);
            }
            require(e.rights, rights::WRITE)?;
            if a[2] > 1024 {
                return Err(Error::TooBig);
            }
            let mut text = vec![0u8; a[2] as usize];
            p.space.lock().copy_in(a[1], &mut text)?;
            let text = core::str::from_utf8(&text).map_err(|_| Error::InvalidArgument)?;
            kprintln!("[{}] {}", p.name, text.trim_end_matches('\n'));
            Ok((0, 0))
        }

        72 => {
            let e = get(p, a[0])?;
            if !matches!(e.object, Object::Log) {
                return Err(Error::WrongType);
            }
            require(e.rights, rights::CONTROL)?;
            crate::console::release_screen();
            Ok((0, 0))
        }

        // Device memory, interrupts and I/O ports (note 0010).
        80 => {
            let (offset, length) = (a[1], a[2]);
            let mask = u32::try_from(a[3]).unwrap_or(u32::MAX);
            let c = p.budget.charge(charge::NOTIFICATION)?;
            caps::derive(p, handle_arg(a[0])?, mask, |e| {
                let Object::DeviceMemory(d) = &e.object else { return Err(Error::WrongType) };
                d.slice(offset, length, c).map(Object::DeviceMemory)
            })
            .map(|h| (u64::from(h), 0))
        }
        81 => {
            let d = device_memory(p, a[0], rights::INSPECT)?;
            Ok((d.range.start(), d.range.len()))
        }
        82 => {
            let (first, count) = (a[1], a[2]);
            let mask = u32::try_from(a[3]).unwrap_or(u32::MAX);
            let c = p.budget.charge(charge::NOTIFICATION)?;
            caps::derive(p, handle_arg(a[0])?, mask, |e| {
                let Object::Interrupts(i) = &e.object else { return Err(Error::WrongType) };
                i.slice(first, count, c).map(Object::Interrupts)
            })
            .map(|h| (u64::from(h), 0))
        }
        83 => {
            let i = interrupts(p, a[0], rights::INSPECT)?;
            Ok((i.kind as u64 | i.range.len() << 32, i.range.start()))
        }
        84 => {
            let i = interrupts(p, a[0], rights::BIND)?;
            let _ = i;
            let n = notification(p, a[2], rights::SIGNAL)?;
            caps::bind_interrupt(p, handle_arg(a[0])?, a[1], n, a[3]).map(|()| (0, 0))
        }
        85 => caps::unbind_interrupt(p, handle_arg(a[0])?, a[1]).map(|()| (0, 0)),
        86 => {
            let i = interrupts(p, a[0], rights::ACKNOWLEDGE)?;
            crate::irq::acknowledge(i.number(a[1])?);
            Ok((0, 0))
        }
        87 => {
            let mask = u32::MAX;
            let c = p.budget.charge(charge::NOTIFICATION)?;
            caps::derive(p, handle_arg(a[0])?, mask, |e| {
                let Object::Interrupts(i) = &e.object else { return Err(Error::WrongType) };
                i.allocate(c).map(Object::Interrupts)
            })
            .map(|h| (u64::from(h), 0))
        }
        88 => {
            let i = interrupts(p, a[0], rights::INSPECT)?;
            let v = i.msi.as_ref().ok_or(Error::WrongType)?;
            Ok((v.address, u64::from(v.data)))
        }
        90 => {
            let (first, count) = (a[1], a[2]);
            let mask = u32::try_from(a[3]).unwrap_or(u32::MAX);
            let c = p.budget.charge(charge::NOTIFICATION)?;
            caps::derive(p, handle_arg(a[0])?, mask, |e| {
                let Object::IoPorts(i) = &e.object else { return Err(Error::WrongType) };
                i.slice(first, count, c).map(Object::IoPorts)
            })
            .map(|h| (u64::from(h), 0))
        }
        91 => io_ports(p, a[0], rights::READ)?.read(a[1], a[2]).map(|v| (v, 0)),
        92 => io_ports(p, a[0], rights::WRITE)?.write(a[1], a[2], a[3]).map(|()| (0, 0)),
        93 => {
            let i = io_ports(p, a[0], rights::INSPECT)?;
            Ok((i.range.start(), i.range.len()))
        }
        99 => {
            let i = interrupts(p, a[0], rights::ACKNOWLEDGE)?;
            crate::irq::trigger(i.number(a[1])?).map(|()| (0, 0))
        }

        // The machine (note 0019, section 1).
        100 => {
            let e = get(p, a[0])?;
            if !matches!(e.object, Object::Machine) {
                return Err(Error::WrongType);
            }
            require(e.rights, rights::POWER)?;
            // `power` only comes back with an error. `Infallible` is a type with no values, so
            // the empty `match` tells the compiler the success case can't happen.
            match crate::power::power(a[1])? {}
        }

        _ => Err(Error::InvalidCall),
    }
}

fn received(r: Received) -> (u64, u64) {
    match r {
        Received::Message(h) => (1, u64::from(h)),
        Received::Bits(b) => (2, b),
        Received::Gone(b) => (3, b),
    }
}

fn reply(me: &Arc<Thread>, p: &Arc<Process>, h: u64) -> Result<(), Error> {
    let hv = handle_arg(h)?;
    let e = caps::lookup(p, hv)?;
    let Object::Reply(r) = e.object else { return Err(Error::WrongType) };
    caps::close(p, hv)?; // a reply capability is used once, whatever happens
    r.answer(me)
}
