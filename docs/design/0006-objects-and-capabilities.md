# 0006: Kernel objects, capabilities and budgets

Status: Accepted · 31 October 2026 · Related: DESIGN.md sections 6.3, 6.6 and 7; notes 0005,
0007, 0008, 0009; milestone M3

## The problem

DESIGN.md says that nothing is reachable without a capability, that capabilities can be
copied with fewer rights but never more, and that revoking one revokes everything derived
from it. Before any program runs, the kernel needs exact rules for:
- what a program is, and where its capabilities live;
- what a handle is, and what rights exist;
- how capabilities are copied, moved, closed and revoked, and what revocation does to things
  in use;
- who pays for the memory every object uses.

These are long-term contracts: every program, library and service is built on them. The
decisions were agreed with Chris on 31 October 2026 (decisions 1, 2, 3 and 6 of M3).

## The design

### 1. Objects

Everything the kernel manages is an object. Each has a **type number**, which is part of the
system call interface and never reused:

| Type | Object | What it is | Built in |
|------|--------|-----------|----------|
| 1 | Process | An address space, a capability table, threads and a budget | M3 |
| 2 | Thread | A thread of execution inside one process | M3 |
| 3 | Memory object | A set of pages that can be mapped into processes | M3 |
| 4 | Endpoint | A service's receiving point for messages (note 0008) | M3 |
| 5 | Notification | 64 signal bits (section 8) | M3 |
| 6 | Reply | A one-time capability to answer one call (note 0008) | M3 |
| 7 | Kernel log | Permission to write to the kernel's console | M3 |
| 8 | Interrupt | One hardware interrupt, delivered as a notification | M4 |
| 9 | Device memory | A range of physical device registers | M4 |
| 10 | I/O ports | A range of x86 I/O ports | M4 |
| 11 | Virtual CPU | For virtual machines (DESIGN.md Q7) | later |

An object lives as long as anything refers to it: a capability, a mapping, a blocked thread,
or the kernel itself. Objects are reference-counted inside the kernel.

### 2. Processes

**A process is an address space, one capability table, its threads and its budget.** Every
thread belongs to exactly one process, and every thread in it sees the same handles. Holding
a capability to a process, with the right rights, is how a launcher builds it (maps memory
into it, places its first capabilities, starts its first thread), watches it, and stops it.

A process ends when:
- it calls `process_exit`;
- its last thread exits;
- one of its threads faults (an illegal memory access, an illegal instruction and so on);
- someone stops it with a capability that has the **stop** right.

When it ends, the kernel:
1. stops its threads;
2. closes every capability in its table (section 5; closing re-parents the derived copies,
   so nothing it gave away disappears);
3. removes its mappings and frees its page tables;
4. returns its budget to its creator (section 9);
5. records an **exit status** (a `u64` it passed to `process_exit`, or a fault record:
   what kind of fault, the address, and the instruction address) and signals everyone
   watching it (`process_watch`, note 0007).

### 3. Handles

A program refers to a capability by a **handle**: a `u32` that indexes its own table.

| Bits | Meaning |
|------|---------|
| 0–19 | Slot number (up to about a million capabilities per process) |
| 20–30 | Generation: changes each time the slot is reused |
| 31 | Always 0 |

- **Handle 0 is never valid.** It's used to mean "no handle".
- **The generation catches stale handles.** If a slot is freed and reused, the old handle
  number no longer matches, and using it fails with `BAD_HANDLE` instead of silently
  reaching the new capability.
- **Handles mean nothing outside their process.** A handle number sent as plain data in a
  message is just a number; only capabilities sent in a message's capability slots are
  transferred (note 0008).
- **The table grows on demand**, in pages, and is charged to the process's budget. It
  shrinks only when the process ends.

### 4. Rights

A capability is an object plus a `u32` set of **rights**. Bits 0–7 mean the same for every
type; bits 8–15 depend on the type; bits 16–31 are reserved and always 0.

| Bit | Name | Meaning |
|-----|------|---------|
| 0 | `DUPLICATE` | May be copied (section 5) |
| 1 | `TRANSFER` | May be sent in a message |
| 2 | `REVOKE` | May revoke the copies derived from it |
| 3 | `READ` | Memory: read it, or map it readable |
| 4 | `WRITE` | Memory: write it, or map it writable. Kernel log: write to it |
| 5 | `EXECUTE` | Memory: map it executable |
| 6 | `MAP` | Memory: map it at all |
| 7 | `INSPECT` | Read information about the object (`handle_info` needs no right; this is for details such as a process's memory use and exit status) |

Type-specific rights:

| Type | Bit 8 | Bit 9 | Bit 10 | Bit 11 |
|------|-------|-------|--------|--------|
| Process | `MANAGE`: map memory into it, place capabilities in it, create threads in it | `STOP` | `WATCH` | |
| Thread | `MANAGE`: start it | `STOP` | `WATCH` | |
| Endpoint | `SEND` | `RECEIVE` | `WATCH`: ask for the "last sender gone" signal | |
| Notification | `SIGNAL` | `WAIT` | | |
| Interrupt (M4) | `WAIT` | `ACKNOWLEDGE` | | |

A Reply capability has no rights at all: it can only be used once, by the process that
received it, to reply (note 0008).

### 5. Copying, moving, closing and revoking

Every capability is a node in one kernel-wide **derivation tree**. Each node records the
node it was copied from (its parent) and the copies made from it (its children).

- **Copy** (`handle_duplicate`, or sending with "copy" in a message): needs `DUPLICATE`, and
  `TRANSFER` too when sent. The copy is a **new child** of the original. Its rights are the
  original's rights masked by what the caller asks for, so they can only shrink.
- **Badged copy** (`endpoint_mint`): a copy of an endpoint capability carrying a `u64`
  badge, which the endpoint's receiver sees with every message sent through it. Only an
  unbadged capability can be minted from; a badged one can be copied (keeping its badge)
  but never re-badged.
- **Move** (sending with "move" in a message): needs `TRANSFER`. The node itself moves to the
  receiver's table, keeping its parent and children. The sender's handle becomes invalid.
- **Close** (`handle_close`): removes the caller's node. Its children are **re-parented**
  to its parent, so copies the caller gave away keep working, and revoking further up the
  tree still reaches them. Closing never needs a right.
- **Revoke** (`handle_revoke`): needs `REVOKE`. Destroys **every descendant** of the node,
  wherever they are, and keeps the node itself. It's how the file picker takes back the file
  it gave Notepad, while keeping its own access.

**What destroying a capability does** (by revocation, or when a process ends):
- **Mappings.** Mapping memory creates a hidden child node, held by the address space rather
  than any table. Revoking an ancestor destroys it and **unmaps the memory**. Closing the
  handle that was used to map doesn't (as with closing a file after `mmap` on Unix), because
  the hidden node is re-parented, not destroyed.
- **Blocked threads.** A thread blocked in a call or a wait through a capability that's
  destroyed wakes up with `REVOKED`.
- **Endpoints.** When the last capability with `SEND` to an endpoint is destroyed or closed,
  the endpoint signals its "last sender gone" watcher (note 0008). When the last capability
  with `RECEIVE` goes, every caller waiting on it fails with `PEER_GONE`.
- **Capabilities in messages.** Messages are never queued in the kernel (note 0008), so a
  capability is always in some table or a blocked sender's pending message, and the sender
  still owns it until the message is delivered. There's no "in flight" state to chase.

**Locking.** The derivation tree is guarded by one kernel-wide **sleeping lock** (a mutex
built on wait queues), so a long revocation can be preempted and doesn't mask interrupts.
Each process's table has its own spin lock for the fast path (looking a handle up), taken
inside the tree lock when both are needed. Splitting the tree lock is a later, local change.
Lock-order rules for sleeping locks: a sleeping lock may never be taken while any spin lock
is held, and the debug-build checker reports it if one is.

**Where the logic lives.** The tree itself (insert, move, close with re-parenting, revoke)
is ordinary logic, so it lives in its own library, `lib/captree`, built both for the kernel
and for the host. On the host it's tested against a simple model, with random sequences of
operations (note 0002, testing).

### 6. Memory objects and mappings

- **A memory object** is a whole number of pages. In M3 its pages are allocated and zeroed
  when it's created, and charged to the creator's budget. Pages allocated only when first
  touched, and copy-on-write copies (for `fork` emulation, M9), are later additions behind
  the same calls.
- **Mapping** (`memory_map`) places part of a memory object into a process at a page-aligned
  address, or at a random address the kernel chooses. It needs `MAP` on the memory object,
  `MANAGE` on the process, and `READ`, `WRITE` and `EXECUTE` for the matching permissions.
- **Never writable and executable at once.** A mapping may be readable, readable and
  writable, or readable and executable. Asking for writable and executable fails with
  `INVALID_ARGUMENT`. `memory_protect` can change a mapping between these, which is how a
  loader writes code and then makes it executable.
- **Program addresses** are the lower half, from `0x0000_0000_0001_0000` up to
  `0x0000_7FFF_FFFF_F000` on both architectures. The first 64 KiB are never mapped, so a
  null pointer always faults.
- **Mappings can't overlap.** Mapping over an existing mapping fails with `ALREADY_EXISTS`.
- **Shared memory** is a memory object mapped into more than one process. It's charged to its
  creator.

### 7. Threads

- A thread is created inside a process (`thread_create`), given an entry point, a stack
  pointer, an argument and a priority, and started separately (`thread_start`), so a
  launcher can set everything up first.
- **Each thread gets a message page** (note 0008), allocated by the kernel and charged to the
  process.
- **Priority.** Each process has a maximum priority, set by its creator and never above the
  creator's own. A thread's priority can't exceed its process's maximum.
- **Floating point and vector registers** are saved and restored for every program thread on
  every switch (eagerly, not lazily, since lazy saving leaks data between programs through
  known processor flaws). x86-64 uses `XSAVE` where available and `FXSAVE` otherwise;
  AArch64 saves `V0`–`V31`, `FPCR` and `FPSR`. The kernel itself still never uses these
  registers (note 0002).

### 8. Notifications

A notification is a `u64` of signal bits.
- **Signal** (`SIGNAL` right): sets bits. Never blocks, and never fails for being "full":
  setting a bit that's already set changes nothing.
- **Wait** (`WAIT` right): blocks until some bit is set (or the deadline passes), then returns
  the set bits and clears them. If several threads wait, each wake-up goes to one of them.
- **Binding.** A notification can be bound to an endpoint, so one receive returns either a
  message or the notification's bits (note 0008).
- The kernel signals notifications for its own events too: a process or thread ending
  (`process_watch`, `thread_watch`), an endpoint's last sender going, and (from M4)
  interrupts.

### 9. Budgets

Every process has a **budget** in bytes, and a count of how much of it is used.
- **Creating a process** takes a budget out of the creator's own: the creator's "used" goes up
  by that amount until the child ends, then it's given back.
- **The root program** starts with all free memory, less the kernel's reserve.
- **What's charged**, to the process that creates it:
  - memory objects: their pages;
  - threads: the kernel stack (128 KiB), the message page, the saved registers and the
    kernel's bookkeeping;
  - endpoints, notifications, replies: their kernel structures;
  - the capability table and the process's page tables, as they grow.

  The exact charge for each kind of object is a constant in the kernel, listed by
  `process_info`, and rounded up so the charge is never less than the real cost.
- **Running out** fails that one request with `OUT_OF_MEMORY`. Nothing else is affected.
- **The kernel's reserve.** The kernel keeps 1/64 of memory (at least 4 MiB) that budgets
  can't touch, so it can always finish tearing down a process, and its own structures
  (wait queues, run queues) never fail for lack of memory.

## Alternatives considered

- **Separate capability spaces** (seL4's CNodes), where threads choose an address space and a
  capability table independently. It's more flexible, but few systems use the flexibility,
  and it complicates the permissions viewer and the POSIX layer. Sandboxing within a program
  can be done with a second process sharing memory.
- **No revocation in the kernel** (Zircon), where programs build it from go-between objects.
  The sandbox manager and the file picker depend on revocation, so it belongs in the kernel.
- **Programs managing the kernel's memory** (seL4's untyped memory). The strongest guarantee,
  but it makes every program and the library far more complicated.
- **Closing a capability destroys its copies.** Simpler, but then a program that closes its
  own handle would silently cut off everyone it gave copies to. Re-parenting keeps closing
  harmless and revocation explicit.
- **64-bit handles.** More room for generations, but every message and table entry would be
  bigger. 11 bits of generation and 20 bits of slot number is plenty.

## Open questions

- Pages allocated on first touch, and copy-on-write (with M9's `fork` emulation).
- Charging a shared memory object to the processes that map it, rather than only to its
  creator.
- Per-core caches for handle lookups, if the table lock becomes a bottleneck.

## Changes
- 31 October 2026: first version, accepted.
