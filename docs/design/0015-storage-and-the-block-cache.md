# 0015: Storage: disks, partitions, volumes and the shared block cache

Status: Accepted · 2 November 2026 · Related: DESIGN.md section 12; notes 0011, 0012, 0014, 0016,
0018; milestone M5

## The problem

Between the disk drivers (note 0011) and the filesystems (note 0016) something has to:
- find the volumes on each disk, by reading its partition table (DESIGN.md section 12: the
  storage stack reads partition tables, not filesystems);
- start a filesystem server for each volume, and let it reach **only its own volume**. A FAT32
  server reading a stranger's USB stick is parsing hostile data, and must not be able to touch
  the other partitions on the same disk;
- keep **one block cache shared by all filesystems** (DESIGN.md section 12), while leaving the
  order of writes to each filesystem, as copy-on-write requires;
- carry on when a disk driver crashes and is restarted (note 0012);
- know which volumes exist, and give their root folders to whoever may have them.

## The design

### 1. The storage service

`services/storage` is an ordinary program (GPL-3.0-or-later), started by the root program,
tied to it (note 0013). It is the only program that talks to disk drivers.

```
 devmgr ──pbr.devices──▶ storage ──pbr.block (badge per volume)──▶ filesystem servers
 drivers ◀──pbr.block──┘    │
                            └──pbr.storage──▶ the users service (note 0017)
```

At start it:
1. asks the device manager for every device of class `Block` (`pbr.devices`; it gets the
   device manager's endpoint from its supervisor, note 0018);
2. opens each disk's driver, and reads its partition table (section 2);
3. identifies each volume's filesystem by its contents (section 2);
4. starts a filesystem server for each volume it can serve (RockFS and FAT32 in M5; note 0016),
   tied to itself, giving it a capability for that volume alone (section 4);
5. serves `pbr.storage` (section 3).

Disks are found once, at start. Hot-plug (USB sticks, M7) is an open question.

### 2. Partitions: `lib/partition`

A host-built library (`pbr-partition`, no_std) that reads, and treats as hostile:
- **GPT**: the protective MBR, the header at sector 1 with its CRC, the entry array with its
  CRC; if the primary header or array is damaged, the backup at the disk's last sector;
- **MBR**: the four primary entries (extended partitions are reported as unsupported in M5);
- **a whole disk with no table**: a RockFS superblock at sector 0 (ROCKFS.md, "Where a volume
  lives"), or a FAT boot sector, as on many USB sticks.

Partitions must lie inside the disk and not overlap; a table that breaks that is reported as
damaged and none of its partitions are used. Sizes, counts and offsets are never trusted:
nothing is indexed, multiplied or added without a check. It is tested on the host with
synthetic tables, real ones from our build tool and from Linux's tools, and random corruption
of each (note 0002).

**Identifying a filesystem** is by contents, not only by the partition's type, which can be
wrong:
- **RockFS**: partition type `0x7F` or the RockFS GPT type, or a whole disk; then the
  superblock's magic, version and checksum (or the backup's);
- **FAT32**: MBR types `0x0B`/`0x0C`, the GPT basic data or EFI system partition types, or a
  whole disk; then a boot sector that parses as FAT32 (note 0016);
- **empty**: a disk with no table and only zeros in its first sectors (1.3 offers to set such a
  disk up, and so will the shell);
- anything else is listed as unknown, and left alone.

### 3. Volumes, and `pbr.storage`

Each volume found gets an id (a `u32`, in the order found; stable names across starts are an
open question) and a state. Holding `pbr.storage` means power over every volume, so the root
program gives it only to the users service (note 0017).

```
/// Disks and volumes. Served by the storage service.
interface pbr.storage version 1;

enum Filesystem { RockFs @1, Fat32 @2, Empty @3, Unknown @4 }
enum VolumeState { Ready @1, Starting @2, Unsupported @3, Damaged @4, Failed @5 }

struct VolumeInfo {
    id: u32,
    /// The disk: the device manager's id for it (so a volume can be tied to its driver), or
    /// 1000 and up for a disk in memory; and the partition (0: the whole disk).
    disk: u32,
    partition: u32,
    filesystem: Filesystem,
    state: VolumeState,
    label: string<64>,
    /// In bytes.
    size: u64,
    read_only: bool,
    in_memory: bool,
}

@1 fn list() -> (volumes: list<VolumeInfo, 32>) error StorageError;
/// A copy of the volume's root folder capability, with every right.
@2 fn open(id: u32) -> (root: handle<pbr.file>) error StorageError;
/// Makes a RockFS volume in memory (for when there's no system disk; note 0017).
@3 fn create_ram_volume(size: u64, label: string<64>) -> (id: u32) error StorageError;
/// Puts a RockFS volume on an empty disk: an MBR with one partition from 1 MiB, as 1.3 does.
@4 fn format(id: u32, label: string<64>) -> () error StorageError;
/// Makes every volume clean (its servers save everything and mark it shut down cleanly),
/// and flushes every disk. Afterwards `open` fails.
@5 fn shutdown() -> () error StorageError;

flexible enum StorageError { NoSuchVolume @1, NotReady @2, NotEmpty @3, NoMemory @4,
                             DiskError @5, ShutDown @6, Unsupported @7, BadArgument @8 }
```

`format` only accepts a volume whose filesystem is `Empty`, so nothing with data on it is ever
formatted by mistake. (Re-formatting a volume with data, as 1.3's `format yes` does, is a later,
deliberate addition.)

**RAM volumes** are memory charged to the storage service's budget, presented as a disk of their
own and formatted as RockFS. They bypass the cache, since they already are memory, and vanish
at shutdown.

### 4. What a filesystem server gets

The storage service serves **`pbr.block` itself** (note 0011's interface, unchanged) to each
filesystem server, on its own endpoint, with a badge per volume:
- sector numbers are **relative to the volume**; a request outside it fails with `OutOfRange`;
- `info` gives the volume's size, the sector size, and whether it is read-only;
- reads, writes and `flush` go through the cache (section 5).

So a filesystem server can reach its own volume and nothing else, and filesystem code is the
same whether it talks to the storage service, or (in host tests) to a disk image in a file.

A filesystem server starts with that capability, the kernel log, its volume's id and whether to
mount read-only (as arguments), and an endpoint to the storage service (role 5) on which it
calls `pbr.fs.host`:

```
/// What a filesystem server says to the storage service.
interface pbr.fs.host version 1;
/// The volume is mounted: here is its root folder, and the server's control capability.
@1 fn ready(root: handle<pbr.file>, control: handle<pbr.fs.control>, label: string<64>)
    -> () error HostError;
/// The volume can't be used: its structures are damaged, or it has a feature we don't know.
@2 fn failed(reason: FsFailure) -> () error HostError;
```

The storage service controls the server through a second interface on the server's own
endpoint, under a badge only the storage service holds, sent with `ready` (operation numbers
from 100, as `pbr.driver.debug` does, note 0012):

```
/// What the storage service asks of a filesystem server.
interface pbr.fs.control version 1;
/// Save everything, mark the volume clean, and stop: every capability for it then fails.
@100 fn unmount() -> () error ControlError;
/// How many records the object table holds (for tests: records must go when capabilities do).
@101 fn records() -> (count: u32) error ControlError;
```

### 5. The shared block cache

The cache's logic (lookup, replacement, dirty pages, the ordering rules) is in a host-built
library, `lib/blockcache` (`pbr-blockcache`, no_std), tested on the host against a model and a
simulated disk. The storage service supplies the memory and the disk calls.

**Pages.** The cache works in **4 KiB pages**, each 8 sectors of 512 bytes, aligned to 4 KiB
from the start of the disk. A partition that doesn't start on a 4 KiB boundary (old MBR disks
often start at sector 63) shares a page with its neighbour, so flushing a volume writes every
dirty page that touches it, whichever volume changed it. A request that covers part of a page
reads the whole page; a write that covers whole pages reads nothing. Disks with sectors of other
sizes are refused in M5 (an open question). One pool of pages serves every disk; its size is
fixed when the service starts (16 MiB in M5, while the QEMU tests run with 512 MiB), allocated
once from the storage service's heap.

**Finding and replacing pages.** Pages are found by (disk, page number) in a sorted map (a
B-tree), and replaced in least-recently-used order. A read that misses reads every missing page of the
request in one call to the driver, up to 64 KiB.

**Writes are cached** ("write-back"): a write changes the cached pages and marks them dirty, and
returns. Dirty pages reach the disk when the volume is flushed, when they are replaced to make
room, or after they have been dirty for 30 seconds.

**The ordering rules.** For each volume, the cache behaves exactly like a disk with a write
cache of its own, which is what filesystems are already written for:
1. a read returns the data most recently written;
2. writes may reach the disk **in any order**, at any time before the next flush;
3. **`flush` returns only when every write to that volume made before the flush was called is
   on the disk** (the cache writes the volume's dirty pages, then asks the driver to flush).

A filesystem that needs one write on the disk before another puts a flush between them. That is
how RockFS keeps its promise that a file is never half old and half new (note 0016). This is
what DESIGN.md means by write ordering staying the filesystem's responsibility: the cache never
reorders across a flush, and promises nothing within one.

**Fairness.** One volume may have at most a quarter of the cache dirty; a write beyond that first
writes that volume's oldest dirty pages. So a volume being filled can't hold all the memory
hostage, and the others keep their cached pages.

**Errors.** A failed read fails the request (`DeviceError`). A page that fails to be written
stays dirty, and the volume's next `flush` fails with `DeviceError`, so the filesystem always
finds out, even when the failure happened in the background (as Linux does).

### 6. When a disk driver restarts

When a call to a driver fails with `PEER_GONE`, the storage service asks the device manager for
the driver again (`pbr.devices.open`; `NotReady` means try again after 50 ms, for up to 10
seconds), and repeats the call: block reads, writes and flushes are safe to repeat (note 0012,
section 3). Filesystem servers never see the restart. If the device is `Disabled`, the disk's
volumes become `Failed` and their requests fail with `DeviceError`. Since the cache holds dirty
pages in the storage service's memory, nothing written is lost when a driver crashes.

### 7. One thread, for now

The storage service handles one request at a time. That keeps the cache simple and obviously
correct; a slow disk delays requests for other disks. Several threads, or one per disk, are a
later change inside the service.

## Alternatives considered

- **A cache inside each filesystem server**, written once as a library. There would be no single
  pool of memory to share, and each server would need its own connection to the disk driver,
  which serves the whole disk: a FAT32 server parsing a hostile stick could then write to any
  partition on it.
- **Filesystems as libraries inside the storage service.** One process would parse every
  filesystem, including hostile ones; a bug in any would expose every disk.
- **A cache of file contents instead of disk blocks** (a "page cache", which Linux has alongside
  its block cache). It is what memory-mapped files and reading without copying want, but it
  belongs with the filesystem servers. It comes when memory-mapped files do (M9 or M12).
- **Ring buffers between the storage service and the drivers** now (note 0011 left this for
  M5 to decide). Calls of up to 64 KiB are simple and enough for QEMU's disks and Roxy's SATA
  disk; rings need memory the driver can read without a copy, which needs the scatter-gather
  work in note 0010. Measure first.
- **The device manager reading partition tables.** It knows hardware, not what's on disks.

## Open questions

- Hot-plug: volumes appearing and going (USB, M7).
- Stable names for volumes across starts (the RockFS UUID, the GPT partition's GUID, the FAT
  serial number), with note 0011's question about stable device names.
- Extended MBR partitions; disks with 4096-byte sectors.
- Reading without copying: rings and scatter-gather to the drivers, cache pages mapped
  read-only into filesystem servers.
- Several threads; a cache size that adapts to free memory.
- Telling SSDs which blocks are free (TRIM).

## Changes
- 2 November 2026: first draft, for discussion with Chris; accepted by Chris the same day.
- 2 November 2026, built (M5 stage 2):
  - volumes' disks are the device manager's ids (1000 and up in memory); `StorageError` gained
    `Unsupported` and `BadArgument`;
  - a self-test-only `pbr.storage.debug` (badge 2) hands out a volume's `pbr.block`, as a
    filesystem server will get it, and reports the cache's dirty pages;
  - volume capabilities carry `SEND` and `TRANSFER` (a capability must be transferable to
    travel in a reply) but not `DUPLICATE`;
  - the cache is 16 MiB, from the heap, found through a sorted map; partitions that share a
    4 KiB page are handled (section 5 had wrongly said they couldn't);
  - partition tables are read directly from the drivers, not through the cache;
  - until the filesystem servers exist (stages 3 to 5), recognised volumes are listed as
    `Unsupported`, `open` and `format` answer `Unsupported`, volumes in memory are left empty,
    and `shutdown` only flushes the cache;
  - until supervision (stage 7), the root program gives the storage service the device
    manager's endpoint at start-up role 1001.
- 2 November 2026, M5 stage 3: `pbr.fs.host`'s `ready` also carries the control capability,
  and `pbr.fs.control` gained `records`.
- 7 November 2026, M5 stage 4b, built with the RockFS server:
  - the storage service starts the filesystem servers itself: the root program gives it the
    start-up archive (role 4), and it starts a `rockfs` server for every RockFS volume it
    finds, and for volumes it formats. Each server is a *tied* child with an 8 MiB budget paid
    from the storage service's (now 128 MiB); its roles are its endpoint, `pbr.fs.host` here
    (badge `0x2000_0000` + the volume's id, *watched*, so the service hears when the server
    ends), the volume's `pbr.block` (1003) and a stop notification (1004);
  - **the storage service never waits for a server.** A server writes through it, so a call
    from the service to a server can deadlock: the boot tests found exactly that, a server
    saving files just closed while the service waited for it to answer `unmount`. So
    `shutdown` signals each server's stop notification (bound to the server's endpoint),
    keeps serving their writes, and answers once every server has exited (with `DiskError`
    if one didn't exit with status 0 within 30 seconds). `pbr.fs.control`'s `unmount` stays
    for other callers;
  - `open` duplicates the root folder capability from `ready`, with the rights the server
    gave it; it answers `NotReady` while the server starts;
  - volumes in memory and `format` are formatted by the server (its arguments say
    `format LABEL`); `format` of an empty whole disk writes 1.3's MBR first and scans the
    disk again, so the new partition is a new volume;
  - the storage self-test's raw writes go to a partition no server uses (the test disks gained
    one), since writing under a mounted filesystem would damage it.
