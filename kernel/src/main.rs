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
//! Status: milestone M3, capabilities, messages and the first programs (notes 0006 to 0009),
//! on top of the kernel core (note 0005). The kernel starts the root program from the start-up
//! archive; with `selftest` on its command line, it tests itself first, and the root program
//! then tests capabilities and messages from inside running programs.

#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod console;
mod hw;
mod mem;
mod obj;
mod percpu;
mod rng;
mod root;
mod sched;
mod selftest;
mod sync;
mod syscall;
mod syspage;
mod time;
mod user;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use pbr_bootinfo::{BootInfo, Framebuffer, LoaderKind, MemoryKind, MemoryRegion};
use sync::Once;

#[global_allocator]
static HEAP: mem::heap::Heap = mem::heap::Heap;

/// The kernel's version, from the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The line the automatic boot tests wait for (`cargo xtask qemu-test`).
pub const REPORT_DONE: &str = "Start-up report complete.";

/// What the kernel keeps from the boot information once the loader's memory is reused.
struct Boot {
    map: Vec<MemoryRegion>,
    command_line: String,
    framebuffers: Vec<Framebuffer>,
    loader: LoaderKind,
    flags: u32,
    acpi_rsdp: u64,
    device_tree: bool,
    smbios: u64,
    boot_time_ns: u64,
    seeded: bool,
    /// The start-up archive (note 0009), copied before the loader's memory is reused.
    startup: Option<Vec<u8>>,
}

static BOOT: Once<Boot> = Once::new();
static MACHINE: Once<hw::Machine> = Once::new();
static KERNEL_PHYS: AtomicU64 = AtomicU64::new(0);
static KERNEL_VIRT: AtomicU64 = AtomicU64::new(0);
static KERNEL_LEN: AtomicU64 = AtomicU64::new(0);
static PANICKING: AtomicBool = AtomicBool::new(false);
static CORES_STARTED: AtomicU64 = AtomicU64::new(0);

/// Where the kernel image is: (physical address, virtual address, length).
pub fn kernel_image() -> (u64, u64, u64) {
    (
        KERNEL_PHYS.load(Ordering::Relaxed),
        KERNEL_VIRT.load(Ordering::Relaxed),
        KERNEL_LEN.load(Ordering::Relaxed),
    )
}

/// What the kernel learnt about the machine at start-up.
pub fn hw_machine() -> &'static hw::Machine {
    MACHINE.get().expect("the machine hasn't been described yet")
}

/// True once some core has panicked.
pub fn panicking() -> bool {
    PANICKING.load(Ordering::Relaxed)
}

/// Where every architecture's start-up code goes: the first architecture-independent code that
/// runs. The loader's promises about the machine at this point are in
/// `docs/design/0003-boot-information.md`; what happens next is in note 0005, section 1.
pub fn kernel_main(boot_info: u64, direct_map_base: u64) -> ! {
    arch::zero_percpu();
    mem::set_direct_map(direct_map_base);
    // SAFETY: note 0003 promises `boot_info` is the physical address of a BootInfo, reachable
    // through the direct map, and kept until the kernel is done with it.
    let info = unsafe { &*mem::phys_ptr::<BootInfo>(boot_info) };
    if info.validate().is_err() {
        arch::halt_forever(); // without valid boot information there's no console to complain on
    }
    // SAFETY: called once, here, with the loader's boot information.
    unsafe { console::init(info, direct_map_base) };
    // SAFETY: the memory map holds `len / 24` regions, in LoaderReclaimable memory (note 0003).
    let map = unsafe {
        core::slice::from_raw_parts(
            mem::phys_ptr::<MemoryRegion>(info.memory_map.base),
            (info.memory_map.len / 24) as usize,
        )
    };

    // Physical memory, then the heap, then copies of what's still needed from the loader.
    mem::pmm::init(map);
    let command_line = if info.command_line.is_none() {
        String::new()
    } else {
        // SAFETY: the command line is text in LoaderReclaimable memory (note 0003).
        let b = unsafe {
            core::slice::from_raw_parts(
                mem::phys_ptr::<u8>(info.command_line.base),
                info.command_line.len as usize,
            )
        };
        String::from_utf8_lossy(b).trim().into()
    };
    rng::seed(&info.random_seed);
    let mut startup = None;
    if !info.modules.is_none() {
        // SAFETY: the loader's array of modules, in LoaderReclaimable memory (note 0003).
        let mods = unsafe {
            core::slice::from_raw_parts(
                mem::phys_ptr::<pbr_bootinfo::Module>(info.modules.base),
                (info.modules.len / 64) as usize,
            )
        };
        if let Some(m) = mods.iter().find(|m| m.name() == Some("startup")) {
            // SAFETY: the module's contents, in memory the loader set aside (note 0003).
            let bytes = unsafe {
                core::slice::from_raw_parts(mem::phys_ptr::<u8>(m.data.base), m.data.len as usize)
            };
            startup = Some(bytes.to_vec());
        }
    }
    BOOT.set(Boot {
        map: map.to_vec(),
        command_line,
        framebuffers: info.framebuffers().to_vec(),
        loader: info.loader(),
        flags: info.flags,
        acpi_rsdp: info.acpi_rsdp,
        device_tree: !info.device_tree.is_none(),
        smbios: info.smbios,
        boot_time_ns: info.boot_time_ns,
        seeded: info.random_seed.iter().any(|&b| b != 0),
        startup,
    });
    KERNEL_PHYS.store(info.kernel_physical.base, Ordering::Relaxed);
    KERNEL_VIRT.store(info.kernel_virtual_base, Ordering::Relaxed);
    KERNEL_LEN.store(info.kernel_physical.len, Ordering::Relaxed);

    // The kernel's own page tables, and its own stack.
    let boot = BOOT.get().expect("set just now");
    mem::vm::init(&boot.map, info.kernel_physical.base, info.kernel_virtual_base);
    let stack = mem::vm::alloc_stack(32).expect("out of memory for the boot stack");
    arch::switch_stack(stack, continue_start_up)
}

/// The rest of start-up, on the kernel's own stack and page tables.
extern "C" fn continue_start_up() -> ! {
    let boot = BOOT.get().expect("boot information copied");
    // Nothing of the loader's is in use any more.
    for r in boot.map.iter().filter(|r| r.kind == MemoryKind::LoaderReclaimable as u32) {
        mem::pmm::add(r.range.base, r.range.len);
    }

    MACHINE.set(hw::discover(boot.acpi_rsdp, arch::this_core_hw_id()));
    let m = hw_machine();
    let ids: Vec<u64> = m.cores.iter().map(|c| c.hw_id).collect();
    percpu::create(&ids);
    for (cpu, core) in percpu::all().iter().zip(&m.cores) {
        arch::set_core_info(cpu, core);
    }
    percpu::install(percpu::all()[0]);
    arch::init_boot_core(m);
    sched::init_this_core();
    percpu::all()[0].online.store(true, Ordering::Release);
    arch::irq_enable();
    CORES_STARTED.store(1 + arch::start_other_cores(m) as u64, Ordering::Relaxed);

    report(boot);
    obj::process::start_reaper();
    syspage::init();
    let selftest = boot.command_line.split_whitespace().any(|w| w == "selftest");
    sched::spawn("start-up", sched::NORMAL, move || {
        if selftest {
            match selftest::run() {
                Ok(summary) => kprintln!("Self-test passed: {summary}"),
                Err(e) => kprintln!("Self-test FAILED: {e}"),
            }
        }
        start_root(selftest);
    });
    sched::idle_loop()
}

/// Starts the root program. With the self-test, waits for it (it runs the program self-test
/// and exits) before saying the start-up report is complete.
fn start_root(selftest: bool) {
    let boot = BOOT.get().expect("boot information copied");
    let Some(archive) = boot.startup.clone() else {
        kprintln!("There's no start-up archive, so there's no root program to start.");
        kprintln!("{REPORT_DONE}");
        return;
    };
    match root::start(archive, &boot.command_line) {
        Err(e) => {
            kprintln!("Couldn't start the root program: {e}");
            kprintln!("{REPORT_DONE}");
        }
        Ok(p) if selftest => {
            let done = obj::notification::Notification::new(None);
            p.watch(done.clone(), 1);
            let _ = done.wait(u64::MAX, || true);
            match p.status() {
                Some(obj::process::Status::Exited(s)) => {
                    kprintln!("The root program {} exited with status {s}.", root::describe(&p))
                }
                other => kprintln!("The root program {} ended: {other:?}.", root::describe(&p)),
            }
            kprintln!("{REPORT_DONE}");
        }
        Ok(p) => kprintln!("{REPORT_DONE} The root program {} is running.", root::describe(&p)),
    }
}

fn report(boot: &Boot) {
    kprintln!("Hello from PrettyBigRock!");
    let loader = match boot.loader {
        LoaderKind::Uefi => "UEFI",
        LoaderKind::Bios => "BIOS",
        LoaderKind::RaspberryPi => "the Raspberry Pi start-up stage",
        LoaderKind::Test => "a test harness",
        LoaderKind::Unknown => "an unknown loader",
    };
    kprintln!(
        "PrettyBigRock 2 kernel {VERSION} (milestone M3) on {}, started by {loader}.",
        arch::NAME
    );
    kprintln!();

    let mut cpu = Text::<160>::new();
    let _ = arch::describe(&mut cpu);
    kprintln!("Processor:     {}", cpu.as_str());
    kprintln!(
        "Cores:         {} found, {} running",
        percpu::count(),
        CORES_STARTED.load(Ordering::Relaxed)
    );
    let (free, total) = mem::pmm::stats();
    kprintln!(
        "Memory:        {} MiB free of {} MiB",
        (free * mem::PAGE) >> 20,
        (total * mem::PAGE) >> 20
    );
    let (kphys, _, klen) = kernel_image();
    kprintln!("Kernel:        {} KiB at {:#x}", klen >> 10, kphys);
    kprint!("Screens:       {}", boot.framebuffers.len());
    for f in &boot.framebuffers {
        kprint!(" ({}x{}, {}-bit)", f.width, f.height, f.format.bits_per_pixel);
    }
    kprintln!();
    let yes = |b: bool| if b { "yes" } else { "no" };
    kprintln!(
        "Hardware info: ACPI {}, device tree {}, SMBIOS {}",
        yes(boot.acpi_rsdp != 0),
        yes(boot.device_tree),
        yes(boot.smbios != 0)
    );
    kprintln!("Command line:  \"{}\"", boot.command_line);
    kprintln!("Randomness:    {}", if boot.seeded { "seeded" } else { "none available" });
    if boot.boot_time_ns == 0 {
        kprintln!("Time:          unknown");
    } else {
        let (y, mo, d, h, mi, s) = civil(boot.boot_time_ns / 1_000_000_000);
        kprintln!("Time:          {y}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC");
    }
    #[cfg(target_arch = "aarch64")]
    if boot.flags & pbr_bootinfo::FLAG_EL2_STUB != 0 {
        // SAFETY: the loader says the stub is there.
        unsafe { arch::ping_el2_stub() };
        kprintln!("EL2 stub:      present and answering");
    }
    let _ = boot.flags;
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

/// A small fixed-size text buffer, for formatting without the heap.
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
    arch::irq_disable();
    if !PANICKING.swap(true, Ordering::SeqCst) {
        // Stop the other cores first, so the report isn't interleaved with anything.
        if let Some(me) = percpu::try_this() {
            for c in percpu::all() {
                if c.index != me.index && c.online.load(Ordering::Relaxed) {
                    arch::send_ipi(c, arch::Ipi::Stop);
                }
            }
        }
        let core = percpu::try_this().map(|c| c.index).unwrap_or(0);
        sched::with_current_name(|name, id| {
            console::print_panic(format_args!(
                "\nThe kernel stopped because of a bug, on core {core} in thread \"{name}\" (#{id}): {info}\n"
            ))
        });
    }
    arch::halt_forever()
}
