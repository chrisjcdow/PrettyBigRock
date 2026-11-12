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
- **Stage 4a, RockFS in Rust** (Sonnet wrote `lib/rockfs`, `rockfs-rs` and the cross-checks;
  Opus reviewed them, wrote the power-cut tests and `classic-disk`). `lib/rockfs` implements
  ROCKFS.md version 1 behind `pbr_fs::Filesystem`, with a flush wherever order matters, 89 host
  tests including hostile images; `rockfs-rs` has `rockfs.py`'s commands plus `stress` and
  `verify`. Rust, Python and 1.3's C code read and check each other's volumes, including 25
  images cut off mid-write by the C code. The power-cut tests use a disk with a write cache
  that loses a random half of what wasn't flushed: 300 runs in every test (5,000 once), each
  checking the volume holds exactly what it did just before or just after the operation that
  was cut. They found a real gap: a crash while a rename replaced a file could leave that name
  pointing at nothing, because nothing said which of an item's two names a check keeps. Fixed
  in the code and in ROCKFS.md (renaming within a folder is one folder change; across folders
  the item's `parent` decides). Removing either the fix or the flush before an inode write
  makes the tests fail. `cargo xtask classic-disk` made `tests/data/classic-1.3.img.gz` by
  booting 1.3 in QEMU and typing at it: folders three deep, a 70-character name, `write`,
  `append`, `cp`, `mv`, `rm`, a Paint drawing in 28 extents and its copy, a restart and a
  clean shutdown. The Rust code reads it exactly as its manifest says. One of four attempts
  left an empty file in `/Fill` that no folder named (harmless, and repaired by every check);
  the cause in 1.3 wasn't found, and the disk kept checks clean. Next: stage 4b, the RockFS
  server and the boot test with this disk.
- **Stage 4b, the RockFS server** (Opus). `services/rockfs` joins `lib/rockfs` to
  `lib/fsserve`; the storage service now starts one for every RockFS volume, formats volumes in
  memory and empty disks (1.3's MBR layout), hands out root folders with `open`, and shuts down
  by having every server save its volume. The boot tests found two real bugs on the way: a
  deadline set once and used much later (deadlines are moments, not lengths), and a deadlock,
  with the storage service waiting for a server's answer while that server waited to write
  through it. The fix is a rule: the storage service never waits for a server; it signals a
  stop notification bound to the server's endpoint and carries on serving. The boot tests
  attach a fresh, writable copy of the disk 1.3 made: the root program finds every one of its
  35 folders and files exactly as 1.3 left them, changes six things, and shuts down; afterwards
  `rockfs.py` and 1.3's own code both find the disk clean, holding exactly what was expected.
  That is M5's second "done when". Next: stage 5, FAT32 (Sonnet).
- **Stage 5, FAT32** (Sonnet wrote `lib/fat` and `services/fat`; Opus reviewed them and did the
  storage wiring and boot tests). 105 host tests check `lib/fat` against Linux's own tools:
  it reads what `mkfs.fat` and `mtools` write, and what it writes passes `fsck.fat -n` and
  reads back the same with `mtools`. The storage service now serves every FAT32 volume; the
  boot tests read the file the build tool put on the USB test volume, write long and
  non-ASCII names, a folder and a 200 KB file, rename and move, and the runner checks the
  volume afterwards with `fsck.fat -n` and `mtools`. On the way: the build tool's FAT32 writer
  lacked the root folder's label entry (`fsck.fat` objected); the BIOS boot partition grew to
  16 MiB, since a second filesystem server made the debug payload too big; and the RockFS
  server's disk client had a deadline set once at start-up, so every call failed a minute
  later. That was the second deadline bug, so generated clients now also take a timeout
  counted from each call (`set_timeout_ms`), and long-lived clients use it. Next: stage 6,
  users, the sandbox manager and manifests (Opus).
- **Stage 6, users, the sandbox manager and manifests** (Opus). `lib/manifest` reads every
  plain-text settings file with one line reader (driver manifests moved onto it): app
  manifests, `users.txt`, rules files, and the decision of what an app is granted, host-tested
  with ordinary, hostile and randomly damaged files. `services/users` finds the system volume
  (the first RockFS volume with `System/users.txt`; without one, or with a damaged one, a
  volume in memory with an `owner`), and starts a sandbox manager per session with that user's
  tree: `/Home`, their rules at `/System/rules.txt` (read only) and, for administrators, every
  volume under `/Volumes`. `services/sandbox` reads the manifest and the rules at each launch,
  derives a capability of the app's own for each grant and each chosen file, starts the app
  tied to itself, and serves `list`, `grants`, `revoke` and `stop`. The boot test (the root
  program standing in for the shell) launches `tests/programs/sandboxed` from its manifest in
  two sessions, on a system volume the build tool prepares, and the app checks from the inside
  that it holds exactly what it was granted: M5's first "done when" at the level of real apps.
  Decisions the note left open are under its Changes (rules files that can't be read refuse
  every launch; trusted apps also get `pbr.sandbox` and `pbr.users`; `grants` pages). On the
  way: `granite` kept at most 64 start-up capabilities and silently dropped the rest, which a
  trusted app with many volumes would have hit; it now keeps them all. ARM's self-test now
  takes about 330 seconds in this workspace (the new test about 85 of them), so a boot test
  may take 600 seconds instead of 300. Next: stage 7, supervision (Opus).
- **Stage 7, supervision** (Opus). The restart policy moved to `lib/supervise`. The root
  program now supervises the device manager, the storage service and the users service from a
  thread of its own (`services/root/src/supervisor.rs`), serving `pbr.supervisor`: services
  find each other through `lookup`, and the device manager has its DMA memory kept with
  `keep` until the next device manager has switched every device it drives off and called
  `release_kept`. The storage service finds the device manager again after it restarts and
  restarts crashed filesystem servers; the users service finds the storage service again,
  notices lost volumes, and restarts sessions. Two things building it taught: the root program
  had kept a receiving copy of each service's endpoint, so clients would never have seen
  `PEER_GONE` (endpoints are now moved to the services); and a capability to anything a dead
  process made keeps its whole budget taken, so a device manager restart needs room for two
  device managers while the old one's DMA memory is kept (both in note 0018's Changes, the
  second also an open question). Drivers are now tied to the device manager. The new
  supervision self-test crashes the system volume's RockFS server, the storage service and the
  device manager in turn; each comes back with what depends on it, the files read back intact,
  and the kept DMA memory is released only after the new device manager has switched the
  devices off. Next: stage 8, the console and the shell (Sonnet, with Opus on the screen
  hand-over and review).
- **Stage 8, the console and the text shell** (Sonnet wrote `lib/font`, `lib/terminal`, the
  US layout, `services/console`, `lib/shell` and `apps/shell` from self-contained briefs; Opus
  did the screen hand-over, the wiring and test 8, and reviewed). The screen hand-over found
  that `Spawn::log` gave every child the kernel log with all rights, so any service, driver or
  filesystem server could have taken the screen; now only the display driver holds `CONTROL`,
  passed from the root program through the device manager, and it takes the screen when the
  console first asks for the framebuffer. The console is a fourth supervised service; the users
  service gives its terminal to the first administrator's session, whose sandbox manager starts
  the shell on it and starts it again when it ends. The shell has the commands of note 0018
  section 6 (without `check`, left for later), with its logic host-tested in `lib/shell`. Test
  8 types twenty commands into the shell through QEMU's control socket, checks every answer the
  console mirrors to the serial port, and then checks the system volume with the Python tool.
  Review fixes: a failed `choose` left a launch waiting for good, so a few failed `run`s
  blocked every later one (the caller's own oldest launch now makes room); and a shell started
  again while its predecessor's `read_line` waited would have been refused (a newer
  `read_line` now takes over). After `shutdown` the users service no longer takes the closed
  volumes for a crash. Boot tests may now take 1200 seconds: with the shell test, ARM's
  self-test (and x86's, under emulation on a busy 2-CPU machine) takes over 600. A normal boot
  was checked by hand on x86-64 (BIOS): the users service logs `owner` in with the terminal,
  the shell starts, and `whoami`, `help`, `ls /` and `df` typed through QEMU show on the screen.
  (The console's first look for devices can time out while the device manager is still
  starting drivers; its watcher finds them a moment later.) Next: stage 9, the final review of
  M5.
- **Stage 9, documents and review** (Opus; Chris switched the session from Sonnet to Opus for
  it). Read what M5 added since `856e031`, with most care for everything that hands out,
  derives, checks or revokes capabilities: kernel 1.2's derived replies, watched badges and
  tied processes; `lib/fsserve`, `lib/files` and `lib/filetree`; `lib/manifest`'s decision;
  the sandbox manager, the users service, the storage service's volume capabilities, the
  supervisor and the screen hand-over. Two things changed:
  - **every capability given to a client now carries a badge.** The supervisor's lookups for
    the device manager and the console, and the endpoint each driver hands the device manager,
    were unbadged copies that could be copied, and a copy of an unbadged capability can be
    minted with any badge: harmless today, since those services don't tell clients apart, but
    a hole the day one does (the console's terminal reaches apps granted it);
  - **`sync` needs the write right**, so a program that can only read a file can't make another
    program's half-finished save durable (which would break RockFS's atomic save in a crash).

  Questions it raised, for Chris, are in notes 0017 and 0018: the system volume is the first
  RockFS volume with `System/users.txt`, so a USB stick could be taken for it once there is USB
  storage (M7); the sandbox manager answers one request at a time, so a trusted program can
  hold it up or fill its prepared launches; and one client can use all of a filesystem
  server's records. `run-prettybigrock.bat` gained a virtio keyboard on ARM64, since QEMU's USB
  keyboard there is one PrettyBigRock can't read yet (untested: batch files can't run in this
  workspace). Every M5 note is marked built with what building taught, notes 0001, 0006, 0008
  and 0009 have M5's changes, and README and ROADMAP describe M5 as done. Both "done when"s are
  met, as note 0018's Changes says how: test 6 (an app checking from the inside that it holds
  exactly its grants) and test 4 (the disk 1.3 made, compared file by file, changed, and found
  clean afterwards by `rockfs.py` and 1.3's own code). `cargo xtask ci` passed on all four
  configurations with the review's fixes. **M5 is done**; the Stress workflow's results come
  from Chris.

## Session 13, continued: 9 November 2026, a hang the Stress workflow found

- GitHub's CI on `a7e8f09` and `db890a0` failed now and then, and the Stress workflow on
  `d1a274e` failed 9 of its 33 boots, on every configuration, always in the same place: after
  the shell test, the users service didn't finish ending. Here (2 CPUs) about 15 boots never
  failed; GitHub's runners have 4. The supervisor's report (`d1a274e`) showed users' own
  threads had stopped; a new kernel report in self-test boots (`89acee3`) showed users closed
  down, with no thread or tied process of it left anywhere, yet never complete.
- Cause, in kernel 1.2's tied processes: a tied child's link to its creator was weak. Closed
  down in the order users, a sandbox manager, then an app, the sandbox manager was freed while
  waiting for the app (nothing held it once users had been closed down), so the app's
  "complete" went nowhere and users waited for ever. The link is now strong until the child's
  end is complete (note 0013, Changes). GitHub's CI and Stress now also install `dosfstools` and
  `mtools`, which they lacked, so the FAT32 checks run there too.

## Session 14: 9 November 2026, milestone M6 (the desktop, 2.0 alpha): the design

- Chris asked for M6 to start, as M5 did, with the design: clone main (`9345496`), make the
  `m6` branch, read the M5 material, and propose the notes M6 needs, the questions he must
  decide (with recommendations), which of M5's open questions M6 should settle, and the stages
  with their tests and what "done when" means in practice. The plan is in the Project
  (`claude/m6-plan.md`).
- Chris agreed with every recommendation (Q1 to Q16): our own display protocol shaped so
  Wayland can be translated to it later (DESIGN.md Q8); title bars drawn by the display server;
  window buffers allocated by the display server; a system-wide display server with a
  per-session desktop app; the desktop by default, `text` for the console, falling back to it;
  `lib/draw` and `lib/window` beside `granite`, `lib/sandstone` the toolkit; bitmap fonts at 1×
  and 2×; 1.3's look; US, UK and US-International layouts; each app's own folder; power off and
  restart in M6 (PSCI on ARM64, a stopgap `\_S5` scan on x86-64 until M7); virtio-gpu with two
  screens on x86-64 UEFI and ARM64 EL2, the firmware's screen on the other two; a second,
  desktop boot per configuration; `2.0.0-alpha.1`; the clipboard in the display server; only
  Terminal and Files trusted.
- Opus drafted notes 0019 to 0026 from those answers. One detail differs from the plan's Q10 as
  written: an app's own folder is at `/Home/Apps/<app>`, the path the user sees, rather than
  `/App`, to keep note 0014's rule that the same paths mean the same thing (note 0024, section
  4). Next: walking through the notes with Chris; then stage 0.
- Chris accepted notes 0019 to 0026 the same evening without walking through them ("I'm sure
  your recommendations are all fine") and asked for stage 0. DESIGN.md's Q8 is marked settled
  (note 0022, section 12).

### Stage 0 (Opus)
- **QEMU set-ups** (note 0026, section 6): x86-64 UEFI and ARM64 EL2 now also have a virtio-gpu
  with two screens (`max_outputs=2`) and a virtio tablet tied to each screen (`display=gpu2,
  head=0|1`); ARM64 EL1 has a tablet on its `ramfb` screen; x86-64 BIOS keeps PS/2. The
  firmware's screen (standard VGA, `ramfb`) stays in every configuration until the display
  drivers of stage 2: tried by hand first, OVMF and the ARM firmware still hand the loader that
  screen (the kernel and the console draw on it as before), QEMU's `screendump` with `device`
  and `head` takes a picture of each virtio-gpu screen (showing the firmware's text, untouched),
  and the device manager lists the virtio-gpu as having no driver. The tablets are given after
  the virtio mouse, since M4's driver test uses the first virtio pointer; for now
  `virtio-input` takes a tablet for a mouse and ignores its positions (stage 2). `-no-reboot`
  was already there. `cargo xtask run` gained `--two-screens`.
  - Two things the first CI run found: M4's driver test expected exactly two virtio input
    devices (it now tells QEMU's tablets apart by name and expects them on top, at most two;
    stage 2 gives tablets a kind of their own); and on ARM64 EL1 a tablet tied to the same
    screen as the virtio mouse took the mouse's button events (QEMU sends a screen's buttons to
    the last device tied to it), so there the tablet isn't tied to a screen.
- **Driver start-up in a release build** (M5's leftover): one normal boot each, 4 cores, the
  time from a driver's start to its `ready`, as the device manager logs it:

  | | debug | release |
  |--|-------|---------|
  | x86-64 UEFI (`virtio-input` ×2, `ps2`, `framebuffer`) | 1189 to 1370 ms | 12 to 41 ms |
  | ARM64 UEFI (`virtio-blk`, `virtio-input` ×2, `framebuffer`) | 79 to 863 ms | 12 to 28 ms |

  So the second or so that drivers take in test boots is unoptimised code under emulation, not
  something a real start would see; nothing to fix. (The console's first look for devices,
  which times out in debug boots, won't in release ones.)
- **Drawing optimised in test builds**: `lib/font` and `lib/terminal` get `opt-level = 2` in the
  dev profile (debug assertions stay on); `lib/draw`, the display server and the display drivers
  join as they are written.
- `cargo xtask ci` passed on all four configurations (the last runner change, which only
  touches ARM64 EL1, with both ARM64 boots, check and the host tests run again). The whole run
  took **23 minutes** here, not M5's 55: this time the machine was less busy, and the console's
  drawing is now optimised. CLAUDE.md says "about 25 to 55 minutes".

### Stage 1: kernel interface 1.3 (10 November 2026)
- Built by the session's model (Claude Fable 5.2, which Chris switched to), as CLAUDE.md asks
  for kernel work to be done by the main model itself, not a subagent.
- **Receiver watches** (`endpoint_watch_receiver`, call 59): each endpoint keeps a list of
  (capability node, notification, bits, charge), emptied and signalled when its last receiving
  capability goes. The hook that already runs when a capability leaves a table removes that
  capability's watch, which covers closing, revoking, moving and a process ending in one place.
- **The machine capability** (object type 12, right `POWER`, root's role 13) and
  `machine_power` (call 100): `kernel/src/power.rs` decides refusals, stops the other cores
  (counting them as they stop, 100 ms at most), then the architecture does it: PSCI on ARM64;
  on x86-64 the FADT's reset register (new in `lib/platform`, host-tested), port `0xCF9`, the
  keyboard controller, a triple fault. Turning a PC off answers `NOT_SUPPORTED` (the power
  service's job, stage 6).
- `granite`: `Endpoint::watch_receiver`, `machine::{Machine, Power}`, version 1.3.
- **Tests** (`services/root/src/kernel13.rs`, in the program self-test on all four
  configurations): the watch firing when the last receiver is closed, revoked, or its process
  ends, and at once on an endpoint with none; not firing early; its charge taken and given back;
  replacing; three clients; removed with its capability (closed, revoked, moved through the echo
  service and back); a waiting call failing with `PeerGone` as it fires; through a notification
  bound to an endpoint. The machine capability: its type and rights, and every refusal.
- **The self-test now ends by restarting the machine.** After `Program self-test passed` the
  root program calls `machine_power`; QEMU (`-no-reboot`) exits, and the runner accepts the boot
  only if the kernel logged `Restarting` and QEMU's `SHUTDOWN` event says `guest-reset`. So the
  kernel's old last line, the root program "exited with status 0", is no longer expected.
- Two mistakes in the new tests, found by running them (a leftover capability that could still
  receive; a revoke through a capability without the revoke right). QEMU itself crashed
  (SIGSEGV) once on x86-64 UEFI, just after the device manager started, as in session 11; the
  rerun passed. Worth watching now that those configurations have more devices.
- `cargo xtask ci` passed on all four configurations (36 minutes).
- **Stress after stage 1** (10 to 11 November): the first run on `e8eccf0` failed 1 boot of 120.
  ARM64 EL1, in the supervision test: the storage service was killed and started again
  ("supervisor: storage started again (1 restarts)"), and then nothing at all was printed for
  17 minutes, by the new storage service or anything else, until the runner's time limit. Two
  cores were in the kernel with interrupts on; two were idle. Every wait in the storage
  service's start has a time limit and says so when it runs out, so the silence points at the
  kernel, not at storage. The services involved don't use the new calls, and the stage 1
  changes to `endpoint.rs` and `caps.rs` were read again without finding anything that could
  loop, but that doesn't clear them.
  - The dump couldn't be read: the kernel GitHub builds isn't the same as the one built here
    (its loader reports 860 KiB against 1200 KiB here, cause not yet known), so its addresses
    can't be matched afterwards, and artifacts can't be downloaded from this workspace.
  - So the runner now names the kernel function for every address in a hung boot's registers
    and stacks, from the symbol table of the kernel it booted (`xtask/src/symbols.rs`, host
    tests; demangled when `llvm-cxxfilt` or `rustfilt` is there). `8443d13`; `cargo xtask ci`
    passed on all four (41 minutes).
  - Then: Stress on `m6` at `8443d13`, 120 boots, all passed; Stress on `m5` (`9345496`) as a
    comparison, 120 boots, all passed; 12 ARM64 boots here, all passed. So it is 1 hang in 252
    boots with stage 1 and none in 240 without (120 now, 120 on 9 November): too few to say
    whether stage 1 brought it or only met it. **Unexplained.** The next time it happens the
    log will say where each core is.

### Stage 2: screens (11 November 2026)

Built by the session's model (Fable 5.2), with two pieces by Sonnet subagents to written
briefs, both reviewed in full: the virtio-gpu driver and tablets in the virtio-input driver.

- **`pbr.display` version 2** (note 0020): screens with modes; `take`, which answers with a
  capability of its own that the drawing calls must come through; buffers the driver makes;
  `present` with the rectangles that changed; a notification when the picture is on the screen.
  The draft note had no way to tell the taker from any other client; building found that at
  once, and the taker's capability (derived, badged, watched) is the fix.
- **The rules are in `lib/devices/src/display.rs`**, tested on the host: who the taker is,
  which buffers exist, one picture at a time for each screen, rectangles inside the buffer,
  the pixel conversions, and the checked row spans the framebuffer driver copies by.
  `lib/driver/src/display.rs` answers the interface for any driver that implements `Backend`.
- **The framebuffer driver** makes buffers of ordinary memory and copies presented rectangles
  to the screen, converting to its format, so screens of three bytes a pixel work (M5's open
  question). **The console** draws in a buffer and presents the rows that changed.
- **Display buffers are DMA memory of their own**: a manifest key `buffers COUNT SIZE`, kept
  (zeroed) across a driver's restart; the kernel's limit on one piece of DMA memory is 16 MiB.
  Along the way: a driver's ordinary DMA memory is now handed over as a copy of the kept
  original, so revoking the driver's access no longer takes the supervisor's copy too.
- **The virtio-gpu driver** (subagent): two-dimensional, any number of screens, EDID, screens
  plugged in and out. It passed its first boot: every picture on both screens matched.
- **Tablets** (subagent): a virtio device with absolute axes is a tablet; `EVENT_POSITION`.
  The brief was wrong about where y goes in a 16-byte event (the subagent caught it): both
  coordinates are packed into `value`.
- **Tests.** The display self-test shows pictures on every screen and the runner compares
  QEMU's screenshots with them, pixel for pixel: a whole pattern while the kernel prints; one
  changed rectangle; a new client after the driver crashed; and on the virtio-gpu, both
  screens after the second was unplugged and plugged in again. The runner plugs that screen
  by being a VNC client (`xtask/src/vnc.rs`), since QEMU has no command for it. Each tablet is
  pointed with and must report exactly that place.
- **The UEFI and EL2 test machines now have only the virtio-gpu.** Test machines have 1 GiB
  (note 0026's Changes says why).
- **Two bugs found outside stage 2:**
  - *ARM64 at EL2 under newer firmware.* Chris ran the stress script on Impala (QEMU 10.2,
    EDK2 2025.11) and the EL2 boot stopped before the kernel's first line. His register dump
    showed the loader faulting for ever at its write to `HCR_EL2`: that firmware hands over
    with `E2H` on, which makes EL1 register names used at EL2 reach EL2's registers, so the
    loader was reprogramming EL2. Fixed in the loader and for the other cores (`dc6a364`;
    note 0003). Chris confirmed the boot on Impala.
  - *Storage giving up on a disk.* Stress on `7ec222c` failed 1 boot of 120 (ARM64 EL2): after
    the device manager was restarted, the storage service gave up reopening a disk on the
    first unexpected error, and the next write answered `DiskError`. It now retries anything
    but "disabled" for the 30 seconds already allowed (`5d92be4`).
  - The 17-minute hang of 10 November is still unexplained.

## Session 15: 12 November 2026, M6 stage 3 (`lib/draw`)

Chris asked for stage 3 as note 0026 defines it, with the same rules as before. Built by the
session's model (Claude Fable 5.2) with four pieces by Sonnet subagents, each to a written
brief, each reviewed in full.

- **The core, written first** (`lib/draw`: `canvas.rs`, `colour.rs`, `geometry.rs`,
  `damage.rs`): a canvas over `Bgrx` pixels with a clip, a scale and sub-canvases; colours
  mixed with exact rounding; rectangles that can't overflow; the list of changed rectangles.
  The subagents built on its few internal calls (a clipped row, a clipped run, "this
  rectangle changed"), which is what keeps every drawing call inside the clip.
- **`lib/testpng`** (subagent): reference pictures as uncompressed PNG files, read and
  written with our own code, cross-checked against Python's `zlib`. It passed review as
  written.
- **The font** (subagent): the 5 × 11 cell and 239 new characters, drawn as text in
  `lib/font/glyphs.txt`, with tests that every accented letter keeps its base letter's dots
  and that no two characters look alike by accident. Reviewed as pictures; ten glyphs redrawn
  (`Ð`, `Đ`, `ß`, `ð`, `ď`, `Ĳ` and the four curly quotes). The kernel's console and
  `lib/terminal` now use the 11 rows as the whole line.
- **Shapes, lines and flood fill** (subagent), **pictures and text** (subagent), side by
  side in the same checkout on separate files. Each rule is written twice: in the library,
  worked out a row at a time, and in the tests as a per-pixel model; they agree on tens of
  thousands of generated cases. The shapes subagent found a mistake in its brief (the
  shadow's square root must round up, or a few pixels outside each corner get full strength)
  and a faster thin line than asked for (where Bresenham's walk is after *k* steps has a
  closed form, so a line of any length costs only its visible part).
- **20 reference pictures** in `tests/pictures/draw/`, each at 1× and 2×, each looked at:
  rectangles, shapes, gradients, shadows, lines, flood fill, pictures, every glyph of the
  font, text styles, and `look-*`: a window as 1.3 drew it, made with the new calls.
- **What building changed in the notes** is in note 0023's Changes: a canvas is always
  solid; no anti-aliasing; smooth scaling by area; damage joined only when cheap; the font's
  extra punctuation; no heading font yet (sizes 2 and 3 and bold, as 1.3 had), left for Chris
  to judge when the desktop is on screen.
- `lib/draw` is also checked with Clippy for the programs' targets (it is in the build
  tool's list), since no program uses it until stage 5.
- **A flaw in the kernel's own self-test, found by the stage's first CI run.** One ARM64 EL1
  boot failed with "heap: 32 bytes not returned". The heap test (M2) compared the kernel
  heap's bytes in use before and after its own work, but other kernel threads use the heap
  meanwhile: since 9 November a thread wakes every second to watch for slow process ends, and
  a timer tick makes a short list of the threads to wake. The test now tries up to three
  times (a real leak shows every time). Nothing to do with drawing; the other three
  configurations passed that run.
- `cargo xtask ci` passed on all four configurations with that fix (about 55 minutes here);
  GitHub's CI passed on the same commit (`8a6273d`), and Stress was started on it (run
  43206630132), since the font's cell and the self-test both run at every boot. Last
  session's Stress run on `2de1a6c` (43072806182) had passed.

