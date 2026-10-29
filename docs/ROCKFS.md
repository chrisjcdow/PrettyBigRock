# RockFS, version 1

RockFS is PrettyBigRock's own filesystem. This document describes exactly how it is laid out
on a disk, so that anyone (PrettyBigRock itself, a tool on Windows or Linux, or a curious
reader) can read and write it.

Status: **version 1**. Implemented in PrettyBigRock 1.3 (`kernel/rockfs.c`) and, independently, in
`tools/rockfs.py`.

## Goals

- **Simple enough to understand in one sitting**, and to implement without heroics.
- **Hard to lose data with.** Important structures carry checksums, saving a file never
  leaves it half old and half new, and the disk records whether it was shut down cleanly.
- **Real folders**, nested as deep as you like, from the first version.
- **Friendly to Windows and Linux.** Long names, rules that let any file be copied to
  Windows unchanged, standard partition support, and only standard building blocks
  (little-endian integers, UTF-8 names, CRC-32 checksums).
- **Room to grow.** 64-bit sizes throughout, and feature flags that let later versions add
  things without older versions misreading the disk.

Things RockFS 1 deliberately leaves out: permissions and users, links, encryption, and
checksums on file contents. Feature flags leave room for these later.

## Limits

| | Limit |
|---|---|
| Size of a volume | 2^64 blocks, millions of times larger than any disk that exists |
| Size of a file | 2^64 - 1 bytes, about 16 EiB |
| Files and folders per volume | About 4.29 billion |
| Length of a name | 255 characters |
| Depth of folders | No limit |

(An EiB, or exbibyte, is about a million terabytes.)

## Basics

| Term | Meaning |
|---|---|
| Sector | 512 bytes, the unit the disk hardware reads and writes |
| Block | 4096 bytes (8 sectors), the unit RockFS allocates |
| Volume | The area of disk RockFS lives in: a whole disk or one partition |
| Block number | A block's position from the start of the volume, starting at 0 |
| Inode | A 256-byte record describing one file or folder: its type, size, dates and where its contents live. Names are not in the inode; they live in the folder that contains it. |
| Extent | A run of consecutive blocks holding part of a file, e.g. "file blocks 0-99 are disk blocks 5000-5099" |

All integers are **little-endian** (the x86 byte order). Block numbers and sizes are
**64-bit**; inode numbers are 32-bit. All text is **UTF-8**. Times are **seconds since
1 January 1970, UTC**, stored as unsigned 64-bit integers. Checksums are **CRC-32** with the
standard IEEE polynomial (the same one used by ZIP, PNG and Python's `zlib.crc32`).

## Where a volume lives

A RockFS volume can sit in any of three places:

| Layout | How it's recognised |
|---|---|
| **MBR partition** (the usual choice) | Partition type byte `0x7F` |
| **GPT partition** | Partition type GUID `03971446-6A02-47DC-BB5F-6A7572E5247C` |
| **Whole disk**, no partition table | The RockFS superblock is in sector 0 |

Partition type `0x7F` is set aside for hobby and experimental operating systems, which is
exactly what PrettyBigRock is. The GPT type GUID was generated randomly for RockFS.

To find a volume, a reader first checks sector 0 for the RockFS `magic`. If it isn't there,
it reads the MBR or GPT and looks for a partition with a RockFS type, then confirms the
`magic` at the start of that partition.

PrettyBigRock creates new data disks with an MBR holding a single RockFS partition that
starts at 1 MiB (sector 2048). Partitioning the disk means Windows and Linux see it as
"a disk with an unfamiliar partition" rather than "an empty disk", so they won't offer to
format it.

## Layout of a volume

```
block 0             superblock
block 1 .. B        block bitmap         (which blocks are in use)
block B+1 .. end-1  data blocks          (everything else: files, folders, the inode table)
last block          backup superblock
```

There is no fixed inode table. The inode table is stored as an ordinary file whose blocks
come from the data area, and it grows when it runs out of free inodes. The superblock holds
the inode that describes it. So a disk can't run out of room for new files while it still
has free space.

## The superblock (block 0, with a backup in the last block)

The superblock occupies the first 1024 bytes of the block; the rest of the block is zero.

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 8 | `magic` | The ASCII bytes `ROCKFS!` followed by a zero byte |
| 8 | 2 | `version` | 1 |
| 10 | 2 | `block_shift` | 12, meaning blocks are 2^12 = 4096 bytes |
| 12 | 2 | `name_rule` | How names are compared; see **Names** (1 in this version) |
| 14 | 2 | reserved | Zero |
| 16 | 8 | `block_count` | Total blocks in the volume, including the backup superblock |
| 24 | 8 | `bitmap_start` | First block of the block bitmap (always 1) |
| 32 | 8 | `bitmap_blocks` | Number of bitmap blocks |
| 40 | 8 | `data_start` | First data block |
| 48 | 8 | `free_blocks` | Number of free blocks (a hint; the bitmap is authoritative) |
| 56 | 8 | `reserve_blocks` | Free blocks held back for deleting and housekeeping; see **Running out of space** |
| 64 | 4 | `inode_count` | Number of inode slots in the inode table, including inode 0 |
| 68 | 4 | `free_inodes` | Number of free inode slots (a hint) |
| 72 | 4 | `root_inode` | Inode number of the top-level folder (always 1) |
| 76 | 4 | `state` | 0 = shut down cleanly, 1 = in use (or not shut down cleanly) |
| 80 | 4 | `features_compat` | Feature flags, see below |
| 84 | 4 | `features_ro_compat` | Feature flags, see below |
| 88 | 4 | `features_incompat` | Feature flags, see below |
| 92 | 4 | `mount_count` | How many times the volume has been mounted |
| 96 | 8 | `created` | When the volume was formatted |
| 104 | 8 | `mounted` | When the volume was last mounted |
| 112 | 16 | `uuid` | A random ID, unique to this volume, chosen when it's formatted |
| 128 | 64 | `label` | Volume name, UTF-8, padded with zero bytes (default `Data`) |
| 192 | 320 | reserved | Zero |
| 512 | 256 | `inode_table` | The inode describing the inode table file |
| 768 | 252 | reserved | Zero |
| 1020 | 4 | `checksum` | CRC-32 of bytes 0-1019 |

A reader must check `magic`, `version` and `checksum` before trusting anything else.
If the main superblock is damaged, the backup in the last block can be used instead.
The backup is rewritten whenever the volume's structure changes (when the inode table grows
or moves) and at every clean shutdown. When the structure changes, the backup is written
**first** and the main superblock second, so that if a write is interrupted, one of the two
always holds the new structure with a valid checksum.

**Clean shutdown flag.** When PrettyBigRock starts using a volume it sets `state` to 1.
When it shuts down or restarts properly it sets `state` back to 0. Finding `state` = 1 at
startup means the last session ended abruptly, and the volume should be checked.

### Feature flags

Each of the three feature fields is a set of 32 on/off bits. RockFS 1 defines none of them,
so they are all 0 on a version 1 volume. Later versions will define bits as they add features.
Each bit's field says what an older reader that doesn't recognise it must do:

| Field | If a reader finds a bit it doesn't recognise... |
|---|---|
| `features_compat` | ...it can ignore it. The volume is safe to read and write. |
| `features_ro_compat` | ...it may read the volume but must not write to it. |
| `features_incompat` | ...it must not use the volume at all. |

This lets the format grow without an old version of PrettyBigRock, or an old copy of the
`rockfs` tool, damaging a disk it doesn't fully understand.

## The block bitmap

One bit per block, for every block in the volume. Bit `n` lives in byte `n / 8` of the
bitmap, at bit position `n % 8` (bit 0 is the lowest bit). A 1 bit means the block is in use.

The superblock, bitmap and backup superblock are marked as in use when the volume is
formatted. Bits past `block_count` in the last bitmap block are set to 1, so they're never
handed out.

## Inodes

An inode is 256 bytes:

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 2 | `type` | 0 = free, 1 = file, 2 = folder |
| 2 | 2 | `flags` | Bit 0: hidden. Other bits are reserved and must be 0 |
| 4 | 4 | `parent` | Inode number of the folder this one is in (the root's parent is itself) |
| 8 | 8 | `size` | Size in bytes |
| 16 | 8 | `created` | When it was created |
| 24 | 8 | `modified` | When its contents last changed |
| 32 | 4 | `generation` | Increases each time this inode number is reused |
| 36 | 2 | `extent_count` | How many of the 7 entries below are in use |
| 38 | 2 | `extent_depth` | 0 if the entries below are extents; 1 or more if they are index entries |
| 40 | 24 | reserved | Zero |
| 64 | 168 | `entries[7]` | Up to 7 extent or index entries, 24 bytes each (see below) |
| 232 | 20 | reserved | Zero |
| 252 | 4 | `checksum` | CRC-32 of bytes 0-251 |

### The inode table

The inode table is a file, described by the `inode_table` inode inside the superblock.
Inode `n` is found at byte `n * 256` of that file (16 inodes per block). Inode 0 is never
used, so 0 can mean "no inode". Inode 1 is the root folder.

To create a file or folder, a writer looks for an inode with `type` 0. If there are none,
it adds zeroed blocks to the end of the inode table file, increases `inode_count` and
`free_inodes`, and rewrites both superblocks.

## Where a file's contents live: extents

A file's contents are divided into 4 KiB **file blocks**, numbered from 0. Rather than
listing every block, RockFS records **extents**: runs of consecutive file blocks stored in
consecutive disk blocks. A 1 GiB video saved in one go might be a single extent; a document
edited many times might be a handful.

**An extent entry** (24 bytes):

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 8 | `file_block` | The first file block this extent covers |
| 8 | 8 | `disk_block` | Where that block is on the disk |
| 16 | 4 | `length` | How many consecutive blocks (1 or more) |
| 20 | 4 | `flags` | Reserved, must be 0 |

Extents are kept sorted by `file_block` and never overlap. Any file block that no extent
covers reads as zeroes; this lets a large, mostly empty file take up very little space.

**When a file needs more than 7 extents**, the inode's entries become **index entries**
instead, each pointing to an **extent block**:

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 8 | `file_block` | The first file block covered by the child block |
| 8 | 8 | `child_block` | The disk block holding the child extent block |
| 16 | 8 | reserved | Zero |

**An extent block** is one 4096-byte block: a 16-byte header followed by up to 170 entries.

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 4 | `magic` | The ASCII bytes `RXTB` |
| 4 | 2 | `count` | Entries in use (up to 170) |
| 6 | 2 | `depth` | 0 if the entries are extents; 1 or more if they are index entries |
| 8 | 4 | reserved | Zero |
| 12 | 4 | `checksum` | CRC-32 of the whole block, computed with this field set to 0 |
| 16 | 4080 | `entries[170]` | Extent or index entries, sorted by `file_block` |

This forms a tree: the inode's 7 entries at the top, then extent blocks, as many levels as
needed. One level covers 7 x 170 = 1,190 extents, two levels over 200,000, and more levels
cover anything. Most files never need more than the 7 extents in the inode.

**To find file block `k`:** start at the inode's entries. At each level, pick the last
entry whose `file_block` is not greater than `k`. If the level holds index entries, follow
`child_block` and repeat. When you reach extents, the block is at
`disk_block + (k - file_block)` if `k` is less than `file_block + length`. Otherwise
the block is a gap and reads as zeroes.

## Folders

A folder's contents are a list of entries, stored in the folder's blocks just as a file's
bytes would be. A folder's `size` is always a whole number of blocks.

Each entry has a 16-byte header followed by the name:

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 4 | `inode` | The inode this name refers to (0 = unused space) |
| 4 | 2 | `entry_length` | Bytes from the start of this entry to the start of the next one |
| 6 | 2 | `name_length` | Length of the name in bytes |
| 8 | 1 | `type` | A copy of the inode's type (1 = file, 2 = folder), for quick listings |
| 9 | 7 | reserved | Zero |
| 16 | `name_length` | `name` | The name, UTF-8, no terminating zero |

Rules for entries:
- `entry_length` is a multiple of 8 and at least `16 + name_length`.
- Entries never cross a block boundary. The last entry in each block stretches to the end
  of the block, and a fresh block starts with a single empty entry covering all of it.
- To add a name, find an entry with enough spare room after its own name (or an empty one),
  and split it. If there is none, add a new block to the folder.
- To remove a name, set its `inode` to 0. A writer may also merge the space into the entry
  before it, in the same block.

There are no `.` or `..` entries: each inode's `parent` field records where it lives.
A folder can only be deleted when it has no entries left.

## Names

**Length.** 1 to 255 characters (Unicode code points), which is at most 1,020 bytes of UTF-8.
This matches the limit used by Windows, Linux and macOS.

**Allowed characters.** Any valid UTF-8 except:
- control characters (codes 0-31 and 127);
- `/` (it separates folders in paths);
- the characters Windows forbids: `\ : * ? " < > |`;
- a name may not be `.` or `..`, and may not end with a space or a dot (Windows removes
  those silently).

Together these mean any RockFS file can be copied to Windows, Linux or a Mac without being
renamed. (Windows also reserves a few names, such as `CON` and `NUL`; tools that copy files
to Windows should rename those.)

Writers should store names in Unicode normalisation form NFC, the form most systems use, so
that an accented letter typed on different systems is stored the same way.

**Capitals.** Names are stored exactly as typed. Whether two names count as the same depends
on the volume's `name_rule`:

| `name_rule` | Meaning |
|---|---|
| 1 | The letters A-Z match a-z; every other character must match exactly. So `Notes.txt` and `NOTES.TXT` are the same file, but `École` and `école` are different. |

Two names that count as the same can't exist in one folder. Later versions may add rules
(for example, one that also matches accented capitals), but a volume always keeps the rule it
was formatted with, so names that are distinct today never become clashes later. A reader
that doesn't recognise a volume's `name_rule` may read the volume but must not write to it.

**Paths** are written with forward slashes from the root, e.g. `/letters/thank-you.txt`.

## Saving safely

RockFS follows one rule: **only three kinds of thing are ever overwritten in place.**
- the superblock (and its backup);
- the block bitmap;
- a single inode. Inodes are 256 bytes and never straddle a sector boundary, so updating
  one is a single sector write.

Everything else (file contents, folder blocks and extent blocks) is **copy-on-write**:
a changed block is written somewhere new, and the old copy is only released once nothing
points at it any more. Writing the inode is the moment a change takes effect. A crash or
power cut at any moment therefore leaves every file either fully old or fully new, never
a mixture.

**Changing a file (or a folder, which is stored the same way)**
1. Work out which file blocks change. Only those need new space; unchanged blocks stay where
   they are and remain part of the file.
2. Check there is enough free space (see **Running out of space**). If not, stop: nothing
   has been changed.
3. Allocate new blocks for the changed parts (preferably as one run, to keep extents few),
   and mark them used in the bitmap.
4. Write the new contents into those blocks.
5. Work out the file's new list of extents. Write any extent blocks that changed to new
   locations too.
6. Write the inode with the new extents, size and `modified` time. The change now takes effect.
7. Mark the replaced blocks, and any replaced extent blocks, free in the bitmap.

Adding to the end of a file is the simple case: there is nothing to replace, only new blocks.

**Creating a file or folder**
1. Find or create a free inode, and write it with its type, `parent` and dates.
2. Add the name to the parent folder (a folder change, as above).

**Deleting a file or folder**
1. Remove its name from the parent folder (a folder change, as above).
2. Mark the inode free (`type` = 0) and increase its `generation`.
3. Mark its blocks, and its extent blocks, free in the bitmap.

If the machine stops partway, the worst outcome is space or an inode marked as used that
belongs to nothing. A check reclaims it.

## Running out of space

Copy-on-write needs room for the new version of whatever is changing, alongside the old
version, until the change is complete. So:

- **Before any change, a writer checks that the changed contents will fit.** If more extent
  blocks or folder blocks turn out to be needed and there isn't room for them, the writer
  releases what it allocated and stops, before writing the inode, so nothing has changed.
- **If there isn't room, the change fails cleanly and nothing is altered.** Apps should say
  something like *"There isn't enough free space to save this safely. Delete something you
  don't need and try again."*
- **A reserve is kept back.** `reserve_blocks` free blocks (1% of the volume when formatted,
  at least 16 blocks) can't be used for ordinary saving. Only deleting files and folders,
  and checks, may use them. This guarantees that a full disk can always be cleaned up,
  because deleting itself needs a few blocks to update folders.

Saving a whole new version of a file (as Notepad does) needs room for the complete new file.
Changing part of a large file needs room only for the part that changed.

## Checking a volume

A check (in PrettyBigRock, or later with the `rockfs check` tool on Windows and Linux):
- verifies the superblock, every in-use inode and every extent block against their
  checksums, falling back to the backup superblock if the main one is damaged;
- checks that each file's extents are sorted, don't overlap, and stay inside the volume,
  and that no block belongs to two things at once;
- walks every folder from the root, making sure each entry points at a valid inode of the
  right type, that each inode's `parent` matches, and that no folder has two names that
  count as the same;
- rebuilds the bitmap from the blocks actually in use, reclaiming lost space;
- moves any in-use inode that no folder points to into a `/lost+found` folder
  (the traditional name for this);
- corrects `free_blocks`, `free_inodes` and the backup superblock, and sets `state` to 0.

## Formatting a new volume

1. `block_count` = volume size / 4096. `bitmap_blocks` = `ceil(block_count / 32768)`.
   `data_start` = 1 + `bitmap_blocks`. `reserve_blocks` = the larger of 16 and 1% of
   `block_count`.
2. Zero the bitmap, then mark block 0, the bitmap blocks and the last block as in use.
3. Create the inode table with one block (16 inodes), recorded as a single extent in the
   superblock's `inode_table` inode. Inode 0 stays empty.
4. Make inode 1 an empty folder with `parent` = 1.
5. Choose a random `uuid`, set the `label` (default `Data`), `name_rule` = 1, all feature
   flags 0 and `state` = 0, then write the superblock and its backup.

## Changes to this document

- **Version 1**: implemented. An app's "not enough space" message was reworded, and two rules
  were added for checks: empty items that belong to no folder (left by an unfinished save)
  are removed rather than moved to `/lost+found`, and a writer only needs to check that the
  data itself fits before starting, because building the extent tree fails cleanly if the
  tree doesn't.

- **Draft 3**: 64-bit block numbers and sizes throughout. Extents replace lists of blocks.
  Copy-on-write now covers only the parts of a file that change, and applies to folders
  too. Added a clean "not enough space" failure and a reserve so full disks can always be
  cleaned up. Inodes grew to 256 bytes and the superblock to 1024 bytes.
- **Draft 2**: after Chris asked what we'd regret in the long run. Added feature flags, a
  volume ID, partition support (MBR and GPT), names up to 255 characters with Windows-safe
  rules, a recorded `name_rule`, a growable inode table, and safer saving that never
  overwrites a file in place.
- **Draft 1**: first design, written by Claude for review by Chris.
