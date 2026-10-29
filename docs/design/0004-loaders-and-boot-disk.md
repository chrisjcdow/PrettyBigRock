# 0004: Loaders and the boot disk

Status: Accepted · 29 October 2026 · Related: DESIGN.md section 5; D3; notes 0001 and 0003;
code: `boot/`

## The problem

PrettyBigRock has to start in three ways for milestone M1:
- UEFI on x86-64;
- UEFI on ARM64;
- BIOS on x86-64.

The Raspberry Pi's own start-up stage comes with milestone M10. Each loader has to produce
exactly the state in note 0003. They should share as much code as possible, so that the
state is produced the same way every time. The result should also be one disk image that
works on every machine, so a single USB stick can start any of them.

## The design

### One disk image for everything

The build tool makes `build/prettybigrock.img`: a GPT disk, with a protective MBR whose boot
code is the BIOS loader's first stage.

| Where | What |
|-------|------|
| Sector 0 | Protective MBR. Bytes 0–439 are the BIOS first stage. Bytes 432–439 hold the BIOS partition's first sector (`u32`) and the number of sectors of BIOS loader (`u16`), filled in by the build tool. |
| Sectors 1–33 | GPT header and partition table (and the backup copy at the end of the disk) |
| Partition 1 | **BIOS boot partition** (type `21686148-6449-6E6F-744E-656564454649`), 8 MiB. It holds the BIOS loader, then the payload (below). |
| Partition 2 | **EFI system partition** (type `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`), 64 MiB, FAT32. |

The EFI system partition holds:
```
EFI/BOOT/BOOTX64.EFI       the UEFI loader for x86-64
EFI/BOOT/BOOTAA64.EFI      the UEFI loader for ARM64
PBR/x86_64/kernel.elf      the kernel for each architecture
PBR/aarch64/kernel.elf
PBR/cmdline.txt            the kernel command line (optional)
```

The BIOS payload, starting at the first sector after the BIOS loader, is a 512-byte header
followed by the files it lists. The header is:
- the magic `"PBRPAYLD"`;
- a `u32` count of files;
- a `u32` total length of the payload in sectors, header included;
- then, from offset 16, per file, an entry of 48 bytes:
  - a `u32` offset in sectors from the header;
  - a `u32` length in bytes;
  - a 40-byte UTF-8 name, padded with zero bytes.

The names are `kernel.elf` and `cmdline.txt`, the same files as on the EFI system partition.

The FAT32 filesystem and the GPT are written by the build tool's own code, not by outside
tools.

### What's shared: `boot/common` (`pbr-boot-common`)

Everything that isn't firmware-specific is written once and tested on the host:
- **ELF loading**: reading the kernel's program headers, checking it's the right machine,
  and placing its segments.
- **The memory map**: converting firmware maps into note 0003's kinds, carving out the
  kernel, the framebuffers and the stub, sorting, merging neighbours, and rounding Usable
  regions inwards to 4 KiB.
- **Page tables** for x86-64 and AArch64, built from the memory map:
  - the kernel's segments, with their own permissions;
  - the direct map at `0xFFFF_8000_0000_0000`, with each region mapped by kind (RAM as normal
    memory, framebuffers as non-cacheable or write-combining memory, device memory as device
    memory, nothing else). Large pages are used where alignment allows;
  - an identity map of RAM, so the loader keeps running while it switches page tables.
- **Hardware description helpers**: checking the ACPI root pointer, finding ACPI tables (SPCR,
  for the serial console on Arm), and a small reader for device trees.
- **The hand-over**: the short piece of assembly per architecture that loads the page tables,
  sets up the registers note 0003 requires, and jumps to the kernel.

### The UEFI loader: `boot/uefi`

It's a UEFI application (Rust targets `x86_64-unknown-uefi` and `aarch64-unknown-uefi`), using
our own small definitions of the UEFI interfaces it needs. Its steps:
1. Find the volume it was loaded from, and read `PBR/<arch>/kernel.elf` and `PBR/cmdline.txt`.
2. Load the kernel into newly allocated pages.
3. Gather:
   - every screen's framebuffer (all Graphics Output Protocol handles, ignoring duplicates);
   - the ACPI, device tree and SMBIOS tables;
   - random bytes (the RNG protocol, then `RDRAND` on x86-64);
   - the time;
   - the serial console (COM1 on x86-64; the ACPI SPCR table or the device tree on ARM64).
4. Allocate everything the hand-over will need in advance: page-table memory, the stack, the
   boot information, and room for the memory map. After `ExitBootServices` nothing can be
   allocated.
5. Get the memory map, call `ExitBootServices`, then build the boot information and page
   tables, and hand over.

On ARM64, if the firmware left the processor at EL2, the loader installs the EL2 stub and
drops to EL1 as part of the hand-over (note 0003).

### The BIOS loader: `boot/bios`

There are three stages, all in one crate. The 16-bit and 32-bit parts are assembly inside the
Rust crate, so no separate assembler is needed.
1. **First stage** (the MBR, 440 bytes): reads the BIOS loader from the BIOS boot partition
   with the BIOS's extended disk reads, and jumps to it.
2. **Second stage** (16-bit, then 64-bit):
   - turns on the A20 line;
   - gets the memory map (`E820`);
   - chooses and sets a graphics mode (VESA BIOS Extensions; prefers the largest 32-bit
     mode up to 1920×1080, with the same choice logic as 1.x);
   - copies the payload above 1 MiB through a buffer in low memory;
   - builds temporary page tables identity-mapping the first 4 GiB, and switches to 64-bit
     mode.
3. **Third stage** (Rust): does what the UEFI loader does from step 2 onwards:
   - finds the ACPI root pointer and SMBIOS by scanning the BIOS areas;
   - reads the time from the CMOS clock;
   - takes randomness from `RDRAND`;
   - converts the `E820` map.

   It takes memory for page tables and the boot information from Usable memory above the
   payload, and records it as LoaderReclaimable.

### Testing

`cargo xtask qemu-test` boots the image four ways:
- x86-64 with UEFI (OVMF);
- x86-64 with BIOS (SeaBIOS);
- ARM64 with UEFI at EL1;
- ARM64 with UEFI at EL2 (`virtualization=on`, which tests the EL2 stub).

Each run watches the serial port for the kernel's start-up report, and fails if it doesn't
appear within the time limit. GitHub Actions runs this on every change.

## Alternatives considered

- **Separate images for BIOS and UEFI.** That's simpler to build, but one image that starts
  anywhere is far more useful on real hardware.
- **Reading the kernel from FAT in the BIOS loader.** It's tidier, but it needs a FAT reader in
  16-bit code. The BIOS boot partition is simpler and is what GRUB does.
- **An outside assembler (NASM) for the BIOS stages.** Rust's built-in assembler handles 16-bit
  code, so the build needs only Rust.

## Open questions

- Some old BIOSes only boot a disk with an "active" partition, which a protective MBR doesn't
  have. If Roxy turns out to be one of them, the image needs a hybrid MBR. We'll find out at
  M7.
- Secure Boot: the loaders aren't signed. Machines need Secure Boot turned off for now.

## Changes
- 29 October 2026: first version.
- 29 October 2026: built. The payload header gained its total length, so the second stage
  can copy it without understanding it.
