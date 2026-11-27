// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Loading Intel's microcode updates (docs/design/0027-kernel-1.4.md, section 1e).
//!
//! A processor's microcode is the firmware inside it that carries out its more complicated
//! instructions; Intel fixes flaws by publishing new microcode, which is forgotten at every
//! power-off and must be loaded again at every start. Roxy's firmware loads a revision from
//! before the fix for MDS, so the kernel loads Intel's newer one itself, if the loader found the
//! file for this processor (the start-up module `microcode`). The boot core does it before
//! anything else that depends on the processor's behaviour, and each other core as the first
//! thing it does. The processor checks Intel's signature on the update and refuses anything
//! else; the kernel's own checks (in `lib/microcode`) only keep it from passing nonsense.

use super::cpu;
use crate::kprintln;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

/// `IA32_PLATFORM_ID`: bits 50 to 52 say which platform this processor is, which an update's
/// processor flags must include.
const IA32_PLATFORM_ID: u32 = 0x17;
/// `IA32_BIOS_UPDT_TRIG`: writing an update's address here loads it.
const IA32_BIOS_UPDT_TRIG: u32 = 0x79;
/// `IA32_BIOS_SIGN_ID`: the running revision, in the high half.
const IA32_BIOS_SIGN_ID: u32 = 0x8B;

/// The update's data, 16-byte aligned as the processor requires, kept until every core has
/// loaded it; null if there is none.
static DATA: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
/// The revision the boot core ended up with.
static REVISION: AtomicU32 = AtomicU32::new(0);
/// The revision it had before, and the one it was offered, for the report.
static BEFORE: AtomicU32 = AtomicU32::new(0);
static OFFERED: AtomicU32 = AtomicU32::new(0);
/// What happened: 0 nothing (no file, or not Intel), 1 no newer update, 2 updated, 3 refused.
static OUTCOME: AtomicU32 = AtomicU32::new(0);

/// A 16-byte unit, so a boxed slice of them is 16-byte aligned. `repr(align)` raises a type's
/// alignment.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Aligned([u8; 16]);

fn intel() -> bool {
    let r = core::arch::x86_64::__cpuid(0);
    (r.ebx, r.edx, r.ecx) == (0x756E_6547, 0x4965_6E69, 0x6C65_746E) // "GenuineIntel"
}

/// The revision running on this core. The processor only fills the register in when CPUID runs
/// after it has been cleared.
fn running() -> u32 {
    // SAFETY: on Intel processors, which have both registers; writing 0 is how the revision
    // is asked for.
    unsafe {
        cpu::wrmsr(IA32_BIOS_SIGN_ID, 0);
        core::arch::x86_64::__cpuid(1);
        (cpu::rdmsr(IA32_BIOS_SIGN_ID) >> 32) as u32
    }
}

/// Loads the update at `data` on this core, and returns the revision it is running after.
fn apply(data: *const u8) -> u32 {
    // SAFETY: `data` is the update's data, 16-byte aligned, and stays allocated until every core
    // has loaded it. The processor checks it before using it.
    unsafe { cpu::wrmsr(IA32_BIOS_UPDT_TRIG, data as u64) };
    running()
}

/// The boot core: finds the update for this processor in `file` (the `microcode` module) and
/// loads it. [`report`] says what happened.
pub fn load(file: Option<&[u8]>) {
    // Only on Intel processors, and only when there is a file, since the registers it uses
    // don't exist elsewhere (QEMU's own processors among them).
    let Some(file) = file.filter(|_| intel()) else { return };
    let signature = core::arch::x86_64::__cpuid(1).eax;
    // SAFETY: Intel processors from the Pentium Pro on have the platform register.
    let platform = ((unsafe { cpu::rdmsr(IA32_PLATFORM_ID) } >> 50) & 7) as u32;
    let before = running();
    BEFORE.store(before, Ordering::Relaxed);
    REVISION.store(before, Ordering::Relaxed);
    let Some(update) = pbr_microcode::newest(file, signature, platform, before) else {
        OUTCOME.store(1, Ordering::Relaxed);
        return;
    };
    OFFERED.store(update.revision, Ordering::Relaxed);
    let mut buf: Box<[Aligned]> =
        (0..update.data.len().div_ceil(16)).map(|_| Aligned([0; 16])).collect();
    // SAFETY: `buf` has at least `update.data.len()` bytes; the two don't overlap.
    unsafe {
        core::ptr::copy_nonoverlapping(
            update.data.as_ptr(),
            buf.as_mut_ptr().cast::<u8>(),
            update.data.len(),
        )
    };
    // `into_raw` hands the allocation over as a plain pointer, so it lives on past this
    // function until `free` takes it back.
    let data = Box::into_raw(buf).cast::<u8>();
    let after = apply(data);
    REVISION.store(after, Ordering::Relaxed);
    if after == update.revision {
        LEN.store(update.data.len() as u32, Ordering::Relaxed);
        DATA.store(data, Ordering::Release);
        OUTCOME.store(2, Ordering::Relaxed);
    } else {
        OUTCOME.store(3, Ordering::Relaxed);
        free(data, update.data.len());
    }
}

/// The start-up report's line, if there was a microcode file for this processor.
pub fn report() {
    let (before, after) = (BEFORE.load(Ordering::Relaxed), REVISION.load(Ordering::Relaxed));
    match OUTCOME.load(Ordering::Relaxed) {
        1 => kprintln!("Microcode:     {before:#x} (no newer update)"),
        2 => kprintln!("Microcode:     {before:#x}, updated to {after:#x}"),
        3 => kprintln!(
            "Microcode:     {before:#x}; the update to {:#x} was refused",
            OFFERED.load(Ordering::Relaxed)
        ),
        _ => {}
    }
}

/// The update's length in bytes, for freeing it.
static LEN: AtomicU32 = AtomicU32::new(0);

fn free(data: *mut u8, len: usize) {
    let units = len.div_ceil(16);
    // SAFETY: `data` came from `Box::into_raw` of a boxed slice of `units` `Aligned`s, and is
    // freed once.
    drop(unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(data.cast::<Aligned>(), units))
    });
}

/// Another core, as the first thing it does: loads the boot core's update, if it loaded one,
/// and says if this core ends up on a different revision.
pub fn load_this_core() {
    let data = DATA.load(Ordering::Acquire);
    if data.is_null() {
        return;
    }
    let after = apply(data);
    let want = REVISION.load(Ordering::Relaxed);
    if after != want {
        kprintln!("Microcode:     a core is on revision {after:#x}, not {want:#x}");
    }
}

/// Frees the update, once every core has loaded it.
pub fn every_core_loaded() {
    let data = DATA.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !data.is_null() {
        free(data, LEN.load(Ordering::Relaxed) as usize);
    }
}
