// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PrettyBigRock BIOS loader, for x86-64 PCs without UEFI
//! (docs/design/0004-loaders-and-boot-disk.md).
//!
//! The first two stages are assembly (`stage1.s`, `stage2.s`). They gather what only the BIOS
//! can tell us, copy the payload to 1 MiB, switch to 64-bit mode and call [`bios_main`], which
//! does everything else in Rust, sharing its code with the UEFI loader.

#![no_std]
#![no_main]

use core::fmt::Write;
use pbr_boot_common::elf::{EM_X86_64, Kernel};
use pbr_boot_common::handover::{Handover, jump};
use pbr_boot_common::memmap::{MapBuilder, from_e820};
use pbr_boot_common::paging::{BumpAlloc, Format, build_standard};
use pbr_boot_common::{DIRECT_MAP_BASE, IdentityMapped, PAGE, acpi, align_up};
use pbr_bootinfo::{
    BootInfo, EarlyConsole, Framebuffer, LoaderKind, MemoryKind, MemoryRegion, PhysRange,
    PixelFormat,
};

core::arch::global_asm!(include_str!("stage1.s"));
core::arch::global_asm!(include_str!("stage2.s"));

/// Where the second stage leaves things (see the table at the top of stage2.s).
const E820_COUNT: u64 = 0x1000;
const E820_ENTRIES: u64 = 0x1008;
const VBE_MODE_INFO: u64 = 0x2200;
const VBE_CHOSEN: u64 = 0x2600;
const PAYLOAD: u64 = 0x10_0000;
/// The loader's own low memory: everything from 0x500 to the top of its stack.
const LOW_START: u64 = 0x500;
const LOW_END: u64 = 0x8_0000;
/// The temporary page tables identity-map this much; everything the loader allocates is below.
const IDENTITY_LIMIT: u64 = 2 << 30;

const STACK_SIZE: u64 = 128 * 1024;
const COM1: u16 = 0x3F8;

fn outb(port: u16, v: u8) {
    // SAFETY: only used for COM1 and the CMOS clock, which every PC has.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) };
}

fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: as for `outb`.
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) };
    v
}

/// Messages go to COM1: once the graphics mode is set, the BIOS can't show text.
struct Serial;

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.byte(b'\r');
            }
            self.byte(b);
        }
        Ok(())
    }
}

impl Serial {
    fn init() {
        for (off, v) in
            [(1, 0x00), (3, 0x80), (0, 0x01), (1, 0x00), (3, 0x03), (2, 0xC7), (4, 0x03)]
        {
            outb(COM1 + off, v);
        }
    }
    fn byte(&mut self, b: u8) {
        for _ in 0..100_000 {
            if inb(COM1 + 5) & 0x20 != 0 {
                break;
            }
        }
        outb(COM1, b);
    }
}

macro_rules! say {
    ($($a:tt)*) => { let _ = writeln!(Serial, $($a)*); };
}

fn fail(what: &str) -> ! {
    say!("PrettyBigRock can't start: {what}");
    halt()
}

fn halt() -> ! {
    loop {
        // SAFETY: stops the processor; nothing else.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    say!("The PrettyBigRock BIOS loader stopped because of a bug: {info}");
    halt()
}

fn read<T: Copy>(addr: u64) -> T {
    // SAFETY: only used on the fixed low-memory addresses the second stage filled in, and on
    // the payload it copied; all identity-mapped.
    unsafe { core::ptr::read_unaligned(addr as *const T) }
}

/// A file in the payload.
fn payload_file(name: &str) -> Option<(u64, u64)> {
    if read::<[u8; 8]>(PAYLOAD) != *b"PBRPAYLD" {
        fail("the boot disk's BIOS payload is damaged");
    }
    let count = read::<u32>(PAYLOAD + 8);
    (0..u64::from(count.min(9))).find_map(|i| {
        let e = PAYLOAD + 16 + i * 48;
        let raw = read::<[u8; 40]>(e + 8);
        let len = raw.iter().position(|&b| b == 0).unwrap_or(40);
        (&raw[..len] == name.as_bytes())
            .then(|| (PAYLOAD + u64::from(read::<u32>(e)) * 512, u64::from(read::<u32>(e + 4))))
    })
}

/// Memory for the loader's own needs: from usable memory above the payload, below the end of
/// the identity map.
struct Allocator {
    next: u64,
}

impl Allocator {
    fn alloc(&mut self, bytes: u64, e820: &[(u64, u64, u32)]) -> u64 {
        let len = align_up(bytes, PAGE);
        for &(base, size, kind) in e820 {
            if kind != 1 {
                continue;
            }
            let start = align_up(self.next.max(base), PAGE);
            let end = (base + size).min(IDENTITY_LIMIT);
            if start + len <= end {
                self.next = start + len;
                // SAFETY: usable memory, identity-mapped, past everything already in use.
                unsafe { core::ptr::write_bytes(start as *mut u8, 0, len as usize) };
                return start;
            }
        }
        fail("not enough memory")
    }
}

fn cmos(reg: u8) -> u8 {
    outb(0x70, reg);
    inb(0x71)
}

/// The time from the PC's clock, in nanoseconds since 1970, assuming the clock keeps UTC.
fn clock_ns() -> u64 {
    for _ in 0..1_000_000 {
        if cmos(0x0A) & 0x80 == 0 {
            break; // not in the middle of an update
        }
    }
    let status_b = cmos(0x0B);
    let bcd = |v: u8| if status_b & 4 == 0 { (v & 0x0F) + (v >> 4) * 10 } else { v };
    let (s, mi) = (bcd(cmos(0)), bcd(cmos(2)));
    let raw_h = cmos(4);
    let mut h = bcd(raw_h & 0x7F);
    if status_b & 2 == 0 && raw_h & 0x80 != 0 {
        h = (h % 12) + 12; // 12-hour clock, afternoon
    }
    let (d, mo, y) = (bcd(cmos(7)), bcd(cmos(8)), bcd(cmos(9)));
    let century = bcd(cmos(0x32));
    let year = if (19..=21).contains(&century) {
        u32::from(century) * 100 + u32::from(y)
    } else {
        2000 + u32::from(y)
    };
    if !(1..=12).contains(&mo) || d == 0 {
        return 0;
    }
    let (yy, m) = (i64::from(if mo <= 2 { year - 1 } else { year }), i64::from(mo));
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + i64::from(d) - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let secs = days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s);
    secs.max(0) as u64 * 1_000_000_000
}

fn find_smbios() -> u64 {
    let mut a = 0xF0000;
    while a < 0x10_0000 {
        let sig = read::<[u8; 5]>(a);
        if &sig == b"_SM3_" || &sig[..4] == b"_SM_" {
            return a;
        }
        a += 16;
    }
    0
}

/// Called by the second stage in 64-bit mode, with the first 2 GiB identity-mapped.
#[unsafe(no_mangle)]
pub extern "C" fn bios_main() -> ! {
    Serial::init();
    say!("PrettyBigRock loader (BIOS, x86_64)");

    // The memory map the BIOS gave.
    let n = usize::from(read::<u16>(E820_COUNT)).min(120);
    let mut e820 = [(0u64, 0u64, 0u32); 120];
    for (i, slot) in e820.iter_mut().enumerate().take(n) {
        let at = E820_ENTRIES + i as u64 * 24;
        *slot = (read::<u64>(at), read::<u64>(at + 8), read::<u32>(at + 16));
    }
    let e820 = &mut e820[..n];
    e820.sort_unstable_by_key(|e| e.0);

    // The kernel.
    let (kaddr, klen) = payload_file("kernel.elf").unwrap_or_else(|| fail("the kernel is missing"));
    // SAFETY: the second stage copied the payload here.
    let kbytes = unsafe { core::slice::from_raw_parts(kaddr as *const u8, klen as usize) };
    let kernel =
        Kernel::parse(kbytes, EM_X86_64).unwrap_or_else(|_| fail("the kernel file is damaged"));
    let payload_end = PAYLOAD + u64::from(read::<u32>(PAYLOAD + 12)) * 512;
    let mut mem = Allocator { next: align_up(payload_end, PAGE) };
    let (lo, hi) = kernel.span();
    let kernel_phys = mem.alloc(hi - lo, e820);
    // SAFETY: just allocated, `hi - lo` bytes.
    let kmem =
        unsafe { core::slice::from_raw_parts_mut(kernel_phys as *mut u8, (hi - lo) as usize) };
    kernel.load_into(kmem).unwrap_or_else(|_| fail("the kernel file is damaged"));

    let mut info = BootInfo::new(LoaderKind::Bios);
    info.kernel_physical = PhysRange { base: kernel_phys, len: hi - lo };
    info.kernel_virtual_base = lo;
    info.direct_map_base = DIRECT_MAP_BASE;
    if let Some((base, len)) = payload_file("cmdline.txt") {
        info.command_line = PhysRange { base, len };
    }
    info.acpi_rsdp = acpi::find_rsdp_bios(&IdentityMapped).unwrap_or(0);
    info.smbios = find_smbios();
    info.early_console = EarlyConsole { kind: 1, register_width: 1, address: u64::from(COM1) };
    info.boot_time_ns = clock_ns();
    info.boot_cpu_id = u64::from(core::arch::x86_64::__cpuid(1).ebx >> 24);
    if core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0 {
        for chunk in info.random_seed.chunks_mut(8) {
            let mut v = 0u64;
            // SAFETY: CPUID said RDRAND exists.
            unsafe { core::arch::x86_64::_rdrand64_step(&mut v) };
            chunk.copy_from_slice(&v.to_le_bytes());
        }
    }

    // The screen the second stage set up, if any.
    if read::<u16>(VBE_CHOSEN) != 0 {
        let m = VBE_MODE_INFO;
        let version_3 = read::<u16>(0x2004) >= 0x300;
        let stride = u32::from(if version_3 { read::<u16>(m + 50) } else { read::<u16>(m + 16) });
        let height = u32::from(read::<u16>(m + 20));
        info.framebuffers[0] = Framebuffer {
            memory: PhysRange {
                base: u64::from(read::<u32>(m + 40)),
                len: u64::from(stride * height),
            },
            width: u32::from(read::<u16>(m + 18)),
            height,
            stride,
            format: PixelFormat {
                bits_per_pixel: read::<u8>(m + 25),
                red_size: read::<u8>(m + 31),
                red_shift: read::<u8>(m + 32),
                green_size: read::<u8>(m + 33),
                green_shift: read::<u8>(m + 34),
                blue_size: read::<u8>(m + 35),
                blue_shift: read::<u8>(m + 36),
                reserved: 0,
            },
        };
        info.framebuffer_count = 1;
    }

    // Memory for the hand-over.
    let top = e820
        .iter()
        .map(|e| e.0 + e.1)
        .max()
        .unwrap_or(0)
        .max(info.framebuffers[0].memory.base + info.framebuffers[0].memory.len);
    let table_pages = 2 * ((top >> 30) + 4) + 4 * n as u64 + 64;
    let tables = mem.alloc(table_pages * PAGE, e820);
    let regions_cap = 2 * n + 64;
    let regions = mem.alloc(regions_cap as u64 * 24, e820);
    let boot_info = mem.alloc(PAGE, e820);
    let stack = mem.alloc(STACK_SIZE, e820);
    let used_end = mem.next;

    say!(
        "Starting PrettyBigRock: kernel {} KiB, {} screen(s), {} MiB of address space",
        (hi - lo) >> 10,
        info.framebuffer_count,
        top >> 20
    );

    // The memory map, in note 0003's terms.
    // SAFETY: `regions` has room for `regions_cap` regions, and nothing else uses it.
    let rbuf =
        unsafe { core::slice::from_raw_parts_mut(regions as *mut MemoryRegion, regions_cap) };
    let mut mb = MapBuilder::new(rbuf);
    let mut ok = true;
    for &(base, len, kind) in e820.iter() {
        ok &= mb.set(base, len, from_e820(kind)).is_ok();
    }
    ok &= mb.set(0, LOW_START, MemoryKind::Reserved).is_ok(); // interrupt table and BIOS data
    ok &= mb.set(LOW_START, LOW_END - LOW_START, MemoryKind::LoaderReclaimable).is_ok();
    ok &= mb.set(PAYLOAD, used_end - PAYLOAD, MemoryKind::LoaderReclaimable).is_ok();
    ok &= mb.set(kernel_phys, hi - lo, MemoryKind::Kernel).is_ok();
    if info.framebuffer_count == 1 {
        let f = info.framebuffers[0].memory;
        ok &= mb.set(f.base, align_up(f.len, PAGE), MemoryKind::Framebuffer).is_ok();
    }
    let map = mb.finish();
    if !ok {
        fail("the memory map is too complicated");
    }
    info.memory_map = PhysRange { base: regions, len: (map.len() * 24) as u64 };

    // SAFETY: `tables` is ours, identity-mapped, `table_pages` pages long.
    let mut fa = unsafe { BumpAlloc::new(tables, table_pages * PAGE) };
    let pt = build_standard(&mut fa, Format::X86_64, &kernel, kernel_phys, map)
        .unwrap_or_else(|_| fail("couldn't build the page tables"));
    // SAFETY: `boot_info` is a page of our own.
    unsafe { (boot_info as *mut BootInfo).write(info) };
    let h = Handover {
        entry: kernel.entry,
        boot_info,
        direct_map_base: DIRECT_MAP_BASE,
        stack_top: DIRECT_MAP_BASE + stack + STACK_SIZE,
        upper_root: pt.upper.root,
        lower_root: pt.lower_root,
        el2_stub_page: 0,
    };
    // SAFETY: the page tables map everything note 0003 requires.
    unsafe { jump(&h) }
}
