// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Device interrupts on x86-64 (docs/design/0010-device-access.md, section 4): the I/O APICs,
//! which turn interrupt lines into messages for the local APICs, and the message-signalled
//! interrupts devices send themselves. The kernel hands out vectors 0x30 to 0xEF.

use super::apic;
use crate::mem::vm;
use crate::obj::Error;
use crate::obj::device::{MSI_BASE, MsiVector, WIRED_END};
use crate::sync::{IrqSpinLock, Level};
use crate::{hw, irq};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use pbr_ranges::VectorPool;

/// The first and last vectors given to devices.
pub const FIRST_VECTOR: u8 = 0x30;
/// See [`FIRST_VECTOR`].
pub const LAST_VECTOR: u8 = 0xEF;

const NONE: u32 = u32::MAX;
/// The interrupt number each vector belongs to (`NONE` if nobody).
static VECTOR_ID: [AtomicU32; 256] = [const { AtomicU32::new(NONE) }; 256];
/// Spreads message-signalled interrupts over the cores.
static NEXT_TARGET: AtomicU32 = AtomicU32::new(0);

struct IoApic {
    base: u64,
    first: u32,
    pins: u32,
}

struct State {
    ioapics: Vec<IoApic>,
    pool: VectorPool,
    /// The vector each line is using (0: none), by global system interrupt.
    wired: [u8; WIRED_END as usize],
    /// ISA overrides: (global system interrupt, flags).
    overrides: Vec<(u32, u16)>,
    /// Where line interrupts are sent: the boot core's APIC ID.
    line_target: u32,
    /// APIC IDs of cores message-signalled interrupts may go to (the message format has 8 bits).
    msi_targets: Vec<u32>,
}

static STATE: IrqSpinLock<Option<State>> =
    IrqSpinLock::new(Level::InterruptController, "interrupt controllers", None);

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;
const MASKED: u32 = 1 << 16;

fn io_read(base: u64, reg: u32) -> u32 {
    // SAFETY: the I/O APIC's index and data registers, mapped uncached; the caller holds the
    // state lock, so the pair of accesses isn't interleaved.
    unsafe {
        core::ptr::write_volatile((base + IOREGSEL) as *mut u32, reg);
        core::ptr::read_volatile((base + IOWIN) as *const u32)
    }
}

fn io_write(base: u64, reg: u32, v: u32) {
    // SAFETY: as for `io_read`.
    unsafe {
        core::ptr::write_volatile((base + IOREGSEL) as *mut u32, reg);
        core::ptr::write_volatile((base + IOWIN) as *mut u32, v);
    }
}

impl State {
    fn pin(&self, gsi: u32) -> Option<(u64, u32)> {
        self.ioapics
            .iter()
            .find(|a| gsi >= a.first && gsi < a.first + a.pins)
            .map(|a| (a.base, gsi - a.first))
    }
}

/// Maps the I/O APICs and masks every line (the firmware may have left some open), and sets
/// up the pool of vectors.
pub fn init(m: &hw::Machine) {
    let mut ioapics = Vec::new();
    for &(addr, first) in &m.ioapics {
        let base = vm::map_device(addr, 0x20);
        let pins = ((io_read(base, 1) >> 16) & 0xFF) + 1;
        for p in 0..pins {
            io_write(base, 0x10 + 2 * p, MASKED);
        }
        ioapics.push(IoApic { base, first, pins });
    }
    let msi_targets: Vec<u32> =
        m.cores.iter().map(|c| c.hw_id as u32).filter(|&i| i <= 0xFF).collect();
    *STATE.lock() = Some(State {
        ioapics,
        pool: VectorPool::new(u32::from(FIRST_VECTOR), u32::from(LAST_VECTOR)),
        wired: [0; WIRED_END as usize],
        overrides: m.overrides.iter().map(|&(_, gsi, flags)| (gsi, flags)).collect(),
        line_target: m.cores.first().map_or(0, |c| c.hw_id as u32),
        msi_targets,
    });
}

/// The range of line interrupts: (first, count).
pub fn wired_range() -> (u32, u32) {
    match STATE.lock().as_ref() {
        Some(s) => {
            let end = s.ioapics.iter().map(|a| a.first + a.pins).max().unwrap_or(0);
            (0, end.min(WIRED_END))
        }
        None => (0, 0),
    }
}

/// True if line `id` exists.
pub fn wired_usable(id: u32) -> bool {
    STATE.lock().as_ref().is_some_and(|s| s.pin(id).is_some())
}

/// Message-signalled interrupts work on every x86-64 machine with a local APIC.
pub fn msi_available() -> bool {
    STATE.lock().as_ref().is_some_and(|s| !s.msi_targets.is_empty())
}

/// Takes a vector for a message-signalled interrupt.
pub fn msi_allocate() -> Result<MsiVector, Error> {
    let mut g = STATE.lock();
    let s = g.as_mut().ok_or(Error::NotSupported)?;
    if s.msi_targets.is_empty() {
        return Err(Error::NotSupported);
    }
    let v = s.pool.take().ok_or(Error::OutOfMemory)?;
    let n = NEXT_TARGET.fetch_add(1, Ordering::Relaxed) as usize;
    let target = s.msi_targets[n % s.msi_targets.len()];
    let id = MSI_BASE + v;
    VECTOR_ID[v as usize].store(id, Ordering::SeqCst);
    // Fixed delivery, edge-triggered, to the core with that APIC ID: the local APIC's message
    // format. The vector queue sends the vector to the back when it is given up.
    Ok(MsiVector { id, address: 0xFEE0_0000 | (u64::from(target) << 12), data: v })
}

/// Gives a message-signalled interrupt's vector back.
pub fn msi_free(id: u32) {
    let v = id - MSI_BASE;
    VECTOR_ID[v as usize].store(NONE, Ordering::SeqCst);
    if let Some(s) = STATE.lock().as_mut() {
        s.pool.give(v);
    }
}

/// Gets line `id` ready (masked) with a vector of its own. Message-signalled interrupts are
/// ready already.
pub fn attach(id: u32) -> Result<(), Error> {
    if id >= MSI_BASE {
        return Ok(());
    }
    let mut g = STATE.lock();
    let s = g.as_mut().ok_or(Error::NotSupported)?;
    let (base, pin) = s.pin(id).ok_or(Error::InvalidArgument)?;
    // ISA lines default to active high and edge-triggered; the firmware's overrides change
    // that. Everything else (PCI lines) is active low and level-triggered.
    let flags = s.overrides.iter().find(|o| o.0 == id).map(|o| o.1);
    let (polarity, trigger) = match flags {
        Some(f) => {
            let p = f & 3;
            let t = (f >> 2) & 3;
            let default_isa = id < 16;
            (if p == 0 { !default_isa } else { p == 3 }, if t == 0 { !default_isa } else { t == 3 })
        }
        None if id < 16 => (false, false),
        None => (true, true),
    };
    let v = s.pool.take().ok_or(Error::OutOfMemory)?;
    s.wired[id as usize] = v as u8;
    VECTOR_ID[v as usize].store(id, Ordering::SeqCst);
    let low = v | MASKED | (u32::from(polarity) << 13) | (u32::from(trigger) << 15);
    io_write(base, 0x11 + 2 * pin, s.line_target << 24);
    io_write(base, 0x10 + 2 * pin, low);
    Ok(())
}

/// Masks line `id` and gives its vector back.
pub fn detach(id: u32) {
    if id >= MSI_BASE {
        return;
    }
    let mut g = STATE.lock();
    let Some(s) = g.as_mut() else { return };
    let v = core::mem::take(&mut s.wired[id as usize]);
    if let Some((base, pin)) = s.pin(id) {
        io_write(base, 0x10 + 2 * pin, MASKED);
    }
    if v != 0 {
        VECTOR_ID[v as usize].store(NONE, Ordering::SeqCst);
        s.pool.give(u32::from(v));
    }
}

fn set_mask(id: u32, masked: bool) {
    if id >= MSI_BASE {
        return; // message-signalled interrupts are edge-triggered: nothing to mask
    }
    let g = STATE.lock();
    let Some(s) = g.as_ref() else { return };
    if s.wired[id as usize] == 0 {
        return;
    }
    if let Some((base, pin)) = s.pin(id) {
        let reg = 0x10 + 2 * pin;
        let cur = io_read(base, reg);
        io_write(base, reg, if masked { cur | MASKED } else { cur & !MASKED });
    }
}

/// Lets line `id` interrupt.
pub fn unmask(id: u32) {
    set_mask(id, false);
}

/// Makes interrupt `id` pending by software, by sending its vector to this core.
pub fn trigger(id: u32) -> Result<(), Error> {
    let v = if id >= MSI_BASE {
        (id - MSI_BASE) as u8
    } else {
        let g = STATE.lock();
        g.as_ref().map_or(0, |s| s.wired[id as usize])
    };
    if v == 0 {
        return Err(Error::BadState);
    }
    apic::send(super::this_core_hw_id() as u32, u32::from(v));
    Ok(())
}

/// The handler for a device vector: masks a line (so a level-triggered one can't fire again
/// at once), tells the local APIC it's done, and passes the interrupt on.
pub fn interrupt(vector: u8) {
    let id = VECTOR_ID[vector as usize].load(Ordering::SeqCst);
    if id != NONE && id < MSI_BASE {
        set_mask(id, true);
    }
    apic::eoi();
    if id != NONE {
        irq::fired(id);
    }
}

/// True if `vector` is one of the device vectors.
pub fn is_device_vector(vector: u8) -> bool {
    (FIRST_VECTOR..=LAST_VECTOR).contains(&vector)
}
