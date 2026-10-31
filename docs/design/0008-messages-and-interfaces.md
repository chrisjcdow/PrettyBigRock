# 0008: Messages, rings and the interface language

Status: Accepted · 31 October 2026 · Related: DESIGN.md section 6.4; notes 0006, 0007, 0009;
milestone M3

## The problem

Messages are how every part of PrettyBigRock talks to every other part, so their design is
one of the longest-lived contracts in the system. This note specifies:
- endpoints, and how a call, a receive and a reply work;
- the message page, which is the exact layout of a message;
- the "last sender gone" signal;
- ring buffers in shared memory, for bulk data;
- the interface language, its encoding, and the code generated from it.

The decisions were agreed with Chris on 31 October 2026 (decisions 4, 5, 6 and 9 of M3).

## The design

### 1. Endpoints

A service receives requests at an **endpoint**: one receiving point for all its clients.
- **Clients hold capabilities with `SEND`**, each usually **badged**: stamped with a `u64`
  that the service chooses and only the service sees (`endpoint_mint`, note 0006). The badge
  tells the service which client is calling. Revoking a badged capability cuts off that
  client alone.
- **The service holds a capability with `RECEIVE`.** Several of its threads may receive on
  the same endpoint; each request goes to one waiting thread.
- **Nothing is queued in the kernel.** A call is a direct hand-over: if a receiver is waiting,
  the message is copied straight to it; if not, the caller blocks on the endpoint until one
  is. Blocked callers are served highest priority first, then in arrival order.
- **Direct switching.** When a call is handed straight to a receiver, the kernel switches
  from the caller to the receiver on the same core without going through the run queue, and
  the same on the way back with the reply. This is the L4-style fast path DESIGN.md asks
  for.

### 2. Call, receive and reply

**`endpoint_call(h, deadline)`.** The caller's message page holds the request. The call
blocks until the reply arrives, then the message page holds the reply, and the call returns
`OK`. It fails with:
- `PEER_GONE` if no capability with `RECEIVE` exists any more, or the service dropped the
  reply without answering (its reply capability was closed, or its process ended);
- `TIMED_OUT` if the deadline passed before the reply (the service's later reply then fails
  with `PEER_GONE`);
- `REVOKED` if the caller's capability was revoked while it waited;
- `TOO_BIG`, `TABLE_FULL` or `BAD_ADDRESS` if the request couldn't be delivered (section 4).
  The service never sees a request that failed to deliver.

**`endpoint_receive(h, deadline)`.** Blocks until a call arrives, or a bound notification
has bits set. It returns a **kind**:
- `1` (message): the receiver's message page holds the request, its `badge` field holds the
  badge of the capability the caller used, and the result is a **reply handle**: a one-time
  Reply capability in the receiver's table.
- `2` (notification): the result is the bound notification's bits, which are cleared.

If both are ready, notification bits are delivered first, so events from drivers aren't
starved by a flood of requests.

**`endpoint_reply(reply h)`.** Sends the receiver's message page back as the reply, and
consumes the reply handle (it becomes invalid, whatever the outcome). If the caller has gone
(it timed out, was stopped, or its process ended), it fails with `PEER_GONE`. If the reply
can't be delivered (too big for the caller's buffer, or the caller's table is full), the
reply fails with that error, and the caller's call fails with `PEER_GONE`.

**`endpoint_reply_receive(reply h, endpoint h, deadline)`.** A reply followed by a receive, in
one call: a service's main loop. A failed reply doesn't stop the receive; the reply's error
is reported in the result's upper half and the receive goes ahead.

A reply capability can be kept and answered later. This is how a service answers "tell me
when something happens": it keeps the reply handle until it does. Reply capabilities can't
be copied or sent to another process in version 1.0.

### 3. The message page

Every thread has one **message page** of 4096 bytes, mapped read-write in its process, whose
address it's given when it starts (note 0007, section 3). A message is written into it before
a call or a reply, and a received message or reply is found in it afterwards. All fields are
little-endian.

| Offset | Size | Field | Meaning |
|--------|------|-------|---------|
| 0 | 4 | `label` | Set by the sender: which operation this is, or a reply's status (section 7) |
| 4 | 2 | `data_len` | Bytes of data at offset 64, 0 to 240 |
| 6 | 1 | `cap_count` | Capabilities in `caps`, 0 to 4 |
| 7 | 1 | `cap_move` | Bit *i* set: capability *i* is moved; clear: copied. Bits 4–7 are 0 |
| 8 | 16 | `caps[4]` | Sender: the handles to send. Receiver: the new handles in its table |
| 24 | 16 | `cap_rights[4]` | Sender: a mask applied to each capability's rights. Receiver: the rights each one now has |
| 40 | 8 | `badge` | Receiver: the badge of the capability the caller used (0 for replies) |
| 48 | 8 | `buf_addr` | Sender: the address of the attached buffer. Receiver: where to put an incoming one |
| 56 | 4 | `buf_len` | Sender: its length, 0 to 65536. Receiver: the length that arrived |
| 60 | 4 | `buf_capacity` | Receiver: the size of the space at `buf_addr`, 0 to 65536 |
| 64 | 240 | `data` | The message's data |
| 304 | 3792 | | Reserved for later versions; neither read nor written by the kernel |

The kernel checks every field before delivering anything. Out-of-range lengths or counts,
or bits 4–7 of `cap_move` set, fail with `INVALID_ARGUMENT`.

### 4. Delivery

Delivering a message (a call or a reply) is **all or nothing**. The kernel first checks that:
- each capability to send exists in the sender's table and has `TRANSFER`, and `DUPLICATE`
  too if copied;
- the receiver's table has room for all of them;
- the attached buffer (if any) is readable in the sender's process and fits in the
  receiver's `buf_capacity`, which must be writable in the receiver's process.

Then it copies the header and data, transfers the capabilities (moved ones keep their place
in the derivation tree; copied ones become children, note 0006), and copies the attached
buffer straight from the sender's memory into the receiver's.

If anything fails, nothing has changed: the sender gets the error, and the receiver keeps
waiting. A receiver offering a smaller `buf_capacity` than a request needs never sees that
request. Services that accept attached buffers always offer the interface's maximum
(section 7 makes that known in advance).

### 5. The "last sender gone" signal

`endpoint_watch(endpoint, notification, bits)` asks the kernel to signal `bits` on the
notification whenever the number of capabilities with `SEND` to that endpoint drops to zero.
- The count includes every table, and capabilities held in a blocked caller's pending
  message.
- It fires each time the count reaches zero, so a service that hands out a new capability
  and later loses it again is told again.

A service that keeps state per client (open files, windows) gives each client its **own
endpoint** for that state (a "session"), watches it, and frees the state when the client
closes it, crashes, or has it revoked. The library's session helpers do this.

### 6. Ring buffers

Bulk data moves through **rings** in shared memory, with a notification as the doorbell. The
kernel knows nothing about rings; this layout is a specification for the native library, so
any two programs can use it.

A ring is a memory object mapped by both sides, with one **producer** and one **consumer**:

| Offset | Size | Field | Written by |
|--------|------|-------|-----------|
| 0 | 4 | Magic `"RING"` | creator |
| 4 | 2 | Version (1) | creator |
| 6 | 2 | Kind: 0 fixed-size entries, 1 byte stream | creator |
| 8 | 4 | `entry_size` in bytes (1 for a byte stream) | creator |
| 12 | 4 | `entry_count`, a power of two | creator |
| 64 | 4 | `head`: entries produced, counting from 0, wrapping at 2³² | producer |
| 68 | 4 | `consumer_waiting`: 1 when the consumer wants a doorbell for new entries | consumer |
| 128 | 4 | `tail`: entries consumed, wrapping at 2³² | consumer |
| 132 | 4 | `producer_waiting`: 1 when the producer wants a doorbell for free space | producer |
| 256 | | The entries, `entry_size × entry_count` bytes | producer |

`head` and `tail` sit on separate 64-byte lines so the two sides don't slow each other down.

- **Producing:** check there's room (`head - tail < entry_count`, wrapping), write the entry
  at `(head mod entry_count) × entry_size`, then store `head + 1` with release ordering. If
  `consumer_waiting` is 1, ring the doorbell.
- **Consuming:** load `head` with acquire ordering; while `tail != head`, **copy** the entry
  out, then store `tail + 1` with release ordering. If `producer_waiting` is 1, ring the
  doorbell.
- **Never trust the other side.** The other side can change anything in the shared memory at
  any moment. So each side reads a field once into a local copy before checking it, copies
  entries out before decoding them, and treats an impossible state (`head - tail` greater than
  `entry_count`, a wrong magic, sizes changing) as a broken ring: it stops using the ring and
  reports an error, and never crashes or overruns. The library's tests include a deliberately
  hostile other side.

Typical use: a disk driver and the file layer share two rings, one for requests and one for
completions, with entries that point at data buffers in a further shared memory object.

### 7. The interface language

Interfaces are written in `.pbi` files in `interfaces/`, one interface per file. An example:

```
/// Reads and writes one file.
interface pbr.file version 1;

/// Reads up to `max` bytes from `offset`.
@1 fn read(offset: u64, max: u32) -> (data: bytes<65536>) error FileError;

/// Writes `data` at `offset`.
@2 fn write(offset: u64, data: bytes<65536>) -> (written: u32) error FileError;

/// Makes a copy of this file capability with fewer rights.
@3 fn reduce(rights: FileRights) -> (file: handle<pbr.file>) error FileError;

/// Operation 4 was removed in an earlier draft and must not be reused.
reserved @4;

flexible enum FileError { NotFound @1, NoRight @2, DiskFull @3 }

bits FileRights: u32 { Read @0, Write @1 }
```

**Grammar** (in EBNF; whitespace and comments separate tokens):

```
file      = doc* "interface" qname "version" INT ";" item* ;
item      = doc* ( function | reserved | struct | enum | bits | const ) ;
function  = "@" INT "fn" IDENT "(" [ params ] ")" [ "->" "(" [ params ] ")" ]
            [ "error" IDENT ] ";" ;
reserved  = "reserved" "@" INT ";" ;
params    = param { "," param } [ "," ] ;
param     = IDENT ":" type ;
struct    = "struct" IDENT "{" { doc* IDENT ":" type "," } "}" ;           (last comma optional)
enum      = [ "flexible" ] "enum" IDENT "{" { doc* IDENT "@" INT "," } "}" ; (last comma optional)
bits      = "bits" IDENT ":" uint "{" { doc* IDENT "@" INT "," } "}" ;     (INT is a bit number)
const     = "const" IDENT ":" uint "=" INT ";" ;
type      = uint | sint | "bool"
          | "string" "<" INT ">" | "bytes" "<" INT ">"
          | "array" "<" type "," INT ">" | "list" "<" type "," INT ">"
          | "option" "<" type ">" | "handle" "<" hkind ">" | IDENT ;
uint      = "u8" | "u16" | "u32" | "u64" ;
sint      = "i8" | "i16" | "i32" | "i64" ;
hkind     = qname | "memory" | "notification" | "process" | "thread" | "any" ;
qname     = IDENT { "." IDENT } ;
doc       = "///" text to end of line ;
```
`//` starts an ordinary comment (so does `////`). `IDENT` is ASCII letters, digits and `_`,
not starting with a digit. `INT` is decimal or `0x` hexadecimal. Words like `fn` and `struct`
are only special where the grammar expects them.

**Rules the compiler enforces:**
- operation numbers are 1 to 65535, unique, and never the number of a `reserved` line;
- every `string`, `bytes` and `list` has an explicit maximum, so the largest possible message
  is known; a message's fixed part must fit in 240 bytes and its variable part in 65536;
- at most 4 handles in a request and 4 in a reply, and handles can't be inside a `list`;
- names are unique; types are defined in the file (importing other files is a later
  addition), and may be used before they're defined;
- enum values and bit numbers are unique within their type, and bits fit their size; enums
  and bits types have at least one value;
- **handles are only parameters or results themselves** (`handle<…>` or
  `option<handle<…>>`), never inside a struct, list, array or other option. This keeps every
  type copyable and comparable, and a message's capabilities easy to count;
- a struct can't contain itself (directly or through an array, option or list), an option
  can't hold another option, a list's elements take up space, and lengths and maximums are
  from 1 to 65536;
- **names follow Rust's style**, so generated code reads naturally: the interface's parts,
  operations, parameters and fields in `snake_case`; types, enum values and bits in
  `CamelCase`; constants in `UPPER_CASE`. Names that are reserved words in Rust or C are
  refused (`type`, `long`, …), so a C generator (M9) never meets one it can't express;
- a few names belong to the generated code: types can't be called `Client`, `Server`,
  `Reply`, `Request` or `Results`; constants can't be `VERSION`, `NAME`, `OP`, `LABEL`,
  `ALL` or the buffer and size names of section 9; operations can't be `new`, `endpoint`,
  `into_endpoint`, `set_deadline`, `deadline` or a primitive type's name; a parameter can't
  be `reply`; a flexible enum can't have a value called `Unknown`.

**Compatibility.** `version` is the interface's major version. Within a version, these are
compatible: adding an operation, adding a value to a `flexible enum`, adding a bit to a
`bits` type, adding a `reserved` line. Anything else (changing or removing an operation,
changing a type) needs a new version. A service can serve several versions at once.

**Labels.** A request's `label` is `(version << 16) | operation`. A reply's `label` is its
status:
- `0`: success; the data holds the results;
- `1`: the operation's error; the data holds the error value as a `u32`;
- `0xFFFF_0001`: unknown version or operation;
- `0xFFFF_0002`: the request was malformed (it failed the checks in section 8);
- `0xFFFF_0003`: the service failed internally.

### 8. The encoding

All values are little-endian.

**The fixed part** goes in `data`. Parameters are laid out in order, each aligned to its own
size (at most 8):
- integers: their size; `bool`: one byte, 0 or 1;
- `enum`: `u32`; `bits`: its declared size;
- `struct`: its fields in order, each aligned to its own alignment; the struct is aligned
  to its largest field's alignment, and padded at the end to a multiple of it;
- `array<T, N>`: N copies of T;
- `option<T>`: aligned to T's alignment: a `u8` tag (0 absent, 1 present), padding to T's
  alignment, then T, which is all zeros when absent;
- `handle<…>`: a `u32` index into the message's `caps` (0 to 3). An absent
  `option<handle>` is all zeros, like any absent option, and takes no capability;
- `string`, `bytes`, `list`: a **reference**, two `u32`s: offset and length in bytes into the
  attached buffer.

The parameters as a whole are laid out exactly like a struct's fields, including the padding
at the end.

**The variable part** goes in the attached buffer. Each `string`, `bytes` and `list` is placed
at an offset aligned to 8, in the order they appear, depth first. A list's elements are laid
out like the fixed part, in one block, and whatever they refer to follows the block; a list
of strings holds references, which point elsewhere in the same buffer. The largest a
variable part can be counts each item rounded up to 8.

**What a receiver checks** before the service sees anything (the generated code does this):
- `data_len` is exactly the fixed part's size, and the capability count is what the
  operation expects (absent options count as absent);
- every reference lies within the buffer received, and no length exceeds its declared
  maximum;
- strings are valid UTF-8; bools are 0 or 1; option tags are 0 or 1; absent options are all
  zero;
- enum values are known (a `flexible enum` decodes an unknown value as `Unknown(n)` instead);
  `bits` have no unknown bits set (unknown bits of a newer peer are rejected, so new bits
  are only used once both sides know them);
- every handle has the expected type, and for `handle<qname>` it's an endpoint capability
  with `SEND`;
- padding bytes are 0.

A receiver doesn't insist that each item sits exactly where an encoder would put it: a
reference anywhere inside the buffer received is accepted. Overlapping references cost the
receiver nothing (everything is copied out), and checking placement would only add ways to
get it wrong.

A message that fails any check is answered with `0xFFFF_0002` and its capabilities are closed.

### 9. Generated code

The **interface compiler** (`lib/interface-compiler`, crate `pbr-interface-compiler`) is a
host program using only Rust's standard library. `lib/interfaces` (crate `pbr-interfaces`)
runs it from its build script over every file in `interfaces/`, generating one Rust module
per interface, for programs (`no_std`, with `alloc`). `cargo xtask interfaces` prints the
generated code for review. Generated code isn't kept in git.

For each interface it generates a module named after it (`pbr.file` becomes `pbr_file`):
- **types** for its structs, enums and bits. Structs derive `Clone`, `Debug`, `PartialEq` and
  `Eq`; enums are Rust enums, and a flexible one has an extra `Unknown(u32)`; bits types are
  a newtype over their integer with a constant per bit, `ALL`, `contains` and `|`;
- **constants**: `NAME`, `VERSION`, the file's own constants, and `REQUEST_BUFFER` and
  `REPLY_BUFFER`, the largest attached buffer any request or reply can have (what a service
  receives into);
- **a module per operation**, with `OP`, `LABEL`, the exact fixed size and largest buffer and
  capability count of each direction (`REQUEST_FIXED`, `REQUEST_BUFFER`, `REQUEST_CAPS`, and
  the same for `REPLY_`), a `Request` struct (and `Results` when there are several results),
  functions to encode and decode each direction, and `Reply`, the responder (below);
- **a client**, `Client`: an endpoint capability and a deadline for its calls, with one
  method per operation. Small values are passed by value, strings, lists and structs by
  reference, and handles are moved. It returns the result (nothing, the single result, or
  `Results`) or a `CallError<E>`: the operation's error `E`, a failure of the call itself, an
  unknown operation, a rejected request, a failed service, or a malformed reply. If the call
  fails, the capabilities it would have moved are closed;
- **a server trait**, `Server`, with one method per operation, taking the decoded
  parameters and the operation's `Reply`. A method answers with `reply.ok(…)`,
  `reply.err(…)` or `reply.fail()`, at once or later; `reply.badge()` says which client
  called. Dropping a `Reply` unanswered tells the caller the service has gone;
- **`dispatch`**, which checks and decodes a request and calls the right method. It answers
  malformed requests and unknown operations itself, and hands back a request for another
  version of the interface so the service can try another dispatcher.

Paths in generated code are written in full (`::granite::…`, `::core::…`), so no name in an
interface can hide them. The encoding and checking themselves are in
`granite::interface` (MPL-2.0), so the generated code stays small.

The compiler is tested on the host: its parser and checks (including a list of files it
must refuse), and the generated code for `interfaces/pbr.test.echo.pbi` (round trips, exact
bytes worked out by hand, and every single-byte change to valid messages). The program
self-test then runs that interface between two programs on PrettyBigRock. The layouts are
cross-checked by a second, independent **layout calculator** in Python
(`tools/pbi_check.py`), which reads the same files and must agree on every struct's size and
every message's fixed size, largest buffer and capability count.

### 10. Licences

Interface files and the code generated from them are under the MIT licence (DESIGN.md D16,
section 9.1), so generated code inside an app carries no obligations. The compiler itself is
GPL-3.0-or-later, with a statement that its output is MIT-licensed.

## Alternatives considered

- **A private channel per client** (Zircon). Each connection is a pair of ends with a queue in
  the kernel. It tells each side when the other goes, but needs a "wait on many" object, and
  its queues are kernel memory a client can fill. The one strength that matters, knowing
  when a client has gone, is covered by the "last sender gone" signal and per-client
  session endpoints.
- **Building both into the kernel.** Twice the hardest code. Adding a queued object later is
  a compatible change if it's ever needed, and the library already builds two-way message
  streams from an endpoint, a ring and a notification.
- **Tiny messages only** (seL4's roughly 120 bytes and no buffer). Simple services would then
  need shared memory for a file name.
- **Large queued messages** (Zircon's 64 KiB). That's the kernel queue memory problem again.
- **Messages in registers.** Faster for the smallest messages, but different on each
  architecture. It can be added later inside the kernel and the library.
- **Rings in the kernel** (Linux's `io_uring`). Useful when the kernel is one side; here both
  sides are programs.
- **An existing interface language** (Cap'n Proto, protobuf, FIDL). Borrowed tools (D13), built
  for other message systems. FIDL is the closest relative, and its ideas (explicit operation
  numbers, flexible enums, checking everything in generated code) are used here.

## Open questions

- **Priority inversion.** A high-priority client calling a low-priority service waits for the
  service to be scheduled. The fix (the client lending its priority and time to the service,
  as seL4's scheduling contexts do) belongs with the real-time scheduling class.
- Interfaces importing types from other interface files.
- A C generator, for the POSIX layer (M9).
- Moving a reply capability to another process (for a service that passes work to a helper).

## Changes
- 31 October 2026: first version, accepted.
- 31 October 2026, while building M3: handles only as parameters or results themselves; an
  absent `option<handle>` is all zeros; an option starts at its value's alignment; the order
  of a list's block and what it refers to; documentation on fields and values; naming rules
  and reserved names; receivers accept references anywhere in the buffer; what is generated;
  the Python cross-check is a layout calculator.
