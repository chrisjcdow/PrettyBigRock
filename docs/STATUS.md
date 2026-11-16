# PrettyBigRock: where things stand

Read this first in every session, and update it at the end of every stage (and whenever
something here stops being true). Until 16 November 2026 it was kept in the claude.ai Project
as `claude/status.md`; the repository's copy is now the one that counts.

Updated 16 November 2026. **M6 stage 8 built** (session 20: the file picker, the permissions
viewer, `lib/browse`, `check`, records per grant, `pbr.sysinfo`); `cargo xtask ci` passed on all
four configurations, every boot, on `1a648d2`. Since then: **the PS/2 driver fix** (`e81a6f0`,
below). `m6` at the commit that added this file; Harriet's checkout is on `m6`.

## M6 (the desktop, 2.0 alpha): stages 0 to 8 built

### After stage 8 (16 November)
- Chris ran `m6` on Harriet with `run-prettybigrock.bat` (UEFI and BIOS): the desktop, the
  start menu (Windows key, Ctrl+Esc) and apps all work. **The keyboard and mouse stopped after a
  minute or two**: the PS/2 driver drained the controller and only then acknowledged its
  interrupts. The lines are edge-triggered and masked until acknowledged, so a byte arriving in
  between was never signalled, and the controller then waited for it to be read forever. Fixed
  by acknowledging first (`e81a6f0`); reproduced and checked with QEMU's PS/2 pair driven over
  QMP. The test boots mostly use virtio input, which is why they never caught it.
- `run-prettybigrock.bat` gives QEMU no tablet, so the pointer is QEMU's grabbed PS/2 mouse
  (Ctrl+Alt+G releases it); on `arm` there is no pointer at all. Adding `-device
  virtio-tablet-pci` would fix both (offered to Chris, not done).
- Chris is moving the long build sessions to Claude Code on Impala to save usage; this file and
  `docs/m6-plan.md` moved into the repository so that every session keeps them up to date.

### Stage 8, session 20
Note 0025's and 0026's Changes list everything settled.
- `lib/fsserve`: `pbr.file.limit(rights, records)` (records per grant), nested shares. The
  sandbox manager makes every grant with it (4096; 16384 per trusted entry).
- `pbr.sysinfo` (users service, one badge; memory via `pbr.supervisor @4 memory`, users only).
  Sandbox manager gets it at role 1007, apps at 1005. `pbr_manifest::VERSION` = 2.0.0-alpha.1.
- `check`: `pbr.storage @6`, `pbr.users @12` (volume 0 = system), RockFS server `check [repair]`
  mode reporting through `pbr.fs.host @3 checked`. Checking a volume restarts sessions that
  hold it (open question: check while mounted).
- Sandbox manager: `grant @10` (picker only; app must have `picker` decided in its manifest);
  covered paths answered through the app's own grant; desktop sessions keep `apps/picker`
  running; picker kind may also call `details`; apps' picker caps are *moved* (no DUPLICATE).
- `lib/browse` (MPL-2.0, Sonnet-written): `Folders` (lib/files on target, `MemTree` on host),
  sorting, filters, places, kinds + icons, paths, formats.
- `apps/picker`: receiving thread + sandstone window (`Settings.over` → `create_over`), one
  per app, serial checked on arrival; last folder per app in `/Home/Apps/picker/last.txt`.
- `sandstone::picker::FilePicker`: helper thread, shares the event loop's notifier (the
  notifier copy can't be duplicated!), answers via an own bit; `FilePicker::mount`.
- `apps/permissions`: `Settings.system`; polls once a second; Take back / Stop.
- Desktop: window menu "What can this app reach?" launches the viewer with the app number.
- Compositor: system windows over a window are clamped onto its screen.
- Budgets: session 64 MiB, users 224, storage 144, gallery 16.
- Tests: host (picker 15, viewer 6, browse 61, fsserve limits), self-test
  `services/root/src/pickerparts.rs`, desktop boot (gallery → picker save/replace/open/1.3
  disk; viewer from window menu; windowprobe picker refusals). Runner helpers:
  `last_program_view`, `program_view`, `click_row`, `keep_window`, `visible_point`;
  `move_to` now corrects a relative move that lands elsewhere; `pick_row` accepts a click that
  opened the row as a double click.
- Screenshots `build/qemu/desktop-<tag>.picker-1x/2x.png`, `permissions-1x/2x.png` (sent to
  Chris with the host 2x pictures).

### Next
- Chris's answer on the heading font (`theme::HEADING`).
- Stage 9: the apps (Terminal, Notepad, Files, Calculator, Paint, Snake, About) by Sonnet,
  probably split into 9a (Calculator, Snake, About, Paint) and 9b (Notepad, Files, Terminal);
  the Screens dialog; the desktop's start-menu serial fix (`Window(BadSerial)` after a screens
  change); system windows bigger than their screen should be made to fit (the 600 x 400 picker
  on the test 2x screen); Notepad/Terminal parts of note 0025's desktop tests; Open with… in
  Files; the clipboard in the apps.
- Left open: QEMU 8.2.2 SIGSEGV now seen on GitHub too (early kernel, x86-64 UEFI); no
  backtrace yet (16 early boots in a loop here didn't crash): keeping the Stress workflow's
  core files as artifacts would give one. Why a long relative pointer move overshot to the
  screen's corner (twice in Stress, after the probe's picker was cancelled). The users service
  couldn't restart chris's session beside alice's on ARM64 in a self-test variant (budget never
  came back within a minute).
- Lock-order checker for plain `SpinLock`s: Chris to decide whether before M7.

### Earlier stages
- Stage 7 (`4d08afe`): sandstone. Stage 6 (`6c61545`): the session's desktop. Stage 5
  (`47780cf`): display server. Stage 4 (`b8044d1`): input. Stages 0 to 3: `f639a72`, `7ec222c`,
  `2de1a6c`, `c145d03`.
- Notes 0019 to 0026 accepted by Chris on 9 November. Plan in `docs/m6-plan.md`.
- Open from earlier: ARM64 EL2 virtio-gpu slow restart (seen once); us-intl AltGr to check on a
  real keyboard; Chris hasn't run `2de1a6c` or later on Impala; `run-prettybigrock.bat arm`
  untested.

## Published (GitHub)
- Repo: https://github.com/chrisjcdow/PrettyBigRock. main and `m5` at `9345496`; `m6` ahead
  (stage 8 and the PS/2 fix).
- Releases: `fynbos-1.0`, `v1.1`, `v1.2`, `v1.3`. M6 ends with `2.0.0-alpha.1`.

## Open questions
- DESIGN.md section 16: Q5 cryptography; Q6 a kernel name; Q7 running Linux programs.
- The heading font. Named time zones with summer time (note 0024). Note 0022's open questions.
- A lock-order checker for plain `SpinLock`s before M7.
- Checking a volume while it stays mounted (note 0026).

## Working notes
- `CLAUDE.md` has the workspace set-up and test commands. Additionally:
- **Saving usage** (Chris's weekly limit is shared by every Claude app): keep sessions short
  (one sub-stage each); read only the note sections the part being built needs; don't run
  `cargo xtask ci` locally and poll it: run host tests and the one boot configuration changed,
  push, and let GitHub's CI run the rest; Stress once, at the end of a stage.
- **On Impala** (Claude Code, Ubuntu, 4 cores / 8 threads, 32 GB): the workspace workarounds in
  CLAUDE.md (pinned `stable`, `library/` cloned by hand) aren't needed; rustup and the Rust
  download site work. Push with Chris's own git sign-in; no bundles or Harriet folder needed.
- **In the claude.ai cloud workspace** (2 CPUs): clone with `add_repo` (access push), then `git
  clone --depth 50 --branch m6 https://github.com/chrisjcdow/prettybigrock`; `git config
  user.name Claude`, `user.email noreply@anthropic.com`. Pushing works: `git -c
  push.negotiate=true push origin m6`.
- apt (`apt-get update` first): `qemu-system-x86 qemu-system-arm qemu-efi-aarch64 ovmf nasm
  gcc-multilib dosfstools mtools`.
- In the cloud workspace every cargo command **and rustfmt** needs `RUSTUP_TOOLCHAIN=stable`;
  `mkdir -p` the `library/` symlink's parent first.
- Clippy for the OS target: `RUSTC_BOOTSTRAP=1 cargo clippy --package X --target
  targets/x86_64-unknown-prettybigrock.json --target-dir target/programs -Zjson-target-spec
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem -- -D
  warnings`. Host `cargo build -p pbr-files` fails by design (OS-only); OS-only deps go under
  `[target.'cfg(target_os = "prettybigrock")'.dependencies]`.
- A program crate with a host-tested library (gallery, picker, permissions) needs its `main.rs`
  empty on the host. New MPL-2.0 libraries go in `licence_for`; new apps in xtask's program
  list, archive file list, PROGRAM_CRATES and HOST_CRATES.
- Giving a capability to a spawned process *as a copy* needs DUPLICATE on it; move it
  (`into_raw`, `true`) otherwise. Things derived through a capability have at most its rights.
- Self-test memory is tight: a new filesystem server takes 8 MiB of the storage service's
  budget for good; a session restart needs a session's budget free in the users service.
- One configuration's boots: `PBR_QEMU_ONLY="x86-64, UEFI" PBR_QEMU_BOOT=desktop cargo xtask
  qemu-test` (about 9 minutes in the cloud workspace). A full `cargo xtask ci` there about 80
  minutes. Stress takes 2 to 2¾ hours on GitHub. A running job's log: `gh api
  repos/chrisjcdow/PrettyBigRock/actions/jobs/JOB/logs` (the run's `--log-failed` only works
  once the whole run is done); grep `-a "FAILED\|SIGSEGV\|Run [0-9] failed"`.
- The runner's failure message is in its log after a line `FAILED` (`grep -a -A1 "^FAILED"`).
- **Subagents**: Sonnet via the Agent tool, `model: sonnet`, in the background, each in **its own
  git worktree** (`git worktree add ../wt-NAME HEAD`) with its own `CARGO_TARGET_DIR`. Briefs
  in a scratch folder (common.md + one per job). Copy back, review, test, commit.
- **Scripted edits to Rust**: assert the old text is found; rustfmt may have reflowed it.
- Boot logs: `build/qemu/desktop-<tag>.txt`, `desktop-again-<tag>.txt`, `desktop-text-<tag>.txt`;
  registers dumped on hangs (`grep -a "is in"`).
- Trying a boot by hand with PS/2 input: boot as `run-prettybigrock.bat` does with `-qmp
  unix:/tmp/qmp.sock,server,nowait` and send `input-send-event` (`rel`, `btn`, `key`) over QMP;
  `screendump` for pictures. Values must be whole numbers.
- GitHub: `git fetch origin m6`, then `git -c push.negotiate=true push origin m6`. Stress:
  `gh workflow run stress.yml --repo chrisjcdow/PrettyBigRock --ref m6 -f runs=5`.
- Harriet (from the cloud workspace): the Software Development folder connected; ask for
  deletion before the first fetch (git leaves lock and `tmp_obj_*` files otherwise). Harriet now
  has `m6` checked out, so update it with `git pull --ff-only origin m6`, not `git fetch origin
  m6:m6`. From Linux, git there lists every file as changed: only Windows line endings.
- Batch files can't be tested in the workspace. QEMU's `-name` can't contain commas.
