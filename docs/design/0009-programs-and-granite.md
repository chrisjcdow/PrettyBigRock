# 0009: Programs, the start-up archive, the root program and `granite`

Status: Accepted · 31 October 2026 · Related: DESIGN.md sections 4, 5, 9 and 9.1; D16; notes
0003, 0004, 0006, 0007, 0008; milestone M3

## The problem

M3 is when PrettyBigRock 2 first runs something other than the kernel. That needs:
- a format for the start-up archive (left open by note 0003);
- rules for what a program file is, and who loads it;
- the root program, and what it starts with;
- the first version of the native library, `granite`;
- a way to build programs, which need a Rust target of their own;
- the licences of the app-facing parts (D16);
- a test proving M3's "done when".

The decisions were agreed with Chris on 31 October 2026 (decisions 8 and 10 of M3).

## The design

### 1. The start-up archive (format version 1.0)

A simple format of our own. All fields are little-endian.

**Header** (64 bytes, at offset 0):

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | Magic: `"PBRARCHV"` |
| 8 | 2 | Version, major (1) |
| 10 | 2 | Version, minor (0) |
| 12 | 4 | Number of entries |
| 16 | 8 | Total size in bytes, a multiple of 4096 |
| 24 | 4 | CRC-32 (the common polynomial, as in zip and PNG) of the whole archive, computed with this field set to 0 |
| 28 | 36 | Reserved, 0 |

**Entries** (64 bytes each, from offset 64):

| Offset | Size | Field |
|--------|------|-------|
| 0 | 48 | Name: UTF-8, padded with zero bytes |
| 48 | 8 | Offset of the file from the start of the archive, a multiple of 4096 |
| 56 | 8 | Size of the file in bytes |

**Rules** (a reader rejects an archive that breaks any of them):
- names are 1 to 47 bytes of UTF-8, with no zero bytes inside; `/` separates folders
  (`drivers/serial`); names are unique;
- files are in ascending order of offset, don't overlap, don't overlap the header or the
  entries, and end within the total size;
- the space between files is zero bytes;
- minor versions only use reserved fields where zero means "as before".

Files are page-aligned so programs can be used straight from the archive without copying.

**Where it's kept.** The build tool makes one archive per architecture:
- on the EFI system partition as `PBR/<arch>/startup.arc`;
- in the BIOS payload (note 0004) as a file named `startup.arc`.

Each loader loads it and passes it to the kernel as the module named `startup` (note 0003).
A missing archive is not fatal to the loaders: the kernel reports it and stops.

The archive code lives in `lib/archive` (`pbr-archive`), built for the kernel, for programs
and for the host. The build tool writes archives with it, and `tools/startarc.py` is an
independent reader for cross-checking.

### 2. Program files

Programs are ELF files. A program file must be:
- ELF64, little-endian, for the right machine (`EM_X86_64` or `EM_AARCH64`);
- a **static position-independent executable** (`ET_DYN`, with no `PT_INTERP`), so it can be
  loaded at any address;
- made of `PT_LOAD` segments that are page-aligned in the file and in memory, don't overlap,
  are never both writable and executable, and span at most 1 GiB in total;
- relocated only by `R_X86_64_RELATIVE` or `R_AARCH64_RELATIVE` entries (in `PT_DYNAMIC`'s
  `RELA` table); anything else is refused;
- marked with a non-executable stack (`PT_GNU_STACK` without execute);
- **without** thread-local storage (`PT_TLS`) for now (it comes with the C library, M9);
- carrying a **PrettyBigRock note** (`PT_NOTE`, owner name `"PrettyBigRock"`):
  - type 1, the interface version it needs: `u16` major, `u16` minor (note 0007, section 7);
  - type 2, optional: the main thread's stack size in bytes (default 256 KiB).

The checks live in `lib/elf` (`pbr-elf`), built for the kernel, programs and the host, and
tested on the host with valid and deliberately broken files. The loaders' existing kernel
loader (`boot/common`) moves onto it, so there's one ELF reader.

**Loading a program:**
1. Check the file against all of the above.
2. Create a process.
3. Choose a random base address. For each segment, create a memory object, fill it with
   `memory_write` (the file's bytes, then zeros), and map it at base + its address,
   writable for now.
4. Apply the relocations (each adds the base to a 64-bit value).
5. Change each segment to its final permissions (`memory_protect`): code read-execute,
   read-only data read-only, data read-write.
6. Create the stack (a memory object, mapped at a random address with an unmapped guard page
   below it) and the start-up block (section 4), and give the program its capabilities.
7. Create the main thread and start it.

### 3. Who loads programs

- **The kernel loads only the root program**, the file named `root` in the start-up archive,
  with the steps above.
- **Every other program is loaded by a program**, using `granite`'s program loader and
  ordinary system calls: the root program now, the sandbox manager later. This keeps file
  formats out of the kernel (principle 2) and lets each launcher decide exactly what a new
  program starts with.

### 4. The start-up block

Every program starts with a **start-up block**: a read-only page mapped into it, whose
address is the main thread's argument (note 0007, section 3).

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | Magic: `"PBRS"` |
| 4 | 2 | Version (1) |
| 6 | 2 | Number of handle entries |
| 8 | 4 | Offset of the arguments (UTF-8) from the start of the block |
| 12 | 4 | Length of the arguments in bytes |
| 16 | | Handle entries, 8 bytes each: `u32` role, `u32` handle |

Roles:

| Role | What the handle is |
|------|-------------------|
| 1 | The program's own process |
| 2 | Its main thread |
| 3 | The kernel log |
| 4 | The start-up archive, as a read-only memory object (root program only) |
| 5 | An endpoint to whoever started it (its parent) |
| 6 | Its message page's memory object |
| 1000 and up | Specific to the program, agreed with its launcher |

The arguments are a UTF-8 string, at most 3 KiB in total, so the whole block fits in one
page; anything larger is passed through a capability.

**The root program starts with:** its process, its main thread, the kernel log, the start-up
archive, and all of memory as its budget. Its arguments are the kernel command line, so it
can see `selftest`. From M4 it also gets capabilities for every device memory range,
interrupt and I/O port range, to hand to the device manager.

### 5. A Rust target for programs

The kernel's targets (`x86_64-unknown-none`, `aarch64-unknown-none-softfloat`) turn floating
point off, and Rust's x86-64 target can't turn it back on. Programs need floating point, and
their calling convention is a long-term contract, so they get **targets of their own**:
`x86_64-unknown-prettybigrock` and `aarch64-unknown-prettybigrock`. Each is a target
description file in `targets/`, based on Rust's own bare-metal targets with:
- the usual hardware floating point and vector registers (SSE2 on x86-64; FP and NEON on
  AArch64), and the standard calling conventions for each processor;
- position-independent code, linked as a static position-independent executable;
- `os = "prettybigrock"`, so code can tell it's running on PrettyBigRock.

Building for a target of our own needs two unstable Cargo features: target description files
(`-Zjson-target-spec`) and building `core` and `alloc` from source (`-Zbuild-std`). Note 0002
lists them, with the plan: **add PrettyBigRock as an official Rust target** with the Rust
standard library work in M9, after which programs build with stable Rust like everything else.

### 6. `granite`, the native library

`granite` (`lib/granite`, crate `pbr-granite`, used as `use granite::…`) is the bedrock every
program is built on. In M3 it provides:

| Module | What it does |
|--------|--------------|
| `sys` | The system calls, through the system page; the error type |
| `handle` | `Handle`, which closes itself when dropped; duplicating, revoking, rights |
| `endpoint` | Endpoints, calls, receiving, replying, badges, sessions |
| `notification` | Notifications, signal and wait |
| `memory` | Memory objects and mappings |
| `ring` | Ring buffers (note 0008, section 6) |
| `process` | Processes, and the program loader (sections 2 and 3) |
| `thread` | Threads, with their own stacks and message pages |
| `time` | The clock, deadlines and durations |
| `log` | Writing to the kernel log, with `println!`-style macros |
| `alloc` | A memory allocator for programs, so `Box`, `Vec` and `String` work |
| `start` | Program start-up: reads the start-up block, sets up the allocator, calls `main`, exits with its result; a panic ends the process with a message |

A program looks like this:

```rust
#![no_std]
#![no_main]

granite::main!(main);

fn main(start: granite::Start) -> i32 {
    granite::println!("Hello from a program");
    0
}
```

Rules for `granite`:
- it's `no_std` with `alloc`, and has no processor-specific code except the few lines that
  set the thread pointer, inside `sys`;
- every unsafe operation is wrapped, so a program using `granite` needs no `unsafe` for
  ordinary work;
- it never panics on anything another program can cause (a malformed message, a broken ring):
  those are errors.

### 7. Licences (D16)

Agreed on 31 October 2026, replacing the earlier plan of the GPL plus a linking exception:

| Part | Licence |
|------|---------|
| `granite`, the program targets, the C library (M9) and the start-up code linked into every program | **MPL-2.0** |
| Interface files and the code generated from them | **MIT** |
| Everything else (the kernel, services, drivers, tools, the interface compiler) | GPL-3.0-or-later |

The Mozilla Public License works file by file: anyone who changes a `granite` file must keep
that file open, but `granite` can be compiled into any program, statically and with its
generics and inlined code, under any licence. That's D16's intent, without depending on how
"linking" is read for Rust, where much of a library's code is compiled into the program
itself. MPL-2.0 is compatible with the GPL. (A plan, not legal advice.)

In practice:
- each file's SPDX line says its licence, and `cargo xtask check` knows which licence each
  folder must have;
- the licence texts go in `LICENSES/` (`GPL-3.0-or-later.txt`, `MPL-2.0.txt`, `MIT.txt`), and
  `LICENSE` explains which applies where.

### 8. New folders

| Folder | What |
|--------|------|
| `lib/captree` | The capability derivation tree (note 0006) |
| `lib/elf` | Program files |
| `lib/archive` | The start-up archive |
| `lib/granite` | The native library |
| `lib/interface-compiler` | The interface compiler (note 0008) |
| `lib/interfaces` | The generated interface code |
| `interfaces/` | Interface files |
| `services/root` | The root program |
| `targets/` | The program targets |
| `tests/programs/` | Programs used by the self-test |
| `LICENSES/` | Licence texts |

Note 0001 is updated to match.

### 9. The test: M3's "done when"

With `selftest` on the command line, the kernel runs its own self-test (note 0005) and then
starts the root program, which runs the **program self-test** and prints `Program self-test
passed` or the first failure. `cargo xtask qemu-test` waits for both, on every configuration,
with 4 cores. The program self-test checks:
- **Messages:** two programs (`tests/programs/echo` and `tests/programs/client`) exchange
  calls and replies: empty, full 240-byte, with the largest attached buffer, and with
  capabilities copied and moved; badges tell clients apart.
- **Isolation:** a program fails cleanly when it:
  - uses handle numbers it wasn't given (`BAD_HANDLE`);
  - uses a capability without the right (`NO_RIGHT`);
  - asks for a writable and executable mapping (`INVALID_ARGUMENT`);
  - passes bad addresses to system calls (`BAD_ADDRESS`).

  A program that writes to read-only memory, reads kernel memory, or runs an illegal
  instruction is ended with a fault record, and the system carries on.
- **Revocation:** the root program gives a program copies of an endpoint capability and a
  memory object, which it maps; revoking them makes its calls fail and its mapping
  disappear, while the root program keeps its own.
- **The last sender gone:** a service is told when its client exits; clients are told
  (`PEER_GONE`) when their service crashes.
- **Budgets:** a program with a small budget gets `OUT_OF_MEMORY` and nothing else is affected;
  its budget is returned when it ends.
- **Load across cores:** several clients on several threads call one service with several
  threads, across all 4 cores, and every call is accounted for.

Host tests cover the libraries (`captree`, `elf`, `archive`, `ring`, the interface compiler)
with ordinary and hostile inputs. The long stress runs (many boots in a row) run on GitHub
Actions.

## Alternatives considered

- **An existing archive format** (tar, cpio). Both work, but have more than we need, several
  dialects, and no page alignment.
- **The kernel loading every program** (Unix `exec`). It puts format parsing, a classic source
  of security bugs, in the most trusted code, and takes control away from launchers.
- **Programs at a fixed address.** Simpler for one release, but no address randomisation, and
  changing later would mean rebuilding every program.
- **Shared libraries now.** They save memory, but add a dynamic linker before anything needs
  one. They come with the POSIX layer (M9).
- **Building programs for the kernel's targets.** No floating point on x86-64, and a calling
  convention without floating point would become part of every program's contract.
- **Licences:** the GPL with the Classpath exception (well known, but its "linking" wording is
  a grey area for Rust's generics and generated code); the GCC Runtime Library Exception
  (written around the GCC compiler); the eCos exception (requires apps to ship the
  library's source, and refers to GPL version 2's sections).

## Open questions

- Thread-local storage in programs (with M9's C library).
- Whether the start-up archive should be compressed, once it holds many drivers.
- Signing the start-up archive (with secure boot, after Q5).

## Changes
- 31 October 2026: first version, accepted.
