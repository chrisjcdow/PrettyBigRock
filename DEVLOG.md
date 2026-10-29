# PrettyBigRock development log

PrettyBigRock (originally FynbosOS) is developed by Chris together with Claude, Anthropic's AI model.
This log records what was built in each session and who did what.

## Session 1: 28 October 2026, from one prompt to a booting desktop (FynbosOS 1.0)

**The prompt (Chris):** "Could you write me an operating system that I could boot in a VM.
This OS should have a GUI. Try not to plagiarise too much."

**What Claude built, in about 24 minutes:**
- A two-stage boot loader that picks a VESA graphics mode and enters 32-bit protected mode
- A kernel with interrupts, timer, PS/2 keyboard and mouse, CMOS clock and ACPI power-off
- Double-buffered graphics, a hand-drawn 5x9 font, pixel-art icons and a mouse cursor
- A window manager with draggable windows, taskbar, start menu and desktop icons
- A procedurally drawn wallpaper of Table Mountain at sunset, tablecloth included
- Seven apps: Terminal, Notepad, Files, Calculator, Paint, Snake and About
- Disk images for QEMU, VirtualBox and VMware

Claude tested it in QEMU by scripting mouse and keyboard input and reading screenshots,
fixing a few things along the way (a hidden sun, a spiky cloud, Ctrl+Esc, scrollback).

**Chris:** booted it on Harriet (Windows 11 laptop) in QEMU, and decided to publish it on GitHub
and keep developing it together.

## Session 2: 28 October 2026, a new name (PrettyBigRock 1.1)

**Chris** chose to publish under GPL-3.0-or-later so the project stays open source, and started
rethinking the name. He'd wondered about "FynbOS", but "fynbos" is hard to search for and is
already a registered software trademark (the Fynbos Money app). Chris has a habit of pointing at
Table Mountain and saying "big rock" or "pretty rock", so he suggested **PrettyBigRock**.

**Claude:**
- Renamed the OS everywhere: boot loader, splash, start button and menu, About, Terminal
  (prompt is now `rock>`), crash and shutdown screens, docs, disk images and launcher
- Drew a new 16x16 pixel-art logo of Table Mountain with its tablecloth, the sun and the sea
- Replaced the protea in the Terminal's `fetch` output with coloured text art of the mountain
  (Chris spotted the leftover protea while test-booting before publishing)
- Hid an Easter egg in the Terminal for Chris and his wife, at Chris's request. It isn't in `help`.
- Kept the original FynbosOS 1.0 safe, as tag `fynbos-1.0` and as Chris's own copy

## Session 3: 28-29 October 2026, publishing, RockFS design, and PrettyBigRock on a tablet (1.2)

**Chris** published the repository on GitHub with releases for FynbosOS 1.0 and PrettyBigRock 1.1.
He chose to build PrettyBigRock's own filesystem, RockFS, rather than use FAT, and reviewed the
design with Claude over three drafts, asking what we might regret in the long run. His points led
to feature flags, 255-character names, safe saving, recorded name rules, a growable inode table,
partition support, 64-bit extents, and a clean failure when there isn't room to save safely.
Then he asked to run PrettyBigRock on Isaac, his Galaxy Tab S10+.

**Claude:**
- Wrote the RockFS spec (`docs/ROCKFS.md`, draft 3). It is not implemented yet.
- Added an absolute pointer driver (the VMware mouse protocol, supported by QEMU, VMware and
  the v86 emulator), with automatic fallback to the PS/2 mouse. QEMU no longer needs to capture
  the mouse, and on a touch screen your finger goes straight to where you touch.
- Built a browser player (`web/player.html`, `tools/mkweb.py`) that runs PrettyBigRock in the v86
  emulator, with touch as the mouse, an on-screen keyboard that copes with predictive text, and
  a row of extra keys. Tested on a simulated tablet; Chris confirmed it works on Isaac.

## Session 4: 29 October 2026, RockFS (1.3)

**Chris** asked Claude to build RockFS while he was at work, following the version 1 design
they had settled together the night before.

**Claude:**
- Wrote `tools/rockfs.py` first: an independent Python implementation of the spec, which is
  both the Windows/Linux/macOS tool for getting files in and out of a RockFS disk and a
  reference to test the OS against. Its own tests run thousands of random operations, a
  fragmented disk with a deep extent tree, a full disk, and 60 simulated power cuts.
- Added a memory allocator that can free memory, an ATA (IDE) disk driver with LBA48 and cache
  flushes, a RAM disk, and a write-through block cache.
- Wrote RockFS in the kernel (`kernel/rockfs.c`): finding volumes on MBR, GPT or whole disks,
  format, mount, the growable inode table, 64-bit extents and extent trees, copy-on-write
  saving, folders, names, rename, and check-and-repair.
- Compiled the kernel's RockFS as an ordinary program (`tools/hosttest/`) and checked it
  against the Python version both ways, plus 40 power cuts at random moments. This found one
  real bug: repairing a disk could overflow a buffer when it had to create `/lost+found`.
  It also showed that the space check was too cautious about very large files.
- Rebuilt the apps on the filesystem: Files has folders, New file, New folder, Rename and
  Delete; Notepad has Open, Save and Save as; Paint saves and opens .bmp pictures; the
  Terminal gained `cd`, `mkdir`, `cp`, `mv`, `disk`, `check` and `format`; small dialog boxes
  ask for names and confirm deletes.
- At start-up PrettyBigRock looks for a data disk. It offers to set up a blank one, checks
  and repairs one that wasn't shut down cleanly, and otherwise keeps files in memory.
  Shutting down or restarting marks the disk clean.
- Tested in QEMU: formatting, saving, restarting, pulling the plug mid-copy, a full disk,
  and swapping files with the Python tool. A picture drawn in Paint came out of the disk
  image as an ordinary .bmp.
- The Windows launcher now makes a 64 MB `data.img` the first time it runs.

## Session 5: 29 October 2026, planning PrettyBigRock 2

**Chris** said that if PrettyBigRock is to become a usable OS, it needs planning with a
long-term view, so that future problems are avoided as early as possible. Over the afternoon
the two of them settled the design:
- 64-bit, on x86-64 and ARM64, with portability to other processors designed in;
- UEFI and BIOS booting;
- a Rust microkernel;
- every program sandboxed with capabilities, managed by a sandbox manager (Chris's idea);
- a native PrettyBigRock interface with a POSIX layer on top ("we don't want to just make
  another Linux");
- multi-core and multi-threaded from the start;
- several filesystems;
- the GUI as a program rather than part of the kernel;
- real hardware;
- everything written ourselves;
- documentation first.

**Claude** wrote it all up as `docs/DESIGN.md` (the decisions, the reasons, and the open
questions) and `docs/ROADMAP.md` (the milestones, from an empty kernel to real hardware).

## Session 5, continued: milestone M0

**Chris** settled the open questions: a linking exception for the app libraries, so that
proprietary apps are possible; 1.x moving into `classic/`; the browser player staying on
1.x; and PrettyBigRock being general purpose. As a desktop that means web browsing, media,
personal documents, software development and music production; as a server, running what
Roxy runs today. He also asked for support for several screens.

**Claude:**
- Added those to the design (draft 2). The uses brought in real-time scheduling, room for
  virtual machines (for running Linux server software such as Docker), GPU-ready display
  buffers, and porting large software as a goal of the POSIX layer. They also extended the
  roadmap to M15.
- Moved the C system into `classic/`. It still builds a byte-identical image, and the
  cross-checks still pass.
- Started PrettyBigRock 2:
  - a Rust workspace, pinned to Rust 1.97.0;
  - the kernel with its architecture layer, building for x86-64 and ARM64;
  - `lib/bootinfo`, the boot information as code, with its layout checked at compile time;
  - `xtask`, the build tool (`build`, `check`, `test`, `ci`, `classic`, `doctor`);
  - a GitHub Actions workflow.
- Wrote the first design notes: the repository layout (0001), coding rules (0002), and the
  boot information with the machine's state at the kernel's entry point (0003).
- Claude's workspace can't download Rust's ready-built targets (static.rust-lang.org is
  blocked), so the build tool can also build Rust's core library from source. It used the
  `library/` folder at the exact 1.97.0 commit, from GitHub. Elsewhere, rustup's normal
  targets are used.

## Session 6: 29 October 2026, milestone M1 (booting)

**Chris** asked Claude to carry on with M1 while he was at work.

**Claude:**
- Wrote design note 0004, on the loaders and the boot disk. Fixed two things in note 0003
  found along the way: the kernel also gets the direct map's address in a register, and the
  EL2 stub's interface is now defined.
- Wrote `boot/common`, which every loader shares, with 14 tests on the host:
  - loading the kernel's ELF file;
  - building the memory map;
  - page tables for x86-64 and ARM64;
  - finding ACPI tables, and reading device trees;
  - the hand-over to the kernel, including dropping from EL2 to EL1 on ARM.
- Wrote the UEFI loader (`boot/uefi`), with our own definitions of the UEFI interfaces. It
  builds for x86-64 and ARM64 and finds every screen, the ACPI, device tree and SMBIOS
  tables, randomness, the time and the serial port.
- Wrote the BIOS loader (`boot/bios`):
  - a 16-bit first stage in the MBR;
  - a second stage that sets the memory map, the graphics mode and 64-bit mode;
  - a Rust third stage sharing everything else with the UEFI loader.

  It's all assembled by Rust itself, with no NASM.
- Gave the kernel its first output: a serial console (PL011 and 16550) and a text console on
  the screen in the 1.x font, plus a start-up report of what the loader handed over.
- Taught the build tool to make the boot disk image (GPT and FAT32 written by our own code,
  with a test that reads the files back) and to boot it in QEMU:
  - `cargo xtask run` starts it in a window;
  - `cargo xtask qemu-test` boots it four ways (x86-64 with UEFI and with BIOS, ARM64 with
    UEFI at EL1 and at EL2) and checks the report each time. GitHub Actions now does too.
- All four boot configurations worked first time, in debug and release builds.
- Added `run-prettybigrock.bat` for trying PrettyBigRock 2 in QEMU on Windows.

## Session 7: 29 October 2026, milestone M2 (the kernel core)

**Chris** got home and booted PrettyBigRock 2 on Harriet all three ways. He pushed
everything to GitHub, where the automatic checks passed first time, and published the v1.2
and v1.3 releases. Then he asked for M2.

**Claude:**
- Wrote design note 0005, on the kernel core, before the code.
- Moved the page-table, ACPI and device-tree code into `lib/platform`, shared by the loaders
  and the kernel, and added readers for ACPI's MADT, HPET, GTDT and FADT tables, with tests.
- Built the kernel core, on both architectures:
  - a physical page allocator;
  - the kernel's own page tables, with stacks behind guard pages and an area for device
    registers;
  - the heap, so the kernel can use `Box`, `Vec` and `Arc`;
  - per-core data;
  - locks, with a lock-order checker in debug builds;
  - the clock and a 1000 Hz timer on every core (HPET/TSC and the local APIC on x86-64,
    the generic timer and the GIC on ARM64);
  - exceptions;
  - starting the other cores: a real-mode trampoline and INIT-SIPI on x86-64, and PSCI with
    its own EL2-to-EL1 drop on ARM64;
  - a scheduler with a run queue per core, priorities, preemption, work stealing, sleep,
    join and wait queues.
- Added a self-test (`selftest` on the command line) and a second disk image that uses it.
  The boot tests now run it on every configuration, with 4 cores.
- Problems found along the way:
  - The lock-order checker caught the heap and page allocator taken in the wrong order, and
    a diagnostic list taking the heap.
  - Secondary ARM cores read their start-up record after turning the MMU on, when it was no
    longer mapped.
  - A thread could be moved to another core halfway through marking itself not preemptible.
  - On x86 without x2APIC, an interrupt in the middle of sending an inter-core interrupt
    could redirect it.
- Stress-tested the result by booting all four configurations 60 times in a row.
