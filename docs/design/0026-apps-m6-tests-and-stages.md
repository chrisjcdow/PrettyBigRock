# 0026: The 1.x apps rewritten, how M6 is tested, and the order of building

Status: Accepted · 9 November 2026 · Related: DESIGN.md D11, D17, sections 7 and 15; ROADMAP.md M6;
notes 0014, 0016, 0017, 0018, 0019 to 0025; milestone M6

## The problem

M6's "done when": **PrettyBigRock 2 does everything 1.3 does, with every app sandboxed. This is
the first 2.0 alpha.** This note settles what each 1.3 app becomes, the smaller pieces M6 also
owes (`check VOLUME`, limits on a filesystem server's records per grant, system information,
the version), the table that makes "everything 1.3 does" checkable, how the desktop is tested,
and the stages.

## The design

### 1. The apps

Each app's logic lives in a library built for the host and tested there, as `lib/shell` is for
the shell (note 0018); the app itself is the window and the glue. All are system apps in the
start-up archive (`apps/<name>`, with `apps/<name>.app`). Their grants follow Chris's answer to
Q16.

| App | Manifest | What it does (1.3, and what M6 adds) | Logic |
|-----|----------|--------------------------------------|-------|
| **Terminal** | `trusted`, `windows`, `sysinfo`, `data` | 1.3's Terminal: the shell in a window, scrollback (Page Up, Page Down), history (Up, Down), kept in its folder. Copy and paste (note 0025) | `lib/shell` (its terminal side becomes a trait: `apps/shell` implements it with `pbr.terminal`, `apps/terminal` with sandstone's `TerminalView`) |
| **Notepad** | `windows`, `picker`, `files documents read write`, `opens txt md` | New, Open, Save, Save as, Ctrl+N, Ctrl+O, Ctrl+S, 1.3's editing keys. Adds: the picker for opening and saving anywhere the user chooses, the clipboard, undo, "•" in the title while unsaved, asking before closing unsaved work, the file it was launched with | sandstone's `TextEditor`; saving is write, set size, `sync` (atomic on RockFS, note 0014) |
| **Files** | `trusted`, `windows` | 1.3's Files: Up, Open, New file, New folder, Rename, Delete (asking first). Adds: places (as the picker), Open with… and double-click to open (note 0025), sizes and dates | `lib/browse`, shared with the picker: listing, sorting, places, kinds of file and their icons |
| **Calculator** | `windows` | 1.3's Calculator: digits, `+ − × ÷`, `%`, `+/−`, `C`, `=`, and the keyboard | `lib/calc`, shared with Terminal's `calc` (1.3's expressions: `(2+3)*4^2`) |
| **Paint** | `windows`, `picker`, `files pictures read write`, `data`, `opens bmp` | 1.3's Paint: a 600 × 400 canvas, the brush and the colour palette, Open, Save, Clear; saves 24-bit uncompressed `.bmp` (`Drawing N.bmp` in Pictures), opens 24- and 32-bit ones. Adds: the picker, copy and paste of the picture, its last colour remembered | `lib/bmp` (reading and writing, host-tested with files 1.3's Paint saved) and the brush (lines with `lib/draw`) |
| **Snake** | `windows`, `data` | 1.3's Snake: the board, the arrow keys, food, growing, the score, pausing, game over. Adds: the best score, kept in its folder | `lib/snake`: the game, stepped with a seed, so tests can play it |
| **About** | `windows`, `sysinfo` | 1.3's About: the logo, "Named for Table Mountain, a pretty big rock.", the version, and Display, Memory, Uptime and Disk | |
| **The desktop** | note 0024, section 6 | 1.3's desktop: wallpaper, icons, taskbar and clock, start menu, Restart and Shut down | |
| **Picker**, **Permissions** | note 0025 | New | `lib/browse` |

**The potato stays**: Terminal's `potato` answers as in 1.3 and isn't listed in `help`.

**Terminal's commands** are M5's shell's plus 1.3's that M5 didn't have: `time` and `date`
(the wall clock in the user's time zone), `tz HOURS` (sets the time zone setting, note 0024),
`uptime`, `mem`, `fetch` (the system summary, with 1.3's picture of the mountain), `calc EXPR`,
`edit FILE` and `open APP` (launching through `pbr.sandbox`, as `run`), `disk`, `check [VOLUME]
[fix]` (section 2), `fortune`, `ver`, `reboot` and `poweroff` (note 0024, section 7), and 1.3's
other names for commands (`dir`, `del`, `copy`, `move`, `ren`, `md`, `rmdir`, `type`, `cls`,
`fsck`).

### 2. `check VOLUME`

1.3 checks and repairs its disk (`check`, `check fix`); M5 left it out (note 0018, Changes).
- `pbr.storage` gains `@6 fn check(id: u32, repair: bool) -> (report: string<4000>, problems:
  u32)`. The storage service stops the volume's filesystem server (its clients get
  `PEER_GONE`, as when a server restarts), runs the check (the RockFS server started with
  `check` or `repair` as its argument, using `lib/rockfs`'s existing checker, which reports and
  ends), and starts the server again.
- `pbr.users` gains `@12 fn check(volume: u32, repair: bool) -> (report, problems)`, for
  administrators. The shell's `check VOLUME [fix]` uses it, and with no volume, checks the
  system volume.
- RockFS only in M6: FAT32 answers `NotSupported` (checking FAT is note 0016's open question).

### 3. System information: `pbr.sysinfo`

For About and Terminal's `mem`, `disk` and `fetch`, served by the users service to apps with
`sysinfo` (role 1005):

```
interface pbr.sysinfo version 1;
@1 fn version() -> (name: string<64>, version: string<32>) error SysinfoError;
@2 fn memory() -> (total: u64, available: u64) error SysinfoError;
@3 fn screens() -> (screens: list<ScreenSummary, 16>) error SysinfoError;
@4 fn volumes() -> (volumes: list<VolumeInfo, 32>) error SysinfoError;
flexible enum SysinfoError { NotReady @1 }
```

The memory comes from the root program (a new `pbr.supervisor` operation for the users service
only, `@4 fn memory() -> (total, available)`, from the root program's own budget), the screens
from the display server (`pbr.display.sessions.screens`, note 0022), the volumes from the
storage service. Uptime is the monotonic clock, which every program can read. Nothing in it says
anything about other apps or other users.

### 4. Records per grant (notes 0014 and 0018's open question)

One client can fill a filesystem server's 65536 records and stop every other program opening
files on that volume. In M6, with many apps at once, each grant gets its own share:
- `pbr.file` gains `@4 fn limit(rights: FileRights, records: u32) -> (node: handle<pbr.file>)`:
  like `reduce`, but whatever is opened, created or reduced through the new capability (and
  through what that opens) may hold at most `records` records at once, and those also count
  against every limit above it. Past it, `TooManyOpen`. A limit can be set within a limit, never
  above it.
- The sandbox manager gives each grant with `limit` instead of `reduce`: 4096 records for an
  app's place or chosen file, and 16384 for each entry of a trusted app's tree. Other apps' grants
  of the same volume are unaffected by one that fills its share.
- It lives in `lib/fsserve` (each record remembers the nearest limit above it), so every
  filesystem server has it. Note 0014 gets a line under Changes when it is built.

### 5. The version

The alpha is **`2.0.0-alpha.1`** (ROADMAP.md's scheme): in `pbr.sysinfo.version`, the shell's
`about` and `ver`, About, and the build tool's disk image names. It is released on GitHub with
disk images for QEMU, as the 1.x releases were (Q14), and `run-prettybigrock.bat` gains a choice
of one or two screens. VirtualBox, VMware and Roxy wait for AHCI and USB (M7).

### 6. How M6 is tested

**Two boots per configuration.**
1. **The self-test boot** (`selftest`, text mode), as M5's, with M6's additions in the program
   self-test: kernel 1.3 (note 0019), the display drivers (note 0020: patterns on every screen,
   screenshots), the input service and layouts (note 0021), the sandbox manager's changes, app
   folders, settings and power refusals (note 0024), the picker's `grant` (note 0025), `check`,
   per-grant limits and `pbr.sysinfo` (this note). The runner waits for `Program self-test
   passed`, as now.
2. **The desktop boot** (`desktoptest` on the command line): a normal start to the desktop, as
   Chris will see it, with the display server describing its screens to the serial port (note
   0022, section 13) and the root program running a desktop test thread beside it (launching the
   hostile `tests/programs/windowprobe` through `pbr.users.debug`, and checking grants). The
   runner drives it through QEMU's control socket: keys, pointer moves and clicks (absolute on a
   tablet, relative on PS/2), waits for description lines, and takes screenshots. It follows the
   steps of section 7's table (the steps are a list in `xtask`, `DESKTOP_STEPS`, as
   `SHELL_COMMANDS` is for test 8). It ends with **Restart**: QEMU, told `-no-reboot`, stops
   with `guest-reset`; the runner starts the same disks again, checks that what was saved and the
   screen arrangement are still there, and chooses **Shut down** (`guest-shutdown`). The runner
   then checks the disks with `rockfs.py` and 1.3's own code, as in M5.

**The configurations** (Q12):

| Configuration | Screens | Keyboard and pointer |
|---------------|---------|----------------------|
| x86-64 UEFI | virtio-gpu, two screens (the second at 2× from `screens.txt`, which the build tool writes into the test user's desktop folder) | virtio keyboard, two virtio tablets (one tied to each screen) |
| x86-64 BIOS | the firmware's screen (QEMU's standard VGA), framebuffer driver | PS/2 keyboard and mouse |
| ARM64 EL1 | the firmware's screen (`ramfb`), framebuffer driver | virtio keyboard and tablet |
| ARM64 EL2 | virtio-gpu, two screens as x86-64 UEFI | virtio keyboard, two virtio tablets |

**Pictures.** Most checks use the description lines, which don't depend on drawing. Screenshots
(`screendump` per screen, compared with PNG reference pictures in `tests/pictures/`, note 0023)
are taken only of things that should look exactly the same every time: the wallpaper and an
empty desktop with the clock area left out, a window of each app just opened, the picker, and
the device self-test's pattern. The rest of the look is tested on the host (note 0023,
section 6).

**Every app sandboxed, from the inside.** In `desktoptest` boots `lib/files` reports each app's
file tree at start (`[tree] notepad /Home/Documents rw`, one line per entry) and the desktop
test thread compares it with what `grants` says and with what the manifest, rules and choices
should give. M5's `tests/programs/sandboxed` still does the deep check (every handle number,
`..` from every entry) in the self-test boot.

**Speed.** Drawing in emulated x86 is slow, so `lib/draw`, the display server and the drivers'
copying are built optimised in test builds too (a Cargo profile override; debug assertions stay
on). Expected CI time: about 80 to 90 minutes (each desktop boot 6 to 10 minutes), measured in
stage 0 and recorded in CLAUDE.md. The Stress workflow runs both boots.

### 7. "Everything 1.3 does"

Every feature in 1.3's README and start menu, and every Terminal command, with where it is in
2.0 and the check that shows it (D = the desktop boot; S = the self-test boot; H = host tests):

| 1.3 | 2.0 | Check |
|-----|-----|-------|
| Boots on BIOS, VESA framebuffer, 1024 × 768 or the best mode | The BIOS loader (M1), the framebuffer driver | D on x86-64 BIOS |
| PS/2 keyboard and mouse | M4's driver, the input service | D on x86-64 BIOS |
| QEMU's absolute pointer (touch works) | virtio tablet (note 0021) | D, three configurations |
| Clock (CMOS) | The wall clock (M5) and the time zone | D (the taskbar's clock is shown), S (`date`) |
| ACPI power off; reboot | `pbr.power` (note 0024) | D: Restart (`guest-reset`), Shut down (`guest-shutdown`) |
| Crash screen | Kernel panics are drawn on the screen and the serial port (M1); apps crashing don't take the system down | S (M4, M5's crash tests) |
| Double-buffered drawing, alpha, gradients, rounded shapes, the 5 × 9 font | `lib/draw`, `lib/font` | H |
| Overlapping draggable windows with shadows, focus, minimise, close | The display server | D: drag, focus, minimise from the title bar and the taskbar, close |
| Taskbar with clock, start menu (Windows key or Ctrl+Esc), desktop icons | The desktop app | D: each |
| Ctrl+W closes the active window | The display server | D |
| RockFS: folders, long names, saving that survives power cuts, check and repair | M5's RockFS, `check` (section 2) | S (M5's tests, `check`), D (files saved before Restart are there after) |
| A RAM disk without a data disk | M5's volume in memory | S (M5) |
| The 1.3 data disk's files | M5's test 4; Notepad and Paint open its files | S, D |
| Terminal: about 30 commands, folders, history, scrollback | Terminal (section 1) | H (`lib/shell`, every command), D (a set typed in the window) |
| Notepad: New, Open, Save, Save as, Ctrl+S, Ctrl+N, Ctrl+O, editing | Notepad | H, D (type, save through the picker, open it again, open a 1.3 file) |
| Files: Up, Open, New file, New folder, Rename, Delete | Files | H (`lib/browse`), D (each, and opening a text file in Notepad and a picture in Paint) |
| Calculator | Calculator | H (`lib/calc`), D (a sum with the keys and the pointer) |
| Paint: draw, colours, Clear, saves and opens `.bmp` | Paint | H (`lib/bmp` with 1.3's files), D (draw, save, open again, open 1.3's picture) |
| Snake | Snake | H (`lib/snake`), D (starts, turns, eats, ends) |
| About: display, memory, uptime, disk | About, `pbr.sysinfo` | D |
| `potato` | Terminal | H, D |
| Table Mountain at dusk | The desktop's wallpaper | D (screenshot) |
| The browser player | Stays on 1.x (D18) | — |
| VirtualBox, VMware, IDE disks | M7 (AHCI) | — |

And what 1.3 never had, which M6 also checks in D: two screens (a window dragged from one to
the other changes scale; a screen going moves its windows; mirroring), the clipboard between
apps, the file picker, the permissions viewer (Take back, Stop), keyboard layouts (`£`, `ê`),
and a hung app marked "not responding" while the rest carry on.

### 8. "Every app sandboxed"

- Every app, the desktop, picker and permissions viewer included, is started by the session's
  sandbox manager from its manifest; nothing with a window is started any other way (the
  display server only gives windows to capabilities the sandbox manager asked for).
- For each running app in the desktop boot, what the permissions viewer shows, what `grants`
  says, and what the app's own tree says (section 6) are the same, and are what its manifest,
  the test user's rules and the choices made give it.
- `tests/programs/windowprobe`, a hostile windowed app, tries each of note 0022's guarantees and
  the picker's rules, and every attempt fails: another window's pixels, input sent elsewhere,
  grabbing, the focus, faking a system window with a popup, pasting unasked, copying in the
  background, the picker from a window not just used, a second picker, files outside its grants,
  handle numbers it wasn't given, more windows and memory than its limits.
- Only Terminal, Files and the picker are trusted (shown as trusted on their windows and in the
  permissions viewer; the picker's windows are system windows).

### 9. The order of building

Each stage ends with `cargo xtask ci` passing on all four configurations, small commits pushed
to `m6` on GitHub (and fetched into Chris's folder on Harriet), the notes and DEVLOG brought up
to date, and a report; Stress is run where marked.

| Stage | What | Who |
|-------|------|-----|
| 0 | Notes 0019 to 0026 accepted; driver start-up measured in a release build (M5's leftover); the QEMU set-ups of section 6 (virtio-gpu with two screens, tablets, `-no-reboot`) with M5's tests still passing; the optimised drawing profile | Opus |
| 1 | **Kernel 1.3** (note 0019) and its tests. Stress | Opus |
| 2 | **Screens**: `pbr.display` version 2, the framebuffer driver's buffers and 24-bit screens, `drivers/virtio-gpu`, tablets in virtio-input, the device self-test's patterns and screenshots, the console on version 2 | virtio-gpu, the tablet and the copying library by Sonnet (with `virtio-blk` and `virtio-input` as the pattern); the interface, the device manager's choice of driver and review by Opus |
| 3 | **`lib/draw`** and `lib/font`'s new characters, with `lib/testpng` and the reference pictures | Sonnet; review by Opus; the heading font, if any, decided with Chris |
| 4 | **The input service**: `pbr.seat`, `lib/keymap` and the three layouts, the console moved onto it, the users service's layout setting | `lib/keymap` and the layouts by Sonnet; the service and who may take the seat by Opus |
| 5 | **The display server**: `pbr.window`, sessions, composing, screens and scales, decorations, input routing, the clipboard, `lib/window`, the description lines, `windowprobe`, the desktop boot's first steps with test programs. Stress | Opus |
| 6 | **The session's desktop**: the manifest keys, app folders, settings, the sandbox manager's changes, `services/power`, text mode and falling back, `apps/desktop` | Opus; the desktop's drawing (wallpaper, taskbar, start menu) by Sonnet from 1.3's code |
| 7 | **sandstone**: the widgets, the harness, a gallery test program | Sonnet, to the API in note 0023, one group of widgets per brief; review by Opus |
| 8 | **The picker, the permissions viewer**, `lib/browse`, `check`, records per grant, `pbr.sysinfo`. Stress | Opus |
| 9 | **The apps**: Terminal, Notepad, Files, Calculator, Paint, Snake, About, with `lib/calc`, `lib/bmp`, `lib/snake` | Sonnet, one app per brief; review by Opus |
| 10 | **The "done when"**: the whole desktop boot of section 7; the review of everything M6 added that hands out, derives, checks or revokes capabilities; README, ROADMAP, DESIGN.md (Q8 settled, section 8's text brought up to date), every M6 note marked built; `2.0.0-alpha.1`; Stress with 120 boots of each kind; Chris tries it on Harriet | Opus; Haiku summarises the stress logs |

Stages 3 and 4 need only stage 2's interface, so they can be built side by side by subagents
while stage 2 is reviewed.

## Alternatives considered

- **Desktop tests inside the self-test boot.** The console and the display server both want the
  screens and the seat; a boot is one or the other. A separate desktop boot also tests the start
  a user actually gets.
- **Comparing screenshots for everything.** Any change to the look would break dozens of
  QEMU tests that are slow to update; the description lines check behaviour, the host tests the
  look.
- **Rewriting the apps as faithfully as possible**, without the picker or the clipboard. That
  would give a sandboxed Notepad that can only open `/Home/Documents`, which is less than 1.3
  (where Notepad could open anything).
- **Every app trusted, like 1.3.** It would meet "does everything 1.3 does" and fail "with every
  app sandboxed".

## Open questions

- A text editor for programmers (the "Software development" use), beyond Notepad.
- Pictures in other formats (PNG reading, for the picker's previews and Paint).
- Sound for Snake (M13).
- Installing apps from outside the start-up archive (Beyond).

## Changes
- 14 November 2026 (stage 6): the root program's stand-in is gone: the desktop boot runs the
  users service, the sandbox manager and the desktop app, and the root program's test thread
  launches the test programs as apps through `pbr.users.debug` (they are `apps/wintest` and
  `apps/windowprobe` now, with manifests). The desktop boot is three boots on each
  configuration (note 0024, Changes): fresh disks, the same disks again, and the fourth image
  `prettybigrock-texttest.img` (`text desktoptest`). The runner keeps screenshots of the
  desktop and the start menu beside the log. Section 6's live mirroring check waits for the
  Screens dialog.
- 13 November 2026 (stage 5): **the desktop boot runs from stage 5**, with test programs:
  - The build tool makes a third disk image, `prettybigrock-desktoptest.img`, with
    `desktoptest` on the kernel's command line. `cargo xtask qemu-test` boots each
    configuration twice, the self-test and then the desktop boot (`PBR_QEMU_BOOT=selftest` or
    `desktop` runs only one).
  - Until stage 6 builds the users service's and sandbox manager's parts, **the root program
    stands in for both** (`services/root/src/desktoptest.rs`): it asks the display server for
    a session and, for each test program, a window capability, which it gives the program at
    role 1003. The test programs are `tests/programs/wintest` (a well-behaved window that
    logs what it is sent; with `desktop`, the session's desktop arranging the screens) and
    `tests/programs/windowprobe` (the hostile app of section 8, as far as stage 5 can test
    it: no picker or file grants yet).
  - **The steps are code, not a list** (`xtask/src/desktop.rs`, not a `DESKTOP_STEPS` table):
    each waits for what the last one should have shown (a description, a program's log line)
    and works out where to click from it. The root program and the runner keep in step through
    the programs: the runner closes a window when it is done with it, the program ends, and
    the root program starts the next step.
  - **Screenshots check colours at points**, not whole pictures: a window's colour in its
    corner and middle, on the screen the description says it is on, at that screen's scale.
    Stage 6's wallpaper and empty desktop are where whole reference pictures start.
  - The runner waits until the input service uses the virtio keyboard and mouse and the display
    server knows the tablets before it sends anything: QEMU drops input sent to a virtio device
    whose driver isn't listening yet.
- 11 November 2026 (stage 2): the test machines have 1 GiB of memory, not 512 MiB. The
  services' budgets are fixed shares (note 0006, section 9); with a display driver's buffers
  added, 512 MiB no longer left room to restart the storage service, which needs its budget
  free while the one it replaces still holds its own. How the two virtio-gpu machines get
  their screens, and what their keyboard and mouse are tied to, is in note 0020's Changes.
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
