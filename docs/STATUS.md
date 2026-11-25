# PrettyBigRock: where things stand

Read this first in every session, and update it at the end of every stage (and whenever
something here stops being true). Until 16 November 2026 it was kept in the claude.ai Project
as `claude/status.md`; the repository's copy is now the one that counts.

Updated 23 November 2026 (session 26). **M6 is done: `2.0.0-alpha.1` is released** (a
pre-release on GitHub, tag `v2.0.0-alpha.1`, `m6` merged into `main`). The last part of stage
10b gave the release a ready-made system disk and a **Welcome** window (note 0024, section 6a)
after Chris's first try on Harriet lost a file kept in memory. CI passed; Stress (5 runs × 6
jobs) passed on `936e1f6`; Chris tried it on Harriet under Windows, both making a user through
Welcome and carrying on as Owner, and files were kept across restarts. **Now**: M7 (real
hardware I, Roxy; docs/ROADMAP.md) has started on branch `m7`. `docs/m7-plan.md` records Roxy's
hardware (Foxconn H61MX, AMI firmware with UEFI and BIOS, EHCI without MSI, COM2 at `0x2F8`,
an RTL8168) and the notes M7 needs. Chris agreed with Q1 (Ethernet moves to M8) and Q2 (our
own AML interpreter); he hasn't read the rest, so the other questions go into the notes as
recommended and are settled as he walks through each note. Roxy's ACPI tables are in
`tests/data/acpi/roxy-h61mx/`. Chris takes Roxy's drives out for every PrettyBigRock boot, and
will get a COM2 header bracket and a USB-serial adapter. Note 0027 (kernel 1.4) is accepted (25
November), with microcode loading (DESIGN.md section 14's rule for firmware). Note 0028
(ACPI) is accepted too, answering the firmware like Windows (Roxy's firmware only asks about
Windows names). Note 0029 (USB) is drafted, waiting for Chris. **Next**: notes 0030
and 0031.

## M6 (the desktop, 2.0 alpha): stages 0 to 9, 10a and the review built

### Stage 10a, session 23
Chris split stage 10: 10a (this) the failures and section 7; 10b the review and the release.
- **The failures on 17 November** (CI on all but x86-64 BIOS, all six Stress jobs). (1) Apps
  launched from the start menu opened without the focus: on slow machines they took longer
  than note 0022's 2 seconds. Now the first window takes the focus if the user has pressed
  nothing since the launch, within 30 s (note 0022's Changes; `Desk::last_press`). The hostile
  probe relied on opening too late; it now waits to lose the focus, and the runner drags Hung
  (to be pinged it must have the focus) off the probe. (2) The runner read Files' listing from
  before a rename or delete (wrong row, "no boulder.txt"); it now waits for the new one.
- **Section 7's table, the rest** (note 0026's Changes): the second boot compares each screen
  with the desktop drawn on the host by `lib/desktop` (clock left out; xtask now uses
  `pbr-desktop` and `pbr-draw`), checks the clock against the host's, launches Terminal from
  its icon, `ls` finds what the first boot saved, `set layout gb` types `£€` and `us-intl`
  `êñ`, and Terminal is minimised from the title bar and the taskbar and brought back. The
  desktop logs `icon APP "TITLE" ICON at X,Y`, `taskbar "TITLE" at X,Y [state]` and `clock …`.
  Calculator, Notepad, Terminal (1x, 2x) and Paint (1x) are compared with their host
  pictures; Snake, About, Files, the picker and Paint at 2x only kept (they differ by nature).
- **Chris's decisions**: Paint's title **Rock Art**, Snake's **Boomslang**, Notepad stays
  (programs and folders keep their names). **Session budget 96 MiB**, users service 320 MiB,
  and since budgets are reserved at start (not just limits: the display server couldn't be
  restarted in 1 GiB), **test machines and `run-prettybigrock.bat` have 2048 MiB**.
- **Double clicks** are timed by the click's own time (`PointerEvent::time`, note 0023's
  Changes), in sandstone and the desktop's icons: after the first click the desktop redraws
  its wallpaper, which emulated ARM64 may take longer than half a second to do.
- Seen, for Chris: "About PrettyBigRock" is cut to "About PrettyBi" under its desktop icon.
- Tests run here: host tests of every changed crate; under KVM, all four x86-64 boots (UEFI and
  BIOS, self-test and desktop) with 2 GiB, and the UEFI desktop boot after the last change.
  ARM64 left to GitHub's CI; Stress (5 runs) started on `f7ad74f`'s successor.

### Stage 10b, part 3, session 25: the Welcome window
- **Why**: on Harriet, files saved as `owner` were gone after a restart; nothing said they
  were kept in memory. Chris chose: files kept from the first start, and a window asking
  whether to make a user or carry on as Owner.
- **Built** (notes 0017, 0024 section 6a, 0026 and 0004, Changes of 22 November):
  `prettybigrock-data.img` (256 MiB, RockFS `Files`, a system volume for `owner`, written by
  the build tool with `lib/rockfs`; `tools/rockfs.py` finds it clean); the launcher copies it
  to `data.img` when there is none. `apps/welcome`: on a disk, Make my user (`add_first`, new
  in `pbr.users`) or Carry on; in memory, Set up a disk. A third `pbr.users` badge,
  `users welcome`, which may ask only `whoami`, `settings`, `set` of `welcome`, `volumes`,
  `setup`, `add_first` and `restart` (the self-test checks the rest is refused, and a
  standard user's administrator operations). The desktop opens Welcome until `welcome done`;
  users made by `add_first` or `setup` start with it. The ESP grew to 96 MiB (debug archives).
- **Tests**: host tests for the window (pages, names, disks offered, refusals, pictures) and
  the data disk; the desktop boot answers Welcome in its first boot, makes `dana` through it
  at the end of the second, and a fourth boot (`desktop-dana`) checks dana is logged in and
  does the Shut down steps. The in-memory page's Set up was driven here by hand on the
  release image: set up, Restart now, chris logged in from the new disk, no Welcome again.

### Stage 10b, part 2, session 25: the release
- **Docs**: notes 0020 to 0026 say "Built 22 November 2026"; DESIGN.md section 8 says what M6
  built under each service, and gives Q8's answer where it was asked; README describes the
  desktop, the apps and the launcher, with a desktop screenshot; ROADMAP's Releases paragraph.
- **Chris's choices** (note 0026's Changes): the version goes on the release's files
  (`prettybigrock-2.0.0-alpha.1.zip`, `.img`, `-selftest.img`), not the build tool's; a
  draft first, published after Stress and Harriet.
- **Found by booting the release images** (`cargo xtask qemu-test --release`, never run by
  CI): (1) the ordinary image held the test programs, and its desktop listed Test window and
  Gallery: the build tool now packs it a start-up archive without `pbr-test-` programs and
  `tests/programs` manifests (`build/<arch>/<profile>/startup-plain.arc`); (2) the text-mode
  desktop boot typed `poweroff` before the input service had the keyboard (the shell got
  `ff`): the runner waits for `input: using QEMU Virtio Keyboard`.
- **The launcher**: `two-screens` (virtio-gpu with two outputs, a tablet tied to each; UEFI
  only), a tablet with one screen, and a blank `data.img` (256 MiB) attached. Booted here with
  each set-up (x86-64 UEFI one and two screens, BIOS, ARM64 one and two screens) to the
  desktop with 8 apps; `setup 1 chris` in Terminal then a restart logged chris in from the new
  system volume. In a headless QEMU the second virtio-gpu output stays unplugged until a viewer
  asks for it (as in the boot tests).
- **For Chris on Harriet** (the draft's zip): double-click the launcher; `two-screens` (does
  QEMU's View menu bring up the second screen?); `arm`; `bios`; `selftest`; in Terminal
  `volumes`, `setup N chris`, Restart, and a file kept across a restart. Seen before: "About
  PrettyBigRock" is cut to "About PrettyBi" under its icon.
- Tests run here: `cargo xtask check`; under KVM, all x86-64 boots on the release images
  (self-test and desktop, UEFI and BIOS), and the desktop boots again after the runner fix;
  the launcher's five set-ups and `setup` as above. Stress (5 runs × 6 jobs) **passed** on
  `5f64280`: no system code changed after it, only the runner, the build tool's plain archive
  (not booted by Stress), the launcher and docs.

### Stage 10b, part 1, session 24: the capability review
Everything in the list below was read against its note; what was checked is in each note's
Changes (0019, 0021, 0022, 0024, 0025, 0026, dated 21 November).
- **Fixed** (sandbox manager, one commit each): an app's `pbr.picker` capability and the
  picker's receiving end are given as copies, not moved, so a failed spawn can't leave them
  (and the app's watched badge) in the sandbox manager's table; an app started but not
  recorded (a failed `watch` or copy for the caller) is stopped instead of left running out of
  reach of `stop` and `revoke`; a covered `grant` is refused if the app ended meanwhile.
- **Decided by Chris, and done** (one commit each, each with a self-test): (1) what the
  picker grants, or a trusted caller such as Files chooses, is opened by path in the sandbox
  manager's session tree. It is no longer derived from the chooser's capability, so taking
  back the picker's or Files' grant leaves apps' files alone, and their records count only
  against their own share. The desktop's `choose` is unchanged (note 0025, section 1). (2)
  Nobody may launch `desktop` or `picker` (`NotAllowed`). (3) Chosen-capability checks are
  limited to 4 per caller and 16 in all, as launches are (note 0024, section 9). (4)
  `pbr.sysinfo` has a badge per session: an administrator's apps see every volume, a
  standard user's only the system volume (note 0026, section 3). The users service waiting
  during a `check` is an open question in note 0026, for M7.
- Nothing wrong found in the kernel's 1.3 calls, the power service, the supervisor's table and
  `memory`, the input service, the display server's clients, `lib/fsserve`'s limits,
  `lib/manifest`, the users service's badges, or the picker, viewer and desktop apps.
- Tests run here: `cargo xtask check`; under KVM, the x86-64 UEFI self-test and desktop boots
  passed (the picker, Open with… and every launch go through the changed code). The rest is
  left to GitHub's CI.

### Stage 10b: what it has to do
- **The capability review** (done in session 24, above): everything M6 added that hands out,
  derives, checks or revokes capabilities. From `git diff 9345496 HEAD`: the kernel's changes
  (`obj/endpoint.rs`, `syscall.rs`, `power.rs`); `services/sandbox` (window capabilities and
  `launched`, rights and limits, `grant @10`, `openers @12` and Open with…'s
  prepare/choose/start/cancel, badges by kind, picker caps moved, `DESCRIBED`, `NoMemory`);
  `services/users` (sessions, `pbr.users @12 check`, sysinfo, layouts, debug); the display
  server's clients (`services/display/src/clients.rs`, `client()` per app, watched; system
  and desktop flags; `check_serial`); `services/input` (who may take the seat, `set_layout`);
  `services/power`; `services/storage @6` and `services/rockfs` (check, `pbr.fs.host @3`);
  `lib/fsserve` (`limit`, nested shares); `lib/manifest` (app keys, rules, settings);
  `apps/picker`, `apps/permissions` (take back, stop), `apps/desktop`; the supervisor's
  `@4 memory`. Write findings into the notes' Changes; fix what is wrong.
- Then: README, ROADMAP, DESIGN.md (Q8 settled, section 8's text), every M6 note (0019 to
  0026) marked built; tag and release `2.0.0-alpha.1` with disk images (Q14) and
  `run-prettybigrock.bat`'s choice of one or two screens; Stress (runs=5: 120 boots of each
  kind); Chris tries it on Harriet.
- Still waiting for Chris: the heading font, the lock-order checker.

### Stage 9a, session 21
Note 0026's Changes list what building settled.
- Apps (each by a Sonnet subagent, reviewed): `apps/calculator` + `lib/calc` (1.3's
  expressions; `eval`, `format`, `Keypad`), `apps/snake` + `lib/snake` (seeded game),
  `apps/paint` + `lib/bmp` (MPL-2.0; 1.3's 24-bit files byte for byte, reads 24/32-bit),
  `apps/about` (`pbr.sysinfo`). Budgets: 4 MiB each, Paint 12.
- Sandstone: `Button::colour`, `Canvas::label`, `App::focus_changed` (note 0023, Changes).
- `lib/files`: `Files::report(app)` writes `[tree]` lines; the root program's desktop test
  follows each app (`follow_app`, `APPS`) and logs `Desktop test: app N (APP) grants: …`; the
  runner's `check_tree` compares tree, grants and manifest. Sandbox manager: `DESCRIBED`.
- Fixes: system windows bigger than their screen fit it (compositor `fit_in`); the desktop
  draws wallpapers only when idle (`undrawn`), the start menu's `BadSerial`.
- Desktop boot (first boot, after the gallery): `Run::apps` → calculator, snake, paint, about.
  Screenshots `build/qemu/desktop-<tag>.<app>-1x/2x.png`.
- Tests run here: host tests of every changed crate; desktop boots: x86-64 UEFI emulated, all
  three boots passed; x86-64 UEFI and BIOS under KVM, the first boot (every app step) passed
  and the second failed at Shut down (KVM only, below). ARM64 and the self-test boots left to
  GitHub's CI.

### Stage 9b, session 22
Notes 0018, 0022, 0024, 0025 and 0026 have what building settled in their Changes.
- **Leftovers from 9a**: the maximise button is greyed out on windows that can't be resized
  (compositor pictures redrawn); `-no-reboot` only for boot tests, so Restart restarts under
  `cargo xtask run`.
- **The KVM hang** was the desktop test's: the root program polls every 200 ms for the app
  launched from the start menu, and under KVM it could open and close in between; it now
  logs seeing it and the runner waits for that line. Found with a new diagnostic: on x86-64 an
  NMI makes the kernel print its scheduler's state, and the runner sends one when a boot hangs.
- **Apps** (each by a Sonnet subagent, reviewed): `apps/notepad`, `apps/files` (with
  `lib/browse` making, renaming and removing), `apps/terminal` (with the shell moved into
  `lib/shell::program` behind a `Console` trait, shared with the text shell, and 1.3's missing
  commands in both), the desktop's **Screens dialog** (`lib/desktop::arrange`,
  `screens_window`). Manifests and grants are Opus's: Notepad (windows, picker, documents rw,
  opens txt md), Files (trusted, windows), Terminal (trusted, windows, sysinfo, data); the text
  shell asks for sysinfo too. Budgets 8 MiB each.
- **Capability work (Opus)**: `pbr.sandbox @12 openers()`; Files' Open with… (`prepare`,
  `choose` with the file read-write only if the user may change it, `start`, `cancel` on
  failure); `check_serial` counts a press in the window's own dialog (Notepad's "Save
  changes?" → picker); spawning short of memory answers `NoMemory` instead of panicking the
  sandbox manager (`ElfError::NoMemory`).
- **Found by the boots**: the desktop dropped every event when no Screens window was open (a
  `?`); the root program held every test app's process to the end, pinning their budgets in
  the session until Files couldn't open Notepad (`ended` now takes the process); Terminal's
  `edit` didn't pass the path to Notepad; runner races (double clicks, lines seen before).
- **Desktop boot**: Notepad, Files and Terminal steps as note 0026 section 7 lists, each app's
  `[tree]` lines checked (trusted apps against grants that are all `Trusted`), the clipboard
  from Notepad to Terminal, and on two-screen machines the Screens dialog's live mirroring.
  Screenshots: `build/screens/desktop-x86_64-uefi.{notepad,files,terminal}-{1x,2x}.png`.

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
- Chris's answer on the heading font (`theme::HEADING`; left as it is in 9a).
- **Start menu and Shut down** (session 21, after 9a, Chris's request): menus choose on
  release; Shut down and Restart ask first (`pbr_desktop::confirm`, note 0024's Changes). The
  KVM-only `Broken pipe` at Shut down is gone with it (the runner's last write is now the
  release; `click_ending` also tolerates QEMU stopping). Desktop boots under KVM passed on
  x86-64 UEFI and BIOS, all three boots.
- **Solved in 9b: the hang under KVM, seen twice in six runs** (x86-64 UEFI desktop boot, both at the
  start-menu step, just after the test window ends; never emulated or on CI; both after the
  start menu began choosing on release, none in the four KVM runs before). Nothing more is
  logged by anyone; the root program, whose calls all time out in 30 s, never reports; cores
  sit in `idle_loop`/`wait_for_interrupt`, one in `sched::tick`. Hypothesis: a core stops
  getting timer interrupts, so threads asleep on it (each core wakes only its own timed
  waits, `sched/mod.rs` `tick`) are never woken. **It wasn't**: an NMI dump showed every core
  ticking; the root program's 200 ms poll missed an app that opened and closed in between, and
  it now logs seeing it (DEVLOG, stage 9b). Not a kernel bug.
- **Done in 9b**: the greyed maximise button and Restart under `cargo xtask run` (Chris, 17
  November).
- **Windows fit their screen** (after 9a, Chris's choice): resizable app windows open no bigger
  than the work area (note 0022's Changes); Paint is resizable with a scrolling picture and
  File and Edit menus (note 0026's Changes). A window dragged by hand to a smaller screen keeps
  its size.
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
  (stages 8 and 9a, the PS/2 fix).
- Releases: `fynbos-1.0`, `v1.1`, `v1.2`, `v1.3`. M6 ends with `2.0.0-alpha.1`.

## Open questions
- **App names**: keep the generic names unless Chris strongly approves a change; names must
  make sense for the app, puns welcome. Rock Art (Paint) and Boomslang (Snake) done in 10a;
  Chris kept Notepad (18 November).
- Terminal: per-cell colour in `lib/terminal`'s grid (1.3's green prompt, coloured `fetch`);
  stopping a running command with Ctrl+C. The display server can't list tablets (the Screens
  dialog offers only those in `screens.txt`).
- DESIGN.md section 16: Q5 cryptography; Q6 a kernel name; Q7 running Linux programs.
- The heading font. Named time zones with summer time (note 0024). Note 0022's open questions.
- A lock-order checker for plain `SpinLock`s before M7.
- Checking a volume while it stays mounted (note 0026).
- Window options apps may ask for (style, size hints, full screen, staying on top), chosen from
  what the display server offers, which keeps the last word; full screen only in answer to a
  press, always left with a key the server keeps, and announced by it; the user overrides with
  rules and the window menu. Wants its own design note (a `pbr.window` version 2) when needed;
  not for M6. Discussed with Chris, 17 November.

## Working notes
- `CLAUDE.md` has the workspace set-up and test commands. Additionally:
- **Saving usage** (Chris's weekly limit is shared by every Claude app): keep sessions short
  (one sub-stage each); read only the note sections the part being built needs; don't run
  `cargo xtask ci` locally and poll it: run host tests and the one boot configuration changed,
  push, and let GitHub's CI run the rest; Stress once, at the end of a stage.
- **On Impala** (Claude Code, Ubuntu, 4 cores / 8 threads, 32 GB): the workspace workarounds in
  CLAUDE.md (pinned `stable`, `library/` cloned by hand) aren't needed; rustup and the Rust
  download site work. Push with Chris's own git sign-in; no bundles or Harriet folder needed.
- **KVM on Impala**: `PBR_QEMU_KVM=1` makes `cargo xtask qemu-test` and `run` add `-accel kvm`
  to the x86-64 machines (opt-in; ARM64 and GitHub's CI emulate as before). Use it for x86-64
  boot tests here; a failure seen only under KVM goes to Chris.
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
  git worktree** (`git worktree add ~/pbr-wt/NAME HEAD`) with its own `CARGO_TARGET_DIR`. Briefs
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
  has `m6` checked out, so update it with `git -c core.autocrlf=true pull --ff-only origin m6`,
  not `git fetch origin m6:m6`. Without `core.autocrlf=true`, git from Linux sees every file
  as changed (Windows line endings) and refuses to pull.
- Batch files can't be tested in the workspace. QEMU's `-name` can't contain commas.
