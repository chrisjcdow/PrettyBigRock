# 0014: Files: file capabilities, `pbr.file`, and each program's file tree

Status: Accepted · 2 November 2026, built 8 November 2026 · Related: DESIGN.md principles 1 and 5,
sections 7.1, 7.2, 9 and 12; notes 0006, 0008, 0013, 0015, 0016, 0017; milestone M5

## The problem

DESIGN.md says there are no global file paths (principle 1), that choosing a file is what
grants access to it (principle 5), and that each app sees its own file tree, assembled from the
folders and files it has been given, so that "what an app can see is exactly what it can reach"
(section 7.2). M5's "done when" starts with exactly that: *a program sees exactly the files it
was given*.

This note settles:
- what a capability for a file or a folder is, and what rights it carries;
- how access reached through a folder stays inside it, and is revoked with it;
- the operations on files and folders (`pbr.file`), and when changes are safely on the disk;
- how a program sees its files as a tree of paths, and who builds that tree.

Where the volumes come from is note 0015; the filesystems themselves are note 0016; who
decides what each app is given is note 0017.

## The design

### 1. The parts

```
 program ── lib/files ──pbr.file───▶ filesystem server ──pbr.block──▶ storage ──pbr.block──▶ disk driver
            (its file tree)            (one per volume)                (partitions,
                                                                        the block cache)
```

- **A filesystem server** runs for each volume (a partition, a USB stick, a RAM disk) and
  serves `pbr.file` (section 5) for everything on it. It is an ordinary program, started by the
  storage service (note 0015).
- **A capability for a file or a folder** is an endpoint capability to that server, with a
  watched badge (note 0013) that names a record in the server's **object table**.
- **Programs talk to filesystem servers directly.** No file-layer process sits in between.
  DESIGN.md's "file layer" is three parts that together join filesystems and build file trees:
  - the **storage service**, which finds volumes and hands out their root folders (note 0015);
  - **`lib/fsserve`**, the half of every filesystem server that deals with capabilities: the
    object table, rights, paths and the `pbr.file` protocol, written once (section 7);
  - **`lib/files`** (`pbr-files`), the half in every program: its file tree, and turning paths
    into requests (section 8).

  A forwarding process would add a hop to every read and write, and a component every file
  passes through, without adding any protection: the capabilities already decide what each
  program can reach.

### 2. The object table, and rights

The server keeps one record per badge: which **node** (file or folder) it refers to, its kind,
and its **rights**. A node is the filesystem's own name for a file or folder that stays the
same while the server runs (for RockFS, the inode number and its generation; note 0016).

**Rights** are `pbr.file`'s `FileRights`, kept by the server (the kernel only knows the
endpoint rights, note 0006):

| Bit | Right | On a file | On a folder |
|-----|-------|-----------|-------------|
| 0 | `Read` | Read its contents | List it; open what's inside it for reading |
| 1 | `Write` | Change its contents and size; `sync` | Create, remove and rename what's inside it; open what's inside it for writing |

Anyone holding a capability may ask for its `info` and the volume's `space`. Rights only ever
shrink: whatever is opened through a folder gets at most the folder's rights, and `reduce`
makes a copy with fewer. Two rights are enough for M5; more can be added as bits later without
a new version (note 0008), for example a folder you can add to but not remove from.

The **endpoint rights** (`TRANSFER`, `DUPLICATE`) are separate, and say whether the holder may
pass the capability on. A launcher can remove them.

### 3. Derivation, revocation, and when records go

- **Everything a server hands out through another capability is derived from it** (note 0013,
  section 2): `open`, `create` and `reduce` answer with a capability that is a kernel child of
  the one the request came through. So:
  - **revoking a capability revokes everything reached through it**, wherever it was sent. The
    sandbox manager takes back a folder from a running app and the files the app opened in it
    go too;
  - **closing a capability doesn't**: a program can close a folder and keep the files it
    opened, because closing re-parents (note 0006).
- **Each volume's root folder** is the one capability the server mints itself
  (`endpoint_mint_watched`). It gives it to the storage service, which hands out copies (note
  0015).
- **Records go when their gone notice arrives** (note 0013, section 1): the last capability
  with that badge has been closed, revoked, or died with its process. If that was the last
  record for a file with unsaved changes, the changes are saved then (section 6).
- **A limit.** A server holds at most 65536 records; past that, `open`, `create` and `reduce`
  fail with `TooManyOpen`. A program that opens files without closing them can fill the table
  and stop others opening files on that volume. Limits per program need the server to know
  which grant a request descends from; that's an open question.

### 4. Paths and names

- **Requests name things by a relative path from the folder called**: names separated by `/`,
  with no empty names, no `.` or `..`, and no leading `/`, at most 4096 bytes. The server only
  walks down. **A folder capability confines its holder to that folder and what's inside it.**
  There is no way up.
- **Names** are UTF-8, 1 to 255 characters, with no `/`, and no control characters (codes 0
  to 31 and 127). Each filesystem may refuse more (RockFS refuses the characters Windows
  forbids; note 0016), with `BadName`. Whether two names count as the same is the volume's
  rule (RockFS: A-Z match a-z, everything else exactly).
- **Capabilities name things, not paths.** A file that is moved stays reachable through the
  capabilities already open for it, even if it moves out of the folder they were opened from.
- **Removing a file or folder** makes every capability for it fail with `Gone` from then on.
  (Unix keeps a removed file's contents readable while it's still open; that's for the POSIX
  layer to decide in M9. It's an open question.)
- There are no links of any kind, so every file and folder has exactly one path on its volume.

### 5. The interface: `pbr.file` version 1

One interface covers files and folders, because one endpoint serves one set of operation
numbers (note 0008). An operation used on the wrong kind fails with `NotAFile` or
`NotAFolder`.

```
/// Files and folders on a volume. Served by filesystem servers.
interface pbr.file version 1;

bits FileRights: u32 { Read @0, Write @1 }
enum NodeKind { File @1, Folder @2 }

struct NodeInfo {
    kind: NodeKind,
    /// What this capability may do.
    rights: FileRights,
    /// Bytes, for a file; 0 for a folder.
    size: u64,
    /// Seconds since 1970 UTC; 0 if the filesystem doesn't record it.
    created: u64,
    modified: u64,
    hidden: bool,
    /// The volume can't be written (the medium, or a filesystem feature we don't know).
    read_only: bool,
}

struct Entry { name: string<1020>, kind: NodeKind, size: u64, modified: u64, hidden: bool }

struct Space { total: u64, free: u64, label: string<64>, filesystem: string<16> }

// Anything.
@1 fn info() -> (info: NodeInfo) error FileError;
/// A copy of this capability with fewer rights (derived: revoked with this one).
@2 fn reduce(rights: FileRights) -> (node: handle<pbr.file>) error FileError;
/// The volume's size and free space.
@3 fn space() -> (space: Space) error FileError;

// Files.
@10 fn read(offset: u64, max: u32) -> (data: bytes<65536>) error FileError;
@11 fn write(offset: u64, data: bytes<65536>) -> (written: u32) error FileError;
@12 fn set_size(size: u64) -> () error FileError;
/// Makes this file's changes durable (section 6; needs `Write`). Returns when they are on the disk.
@13 fn sync() -> () error FileError;

// Folders. Paths are relative to this folder (section 4).
@20 fn open(path: string<4096>, rights: FileRights) -> (node: handle<pbr.file>, info: NodeInfo)
        error FileError;
/// Creates the last name in `path`; the folders before it must exist.
@21 fn create(path: string<4096>, kind: NodeKind, rights: FileRights)
        -> (node: handle<pbr.file>, info: NodeInfo) error FileError;
/// Up to `max` entries from `cursor` (0 to start). `next` is 0 at the end.
@22 fn list(cursor: u64, max: u32) -> (entries: list<Entry, 56>, next: u64) error FileError;
/// Removes a file, or an empty folder.
@23 fn remove(path: string<4096>) -> () error FileError;
/// Renames or moves within this folder's tree. With `replace`, an existing file at `to` is
/// replaced in one step.
@24 fn rename(from: string<4096>, to: string<4096>, replace: bool) -> () error FileError;

flexible enum FileError {
    NotFound @1, AlreadyExists @2, NotAFolder @3, NotAFile @4, NoRight @5, BadName @6,
    BadPath @7, FolderNotEmpty @8, NoSpace @9, ReadOnly @10, TooBig @11, Gone @12,
    Damaged @13, DiskError @14, TooManyOpen @15, BadArgument @16,
}
```

(The exact field order and the largest `list` that fits are settled by the interface compiler
when the file is written, and recorded here.)

- `open` and `create` ask for rights; asking for more than the folder has fails with
  `NoRight` rather than silently giving less, so a program finds out at once.
- `rename` works within one folder capability's tree. Moving something between two folders a
  program holds separately (two grants) is done by copying and removing, which `lib/files`
  does for it; recognising that two capabilities are on the same volume needs the kernel
  addition in note 0013's open questions.
- `Damaged` means the server found a structure on the disk that breaks the filesystem's rules.
  The request fails, nothing is changed, and the volume is marked to be checked.
- `ReadOnly` is the volume; `NoRight` is the capability.

### 6. When changes are on the disk

- **Changes are visible at once** to everyone using the file through the same server.
- **Changes to a file become durable together**, at `sync`; or when the last capability through
  which the file was changed is gone; or when the volume is unmounted. Until then they may be
  lost in a crash or power cut.
- **On RockFS, a crash leaves each file exactly as it was at its last durable point**, never
  half old and half new: its copy-on-write design makes the new version take effect in one
  write (ROCKFS.md, "Saving safely"; note 0016). So "write everything, then `sync`" is an
  atomic save, which is what Notepad wants.
- **Folder changes** (`create`, `remove`, `rename`) are durable when the call returns.
- On FAT32 there is no such guarantee: a crash during a change can leave a file partly changed
  (note 0016).

### 7. The server half: `lib/fsserve`

Every filesystem server is `lib/fsserve` plus one filesystem library (note 0016), which
implements a `Filesystem` trait (lookup, read, write, list, create, remove, rename, commit,
space). `lib/fsserve` does everything in this note that involves capabilities:
- the object table, its badges (an index and a generation, so values are never reused) and
  their records;
- checking rights on every request, and the path and name rules of section 4;
- answering `open`, `create` and `reduce` with derived capabilities;
- gone notices, and saving a file's changes when its last record goes;
- `Gone` for removed nodes; the record limit.

It is written once, so these rules can't differ between filesystems, and its logic is in a
library built for the host and tested there with an in-memory filesystem, including a hostile
client sending every malformed path and wrong-kind request.

### 8. Each program's file tree: `lib/files`

A program sees its files as one tree of paths, like any other system. The tree is built from
**entries**: each a path and a capability for a folder or a file at that path.

- **The tree comes with the program.** Its launcher gives it, at start-up:
  - **role 12, the file tree**: a read-only memory object listing the entries (below);
  - **roles 100 to 355**: the capabilities for entries 0 to 255.
- **The file tree's format** (all little-endian): a 16-byte header (`"PBRT"`, version `u16` 1,
  entry count `u16`, 8 bytes reserved, 0), then each entry: `u32` role, `u16` path length,
  `u8` kind (1 file, 2 folder), one reserved byte (0), the path (UTF-8), padded with zeros to
  a multiple of 8. At most 256 entries and 64 KiB. A memory object is whole pages, so zeros
  may follow the last entry; anything else there makes the tree broken.
- **Rules for the paths**, checked when the program reads its tree (a broken tree is reported at start-up and
  ignored):
  - absolute, starting with `/`; names follow section 4's rules; no `.` or `..`;
  - no two entries the same, and **no entry inside another**: if `/Home/Documents` is an
    entry, `/Home/Documents/letter.txt` can't be. (The sandbox manager never needs that, and
    it keeps "which capability serves this path?" unambiguous.)
- **Folders above the entries** exist only in the program: with entries `/Home/Documents` and
  `/Home/Pictures`, listing `/Home` shows `Documents` and `Pictures`, and `/` shows `Home`.
  They can't be written to.
- **Looking up a path:** `lib/files` makes it absolute (from the program's current folder, which
  is a path it keeps), removes `.`, and resolves `..` by removing the name before it (`..` at
  `/` stays at `/`). It finds the entry the path is in, and sends the rest of the path, relative
  to that entry, to its capability. `..` is never sent to a server, so it can't climb out of a
  grant: at most it moves to the folders above the entries, which only list what the program
  was given.
- **The tree is the program's own.** It can add capabilities it receives later
  (`Files::mount(path, capability)`, for a file chosen in the file picker in M6).
  Changing it gains nothing it doesn't hold: what it can reach is decided by its capabilities,
  and the tree only gives them names.
- **The API:** `Files::from_start(start)` reads the tree; then `open(path, rights)`,
  `create`, `list`, `remove`, `rename` (copying and removing between entries), `info`,
  `set_current`; `File` (`read_at`, `read_all`, `write_at`, `save`, `set_size`, `sync`,
  `reduce`, `list` for a folder; closed when dropped). Folders above the entries can be listed
  and nothing else (`Error::Above`). The path logic (normalising, matching entries, the folders
  above them) is ordinary code, so it lives in a small host-tested library, `lib/filetree`.
  Both are MPL-2.0 like `granite`, since every app links them. `lib/files` is a library of its
  own rather than part of `granite`, because it calls `pbr.file` through `lib/interfaces`,
  which is itself built on `granite`.
- **Launchers** build the tree with `process::Spawn`'s new `file_tree` list of (path,
  capability, whether it's a folder); `granite` encodes it and gives the capabilities.

The same paths mean the same thing for the user and for every app in their session: the
sandbox manager mounts what it grants at the paths the user sees (note 0017), so Notepad given
one letter sees it at `/Home/Documents/letter.txt`, just as the user does, with nothing else
in `/Home/Documents`.

## Alternatives considered

- **A file-layer process that holds every program's tree and forwards requests** (as Plan 9's
  and the Hurd's designs can). One more hop on every read and write, and a component every file
  passes through, for no protection the capabilities don't already give.
- **Global paths with permission checks** (Unix). Ruled out by principle 1, and it's what makes
  sandboxing Unix programs hard.
- **Files as kernel objects.** Filesystems belong in programs (principle 2).
- **Separate `pbr.file` and `pbr.folder` interfaces.** Clearer types, but one endpoint can't
  serve both without the operation numbers colliding, and a capability's kind is only known
  when it's opened.
- **`..` handled by the server**, with each capability remembering a top it can't climb above.
  More state in the server, and the lexical `..` in the library is simpler and can't escape.
- **Keeping a removed file open** (Unix's unlink-while-open). It needs removed files kept on the
  disk until closed, and found again after a crash. Left for the POSIX layer (M9).
- **Every write durable at once.** Simple, but slow, and on RockFS each write would be its own
  copy-on-write step, so a save would no longer be atomic.

## Open questions

- Removing a file that is still open, as Unix allows (M9).
- Limits per program on records, rather than per server (section 3).
- Being told when a folder changes, for the Files app (M6).
- Memory-mapped files, and reading without copying through messages.
- Moving between separately granted folders without copying (note 0013).
- Owners and permissions recorded on the disk (D6); see note 0017.
- Symbolic links: deliberately none for now. They complicate confinement, and nothing needs
  them yet.

## Changes
- 2 November 2026: first draft, for discussion with Chris; accepted by Chris the same day.
- 2 November 2026, built (M5 stage 3):
  - the program half is `lib/files` (`pbr-files`), not `granite::fs`: `granite` can't call
    `pbr.file`, since the generated interface code is built on `granite`;
  - a tree's description may be followed by zeros to the end of its memory object's page;
  - `Spawn::file_tree` entries say whether each is a folder, since the format records it;
  - entries can be added (`Files::mount`) but not yet removed; nothing needs that until M6;
  - the filesystem side is `lib/fs` (the `Filesystem` and `BlockDevice` traits, note 0016)
    and `lib/fsserve` (the object table, `pbr.file`, and `memfs`, a filesystem in memory used
    by its tests and by the `memfs` test program).
- 8 November 2026, end of M5 (stage 9 review): **`sync` needs `Write`.** Without it, a program
  that can only read a file could make another's half-finished save durable, and a crash then
  would leave the file half saved, which section 6 promises "write everything, then `sync`"
  never does on RockFS. Nothing that synced lacked the right. As built, too: the server's
  records are charged to it (each derived capability's badge record, note 0013), so its
  65536-record limit and its budget bound what clients can make it hold; one client can still
  use all of them (open questions).

