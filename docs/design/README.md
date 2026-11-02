# Design notes

Every major part of PrettyBigRock 2 gets a design note here **before** it's built. The
top-level map is [../DESIGN.md](../DESIGN.md), and the order of work is in
[../ROADMAP.md](../ROADMAP.md).

The notes are how the project remembers. Claude starts every session without memory of the
last one, and anyone reading the code on GitHub is in the same position, so the notes are
written to be read cold.

## The notes

| Note | What it covers | Status |
|------|----------------|--------|
| [0001](0001-repository-layout.md) | Repository layout, and adding an architecture | Accepted |
| [0002](0002-coding-rules.md) | Coding rules, the toolchain, and testing | Accepted |
| [0003](0003-boot-information.md) | Boot information, and the machine's state when the kernel starts | Accepted (version 1.0) |
| [0004](0004-loaders-and-boot-disk.md) | The UEFI and BIOS loaders, and the boot disk image | Accepted |
| [0005](0005-kernel-core.md) | The kernel core: memory, interrupts, time, cores, scheduler, locks | Accepted |
| [0006](0006-objects-and-capabilities.md) | Kernel objects, capabilities and budgets | Accepted |
| [0007](0007-system-calls.md) | System calls (interface version 1.0) | Accepted |
| [0008](0008-messages-and-interfaces.md) | Messages, rings and the interface language | Accepted |
| [0009](0009-programs-and-granite.md) | Programs, the start-up archive, the root program and `granite` | Accepted |
| [0010](0010-device-access.md) | Device access: device memory, interrupts, I/O ports and DMA | Accepted |
| [0011](0011-device-manager-and-drivers.md) | The device manager, drivers and their interfaces | Accepted |
| [0012](0012-driver-restart-and-m4-tests.md) | Crashing and restarting drivers, and how M4 is tested | Accepted |
| [0013](0013-kernel-1.2.md) | Kernel interface 1.2: derived capabilities, watched badges, tied processes, the wall clock | Accepted |
| [0014](0014-files-and-file-trees.md) | Files: file capabilities, `pbr.file`, and each program's file tree | Accepted |
| [0015](0015-storage-and-the-block-cache.md) | Storage: disks, partitions, volumes and the shared block cache | Accepted |
| [0016](0016-filesystems.md) | Filesystems: RockFS in Rust, FAT32, and checking them against 1.3 | Accepted |
| [0017](0017-users-sandbox-manager-and-manifests.md) | Users, the sandbox manager and app manifests | Accepted |
| [0018](0018-supervision-shell-and-m5-tests.md) | Supervising services, the console and text shell, and how M5 is tested | Accepted |

## Writing a note

Copy the template below into `NNNN-short-name.md`, using the next free number. Numbers are
never reused, even if a note is withdrawn.

A note's status is one of:
- **Draft**: being written or discussed.
- **Accepted**: agreed, and the code follows it.
- **Replaced by NNNN**: superseded; kept for the record.
- **Withdrawn**: abandoned; kept for the record.

Formats and interfaces (disk layouts, the boot information, system calls, messages) are
**specifications**, and must be exact enough that someone could write a compatible
implementation from the note alone, as `tools/rockfs.py` was written from
[../ROCKFS.md](../ROCKFS.md). When code and a specification disagree, the specification is
right and the code has a bug, unless the note is deliberately changed (with a new version
number where the format demands it).

```markdown
# NNNN: Title

Status: Draft · Date · Related: DESIGN.md section N, notes NNNN

## The problem
What needs deciding or designing, and why now.

## The design
What we'll do, precisely enough to build from.

## Alternatives considered
What else we could have done, and why we didn't.

## Open questions
What's still undecided.

## Changes
- Date: what changed.
```
