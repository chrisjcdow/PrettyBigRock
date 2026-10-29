# 0001: Repository layout

Status: Accepted · 29 October 2026 · Related: DESIGN.md sections 4, 10, 13, 14; D17

## The problem

PrettyBigRock 2 will grow into many parts: loaders, a kernel, libraries, drivers, services,
apps, ported software, and tools. It runs on several architectures, and the 1.x system has to
keep working alongside it. Where things go needs deciding once, so that it stays tidy and
anyone can find their way around.

## The design

```
PrettyBigRock/
├── Cargo.toml            the Rust workspace: lists every Rust crate
├── rust-toolchain.toml   the exact Rust version (note 0002)
├── rustfmt.toml          code formatting rules
├── .cargo/config.toml    makes `cargo xtask` work
├── .github/workflows/    automatic builds and tests on GitHub
│
├── boot/                 loaders, one folder each (from M1)
│   ├── uefi/               UEFI loader, x86-64 and ARM64
│   ├── bios/               BIOS loader, x86-64
│   └── pi/                 Raspberry Pi start-up stage
├── kernel/               the microkernel
│   └── src/
│       ├── arch/<name>/    everything specific to one processor family
│       └── board/<name>/   everything specific to one machine (from M1)
├── lib/                  libraries, one folder each
│   └── bootinfo/           the boot information (note 0003)
├── drivers/              drivers, one program each (from M4)
├── services/             system services, one program each (from M3)
├── apps/                 apps (from M6)
├── interfaces/           interface files for messages (from M3)
├── ports/                software ported from elsewhere, each under its own licence (from M9)
├── xtask/                the build tool (`cargo xtask`)
├── tools/                tools for other computers, such as tools/rockfs.py
│
├── classic/              PrettyBigRock 1.x, the C system (D17)
│   ├── build.sh            builds classic/build/prettybigrock.img
│   └── run-prettybigrock.bat
│
├── docs/
│   ├── DESIGN.md           the top-level design
│   ├── ROADMAP.md          the milestones
│   ├── ROCKFS.md           the RockFS specification
│   └── design/             design notes, like this one
│
├── build/                not in git: what the build produces, by architecture
│   └── <arch>/<debug|release>/kernel.elf
└── target/               not in git: Cargo's working files
```

Rules:
- **One Rust workspace** for everything written in Rust, so one `Cargo.lock` and one set of
  lint settings cover it all.
- **Crate names start with `pbr-`** (for example `pbr-kernel`, `pbr-bootinfo`), except the build
  tool, which is `xtask` by convention.
- **Processor-specific code is only in `kernel/src/arch/`, `kernel/src/board/` and `boot/`.**
  Libraries, drivers, services and apps are written once for every architecture.
- **`ports/` is separate from everything else**, because ported software isn't part of the OS
  (DESIGN.md section 14) and carries its own licences.
- **Folders listed as "from M*n*" don't exist yet.** They're created by the milestone that
  first needs them, so the layout never has empty placeholders.

### Adding an architecture

The checklist, kept up to date as the architecture layer grows:

1. Add `kernel/src/arch/<name>/mod.rs`, providing every function listed in
   `kernel/src/arch/mod.rs`, and `kernel/src/arch/<name>/linker.ld` (copy an existing one: the
   layout is the same everywhere).
2. Add the `cfg` lines for it in `kernel/src/arch/mod.rs`.
3. Add it to `ARCHES` in `xtask/src/main.rs`, with its Rust target and ELF machine number.
4. Add it to `targets` in `rust-toolchain.toml`.
5. Make at least one loader start it (`boot/`), producing the boot information (note 0003).
6. Add it to the QEMU tests, so it's tested on every change.
7. Add a row to the hardware table in DESIGN.md section 11.

The build tool's own tests check steps 1 and 3 match.

## Alternatives considered

- **Separate repositories** for the kernel, drivers and so on. That makes changes that span
  several parts (which an early OS has constantly) painful, and versions hard to keep in step.
- **Keeping 1.x on a separate git branch.** A branch is easy to forget about and awkward for
  someone browsing GitHub. A folder keeps both in plain sight with one history.
- **A Makefile or shell scripts** instead of `xtask`. They work less well on Windows, and a
  Rust build tool is tested like the rest of the code.

## Changes
- 29 October 2026: first version.
