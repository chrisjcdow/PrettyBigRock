# 0017: Users, the sandbox manager and app manifests

Status: Accepted · 2 November 2026 · Related: DESIGN.md D5, D6, principle 5, sections 7.1 to 7.3;
notes 0011, 0014, 0015, 0018; milestone M5

## The problem

DESIGN.md section 7.1: apps start empty, and a **sandbox manager** decides what each one gets,
from what its **manifest** asks for, the **rules** the user has set, and what the user is doing
at that moment. **Users** own files and settings, each with a home; logging in hands the session
that user's capabilities, and the session passes slices of them to apps (D6: users answer whose
files these are; capabilities answer what this app may touch). M5 builds the first versions of
all three, which is what makes *a program sees exactly the files it was given* true for apps.

This note settles where users and their homes live, what a session is, the manifest format, the
rules, how an app's file tree is decided and built, and what can be seen and taken back while
it runs.

## The design

### 1. The parts

```
 root ──starts──▶ users ──starts one per session──▶ sandbox manager ──launches──▶ apps
          │         (the system volume,                (only this user's            (each with
          │          the user records)                  capabilities)               its own tree)
          └──gives pbr.storage to users
```

- **The users service** (`services/users`) owns the system volume's `System` and `Users`
  folders, knows the users, and starts **sessions**.
- **A session** is one user, logged in, with a **sandbox manager** of their own
  (`services/sandbox`), tied to the users service (note 0013). It holds that user's
  capabilities and nobody else's, so a mistake in it can't leak another user's files: it never
  had them.
- **Apps** are launched by their session's sandbox manager, tied to it, each with a file tree
  (note 0014, section 8) decided as below.

### 2. The system volume

- The **system volume** is a RockFS volume with a folder `System` at its root that holds
  `users.txt`. The users service uses the first such volume the storage service lists.
- Its layout:
  ```
  /System/users.txt              the users
  /System/rules/<user>.txt       each user's rules
  /Users/<user>/                 each user's home, with Documents, Pictures, Music, Videos
                                 and Downloads made when the user is
  ```
- **If there's no system volume**, the users service asks the storage service for a RAM volume
  (note 0015), sets it up with one administrator called `owner`, and says so: *"There's no
  system disk, so your files are kept in memory and are gone after a restart."* This is what
  1.3 does without a data disk.
- **Making a volume the system volume is a deliberate step**: an administrator's `setup VOLUME
  NAME` (in the shell) adds `System/` and `Users/NAME/` to a RockFS volume, or formats an empty
  disk first, and touches nothing else on it. It takes effect at the next start. A 1.3 data
  disk is therefore never changed just by being attached: it appears as an ordinary volume
  (below), and its files stay where they are.

### 3. Users

`users.txt` has one line per user, in the same plain style as manifests (section 4):

```
# PrettyBigRock users
user chris admin "Chris"
user alice standard "Alice"
```

- `user NAME KIND "DISPLAY NAME"`. `NAME` is 1 to 32 lowercase letters, digits, `-` and `_`
  (it names the home folder); `KIND` is `admin` or `standard`; the display name is UTF-8.
- **What a session holds:**
  - every user: their home folder, read and write, at `/Home`; their rules file, read only;
  - administrators also: every volume's root folder, at `/Volumes/<label>` (the label made
    unique with a number if two are the same), and the storage operations (`format`, `setup`)
    through the users service.

  These paths are the **session's paths** (note 0014, section 8): the user sees them in the
  shell, and every app in the session sees a part of the same tree at the same paths.
- **Logging in.** M5 has no passwords and no login screen: passwords need cryptography
  (DESIGN.md Q5), and logins for several people are on the roadmap's "Beyond" list. At start,
  the users service starts a session for the first administrator in `users.txt`. The self-test
  starts a second session, for a standard user, to show that sessions are kept apart.
- **No owners on the disk yet.** In M5, whose files these are is answered by where they are
  (`/Users/<name>`) and which capabilities a session holds. Recording an owner on each file
  (a RockFS feature flag, DESIGN.md section 12) waits until several users share volumes in ways
  that need it.
- The users service serves `pbr.users`, with a badge per session (so it knows who's asking):
  ```
  interface pbr.users version 1;
  enum UserKind { Admin @1, Standard @2 }
  struct User { name: string<32>, kind: UserKind, display: string<128> }
  /// Who this session is.
  @1 fn whoami() -> (user: User) error UsersError;
  /// Every user (administrators only).
  @2 fn list() -> (users: list<User, 64>) error UsersError;
  /// Makes a volume the system volume from the next start (administrators only).
  @3 fn setup(volume: u32, name: string<32>) -> () error UsersError;
  /// Adds a user, with a home (administrators only).
  @4 fn add(user: User) -> () error UsersError;
  /// The storage service's volume list, and formatting an empty disk (administrators only).
  @5 fn volumes() -> (volumes: list<VolumeInfo, 32>) error UsersError;
  @6 fn format(volume: u32, label: string<64>) -> () error UsersError;
  ```
  (`VolumeInfo` and, in `pbr.sandbox`, `FileRights` are copied from `pbr.storage` and
  `pbr.file` until interfaces can import each other; note 0008.)

### 4. App manifests

An app is a program in the start-up archive at `apps/<name>`, with its **manifest**,
`apps/<name>.app`, beside it. (Installing apps from elsewhere comes later.) A manifest is a
small text file in the same style as a driver's (note 0011, section 5): one `key value…` per
line, `#` for comments, at most 4 KiB. Values are ASCII, except the title, which may be UTF-8 in
quotes.

```
# apps/notes.app
title "Notes"
budget 16M
files documents read write
files pictures read
```

| Key | Meaning |
|-----|---------|
| `title "…"` | The name people see |
| `budget SIZE` | Its memory budget (suffixes `K`, `M`) |
| `files PLACE read [write]` | Asks for one of the user's places: `documents`, `pictures`, `music`, `videos`, `downloads`. Any number of lines, each place once |
| `trusted` | Asks for the user's whole tree, as the shell and backup tools need. Shown as such everywhere |
| `terminal` | Asks for the session's terminal (note 0018) |
| `args TEXT` | Arguments to start it with |

Unknown keys, repeated keys (except `files` for different places), bad sizes and overlong lines
refuse the whole manifest, and the app can't be launched; the reason is logged. The parser is in
a host-built library, `lib/manifest`, tested with ordinary and hostile manifests. The `.drv`
parser in `lib/devices` moves onto its line-reading half, so there's one reader for both.

### 5. Rules

Each user's rules are a text file, `/System/rules/<user>.txt`, in the same style:

```
deny paint pictures
allow notes documents read
trust backup
```

| Rule | Meaning |
|------|---------|
| `allow APP PLACE read [write]` | The app may have that place, with at most those rights |
| `deny APP PLACE` | The app may never have that place |
| `trust APP` | The app may be trusted, if its manifest asks |
| `distrust APP` | It may not |

**What an app gets** for each thing its manifest asks for:
- a place: granted if a rule allows it, or the app is a **system app** (one shipped in the
  start-up archive, as all M5 apps are) and no rule denies it. The rights are what the manifest
  asks for, cut down to what an `allow` rule gives, if there is one;
- `trusted`: granted only to a system app that no rule distrusts, or to an app a rule trusts;
- `terminal`: granted to trusted apps.

A session can read its own rules but not change them, so an app given the user's whole tree
can't grant itself more. In M5 an administrator edits rules files as text, through the system
volume's root under `/Volumes`; commands for it through `pbr.users`, and a settings app, come
later.

### 6. What the user is doing: grants at launch

**Choosing is granting** (principle 5). In M6 that is the file picker; in M5 it is the shell:
`run notes /Home/Documents/letter.txt` opens that file in the shell's own tree and passes the
capability to the sandbox manager with its path. The app gets it, mounted at the same path, and
nothing else in that folder.

### 7. Launching: `pbr.sandbox`

The session's sandbox manager serves `pbr.sandbox` to the session's trusted programs (the shell
in M5):

```
interface pbr.sandbox version 1;

enum GrantReason { Manifest @1, Rule @2, Chosen @3, Trusted @4 }
struct Grant { path: string<1024>, rights: FileRights, reason: GrantReason }
struct AppInfo { id: u32, app: string<64>, trusted: bool, running: bool }

/// Starts preparing a launch.
@1 fn prepare(app: string<64>, args: string<3072>) -> (launch: u32) error SandboxError;
/// Adds a file or folder the user chose, at `path` in the app's tree.
@2 fn choose(launch: u32, path: string<1024>, node: handle<pbr.file>) -> () error SandboxError;
/// Starts the app. The process capability can be watched and inspected, not stopped.
@3 fn start(launch: u32) -> (app: u32, process: handle<process>) error SandboxError;
@4 fn list() -> (apps: list<AppInfo, 32>) error SandboxError;
/// What an app holds, and why: the permissions viewer.
@5 fn grants(app: u32) -> (grants: list<Grant, 64>) error SandboxError;
/// Takes back what an app holds at `path` (and everything it opened through it).
@6 fn revoke(app: u32, path: string<1024>) -> () error SandboxError;
@7 fn stop(app: u32) -> () error SandboxError;
```

**Launching**, in order:
1. Read the app's manifest from the start-up archive; refuse it if it's malformed.
2. Decide the grants (section 5), and add the chosen ones (section 6).
3. For each place granted, take the session's capability for it and **`reduce`** it to the
   rights granted (note 0014). The result is a new, derived capability for this app alone, so
   revoking it affects no other app. Chosen files are reduced the same way.
4. Build the app's file tree: each grant at its session path (`/Home/Documents`,
   `/Home/Documents/letter.txt`); a trusted app gets the session's whole tree.
5. Create the process **tied** to the sandbox manager, with the manifest's budget, the kernel
   log (without `CONTROL`), the file tree, and the terminal if granted, and start it.
6. Keep a record: the app, its process, and each grant with the capability it was derived from
   (the sandbox manager keeps the parent, so it can revoke).

**While it runs:** `grants` lists what it holds and why; `revoke` revokes one grant's capability,
which takes back everything the app opened through it (note 0013); `stop` stops it. When an app
ends, its record is removed; its capabilities went with its process.

**What the sandbox manager can't do:** it can't give an app anything the session doesn't hold,
because it holds nothing else.

### 8. What "sees exactly the files it was given" means

For an app launched by the sandbox manager, M5's tests (note 0018) check that:
- walking its whole tree from `/` finds exactly its grants and what's inside the granted
  folders, and nothing else;
- no path reaches anything else: `..` from every entry, absolute paths into other places, names
  of other users, other volumes;
- no handle number it wasn't given works (it tries every slot in its table);
- a read-only grant can't be written, created in, renamed or removed in;
- after `revoke`, the grant and everything opened through it fail, while other apps' grants of
  the same folder keep working;
- a standard user's session can't reach another user's home or any volume root;
- rules are obeyed: `deny` removes a place, `allow` cuts rights down.

## Alternatives considered

- **One sandbox manager for every session.** One fewer process, but it would hold every user's
  capabilities, and one bug could hand one user's files to another's app.
- **Unix users and permission bits as the protection.** They answer "who is this?", not "what may
  this app touch?", which is the question sandboxing needs (D6). Owners on disk may still come
  later, for sharing.
- **Asking the user at every launch.** Prompt fatigue (DESIGN.md section 7.3). Manifests plus
  rules decide quietly; choosing a file is the only prompt.
- **Manifests inside the program file.** Harder to read and check by hand, and the driver
  manifests already set the pattern of a small text file beside the program.
- **Rules in the user's home.** Any trusted app (and the user) could then rewrite them without
  going through the users service, and they would sit beside files apps are given.

## Open questions

- Passwords and a login screen (with DESIGN.md Q5), and several people logged in at once.
- A private folder for each app's own settings and data (M6's apps will need one).
- Installing apps, with their manifests shown and approved, and signed packages.
- Owners recorded on disk, for volumes shared between users.
- The network as a capability in manifests (M8).
- Editing rules through `pbr.users` (with the settings app).
- A recovery session that ignores a user's rules, for when an administrator's own rules file is
  broken (launches are refused until it is fixed; see Changes).

## Changes
- 2 November 2026: first draft, for discussion with Chris; accepted by Chris the same day.
- 7 November 2026, stage 6 (built): decisions the note left open, settled while building.
  - **The rules file in the session's tree** is at `/System/rules.txt`, read only. Trusted apps
    see it there too, which is how "a session can read its own rules" shows.
  - **Rules are read afresh at every launch**, so an administrator's edit applies to the next
    launch without restarting anything. Two rules about the same app and place (an `allow` and
    a `deny`, or the same rule twice), or `trust` with `distrust`, refuse the whole file.
  - **A rules file that can't be read or parsed refuses every launch** for that user (error
    `BadRules`, with the line in the log), rather than guessing: a line it can't read might have
    been a `deny`. Running apps carry on. A way back in if an administrator breaks their own
    rules file (a recovery session) is an open question below.
  - **A damaged `users.txt`** is treated like no system volume (the `owner` session, in
    memory), except that the message says what is wrong; `owner` is an administrator, so it
    sees the system volume under `/Volumes` and can repair the file.
  - **Trusted apps also get** the session's `pbr.users` and a `pbr.sandbox` capability with
    their own badge (start-up roles 1001 and 1000), which the shell will need. A trusted app's
    tree is the session's, each entry a capability of its own (so `revoke` works on it too).
  - **A distrusted app whose manifest asks for `trusted`** still launches, with its places.
  - **A chosen file or folder inside something the app gets anyway** is accepted and not added,
    if what it gets covers the chosen capability's rights; otherwise, or if the chosen path would
    contain something the app gets, `choose` answers `Overlaps`. A chosen capability's server
    is asked what it is with a 10-second limit (`BadCapability` if it doesn't answer as a file).
  - **`grants` pages** (`grants(app, from) -> (grants: list<Grant, 32>, next)`): 64 grants of
    1 KiB paths don't fit in one message, and a trusted app can hold up to 256 entries.
    `SandboxError`'s values are in `interfaces/pbr.sandbox.pbi`; `pbr.users`' errors in
    `pbr.users.pbi`.
  - **Volume names under `/Volumes`**: the label, or `Volume` if it is empty or not a valid name;
    later duplicates get ` 2`, ` 3`… (`pbr_manifest::volume_names`).
  - **Budgets**: a session (its sandbox manager and every app it starts) has 16 MiB; the volume
    in memory used without a system volume is 8 MiB. An app's manifest `budget` is 64 KiB to
    1 GiB, its `title` at most 64 bytes, its `args` at most 1 KiB (the launch's own arguments
    follow them).
  - **`terminal`** is parsed and decided, but nothing is handed over until the console exists
    (stage 8). Starting a session again when its sandbox manager ends is stage 7.
  - **Testing**: `pbr.users.debug` (self-test runs only, like `pbr.storage.debug`) gives the
    root program each session's `pbr.users` and `pbr.sandbox`, so it can stand in for the shell.

