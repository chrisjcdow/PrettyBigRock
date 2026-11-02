# 0018: Supervising services, the console and text shell, and how M5 is tested

Status: Draft · 2 November 2026 · Related: DESIGN.md principle 6, sections 8 and 11; notes 0011
to 0017; milestone M5

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
- `lib/captree`: badge records' counts through copies, moves, closes and revocations, against a
  model with random operations (note 0013);
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
1. **Kernel 1.2** (in `probe`, as before): gone notices after closing, revoking, copying and
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
- Keeping apps' work across a filesystem or session restart.
- Crash reports that survive a restart (the logging service).
- Keyboard layouts other than US (the input service, M6).

## Changes
- 2 November 2026: first draft, for discussion with Chris.
