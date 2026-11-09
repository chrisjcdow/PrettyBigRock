# Working on PrettyBigRock

This file is for Claude sessions (and anyone else) working in this repository. Read it at the
start of every session. The project's own rules are in `docs/design/0002-coding-rules.md`; this
file adds how the work is shared out and how the code is commented, and points to everything
else.

## Start here

1. `docs/ROADMAP.md`: which milestone we're on and what "done" means for it.
2. `docs/DESIGN.md`: the decisions that settle arguments (sections 2 and 3).
3. `docs/design/README.md`, then the design notes for the part you're working on. The notes are
   the project's memory: Claude starts every session without remembering the last one.
4. The end of `DEVLOG.md`: what the last session did and left open.

Design notes come before code (D14). A note is drafted, walked through with Chris, and accepted
before what it describes is built. When building teaches us something, the note is updated in
the same stage, with a line under "Changes".

## Who does what

Work is shared between models by how much depends on getting it exactly right.

- **Claude Opus does the foundational and security-critical work itself:**
  - design notes, and any change to an accepted note;
  - the kernel;
  - anything that hands out, derives, checks or revokes capabilities (file capabilities and
    their rights, the sandbox manager, manifests, users, supervision);
  - the file layer and the shared block cache;
  - reviewing everything a subagent produces.
- **Claude Sonnet, as a subagent, writes well-defined code that follows an existing pattern**
  once its design is settled: for example FAT32, the text shell, porting RockFS to Rust after
  its note is accepted, and extra tests.
- **Claude Haiku, as a subagent, only does trivial jobs**, such as summarising a long test log.

Rules for handing work to a subagent:
- **Give it a self-contained task.** List the files it must read (this file, note 0002, the
  note sections it implements, an existing file to follow as a pattern), the files it may
  change, the interfaces it must not change, and the exact tests that must pass. It knows
  nothing else about the project.
- **Review everything before it goes in.** Read the whole diff, run the tests yourself, and
  check it against the note and these rules. Code quality comes first: everything here will be
  built on, again and again. Send it back, or fix it, rather than accept something weak.
- Keep test runs small to save usage, but never at the cost of quality.

## Comments: written for a newcomer to Rust and to operating systems

Write comments for a reader who knows how to program, but is new to **Rust** and new to
**operating-system internals**. That reader should be able to open any file and follow it.

- **Explain an OS idea where it first appears in a file**: what a capability, a page table, an
  interrupt, DMA, a block cache, copy-on-write or a badge is, in a sentence or two, and why this
  code needs it. Point to the design note that goes deeper.
- **Explain a Rust feature where it first appears in a file** when it isn't obvious to someone
  from another language: ownership and borrowing, lifetimes, traits and generics, `Option` and
  `Result` with `?`, `match`, `unsafe`, `Drop`, `#[repr(C)]`, `no_std`, `Arc` and atomics,
  macros. One sentence is usually enough.
- Explain each idea **once per file**, at its first use, not at every use.
- Comments still say **why** more than **what** (note 0002, rule 5). Don't narrate code that
  reads plainly.
- `// SAFETY:` comments on `unsafe` blocks say why the block is sound, in plain words.
- Keep comments true. A comment that no longer matches the code is a bug.

For example:

```rust
// A badge is a number the service stamps on each capability it hands out (note 0008), so
// when a request arrives it knows which file the caller means. `?` returns early with the
// error if the lookup fails, instead of carrying on with a missing value.
let node = self.objects.get(badge)?;
```

## Commits, branches and the DEVLOG

- Each milestone is built on its own branch (`m5` for milestone 5), in stages. Each stage ends
  with every test passing on all four boot configurations and a report to Chris.
- **Keep commits small:** one thing each, with a message saying what and why, ending with the
  attribution lines the session gives you.
- Update the design notes, `DEVLOG.md` and `docs/ROADMAP.md` as you go, not only at the end.
- This workspace usually can't push to GitHub. Deliver commits as a git bundle in Chris's
  Software Development folder on Harriet (`C:\Users\chris\Nextcloud\Personal Admin\Software
  Development`), check its sha256 there, and Chris fetches and pushes it. Always use a new
  bundle file name.

## Building and testing

- `cargo xtask check`: formatting, Clippy for every architecture, licence lines.
- `cargo xtask test`: host tests, including the cross-checks against the Python tools.
- `cargo xtask qemu-test`: boots all four configurations (x86-64 UEFI and BIOS, ARM64 UEFI at
  EL1 and EL2) and runs the self-tests. `PBR_QEMU_ONLY=ARM64` (any part of a configuration's
  name) picks configurations. Don't run two boot tests side by side.
- `cargo xtask ci`: all of it. A change isn't finished until it passes.

### Setting up Claude's workspace

The workspace can't reach `static.rust-lang.org`, so:
- `export RUSTUP_TOOLCHAIN=stable` (it is 1.97.0, the pinned version);
- clone Rust's `library/` at tag 1.97.0 into `~/.cache/pbr/rust-1.97.0` (`git clone --depth 1
  --filter=blob:none --sparse --branch 1.97.0 https://github.com/rust-lang/rust.git`, then
  `git sparse-checkout set library`), and symlink it to
  `$(rustc --print sysroot)/lib/rustlib/src/rust/library`. The build tool then builds `core`
  from source (note 0002);
- install QEMU with apt: `qemu-system-x86 qemu-system-arm qemu-efi-aarch64 ovmf`; for the
  classic system, `nasm gcc-multilib`; for the FAT32 tests, `dosfstools mtools` (tests that
  need them say so and are skipped without them, but a stage isn't finished until they've
  run).

The workspace has only 2 CPUs: since M5 a full `cargo xtask ci` takes about 25 to 55 minutes
(each boot test 4 to 11, depending on how busy the machine is), so run it in the background with
a long timeout. M6's desktop boots (note 0026, section 6) will add to that. When a boot fails,
the runner saves every core's registers and stack in `build/qemu/serial-*.registers.txt`;
resolve addresses with `llvm-addr2line -f -C -e build/<arch>/debug/kernel.elf ADDR`.
