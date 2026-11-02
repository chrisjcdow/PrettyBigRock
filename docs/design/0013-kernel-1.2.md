# 0013: Kernel interface 1.2: derived capabilities, watched badges, tied processes and the wall clock

Status: Draft · 2 November 2026 · Related: DESIGN.md sections 6.3 and 7; notes 0006, 0007,
0008, 0012, 0014, 0018; milestone M5

## The problem

M5 builds the first services that hand out capabilities for things *inside* themselves: a
filesystem server hands out a capability for each file and folder (note 0014). Three things
they need can't be done with interface version 1.1, and a fourth is missing for files:

1. **Access reached through a capability must be revoked with it.** Notepad is given a folder
   and opens a file in it. If the folder is taken back, the file must go too. Today a service
   can only make capabilities by minting from its own unbadged endpoint (note 0006, section
   5), so every capability it hands out is a child of the service's own, and revoking the
   folder doesn't reach the file.
2. **A service must know when an object is no longer used.** A filesystem server keeps a
   record for each file capability it handed out (which file, which rights, changes not yet
   saved). It needs to know when the last capability for a record has gone, or the records
   pile up for ever. Today the kernel only says when the last capability to a whole *endpoint*
   goes (note 0008, section 5). A service has one endpoint for all its objects, because one
   thread can wait on only one endpoint, so that signal is too coarse.
3. **A process must not outlive the one responsible for it.** When the device manager dies,
   its drivers keep running with their devices, and a new device manager would start
   duplicates (note 0012, section 5, left open for M5). Supervising services in general needs
   the same.
4. **Programs can't tell the date.** Files record when they were created and changed (RockFS
   stores seconds since 1970 in UTC). The loader puts the wall-clock time in the boot
   information (note 0003), but programs can't see it.

## The design

All of this is **interface version 1.2**, a compatible minor version (note 0007, section 7):
programs built for 1.0 or 1.1 run unchanged.

### 1. Watched badges

A **watched badge** is a badge the kernel keeps count of, so the service is told when the last
capability carrying it is gone.

- **New call 58, `endpoint_mint_watched(endpoint h, badge, rights mask) → new h`.** Like
  `endpoint_mint` (call 51): it needs `DUPLICATE` on an unbadged endpoint capability, and makes
  a child of it carrying `badge`, with rights masked by `rights mask`. It also needs `RECEIVE`,
  because only the service that receives on an endpoint may watch its badges.
- **The badge record.** The kernel keeps one record per watched mint: the endpoint, the badge,
  and how many capabilities carry it. Copying a capability (`handle_duplicate`, sending it as a
  copy, `process_give` as a copy) adds one; moving it changes nothing; closing or destroying
  one (by `handle_close`, revocation or the end of its process) takes one away. Copies share
  their original's record: a badge minted once and copied ten times is one record with a count
  of eleven.
- **The gone notice.** When the count reaches zero, the record joins the endpoint's **gone
  queue** and a waiting receiver is woken. `endpoint_receive` and `endpoint_reply_receive`
  return a new kind:

  | Kind | Meaning | Result |
  |------|---------|--------|
  | 1 | A message (note 0008) | The reply handle |
  | 2 | Notification bits (note 0008) | The bits |
  | 3 | **Gone**: every capability carrying a watched badge has gone | The badge |

  When several are ready, notification bits come first, then gone notices, then messages, so a
  service frees records before it takes on more work.
- **Each notice is delivered once.** A badge minted twice with `endpoint_mint_watched` has two
  records and gives two notices. Services should never reuse a badge value while a record for
  it may still exist; the library's object table (note 0014) puts a generation number in every
  badge so values are never reused.
- **A request may still be in progress.** A call carrying the badge may have been delivered
  before the caller's capability went. On a service with several threads, one thread can
  receive the gone notice while another is still serving that call. A service must keep the
  record's state until both are done.
- **Memory.** The record is charged to the budget of the process that minted it, like a
  notification (note 0006, section 9), and the charge is returned when the notice is received.
  If the endpoint's last `RECEIVE` capability goes, queued notices are dropped and their
  charges returned.
- Unwatched badges, and the "last sender gone" signal, work exactly as before.

### 2. Derived capabilities in replies

A service can answer a call with a capability **derived from the one the caller called
through**: a new child of the caller's capability in the derivation tree, to the same
endpoint, with a new watched badge chosen by the service.

- **How a reply asks for one.** In the reply's message page (note 0008, section 3):
  - a `caps[i]` entry of **`0xFFFF_FFFF`** means "derived from the caller's capability";
  - the new capability's badge is the reply's **`badge` field** (offset 40), which senders
    haven't used until now;
  - its rights are the caller's capability's rights masked by `cap_rights[i]`, so they can
    only shrink;
  - `cap_move`'s bit *i* is ignored for this entry;
  - it is watched (section 1), with its own record charged to the replying process.
- **At most one** derived entry per reply. Two fail with `INVALID_ARGUMENT`, as does a derived
  entry in a call (as opposed to a reply). `0xFFFF_FFFF` has bit 31 set, which no handle has,
  so in version 1.1 it failed with `BAD_HANDLE`: no existing program sends it.
- **If the caller's capability has gone** (another thread closed it, or it was revoked) while
  the call was being served, the caller's call has already failed (`REVOKED`, note 0006), so
  the reply fails with `PEER_GONE` like any reply to a caller that has gone. Nothing is
  created.
- Delivery stays all or nothing (note 0008, section 4): if the caller's table is full, the
  reply fails with `TABLE_FULL` and nothing is created.

What this gives: when Notepad opens a file through its folder capability, the file capability
is a child of the folder capability. Revoking the folder (or anything it was copied from)
revokes the file, wherever Notepad sent it. Closing the folder only re-parents the file (note
0006), so a program can close a folder and keep the file it opened, as on any other system.

### 3. Tied processes

- **`process_create`'s second argument** gains a flag. Bits 0 to 7 are the maximum priority, as
  before; **bit 8 is `TIED`**; bits 9 to 63 must be 0 (`INVALID_ARGUMENT`). Version 1.1 refused
  any value above the creator's maximum priority, so no existing program sets bit 8.
- **A tied process ends with its creator.** When a process ends, for any reason, the kernel
  stops every tied process it created, as if by `process_stop` with the status
  `0x5449_4544` (`"TIED"` in ASCII). This cascades: their own tied children are stopped in
  turn.
- **A process's end is complete only when its tied children's are.** Its budget is returned
  to its creator, and its watchers are signalled (note 0006, section 2, steps 4 and 5), only
  after all its tied children have ended. So when a supervisor is told that the device manager
  has ended, its drivers have ended too, and their interrupts and mappings are released.
- Processes created without `TIED` behave as before: they keep running when their creator ends.
  That is for programs meant to outlive whoever started them.

### 4. The wall clock

The system data page (note 0007, section 1) gains one field after the existing ones:

| Offset | Size | Field |
|--------|------|-------|
| 24 | 8 | The wall-clock time when the monotonic clock read 0, in nanoseconds since 1 January 1970 UTC; 0 if the loader didn't know it |

The wall-clock time is this field plus `clock_now`. It is UTC; time zones are handled outside
the kernel (DESIGN.md section 6.1). It comes from the boot information's `boot_time_ns`,
adjusted to the moment the clock started. Setting the clock (from the network, M8) is a later
call; until then the time is as good as the machine's battery clock.

### 5. In `granite` and the interface compiler

- `Endpoint::mint_watched`; `endpoint::Event::Gone(badge)`.
- Generated server code can answer a `handle<…>` result with a derived capability: the reply
  methods take `impl Into<granite::interface::OutCap>`, where an ordinary `Handle` converts as
  before and `OutCap::Derived { badge, rights }` asks for a derived one. Existing services don't
  change. A reply with more than one derived capability is refused by the generated code before
  it reaches the kernel.
- `process::Spawn` gains `tied: bool`.
- `time::wall_now()`, returning nanoseconds since 1970 UTC, or `None` if unknown.

### 6. Numbers

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 58 | `endpoint_mint_watched` | endpoint h, badge, rights mask | new h | `DUPLICATE` and `RECEIVE` |

Also: receive kind 3; the derived entry in replies; `TIED` in `process_create`; the data page's
wall-clock field. No new error codes or rights.

## Alternatives considered

- **An endpoint per open file.** Each would need its own receiving thread, because a thread
  waits on one endpoint. Badges on one endpoint exist precisely to avoid that (note 0008).
- **The client sends its folder capability with each request, and the service re-badges it**
  (a call to make a badged child of a received capability). It works, but every client call
  carries an extra capability, the client could send a different capability from the one it
  calls through, and the service has to check they match. Deriving from the capability the
  call came through needs nothing from the client and can't be confused.
- **The service tracking derivation itself.** It can't: revocation happens in the kernel, and a
  revoked copy doesn't change the badge's other copies, so the service is never told.
- **Stateless file capabilities** (the badge encodes the file's identity, nothing to free).
  There's then nowhere to keep reduced rights or unsaved changes, and derived access still
  escapes revocation.
- **"Unwrapping" capabilities sent back to their own service**, as seL4 does (the receiver
  gets the badge instead of the capability). Useful for moving a file between two folders the
  client holds separately. M5 doesn't need it (note 0014, section 5); it's an open question.
- **Ending a process when the last capability to it goes**, instead of with its creator. A
  process holds a capability to itself (start-up role 1), so it would never end by this rule
  without special cases, and "who is responsible for it" becomes harder to see.
- **A wall-clock system call.** The data page is read without entering the kernel, as
  `clock_now` is, and needs no new call.

## Open questions

- Recognising a capability to one's own endpoint received in a message (an `endpoint_identify`
  call, or seL4-style unwrapping), for moving files between separately granted folders.
- Setting the wall clock (M8), and a clock that drifts.
- Whether a supervisor should be able to tie a process to someone other than its creator.

## Changes
- 2 November 2026: first draft, for discussion with Chris.
