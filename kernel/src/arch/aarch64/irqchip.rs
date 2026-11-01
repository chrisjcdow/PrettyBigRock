// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Device interrupts on AArch64 (docs/design/0010-device-access.md, section 4): the GIC's
//! shared interrupts, and message-signalled interrupts through a GICv2m frame.

use super::gic;
use crate::mem::vm;
use crate::obj::Error;
use crate::obj::device::{MSI_BASE, MsiVector, WIRED_END};
use crate::sync::{IrqSpinLock, Level};
use crate::{hw, irq};
use pbr_ranges::VectorPool;

/// The GICv2m frame's registers.
const MSI_TYPER: u64 = 0x008;
const MSI_SETSPI_NS: u64 = 0x040;

struct State {
    /// How many interrupt IDs the distributor has.
    count: u32,
    /// The CPU interface line interrupts are sent to (the boot core's).
    interface: u32,
    /// The MSI frame's physical address, and its interrupt IDs: (first, count).
    msi: Option<(u64, u32, u32)>,
    pool: VectorPool,
    /// Which line interrupts are attached.
    attached: [bool; WIRED_END as usize],
}

static STATE: IrqSpinLock<Option<State>> =
    IrqSpinLock::new(Level::InterruptController, "interrupt controllers", None);

/// Finds the MSI frame's interrupt range and sets up the pool.
pub fn init(m: &hw::Machine) {
    let count = gic::interrupt_count();
    let mut msi = None;
    let mut pool = VectorPool::new(1, 0);
    if let Some((base, first, n)) = m.msi_frame {
        let (mut first, mut n) = (u32::from(first), u32::from(n));
        if n == 0 {
            let v = vm::map_device(base, 0x1000);
            // SAFETY: the frame's type register: the first interrupt ID in bits 16 to 25 and
            // how many in bits 0 to 9.
            let typer = unsafe { core::ptr::read_volatile((v + MSI_TYPER) as *const u32) };
            first = (typer >> 16) & 0x3FF;
            n = typer & 0x3FF;
        }
        if n > 0 && first >= 32 && first + n <= count {
            msi = Some((base, first, n));
            pool = VectorPool::new(first, first + n - 1);
            for id in first..first + n {
                gic::configure_spi(id, true, m.cores.first().map_or(0, |c| c.gic_interface));
            }
        }
    }
    *STATE.lock() = Some(State {
        count,
        interface: m.cores.first().map_or(0, |c| c.gic_interface),
        msi,
        pool,
        attached: [false; WIRED_END as usize],
    });
}

/// The range of line interrupts: (first, count).
pub fn wired_range() -> (u32, u32) {
    match STATE.lock().as_ref() {
        Some(s) => (32, s.count.min(WIRED_END).saturating_sub(32)),
        None => (32, 0),
    }
}

/// True if `id` is a line interrupt that exists and isn't one of the MSI frame's.
pub fn wired_usable(id: u32) -> bool {
    STATE.lock().as_ref().is_some_and(|s| {
        id >= 32
            && id < s.count
            && id < WIRED_END
            && !s.msi.is_some_and(|(_, first, n)| id >= first && id < first + n)
    })
}

/// True if the machine has an MSI frame.
pub fn msi_available() -> bool {
    STATE.lock().as_ref().is_some_and(|s| s.msi.is_some())
}

/// Takes an interrupt ID from the MSI frame.
pub fn msi_allocate() -> Result<MsiVector, Error> {
    let mut g = STATE.lock();
    let s = g.as_mut().ok_or(Error::NotSupported)?;
    let (base, _, _) = s.msi.ok_or(Error::NotSupported)?;
    let intid = s.pool.take().ok_or(Error::OutOfMemory)?;
    gic::set_enabled(intid, true);
    Ok(MsiVector { id: MSI_BASE + intid, address: base + MSI_SETSPI_NS, data: intid })
}

/// Gives a message-signalled interrupt's ID back.
pub fn msi_free(id: u32) {
    let intid = id - MSI_BASE;
    if let Some(s) = STATE.lock().as_mut() {
        s.pool.give(intid);
    }
}

/// Gets line `id` ready (disabled).
pub fn attach(id: u32) -> Result<(), Error> {
    if id >= MSI_BASE {
        return Ok(());
    }
    let mut g = STATE.lock();
    let s = g.as_mut().ok_or(Error::NotSupported)?;
    if id < 32 || id >= s.count || id >= WIRED_END {
        return Err(Error::InvalidArgument);
    }
    gic::configure_spi(id, false, s.interface);
    s.attached[id as usize] = true;
    Ok(())
}

/// Disables line `id`.
pub fn detach(id: u32) {
    if id >= MSI_BASE {
        return;
    }
    gic::set_enabled(id, false);
    if let Some(s) = STATE.lock().as_mut() {
        s.attached[id as usize] = false;
    }
}

/// Lets interrupt `id` through.
pub fn unmask(id: u32) {
    gic::set_enabled(if id >= MSI_BASE { id - MSI_BASE } else { id }, true);
}

/// Makes interrupt `id` pending by software.
pub fn trigger(id: u32) -> Result<(), Error> {
    gic::set_pending(if id >= MSI_BASE { id - MSI_BASE } else { id });
    Ok(())
}

/// The handler for a shared interrupt `intid`: disables a line interrupt (so a level-triggered
/// one can't fire again at once), tells the GIC it's done, and passes the interrupt on.
pub fn interrupt(iar: u32, intid: u32) {
    let msi = STATE
        .lock()
        .as_ref()
        .is_some_and(|s| s.msi.is_some_and(|(_, first, n)| intid >= first && intid < first + n));
    if !msi {
        gic::set_enabled(intid, false);
    }
    gic::end(iar);
    irq::fired(if msi { MSI_BASE + intid } else { intid });
}
