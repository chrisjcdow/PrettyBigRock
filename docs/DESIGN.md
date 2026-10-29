# PrettyBigRock 2: design

Status: **draft 2**, 29 October 2026. These decisions were agreed between Christopher
Cameron-Dow and Claude. The plan for building it is in [ROADMAP.md](ROADMAP.md).

PrettyBigRock 1.x began as a quick experiment: a 32-bit kernel in C where every app is part of
the kernel. It works and it's fun, but it can't grow into an operating system you'd use every
day. PrettyBigRock 2 is a fresh design aimed at that. It's planned for the long term, so that
the decisions that are expensive to change are made carefully, at the start, and written down.

This document says *what* we decided and *why*. Each major part gets its own detailed design
note (like [ROCKFS.md](ROCKFS.md)) before it's built.

---

## 1. Goals

1. **Usable.** It boots on real computers and does real tasks well enough that you'd choose to
   use it.
2. **Secure by design.** Every program, including every driver, runs in a sandbox and can
   reach only what it has been given.
3. **Portable.** It runs on x86-64 and ARM64 from the start, and adding another processor
   family is a contained, documented job.
4. **Our own.** Written from scratch. Anything borrowed is temporary, sits behind our own
   interface, and is listed with a plan to replace it (section 14).
5. **Robust.** A crashing app or driver doesn't take the system down, and a power cut doesn't
   lose the disk.
6. **Documented.** Design notes come before code and are kept up to date as it changes.

7. **General purpose.** As a desktop and as a home server, PrettyBigRock should do what Chris
   uses computers for today (section 1.1).

### 1.1 What it's for

The aim is everyday use, not a demo. This table shows what each use needs from the
system, which decides what the design must leave room for now.

| Use | What it needs from PrettyBigRock |
|-----|----------------------------------|
| Web browsing | A modern browser engine: porting one, which needs a very complete POSIX layer and C++, plus networking, TLS, fonts and text shaping, and eventually GPU acceleration. |
| Media: music, video, photos | Audio, video decoding (and later hardware-accelerated decoding), and a capable compositor. |
| Personal documents | A word processor and spreadsheet, either ported (an office suite) or our own. Fonts, printing, PDF. |
| Software development | Editors, `git`, compilers. Eventually PrettyBigRock builds itself on PrettyBigRock (self-hosting), which means porting LLVM and Rust. |
| Music production and notation | Low-latency audio with real-time scheduling, MIDI, USB audio devices, and ported apps such as MuseScore (which needs Qt). |
| Recording and editing video | Cameras, USB capture devices (including for digitising VHS), and video encoding. |
| A home server like Roxy | Running without a screen, background services, a web server, file sharing, remote administration (SSH), mirrored disks, and the Linux server software Roxy runs today: Docker containers, Plex, Jellyfin, Nextcloud. |
| Distributed computing | Porting BOINC (C++). |
| Notes and knowledge (an Obsidian vault) | Plain Markdown files work anywhere; the Obsidian app itself is Electron, which effectively means porting Chromium. |
| Emulators and retro computing | Ported emulators (DOSBox, QEMU), usually through SDL. |
| Local AI | GPU compute: the hardest item on this list, since the major GPU vendors' compute stacks are tied to Windows and Linux. |

What follows from the table:
- **Most of these come from porting existing software, not rewriting it.** No project can
  write a browser, an office suite and a music production app as well. So the POSIX layer
  has to be good enough to port large programs, and porting the common toolkits
  (SDL, then Qt) unlocks hundreds of apps at once.
- **The "write everything ourselves" rule (D13) applies to the operating system itself.**
  Applications ported from elsewhere are welcome. They're software that runs on
  PrettyBigRock, not parts of it. They live in a separate ports collection, each under its
  own licence.
- **Running unmodified Linux programs** becomes a real need (Docker, Plex), rather than a
  non-goal. See open question Q7.
- **GPU support** can't stay "long-term" for ever: browsers, video and AI all need it. The
  graphics interface is designed from the start so GPU drivers can be added later
  (section 8).
- **Real-time audio** is a requirement, not a nice-to-have (section 6.1).

### Non-goals

- **Being Linux, or a Unix clone.** Linux already exists and is great. PrettyBigRock has its
  own design and offers Unix compatibility as a layer on top (section 9).
- **Running Windows programs.**
- **Supporting all hardware.** We support chosen machines well and add more over time
  (section 11).

---

## 2. Principles

These settle arguments when a design question comes up.

1. **Nothing is reachable without a capability.** There are no global names in the kernel: no
   global file paths, process IDs anyone can signal, or devices anyone can open.
2. **The kernel does only what can't be done anywhere else.** If something can run as an
   ordinary program, it does.
3. **Interfaces are contracts.** System calls, message formats and disk formats are
   versioned, documented before they're built, and changed only with a migration plan.
4. **Processor-specific code lives in one place** (section 10). Code outside it never
   assumes which processor it runs on.
5. **Choosing is granting.** Permission comes from what the user does (picking a file,
   dropping a picture), not from pop-ups.
6. **Fail small.** A component that fails is restarted or reported. It doesn't spread its
   failure.
7. **No hidden dependencies.** Anything borrowed is behind our own interface, so it can be
   swapped out without changing anything that uses it.

---

## 3. Decisions

| # | Decision | Why |
|---|----------|-----|
| D1 | **64-bit only**: x86-64 and ARM64 | Modern hardware; no 4 GB limit; both are long-lived. |
| D2 | **Multiple processor families** through one architecture layer | The Pi 5 from the start; RISC-V and others later. |
| D3 | **UEFI and BIOS** on x86, through a shared hand-over to the kernel | UEFI for modern PCs; BIOS for older machines such as Roxy. BIOS can be dropped later by deleting one component. |
| D4 | **Microkernel** | Drivers and services become sandboxed programs; crashes are survivable; the processor-specific part stays small. |
| D5 | **Capability-based security** | Every app is sandboxed by construction; a sandbox manager decides what each app gets. |
| D6 | **Users and capabilities** are separate | Users answer whose files these are; capabilities answer what this app may touch. |
| D7 | **Native PrettyBigRock interface**, with a **POSIX layer** on top | Our own identity, and Unix software can still be ported. |
| D8 | **Rust**, with no third-party packages | Memory safety for a large codebase; one language across architectures. |
| D9 | **Multi-core and multi-threaded** from the first line | Retrofitting several cores onto a one-core kernel is notoriously painful. |
| D10 | **Multiple filesystems** behind one file layer; RockFS is native | USB sticks, SD cards and CDs use FAT, exFAT and ISO 9660. |
| D11 | **The GUI is a program, not part of the kernel** | Boots to the desktop by default; also boots text-only for recovery, new hardware or a headless Pi. |
| D12 | **Real hardware**, on chosen target machines | "Usable" means real machines. |
| D13 | **Write the OS ourselves**; anything borrowed is temporary. Ported apps are welcome (section 1.1) | Control over the whole system; no dependence on outside code. |
| D14 | **Documentation first** | Every major part has a design note before code. These notes are the project's memory between sessions. |
| D15 | **GPL-3.0-or-later**, copyright Christopher Cameron-Dow | Stays open source. |
| D16 | **A linking exception for app libraries** (section 9.1) | So that anyone, including companies making proprietary software, can write apps for PrettyBigRock. The kernel and system services stay plain GPL. |
| D17 | **The 1.x C system moves to `classic/`** and stays buildable and releasable | One repo and one history; the playable version stays available while 2.x is built. |
| D18 | **The browser player stays on 1.x** | v86 only emulates 32-bit x86. A 64-bit browser version can come later. |
| D19 | **General purpose**: desktop and home server (section 1.1) | Chris wants to use it for what he uses computers for today. |

---

## 4. The overall shape

```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  Apps: Notepad, Paint, Files, Terminal, ported Unix programs (via POSIX)  │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  System services (ordinary sandboxed programs):                            │
  │    sandbox manager · file picker · clipboard · desktop/compositor · input  │
  │    file layer · RockFS / FAT / exFAT / ISO servers · network stack · audio │
  │    device manager · drivers (one program each) · logging and tracing       │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  Microkernel: address spaces · threads · scheduling · messages ·           │
  │               capabilities · interrupts · timers                           │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  Architecture layer: x86-64 · ARM64 · (later: RISC-V …)                    │
  ├──────────────────────────────────────────────────────────────────────────┤
  │  Boot: UEFI loader · BIOS loader · Pi firmware  →  one shared hand-over    │
  └──────────────────────────────────────────────────────────────────────────┘
```

**Starting up:**
1. A loader puts the kernel and the **root program** (init) in memory and passes over the
   **boot information**.
2. The kernel sets itself up and gives the root program capabilities for all of memory, all
   devices and all interrupts.
3. The root program starts the device manager, drivers and services, giving each only what it
   needs.
4. It then starts the desktop, or the text console.

---

## 5. Booting

All loaders produce the same **boot information** structure, defined in its own design note
and versioned. It contains:
- the memory map;
- the framebuffer, if there is one;
- where to find the hardware description (the ACPI tables on PCs, the device tree on ARM);
- the command line;
- the kernel and root program images, plus any other start-up files.

| Loader | Used on | Notes |
|--------|---------|-------|
| UEFI application | Modern PCs, QEMU with OVMF, ARM64 machines with UEFI | The main path. |
| BIOS loader | Older PCs (Roxy), QEMU with SeaBIOS | Small; builds on what 1.x already has. |
| Pi firmware | Raspberry Pi 5 | The Pi's own firmware loads the kernel directly; a UEFI port for the Pi also exists. |

The kernel never knows which loader started it.

**Start-up files** are one simple archive format of our own. It holds the root program,
early drivers and the services needed to reach the disk, and it's the only thing the kernel
unpacks.

---

## 6. The kernel

### 6.1 What the kernel does

- **Address spaces**: creating them, mapping memory into them, and the processor's page
  tables (through the architecture layer).
- **Threads and scheduling**: pre-emptive scheduling across all cores, with a run queue per
  core and priorities. A real-time class for audio and similar work is planned.
- **Messages** between programs (6.3).
- **Capabilities** (6.2).
- **Interrupts**: delivered to the driver that holds the capability for them.
- **Timers and time**: a 64-bit monotonic clock in nanoseconds; wall-clock time in UTC.
  Time zones are handled outside the kernel.
- **Real-time scheduling**: a scheduling class with guaranteed time slices for threads that
  can't afford to be late, such as audio for music production. It's designed in from the
  start because scheduling is very hard to change later.
- **Room for virtual machines**: a kernel object for a virtual CPU, using the processor's
  virtualisation features (Intel VT-x, AMD-V, ARM EL2), so that PrettyBigRock can later run
  other operating systems as guests (open question Q7). Not built early, but the kernel's
  object and scheduling design must allow it.
- **Starting the other cores.**

### 6.2 What the kernel doesn't do

Drivers, filesystems, networking, graphics, users, logins, policy, and file names of any
kind. All of these are ordinary programs.

### 6.3 Kernel objects and capabilities

Everything the kernel manages is an **object**:
- address space;
- thread;
- memory object (a range of memory that can be mapped, shared or copied on write);
- channel (one end of a message connection);
- notification (a lightweight signal);
- interrupt;
- device memory range;
- I/O port range (x86 only);
- timer.

A program refers to objects through **handles**: small numbers indexing its own capability
table. A capability is a handle plus a set of **rights**, such as read, write, map,
duplicate and transfer.

Rules that are cheap now and expensive later:
- **Reduced copies.** A capability can be duplicated with fewer rights, for example turning
  read-write into read-only, but never with more.
- **Revocation.** Every capability records the one it was derived from. Revoking one revokes
  everything derived from it, wherever those copies went. This is designed into the kernel's
  data structures from the start.
- **Moving capabilities in messages.** This is the only way a program gets new access.
- **No forging.** Handles are meaningful only inside the program that holds them.

### 6.4 Messages

Messages are the heart of a microkernel, and their design is a long-term contract.

- **Small messages.** Synchronous, with a fixed-size body plus up to a few capabilities.
  Used for requests and replies, and built to be fast (L4-style direct switching from caller
  to receiver).
- **Bulk data.** Shared memory with ring buffers between the two sides, plus notifications to
  say "there's more". Disk, network, audio and graphics use this, so their data is never
  copied through the kernel.
- **An interface language.** Services describe their messages in **interface files**: a
  small language of our own from which the build generates Rust code for both sides. Every
  interface has a name and a version. These files are also the documentation of how the
  system's parts talk to each other.

### 6.5 System calls

- **A small set.** Roughly creating objects, operating on handles, sending and receiving
  messages, mapping memory, waiting, and threads. Everything else goes through messages to
  services.
- **Handles only.** Every system call that touches an object takes a handle. None takes a
  path or a global ID.
- **Versioned.** System call numbers are never reused. Changes add new calls, old ones are
  retired on a published schedule, and the interface version is readable at run time.
- **Called through our library.** Programs call the system through the native library, not
  directly, which leaves room to change how calls are made underneath.

### 6.6 Memory

The kernel needs:
- a physical page allocator that knows about NUMA, the memory layout of multi-socket
  machines (used on larger machines, harmless on small ones);
- page tables per address space;
- copy-on-write memory objects;
- shared memory.

Memory a program uses is charged to it, so one program can't exhaust the system.

### 6.7 Multiple cores

The kernel runs on every core from the start:
- fine-grained locks, with a documented order for taking them;
- a run queue per core, with work moved between cores;
- every structure designed for concurrent access from day one.

Programs get threads, and the native library offers locks and channels that are safe across
threads.

---

## 7. Security: users, capabilities and sandboxes

### 7.1 The model

- **Apps start empty.** A program starts with only the capabilities its launcher gives it.
- **The sandbox manager** launches apps and decides what each one gets. Its decisions come
  from:
  - what the app's **manifest** asks for (a small file shipped with the app);
  - **rules** the user has set, such as "Paint may always use /Pictures";
  - what the **user is doing at that moment**.

  Rules are stored. Capabilities are not: they exist only while the system runs and are
  handed out fresh each time an app starts.
- **The file picker** runs as part of the system, outside the app. The app asks for "a file
  to open". You choose one, and the app receives a capability for that file alone. Saving
  works the same way.
- **The clipboard, drag and drop, and "Open with…"** are system services too, so apps never
  talk to each other directly unless they've been connected.
- **Users** own files and settings. Each user has a home space. Logging in hands the desktop
  that user's capabilities, and the desktop passes slices of them to apps.
- **Trusted apps** (Terminal, backup and developer tools) can be given a user's full
  capabilities. This is explicit and visible, never a default.
- **The permissions viewer** shows what each running app holds and why it was given it.
- **Drivers** get only their own device: its memory ranges, its interrupt, and access to
  memory for data transfers, limited by the IOMMU (the hardware's memory guard for devices)
  where the machine has one.
- **The network is a capability too.** An app without one can't connect anywhere.

### 7.2 What each app sees as its files

Each app has its own **file tree**, assembled from the folders and files it has been given.
Notepad might see `/Documents` and the one file you opened. This makes the POSIX layer work
(section 9) and makes sandboxes understandable: what an app can see is exactly what it can
reach.

### 7.3 Known costs

- **Ported Unix programs** see only their sandbox, so some will need settings or a trusted
  launch.
- **Stopping and inspecting processes.** Programs can only be stopped or inspected through a
  capability for them. Debuggers and task managers are therefore trusted tools.
- **Prompt fatigue.** If you're clicking "Allow" all day, the design has gone wrong. Prompts
  are a last resort.

---

## 8. System services

Each of these gets its own design note before it's built.

- **Root program**: starts everything; holds the initial capabilities.
- **Device manager**:
  - reads the ACPI tables or the device tree;
  - scans PCI Express and USB;
  - matches each device to a driver and starts the driver with that device's capabilities;
  - restarts drivers that crash.
- **Drivers**: one program each, using a shared driver framework.
- **File layer**: joins filesystems, builds each app's file tree, and caches.
- **Filesystem servers**:
  - **RockFS**, the native filesystem, carried over from 1.x. Disks made by 1.3 stay
    readable, and `tools/rockfs.py` keeps working as the reference.
  - FAT32, exFAT and ISO 9660, then NTFS and ext4, read-only first.
- **Network stack**: Ethernet, IPv4 and IPv6, TCP, UDP, DHCP and DNS, then TLS.
- **Display server and compositor**:
  - each window is a shared buffer the app draws into;
  - the compositor puts the screen together;
  - it understands high-resolution screens (scale factors) from the start;
  - **several screens from the start**:
    - the desktop is one coordinate space with each screen placed in it;
    - each screen has its own resolution, scale factor, refresh rate and rotation;
    - a window moving between screens of different sharpness is redrawn at the right
      scale;
    - screens can be plugged in and removed at any time, with windows moved off a screen
      that disappears;
    - mirroring (the same picture on two screens, as with a projector);
    - a touch screen is tied to its own screen;
    - more than one graphics card;
  - the firmware's framebuffer usually drives only one screen, so real multi-screen
    support arrives with display drivers. virtio-gpu in QEMU can drive several screens,
    so the design is tested there from the beginning;
  - buffers are memory objects with fences (signals saying "drawing is finished"), so that a
    GPU can draw into them later without changing the interface;
  - the protocol is designed so that a Wayland-compatible front end (the protocol that most
    Linux desktop toolkits now speak) can be added, which would make porting GUI apps much
    easier (open question Q8).
- **Graphics acceleration**: GPU drivers come later, but the interface for them is planned
  with the display server. The first likely targets are GPUs with open documentation (Intel
  and AMD) and virtio-gpu in VMs.
- **Server and headless use**: starting services without a desktop, restarting them when they
  fail, logging, and remote administration.
- **Input**: keyboard layouts, mouse, touch, and later tablets and pens.
- **Audio**: mixing and low-latency playback and recording.
- **Sandbox manager, file picker, clipboard**: see section 7.
- **App installer**: installs apps with their manifests; a package format of our own.
- **Logging and tracing**: one place to see what every part of the system is doing,
  including messages flowing between programs. This is essential for debugging a microkernel.

### Text and fonts

- **Unicode everywhere.** UTF-8 in every interface.
- **Our own font rendering**: TrueType and OpenType outlines, anti-aliasing and scaling.
- **Shaping later.** Shaping is what joins letters correctly in scripts like Arabic and
  Devanagari; we build it afterwards.

The bitmap font from 1.x stays for the early text console.

---

## 9. Programming interfaces

- **The native library** (Rust) is the PrettyBigRock interface. It covers:
  - handles and messages;
  - files through capabilities;
  - threads;
  - the generated interface code;
  - windows and drawing (through the display service).
- **Rust standard library support.** Later we'll add PrettyBigRock as a target for Rust's
  standard library, so ordinary Rust programs can be built for PrettyBigRock.
- **The POSIX layer** is our own C library, written in Rust with a C interface. It turns Unix
  calls into native ones: `open` looks the path up in the app's own file tree, and sockets
  use the network capability. Unix programs are ported by recompiling.
- **`fork`.** The Unix way of starting programs, `fork`, copies a whole running program. It
  doesn't fit a capability system well. The POSIX layer supports `posix_spawn` properly and
  emulates `fork` as well as it reasonably can, which is a known limit.
- **Programs are ELF files**, the usual format for programs on Unix-like systems. A
  PrettyBigRock note inside each one marks it as ours and says which interface version it
  needs.
- **Porting large software** is a first-class goal of the POSIX layer. Its success is
  measured by what it can port: SDL, then Qt, then a browser engine. C++ support and its
  runtime library are part of the porting work.

### 9.1 Licences for apps (D16)

Apps link against the native library and the C library, and include code generated from
interface files. If all of that were plain GPL, every app would have to be GPL too, and
nobody would write proprietary software for PrettyBigRock.

So these app-facing parts carry the GPL **plus a linking exception**: an app may use them,
including linking them statically, under any licence it likes. Changes to the libraries
themselves stay GPL.

Covered by the exception:
- the native library;
- the C library and POSIX layer;
- the interface files and the code generated from them;
- the start-up code linked into every program.

Not covered: the kernel and the system services. Apps talk to those through messages, not by
linking, so the GPL doesn't reach the apps anyway, in the same way that programs on Linux
aren't covered by the kernel's GPL.

The exact wording will be settled before the native library is first published (milestone
M3). We'll prefer an existing, well-known exception over writing our own, since a recognised
text is better understood by developers and lawyers. Chris holds the copyright, so he can
grant the exception. (This is a plan, not legal advice.)

---

## 10. Portability

All processor-specific code lives in `arch/<name>/`. Each architecture provides one
documented set of functions:
- early start-up;
- page tables;
- switching between threads;
- system call entry;
- interrupt controllers;
- timers;
- starting other cores;
- reading the hardware description.

Machine-specific details, such as a particular board's interrupt controller or the Pi's
start-up quirks, live in `board/<name>/` underneath it.

Rules:
- **Outside `arch/`,** code doesn't assume a processor, its page size, or how many cores
  there are.
- **Little-endian is assumed** (all our targets are), but data on disk and on the wire is
  always converted explicitly.
- **Drivers for shared buses** (PCI Express, USB, NVMe, AHCI, virtio) are written once and run
  on every architecture.
- **Every change is built and tested on every architecture** automatically. An architecture
  that isn't tested quietly breaks.
- **Porting checklist.** `docs/design/porting.md` will list exactly what a new architecture
  needs. RISC-V is the planned third architecture, and the test of whether the layer is
  really clean.

---

## 11. Hardware

We support chosen machines well, then widen the list.

| Machine | What it needs |
|---------|---------------|
| QEMU (x86-64 and ARM64) | virtio disk, network, graphics and input; serial; UEFI and BIOS |
| Roxy (Intel H61, i5-2300, about 2011) | BIOS boot, AHCI (SATA), USB 2 (EHCI) keyboard, mouse and storage, onboard Ethernet, VESA framebuffer, ACPI |
| Raspberry Pi 5 | BCM2712, the RP1 chip (USB, Ethernet), its interrupt controller, SD card, framebuffer |
| A modern PC or laptop (perhaps Harriet) | UEFI, NVMe, USB 3 (xHCI), its graphics in framebuffer mode, ACPI |

**Graphics.** We use the framebuffer the firmware gives us. Accelerated graphics drivers are
a long-term aim, not a near one.

**Wi-Fi.** It's among the hardest hardware to support (each chip needs firmware and a complex
driver), so wired Ethernet comes first.

**ACPI.** Turning machines off properly, handling batteries and lids, and finding some
devices needs an interpreter for ACPI's own bytecode (AML). That's a big project of its own
and gets its own design note.

---

## 12. Storage and filesystems

- **Disk layout.** Partition tables (GPT and MBR) are read by the storage stack, not by
  filesystems.
- **RockFS** stays the native filesystem. Its spec, [ROCKFS.md](ROCKFS.md), has the feature
  flags we need for growth. The first planned additions are recording owners and permissions
  for users (D6) and support for mounted volumes.
- **One block cache** is shared by all filesystems. Write ordering stays the filesystem's
  responsibility, as copy-on-write requires.

---

## 13. Languages and tools

- **Rust** for the kernel, drivers, services, libraries and apps.
- **Assembly** only where the processor demands it, kept in `arch/`.
- **The Rust toolchain version** is pinned in the repo so builds are repeatable. Some kernel
  work needs features that only exist in Rust's nightly releases; each one we use is listed
  with a plan to move off it.
- **No third-party Rust packages**; only Rust's built-in `core` and `alloc` libraries.
- **The build** is driven by a small build tool of our own written in Rust, run through
  `cargo`. It builds every architecture, makes disk images and runs QEMU.
- **Python** stays for the host tools (`tools/rockfs.py`) and tests.

---

## 14. Borrowed parts

The rule is to write everything ourselves. Anything borrowed:
- is temporary;
- sits behind our own interface;
- appears in this list with its licence and a plan to replace it.

| Part | Used for | Licence | Plan |
|------|----------|---------|------|
| Rust compiler and its `core`/`alloc` libraries | Building | MIT/Apache-2.0 | Part of the language; not replaced. |
| QEMU, OVMF, SeaBIOS | Testing in VMs | GPL/LGPL/BSD | Tools, not part of the OS. |
| Raspberry Pi firmware | Starting the Pi 5 | Broadcom licence | Firmware the board needs; not part of the OS. |
| v86 emulator | Browser player for 1.x | BSD-2 | 32-bit only; see open question Q3. |

Nothing borrowed is part of PrettyBigRock 2 itself yet.

**Ported software** (applications and toolkits such as SDL, Qt, LLVM, a browser engine, or a
Linux guest in a virtual machine) isn't borrowed OS code. It goes in a separate ports
collection with its own list of licences, and the OS never depends on it.

---

## 15. How we work

- **Design notes** live in `docs/design/`, one per major part, written before the code.
  A note says the problem, the design, the alternatives considered, and what's still open.
  Disk formats, boot information, system calls and interfaces each get exact
  specifications.
- **This document** is the top-level map and is updated when a decision changes, with the
  change recorded in section 17.
- **The DEVLOG** records each session: who asked for what, and what Claude did.
- **Testing:**
  - unit tests run on the host;
  - QEMU tests on every architecture boot the system and run scripted checks;
  - GitHub Actions runs all of it for every change;
  - formats get cross-checks against independent implementations, as RockFS has with
    `tools/rockfs.py`.
- **Versions:**
  - the 1.x line (the C system) continues as it is;
  - PrettyBigRock 2.0 is the first release of this design;
  - interfaces carry their own version numbers, separate from the OS version.
- **Authorship.** The code is written by Claude and directed by Chris, and the repo says so
  plainly.

---

## 16. Open questions

Settled on 29 October 2026: Q1 (became D16), Q2 (D17), Q3 (D18) and Q4 (D19, section 1.1).

- **Q5. Cryptography.** TLS and disk encryption need cryptography. Writing our own is
  possible but risky to get wrong. The plan is to write it carefully against published test
  vectors and treat it as high-stakes code, with outside review if anyone offers.
- **Q6. A name for the kernel**, if it should have its own.
- **Q7. Running Linux programs unchanged.** Docker, Plex and other Linux-only software need
  it. There are two routes, and they aren't exclusive:
  - **A Linux compatibility layer**, translating Linux system calls into PrettyBigRock ones,
    as FreeBSD and the first version of WSL do. A microkernel can run it as a service. It's
    fast and integrated, but it has to chase Linux's very large interface, and Docker would
    also need Linux's container features.
  - **A virtual machine**: running a small Linux inside PrettyBigRock, as WSL 2 does. It's
    far more compatible (Docker just works), at the cost of being less integrated.

  Claude's suggestion: start with the virtual machine for server software, because it's the
  surest way to run Roxy's workload, and consider the compatibility layer later for
  individual desktop programs. Either way, the kernel leaves room for it now (section 6.1).
  Note that a Linux guest would be borrowed software running *on* PrettyBigRock, like any
  ported app, not part of the OS.
- **Q8. A Wayland-compatible front end for the display server.** It would make porting
  Linux GUI apps far easier, but it means following a protocol we didn't design. It's
  decided when the display server is designed (milestone M6).

---

## 17. Changes to this document

- 29 October 2026: draft 1.
- 29 October 2026: draft 2. Settled Q1 to Q4: the linking exception for app libraries (D16),
  `classic/` for 1.x (D17), the browser player staying on 1.x (D18), and general-purpose
  use (D19, section 1.1). From the uses, added: real-time scheduling, room for virtual
  machines, GPU-ready display buffers, headless use, porting as a goal of the POSIX layer,
  a clarification that D13 covers the OS rather than ported apps, and open questions Q7
  and Q8.
- 29 October 2026: added support for several screens to the display server design.
