# M6 plan: the desktop (2.0 alpha), 9 November 2026

This is the plan Chris and Claude agreed before M6's design notes were written. It was kept in
the claude.ai Project as `claude/m6-plan.md` until 16 November 2026. The accepted notes (0019 to
0026) now settle everything here; where they differ, the notes win.

**Chris agreed with every recommendation in section 2 (Q1 to Q16) on 9 November 2026.** Notes
0019 to 0026 were drafted from them (one detail differs: an app's own folder is at
`/Home/Apps/<app>`, not `/App`; note 0024, section 4).
Branch `m6` started at `9345496` (same as main).

## 1. The design notes M6 needs

**0019 Kernel interface 1.3 (small, only what M6 needs).**
- Powering off and restarting: a capability-gated call (ARM64 needs PSCI, which only the kernel
  can call); on x86 the ports and the ACPI reset register stay in user space (Q11).
- Waiting for a service to go: a notification when an endpoint's receiver is gone, so the
  desktop and apps notice a restarted display server without polling (M5 left the users
  service checking every 500 ms).
- Nothing else unless the other notes find a need. Opus.

**0020 Screens: `pbr.display` version 2, and the virtio-gpu driver.**
- Several screens per device, each with its modes, size in millimetres (for the scale factor),
  rotation and a stable identity (so an arrangement can be remembered).
- Presenting: the client gives a memory object and the changed rectangles, and is told when it
  is on the screen (a fence, so a GPU can take over later without a new interface). One way for
  both drivers: virtio-gpu needs transfer-and-flush, the framebuffer driver copies, converting
  24-bit screens (1.3 and Roxy's VESA can be 24-bit; M5's console needs 32).
- Screens appearing and going (virtio-gpu's display events), and who may present (one client
  at a time, taken over as `pbr.input` does).
- virtio-gpu 2D (no 3D yet), on `lib/virtio`; taking the screen from the kernel console when
  the firmware's screen is the virtio-gpu one; how the device manager avoids two drivers for one
  device.

**0021 Input: the input service and keyboard layouts.**
- `services/input` owns every keyboard and pointer (from the device manager), merges them, and
  serves one consumer at a time: the display server, or the console in text mode. The console
  stops reading keyboards itself.
- Key events carry the position code, the character(s) the layout makes, modifiers and repeat
  (the service does its own key repeat, since drivers differ).
- Layouts: a small text format of our own, host-tested in `lib/keymap`, with dead keys (for
  ê, ë, ô); which layouts ship (Q9); where the user's choice is kept.
- Pointers: relative (PS/2, virtio mouse) and absolute (QEMU's tablet, which 1.3 supports and
  M4's virtio-input doesn't yet; touch screens later), mapped onto the desktop's screens.
- Restarts: a driver going, the input service going.

**0022 The display server and compositor.**
- `services/display`, supervised by the root program: owns the screens and input, composes
  windows, routes input to the window that should have it.
- The window protocol (`pbr.window`): surfaces, buffers, damage, commit, frame pacing, fences;
  kinds of window (normal, dialog, menu/popup, system); move, resize, minimise, close requests.
- One coordinate space with each screen placed in it; scale factors (windows told the scale of
  the screen they're mostly on, and redraw at it); mirroring; a screen going moves its windows.
- Decorations (Q2), buffers (Q3), the clipboard if it lives here (Q15).
- What it guarantees: an app can't see another window's pixels, can't take input it wasn't
  sent, can't grab the keyboard or pointer, can't draw something that looks like a system
  window (file picker, permissions), and can't stall the compositor (no waiting on apps;
  "not responding" from a ping).
- Crash and restart: what apps see, and how they reconnect.
- DESIGN.md Q8 (a Wayland front end) is decided here (Q1).

**0023 Drawing, windows and sandstone (the app-facing libraries).**
- `lib/draw`: pixels, rectangles, rounded shapes, gradients, alpha, clipping, damage, text with
  `lib/font` at integer scales, images. Used by the compositor, the console and every app.
  Host-tested against reference images.
- `lib/window`: the client half of `pbr.window`, an event loop that waits on the window, timers
  and notifications. A crate of its own beside `granite` (like `lib/files`, it calls generated
  interface code, which `granite` can't; Q6).
- `sandstone`: the widget toolkit on top: labels, buttons, text fields and a multi-line editor,
  lists and trees, scroll bars, menus, dialogs, a canvas, layout, keyboard focus and shortcuts,
  scale handling. Its logic is host-testable (fake window, rendered to memory).
- All MPL-2.0 (D16).

**0024 The session's desktop.**
- How a session gets the display (like the terminal in M5: the users service hands the display
  server's capability, badged per session, to the session's sandbox manager), and a manifest
  key, `windows`, without which an app can't make one.
- The desktop app (`apps/desktop`): wallpaper, taskbar with clock, start menu, desktop icons,
  the shutdown screen; it launches apps through `pbr.sandbox`.
- Each app's own data folder (note 0017's open question; Q10) and where per-user settings live
  (keyboard layout, screen arrangement, time zone).
- The sandbox manager answering more than one request at a time, cancelling a launch, and a
  limit per caller (M5's open question: the desktop, file picker and permissions viewer all use
  it now).
- Power off and restart (Q11); text-mode start and falling back to the console (Q5).

**0025 The file picker, the clipboard and the permissions viewer.**
- `pbr.picker`: open and save, folders, filters by type, overwriting; what the app receives
  (a capability for that file alone, mounted in its tree with `Files::mount`), recorded by the
  sandbox manager as `Chosen` so the permissions viewer shows it and `revoke` takes it back; only
  asked from a window that is on the screen and was just used.
- The clipboard: text and pictures; pasting only in answer to the user's own key press or click
  (choosing is granting), never by an app on its own.
- The permissions viewer: an app, and a "What can this app reach?" entry on every window's menu;
  showing grants and why, `revoke`, `stop`; which badge of `pbr.sandbox` may see and change what.
- "Open with…" from Files, through `prepare`/`choose`.

**0026 The apps, the M6 tests and the order of building.**
- Each 1.3 app rewritten: Terminal, Notepad, Files, Calculator, Paint, Snake, About, the desktop
  itself; each one's manifest and what it is granted (Q16); what each keeps from 1.3; the
  potato. Each app's logic in a host-tested library (as `lib/shell` was): a text editor model,
  BMP reading and writing (1.3's Paint files), the calculator, Snake.
- `check VOLUME` (1.3 has check and fix; left out of M5).
- The "done when" as a table: every 1.3 feature, where it is in 2.0, and the test that shows
  it.
- How the desktop is tested (Q13), the stages, and CI time.

## 2. Questions for Chris (with recommendations)

**Q1. A Wayland front end (DESIGN.md Q8).** *Recommend:* our own protocol, shaped so that a
Wayland front end can translate to it (surfaces, buffers, commits, frame callbacks, popups and
roles mean the same things), and the front end itself built as a separate translating service
when ported toolkits need it (M12). Following Wayland exactly now would bend the design around
Unix file descriptors and global objects.

**Q2. Who draws title bars and frames?** *Recommend:* the display server, showing the title from
the app's manifest (not one the app chooses), with system windows (file picker, permissions)
marked in a way apps can't imitate. Apps draw only their contents. Otherwise any app could draw
a convincing fake file picker. (Wayland's default is the opposite; we can still allow
app-drawn decorations later for ported apps.)

**Q3. Who allocates window buffers?** Revoking a memory capability unmaps it everywhere (note
0006), so if apps allocated their own buffers, an app could revoke one while the compositor
reads it and crash the desktop. *Recommend:* the display server allocates each window's
buffers and lends them to the app, with limits per window and per session (charging them to the
session is an open question for later).

**Q4. How the desktop is split into programs.** *Recommend:* a system-wide display server
(screens, composing, input routing, decorations), supervised by the root program; and a
per-session desktop app (wallpaper, taskbar, start menu, icons) launched by the session's
sandbox manager like any app, holding only the session's `pbr.sandbox`. A bug in the taskbar then
can't reach the screens, and the display server never holds anyone's files.

**Q5. Desktop, console and Terminal.** *Recommend:* the desktop starts by default; `text` on the
command line starts the M5 console and shell instead (D11: recovery, new hardware); if the
display server is given up on after its restarts, the system falls back to the console. The
Terminal app is `lib/shell` and `lib/terminal` in a window, a trusted app, with no console
involved.

**Q6. Names and places for the libraries.** You asked for the window toolkit "in granite", with
sandstone on top. The window client can't be inside the `granite` crate itself (the same reason
as `lib/files`: it calls generated interface code built on granite). *Recommend:* `lib/draw`
and `lib/window` as granite-level libraries beside it, and `lib/sandstone` the widget toolkit,
built in M6 (seven apps, the picker and the permissions viewer all need the same controls).

**Q7. Text in M6.** *Recommend:* bitmap fonts only: 1.x's font at whole-number scales (1× and 2×
tested), plus one larger bitmap font of our own for headings if the look needs it. Outline fonts
(TrueType) wait for M12, where the roadmap puts fonts. The protocol supports any scale from the
start.

**Q8. The look.** *Recommend:* 1.3's look carried over faithfully (Table Mountain at dusk,
rounded windows with shadows, the taskbar and start menu, the colours), tidied for scale factors
and several screens, rather than a redesign now.

**Q9. Keyboard layouts.** *Recommend:* US, UK and US-International (dead keys, so Afrikaans' ê,
ë and ô can be typed) in M6, chosen per user, with the format making others easy to add. Do you
want any others (Dvorak, German, French)?

**Q10. Each app's own folder.** *Recommend:* settle note 0017's open question now: an app whose
manifest says `data` gets `/Users/<user>/Apps/<app>/` (made at first launch) at `/App` in its
tree, read and write, and nothing else sees it except trusted apps. The desktop keeps its
settings there (screen arrangement, wallpaper); the keyboard layout and time zone go in a
per-user settings file the users service reads, beside the rules in `/System`.

**Q11. Power off and restart.** 1.3 powers off (it finds `\_S5` in the DSDT by scanning
bytes) and restarts; PrettyBigRock 2 can only say "it's safe to turn off". *Recommend:* do both
in M6: on ARM64 through PSCI (a kernel call, note 0019), on x86 a small power service using the
FADT's reset register and 1.3's `\_S5` scan, labelled temporary and replaced by the AML
interpreter at M7. The alternative is to leave power to M7 and accept that one gap.

**Q12. Which test configurations get which screens.** *Recommend:* x86-64 UEFI and ARM64 EL2 on
virtio-gpu with two screens (the second at scale 2); x86-64 BIOS and ARM64 EL1 keep the
firmware's single screen through the framebuffer driver. Both drivers are then tested on both
architectures. The pointer: QEMU's absolute tablet on three, the PS/2 mouse on BIOS.
(Unplugging a screen under `-display none` may not be possible in QEMU; if not, a
self-test-only switch in the driver stands in, and I'll say so.)

**Q13. How the desktop is tested.** *Recommend:* the M5 self-test stays as it is, and each
configuration gets a **second boot**: a normal start to the desktop, as you'd see it, driven by
the runner through QEMU's control socket (keys, pointer moves and clicks), with the display
server mirroring a description of the screen to the serial port (windows, titles, positions,
focus, scale; as the console mirrors text) and a few screenshots of each screen compared with
reference pictures where the content is fixed. Drawing code would be built optimised even in
test builds, or emulated x86 will be too slow. CI would grow from about 55 minutes to roughly
80 to 90.

**Q14. The alpha itself.** *Recommend:* version `2.0.0-alpha.1`, released on GitHub at the end
of M6 with disk images for QEMU, and `run-prettybigrock.bat` able to start it with one or two
screens. QEMU only: VirtualBox, VMware and Roxy need AHCI and USB (M7), so the 1.x releases'
other hypervisor images don't have a 2.0 equivalent yet.

**Q15. Where the clipboard lives.** *Recommend:* in the display server, per session, as
Wayland does: it alone knows which window has focus and that a key press really came from the
keyboard, which is what makes "paste only when the user pastes" enforceable. A separate service
would need tokens from the display server to know the same.

**Q16. What each app is granted.** *Recommend:*
- Terminal and Files: `trusted` (their job is the whole tree), and shown as trusted everywhere.
- Notepad: `documents`, plus anything chosen in the picker. Paint: `pictures`, plus chosen.
- Calculator, Snake: nothing but their window (Snake's high score in its own folder, Q10).
- About: its window and a read-only system information capability (memory, uptime, screens,
  disks).
- The desktop: the session's `pbr.sandbox` (launching), not the session's files.

## 3. Open questions from M5: settle in M6, or wait

**Settle in M6** (M6 needs them):
- The sandbox manager answering one request at a time; cancelling a launch; a limit per caller
  (0017): the desktop, picker and viewer all call it (note 0024).
- Each app's own folder (0017; Q10).
- Keyboard layouts other than US (0018; note 0021).
- Screens other than 4 bytes a pixel (status; note 0020).
- `check VOLUME`, since 1.3 has check and fix (status, 0018; note 0026).
- Telling a client a service has gone without its calling (0018; note 0019).
- A filesystem server's records shared by all clients (0014, 0018): cheap to settle in
  `lib/fsserve` once: each record remembers the grant it came through, with a limit per grant.
  With seven apps opening files at once it starts to matter.
- DESIGN.md Q8 (Wayland; Q1), as DESIGN.md says.
- Measuring driver start-up in a release build (status said "before M6"): stage 0.
- Stable identities, for screens only (0011's stable device names, the part M6 needs).

**Can wait:**
- Which volume is the system volume, before USB storage (0017): M7.
- Hot-plug of disks, stable volume names (0015): M7.
- AML instead of the built-in list, shared interrupt lines (0010, 0011): M7.
- KPTI and PCID (0007): before M7, as planned.
- Services and drivers that hang (0012, 0018): with the logging service. (Windows that hang are
  settled by the display server's ping.)
- Crash reports that survive (0012, 0018), keeping apps' work across restarts (0018), charging
  kept DMA memory to the supervisor (0018).
- A recovery session for a broken rules file, passwords and logins (0017): with logins.
- Being told when a folder changes (0014): 1.3's Files doesn't refresh on its own either; Files
  refreshes when it's shown and after its own changes. A flexible addition to `pbr.file` later.
- Moving between separately granted folders (0013, 0014), Unicode normalising (0016), owners
  on disk (0016, 0017), the network in manifests (M8), installing apps.
- Priority inversion across calls, imports between interface files, a C generator (0008).
- GICv3, IOMMU, non-coherent DMA, function-level reset, re-enabling devices, typed handles from
  `pbr.devices.open`: later milestones.
- DESIGN.md Q5 (cryptography), Q6 (a kernel name), Q7 (Linux programs).

## 4. Stages

Each stage ends with `cargo xtask ci` passing on all four configurations, small commits pushed to
`m6`, the notes and DEVLOG up to date, Harriet's folder fetched, and a report. Stress is run at
the stages marked.

| Stage | What | Tests | Who |
|---|---|---|---|
| 0 | Notes 0019 to 0026 accepted; driver start-up measured in a release build; QEMU set-ups for virtio-gpu, two screens and the tablet; optimised drawing in test builds | Existing CI still green with the new QEMU set-ups | Opus |
| 1 | Kernel 1.3 (note 0019) | Power calls refused without the capability; restart works on all four (last test in a boot); gone notifications in order with messages, after restarts; Stress | Opus |
| 2 | Screens: `pbr.display` v2, framebuffer driver (24-bit), virtio-gpu driver, absolute pointer in virtio-input | Device self-test draws a known pattern on every screen, runner screenshots each one and compares; present with damage; one client at a time; virtio-gpu crashed and restarted; a screen going and coming | virtio-gpu and the tablet by Sonnet (virtio-blk as the pattern); `pbr.display` v2 and review by Opus |
| 3 | `lib/draw`, fonts at scale | Host: reference images for every primitive, clipping, alpha, text at 1× and 2×, hostile sizes | Sonnet; review by Opus |
| 4 | Input service, `lib/keymap`, console moved onto it | Host: every layout, dead keys, repeat, modifiers; QEMU: M5's shell test passes through the input service, a line typed in each layout, keyboard driver and input service crashed | Layouts and `lib/keymap` by Sonnet; the service (who receives input) by Opus |
| 5 | Display server: `pbr.window`, composing, several screens, scale, decorations, focus and input routing, clipboard, `lib/window` | Test programs (no desktop yet): windows appear where asked; a window moved between screens is told the new scale and redraws; a screen going moves its windows; mirroring; input reaches only the focused window; an app can't read another's buffer, grab input, paste unasked, or look like a system window; a hung app is "not responding" and the rest carry on; display server crashed, apps reconnect; Stress | Opus |
| 6 | The session's desktop: `windows` and `data` in manifests, the sandbox manager's changes, settings, the desktop app, power, text-mode fallback | Second boot starts to the desktop; the runner opens the start menu and launches a test app; the desktop's description matches; shutdown and restart; `text` boots the console; display server given up on falls back to the console | Sandbox manager, manifests, settings, power by Opus; the desktop's visuals by Sonnet |
| 7 | sandstone | Host: each widget's behaviour and look (reference images), keyboard navigation, scale 1× and 2×; a widget gallery test program in QEMU | Sonnet, to an API Opus writes in note 0023; review by Opus |
| 8 | File picker, permissions viewer, `pbr.sandbox` rights split, `check VOLUME`, per-grant record limits | Picker: open and save, overwrite, refused when not asked from a used window; the chosen file appears in grants and `revoke` takes it; permissions viewer shows exactly each app's grants; `check` finds a damaged test disk; Stress | Opus |
| 9 | The apps: Terminal, Notepad, Files, Calculator, Paint, Snake, About, the potato | Host: each app's logic library (editor, BMP with 1.3's own files, calculator, Snake); QEMU: each app driven by the runner (below) | Sonnet, one app per brief; review by Opus |
| 10 | The "done when", documents, review, alpha | The parity run below; the full review of everything that hands out or checks capabilities; Stress (120 boots); README, ROADMAP, DESIGN.md (Q8), notes marked built, `2.0.0-alpha.1` | Opus; Haiku summarises stress logs |

## 5. What "done when" means in practice

*PrettyBigRock 2 does everything 1.3 does, with every app sandboxed.*

**Everything 1.3 does** becomes a table in note 0026: each 1.3 feature (from its README, its
start menu and its Terminal's commands), where it is in 2.0, and the automated check that shows
it. The desktop boot (Q13) walks through it as a user would:
- the desktop appears with the wallpaper, taskbar, clock and icons, on both screens where there
  are two; the start menu opens with the Windows key and Ctrl+Esc;
- windows are dragged (including from one screen to the other, changing scale), focused,
  minimised and closed (Ctrl+W);
- Terminal runs a set of commands (the M5 set plus 1.3's `date`, `uptime`, `mem`, `ver`,
  `fortune`, `tz`, `check`, `potato`);
- Notepad: type, Ctrl+S, save through the picker into Documents; open it again; open a text
  file from the 1.3 disk;
- Files: browse, make a folder, rename, delete, double-click a text file to open it in Notepad
  and a picture to open it in Paint;
- Paint: draw, save a .bmp, open it again; open a picture 1.3's Paint saved;
- Calculator: a sum; Snake: starts, moves, ends; About: shows the screens, memory, uptime and
  disk;
- copy in Notepad, paste in Terminal;
- shut down and restart; `potato` still answers.

Afterwards the runner checks the disks with `rockfs.py` and 1.3's own code, as M5 did.

**Every app sandboxed:**
- every app, the desktop included, is started by the sandbox manager from its manifest; nothing
  windowed is started any other way;
- for each app, what the permissions viewer shows equals what its manifest, the rules and the
  user's choices give it, and the test checks it against what the app itself can reach (as
  M5's test 6 did, from the inside);
- a hostile windowed test app tries what it shouldn't: other windows' pixels, others' input,
  grabbing the keyboard, faking a system window, pasting unasked, the picker from a hidden
  window, files outside its grants, handle numbers it wasn't given; all refused;
- only Terminal and Files are trusted, and they are shown as trusted.

**And by hand:** you run it on Harriet with `run-prettybigrock.bat` (one and two screens) and
say whether it feels like 1.3. That's the alpha's real test.

**What 2.0 alpha won't do that 1.3 does:** run in VirtualBox or VMware, on IDE disks, or on Roxy
(M7); the browser player stays on 1.x (D18). (Power off too, if Q11 goes the other way.)
