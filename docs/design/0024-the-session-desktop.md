# 0024: The session's desktop: windows for sessions, app folders, settings, the desktop app and power

Status: Accepted · 9 November 2026 · Related: DESIGN.md D11, sections 7.1 and 8; notes 0017, 0018,
0019, 0021, 0022, 0023, 0025, 0026; milestone M6

## The problem

Notes 0020 to 0023 give PrettyBigRock screens, input, a display server and the libraries to
draw windows. This note joins them to M5's users, sessions and sandbox manager (note 0017):
how a session gets the screens and an app gets a window; the desktop itself (wallpaper,
taskbar, start menu, icons); each app's own folder and the user's settings (Chris's answer to
Q10); the sandbox manager's open questions from M5, which M6's many callers make pressing;
turning the machine off and restarting (Q11); and starting in text mode, or falling back to it
(Q5).

## The design

### 1. What the root program starts

```
 root ──┬── devmgr
        ├── storage
        ├── input          (note 0021)
        ├── display        (note 0022)    ┐ the desktop (the default)
        ├── console        (note 0018)    ┘ or text mode: one or the other
        ├── power          (section 7)
        └── users ──── sandbox manager ──── the desktop app, apps
```

- **The desktop is the default.** With `text` on the kernel command line, the root program
  starts the console instead of the display server (D11: recovery, new hardware). Self-test
  boots are text mode, as in M5 (note 0026, section 6).
- **The supervisor's lookup table** (note 0018, section 2) becomes:

  | Who | May look up |
  |-----|-------------|
  | storage | `devices` |
  | input | `devices` |
  | display | `devices`, `input` |
  | console | `devices`, `input` |
  | users | `storage`, `console`, `display`, `input` (a badge that can only set the layout), `power` |

### 2. Sessions and the display

- **The users service** looks up `display` and, for each session it starts, asks
  `pbr.display.sessions.start` for a session capability, which it gives to the session's
  sandbox manager at start-up role 1003, and makes it the active one (`activate`). In text mode
  there is no display server, and the session gets the terminal instead, as in M5.
- **Its 500 ms checks go** (an open question from M5): it sets receiver watches (note 0019) on
  the storage service, the console or the display server, the input service and each volume
  root it holds, and acts when one fires, as it did when a check failed.
- **If the display server restarts**, the users service gives the session a new session
  capability; the sandbox manager stops every app it gave `windows` to and starts the desktop
  app again (note 0022, section 11).

### 3. Manifests: new keys

| Key | Meaning | Granted to |
|-----|---------|-----------|
| `windows` | A `pbr.window` client capability (note 0022), at role 1003 | Any app, if the session has the display |
| `data` | Its own folder (section 4) | Any app |
| `picker` | The file picker (note 0025), at role 1004 | Any app with `windows` |
| `sysinfo` | Read-only system information (note 0026, section 3), at role 1005 | System apps |
| `system` | Its windows may be `System` windows (note 0022, section 6) | System apps only, and never by a rule |
| `desktop` | It is the session's desktop (below) | System apps only, and never by a rule |
| `sandbox view` | A `pbr.sandbox` capability that may look at apps and take grants back (section 9): the permissions viewer | System apps only, and never by a rule |
| `hidden` | Not listed in the start menu or on the desktop | (a hint, not a grant) |
| `icon NAME` | Which of the system's icons to show for it | (a hint, not a grant) |
| `opens EXT…` | The kinds of file it opens (`txt md`), for Files' "Open with…" (note 0025) | (a hint, not a grant) |

"System apps" are those shipped in the start-up archive (note 0017, section 5), as every M6 app
is; apps installed later can never be given `system` or `desktop`. Rules (`allow`, `deny`,
`trust`, `distrust`) apply as before, and gain the place name `data`, so that `deny snake data`
works. An unknown key still refuses the whole manifest.

The **client flags** the sandbox manager gives the display server for an app are: `Trusted`
for a trusted app, `System` for `system`, `Desktop` for `desktop`. The title is the manifest's.

### 4. Each app's own folder (Q10)

- An app whose manifest says `data` gets the folder `Users/<user>/Apps/<app>/` on the system
  volume, made by the sandbox manager the first time, **read and write**, in its tree at
  **`/Home/Apps/<app>`**, the same path the user sees it at (note 0014, section 8: the same
  paths mean the same thing for the user and every app in the session; Q10 suggested `/App`,
  but that would be the one path that differs). `lib/files` gives it as
  `Files::data_folder()`, so apps don't build the path themselves.
- No other app sees it, except trusted ones (Terminal, Files), which see the whole home.
- Snake keeps its high score there, the desktop its screen arrangement (section 6), Paint its
  last colour.

### 5. The user's settings

- **Each user's settings** are a text file beside their rules, `/System/settings/<user>.txt`,
  in the same style as manifests and rules:
  ```
  layout gb
  timezone +02:00
  ```
  | Key | Meaning |
  |-----|---------|
  | `layout NAME` | The keyboard layout (note 0021); `us` if absent |
  | `timezone ±HH:MM` | The offset from UTC for showing times; `+00:00` if absent |

  Unknown keys are kept but ignored (a later version may add some); a line that can't be read
  makes the file be treated as empty, with the line in the log. It is read when a session
  starts.
- **The users service** reads and writes it, with new operations (any user, for their own):
  ```
  struct Setting { key: string<32>, value: string<128> }
  @8 fn settings() -> (settings: list<Setting, 32>) error UsersError;
  @9 fn set(key: string<32>, value: string<128>) -> () error UsersError;
  ```
  `set` checks the value (a layout the input service has, a valid offset), writes the file, and
  applies it at once (the layout through the input service).
- It is in `/System` rather than the user's home for the same reason as rules: not because a
  setting is secret, but so that only the users service writes it, through checks.
- Time zones are offsets, as in 1.3's `tz`; named zones with summer time are an open question.

### 6. The desktop app

- **`apps/desktop`**, with a manifest asking for `windows`, `desktop`, `data` and `hidden`. The
  session's sandbox manager starts it when the session has the display, and starts it again
  when it ends (the restart policy), as it does the shell on the terminal.
- **What it holds:** its window client capability (with the `Desktop` flag), its own folder, a
  `pbr.sandbox` capability that may launch and stop apps (section 9), and a `pbr.users`
  capability that may only ask `whoami`, `settings`, `set` and the power operations (section
  7). **None of the user's files.**
- **What it shows**, carried over from 1.3 (Chris's answer to Q8; `classic/kernel/wm.c` is the
  reference for colours and shapes):
  - on each screen a `Background` window: the **Table Mountain at dusk** wallpaper, drawn at
    the screen's size and scale (1.3's drawing, the same on any size), and on the main screen
    the **desktop icons**, one per app in the start menu; a double click launches it;
  - on the main screen a `Panel` along the bottom: the **start button**, a **button for each
    window** (click to focus, again to minimise; highlighted when it wants attention; "(not
    responding)" when so), and the **clock** (local time, with the user's time zone);
  - the **start menu** (the start button, the Windows key or Ctrl+Esc): the user's display name,
    every app whose manifest has `windows` and not `hidden`, then **Restart** and **Shut down**;
  - the **window menu's** choices sent to it (note 0022, sections 6 and 10): **What can this
    app reach?** launches the permissions viewer for that app (note 0025); **Stop** asks the
    sandbox manager to stop it;
  - **Screens…** on a right click on the wallpaper: a dialog to arrange the screens (order,
    which is the main one, scale 1× or 2×, rotation, mirroring), applied with `arrange` and kept
    in its folder as `screens.txt`, one line per screen identity; the pointer's speed and which
    screen each tablet is tied to go there too.
- **Launching**: the desktop calls `prepare` and `start` on `pbr.sandbox`, and the sandbox
  manager tells the display server the launch came from the user (note 0022, section 4).

### 7. Turning off and restarting (Q11)

- **`services/power`** is a small service started by the root program with:
  - the machine capability (note 0019);
  - on x86-64 only: I/O port slices for the FADT's `PM1a` and `PM1b` control registers, and
    read-only device memory slices for the DSDT and SSDT tables, which the root program finds
    with `lib/platform`'s ACPI code.
- It serves `pbr.power` to the users service only (lookup `power`):
  ```
  interface pbr.power version 1;
  @1 fn off() -> () error PowerError;
  @2 fn restart() -> () error PowerError;
  flexible enum PowerError { NotSupported @1 }
  ```
  - `restart` calls `machine_power` with action 2 (on both architectures).
  - `off` on ARM64 calls `machine_power` with action 1. On x86-64 it finds the `_S5_` object
    in the DSDT or SSDTs by scanning their bytes, as 1.3 does, reads the two sleep-type values
    from the package that follows, and writes them with the sleep-enable bit to `PM1a` (and
    `PM1b` if there is one). **This is a stopgap**, labelled so in the code, until the ACPI
    interpreter (M7) reads `\_S5` properly; it lives in its own file so that it can be removed
    whole. If it can't find `_S5_`, it answers `NotSupported`.
- **`pbr.users` gains** `@10 fn power_off()` and `@11 fn restart()`, for any user in M6 (one
  person at the machine, as in 1.3). In order: the display server shows the shutting-down
  screen (`farewell`, note 0022), every session's apps are stopped, the storage service saves
  every volume (as `shutdown` does), then `pbr.power`. If turning off isn't supported, the
  screen says "PrettyBigRock has shut down. You can turn the computer off now." (1.3's
  `shutdown_screen`, worded for a real machine too).
- The shell's `shutdown` keeps its M5 meaning in text mode, and gains `poweroff` and `reboot`
  (1.3's names).

### 8. Text mode, and falling back to it (Q5)

- **`text`** starts the console and not the display server; the session gets the terminal and
  the shell, exactly as M5.
- **Falling back**: if the display server is given up on (the restart policy, note 0018), the
  root program starts the console in its place, the users service's watch on the display server
  fires, and it starts the session afresh with the terminal, so the user gets the shell. The
  kernel log says why. The root program doesn't try the display server again until the next
  start.
- The **Terminal app** in the desktop is not the console: it is `lib/shell` and `lib/terminal`
  in a window (note 0026), and needs no console at all.

### 9. The sandbox manager's changes

**Who may do what.** `pbr.sandbox` capabilities now carry what their holder may do, in the
badge the sandbox manager mints for each:

| Holder | May |
|--------|-----|
| Trusted apps (Terminal, Files, the shell) | everything below except `grant` |
| The desktop | launch (`prepare`, `choose`, `start`, `cancel`), `list`, `details`, `stop` |
| The permissions viewer (`sandbox view`; note 0025) | `list`, `details`, `grants`, `revoke`, `stop` |
| The file picker (started by the sandbox manager itself; note 0025) | `grant` |

Anything else answers `NotAllowed` (a new `SandboxError` value).

**New operations** (adding operations is a compatible change, note 0008, so `pbr.sandbox` stays
version 1):

```
/// Gives up a prepared launch.
@8 fn cancel(launch: u32) -> () error SandboxError;
/// More about an app than `list` says.
/// What an app holds besides files (each a start-up role, section 10).
bits AppServices: u32 { Windows @0, Picker @1, Sysinfo @2, Terminal @3, Data @4, Users @5,
                        Sandbox @6 }
struct AppDetails { id: u32, app: string<64>, title: string<64>, trusted: bool, system: bool,
                    services: AppServices, started: u64 }
@9 fn details(app: u32) -> (details: AppDetails) error SandboxError;
/// Gives a running app a file or folder the user chose (the file picker only): records it as
/// `Chosen`, reduces `node` to `rights`, and answers with the app's capability, derived from
/// the one the sandbox manager keeps, so that `revoke` can take it back.
@10 fn grant(app: u32, path: string<1024>, node: handle<pbr.file>, rights: FileRights)
        -> (granted: handle<pbr.file>) error SandboxError;
```

**Never held up** (M5's open question): the sandbox manager answers one request at a time, so
it must never wait for something it doesn't control. The only such wait is asking a chosen
capability's server what it is (`choose`, `grant`). That now happens on a helper thread with a
2-second limit: the request is answered when it finishes, and the sandbox manager serves other
requests meanwhile. (Reply capabilities can be kept and answered later, note 0008.)

**Limits**: at most 4 prepared launches per caller and 16 in all (`TooMany`); `cancel` gives one
back; a caller's launches go when its capability does (a watched badge). At most 32 running
apps, as before.

**Starting the desktop**: like the shell in M5, an ordinary launch from its manifest, made by
the sandbox manager itself, started again with the restart policy when it ends, unless the
session has lost the display.

### 10. Start-up roles, all together

| Role | What | When |
|------|------|------|
| 1000 | `pbr.sandbox`, badged with what the app may do (section 9) | Trusted apps, the desktop, the picker, the permissions viewer |
| 1001 | `pbr.users`, badged (the desktop's can do less, section 6) | Trusted apps, the desktop |
| 1002 | `pbr.terminal` | `terminal`, in text mode |
| 1003 | `pbr.window` | `windows` |
| 1004 | `pbr.picker` | `picker` |
| 1005 | `pbr.sysinfo` | `sysinfo` |
| 12, 100 to 355 | The file tree, as in M5 | Every app |

The sandbox manager itself gets the session's display capability at its own role 1003.

### 11. Testing

- **On the host**: `lib/manifest` with the new keys and their refusals (`system` or `desktop`
  asked by an app that isn't a system app, or given by a rule); the decision for each key; the
  settings file (every key, bad values, unknown keys kept); `pbr.sandbox`'s badge rights against
  every operation; the launch limits.
- **In QEMU, the self-test boot** (text mode): the sandbox test (M5's test 6) gains: an app
  with `data` sees its folder at `/Home/Apps/<app>` and another app doesn't; `deny APP data`;
  every operation through each kind of `pbr.sandbox` badge, refused where the table says;
  `cancel` and the limits; a chosen capability whose server never answers holds up only its
  own request while `list` is answered; `grant` records a `Chosen` grant that `revoke` takes
  back; `pbr.power` refused to anyone but the users service; settings `set` and read back.
- **In QEMU, the desktop boot** (note 0026, section 6): the desktop appears; the start menu
  opens with the Windows key and lists the apps; an app launched from it appears focused; the
  window menu's Stop and permissions choices; the screens arrangement from `screens.txt`;
  restart (QEMU's `guest-reset`) and power off (`guest-shutdown`) on all four configurations;
  `text` starts the shell; the display server stopped past its restart policy (a self-test-only
  switch) gives the shell.

## Alternatives considered

- **The desktop as part of the display server** (note 0022, alternatives): one program would
  hold every screen, all input and the launcher.
- **The desktop as a trusted app.** It would see every file of the user's for no reason: it
  launches apps, and launching needs only `pbr.sandbox`.
- **Each app's folder at `/App`** (Q10 as first written). Shorter, but the one place where an
  app's path wouldn't be the path the user sees.
- **Settings in the user's home.** Any trusted app could then change the layout under the
  user; settings go through the users service like rules.
- **Making the sandbox manager multi-threaded.** Its records (launches, apps, grants) would
  need locks everywhere; moving its only wait onto a helper thread settles the problem with
  none.
- **Turning x86 PCs off in M7 only.** One more gap from 1.3 in the alpha (Chris chose to close
  it, Q11).

## Open questions

- Logging in, several users at once, and who may turn the machine off while others are logged
  in.
- Named time zones with summer time.
- A settings app (rules, users, layouts, screens in one place), and editing rules through
  `pbr.users` (note 0017).
- Keeping apps across a display server restart (note 0022).
- Desktop icons for files and folders, not only apps.

## Changes
- 14 November 2026, built (M6 stage 6): sections 1 to 4 and 6 to 11, except what the note
  leaves to later stages (the picker and `grant`, stage 8; the Screens dialog, which needs
  sandstone, stage 7; the permissions viewer, stage 8: the window menu's "What can this app
  reach?" is passed to the desktop, which only logs it for now). Settled while building:
  - **`pbr.sandbox` gains `@11 apps`**: the start-up archive's apps whose manifest reads and
    whose program is there, with title, `icon`, `windows` and `hidden`, so that the desktop,
    which holds no files, can list them. Trusted apps and the desktop may call it. `@10` is
    kept for `grant`. `details` answers `AppDetails` (id, app, title, trusted, system,
    `AppServices` bits, started).
  - **Badges**: the kind of holder in the top byte (1 trusted, 2 desktop, 3 permissions
    viewer, 4 file picker), the app's number below; app badges are minted *watched*, so a
    caller's waiting launches go when every copy of its capability has. The self-test's
    capabilities, minted by the users service, have number 0 or the top bit set.
  - **Roles**: the sandbox manager gets the session's display capability at 1003 and the
    desktop's limited `pbr.users` at 1006; an app gets `pbr.sandbox` 1000, `pbr.users` 1001,
    the terminal 1002, its window capability 1003. The desktop's `pbr.users` has the session's
    badge plus 1<<40 and may only call `whoami`, `settings`, `set`, `power_off`, `restart`.
  - **A manifest asking for `system`, `desktop` or `sandbox view` refuses an app that isn't a
    system app** (`decide` answers `Refused::SystemOnly`, launching answers `BadManifest`),
    rather than quietly not granting them. `data`, `picker`, `sysinfo` as section 3; rules
    gain the target `data` (`deny snake data`, `allow snake data read`).
  - **When the display server restarts, the users service starts the session again** with
    the new display server's session capability, rather than handing the sandbox manager a
    new one: everything in a desktop session has windows (or is the desktop), so the result is
    the same, and it is far simpler. The sandbox manager still watches the display server and
    stops the apps with windows at once when it goes. A session restarted for the screen's
    sake isn't charged to its restart policy; the users service waits for the old one to end
    (up to two seconds, so its memory is back) and retries in half a second if the new one
    can't start yet.
  - **The supervisor's `lookup` gains the error `Stopped`**: the service wasn't started in
    this boot (the console when the display server runs, and the other way round) or was given
    up on. The users service tries `display`, then `console`.
  - **Who launched it**: the sandbox manager tells the display server that the user launched an
    app (`launched` = now) when a trusted app or the desktop asked, not for the apps it starts
    itself. The hostile test app therefore waits 2.5 seconds before its first window, so that
    the window doesn't take the focus its clipboard checks rely on not having.
  - **Budgets**: a session 48 MiB (the desktop 8, the test apps 4 to 8 each), the users service
    160 MiB, the power service 1 MiB.
  - **Power**: `pbr.power` (`off`, `restart`; `NotSupported`, `NotAllowed`); the display
    server's `halted` says on every screen that the computer can be turned off by hand. On x86
    the root program gives the power service the FADT's PM1a/PM1b control ports (roles 1001,
    1002) and read-only slices of the DSDT and every SSDT (from role 1010, where each table is
    in its slice passed as `table=ROLE,OFFSET,LENGTH` arguments); the `_S5_` scan is
    `lib/platform/src/s5.rs`. QEMU's sleep type for off is 0. The shell gains `poweroff` and
    `reboot`.
  - **The desktop's start menu** is a popup of its panel, opened with the serial the display
    server now sends with the start-menu key (note 0022, Changes); it may be as tall as the
    screen. Keys don't reach it (a panel never has the keyboard): the Windows key opens and
    closes it, a click outside closes it. The heading is the user's display name, at size 2
    until Chris chooses (note 0023).
  - **`screens.txt`**: `screen "IDENTITY" at X,Y scale S [rotation R] [mirrors "IDENTITY"]
    [main]`, `pointer SPEED`, `tablet "NAME" "IDENTITY"` (`lib/desktop/src/screens.rs`);
    applied at start and whenever the screens change. A screen's identity is the display
    server's (`RHT-1234-00000000 Virtual-1` for QEMU's virtio-gpu).
  - **Tests**: section 11's self-tests are `services/root/src/desktopparts.rs`. The desktop
    boot is three boots: the desktop on fresh disks ending in falling back to the shell and
    `reboot`; the same disks again, finding what the first saved, ending with Shut down from the
    start menu; and `text desktoptest` (a fourth image), the shell and `poweroff`. The desktop
    boot no longer checks mirroring live (stage 5's test desktop did it on the start menu key):
    that comes back with the Screens dialog; mirroring stays host-tested.
  - The BIOS boot partition grew to 32 MiB (note 0004, Changes).
- 13 November 2026, built ahead of the rest (M6 stage 5): section 1's lookup table rows for the
  display server (`devices`, and `input` as a consumer of the seat) and for the users service
  (`display`, badged for `pbr.display.sessions`). The users service doesn't use its row until
  stage 6; until then the desktop boot's root program stands in for it (note 0026, Changes).
- 12 November 2026, built ahead of the rest (M6 stage 4): section 5's settings, because the
  input service needs the layout. As written, with these points settled: `set` accepts only
  the keys this version knows (`layout`, `timezone`) and answers the new error `BadSetting`
  for anything else or a value that can't be used; keys it doesn't know that are already in
  the file are kept. `settings()` gives `layout` and `timezone` always, with their defaults
  if never set, then the file's other keys. There is one seat, so **the layout that applies
  is that of the user who has the terminal** (when the desktop has a login screen, whoever
  logged in there): another user's `set layout` is saved and waits. The shell has `settings`
  and `set KEY VALUE`. The time zone is saved and checked (-12:00 to +14:00) but nothing
  shows times with it yet.
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
