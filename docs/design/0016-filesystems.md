# 0016: Filesystems: RockFS in Rust, FAT32, and checking them against 1.3

Status: Accepted · 2 November 2026 · Related: DESIGN.md sections 8 and 12; ROCKFS.md; notes 0002,
0014, 0015; milestone M5

## The problem

PrettyBigRock 2 needs its own filesystem, RockFS, reading and writing the disks 1.3 made (M5's
"done when": *a 1.3 data disk opens with its files intact*), and FAT32 for USB sticks and SD
cards (DESIGN.md D10). Both are ordinary programs (principle 2). This note settles:
- how a filesystem server is put together, so the capability rules are written once;
- what the Rust RockFS must do, and how it keeps its "never half old and half new" promise over
  a cache that reorders writes (note 0015);
- FAT32's scope;
- how both are checked: against `tools/rockfs.py`, against 1.3's own code, against real disks
  made by 1.3, and against other systems' FAT tools.

## The design

### 1. A filesystem server

A filesystem server is a thin program (`services/rockfs`, `services/fat`) made of:
- **`lib/fsserve`** (note 0014, section 7): `pbr.file`, capabilities, rights, paths, records;
- **a filesystem library**: `lib/rockfs` or `lib/fat`, no_std with `alloc`, built for programs
  and for the host, which knows the disk format and nothing about capabilities.

They meet at two traits (a *trait* is Rust's way of describing what a type can do, like an
interface in Java or C#):

```rust
/// A volume's sectors: the storage service's `pbr.block` in a server, a file in host tests.
pub trait BlockDevice {
    fn info(&self) -> DeviceInfo;                       // sector size and count, read-only
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError>;
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError>;
    fn flush(&mut self) -> Result<(), DeviceError>;    // note 0015's ordering rule 3
}

/// A mounted filesystem. `NodeId` names a file or folder and stays the same while mounted;
/// a node that has been removed is reported as `Gone`, never mistaken for a newer one.
pub trait Filesystem {
    fn root(&self) -> NodeId;
    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError>;
    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError>;
    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError>;
    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError>;
    fn commit(&mut self, node: NodeId) -> Result<(), FsError>;  // note 0014, section 6
    fn list(&mut self, folder: NodeId, cursor: u64, out: &mut dyn FnMut(Entry) -> bool)
        -> Result<u64, FsError>;
    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError>;
    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError>;
    fn rename(&mut self, from: (NodeId, &str), to: (NodeId, &str), replace: bool)
        -> Result<(), FsError>;
    fn space(&mut self) -> Result<Space, FsError>;
    fn unmount(&mut self) -> Result<(), FsError>;              // commit everything, mark clean
}
```

(The exact signatures are settled when `lib/fsserve` is written, in stage 3, and recorded
here.) Each server handles one request at a time in M5.

### 2. RockFS in Rust: `lib/rockfs`

**ROCKFS.md version 1 is the specification**, and wins any disagreement with code (Rust, C or
Python). The Rust version does everything 1.3's `kernel/rockfs.c` does, behind the trait:

- **Finding the superblock**: the storage service has already found the volume (note 0015),
  so the library reads block 0 of its volume; it checks the magic, version and checksum, and
  falls back to the backup in the last block.
- **Features**: an unknown `features_incompat` bit refuses the volume (`failed`, note 0015); an
  unknown `features_ro_compat` bit, or an unknown `name_rule`, mounts it read-only.
  `block_shift` must be 12.
- **Mounting**: if `state` is 1 (the last session didn't end cleanly), the volume is checked and
  repaired first, as 1.3 does (ROCKFS.md, "Checking a volume"). Then `state` = 1,
  `mount_count` + 1, `mounted` = now; unmounting writes `state` = 0 (backup first, then main).
- **Reading**: extent trees, with every rule checked: checksums, sorted and non-overlapping
  extents inside the volume, entry counts within limits, depths that go down by exactly one
  per level (so a corrupt tree can't loop). Anything wrong is `Damaged` for that request, never
  a panic.
- **Names**: ROCKFS.md's rules, compared by `name_rule` 1. Names are stored as given, as 1.3
  does; converting to Unicode's NFC form, which ROCKFS.md says writers *should* do, needs
  Unicode tables of our own (an open question).
- **Dates** from the wall clock (note 0013, section 4).
- **Formatting** (for RAM volumes and empty disks) exactly as ROCKFS.md says. The volume's
  UUID should be random; until there is a source of randomness for programs (DESIGN.md Q5),
  it is made from the wall clock, the monotonic clock and the volume's size. That's unique
  enough for telling volumes apart, which is all it's used for.
- **The check**, with and without repair, as ROCKFS.md describes, for unclean mounts and the
  shell's `check` command.

**Saving safely over a cache that reorders writes.** 1.3 wrote straight to an ATA disk in the
order it chose. Note 0015's cache may reach the disk in any order between flushes, so the Rust
version puts a **flush** wherever the order matters. Changing a file (ROCKFS.md, "Saving
safely") becomes:

1. allocate new blocks in the bitmap, in memory;
2. write the new contents, and any changed extent blocks, to the new blocks;
3. **flush**: everything the new version points at is now on the disk;
4. write the inode. This is the moment the change takes effect;
5. **flush**: the change is now on the disk;
6. free the replaced blocks in the bitmap, in memory. They can be reused from now on, because
   nothing on the disk points at them any more.

The bitmap is written when convenient (at the latest at unmount): if the machine stops first,
`state` is still 1, and the check at the next mount rebuilds the bitmap from what is really in
use (ROCKFS.md: "the bitmap is authoritative" only on a clean volume). Superblock updates that
change the volume's structure (the inode table growing) write the backup, flush, then the main
copy, flush, as ROCKFS.md requires.

**Changes not yet committed** (note 0014, section 6) are written to *new* blocks at once (steps
1 and 2), with the file's new extent list kept in memory, so they take no memory beyond that
list. Commit is steps 3 to 6. Blocks written by uncommitted changes that are overwritten again
before the commit are freed at once: nothing on the disk points at them. If the machine stops
before the commit, they belong to nothing and the check at the next mount reclaims them. So a
long save, committed once at the end, is atomic.

### 3. FAT32: `lib/fat`

- **Reads and writes** FAT32 volumes with long names (VFAT), as made by Windows, Linux's
  `mkfs.fat`, cameras, and our own build tool (the EFI system partition, note 0004). FAT12,
  FAT16 and exFAT are later (DESIGN.md section 8).
- **Distrusts everything on the disk**: the boot sector's numbers are checked against each other
  and the volume's size; cluster chains are followed with a limit (the volume's cluster count),
  so a chain that loops is `Damaged`, not a hang; folder entries and long-name checksums are
  checked.
- **Names**: long names are UTF-16 on the disk, converted to and from UTF-8; matching ignores
  case for A-Z only (Windows also folds accented letters, which needs Unicode tables: an open
  question); new files get a generated short name (`LETTER~1.TXT`) as Windows does. The
  characters FAT forbids are refused with `BadName`.
- **Writing in the order that does least harm** after a crash: allocate clusters (in both copies
  of the FAT), write the data, flush, then update the folder entry (first cluster and size),
  flush, then free replaced clusters. A crash can still leave a file partly changed or space
  lost, which Windows' and Linux's checkers fix; FAT has no way to promise more. The volume's
  "not cleanly unmounted" flag (bit 27 of FAT entry 1) is set while mounted and cleared at
  unmount, as Windows and Linux do. The free-cluster count in the FSInfo sector is a hint and
  is recounted if it's impossible.
- **Times**: FAT records local time without a time zone; M5 treats them as UTC (time zones are
  an open question).
- **Not in M5**: formatting FAT volumes, and checking or repairing them.

### 4. Checking RockFS: against Python, against 1.3, and on real 1.3 disks

The instruction for M5 is to cross-check RockFS against `tools/rockfs.py`, and to test with
real disks made by 1.3, including writing and reading back. Four layers:

1. **Host tests of `lib/rockfs`** (`cargo xtask test`): unit tests; random operations checked
   against a simple model; extent trees forced deep by fragmenting a small volume; full disks
   and the reserve; and **simulated power cuts** with a test `BlockDevice` that behaves like a
   disk with a write cache: it loses, at a random moment, every write since the last flush in a
   random subset, then stops. After each cut the volume is mounted (so checked) and every file
   must be exactly as of its last commit.
2. **Against `tools/rockfs.py`**: a small host program, `rockfs-rs` (from `lib/rockfs`, with
   the same commands as `rockfs.py`: `mkfs`, `put`, `get`, `ls`, `mkdir`, `rm`, `mv`, `check`,
   plus `stress SEED ROUNDS MANIFEST` and `verify MANIFEST`), and a script,
   `tools/rockfs_cross.py`, that alternates them on one image: Rust writes and Python checks the
   volume and reads every file back; Python writes and Rust does the same; many rounds, with
   random sizes and folders.
3. **Against 1.3's own code**: `classic/tools/hosttest/rfs_host` is 1.3's `rockfs.c` built as
   an ordinary program. The same script has it make volumes (formatted by 1.3, random trees,
   and images cut off mid-write, as `crosscheck.py` already does), which Rust mounts, checks,
   verifies against 1.3's manifest, writes to, and hands back to 1.3 to verify and check.
4. **Real disks made by 1.3, and PrettyBigRock 2 itself**:
   - `cargo xtask classic-disk` boots the classic system in QEMU with a blank 16 MiB data disk,
     accepts its offer to set the disk up, types Terminal commands (`mkdir`, `write`, `cp`,
     `mv`, nested folders, long names, a name with accents, a file large enough to need
     several extents) through QEMU's control socket, and shuts it down cleanly. The image is
     kept in the repository, compressed (`tests/data/classic-1.3.img.gz`; a few kilobytes), with
     a manifest of every file's path, size and CRC-32 taken with `rockfs.py`. It's made once,
     and can be remade at any time; its making is recorded in the DEVLOG.
   - The boot tests attach a fresh copy of it to PrettyBigRock 2. The self-test finds the
     volume, compares every file with the manifest, writes new files, changes and renames
     existing ones, and shuts the volume down cleanly. Afterwards the test runner checks the
     disk with `rockfs.py check` and with 1.3's `rfs_host check`, and reads every new and
     changed file back with `rockfs.py`.

### 5. Checking FAT32

- Host tests of `lib/fat` with images made by `mkfs.fat` and `mtools` (Linux tools used only in
  tests, like QEMU; not part of PrettyBigRock): `lib/fat` must read what they wrote, and what
  `lib/fat` writes must pass `fsck.fat -n` and read back the same with `mtools`. Hostile images:
  corrupted boot sectors, looping chains, broken long names.
- Our build tool's own EFI system partition is read back as a further, independent FAT32
  writer.
- The boot tests attach a FAT32 test disk; the self-test reads, writes and renames on it, and
  the runner checks it afterwards with `fsck.fat -n` and `mtools`.

### 6. Who writes what

Once this note and note 0014 are accepted and `lib/fsserve` exists (stage 3), the RockFS port
(`lib/rockfs`, `rockfs-rs`, `services/rockfs`) and FAT32 (`lib/fat`, `services/fat`) are
well-defined work that follows a specification and a pattern, so they go to Sonnet subagents
(CLAUDE.md). Opus writes the power-cut tests and the `classic-disk` tool, and reviews both
filesystems line by line against the specification before they go in.

## Alternatives considered

- **Building 1.3's C code into PrettyBigRock 2.** The project is Rust (D8), and the C code is
  written for one process with the disk to itself.
- **One server for all volumes of a kind.** Fewer processes, but a hostile USB stick could then
  reach the system disk through a bug in the shared server.
- **FAT32 read-only first**, as DESIGN.md section 8 suggests for foreign filesystems. Reading is
  the safer half; but moving files to and from USB sticks and SD cards is what FAT is for, and
  writing it carefully is well understood. Recommended: read and write in M5, with the tests
  above. (A decision for Chris.)
- **A journal for RockFS.** Copy-on-write already gives atomic saves, and ROCKFS.md version 1
  is fixed: 1.3's disks must stay readable and writable by both.

## Open questions

- Unicode: normalising names to NFC (RockFS), and case folding beyond A-Z (FAT).
- FAT12, FAT16 and exFAT; formatting and checking FAT volumes.
- Time zones for FAT's local times.
- Recording owners and permissions in RockFS (a feature flag, D6), when several users share
  volumes (note 0017).
- Several threads in a filesystem server.

## Changes
- 2 November 2026: first draft, for discussion with Chris; accepted by Chris the same day.
- 2 November 2026, built (M5 stage 4a):
  - `lib/rockfs`: `RockFs::mount(device, clock, read_only)` (with `MountError`: not RockFS,
    unsupported, damaged, disk), `format(device, label, uuid, now)`, `RockFs::check(repair)`
    and `info()`; `NodeId` is the generation above the inode number. Written by Sonnet,
    reviewed by Opus;
  - the power-cut tests (layer 1) found that a crash while a rename replaced a file could
    leave the replaced name pointing at nothing (the check kept whichever name it met first).
    A rename within a folder is now one folder change; across folders the item's `parent` is
    the deciding write, and the check keeps the name in the folder `parent` names. ROCKFS.md
    now says so (the format is unchanged);
  - `rockfs-rs` finds volumes with `lib/partition`, and `tools/rockfs_cross.py` runs layers 2
    and 3 (`cargo xtask test` runs layer 2, `cargo xtask classic` adds layer 3);
  - `cargo xtask classic-disk` drives 1.3 through QEMU's monitor (1.3 has no serial console,
    so it types, waits, and then checks the result with `rockfs.py`). 1.3's keyboard is US
    only, so the disk has no accented name (the cross-checks cover non-ASCII names), and its
    Terminal takes lines of under 90 characters, so the long name is 70. The large file is a
    drawing saved from Paint into a fragmented volume (28 extents, a one-level tree), after a
    restart, because 1.3 allocates "next fit". The cross-checks read the disk with Rust,
    write to it, and hand it back to Python and C; the boot test with it is stage 4b.
- 7 November 2026, built (M5 stage 4b): `services/rockfs`, the server (arguments
  `volume ID`, then `readonly` or `format LABEL`); `lib/fsserve`'s `run` also takes a stop
  notification. The boot tests attach a fresh, writable copy of the disk 1.3 made; the root
  program compares all 35 folders and files with the manifest, makes six changes (a new folder,
  a 300 KB file, an addition, a rename, a move, a removal) and shuts the storage service down.
  The runner then checks the disk with `tools/check_classic_after.py`: `rockfs.py` and 1.3's own
  code find it clean and holding exactly what was expected.
