# 0003: Boot information, and the machine's state when the kernel starts

Status: Accepted, format version **1.0** · 29 October 2026 · Related: DESIGN.md section 5; D3;
code: `lib/bootinfo`

## The problem

PrettyBigRock is started in different ways:
- by a UEFI loader on modern PCs and Arm machines;
- by a BIOS loader on older PCs;
- by the Raspberry Pi's own firmware.

The kernel must never care which. So every loader has to leave the machine in the same state,
and describe it in the same way. This note is that contract. It's a specification: a loader
written from this note alone must work.

## The design

### What every loader does

1. Loads the kernel (an ELF executable, linked at `0xFFFF_FFFF_8000_0000`) and the start-up
   modules into physical memory.
2. Finds the hardware description (ACPI tables, device tree, or both), the memory map, the
   screens the firmware has set up, and a source of random numbers.
3. Builds page tables that map, at least:
   - the kernel's segments, at the addresses it was linked at, with the permissions in its
     ELF program headers (code read-and-execute, read-only data read-only, data read-write,
     nothing both writable and executable);
   - a **direct map**: all physical memory in the memory map at `direct_map_base + physical
     address`, read-write and never executable. The exceptions are Bad regions, and on Arm
     also Reserved regions, which may belong to the firmware's secure world and must never
     be touched;
   - a stack of at least 64 KiB.
4. Fills in a `BootInfo` structure, and everything it points to, in memory marked
   `LoaderReclaimable`.
5. Leaves the other processor cores parked (see below), and jumps to the kernel's entry point.

The loader may also leave other mappings (such as an identity map of low memory). The kernel
must not rely on them, and replaces the page tables early.

### The machine's state at the kernel's entry point

**x86-64:**
- 64-bit long mode, privilege level 0, 4-level paging. 5-level paging is off.
- `CR0.WP` (write protection) on; `EFER.NXE` (no-execute) on.
- Interrupts off (`RFLAGS.IF = 0`). The IDT is undefined, so the kernel loads its own before
  enabling interrupts. A GDT with a 64-bit code segment is loaded; the kernel loads its own.
- `RDI` = the **physical** address of the `BootInfo`; `RSI` = `direct_map_base` (so the kernel
  can reach the `BootInfo` before it has read it).
- `RSP` = 8 bytes below a 16-byte-aligned stack top, as if the entry point had been reached
  by a `call`, so ordinary Rust code sees the alignment it expects. The kernel's entry point
  never returns.
- The floating-point and vector state is undefined; the kernel doesn't use it.
- From UEFI: `ExitBootServices` has been called.
- Other cores: still waiting for their start-up signal (INIT-SIPI), as the firmware left them.

**AArch64:**
- Exception level 1, MMU on, 4 KiB pages, 48-bit virtual addresses. `TTBR1_EL1` holds the
  upper half (the kernel and the direct map). `TTBR0_EL1` is undefined.
- `MAIR_EL1`: attribute index 0 = normal memory, write-back cacheable; index 1 = device
  memory (nGnRnE); index 2 = normal memory, non-cacheable. Framebuffers are mapped with
  index 2.
- Data and instruction caches on; alignment checking off.
- All interrupts masked (`DAIF` all set).
- `X0` = the **physical** address of the `BootInfo`; `X1` = `direct_map_base`. `SP` = the top
  of the stack, 16-byte aligned.
- If the processor started at exception level 2, the loader drops to EL1 and leaves a minimal
  EL2 stub so the kernel can take over EL2 later to run virtual machines (DESIGN.md section
  6.1). It sets `FLAG_EL2_STUB` to say so. The stub lives in a page marked Reserved in the
  memory map. Its whole interface, for now: from EL1, `HVC #0` with `X0 = 0` and `X1` = the
  physical address of a new EL2 vector table installs that table (`VBAR_EL2`) and returns.
  Any other `HVC` returns without doing anything.
- Timers: `CNTHCTL_EL2` lets EL1 use the physical counter and timer, and `CNTVOFF_EL2` is 0.
  Floating-point instructions aren't trapped to EL2.
- Other cores: parked by the firmware, as the device tree or ACPI tables describe (PSCI or a
  spin table).

### The `BootInfo` structure, version 1.0

All fields are little-endian, at fixed offsets. All addresses are physical. A `PhysRange` is
two `u64`s: `base` and `len` (in bytes; `len = 0` means "none").

| Offset | Size | Field | Meaning |
|-------:|-----:|-------|---------|
| 0 | 8 | `magic` | `"PBRBOOT\0"` as bytes, so `0x00544F4F42524250` as a `u64` |
| 8 | 2 | `version_major` | 1 |
| 10 | 2 | `version_minor` | 0 |
| 12 | 4 | `size` | Size in bytes of the structure as written: 536 for version 1.0 |
| 16 | 4 | `loader` | 1 UEFI, 2 BIOS, 3 Raspberry Pi stage, 4 test harness, 0 unknown. For logs only |
| 20 | 4 | `flags` | Bit 0: `FLAG_EL2_STUB` (Arm). Other bits zero |
| 24 | 16 | `memory_map` | Array of `MemoryRegion`s |
| 40 | 8 | `direct_map_base` | Virtual address where physical address 0 is mapped |
| 48 | 16 | `kernel_physical` | Where the kernel image is |
| 64 | 8 | `kernel_virtual_base` | The address it's linked and mapped at |
| 72 | 8 | `acpi_rsdp` | ACPI root pointer, or 0 |
| 80 | 16 | `device_tree` | Flattened device tree blob, or none |
| 96 | 8 | `smbios` | SMBIOS entry point, or 0 |
| 104 | 8 | `uefi_system_table` | UEFI system table, or 0 |
| 112 | 16 | `command_line` | UTF-8 text, not zero-terminated |
| 128 | 16 | `modules` | Array of `Module`s |
| 144 | 4 | `framebuffer_count` | Valid entries in `framebuffers`, 0 to 8 |
| 148 | 4 | reserved | 0 |
| 152 | 320 | `framebuffers` | 8 × `Framebuffer` |
| 472 | 16 | `early_console` | `EarlyConsole` |
| 488 | 8 | `boot_time_ns` | Wall-clock time in nanoseconds since 1970-01-01 UTC, or 0 |
| 496 | 32 | `random_seed` | Random bytes, or all zero if none were available |
| 528 | 8 | `boot_cpu_id` | x86 APIC ID, or Arm `MPIDR_EL1` affinity bits, of the core running the kernel |

`MemoryRegion` (24 bytes):
- `range` (`PhysRange`);
- `kind` (`u32`);
- reserved (`u32`, 0).

Kinds:

| Value | Kind | What the kernel may do |
|------:|------|------------------------|
| 1 | Usable | Use freely |
| 2 | Reserved | Never touch |
| 3 | AcpiReclaimable | Use once it's finished with the ACPI tables |
| 4 | AcpiNvs | Never touch; must survive sleep |
| 5 | Bad | Never touch |
| 6 | LoaderReclaimable | Use once it has copied out what it needs (this structure lives here) |
| 7 | Kernel | The kernel image |
| 8 | Module | Start-up modules; free them once used |
| 9 | Framebuffer | Screen memory |
| 10 | FirmwareRuntime | Keep, and map, if UEFI runtime services are used |
| 11 | DeviceMemory | Device registers the firmware listed |

Unknown kinds are treated as Reserved.

The memory map is sorted by address and has no overlaps. Usable regions are aligned to 4 KiB
(the loader rounds them inwards).

`Framebuffer` (40 bytes):
- `memory` (`PhysRange`);
- `width`, `height`, `stride` (bytes per row), all `u32`;
- `PixelFormat`, 8 bytes:
  - `bits_per_pixel`;
  - then the shift and size in bits of red, green and blue;
  - one reserved byte, 0.

Several framebuffers are allowed, for several screens (DESIGN.md section 8).

`EarlyConsole` (16 bytes):
- `kind` (`u32`): 0 none, 1 16550 UART on x86 I/O ports, 2 16550 memory-mapped, 3 Arm PL011
  memory-mapped;
- `register_width` (`u32`, bytes between registers for memory-mapped UARTs);
- `address` (`u64`: the port number or physical address).

`Module` (64 bytes):
- `data` (`PhysRange`);
- `name` (48 bytes of UTF-8, padded with zero bytes).

The module named `startup` is the start-up archive (DESIGN.md section 5); its format gets its
own note.

### Versions

- **The kernel accepts boot information** if `magic` matches, `version_major` is exactly 1,
  and `size` is at least 536.
- **Minor versions only add fields at the end.** The kernel uses `size` to tell which fields
  are present, so an older kernel works with a newer loader and the other way round.
- **Any other change** (moving, resizing or reinterpreting a field) is a new major version.
- **Reserved fields** are written as zero by loaders and ignored by the kernel. A later minor
  version may give them a meaning, but only one where zero means "as before".

`lib/bootinfo` checks every offset above at compile time, so the code can't drift from this
table.

## Alternatives considered

- **An existing boot protocol** (Multiboot 2, Limine, or Linux's). Multiboot 2 starts x86
  kernels in 32-bit mode with no paging, and has no Arm version. Limine's protocol is good,
  but it's someone else's design, and D13 rules out depending on it. Linux's protocol is
  shaped around Linux.
- **The kernel setting up its own page tables from a flat entry state.** It's simpler for
  the loaders, but it puts a different early, fragile code path in the kernel for each
  loader and architecture. Having loaders build a known memory layout keeps the kernel's
  start-up the same everywhere.
- **Virtual addresses in the boot information.** Physical addresses let the kernel move the
  direct map later without the boot information becoming wrong.

## Open questions

- The start-up archive's format (before M3).
- A fuller EL2 stub interface on Arm, if the virtual-machine work needs one.

## Changes
- 29 October 2026: version 1.0.
- 29 October 2026: still 1.0, since nothing had been released. The entry point also receives
  `direct_map_base` in a second register, and the EL2 stub's interface is defined. Both
  were found while writing the first loaders.
