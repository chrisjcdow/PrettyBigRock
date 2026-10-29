# 0002: Coding rules, the toolchain, and testing

Status: Accepted · 29 October 2026 · Related: DESIGN.md sections 13 and 15; D8, D13, D14

## The problem

A large codebase written over many sessions, by an AI that doesn't remember the previous
session, needs firm, written rules. Otherwise each session drifts a little and the code
becomes inconsistent. The rules have to be checkable by the build tool wherever possible,
so they don't depend on anyone remembering them.

## The design

### The toolchain

- **The Rust version is pinned** in `rust-toolchain.toml`, currently **1.97.0** (a stable
  release). With rustup installed, the right version and components are installed
  automatically the first time you build. The build tool refuses to run with any other
  version.
- **Changing the version** is done on purpose, in its own commit, with the reason recorded in
  the DEVLOG.
- **Kernel targets**: `x86_64-unknown-none` and `aarch64-unknown-none-softfloat`, Rust's
  built-in targets for code with no operating system underneath. "Softfloat" means the
  kernel never uses the floating-point registers, so it doesn't have to save them on every
  entry. Programs (from M3) will get PrettyBigRock targets of their own.
- **No third-party packages** (D13). Only Rust's built-in `core` and `alloc`, and for host
  tools, `std`. `Cargo.lock` should only ever list our own crates.
- **No unstable Rust features** in our code. If one becomes unavoidable, it's listed here
  with a plan to stop needing it.

#### The fallback: building `core` from source

Normally rustup downloads the kernel targets' core library ready-built. Where that's
impossible (Claude's workspace can't reach `static.rust-lang.org`), the build tool instead has
Cargo build the core library from Rust's own source:
- The source is the `library/` folder of the `rust-lang/rust` repository at tag `1.97.0`
  (commit `2d8144b7880597b6e6d3dfd63a9a9efae3f533d3`), installed where rustup would put the
  `rust-src` component.
- This uses Cargo's `build-std` feature, which is unstable, so the build tool sets
  `RUSTC_BOOTSTRAP=1` for those builds only.

`cargo xtask doctor` shows which way each target is being built. The result is the same code
built a different way, so the fallback can be dropped at any time without changing anything
else.

### Code rules

1. **Every file starts with an SPDX licence line** (`SPDX-License-Identifier: GPL-3.0-or-later`)
   and the copyright line. `cargo xtask check` enforces this.
2. **Formatting is automatic**: `cargo fmt`, with the settings in `rustfmt.toml`. `cargo xtask
   check` fails on unformatted code.
3. **Lints are errors.** Clippy runs with warnings treated as errors, for every architecture.
4. **`unsafe` is explained.** Every `unsafe` block has a `// SAFETY:` comment saying why it's
   sound (enforced by Clippy), and `unsafe` operations inside `unsafe fn` still need their own
   block (enforced by the compiler). Unsafe code is kept in small, well-named functions.
5. **Everything public is documented** (warned by the compiler). Comments say *why*, not what.
6. **Panics are bugs.** Kernel code returns errors rather than panicking on anything a program
   or device can cause. A panic in the kernel stops the system with a message.
7. **No processor-specific code outside `arch/`, `board/` and `boot/`** (note 0001).
8. **Sizes are explicit** in anything shared between programs or stored on disk: `u32` or
   `u64`, never `usize`, and `#[repr(C)]` with the layout checked at compile time, as
   `lib/bootinfo` does.
9. **Text is UTF-8**, and names are compared byte for byte unless a specification says
   otherwise.
10. **Plain English** in names, comments, messages and documents. Messages a user might see are
    written for people, not programmers.

### Testing

- **Unit tests** run on the computer doing the build: `cargo xtask test`. Anything that can be
  written as ordinary logic (formats, data structures, parsers) lives in a library that
  builds both for the kernel and for the host, so it can be tested here.
- **QEMU tests** (from M1) boot the real system on every architecture and check what it prints
  to the serial port.
- **Cross-checks**: every format gets a second, independent implementation (usually in Python,
  in `tools/`), and the two are tested against each other, as RockFS is.
- **`cargo xtask ci`** runs all of it, and GitHub Actions runs `cargo xtask ci` on every change.
  A change isn't finished until it passes.

### Commits and the DEVLOG

- Each commit does one thing, with a message saying what and why.
- Commits made by Claude carry a `Co-Authored-By` line for Claude.
- Each session adds an entry to `DEVLOG.md`: who asked for what, and what was done.

## Alternatives considered

- **Tracking Rust's nightly releases** for unstable features. Kernels often do this, but it
  means frequent breakage and a moving target. Stable Rust can now do almost everything a
  kernel needs, including inline assembly.
- **Using third-party crates** for common things (page tables, ELF parsing, UEFI). They'd
  speed up the start, but they're exactly the dependency D13 rules out.

## Changes
- 29 October 2026: first version.
