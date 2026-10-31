# 0007: System calls

Status: Draft · 31 October 2026 · Related: DESIGN.md section 6.5; notes 0005, 0006, 0008,
0009; milestone M3

## The problem

Programs ask the kernel for things through system calls. DESIGN.md section 6.5 asks for a
small set, taking handles only, versioned, with numbers never reused, and called through our
library so the mechanism underneath can change. This note is the specification: the calling
mechanism, the system page, every call in interface version 1.0, the error codes, and what
the kernel does when a program misbehaves. It also settles the AArch64 stack question left
open by note 0005. The decisions were agreed with Chris on 31 October 2026 (decision 7 of M3).

## The design

### 1. The system page

Programs never execute the processor's system call instruction themselves. The kernel maps
two read-only pages into every process:
- the **system code page** (readable and executable), holding the entry code;
- the **system data page** (readable only), holding what the clock needs.

Both are at a random address in each process. The address of the code page is passed to
every thread when it starts (section 3). The code page starts with this header:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | Magic: `"PBRSYSPG"` |
| 8 | 2 | Interface version, major (1) |
| 10 | 2 | Interface version, minor (0) |
| 12 | 4 | Reserved, 0 |
| 16 | 8 | Offset of `call`, from the start of the page |
| 24 | 8 | Offset of `clock_now` |
| 32 | 32 | Reserved, 0 |

- **`call`** enters the kernel, with the register convention in section 2. The library
  reaches it with an ordinary `call` (x86-64) or `blr` (AArch64) instruction instead of the
  system call instruction. Because every program goes through this code, the way the kernel
  is entered can change without rebuilding any program.
- **`clock_now`** is an ordinary C-convention function returning the monotonic clock in
  nanoseconds (`u64`). Where the processor's counter can be read safely by programs (an
  invariant TSC on x86-64; `CNTVCT_EL0` on AArch64), it reads the counter and converts it
  with the scale in the data page, without entering the kernel. Otherwise it makes the
  `clock_now` system call.

### 2. The calling convention behind `call`

| | x86-64 | AArch64 |
|-|--------|---------|
| Call number | `rax` | `x8` |
| Arguments 1–6 | `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9` | `x0`–`x5` |
| Status (0 or an error code) | `rax` | `x0` |
| Results 1 and 2 | `rdx`, `rsi` | `x1`, `x2` |
| Changed by the call | `rax`, `rcx`, `r11`, `rdx`, `rsi` | `x0`–`x2`, `x8`, `x16`, `x17` |

Every other register, including the floating-point and vector registers, is preserved.
Arguments and results are `u64`. Handles are passed as their `u32` value, zero-extended.

This is a specification of the system page's `call`, not of the processor instruction
underneath, which today is `syscall` on x86-64 and `svc #0` on AArch64.

### 3. How a thread starts

A new thread starts at its entry point, in user mode, with:

| | x86-64 | AArch64 |
|-|--------|---------|
| The argument given to `thread_create` | `rdi` | `x0` |
| The system code page's address | `rsi` | `x1` |
| This thread's message page address | `rdx` | `x2` |
| The stack pointer given to `thread_create` | `rsp` | `sp` (`SP_EL0`) |

All other general registers are 0, the floating-point state is the processor's initial
state, and interrupts are on. The thread-pointer register (`FS` base on x86-64,
`TPIDR_EL0` on AArch64) starts at 0 and belongs to the program; the library uses it.

### 4. Deadlines

Calls that can wait take a **deadline**: a time on the monotonic clock, in nanoseconds.
- `0` means "don't wait": fail with `TIMED_OUT` at once if the call would block.
- `u64::MAX` means "wait for ever".
- Anything else means "give up with `TIMED_OUT` at that time".

Deadlines are absolute, not durations, so retrying a call after an interruption doesn't
stretch the total wait.

### 5. Error codes

| Code | Name | Meaning |
|------|------|---------|
| 0 | `OK` | Success |
| 1 | `INVALID_CALL` | No such call number in this kernel |
| 2 | `BAD_HANDLE` | The handle isn't valid in this process (never existed, closed, stale) |
| 3 | `WRONG_TYPE` | The handle is valid but refers to the wrong kind of object |
| 4 | `NO_RIGHT` | The capability lacks a right the call needs |
| 5 | `INVALID_ARGUMENT` | An argument is out of range or a reserved bit or field isn't 0 |
| 6 | `BAD_ADDRESS` | Memory the call had to read or write isn't mapped with that permission |
| 7 | `OUT_OF_MEMORY` | The process's budget can't cover it (note 0006, section 9) |
| 8 | `TIMED_OUT` | The deadline passed |
| 9 | `PEER_GONE` | The other side of a message has gone (note 0008) |
| 10 | `REVOKED` | A capability in use was destroyed while the call waited |
| 11 | `TOO_BIG` | A message, buffer or request exceeds a limit |
| 12 | `TABLE_FULL` | The receiving capability table has no room |
| 13 | `ALREADY_EXISTS` | Something is already there (an overlapping mapping, an endpoint already bound) |
| 14 | `BAD_STATE` | The object isn't in a state that allows this (a thread already started) |
| 15 | `NOT_SUPPORTED` | The call exists but can't be done here (an unsupported option) |

Codes are never reused or renumbered. New ones may be added in a minor version; programs
must treat an unknown code as a failure.

A call that fails changes nothing: it either completes entirely or has no effect, except
where a call's description says otherwise.

### 6. The calls, interface version 1.0

`h` means a handle; `self` is not special (a program gets a handle to its own process in its
start-up block, note 0009). Results are written `→ result1, result2`.

**Version and time**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 1 | `version` | | major, minor | |
| 2 | `clock_now` | | nanoseconds | |

**Handles**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 10 | `handle_close` | h | | |
| 11 | `handle_duplicate` | h, rights mask | new h | `DUPLICATE` |
| 12 | `handle_revoke` | h | | `REVOKE` |
| 13 | `handle_info` | h | type, rights | |

**Processes**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 20 | `process_create` | budget (bytes), max priority, name address, name length | process h (all rights) | |
| 21 | `process_give` | process h, h, rights mask, mode (0 copy, 1 move) | handle in that process | `MANAGE`; `TRANSFER` (and `DUPLICATE` to copy) on h |
| 22 | `process_stop` | process h, status | | `STOP` |
| 23 | `process_info` | process h, buffer address, buffer length | bytes written | `INSPECT` |
| 24 | `process_watch` | process h, notification h, bits | | `WATCH`; `SIGNAL` on the notification |
| 25 | `process_exit` | status | (doesn't return) | |

The name is UTF-8, at most 64 bytes, for reports. `process_info` writes a versioned record:
state, exit status or fault record, budget, memory used, thread count, and the per-object
charges (note 0006, section 9). Its layout is specified with the native library's types.

**Memory**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 30 | `memory_create` | size (bytes, rounded up to pages), flags (0) | memory h (all rights) | |
| 31 | `memory_map` | process h, memory h, offset, length, address (0 = choose), permissions | address | see note 0006, section 6 |
| 32 | `memory_unmap` | process h, address, length | | `MANAGE` |
| 33 | `memory_protect` | process h, address, length, permissions | | `MANAGE`, and the matching rights of the capability that was mapped |
| 34 | `memory_read` | memory h, offset, buffer address, length | | `READ` |
| 35 | `memory_write` | memory h, offset, buffer address, length | | `WRITE` |
| 36 | `memory_info` | memory h | size | |

Permissions: bit 0 read, bit 1 write, bit 2 execute. Write and execute together is refused.
`memory_read` and `memory_write` let a loader fill a program's memory without mapping it.

**Threads**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 40 | `thread_create` | process h, entry, stack pointer, argument, priority | thread h (all rights) | `MANAGE` on the process |
| 41 | `thread_start` | thread h | | `MANAGE` |
| 42 | `thread_exit` | | (doesn't return) | |
| 43 | `thread_stop` | thread h | | `STOP` |
| 44 | `thread_yield` | | | |
| 45 | `thread_sleep` | deadline | | |
| 46 | `thread_watch` | thread h, notification h, bits | | `WATCH`; `SIGNAL` on the notification |

**Endpoints and messages** (the details are in note 0008)

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 50 | `endpoint_create` | | endpoint h (all rights, no badge) | |
| 51 | `endpoint_mint` | endpoint h, badge, rights mask | new h | `DUPLICATE` |
| 52 | `endpoint_call` | endpoint h, deadline | | `SEND` |
| 53 | `endpoint_receive` | endpoint h, deadline | kind, reply h or bits | `RECEIVE` |
| 54 | `endpoint_reply` | reply h | | |
| 55 | `endpoint_reply_receive` | reply h, endpoint h, deadline | kind, reply h or bits | `RECEIVE` |
| 56 | `endpoint_bind` | endpoint h, notification h | | `RECEIVE`; `WAIT` on the notification |
| 57 | `endpoint_watch` | endpoint h, notification h, bits | | `WATCH`; `SIGNAL` on the notification |

**Notifications**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 60 | `notification_create` | | notification h (all rights) | |
| 61 | `notification_signal` | notification h, bits | | `SIGNAL` |
| 62 | `notification_wait` | notification h, deadline | bits | `WAIT` |

**The kernel log**

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 70 | `log_write` | log h, text address, length | | `WRITE` |

The text must be UTF-8, at most 1024 bytes per call. The kernel adds the process's name.

Numbers are grouped by tens to leave room for related calls. A number, once given out, is
never reused, even if its call is retired.

### 7. Versions

- **The interface version is `major.minor`**, readable with `version` and in the system page.
  M3 ships 1.0.
- **Adding calls, error codes, rights or flags** is a minor version. Programs built for 1.0
  run on any 1.x kernel.
- **Retiring a call** is announced with a version that marks it deprecated, and it's removed
  no sooner than the next major version. Removed numbers fail with `INVALID_CALL`.
- **Every program says which version it needs** in its PrettyBigRock ELF note (note 0009).
  The program loader refuses a program that needs a newer kernel.

### 8. Reading and writing programs' memory

Calls that take an address (a name, a buffer, a message page) read or write it through
two routines, *copy in* and *copy out*:
- they check that the whole range is in the lower half;
- a page fault inside them is not a kernel bug: the fault handler sees which routine was
  running and makes it return `BAD_ADDRESS` (this is the "faults taken on behalf of
  programs" case from note 0005);
- everywhere else, the kernel is stopped by the processor from touching program memory by
  accident: **SMEP and SMAP** on x86-64 and **PAN** on AArch64, switched off only inside the
  copy routines. Program memory is never executable by the kernel (SMEP, and `PXN` on
  every lower-half mapping on AArch64).

### 9. When a program faults

A fault in user mode (a page fault on an unmapped or forbidden address, an illegal
instruction, a misaligned stack, a division error, a breakpoint) **ends the thread's
process** with a fault record (note 0006, section 2), and the kernel log reports it with the
process's name. It never affects the kernel or other processes. Handing faults to a debugger
or a supervisor instead of ending the process is a later addition (an "exception endpoint").

### 10. Kernel stacks on AArch64 (note 0005's open question)

Kernel threads and program threads in the kernel run on `SP_EL1`, each on its own kernel
stack, as they do now; programs' own stacks use `SP_EL0`. To make a kernel stack overflow
report cleanly, as the x86 double-fault stack does:
- kernel stacks are aligned so that the bottom of each stack can be recognised from the
  stack pointer alone;
- the exception entry code for exceptions taken at EL1 checks whether the stack pointer has
  gone below the current stack's bottom, before saving anything;
- if it has, it switches to that core's **emergency stack** and reports a kernel stack
  overflow with the usual register dump.

This is what Linux does on Arm. The cost is two instructions on each exception taken in the
kernel.

## Alternatives considered

- **Programs using the system call instruction directly**, with stable numbers as their only
  contract (Linux). Then the instruction and register convention can never change. With the
  system page they can, as Windows does through `ntdll`.
- **A table of function pointers instead of a code page.** It would also hide the mechanism,
  but every call would pay for an indirect call through data the program could overwrite.
- **Durations instead of deadlines.** Durations are easier to write, but retrying a call
  extends the total wait. The library offers durations on top of deadlines.
- **Negative error codes in the result register** (Linux). Mixing results with errors limits
  what results can be; a separate status register keeps both clean.

## Open questions

- Protection against Meltdown-style attacks (separate kernel page tables for programs, as
  Linux's KPTI does). QEMU doesn't need it, but Roxy's processor is affected. To be decided
  before M7 (Roxy).
- Using processor address-space tags (PCID on x86-64, ASIDs on AArch64) to avoid flushing the
  TLB on every switch between processes. A later speed-up, inside the architecture layer.
- Randomness for programs (a `random` call), which needs the cryptography decision (Q5).
- The exception endpoint for debuggers.

## Changes
- 31 October 2026: first version (draft).
