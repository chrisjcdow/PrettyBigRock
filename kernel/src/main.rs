// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PrettyBigRock microkernel.
//!
//! The kernel does only what can't be done anywhere else: address spaces, threads and
//! scheduling, messages, capabilities, interrupts and timers (docs/DESIGN.md, section 6).
//! Everything else is an ordinary sandboxed program.
//!
//! Processor-specific code lives in [`arch`]; nothing outside it may assume which processor
//! it runs on (docs/DESIGN.md, section 10).
//!
//! Status: milestone M1. The kernel is started by a loader, checks the boot information,
//! reports what it was given on the serial port and the screen, and stops.

#![no_std]
#![no_main]

mod arch;
mod console;
mod sync;

use pbr_bootinfo::{BootInfo, LoaderKind, MemoryKind};

/// The kernel's version, from the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The line the automatic boot tests wait for (`cargo xtask qemu-test`).
pub const REPORT_DONE: &str = "Start-up report complete.";

/// Where every architecture's start-up code goes: the first architecture-independent code that
/// runs. The loader's promises about the machine at this point are in
/// `docs/design/0003-boot-information.md`.
pub fn kernel_main(boot_info: u64, direct_map_base: u64) -> ! {
    // SAFETY: note 0003 promises `boot_info` is the physical address of a BootInfo, reachable
    // through the direct map at `direct_map_base`, and kept until the kernel is done with it.
    let info = unsafe { &*((direct_map_base + boot_info) as *const BootInfo) };
    if info.validate().is_err() {
        // Without valid boot information there's no console to complain on.
        arch::halt_forever();
    }
    // SAFETY: called once, here, with the loader's boot information.
    unsafe { console::init(info, direct_map_base) };
    report(info, direct_map_base);
    kprintln!("{REPORT_DONE} Nothing else to do yet, so this core is stopping here.");
    arch::halt_forever()
}

fn report(info: &BootInfo, dmb: u64) {
    kprintln!("Hello from PrettyBigRock!");
    let loader = match info.loader() {
        LoaderKind::Uefi => "UEFI",
        LoaderKind::Bios => "BIOS",
        LoaderKind::RaspberryPi => "the Raspberry Pi start-up stage",
        LoaderKind::Test => "a test harness",
        LoaderKind::Unknown => "an unknown loader",
    };
    kprintln!(
        "PrettyBigRock 2 kernel {VERSION} (milestone M1) on {}, started by {loader}.",
        arch::NAME
    );
    kprintln!();

    let mut cpu = Text::<96>::new();
    let _ = arch::describe(&mut cpu);
    kprintln!("Processor:     {} (boot core {})", cpu.as_str(), info.boot_cpu_id);

    // SAFETY: the memory map is in LoaderReclaimable memory, reachable through the direct map,
    // and holds `len / 24` regions (note 0003).
    let map = unsafe {
        core::slice::from_raw_parts(
            (dmb + info.memory_map.base) as *const pbr_bootinfo::MemoryRegion,
            (info.memory_map.len / 24) as usize,
        )
    };
    let usable: u64 = map
        .iter()
        .filter(|r| matches!(r.kind(), Some(MemoryKind::Usable | MemoryKind::LoaderReclaimable)))
        .map(|r| r.range.len)
        .sum();
    kprintln!(
        "Memory:        {} MiB free once start-up is over, in a map of {} regions",
        usable >> 20,
        map.len()
    );
    kprintln!(
        "Kernel:        {} KiB at {:#x}",
        info.kernel_physical.len >> 10,
        info.kernel_physical.base
    );

    let fbs = info.framebuffers();
    kprint!("Screens:       {}", fbs.len());
    for f in fbs {
        kprint!(" ({}x{}, {}-bit)", f.width, f.height, f.format.bits_per_pixel);
    }
    kprintln!();

    let yes = |b: bool| if b { "yes" } else { "no" };
    kprintln!(
        "Hardware info: ACPI {}, device tree {}, SMBIOS {}",
        yes(info.acpi_rsdp != 0),
        yes(!info.device_tree.is_none()),
        yes(info.smbios != 0)
    );
    let cmdline = if info.command_line.is_none() {
        ""
    } else {
        // SAFETY: the command line is UTF-8 text in LoaderReclaimable memory (note 0003).
        let b = unsafe {
            core::slice::from_raw_parts(
                (dmb + info.command_line.base) as *const u8,
                info.command_line.len as usize,
            )
        };
        core::str::from_utf8(b).unwrap_or("(not valid UTF-8)")
    };
    kprintln!("Command line:  \"{}\"", cmdline.trim());
    kprintln!(
        "Randomness:    {}",
        if info.random_seed.iter().any(|&b| b != 0) { "seeded" } else { "none available" }
    );
    if info.boot_time_ns == 0 {
        kprintln!("Time:          unknown");
    } else {
        let (y, mo, d, h, mi, s) = civil(info.boot_time_ns / 1_000_000_000);
        kprintln!("Time:          {y}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC");
    }

    #[cfg(target_arch = "aarch64")]
    if info.flags & pbr_bootinfo::FLAG_EL2_STUB != 0 {
        // SAFETY: the loader says the stub is there.
        unsafe { arch::ping_el2_stub() };
        kprintln!("EL2 stub:      present and answering");
    }
    kprintln!();
}

/// Converts seconds since 1970 to a calendar date and time (UTC).
fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's days-to-civil algorithm, for dates from 1970 onwards.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d, rem / 3600, rem / 60 % 60, rem % 60)
}

/// A small fixed-size text buffer, for formatting without an allocator.
struct Text<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            let mut b = [0; 4];
            let e = c.encode_utf8(&mut b);
            if self.len + e.len() > N {
                return Ok(()); // silently truncate
            }
            self.buf[self.len..self.len + e.len()].copy_from_slice(e.as_bytes());
            self.len += e.len();
        }
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    kprintln!("\nThe kernel stopped because of a bug: {info}");
    arch::halt_forever()
}
