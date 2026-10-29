// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The PrettyBigRock UEFI loader, for x86-64 and ARM64 (docs/design/0004-loaders-and-boot-disk.md).
//!
//! It reads the kernel from the disk it was started from, gathers what the firmware knows about
//! the machine, leaves the firmware behind (`ExitBootServices`), builds the boot information
//! and page tables, and hands over to the kernel.

#![no_std]
#![no_main]

mod efi;

use core::ffi::c_void;
use core::fmt::Write;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicPtr, Ordering};
use efi::*;
use pbr_boot_common::elf::Kernel;
use pbr_boot_common::handover::{Handover, jump};
use pbr_boot_common::memmap::{MapBuilder, from_uefi};
use pbr_boot_common::paging::BumpAlloc;
use pbr_boot_common::tables::build_standard;
use pbr_boot_common::{DIRECT_MAP_BASE, PAGE, align_up, fdt};
#[cfg(target_arch = "aarch64")]
use pbr_boot_common::{IdentityMapped, acpi};
#[cfg(target_arch = "aarch64")]
use pbr_bootinfo::FLAG_EL2_STUB;
use pbr_bootinfo::{
    BootInfo, EarlyConsole, Framebuffer, LoaderKind, MAX_FRAMEBUFFERS, MemoryKind, MemoryRegion,
    PhysRange, PixelFormat,
};

#[cfg(target_arch = "x86_64")]
mod arch {
    pub const NAME: &str = "x86_64";
    pub const MACHINE: u16 = pbr_boot_common::elf::EM_X86_64;
    pub const FORMAT: pbr_boot_common::paging::Format = pbr_boot_common::paging::Format::X86_64;
}
#[cfg(target_arch = "aarch64")]
mod arch {
    pub const NAME: &str = "aarch64";
    pub const MACHINE: u16 = pbr_boot_common::elf::EM_AARCH64;
    pub const FORMAT: pbr_boot_common::paging::Format = pbr_boot_common::paging::Format::Aarch64;
}

const STACK_SIZE: u64 = 128 * 1024;

/// The system table, while boot services are still available (null afterwards).
static SYSTEM: AtomicPtr<SystemTable> = AtomicPtr::new(null_mut());

/// Text output through the firmware's console.
struct Con;

impl Write for Con {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let st = SYSTEM.load(Ordering::Relaxed);
        if st.is_null() {
            return Ok(());
        }
        let mut buf = [0u16; 128];
        let mut n = 0;
        let flush = |buf: &mut [u16; 128], n: &mut usize| {
            buf[*n] = 0;
            // SAFETY: boot services are still running (SYSTEM isn't null), and the string is
            // zero-terminated.
            unsafe { ((*(*st).con_out).output_string)((*st).con_out, buf.as_ptr()) };
            *n = 0;
        };
        for c in s.chars() {
            if c == '\n' {
                buf[n] = u16::from(b'\r');
                n += 1;
            }
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                buf[n] = *u;
                n += 1;
            }
            if n >= 120 {
                flush(&mut buf, &mut n);
            }
        }
        flush(&mut buf, &mut n);
        Ok(())
    }
}

macro_rules! say {
    ($($a:tt)*) => { let _ = writeln!(Con, $($a)*); };
}

/// Stops with a message on the firmware console.
fn fail(what: &str) -> ! {
    say!("PrettyBigRock can't start: {what}");
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    say!("The PrettyBigRock loader stopped because of a bug: {info}");
    loop {
        core::hint::spin_loop();
    }
}

/// Writes `s` as a zero-terminated UTF-16 string into `buf`.
fn utf16<'a>(s: &str, buf: &'a mut [u16; 64]) -> &'a [u16; 64] {
    let mut n = 0;
    for u in s.encode_utf16().take(63) {
        buf[n] = u;
        n += 1;
    }
    buf[n] = 0;
    buf
}

struct Firmware {
    image: Handle,
    st: *mut SystemTable,
    bs: *mut BootServices,
}

impl Firmware {
    fn alloc_pages(&self, bytes: u64) -> u64 {
        let mut addr = 0u64;
        let pages = (align_up(bytes, PAGE) / PAGE) as usize;
        // SAFETY: boot services are running; `addr` receives the result.
        let s = unsafe {
            ((*self.bs).allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut addr)
        };
        if is_error(s) {
            fail("out of memory");
        }
        // SAFETY: the firmware just gave us these pages.
        unsafe { core::ptr::write_bytes(addr as *mut u8, 0, pages * PAGE as usize) };
        addr
    }

    fn protocol<T>(&self, handle: Handle, guid: &Guid) -> Option<*mut T> {
        let mut p: *mut c_void = null_mut();
        // SAFETY: boot services are running.
        let s = unsafe { ((*self.bs).handle_protocol)(handle, guid, &mut p) };
        (!is_error(s) && !p.is_null()).then_some(p.cast())
    }

    fn locate<T>(&self, guid: &Guid) -> Option<*mut T> {
        let mut p: *mut c_void = null_mut();
        // SAFETY: boot services are running.
        let s = unsafe { ((*self.bs).locate_protocol)(guid, null_mut(), &mut p) };
        (!is_error(s) && !p.is_null()).then_some(p.cast())
    }

    fn config_table(&self, guid: &Guid) -> Option<u64> {
        // SAFETY: the system table's configuration table has `number_of_table_entries` entries.
        let tables = unsafe {
            core::slice::from_raw_parts(
                (*self.st).configuration_table,
                (*self.st).number_of_table_entries,
            )
        };
        tables.iter().find(|t| t.guid == *guid).map(|t| t.table as u64)
    }

    /// The volume this loader was started from.
    fn boot_volume(&self) -> *mut File {
        let li: *mut LoadedImage =
            self.protocol(self.image, &LOADED_IMAGE).unwrap_or_else(|| fail("no loaded image"));
        // SAFETY: `li` is the firmware's loaded-image protocol for us.
        let dev = unsafe { (*li).device_handle };
        let fs: *mut SimpleFileSystem = self
            .protocol(dev, &SIMPLE_FILE_SYSTEM)
            .unwrap_or_else(|| fail("can't read the boot disk"));
        let mut root = null_mut();
        // SAFETY: `fs` is a valid file system protocol.
        if is_error(unsafe { ((*fs).open_volume)(fs, &mut root) }) {
            fail("can't open the boot disk");
        }
        root
    }

    /// Reads a whole file into newly allocated pages. Returns its physical address and size.
    fn read_file(&self, root: *mut File, path: &str) -> Option<(u64, u64)> {
        let mut name = [0u16; 64];
        let mut f = null_mut();
        // SAFETY: `root` is an open directory and `name` a zero-terminated path.
        if is_error(unsafe {
            ((*root).open)(root, &mut f, utf16(path, &mut name).as_ptr(), FILE_MODE_READ, 0)
        }) {
            return None;
        }
        let mut info = [0u64; 64];
        let mut len = core::mem::size_of_val(&info);
        // SAFETY: `f` is open; FILE_INFO's second field is the file's size.
        let size = unsafe {
            if is_error(((*f).get_info)(f, &FILE_INFO, &mut len, info.as_mut_ptr().cast())) {
                return None;
            }
            info[1]
        };
        let buf = self.alloc_pages(size.max(1));
        let mut n = size as usize;
        // SAFETY: `buf` has room for `size` bytes.
        let s = unsafe { ((*f).read)(f, &mut n, buf as *mut u8) };
        // SAFETY: `f` is open.
        unsafe { ((*f).close)(f) };
        (!is_error(s) && n as u64 == size).then_some((buf, size))
    }

    fn framebuffers(&self, out: &mut [Framebuffer; MAX_FRAMEBUFFERS]) -> u32 {
        let mut count = 0usize;
        let mut handles: *mut Handle = null_mut();
        // SAFETY: boot services are running.
        let s = unsafe {
            ((*self.bs).locate_handle_buffer)(
                BY_PROTOCOL,
                &GRAPHICS_OUTPUT,
                null_mut(),
                &mut count,
                &mut handles,
            )
        };
        if is_error(s) {
            return 0;
        }
        let mut n = 0;
        for i in 0..count {
            // SAFETY: the firmware returned `count` handles.
            let h = unsafe { *handles.add(i) };
            let Some(gop) = self.protocol::<GraphicsOutput>(h, &GRAPHICS_OUTPUT) else { continue };
            // SAFETY: a graphics output protocol always has a current mode.
            let (mode, info) = unsafe { (&*(*gop).mode, &*(*(*gop).mode).info) };
            let format = match info.pixel_format {
                0 => PixelFormat {
                    bits_per_pixel: 32,
                    red_shift: 0,
                    red_size: 8,
                    green_shift: 8,
                    green_size: 8,
                    blue_shift: 16,
                    blue_size: 8,
                    reserved: 0,
                },
                1 => PixelFormat {
                    bits_per_pixel: 32,
                    red_shift: 16,
                    red_size: 8,
                    green_shift: 8,
                    green_size: 8,
                    blue_shift: 0,
                    blue_size: 8,
                    reserved: 0,
                },
                2 => {
                    let ch = |m: u32| (m.trailing_zeros() as u8, m.count_ones() as u8);
                    let ((rs, rz), (gs, gz), (bs, bz)) =
                        (ch(info.red_mask), ch(info.green_mask), ch(info.blue_mask));
                    let all = info.red_mask | info.green_mask | info.blue_mask | info.reserved_mask;
                    let bits = (32 - all.leading_zeros()).div_ceil(8) as u8 * 8;
                    PixelFormat {
                        bits_per_pixel: bits,
                        red_shift: rs,
                        red_size: rz,
                        green_shift: gs,
                        green_size: gz,
                        blue_shift: bs,
                        blue_size: bz,
                        reserved: 0,
                    }
                }
                _ => continue, // no framebuffer: drawing only through the firmware
            };
            if mode.frame_buffer_base == 0
                || out[..n].iter().any(|f| f.memory.base == mode.frame_buffer_base)
            {
                continue;
            }
            if n == MAX_FRAMEBUFFERS {
                break;
            }
            out[n] = Framebuffer {
                memory: PhysRange {
                    base: mode.frame_buffer_base,
                    len: mode.frame_buffer_size as u64,
                },
                width: info.horizontal_resolution,
                height: info.vertical_resolution,
                stride: info.pixels_per_scan_line * u32::from(format.bits_per_pixel / 8),
                format,
            };
            n += 1;
        }
        n as u32
    }

    fn random(&self, seed: &mut [u8; 32]) {
        if let Some(rng) = self.locate::<Rng>(&RNG) {
            // SAFETY: `rng` is the firmware's RNG protocol; `seed` has room for 32 bytes.
            if !is_error(unsafe { ((*rng).get_rng)(rng, null(), 32, seed.as_mut_ptr()) }) {}
        }
        #[cfg(target_arch = "x86_64")]
        if core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0 {
            for chunk in seed.chunks_mut(8) {
                let mut v = 0u64;
                for _ in 0..10 {
                    // SAFETY: CPUID said RDRAND exists.
                    if unsafe { core::arch::x86_64::_rdrand64_step(&mut v) } == 1 {
                        break;
                    }
                }
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
    }

    fn time_ns(&self) -> u64 {
        let mut t: Time = Time {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            pad1: 0,
            nanosecond: 0,
            time_zone: 0,
            daylight: 0,
            pad2: 0,
        };
        // SAFETY: runtime services are always available.
        if is_error(unsafe { ((*(*self.st).runtime_services).get_time)(&mut t, null_mut()) })
            || t.year < 1970
        {
            return 0;
        }
        let days = days_from_civil(i64::from(t.year), u32::from(t.month), u32::from(t.day));
        // The clock's time zone is ignored: almost every firmware leaves it "unspecified", and
        // PCs that also run Windows usually keep local time in the clock anyway. The kernel will
        // correct the time from the network later.
        let secs = days * 86_400
            + i64::from(t.hour) * 3600
            + i64::from(t.minute) * 60
            + i64::from(t.second);
        (secs.max(0) as u64) * 1_000_000_000 + u64::from(t.nanosecond)
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(target_arch = "aarch64")]
fn arm_console(rsdp: u64, dtb: PhysRange) -> EarlyConsole {
    let mem = IdentityMapped;
    if rsdp != 0
        && let Some(c) =
            acpi::find_table(&mem, rsdp, b"SPCR").and_then(|t| acpi::spcr_console(&mem, t))
    {
        return c;
    }
    if !dtb.is_none() {
        // SAFETY: the firmware's device tree, `len` bytes long (from its own header).
        let blob = unsafe { core::slice::from_raw_parts(dtb.base as *const u8, dtb.len as usize) };
        if let Some(a) = fdt::find_compatible(blob, "arm,pl011") {
            return EarlyConsole { kind: 3, register_width: 4, address: a };
        }
    }
    EarlyConsole::default()
}

#[unsafe(no_mangle)]
extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    SYSTEM.store(st, Ordering::Relaxed);
    // SAFETY: the firmware passes a valid system table.
    let fw = Firmware { image, st, bs: unsafe { (*st).boot_services } };
    // SAFETY: boot services are running. Turn off the five-minute watchdog.
    unsafe { ((*fw.bs).set_watchdog_timer)(0, 0, 0, null()) };
    say!("PrettyBigRock loader (UEFI, {})", arch::NAME);

    // The kernel and command line.
    let root = fw.boot_volume();
    let mut path = [0u8; 40];
    let kpath = {
        let mut w = Buf(&mut path, 0);
        let _ = write!(w, "\\PBR\\{}\\kernel.elf", arch::NAME);
        let n = w.1;
        core::str::from_utf8(&path[..n]).unwrap_or("")
    };
    let (kfile, ksize) =
        fw.read_file(root, kpath).unwrap_or_else(|| fail("the kernel file is missing"));
    // SAFETY: `read_file` filled `ksize` bytes at `kfile`.
    let kbytes = unsafe { core::slice::from_raw_parts(kfile as *const u8, ksize as usize) };
    let kernel =
        Kernel::parse(kbytes, arch::MACHINE).unwrap_or_else(|_| fail("the kernel file is damaged"));
    let (lo, hi) = kernel.span();
    let kernel_phys = fw.alloc_pages(hi - lo);
    // SAFETY: just allocated, `hi - lo` bytes.
    let kmem =
        unsafe { core::slice::from_raw_parts_mut(kernel_phys as *mut u8, (hi - lo) as usize) };
    if kernel.load_into(kmem).is_err() {
        fail("the kernel file is damaged");
    }
    let cmdline = fw.read_file(root, "\\PBR\\cmdline.txt");

    // What the firmware knows.
    let mut info = BootInfo::new(LoaderKind::Uefi);
    info.kernel_physical = PhysRange { base: kernel_phys, len: hi - lo };
    info.kernel_virtual_base = lo;
    info.direct_map_base = DIRECT_MAP_BASE;
    info.acpi_rsdp =
        fw.config_table(&ACPI_20_TABLE).or_else(|| fw.config_table(&ACPI_TABLE)).unwrap_or(0);
    info.smbios =
        fw.config_table(&SMBIOS3_TABLE).or_else(|| fw.config_table(&SMBIOS_TABLE)).unwrap_or(0);
    if let Some(dtb) = fw.config_table(&DEVICE_TREE) {
        // SAFETY: a device tree's header is at least 8 bytes.
        let hdr = unsafe { core::slice::from_raw_parts(dtb as *const u8, 8) };
        if let Some(len) = fdt::total_size(hdr) {
            info.device_tree = PhysRange { base: dtb, len: u64::from(len) };
        }
    }
    if let Some((base, len)) = cmdline {
        info.command_line = PhysRange { base, len };
    }
    info.framebuffer_count = fw.framebuffers(&mut info.framebuffers);
    fw.random(&mut info.random_seed);
    info.boot_time_ns = fw.time_ns();
    #[cfg(target_arch = "x86_64")]
    {
        info.early_console = EarlyConsole { kind: 1, register_width: 1, address: 0x3F8 };
        info.boot_cpu_id = u64::from(core::arch::x86_64::__cpuid(1).ebx >> 24);
    }
    #[cfg(target_arch = "aarch64")]
    {
        info.early_console = arm_console(info.acpi_rsdp, info.device_tree);
        let mpidr: u64;
        // SAFETY: reading MPIDR_EL1 has no side effects.
        unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack)) };
        info.boot_cpu_id = mpidr & 0xFF_00FF_FFFF;
    }

    // Everything the hand-over needs must be allocated now: nothing can be after
    // ExitBootServices. First, how big is the memory map?
    let (mut size, mut key, mut dsize, mut dver) = (0usize, 0usize, 0usize, 0u32);
    // SAFETY: asking for the size only.
    unsafe { ((*fw.bs).get_memory_map)(&mut size, null_mut(), &mut key, &mut dsize, &mut dver) };
    let entries = size / dsize.max(1) + 64;
    let map_buf = fw.alloc_pages((entries * dsize) as u64);
    let regions_cap = entries * 2 + 64;
    let regions = fw.alloc_pages((regions_cap * 24) as u64);
    let boot_info = fw.alloc_pages(PAGE);
    let stack = fw.alloc_pages(STACK_SIZE);
    #[cfg(target_arch = "aarch64")]
    let el2_stub = fw.alloc_pages(PAGE);
    #[cfg(not(target_arch = "aarch64"))]
    let el2_stub = 0u64;
    // Page tables: the identity and direct maps each need about one table per GiB of address
    // space (2 MiB pages), plus up to one 4 KiB table at each end of each region.
    let mut top = 0u64;
    {
        let mut n = entries * dsize;
        // SAFETY: `map_buf` has room for `n` bytes.
        unsafe {
            ((*fw.bs).get_memory_map)(&mut n, map_buf as *mut u8, &mut key, &mut dsize, &mut dver)
        };
        for i in 0..n / dsize {
            // SAFETY: the firmware wrote `n / dsize` descriptors of `dsize` bytes each.
            let d = unsafe { &*((map_buf as usize + i * dsize) as *const MemoryDescriptor) };
            top = top.max(d.physical_start + d.number_of_pages * PAGE);
        }
        for f in &info.framebuffers[..info.framebuffer_count as usize] {
            top = top.max(f.memory.base + f.memory.len);
        }
    }
    let table_pages = 2 * ((top >> 30) + 4) + 4 * entries as u64 + 64;
    let tables = fw.alloc_pages(table_pages * PAGE);

    say!(
        "Starting PrettyBigRock: kernel {} KiB, {} screen(s), {} MiB of address space",
        (hi - lo) >> 10,
        info.framebuffer_count,
        top >> 20
    );

    // Leave the firmware behind. If the map changed since we read it, read it again (into the
    // same buffer: no allocating now) and retry.
    let mut n;
    let mut tries = 0;
    loop {
        n = entries * dsize;
        // SAFETY: `map_buf` has room for `n` bytes.
        let s = unsafe {
            ((*fw.bs).get_memory_map)(&mut n, map_buf as *mut u8, &mut key, &mut dsize, &mut dver)
        };
        if is_error(s) {
            fail("can't read the memory map");
        }
        // SAFETY: `key` is from the map just read.
        if !is_error(unsafe { ((*fw.bs).exit_boot_services)(image, key) }) {
            break;
        }
        tries += 1;
        if tries > 3 {
            fail("the firmware won't let go");
        }
    }
    SYSTEM.store(null_mut(), Ordering::Relaxed); // no more firmware console

    // The memory map, in note 0003's terms.
    // SAFETY: `regions` has room for `regions_cap` regions, and nothing else uses it.
    let rbuf =
        unsafe { core::slice::from_raw_parts_mut(regions as *mut MemoryRegion, regions_cap) };
    let mut mb = MapBuilder::new(rbuf);
    let mut ok = true;
    for i in 0..n / dsize {
        // SAFETY: as above.
        let d = unsafe { &*((map_buf as usize + i * dsize) as *const MemoryDescriptor) };
        ok &= mb.set(d.physical_start, d.number_of_pages * PAGE, from_uefi(d.kind)).is_ok();
    }
    ok &= mb.set(kernel_phys, hi - lo, MemoryKind::Kernel).is_ok();
    for f in &info.framebuffers[..info.framebuffer_count as usize] {
        ok &= mb.set(f.memory.base, align_up(f.memory.len, PAGE), MemoryKind::Framebuffer).is_ok();
    }
    if matches!(info.early_console.kind, 2 | 3) {
        let a = info.early_console.address & !(PAGE - 1);
        ok &= mb.set(a, PAGE, MemoryKind::DeviceMemory).is_ok();
    }
    if el2_stub != 0 {
        ok &= mb.set(el2_stub, PAGE, MemoryKind::Reserved).is_ok();
    }
    let map = mb.finish();
    if !ok {
        halt(); // can't happen: the buffer has room for twice the firmware's map
    }
    info.memory_map = PhysRange { base: regions, len: (map.len() * 24) as u64 };

    // Page tables.
    // SAFETY: `tables` is ours, identity-mapped, and `table_pages` pages long.
    let mut fa = unsafe { BumpAlloc::new(tables, table_pages * PAGE) };
    let Ok(pt) = build_standard(&mut fa, arch::FORMAT, &kernel, kernel_phys, map) else { halt() };

    #[cfg(target_arch = "aarch64")]
    if pbr_boot_common::handover::current_el() == 2 {
        info.flags |= FLAG_EL2_STUB;
    }
    // SAFETY: `boot_info` is a page of our own.
    unsafe { (boot_info as *mut BootInfo).write(info) };

    let h = Handover {
        entry: kernel.entry,
        boot_info,
        direct_map_base: DIRECT_MAP_BASE,
        stack_top: DIRECT_MAP_BASE + stack + STACK_SIZE,
        upper_root: pt.upper.root,
        lower_root: pt.lower_root,
        el2_stub_page: el2_stub,
    };
    // SAFETY: the page tables map everything note 0003 requires, and the firmware is gone.
    unsafe { jump(&h) }
}

/// Stops after the firmware has gone, when there's no way left to say why.
fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Formats into a byte buffer.
struct Buf<'a>(&'a mut [u8], usize);

impl Write for Buf<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let end = (self.1 + b.len()).min(self.0.len());
        self.0[self.1..end].copy_from_slice(&b[..end - self.1]);
        self.1 = end;
        Ok(())
    }
}
