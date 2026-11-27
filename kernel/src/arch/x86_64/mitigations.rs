// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Which of Meltdown, MDS and L1TF this processor has, and so which protections the kernel
//! turns on (docs/design/0027-kernel-1.4.md, section 1a).
//!
//! Since 2018 we know that many Intel processors, while guessing ahead ("speculative
//! execution"), briefly let a program read memory it has no right to, and that the program can
//! recover what it read from the timing of the caches:
//! - **Meltdown** reads anything the kernel has mapped, so programs get page tables that map
//!   almost nothing of the kernel (KPTI, `entry.rs`);
//! - **MDS** reads the kernel's recent data from small buffers inside the processor, so the
//!   kernel empties them on every return to a program (`verw`, with updated microcode);
//! - **L1TF** follows the address in a page-table entry marked not present, which
//!   `lib/platform` makes harmless by keeping such entries zero.
//!
//! The boot core decides once, for every core. The command line can override it:
//! `mitigations=off` turns everything off, `mitigations=on` everything on, even on a
//! processor that isn't affected (how QEMU on GitHub's AMD runners tests the code).

use super::cpu;
use crate::kprintln;
use core::sync::atomic::{AtomicU8, Ordering};

/// `IA32_ARCH_CAPABILITIES`: which flaws the processor says it doesn't have.
const IA32_ARCH_CAPABILITIES: u32 = 0x10A;
/// In it: not affected by Meltdown (and so not by L1TF).
const RDCL_NO: u64 = 1 << 0;
/// In it: not affected by MDS.
const MDS_NO: u64 = 1 << 5;

const KPTI: u8 = 1 << 0;
const MDS: u8 = 1 << 1;
const L1TF: u8 = 1 << 2;
/// Whether the command line asked: 0 auto, 1 on, 2 off.
const FORCED_ON: u8 = 1 << 6;
const FORCED_OFF: u8 = 1 << 7;

static DECIDED: AtomicU8 = AtomicU8::new(0);

/// Decides, from the processor and the command line's `mitigations=` word.
pub fn decide(command_line: &str) {
    let word = command_line.split_whitespace().rev().find_map(|w| w.strip_prefix("mitigations="));
    let affected = affected();
    let d = match word {
        Some("on") => KPTI | MDS | L1TF | FORCED_ON,
        Some("off") => FORCED_OFF,
        _ => affected,
    };
    DECIDED.store(d, Ordering::Relaxed);
}

/// What the processor itself says: the flaws Intel's processors have unless they say they
/// don't. Other vendors (AMD, Hygon, QEMU's own models) have none of the three.
fn affected() -> u8 {
    let r = core::arch::x86_64::__cpuid(0);
    if (r.ebx, r.edx, r.ecx) != (0x756E_6547, 0x4965_6E69, 0x6C65_746E) {
        return 0; // not "GenuineIntel"
    }
    let has_caps = core::arch::x86_64::__cpuid_count(7, 0).edx & (1 << 29) != 0;
    // SAFETY: CPUID says the register exists.
    let caps = if has_caps { unsafe { cpu::rdmsr(IA32_ARCH_CAPABILITIES) } } else { 0 };
    let mut a = 0;
    if caps & RDCL_NO == 0 {
        a |= KPTI | L1TF;
    }
    if caps & MDS_NO == 0 {
        a |= MDS;
    }
    a
}

/// True if programs get separate page tables.
pub fn kpti() -> bool {
    DECIDED.load(Ordering::Relaxed) & KPTI != 0
}

/// True if the processor's buffers are cleared on every return to a program.
pub fn mds() -> bool {
    DECIDED.load(Ordering::Relaxed) & MDS != 0
}

/// True if the microcode makes `verw` clear the buffers (`MD_CLEAR`).
pub fn md_clear() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).edx & (1 << 10) != 0
}

/// The start-up report's lines.
pub fn report() {
    let d = DECIDED.load(Ordering::Relaxed);
    let how = match d & (FORCED_ON | FORCED_OFF) {
        FORCED_ON => " (mitigations=on)",
        FORCED_OFF => " (mitigations=off)",
        _ => "",
    };
    if d & KPTI != 0 {
        kprintln!("Meltdown:      separate page tables (KPTI){how}");
    } else {
        kprintln!("Meltdown:      not affected, or turned off{how}");
    }
    match (d & MDS != 0, md_clear()) {
        (true, true) => kprintln!("MDS:           buffers cleared on return (VERW){how}"),
        (true, false) => {
            kprintln!("MDS:           affected, but the microcode can't clear buffers{how}")
        }
        (false, _) => kprintln!("MDS:           not affected, or turned off{how}"),
    }
    if d & L1TF != 0 {
        kprintln!("L1TF:          not-present entries are empty{how}");
    }
}
