// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The device access tests (docs/design/0010-device-access.md): the root program holds the
//! big device capabilities, so it can check slicing, mapping, DMA memory, interrupts and I/O
//! ports from the inside, and use child programs for the cases that need a second process
//! (revocation, a program that ends with an interrupt bound).

use crate::selftest::{Files, MEMORY_FAULT, Outcome, spawn, wait};
use alloc::format;
use alloc::string::String;
use granite::Handle;
use granite::device::{self, BELOW_4G, DeviceMemory, Interrupts, IoPorts, interrupt_kind};
use granite::memory::Memory;
use granite::notification::Notification;
use granite::process::{State, role as start_role};
use granite::sys::{self, Error, num, perm, rights};
use granite::time;
use pbr_test_protocol::role;

const PAGE: u64 = 4096;
/// Rights of a slice for tests that hand one on: everything but what's being tested.
const ALL: u32 = rights::ALL;

/// The capabilities the root program has for devices.
struct Caps {
    memory: DeviceMemory,
    interrupts: Option<Interrupts>,
    pool: Option<Interrupts>,
    ports: Option<IoPorts>,
    machine: Memory,
}

/// Runs the device access tests. Returns a short summary.
pub fn run(f: &Files<'_>, start: &mut granite::Start) -> Result<String, String> {
    let take = |start: &mut granite::Start, r: u32| start.take(r);
    let caps = Caps {
        memory: DeviceMemory(take(start, start_role::DEVICE_MEMORY).ok_or("no device memory")?),
        interrupts: take(start, start_role::INTERRUPTS).map(Interrupts),
        pool: take(start, start_role::MSI_POOL).map(Interrupts),
        ports: take(start, start_role::IO_PORTS).map(IoPorts),
        machine: Memory(take(start, start_role::MACHINE).ok_or("no machine description")?),
    };
    let mut parts: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    let flags = description(&caps)?;
    let fb = framebuffer(&caps)?;

    dma(&caps, fb)?;
    parts.push("DMA memory".into());
    // Mapping needs a page of device memory that can be written and read back without
    // upsetting anything, and the only such page a program can find by itself is in the
    // firmware's framebuffer. A machine whose only screens are a virtio-gpu's (note 0020)
    // has none, so there these two are left out; the test machines with a firmware screen,
    // one of each architecture, still run them.
    if let Some(fb) = fb {
        mapping(&caps, fb)?;
        parts.push("mapping device memory".into());
        revoked_mapping(f, &caps, fb)?;
        parts.push("revoking a mapping".into());
    }
    let irq = interrupts(f, &caps)?;
    parts.push(format!("interrupts (line {irq})"));
    if caps.pool.is_some() {
        msi(&caps)?;
        parts.push("message-signalled interrupts".into());
    } else if cfg!(target_arch = "aarch64") {
        // The self-test only runs on QEMU's `virt` machine, whose GICv2m the firmware tables
        // describe; a missing pool would mean the kernel failed to find it.
        return Err("the ARM test machine should have message-signalled interrupts".into());
    } else {
        // The machine description's flag must agree.
        if flags & 2 != 0 {
            return Err("the machine description says there's an MSI pool, but there isn't".into());
        }
        parts.push("no message-signalled interrupts on this machine".into());
    }
    if caps.ports.is_some() {
        ports(&caps)?;
        parts.push("I/O ports".into());
    } else if cfg!(target_arch = "x86_64") {
        return Err("x86-64 should have I/O ports".into());
    }
    screen_right()?;
    Ok(parts.join(", "))
}

fn e(what: &str) -> impl Fn(Error) -> String + '_ {
    move |err| format!("{what}: {err}")
}

/// `want` is the error expected from `got`.
fn refused<T>(what: &str, got: Result<T, Error>, want: Error) -> Outcome {
    match got {
        Err(x) if x == want => Ok(()),
        Err(x) => Err(format!("{what}: expected \"{want}\", got \"{x}\"")),
        Ok(_) => Err(format!("{what}: expected \"{want}\", but it was allowed")),
    }
}

/// Reads the machine description: returns its flags.
fn description(c: &Caps) -> Result<u32, String> {
    let mut head = [0u8; 64];
    c.machine.read(0, &mut head).map_err(e("reading the machine description"))?;
    if &head[..8] != b"PBRMACH\0" {
        return Err("the machine description has the wrong magic".into());
    }
    if u16::from_le_bytes([head[8], head[9]]) != 1 {
        return Err("the machine description has an unknown version".into());
    }
    let lines = u32::from_le_bytes(head[40..44].try_into().unwrap_or([0; 4]));
    if lines == 0 {
        return Err("the machine description says there are no interrupts".into());
    }
    Ok(u32::from_le_bytes(head[12..16].try_into().unwrap_or([0; 4])))
}

/// The last page of the first framebuffer: harmless device memory that is not RAM. (The
/// kernel draws text at the top of the screen.) `None` if the machine has no framebuffer.
fn framebuffer(c: &Caps) -> Result<Option<u64>, String> {
    let mut rec = [0u8; 16];
    c.machine.read(64, &mut rec).map_err(e("reading the machine description"))?;
    let (base, len) = (
        u64::from_le_bytes(rec[..8].try_into().unwrap_or([0; 8])),
        u64::from_le_bytes(rec[8..].try_into().unwrap_or([0; 8])),
    );
    if base == 0 {
        return Ok(None);
    }
    if len < 2 * PAGE {
        return Err("the machine's framebuffer is too small to test with".into());
    }
    Ok(Some((base + len) / PAGE * PAGE - PAGE))
}

/// A slice of one page of device memory at physical address `addr`.
fn page_at(c: &Caps, addr: u64, mask: u32) -> Result<DeviceMemory, Error> {
    c.memory.slice(addr, PAGE, mask)
}

fn dma(c: &Caps, fb: Option<u64>) -> Outcome {
    let m = c.memory.create_dma(4 * PAGE, 0).map_err(e("creating DMA memory"))?;
    if m.size().map_err(e("size"))? != 4 * PAGE {
        return Err("DMA memory has the wrong size".into());
    }
    let a0 = device::bus_address(&m, &c.memory, 0).map_err(e("bus address"))?;
    if a0 % PAGE != 0 {
        return Err("the bus address isn't page-aligned".into());
    }
    for i in 1..4 {
        let a = device::bus_address(&m, &c.memory, i * PAGE).map_err(e("bus address"))?;
        if a != a0 + i * PAGE {
            return Err(format!(
                "DMA memory isn't contiguous: page {i} is at {a:#x}, not {:#x}",
                a0 + i * PAGE
            ));
        }
    }
    // An offset inside a page gives that page's address.
    let mid = device::bus_address(&m, &c.memory, PAGE + 77).map_err(e("bus address"))?;
    if mid != a0 + PAGE {
        return Err("an offset inside a page should round down to the page".into());
    }
    refused(
        "an address past the end",
        device::bus_address(&m, &c.memory, 4 * PAGE),
        Error::InvalidArgument,
    )?;
    // Memory the device can reach with 32 bits.
    let low = c.memory.create_dma(2 * PAGE, BELOW_4G).map_err(e("creating low DMA memory"))?;
    let l = device::bus_address(&low, &c.memory, PAGE).map_err(e("bus address"))?;
    if l + PAGE > 1 << 32 {
        return Err(format!("memory asked for below 4 GiB is at {l:#x}"));
    }
    // The limits.
    refused("17 MiB of DMA memory", c.memory.create_dma(17 << 20, 0), Error::TooBig)?;
    refused("no DMA memory", c.memory.create_dma(0, 0), Error::InvalidArgument)?;
    refused("an unknown flag", c.memory.create_dma(PAGE, 2), Error::InvalidArgument)?;
    // The rights.
    // (A page of the framebuffer if there is one, or else all device memory.)
    let no_dma = match fb {
        Some(fb) => page_at(c, fb, ALL & !rights::DMA).map_err(e("slicing"))?,
        None => DeviceMemory(c.memory.0.duplicate(ALL & !rights::DMA).map_err(e("duplicating"))?),
    };
    refused("DMA memory without the DMA right", no_dma.create_dma(PAGE, 0), Error::NoRight)?;
    refused(
        "a bus address with a device without DMA",
        device::bus_address(&m, &no_dma, 0),
        Error::NoRight,
    )?;
    let no_bus = Memory(m.0.duplicate(ALL & !rights::BUS).map_err(e("duplicating"))?);
    refused(
        "a bus address without BUS",
        device::bus_address(&no_bus, &c.memory, 0),
        Error::NoRight,
    )?;
    // Ordinary memory has no BUS right either.
    let plain = Memory::create(PAGE).map_err(e("creating memory"))?;
    refused(
        "a bus address for ordinary memory",
        device::bus_address(&plain, &c.memory, 0),
        Error::NoRight,
    )?;
    refused(
        "a memory object as a device",
        c.memory.create_dma(PAGE, 0).and_then(|x| DeviceMemory(x.0).create_dma(PAGE, 0)),
        Error::WrongType,
    )?;
    // RAM can't be turned into device memory: here is a RAM address to try.
    refused("slicing RAM", page_at(c, a0, ALL), Error::InvalidArgument)?;
    refused("mapping RAM", c.memory.map(a0, PAGE, perm::READ), Error::InvalidArgument)?;
    refused(
        "slicing past the physical address space",
        c.memory.slice(1 << 49, PAGE, ALL),
        Error::InvalidArgument,
    )?;
    // Refused whatever is at the address, so without a framebuffer any address will do.
    let at = fb.unwrap_or(0xE000_0000);
    refused("an unaligned slice", c.memory.slice(at + 1, PAGE, ALL), Error::InvalidArgument)?;
    refused("an empty slice", c.memory.slice(at, 0, ALL), Error::InvalidArgument)?;
    Ok(())
}

fn mapping(c: &Caps, fb: u64) -> Outcome {
    let d = page_at(c, fb, ALL).map_err(e("slicing the framebuffer"))?;
    if d.info().map_err(e("info"))? != (fb, PAGE) {
        return Err("device_memory_info says something else".into());
    }
    // Uncached, then write-combining: a value written is read back either way.
    for (what, perms) in [
        ("uncached", perm::READ_WRITE),
        ("write-combining", perm::READ_WRITE | perm::WRITE_COMBINING),
    ] {
        let m = d.map(0, PAGE, perms).map_err(e(what))?;
        let p = m.as_mut_ptr().cast::<u32>();
        // SAFETY: inside the mapping just made; framebuffer pixels may hold anything.
        unsafe {
            let old = core::ptr::read_volatile(p);
            core::ptr::write_volatile(p, 0xA5C3_1E7F);
            let back = core::ptr::read_volatile(p);
            core::ptr::write_volatile(p, old);
            if back != 0xA5C3_1E7F {
                return Err(format!("{what}: wrote 0xA5C31E7F, read {back:#x}"));
            }
        }
    }
    refused(
        "executable device memory",
        d.map(0, PAGE, perm::READ_EXECUTE),
        Error::InvalidArgument,
    )?;
    refused("an unknown cache mode", d.map(0, PAGE, perm::READ | 2 << 8), Error::InvalidArgument)?;
    refused("mapping past the end", d.map(0, 2 * PAGE, perm::READ), Error::InvalidArgument)?;
    // A memory object has no cache modes.
    let mem = Memory::create(PAGE).map_err(e("creating memory"))?;
    let a = [
        u64::from(granite::process::self_handle()),
        mem.0.raw().into(),
        0,
        PAGE,
        0,
        perm::READ | perm::WRITE_COMBINING,
    ];
    refused(
        "write-combining ordinary memory",
        sys::call(num::MEMORY_MAP, a),
        Error::InvalidArgument,
    )?;
    // Rights limit the permissions.
    let ro = page_at(c, fb, ALL & !rights::WRITE).map_err(e("slicing"))?;
    refused("writable mapping without WRITE", ro.map(0, PAGE, perm::READ_WRITE), Error::NoRight)?;
    ro.map(0, PAGE, perm::READ).map_err(e("a read-only mapping"))?;
    let no_map = page_at(c, fb, ALL & !rights::MAP).map_err(e("slicing"))?;
    refused("mapping without MAP", no_map.map(0, PAGE, perm::READ), Error::NoRight)?;
    // A slice can't reach outside its parent.
    refused("slicing outside the parent", d.slice(PAGE, PAGE, ALL), Error::InvalidArgument)?;
    let two = c.memory.slice(fb - PAGE, 2 * PAGE, ALL).map_err(e("slicing two pages"))?;
    let upper = two.slice(PAGE, PAGE, ALL).map_err(e("slicing a slice"))?;
    if upper.info().map_err(e("info"))?.0 != fb {
        return Err("a slice of a slice is in the wrong place".into());
    }
    Ok(())
}

fn revoked_mapping(f: &Files<'_>, c: &Caps, fb: u64) -> Outcome {
    let d = page_at(c, fb, ALL).map_err(e("slicing"))?;
    let ready = Notification::create().map_err(e("notification"))?;
    let s = spawn(
        f.probe,
        "probe",
        "device-revoke",
        &[
            (role::DEVICE, d.0.raw(), rights::READ | rights::MAP | rights::DUPLICATE, false),
            (role::READY, ready.0.raw(), rights::SIGNAL, false),
        ],
        8 * crate::selftest::MIB,
    )?;
    ready.wait(time::after_ms(30_000)).map_err(|x| format!("the probe never got going: {x}"))?;
    d.0.revoke().map_err(e("revoking"))?;
    match wait(&s, 60)? {
        State::Faulted(fault) if fault.kind == MEMORY_FAULT => Ok(()),
        other => Err(format!("the probe ended {other:?}, not with a memory fault")),
    }
}

/// Waits for `bits` on `n`.
fn expect(n: &Notification, bits: u64, what: &str) -> Outcome {
    match n.wait(time::after_ms(5_000)) {
        Ok(b) if b & bits == bits => Ok(()),
        Ok(b) => Err(format!("{what}: got bits {b:#x}, wanted {bits:#x}")),
        Err(x) => Err(format!("{what}: {x}")),
    }
}

/// No signal within a short time.
fn quiet(n: &Notification, what: &str) -> Outcome {
    match n.wait(time::after_ms(100)) {
        Err(Error::TimedOut) => Ok(()),
        Ok(b) => Err(format!("{what}: unexpected bits {b:#x}")),
        Err(x) => Err(format!("{what}: {x}")),
    }
}

/// Tests binding, delivery, masking, release. Returns the interrupt number used.
fn interrupts(f: &Files<'_>, c: &Caps) -> Result<u64, String> {
    let all = c.interrupts.as_ref().ok_or("no interrupt lines on this machine")?;
    let (kind, count, first) = all.info().map_err(e("interrupt info"))?;
    if kind != interrupt_kind::WIRED || count == 0 {
        return Err(format!("the interrupt range is kind {kind} with {count} lines"));
    }
    // The last line: nothing on the test machines is wired to it.
    let line = first + u64::from(count) - 1;
    let n = Notification::create().map_err(e("notification"))?;

    let a = all.slice(line, 1, ALL).map_err(e("slicing an interrupt"))?;
    refused(
        "slicing outside the range",
        all.slice(first + u64::from(count), 1, ALL),
        Error::InvalidArgument,
    )?;
    refused("an empty interrupt slice", all.slice(line, 0, ALL), Error::InvalidArgument)?;
    a.bind(0, &n, 0x10).map_err(e("binding"))?;
    refused("binding twice", a.bind(0, &n, 0x10), Error::AlreadyExists)?;
    let again = all.slice(line, 1, ALL).map_err(e("slicing again"))?;
    refused("binding through another capability", again.bind(0, &n, 0x10), Error::AlreadyExists)?;
    refused("binding past the range", a.bind(1, &n, 1), Error::InvalidArgument)?;
    quiet(&n, "before anything fired")?;

    a.trigger(0).map_err(e("triggering"))?;
    expect(&n, 0x10, "the first interrupt")?;
    a.acknowledge(0).map_err(e("acknowledging"))?;
    a.trigger(0).map_err(e("triggering again"))?;
    expect(&n, 0x10, "the second interrupt")?;
    a.acknowledge(0).map_err(e("acknowledging"))?;

    let no_bind = all.slice(line, 1, ALL & !rights::BIND).map_err(e("slicing"))?;
    refused("unbinding without BIND", no_bind.unbind(0), Error::NoRight)?;
    let no_ack = all.slice(line, 1, ALL & !rights::ACKNOWLEDGE).map_err(e("slicing"))?;
    refused("acknowledging without the right", no_ack.acknowledge(0), Error::NoRight)?;
    refused("binding without BIND", no_bind.bind(0, &n, 1), Error::NoRight)?;
    let no_signal = Notification(n.0.duplicate(rights::WAIT).map_err(e("duplicating"))?);
    refused(
        "binding a notification that can't be signalled",
        a.bind(0, &no_signal, 1),
        Error::NoRight,
    )?;

    a.unbind(0).map_err(e("unbinding"))?;
    refused("unbinding twice", a.unbind(0), Error::BadState)?;
    refused("triggering an unbound interrupt", a.trigger(0), Error::BadState)?;

    // Revoking the capability a binding was made through releases it.
    let parent = all.slice(line, 1, ALL).map_err(e("slicing"))?;
    let child = Interrupts(parent.0.duplicate(ALL).map_err(e("duplicating"))?);
    child.bind(0, &n, 0x20).map_err(e("binding through a copy"))?;
    parent.0.revoke().map_err(e("revoking"))?;
    a.bind(0, &n, 0x30).map_err(|x| format!("binding after the revoke: {x}"))?;
    a.unbind(0).map_err(e("unbinding"))?;

    // A program that ends with an interrupt bound loses it.
    let ready = Notification::create().map_err(e("notification"))?;
    let held = all.slice(line, 1, ALL).map_err(e("slicing"))?;
    let s = spawn(
        f.probe,
        "probe",
        "interrupt-hold",
        &[
            (role::INTERRUPT, held.0.raw(), rights::BIND | rights::ACKNOWLEDGE, false),
            (role::READY, ready.0.raw(), rights::SIGNAL, false),
        ],
        8 * crate::selftest::MIB,
    )?;
    ready.wait(time::after_ms(30_000)).map_err(|x| format!("the probe never got going: {x}"))?;
    refused("binding what a running program holds", a.bind(0, &n, 1), Error::AlreadyExists)?;
    match wait(&s, 60)? {
        State::Faulted(_) => {}
        other => return Err(format!("the probe ended {other:?}, not with a fault")),
    }
    a.bind(0, &n, 0x40).map_err(|x| format!("binding after the program ended: {x}"))?;
    a.unbind(0).map_err(e("unbinding"))?;
    Ok(line)
}

fn msi(c: &Caps) -> Outcome {
    let pool = c.pool.as_ref().ok_or("no pool")?;
    let (kind, _, _) = pool.info().map_err(e("pool info"))?;
    if kind != interrupt_kind::POOL {
        return Err(format!("the pool is kind {kind}"));
    }
    refused(
        "binding the pool",
        pool.bind(0, &Notification::create().map_err(e("n"))?, 1),
        Error::InvalidArgument,
    )?;
    refused("MSI info of the pool", pool.msi_info(), Error::WrongType)?;
    if let Some(all) = &c.interrupts {
        refused("allocating from a line range", all.msi_allocate().map(|_| ()), Error::WrongType)?;
    }
    let n = Notification::create().map_err(e("notification"))?;
    let a = pool.msi_allocate().map_err(e("allocating"))?;
    let (kind, count, _) = a.info().map_err(e("info"))?;
    if kind != interrupt_kind::MSI || count != 1 {
        return Err(format!("an MSI is kind {kind} with {count} interrupts"));
    }
    let (address, data) = a.msi_info().map_err(e("MSI info"))?;
    #[cfg(target_arch = "x86_64")]
    if address >> 20 != 0xFEE || !(0x30..=0xEF).contains(&data) {
        return Err(format!("odd MSI message: address {address:#x}, data {data:#x}"));
    }
    #[cfg(target_arch = "aarch64")]
    if address & 0xFFF != 0x40 || data < 32 {
        return Err(format!("odd MSI message: address {address:#x}, data {data:#x}"));
    }
    a.bind(0, &n, 0x20).map_err(e("binding an MSI"))?;
    a.trigger(0).map_err(e("triggering an MSI"))?;
    expect(&n, 0x20, "the MSI")?;
    a.acknowledge(0).map_err(e("acknowledging"))?;
    a.trigger(0).map_err(e("triggering an MSI again"))?;
    expect(&n, 0x20, "the MSI again")?;
    a.unbind(0).map_err(e("unbinding"))?;
    drop(a);
    // A vector given back goes to the end of the queue: the next one is a different vector.
    let b = pool.msi_allocate().map_err(e("allocating again"))?;
    let (_, data2) = b.msi_info().map_err(e("MSI info"))?;
    if data2 == data {
        return Err(format!("the vector {data} was reused at once"));
    }
    Ok(())
}

fn ports(c: &Caps) -> Outcome {
    let all = c.ports.as_ref().ok_or("no ports")?;
    refused(
        "slicing the legacy interrupt controller's ports",
        all.slice(0x20, 2, ALL),
        Error::InvalidArgument,
    )?;
    refused("slicing the console's ports", all.slice(0x3F8, 8, ALL), Error::InvalidArgument)?;
    refused("slicing past the last port", all.slice(0xFFF8, 16, ALL), Error::InvalidArgument)?;
    let com2 = all.slice(0x2F8, 8, ALL).map_err(e("slicing COM2's ports"))?;
    if com2.info().map_err(e("info"))? != (0x2F8, 8) {
        return Err("ioport_info says something else".into());
    }
    com2.read(5, 1).map_err(e("reading a port"))?;
    com2.read(4, 4).map_err(e("reading four bytes"))?;
    com2.write(7, 1, 0x5A).map_err(e("writing a port"))?;
    refused("a port past the range", com2.read(8, 1), Error::InvalidArgument)?;
    refused("a misaligned read", com2.read(1, 2), Error::InvalidArgument)?;
    refused("a width of 3", com2.read(0, 3), Error::InvalidArgument)?;
    refused("a read crossing the end", com2.read(6, 4), Error::InvalidArgument)?;
    let ro = all.slice(0x2F8, 8, ALL & !rights::WRITE).map_err(e("slicing"))?;
    refused("a write without WRITE", ro.write(7, 1, 1), Error::NoRight)?;
    let wo = all.slice(0x2F8, 8, ALL & !rights::READ).map_err(e("slicing"))?;
    refused("a read without READ", wo.read(7, 1), Error::NoRight)?;
    let inner = com2.slice(0x2FC, 4, ALL).map_err(e("slicing a slice"))?;
    inner.read(0, 1).map_err(e("reading the inner slice"))?;
    refused("slicing outside the parent", com2.slice(0x2F0, 8, ALL), Error::InvalidArgument)
}

/// Taking over the screen needs the CONTROL right on the log. (The success case is tested
/// with the display driver, because it stops the kernel's text on the screen.)
fn screen_right() -> Outcome {
    let dup = sys::call(
        num::HANDLE_DUPLICATE,
        [u64::from(granite::log::handle()), u64::from(ALL & !rights::CONTROL), 0, 0, 0, 0],
    )
    .map_err(e("duplicating the log"))?;
    let h = Handle::from_raw(dup.0 as u32);
    refused(
        "releasing the screen without CONTROL",
        device::console_screen_release(&h),
        Error::NoRight,
    )
}
