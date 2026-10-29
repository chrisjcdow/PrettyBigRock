// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The boot information that every PrettyBigRock loader hands to the kernel.
//!
//! This crate is the code form of `docs/design/0003-boot-information.md`. The design note is
//! the specification; if the two ever disagree, the note is right and this crate has a bug.
//!
//! Everything here is `#[repr(C)]` with fixed-size fields, so a loader written in any language
//! (or for a different architecture) produces exactly the same bytes. All addresses are
//! **physical** addresses; the kernel reaches them through the direct map at
//! [`BootInfo::direct_map_base`].

#![cfg_attr(not(test), no_std)]

/// `"PBRBOOT\0"` read as a little-endian `u64`: the first field of every [`BootInfo`].
pub const MAGIC: u64 = u64::from_le_bytes(*b"PBRBOOT\0");

/// The major version of this format. A loader and kernel must agree on it exactly.
pub const VERSION_MAJOR: u16 = 1;

/// The minor version. New minor versions only add fields at the end, so a kernel accepts any
/// minor version, using [`BootInfo::size`] to tell which fields are present.
pub const VERSION_MINOR: u16 = 0;

/// Flag: (Arm) the processor started at exception level 2, and the loader left the EL2 stub
/// in place, so the kernel can take over EL2 later to run virtual machines.
pub const FLAG_EL2_STUB: u32 = 1 << 0;

/// The most framebuffers (screens) a loader can describe. More can be found later by drivers.
pub const MAX_FRAMEBUFFERS: usize = 8;

/// A range of physical memory: where it starts and how many bytes long it is.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysRange {
    /// The first byte.
    pub base: u64,
    /// The length in bytes. Zero means "none".
    pub len: u64,
}

impl PhysRange {
    /// The empty range, meaning "not present".
    pub const NONE: Self = Self { base: 0, len: 0 };

    /// The byte just after the range, or `None` if it would overflow.
    pub const fn end(&self) -> Option<u64> {
        self.base.checked_add(self.len)
    }

    /// True if this range describes nothing.
    pub const fn is_none(&self) -> bool {
        self.len == 0
    }
}

/// Which loader started the kernel. For logs and diagnostics only: the kernel must never
/// behave differently depending on it.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoaderKind {
    /// Unknown or unspecified.
    Unknown = 0,
    /// The UEFI loader (`boot/uefi`).
    Uefi = 1,
    /// The BIOS loader (`boot/bios`).
    Bios = 2,
    /// The Raspberry Pi start-up stage (`boot/pi`).
    RaspberryPi = 3,
    /// A test harness that builds the boot information itself.
    Test = 4,
}

/// What a range of memory is. See the design note for exactly what the kernel may do with each.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryKind {
    /// Free for the kernel to use.
    Usable = 1,
    /// Never touch.
    Reserved = 2,
    /// Holds ACPI tables; usable once the kernel has finished reading them.
    AcpiReclaimable = 3,
    /// Must be preserved across sleep (ACPI non-volatile storage).
    AcpiNvs = 4,
    /// Faulty memory reported by the firmware.
    Bad = 5,
    /// Used by the loader (page tables, this structure); usable once the kernel has copied
    /// what it needs out of it.
    LoaderReclaimable = 6,
    /// The kernel image itself.
    Kernel = 7,
    /// A start-up module (see [`BootInfo::modules`]).
    Module = 8,
    /// A framebuffer.
    Framebuffer = 9,
    /// UEFI runtime services code or data: must be kept and mapped if runtime services are used.
    FirmwareRuntime = 10,
    /// Memory-mapped device registers that the firmware listed.
    DeviceMemory = 11,
}

/// One entry in the memory map.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    /// Where it is.
    pub range: PhysRange,
    /// A [`MemoryKind`] value. Stored as a plain `u32` so that an unknown value from a newer
    /// loader can be read safely; use [`MemoryRegion::kind`].
    pub kind: u32,
    /// Reserved; loaders write zero.
    pub reserved: u32,
}

impl MemoryRegion {
    /// The kind, or `None` if it's a value this kernel doesn't know (treat it as reserved).
    pub fn kind(&self) -> Option<MemoryKind> {
        Some(match self.kind {
            1 => MemoryKind::Usable,
            2 => MemoryKind::Reserved,
            3 => MemoryKind::AcpiReclaimable,
            4 => MemoryKind::AcpiNvs,
            5 => MemoryKind::Bad,
            6 => MemoryKind::LoaderReclaimable,
            7 => MemoryKind::Kernel,
            8 => MemoryKind::Module,
            9 => MemoryKind::Framebuffer,
            10 => MemoryKind::FirmwareRuntime,
            11 => MemoryKind::DeviceMemory,
            _ => return None,
        })
    }
}

/// How pixels are laid out in a framebuffer: the position and width of each colour channel
/// within a pixel, in bits.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PixelFormat {
    /// Bits per pixel (usually 32).
    pub bits_per_pixel: u8,
    /// Red channel: lowest bit, then width.
    pub red_shift: u8,
    /// Red channel width in bits.
    pub red_size: u8,
    /// Green channel: lowest bit.
    pub green_shift: u8,
    /// Green channel width in bits.
    pub green_size: u8,
    /// Blue channel: lowest bit.
    pub blue_shift: u8,
    /// Blue channel width in bits.
    pub blue_size: u8,
    /// Reserved; loaders write zero.
    pub reserved: u8,
}

/// A screen the firmware has already set up, as a plain grid of pixels in memory.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Framebuffer {
    /// The pixel memory.
    pub memory: PhysRange,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub stride: u32,
    /// How each pixel is laid out.
    pub format: PixelFormat,
}

/// Where the early debug console is, if the loader knows of one. The kernel writes its first
/// messages here before any driver exists.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EarlyConsole {
    /// 0 = none, 1 = 16550 UART on x86 I/O ports, 2 = 16550 UART memory-mapped,
    /// 3 = Arm PL011 UART memory-mapped.
    pub kind: u32,
    /// For memory-mapped UARTs, the spacing of registers in bytes (1 or 4).
    pub register_width: u32,
    /// The I/O port number or physical address.
    pub address: u64,
}

/// A file the loader placed in memory for the kernel, such as the start-up archive.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Module {
    /// Where its contents are.
    pub data: PhysRange,
    /// Its name, UTF-8, padded with zero bytes.
    pub name: [u8; 48],
}

impl Default for Module {
    fn default() -> Self {
        Self { data: PhysRange::NONE, name: [0; 48] }
    }
}

impl Module {
    /// The name as text, without the padding. `None` if it isn't valid UTF-8.
    pub fn name(&self) -> Option<&str> {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..len]).ok()
    }
}

/// The boot information. A loader fills one in and passes its physical address to the kernel.
///
/// Fields are only ever added at the end, in a new minor version.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    /// Always [`MAGIC`].
    pub magic: u64,
    /// Always [`VERSION_MAJOR`] for this layout.
    pub version_major: u16,
    /// The minor version the loader wrote.
    pub version_minor: u16,
    /// The size in bytes of the structure the loader wrote (at least `size_of::<BootInfo>()`
    /// for version 1.0).
    pub size: u32,
    /// A [`LoaderKind`] value.
    pub loader: u32,
    /// `FLAG_*` bits describing the machine's state.
    pub flags: u32,

    /// The memory map: an array of [`MemoryRegion`]s, sorted by address, not overlapping.
    pub memory_map: PhysRange,
    /// Where all physical memory is mapped in the kernel's address space: physical address
    /// `p` is at virtual address `direct_map_base + p`.
    pub direct_map_base: u64,
    /// Where the kernel image is in physical memory.
    pub kernel_physical: PhysRange,
    /// The virtual address the kernel image was linked at and mapped to.
    pub kernel_virtual_base: u64,

    /// ACPI's root pointer (RSDP) physical address, or 0. PCs have this.
    pub acpi_rsdp: u64,
    /// The flattened device tree, or none. Arm boards have this; some have both.
    pub device_tree: PhysRange,
    /// The SMBIOS entry point physical address, or 0.
    pub smbios: u64,
    /// The UEFI system table physical address, or 0 when not started by UEFI.
    pub uefi_system_table: u64,

    /// The kernel command line, UTF-8, not zero-terminated.
    pub command_line: PhysRange,
    /// An array of [`Module`]s.
    pub modules: PhysRange,

    /// The number of valid entries in [`BootInfo::framebuffers`].
    pub framebuffer_count: u32,
    /// Reserved; loaders write zero.
    pub reserved1: u32,
    /// Screens already set up by the firmware.
    pub framebuffers: [Framebuffer; MAX_FRAMEBUFFERS],
    /// The early debug console.
    pub early_console: EarlyConsole,

    /// The wall-clock time when the loader ran, in nanoseconds since 1970-01-01 UTC, or 0 if
    /// unknown.
    pub boot_time_ns: u64,
    /// Random bytes from the best source the loader had (UEFI RNG, RDRAND, the Pi's hardware
    /// RNG), for seeding the kernel's randomness. All zero if the loader had none.
    pub random_seed: [u8; 32],
    /// The hardware ID of the processor core that runs the kernel first (x86 APIC ID, Arm
    /// MPIDR affinity bits).
    pub boot_cpu_id: u64,
}

/// Why a [`BootInfo`] was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootInfoError {
    /// The magic number is wrong: this isn't boot information at all.
    BadMagic,
    /// A different major version, which this kernel can't read.
    UnsupportedVersion(u16),
    /// The structure is smaller than version 1.0 requires.
    TooSmall(u32),
    /// More framebuffers claimed than there is room for.
    TooManyFramebuffers(u32),
}

impl BootInfo {
    /// A blank structure for a loader to fill in: magic, version and size set, everything else
    /// zero or empty.
    pub const fn new(loader: LoaderKind) -> Self {
        Self {
            magic: MAGIC,
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            size: core::mem::size_of::<Self>() as u32,
            loader: loader as u32,
            flags: 0,
            memory_map: PhysRange::NONE,
            direct_map_base: 0,
            kernel_physical: PhysRange::NONE,
            kernel_virtual_base: 0,
            acpi_rsdp: 0,
            device_tree: PhysRange::NONE,
            smbios: 0,
            uefi_system_table: 0,
            command_line: PhysRange::NONE,
            modules: PhysRange::NONE,
            framebuffer_count: 0,
            reserved1: 0,
            framebuffers: [Framebuffer {
                memory: PhysRange::NONE,
                width: 0,
                height: 0,
                stride: 0,
                format: PixelFormat {
                    bits_per_pixel: 0,
                    red_shift: 0,
                    red_size: 0,
                    green_shift: 0,
                    green_size: 0,
                    blue_shift: 0,
                    blue_size: 0,
                    reserved: 0,
                },
            }; MAX_FRAMEBUFFERS],
            early_console: EarlyConsole { kind: 0, register_width: 0, address: 0 },
            boot_time_ns: 0,
            random_seed: [0; 32],
            boot_cpu_id: 0,
        }
    }

    /// Checks the header: the first thing the kernel does with boot information.
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.magic != MAGIC {
            return Err(BootInfoError::BadMagic);
        }
        if self.version_major != VERSION_MAJOR {
            return Err(BootInfoError::UnsupportedVersion(self.version_major));
        }
        if (self.size as usize) < core::mem::size_of::<Self>() {
            return Err(BootInfoError::TooSmall(self.size));
        }
        if self.framebuffer_count as usize > MAX_FRAMEBUFFERS {
            return Err(BootInfoError::TooManyFramebuffers(self.framebuffer_count));
        }
        Ok(())
    }

    /// The loader that wrote this, if it's one this kernel knows.
    pub fn loader(&self) -> LoaderKind {
        match self.loader {
            1 => LoaderKind::Uefi,
            2 => LoaderKind::Bios,
            3 => LoaderKind::RaspberryPi,
            4 => LoaderKind::Test,
            _ => LoaderKind::Unknown,
        }
    }

    /// The valid framebuffers.
    pub fn framebuffers(&self) -> &[Framebuffer] {
        let n = (self.framebuffer_count as usize).min(MAX_FRAMEBUFFERS);
        &self.framebuffers[..n]
    }
}

// The layout is a contract between separately built programs, so it's pinned here: changing
// any of these numbers is a format change and needs a new version (see the design note).
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<PhysRange>() == 16);
    assert!(size_of::<MemoryRegion>() == 24);
    assert!(size_of::<PixelFormat>() == 8);
    assert!(size_of::<Framebuffer>() == 36 + 4);
    assert!(size_of::<EarlyConsole>() == 16);
    assert!(size_of::<Module>() == 64);
    assert!(offset_of!(BootInfo, memory_map) == 24);
    assert!(offset_of!(BootInfo, direct_map_base) == 40);
    assert!(offset_of!(BootInfo, kernel_physical) == 48);
    assert!(offset_of!(BootInfo, kernel_virtual_base) == 64);
    assert!(offset_of!(BootInfo, acpi_rsdp) == 72);
    assert!(offset_of!(BootInfo, device_tree) == 80);
    assert!(offset_of!(BootInfo, smbios) == 96);
    assert!(offset_of!(BootInfo, uefi_system_table) == 104);
    assert!(offset_of!(BootInfo, command_line) == 112);
    assert!(offset_of!(BootInfo, modules) == 128);
    assert!(offset_of!(BootInfo, framebuffer_count) == 144);
    assert!(offset_of!(BootInfo, framebuffers) == 152);
    assert!(offset_of!(BootInfo, early_console) == 152 + 8 * 40);
    assert!(offset_of!(BootInfo, boot_time_ns) == 488);
    assert!(offset_of!(BootInfo, random_seed) == 496);
    assert!(offset_of!(BootInfo, boot_cpu_id) == 528);
    assert!(size_of::<BootInfo>() == 536);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_spells_pbrboot() {
        assert_eq!(&MAGIC.to_le_bytes(), b"PBRBOOT\0");
        assert_eq!(MAGIC, 0x0054_4F4F_4252_4250); // the value given in note 0003
    }

    #[test]
    fn new_is_valid() {
        let info = BootInfo::new(LoaderKind::Test);
        assert_eq!(info.validate(), Ok(()));
        assert_eq!(info.loader(), LoaderKind::Test);
        assert!(info.framebuffers().is_empty());
    }

    #[test]
    fn rejects_bad_headers() {
        let mut info = BootInfo::new(LoaderKind::Test);
        info.magic ^= 1;
        assert_eq!(info.validate(), Err(BootInfoError::BadMagic));

        let mut info = BootInfo::new(LoaderKind::Test);
        info.version_major = 2;
        assert_eq!(info.validate(), Err(BootInfoError::UnsupportedVersion(2)));

        let mut info = BootInfo::new(LoaderKind::Test);
        info.size = 100;
        assert_eq!(info.validate(), Err(BootInfoError::TooSmall(100)));

        let mut info = BootInfo::new(LoaderKind::Test);
        info.framebuffer_count = 9;
        assert_eq!(info.validate(), Err(BootInfoError::TooManyFramebuffers(9)));
    }

    #[test]
    fn newer_minor_versions_are_accepted() {
        let mut info = BootInfo::new(LoaderKind::Uefi);
        info.version_minor = 7;
        info.size += 64; // a newer loader appended fields we don't know about
        assert_eq!(info.validate(), Ok(()));
    }

    #[test]
    fn unknown_values_are_handled() {
        let region =
            MemoryRegion { range: PhysRange { base: 0, len: 4096 }, kind: 999, reserved: 0 };
        assert_eq!(region.kind(), None);
        let mut info = BootInfo::new(LoaderKind::Test);
        info.loader = 77;
        assert_eq!(info.loader(), LoaderKind::Unknown);
    }

    #[test]
    fn module_names() {
        let mut m = Module::default();
        m.name[..6].copy_from_slice(b"rootfs");
        assert_eq!(m.name(), Some("rootfs"));
        m.name = [b'x'; 48];
        assert_eq!(m.name().map(str::len), Some(48));
        m.name[0] = 0xFF;
        assert_eq!(m.name(), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(PhysRange { base: 10, len: 5 }.end(), Some(15));
        assert_eq!(PhysRange { base: u64::MAX, len: 2 }.end(), None);
        assert!(PhysRange::NONE.is_none());
    }
}
