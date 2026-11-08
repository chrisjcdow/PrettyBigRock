# 0018: Supervising services, the console and text shell, and how M5 is tested

Status: Accepted · 2 November 2026, built 8 November 2026 · Related: DESIGN.md principle 6, sections
8 and 11; notes 0011 to 0017; milestone M5

## The problem

M5 adds several long-running services (storage, filesystem servers, users, sandbox managers, a
console) to M4's device manager and drivers. Note 0012 left open what happens when the device
manager itself dies, and asked M5 to settle supervision for every service. M5 also needs a way
to use all this by hand, a text shell, and tests that prove the "done when":
*a program sees exactly the files it was given, and a 1.3 data disk opens with its files
intact.* This note covers those, and the order of building.

## The design

### 1. Who starts whom

```
 root ──┬── devmgr ─────── drivers
        ├── storage ────── filesystem servers (one per volume)
        ├── console
        └── users ──────── sandbox manager (one per session) ─── apps
```

Every process in this tree is **tied** to the one that started it (note 0013): if a supervisor
ends, everything below it ends first, and only then is the supervisor's end reported. No
orphans are left holding devices, volumes or files.

### 2. `pbr.supervisor`: what a child may ask of whoever started it

Each supervisor serves this to its children on their start-up role 5 endpoint, with a badge per
child:

```
/// What a service may ask of its supervisor.
interface pbr.supervisor version 1;
/// An endpoint for a service this child may use: "devices", "storage", "console".
@1 fn lookup(name: string<32>) -> (service: handle<any>) error SupervisorError;
/// Keep this memory alive until `release_kept` (for DMA memory; section 4).
@2 fn keep(memory: handle<memory>) -> () error SupervisorError;
/// Release everything kept for this child's previous instance.
@3 fn release_kept() -> () error SupervisorError;
flexible enum SupervisorError { NotAllowed @1, NotReady @2, TooMany @3 }
```

- **Services find each other through their supervisor**, not through capabilities fixed at
  start. The root program has a fixed table of who may look up what: the storage service and
  the console may look up `devices`; the users service may look up `storage` and `console`.
  Nobody else may look up anything.
- After a service is restarted, its clients' capabilities to it fail with `PEER_GONE`; they ask
  their supervisor again, and get the new one. This is how every M5 service reconnects, and why
  services don't hold on to each other's endpoints beyond that.
- The device manager's existing parent interface (`pbr.driver.host`, for drivers) is unchanged.

### 3. Restarting

Supervisors restart a child that ends with the policy drivers already use (note 0012, section
4: at once, after 100 ms, after 1 s, then give up and log it), which moves from `lib/devices`
to a small host-tested library, `lib/supervise`, used by the root program, the device manager
and the storage service.

| What ended | Who notices | What happens |
|------------|-------------|--------------|
| A driver | the device manager | Restarted (note 0012). The storage service reconnects and repeats its request (note 0015). |
| The device manager | the root program | Its drivers have already ended (tied). Section 4, then it's restarted. |
| A filesystem server | the storage service | Restarted on the same volume (a RockFS volume left unclean is checked first). Every capability for that volume fails with `PEER_GONE`; whoever holds the volume's root asks the storage service for it again. |
| The storage service | the root program | Its filesystem servers have ended. Restarted. The users service gets `PEER_GONE`, looks up `storage` again, and restarts its sessions. |
| The console | the root program | Restarted. The shell's terminal fails; the shell ends and its session starts it again. |
| The users service | the root program | Its sessions and their apps have ended. Restarted, and the first administrator's session starts again. |
| A sandbox manager | the users service | Its apps have ended. The session is started again. |

Restarting everything below a failed service is deliberately simple: a service's capabilities
die with it, so its dependants start afresh with new ones, rather than trying to patch up old
state. Apps lose unsaved work when their filesystem or session restarts; on RockFS what they
saved is intact. A service that hangs is not detected in M5 (an open question, as in note
0012).

### 4. When the device manager dies: DMA

Drivers end with the device manager, but their devices may still be writing to memory by DMA
(note 0010, section 6). The device manager owns every driver's DMA memory (note 0011); if it
dies, the memory would be freed while a device might still be writing to it, and given to
someone else. So:
1. When the device manager creates DMA memory, it gives a copy to its supervisor with
   `pbr.supervisor.keep`. The root program holds these copies, so the memory can't be freed
   while it does.
2. When the device manager ends, the root program restarts it.
3. The new device manager's first act, before starting any driver, is to switch off bus
   mastering, memory and I/O decoding, MSI and MSI-X on every PCI function it finds (as note
   0012 does for one device after a crash). After that, no device can write to memory.
4. It then calls `pbr.supervisor.release_kept`, and the root program closes the old copies. The
   memory is freed.

### 5. The console

`services/console` gives the shell a terminal on the screen:
- it finds the display and keyboards through the device manager (looking up `devices`,
  section 2); the users service looks up `console` and hands a terminal to each session;
- the display driver takes the screen from the kernel's own text console
  (`console_screen_release`, note 0010) when a client first asks for the framebuffer; kernel
  messages carry on over serial;
- it draws text with 1.x's bitmap font, which moves from the kernel into a small shared library,
  `lib/font`, used by both;
- it turns key codes into characters with a US layout (a table in `lib/devices` beside the key
  codes; more layouts are the input service's, M6), and edits lines (backspace, left and right,
  history);
- **in self-test runs, everything it shows is also written to the kernel log**, so the test
  runner can check it from the serial port.

```
/// A text terminal. Served by the console to one client at a time.
interface pbr.terminal version 1;
@1 fn write(text: string<4096>) -> () error TerminalError;
/// Waits for a line typed by the user, edited, without its end-of-line.
@2 fn read_line(prompt: string<256>) -> (line: string<4096>) error TerminalError;
@3 fn size() -> (columns: u32, rows: u32) error TerminalError;
@4 fn clear() -> () error TerminalError;
flexible enum TerminalError { Busy @1, Closed @2 }
```

### 6. The text shell

`apps/shell` is an app with a manifest asking for `trusted` and `terminal` (note 0017), so it
sees its session's whole tree (`/Home`, and `/Volumes/…` for administrators). It is built on
`granite::fs` like any other program and has no powers beyond its session's. Its commands,
following 1.3's Terminal where it makes sense:

| Commands | |
|----------|--|
| `help`, `about`, `clear`, `echo` | |
| `ls [-a] [PATH]`, `cd`, `pwd`, `cat`, `info` | Looking around |
| `mkdir`, `write FILE TEXT`, `append FILE TEXT`, `cp [-r]`, `mv`, `rm` | Changing things |
| `volumes`, `df`, `check VOLUME`, `format VOLUME LABEL`, `setup VOLUME NAME` | Disks (administrators) |
| `whoami`, `users`, `adduser` | Users |
| `run APP [FILE…]`, `apps`, `grants APP`, `revoke APP PATH`, `stop APP` | Apps, and the text permissions viewer |
| `shutdown` | Saves everything and says it's safe to turn off (PrettyBigRock can't power off until M7) |
| `potato` | Not listed in `help`, as in 1.3 |

### 7. How M5 is tested

Small, but covering each decision. Long runs go to the Stress workflow on GitHub, as in M4.

**On the host** (`cargo xtask test`), with ordinary and hostile inputs:
- `lib/partition`: tables made by our build tool, by Linux's tools, synthetic ones, and random
  corruption of each;
- `lib/blockcache`: against a model and a simulated disk with a write cache, checking the
  ordering rules (note 0015, section 5) after random operations and simulated power cuts;
- `lib/fsserve` with an in-memory filesystem: every operation, every right, every malformed path
  and wrong kind, derivation and records; `lib/filetree`: normalising, `..`, matching entries,
  the folders above them, broken trees;
- `lib/rockfs` and `lib/fat`: note 0016, sections 4 and 5, including the cross-checks with
  `tools/rockfs.py`, 1.3's own code, `mkfs.fat`, `mtools` and `fsck.fat`;
- `lib/manifest` and the rules: every key, every refusal, and the decisions of note 0017,
  section 5;
- `lib/supervise`: the restart policy (moved from `lib/devices`, with its tests).

**In QEMU, on all four configurations**, after M4's tests, the root program runs:
1. **Kernel 1.2** (in the root program's `kernel12` tests, with `echo` and `probe`; the badge
   records' counts live in the kernel, so they're tested here rather than on the host): gone notices after closing, revoking, copying and
   moving, and a process ending; their order with messages and notifications; charges given
   back. Derived replies: the new capability is a child of the caller's (revoking the caller's
   revokes it; closing the caller's re-parents it); a derived entry in a call, two in a reply,
   and a full table are refused; a caller whose capability went gets nothing. Tied processes:
   stopped when their creator exits, faults or is stopped; cascading; the creator's end reported
   only after its tied children's; budgets returned. The wall clock is after the build date.
2. **Storage**: exactly the volumes on the test disks (partitioned RockFS, a whole-disk RockFS
   volume, FAT32, an empty disk, a disk with a damaged table); a RAM volume; a volume capability
   can't reach outside its volume; writes and flushes read back; reads keep returning correct
   data while the disk driver is crashed underneath (M4's crash tools).
3. **Files**, through the real RockFS server: every `pbr.file` operation; rights; paths that try
   to climb out; bad names; revoking a folder revokes the files opened through it in another
   process; records go when their gone notices arrive (the server's record count returns to
   where it started after 1000 opens); `sync` then crashing the filesystem server keeps the
   change, and an unsynced change after a crash leaves the file entirely as it was.
4. **The 1.3 disk** (note 0016, section 4): every file intact; writing and reading back, checked
   afterwards by the runner with `tools/rockfs.py` and 1.3's own code.
5. **FAT32** (note 0016, section 5): read and write, checked afterwards with `fsck.fat` and
   `mtools`.
6. **The sandbox: the "done when"** (note 0017, section 8). Two sessions (an administrator and a
   standard user, on a system volume the build tool prepares); a test app,
   `tests/programs/sandboxed`, launched from its manifest with a chosen file, reports everything
   it can see and tries everything it shouldn't; the root program compares what it saw with
   exactly what it was given.
7. **Supervision** (section 3): the RockFS server, the storage service and the device manager
   are each crashed (through self-test-only debug interfaces, as drivers are in note 0012); each
   comes back, the tree below it with it, and files read back intact. For the device manager,
   the kept DMA memory is released only after the new instance has switched the devices off.
8. **The shell**: the test runner types commands into the console through QEMU's control
   socket (as M4 typed keys), and checks what the console mirrors to the serial port: `ls`,
   `mkdir`, `write`, `cat`, `mv`, `rm`, `run`, `grants`, `revoke`, `volumes`, `shutdown`.

`cargo xtask qemu-test` attaches the test disks (fresh copies of each, not snapshots, since the
runner checks some of them afterwards), and waits for `Files self-test passed` after M4's
messages.

### 8. Order of building

Each stage ends with everything passing on all four configurations, small commits on the `m5`
branch, the notes and DEVLOG brought up to date, a bundle for Chris, and a report, since each
is a point where the plan can be changed.

| Stage | What | Who |
|-------|------|-----|
| 1 | **Kernel 1.2** (note 0013), its support in `granite` and the interface compiler, and its tests in `probe` | Opus |
| 2 | **Storage**: `lib/partition`, `lib/blockcache`, `services/storage`, `pbr.storage`, RAM volumes, the storage tests | Opus; extra hostile-input tests for `lib/partition` by Sonnet |
| 3 | **The file layer**: `pbr.file`, `lib/fsserve`, `lib/filetree`, `granite::fs`, an in-memory filesystem for tests | Opus |
| 4 | **RockFS**: `lib/rockfs`, `rockfs-rs`, `services/rockfs`, the cross-checks, `xtask classic-disk` and the 1.3 disk test | Port and cross-check scripts by Sonnet; power-cut tests, `classic-disk` and review by Opus |
| 5 | **FAT32**: `lib/fat`, `services/fat`, its tests | Sonnet; review by Opus |
| 6 | **Users, the sandbox manager and manifests**: `lib/manifest`, `services/users`, `services/sandbox`, the sandbox test (the "done when") | Opus |
| 7 | **Supervision**: `lib/supervise`, `pbr.supervisor`, restarts, the device manager's DMA, the supervision tests | Opus |
| 8 | **The console and the shell**: `lib/font`, `services/console`, `apps/shell`, the shell test | Sonnet; review by Opus, who also does the screen hand-over |
| 9 | **Documents and review**: every note brought up to date, README, ROADMAP, DEVLOG, status, the stress runs | Opus; Haiku summarises long stress logs |

Stages 4 and 5 can run while stage 3's review finishes, since they only need `lib/fsserve`'s
traits, which are settled first.

## Alternatives considered

- **Restarting services in place, keeping their clients' capabilities working.** It needs the
  kernel to move an endpoint's clients to a new receiver and the service to rebuild its records,
  which is far harder than restarting the dependants. Worth revisiting once there are services
  whose clients can't simply start again.
- **A separate supervisor program.** The root program already starts the system services; one
  more process would have to be given everything the root program has.
- **The device manager zeroing and keeping its DMA memory on its own death** (impossible: it's
  dead) or **the kernel delaying freeing DMA memory** (the kernel can't know when a device has
  stopped; only whoever switches devices off can).
- **The shell on the serial port** instead of a console. Simpler, but ARM's only serial port is
  the kernel's (note 0011), and Chris would use the shell on the screen.

## Open questions

- Services that hang rather than end (liveness checks), for drivers and services alike.
- Charging memory a supervisor keeps (the device manager's DMA memory) to the supervisor, so
  that a crashed device manager's whole budget isn't held until the next one releases it.
- Telling a client that a service has gone without its having to call (the users service
  checks every 500 ms).
- Keeping apps' work across a filesystem or session restart.
- Crash reports that survive a restart (the logging service).
- Keyboard layouts other than US (the input service, M6).
- A filesystem server's records are shared by all its clients: one program can open files until
  the server's limit or budget is reached, and the others are refused (note 0014's open
  question on limits per program).

## Changes
- 2 November 2026: first draft, for discussion with Chris; accepted by Chris the same day.
- 2 November 2026, stage 1: kernel 1.2's tests are in the program self-test, not `lib/captree`.
- 7 November 2026, stage 6: test 6 as built (`services/root/src/sandboxtest.rs`). The build
  tool makes the GPT test disk's `Home` volume a system volume (`chris`, an administrator, and
  `alice`), and the root program stands in for the shell through `pbr.users.debug`. Checked:
  who each session is and what each may ask the users service; launches refused (no such app,
  a broken manifest, a broken rules file); choosing (bad paths, overlaps, a capability that
  isn't a file's, a file the app gets anyway); grants and their reasons; the launcher can't
  stop the app through its process capability, `stop` can; the app sees exactly its grants,
  with every handle number tried and `..` from every entry; read-only stays read-only;
  `revoke` takes one app's folder (and a file opened through it) while another app's grant of
  the same folder keeps working; `deny`, `allow` and `distrust` obeyed, and trusted apps' whole
  trees (an administrator's with every volume, a standard user's with none); `add` and `setup`;
  and, started again told to ignore the system volume, users kept in memory with `owner`. The
  runner waits for `Sandbox self-test passed`.
- 8 November 2026, stage 7 (built): supervision as built, and what building it taught.
  - **Endpoints are moved to the services, not shared.** The kernel fails callers with
    `PEER_GONE` only once no capability that can *receive* on an endpoint is left. The root
    program used to keep a full copy of the endpoints it made for services, which would have
    left their clients waiting for ever after a crash. Each service's endpoint is now moved to
    it, and its supervisor keeps an unbadged copy that can only send and mint clients'
    capabilities. (The users service already did this for sandbox managers.)
  - **The root program's supervisor** (`services/root/src/supervisor.rs`) runs on a thread of
    its own, beside the self-test. It serves `pbr.supervisor` under a badge for each child *and
    each time it was started*, so a capability left over from an earlier instance is refused.
    The lookup table as built: the storage service may look up `devices`; the users service may
    look up `storage` (and gets the administrators' badge). The console comes in stage 8. Only
    the device manager may `keep`.
  - **Switching devices off** (section 4) is done by every device manager when it starts, the
    first one too. It covers every PCI function a driver is chosen for, not every function: a
    function nobody drives was never given to anyone, and one of them may be the display the
    kernel's own console is on. (A bridge must keep forwarding for the devices behind it.) MSI is
    switched off as well as MSI-X (`pbr_devices::pci::quiet`, host-tested). Drivers are now
    *tied* to the device manager; they weren't before.
  - **Budgets** (note 0006, section 9): a capability to anything an ended process made, its
    process capability included, keeps that process's *whole* budget taken from its parent's.
    So while the root program keeps the old device manager's DMA memory, the old device
    manager's 96 MiB stays taken, and restarting it needs room for a second one. In 512 MiB
    there is room, and the test checks it. Services let go of a dead instance's capabilities
    at once: the users service when it notices, the storage service at its next call to a
    driver, and the self-test before it crashes anything. Charging kept memory to whoever keeps
    it is an open question below.
  - **Filesystem servers** are restarted with the policy, on the same volume, by the storage
    service: not when the server said it couldn't serve the volume (damaged, unsupported), nor
    while shutting down. While one is due, the volume is `Starting`, and `open` answers
    `NotReady`.
  - **The storage service** finds the device manager through `lookup`. If the device manager
    restarts, the storage service finds it again, and finds each disk again under the same
    number (the new device manager finds devices in the same order). The disk must have the same
    size and the same read-only setting, or its volumes fail rather than reach a different disk.
    It waits up to 30 seconds for a driver or device manager to come back (10 before).
  - **The users service** finds the storage service through `lookup`, and notices what it
    depends on going by checking every 500 ms: the storage service, the system volume's root,
    and the volumes each session holds. If the storage service or the system volume went, every
    session starts afresh, with `users.txt` read again. If another volume went, only the
    sessions that held it start afresh. A sandbox manager that ends is restarted with the
    policy. A system volume in memory is lost with the storage service; a new one is made, with
    `owner` again.
  - **Test 7 as built** (`services/root/src/supervisetest.rs`): new self-test-only operations
    `pbr.storage.debug.stop_server` and `flush`, and `pbr.devices.debug.quieted`. Before
    crashing the storage service the test flushes the cache: a real crash loses what was only
    in the cache, and FAT32, which the runner checks afterwards, isn't made to survive that
    (RockFS is). The test checks that the kept memory was released by the new device manager,
    after the time it reports having switched its devices off, and that the right number of
    memory objects is kept again afterwards. The runner waits for `Supervision self-test
    passed`.
- 8 November 2026, stage 8 (built): the console and the shell.
  - **The screen hand-over** is in note 0010's Changes: only the display driver can take the
    screen, and it does so when the console first asks for the framebuffer.
  - **The console** (`services/console`, with `lib/terminal` and `lib/font`) is a fourth
    supervised service, started between storage and users, which may look up `devices`; the
    users service may look up `console`. One thread serves `pbr.terminal` and draws; a watcher
    thread finds the screen and keyboards (and finds them again after a driver or device
    manager restart), and a thread per keyboard reads its ring, waking the main thread through
    a notification bound to its endpoint. A newer `read_line` takes over from a waiting one,
    which is answered `Busy`: otherwise a shell started again while its predecessor's
    `read_line` waited would be refused until someone pressed Enter. The screen must have 4
    bytes a pixel for now (QEMU's firmware screens do); otherwise the console works without
    one.
  - **Who starts the shell**: the sandbox manager of the session that holds the terminal (note
    0017, Changes), not the console.
  - **The shell's commands** are as in section 6 except `check VOLUME`, left out of M5: it
    would need a check operation in every filesystem server. `run` takes the app's own
    arguments after `--` (`run APP [FILE…] [-- ARGS…]`). `shutdown` is `pbr.users.shutdown`.
    Its logic is in `lib/shell`, host-tested; `apps/shell` is the glue.
  - **Test 8 as built** (`services/root/src/shelltest.rs`): it runs straight after the
    supervision test, since the 1.3 disk test shuts the storage service down too. The console
    mirrors to the kernel log in self-test runs (`[console] <text>`, `[console:read]
    <prompt>` when it waits, `[console:clear]`); the runner types a command each time the
    console waits, through QMP into the virtio keyboard, a key at a time, and checks each
    answer (`SHELL_COMMANDS` in `xtask/src/qemu.rs`): `whoami`, `ls`, `mkdir`, `cd`, `pwd`,
    `write`, `cat`, `mv`, `rm`, an error, `run` with a chosen file, `grants`, `revoke`,
    `stop`, `volumes`, an unknown command and `shutdown`. The root program waits for the
    shutdown and starts a new storage service for the tests after. The GPT test disk is now
    attached writable, and `tools/check_shell_after.py` checks its system volume afterwards:
    shut down cleanly, undamaged, with the shell's file there and the removed one gone.
- 8 November 2026, stage 9 (review and documents).
  - **The tests as built run in this order**, after M4's: kernel 1.2, files, devices and the
    crash test, storage, FAT32, the sandbox, supervision, the shell, then the 1.3 disk (which
    shuts the storage service down) and the load test. Each is a step of the program self-test,
    so the runner's `Program self-test passed` means every one passed; it also waits for
    `Sandbox self-test passed`, `Supervision self-test passed` and `Shell self-test passed`, and
    afterwards checks the 1.3 disk, the FAT32 volume and the system volume with the Python
    tools, 1.3's own code, `fsck.fat` and `mtools`.
  - **M5's "done when", and how it is shown.** *A program sees exactly the files it was
    given*: test 6 launches `tests/programs/sandboxed` from its manifest in two sessions; from
    the inside it tries the first 512 handle slots (four generations of each), walks its whole tree
    from `/`, climbs with `..` from every entry, and must find exactly what the root program
    says it was granted, read-only where granted read-only; then a `revoke` takes one grant from
    one app while another app's grant of the same folder keeps working. Test 8 does the same
    through the shell (`run` with a chosen file, `grants`, `revoke`). *A 1.3 data disk opens
    with its files intact*: test 4 attaches a fresh copy of the disk 1.3 itself made
    (`tests/data/classic-1.3.img.gz`), compares all 35 folders and files with its manifest,
    changes six things and shuts down; the runner then has `tools/rockfs.py` and 1.3's own C
    code check it clean and holding exactly what was expected. Both pass on all four
    configurations.
  - **The review** (Opus) read what M5 added, with most care for everything that hands out,
    derives, checks or revokes capabilities. Two things changed: `sync` needs `Write` (note
    0014, Changes), and clients are never given an unbadged capability they could mint badges
    from (note 0008, Changes). Questions it raised are open questions in note 0017 (the system
    volume; the sandbox manager answering one request at a time) and below.
