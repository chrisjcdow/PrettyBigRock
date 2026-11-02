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

## Session 8: 31 October 2026, milestone M3 (capabilities, messages and the first programs)

**Chris** asked for M3, and to be walked through its key decisions one at a time before any
design notes or code, as for the overall design. He settled ten decisions:
- processes that hold the capability tables;
- 32-bit handles, rights, copies that can be revoked;
- memory budgets;
- endpoints with badges (rather than a private channel per client), plus the "last sender
  gone" signal, after asking whether we'd regret it and whether to build both;
- message sizes;
- memory objects, notifications and rings;
- the system page and the list of calls;
- the start-up archive and program files;
- the interface language;
- the licences.

He named the native library: asked what Table Mountain is made of, he chose **granite** for
the bedrock library, keeping **sandstone** for a later layer. He asked that tests stay small
but never at the cost of quality, since everything later is built on this.

**Claude:**
- Wrote design notes 0006 to 0009, which Chris accepted, and draft 3 of DESIGN.md. The
  licences changed from a linking exception to MPL-2.0 for `granite` and MIT for interfaces.
- Built host-tested libraries first:
  - the capability derivation tree (`lib/captree`), with model tests;
  - ELF reading for kernels and programs (`lib/elf`);
  - the start-up archive (`lib/archive`, with `tools/startarc.py` as an independent reader).
- Built the kernel side, on both architectures:
  - user mode, with SMEP, SMAP and PAN on all the time;
  - processes, capability tables, budgets, memory objects and address spaces;
  - endpoints with direct hand-over, replies, notifications;
  - the system page and the system calls;
  - a reaper that finishes ending processes;
  - loading the root program from the start-up archive, which both loaders now bring.
- Built **granite** (programs' start-up, system calls, handles, messages, memory, threads, a
  heap, the program loader, ring buffers) and PrettyBigRock's own Rust targets for programs.
- Built the **interface compiler** (`lib/interface-compiler`) and the generated code
  (`lib/interfaces`). `tools/pbi_check.py` independently works out every message's layout,
  and the two are compared on every test run.
- Added the **program self-test**: the root program runs `echo` and `probe` through:
  - messages and capabilities;
  - an interface's generated code;
  - isolation;
  - four kinds of fault;
  - revocation;
  - the last sender signal;
  - a crashing service;
  - budgets;
  - 1200 calls across 4 threads.

  It passes on all four boot configurations.
- Problems found along the way:
  - revoking was quadratic, so a model test took 4 minutes; it now takes 9 seconds;
  - the debug kernel outgrew the BIOS partition, so the disk now carries stripped files;
  - exiting threads leaked their process;
  - a lock check gave a false alarm;
  - two endpoint races, a deadlock and a lost deadline, were found in review and fixed
    before they could show up;
  - the interface test's exact-bytes check caught options being laid out differently from the
    note, before anything depended on it;
  - the budget check could be confused by earlier test programs still being torn down.
- Updated notes 0001, 0002 and 0007 to 0009 with what building taught us (for example,
  the kernel now copies through programs' mappings instead of switching SMAP off), and added
  `LICENSES/`.

## Session 9: 31 October to 2 November 2026, milestone M4 (devices and drivers)

**Chris** asked for M4, again with the key decisions walked through one at a time before any
design notes or code, and tests kept small but never at the cost of quality. There were eight:
how drivers get hardware access, DMA and the IOMMU, what the kernel keeps and what the device
manager does, how drivers are found and started, the driver framework and class interfaces,
crash and restart, handing over the screen, and the test plan. He asked Claude to go ahead as
it thought best, then approved notes 0010 to 0012 ("if you're happy with the notes, please
build"). Stages 1 to 4 were built with Claude Sonnet 5.5; Chris then switched to Claude Opus
5.5, which reviewed them and did stage 5.

**Claude:**
- Wrote notes 0010 (device access), 0011 (the device manager, drivers and interfaces) and 0012
  (crash and restart, and the tests). They say plainly that until an IOMMU is used, a driver
  for a DMA-capable device is contained when it crashes but not when it is hostile.
- **Stage 1, device access in the kernel:** device memory, interrupts and I/O ports as range
  capabilities (`lib/ranges`, host-tested against a model), DMA memory, I/O APICs and
  message-signalled interrupts on x86-64, GICv2 lines and the GICv2m frame on ARM64, interface
  version 1.1, the machine description, and the hardware misuse tests in `probe`.
- **Stage 2, the device manager and the first driver:** `lib/devices` (PCI configuration
  space, MSI-X, manifests, the device record, all tested with hostile inputs), `lib/virtio`
  (tested against a software device, including one that lies), `lib/driver`,
  `services/devmgr` and `virtio-blk`, with a block test on a generated test disk.
- **Stage 3, crash and restart, M4's "done when":** the device manager revokes, switches the
  device off, zeroes its DMA memory and restarts the driver, backing off and then giving up.
  The crash test crashes the disk driver four ways and stops it from outside, 20 more times to
  look for leaks, ten times under load, and four times quickly. Two kernel bugs turned up: a
  timed wait kept its timer (and a dead driver's whole budget) until its deadline, and emptied
  page tables were never freed.
- **Stage 4, the other drivers:** `virtio-input`, `framebuffer`, `ps2` and `serial16550`, the
  input, display and serial interfaces, key tables, and the device self-test. The test runner
  now types keys and moves the mouse through QEMU's control socket, and reads COM2's output
  from a file.
- **Stage 5, review and documents:**
  - all four configurations pass;
  - review found that the page-table fix freed tables before the TLB flush that made them
    unreachable, which real hardware with cached table walks could trip over; they're now
    freed after it;
  - the register bounds rule had no host tests, because `lib/driver` only builds for
    PrettyBigRock; it moved to `lib/devices` and is tested there;
  - added a Stress workflow for GitHub;
  - brought notes 0001, 0002 and 0005 to 0012, README and ROADMAP up to date.
- **The rare ARM crash.** During stage 3, one ARM boot in about 40 had panicked after the
  crash test, with its report cut off. Stage 5's stress runs caught it twice more:
  - the boot-test runner was taught to ask QEMU for every core's registers and stack when the
    kernel stops;
  - the dumps showed two cores running on the same kernel stack;
  - the cause was a race in the scheduler's wake-up protocol that could queue a thread twice;
  - a per-thread scheduling lock fixes it, and new debug checks catch any thread run on two
    cores at once;
  - with the race's windows deliberately widened, the old code failed within seconds and the
    fixed code passed.

## Session 10: 2 November 2026, milestone M5 (files and sandboxes)

**Chris** asked for M5, starting as earlier milestones did: design notes first (the file layer
and each app's file tree, the block cache, RockFS, the sandbox manager, manifests and users),
then the work broken into stages, with the plan shown to him before any code. He asked that
Claude Opus do the foundational and security-critical work itself (the notes, the file layer,
the block cache, the sandbox manager, manifests, users, and anything that hands out or checks
capabilities), and hand well-defined work that follows a pattern to Sonnet subagents (FAT32,
the text shell, porting RockFS once its design is settled, extra tests), with Haiku only for
trivial jobs such as summarising long test logs, reviewing everything against the tests before
it goes in. Comments are to be written for a reader who knows programming but is new to Rust
and OS internals. RockFS is to be cross-checked against `tools/rockfs.py` and tested with real
disks made by 1.3, including writing and reading back.

**Claude:**
- Added `CLAUDE.md`, the working rules every session reads first (who does which work, how
  subagents are briefed and reviewed, how to comment, building and testing), and the commenting
  rule to note 0002.
- Drafted notes 0013 to 0018:
  - 0013, kernel interface 1.2: replies that hand out a capability derived from the one the
    caller used (so revoking a folder revokes the files opened through it), watched badges (so a
    service is told when an object's last capability goes), tied processes (which end with
    their creator), and the wall clock for programs;
  - 0014, files: file and folder capabilities, two rights, `pbr.file`, durability (an atomic
    save on RockFS), and each program's file tree, kept in its own library;
  - 0015, storage: partitions, volumes, `pbr.storage`, and the shared block cache, with its
    ordering rules;
  - 0016, filesystems: RockFS in Rust with flushes where order matters, FAT32, and four layers
    of checks against `rockfs.py`, 1.3's own code, and a real disk made by booting 1.3;
  - 0017, users, a sandbox manager per session, app manifests and rules;
  - 0018, supervision (including the device manager's death and its DMA memory), the console
    and shell, the tests, and nine stages with who builds each.
- Chris accepted notes 0013 to 0018 ("happy for you to go ahead").
- **Stage 1, kernel interface 1.2** (Opus): watched badges (call 58 and receive kind 3), derived
  capabilities in replies, tied processes, and the wall clock in the system data page; their
  support in `granite` (`mint_watched`, `Event::Gone`, `CapSend::derived`, `Spawn::tied`,
  `time::wall_now`) and in generated code (`Reply::ok` takes `impl Into<OutCap<T>>`). The root
  program's new `kernel12` tests, with `echo`'s new DERIVE and GONE operations and `probe`'s
  `tied` mode, pass on all four configurations. Building it found that the system page's header
  still said interface 1.0 under the 1.1 kernel; it is now written from the kernel's one version
  constant. Notes 0007, 0008, 0013 and 0018 updated.
- **Stage 2, storage** (Opus): `lib/crc`; `lib/partition` (MBR, GPT with its backup, whole-disk
  volumes, identifying RockFS, FAT32 and empty volumes by their contents, damaged tables
  refused), host-tested with every kind of damage and random corruption; `lib/blockcache` (the
  shared cache, with note 0015's ordering rules), host-tested against a simulated disk with its
  own write cache; `services/storage`, serving `pbr.storage`, each volume's `pbr.block`, and
  `pbr.storage.debug` in self-test runs; three test disks made by the build tool and
  `tools/rockfs.py`; and the storage self-test, which passes on all four configurations,
  including a disk driver crashed underneath a volume in use. Building it corrected note 0015:
  partitions that don't start on a 4 KiB boundary do share cache pages, so flushing a volume
  writes every dirty page touching it. Deliberately breaking that rule showed the random cache
  test missed it, so a test aimed at shared pages was added. Not yet: filesystem servers, so
  volumes can't be opened or formatted (stages 3 to 5).
- **Stage 3, the file layer** (Opus): `lib/fs` (the `Filesystem` and `BlockDevice` traits every
  filesystem implements); `pbr.file`, `pbr.fs.host` and `pbr.fs.control`; `lib/fsserve`, the
  half of every filesystem server that deals with capabilities (the object table of watched
  badges, rights that only shrink, paths that only go down, derived capabilities in replies,
  records freed by gone notices, the last writer's close committing), host-tested with
  `memfs`, an in-memory filesystem, and a hostile client; `lib/filetree` (paths, the start-up
  format, finding which entry a path is in) and `lib/files` (each program's file tree and its
  API); `Spawn::file_tree` in `granite`. The root program's files self-test starts `memfs` and
  checks `pbr.file` end to end, that records return to their resting count, and that revoking a
  folder takes what was opened through it; `fileprobe`, started with a tree, checks every handle
  it holds was given to it, that walking its tree finds exactly what it was given, that no path
  (`..` included) gets out, that read-only stays read-only, and that it loses a folder revoked
  while it runs. All pass on all four configurations. Building it moved the program half out
  of `granite` into `lib/files` (the interface code is built on `granite`), and found that a
  tree's description arrives padded to a whole page. Notes 0001, 0014 and 0015 updated. Next:
  RockFS (stage 4a, Sonnet) and its server and the 1.3 data disk (stage 4b).
