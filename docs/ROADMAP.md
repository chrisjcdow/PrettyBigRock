# PrettyBigRock 2: roadmap

Status: **draft 2**, 29 October 2026. This file goes with [DESIGN.md](DESIGN.md).

The roadmap is a series of milestones. Each one ends with something that can be shown working
and tested automatically. The order is chosen so that the decisions that are expensive to
change (the system calls, messages, capabilities, the hand-over from the boot loaders, and
portability) are settled and proven early, before a lot of code depends on them.

There are no dates. Each milestone is done when its checks pass and its design notes are
up to date.

PrettyBigRock 1.x stays available throughout as the playable version.

**Releases.** There are no PrettyBigRock 2 releases until the 2.0 alpha at M6, when it can do
everything 1.3 does. The suggested version scheme (not yet adopted in the code, which still
says 0.0.0) is `2.0.0-alpha.N`, then betas from around M7 or M8, then 2.0. The version is set
when the alpha is prepared.

---

## M0: Foundations
- Reorganise the repo, with the C system moved to `classic/` (D17).
- Pin the Rust toolchain; write the build tool; produce disk images for both architectures.
- Set up GitHub Actions to build everything and boot it in QEMU.
- Add the `docs/design/` template and the first notes: boot information, the repo layout, and
  coding rules.

**Done when:** an empty Rust kernel builds for x86-64 and ARM64 on every change.

**Status: done** (29 October 2026), apart from the first real run on GitHub, which happens
when this is pushed. Notes 0001 to 0003 are in `docs/design/`.

## M1: Booting, on both architectures
- A UEFI loader (x86-64 and ARM64), a BIOS loader (x86-64), and the shared boot information.
- The architecture layer's first pieces: early start-up, a serial port, the memory map.
- A text console on the framebuffer, using the 1.x font.

**Done when:** QEMU boots to "Hello from PrettyBigRock" through UEFI on both architectures and
through BIOS on x86-64, all tested automatically.

**Status: done** (29 October 2026). One disk image starts all three ways, and ARM64 also
starts from EL2 through the EL2 stub. `cargo xtask qemu-test` boots all four and checks the
kernel's report. Note 0004 describes the loaders and the disk.

## M2: The kernel core
- A physical memory allocator, page tables and kernel memory.
- Interrupts and timers; starting all cores; a scheduler with a run queue per core.
- Kernel threads running on every core, with lock checking in test builds.

**Done when:** a stress test runs threads across 4 cores on both architectures without
problems.

**Status: done** (29 October 2026). Every boot test now runs the kernel's self-test with
4 cores: heap, pages, exceptions, timing, and 64 threads sharing work across all 4 cores.
It passed 60 boots in a row, on all four configurations. Note 0005 describes the design.

## M3: Capabilities, messages and the first programs
- Design notes, before code, for the kernel objects and capability rules, the system calls,
  and messages and the interface language.
- Capability tables, rights, reduced copies and revocation.
- Synchronous messages, notifications, shared memory and ring buffers.
- The ELF loader, the root program, and the first version of the native library.

**Done when:** two separate programs exchange messages and capabilities, a program can't reach
anything it wasn't given, and revocation is shown to work, all tested.

**Design:** notes 0006 (objects and capabilities), 0007 (system calls), 0008 (messages and
the interface language) and 0009 (programs and `granite`), accepted 31 October 2026 and
updated with what building taught us.

**Status: done** (31 October 2026). The kernel runs programs in their own processes, with
capability tables, memory budgets, endpoints, notifications and shared memory, on both
architectures. It starts the root program from the start-up archive; programs are built on
`granite`, and services' interfaces are generated from `.pbi` files. Every boot test now runs
the program self-test after the kernel's: messages and capabilities between two programs,
an interface's generated code, isolation, four kinds of fault, revocation, the last sender
signal, a crashing service, budgets, and 1200 calls from 4 threads to a service with 4
threads. Ring buffers are a library format in `granite`, tested on the host against a
hostile other side; drivers will be their first real users (M4).

## M4: Devices and drivers
- The device manager; PCI Express; reading ACPI tables and device trees.
- The driver framework, with drivers as programs and restarted when they crash.
- Drivers: serial, the framebuffer, PS/2 and virtio keyboard and mouse, virtio disk.

**Done when:** a driver can be deliberately crashed and is restarted while the system keeps
running.

## M5: Files and sandboxes
- The file layer, each app's own file tree, and the shared block cache.
- RockFS in Rust, reading and writing disks made by 1.3 and cross-checked with
  `tools/rockfs.py`.
- FAT32.
- The first versions of the sandbox manager, app manifests and users.
- A text shell.

**Done when:** a program sees exactly the files it was given, and a 1.3 data disk opens with
its files intact.

## M6: The desktop (2.0 alpha)
- The display server and compositor, the input service, and the window toolkit in the native
  library.
- Several screens from the start, tested with virtio-gpu driving two or more screens in QEMU.
- The file picker, the clipboard, and the permissions viewer.
- Rewrites of the 1.x apps for the new system: Files, Notepad, Paint, Terminal, Snake and the
  rest. The potato stays.

**Done when:** PrettyBigRock 2 does everything 1.3 does, with every app sandboxed. This is the
first 2.0 alpha.

## M7: Real hardware I, Roxy
- AHCI; USB 2 (EHCI) with keyboard, mouse and storage; Roxy's onboard Ethernet chip.
- An ACPI interpreter, enough to power off and to find devices.
- A bootable USB stick image.

**Done when:** PrettyBigRock boots on Roxy from a USB stick, without touching Roxy's own disks,
and runs the desktop with a USB keyboard and mouse.

## M8: Networking
- Ethernet, IPv4, IPv6, UDP, TCP, DHCP and DNS, with the network as a capability.
- Then cryptography and TLS (open question Q5).

**Done when:** PrettyBigRock gets an address on your network and fetches a page from a web
server.

## M9: POSIX and ported software
- Our C library, `posix_spawn`, and `fork` emulation.
- Porting a first set of programs, such as Lua, a small C compiler, and Doom.
- Rust standard library support.

**Done when:** unmodified C programs, recompiled for PrettyBigRock, run inside a sandbox.

## M10: Raspberry Pi 5
- The BCM2712 board layer, the RP1 chip (USB and Ethernet), the SD card, and the
  framebuffer.

**Done when:** the Pi 5 boots to the desktop.

## M11: A modern PC
- NVMe, USB 3 (xHCI), and the remaining ACPI needs (battery, lid, sleep on a laptop).

**Done when:** a current PC or laptop boots to the desktop and saves files on its own disk.

## M12: A platform for big software
- C++ and its runtime library; a fuller POSIX layer (threads, signals, memory mapping, sockets,
  terminals).
- Ports of SDL (games and emulators), then Qt (MuseScore and many desktop apps).
- A decision on a Wayland-compatible front end (open question Q8).
- Fonts with text shaping, and printing to PDF.

**Done when:** a Qt app, such as MuseScore, runs on PrettyBigRock.

## M13: Sound and music
- An audio service with real-time scheduling; HD Audio (built into most PCs), USB audio,
  and virtio sound.
- MIDI, including USB MIDI keyboards.

**Done when:** you can play a MIDI keyboard through a software instrument with no
noticeable delay.

## M14: A home server
- Services without a desktop, SSH, a web server port, file sharing, and mirrored disks.
- A virtual machine service running a Linux guest for Docker, Plex and Jellyfin (open
  question Q7).
- Reading and writing ext4, so existing data disks can be used.

**Done when:** Roxy's current workload runs on PrettyBigRock.

## M15: The web and media
- A port of a browser engine; video decoding; GPU drivers, starting with Intel.

**Done when:** PrettyBigRock browses ordinary websites and plays video smoothly.

## Beyond
- self-hosting: building PrettyBigRock on PrettyBigRock (LLVM and Rust ports);
- an office suite, ported or our own;
- video capture and editing;
- BOINC;
- a Linux compatibility layer for desktop programs;
- more filesystems (exFAT, ISO 9660, NTFS);
- installing apps;
- logins for several users;
- disk encryption;
- Wi-Fi;
- RISC-V;
- GPU compute.
