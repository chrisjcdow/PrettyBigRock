# 0005: The kernel core

Status: Accepted · 29 October 2026 · Related: DESIGN.md sections 6.1, 6.6, 6.7 and 10;
notes 0003 and 0004; milestone M2

## The problem

After the hand-over (note 0003), the kernel is running on one core, on the loader's page
tables and stack, with interrupts off. Before there can be programs, capabilities or
messages (M3), the kernel needs:
- its own memory management;
- interrupts and exceptions;
- a sense of time;
- all of its cores running;
- a scheduler that shares those cores among threads.

All of this has to work identically on x86-64 and ARM64. This note describes how each part
works and which processor-specific pieces it needs.

## The design

### 1. Start-up order

1. The early console (as in M1).
2. **Physical memory.** Build the page allocator from the boot information's memory map,
   using Usable memory only.
3. **The kernel's own page tables:**
   - the kernel image, mapped with its own permissions;
   - the direct map;
   - nothing else (no identity map).

   Switch to them.
4. **The kernel's own stack** for the boot core, in the stack area (section 2). Switch to it.
5. **Copy what's still needed from the boot information** (the memory map, the command line,
   the framebuffers), then give all LoaderReclaimable memory to the page allocator.
6. **The kernel heap**, so the kernel can use Rust's `alloc` (`Box`, `Vec`, `Arc`).
7. **The hardware description.** Read ACPI (MADT, HPET, GTDT, FADT) to find the cores, the
   interrupt controllers and the timers. The device tree comes with the Raspberry Pi (M10).
8. **Per-core data, exceptions, the interrupt controller and the timer** on the boot core.
9. **The other cores.** Each runs steps 8 and 9 for itself, then joins the scheduler.
10. **The scheduler.** The boot core becomes an ordinary thread and starts the first work.

### 2. The kernel's address space

Both architectures use the same layout in the upper half (48-bit addresses):

| From | To | What |
|------|----|------|
| `0xFFFF_8000_0000_0000` | `0xFFFF_C000_0000_0000` | The direct map of physical memory (64 TiB) |
| `0xFFFF_C000_0000_0000` | `0xFFFF_C800_0000_0000` | Kernel stacks, each with an unmapped guard page below it |
| `0xFFFF_C800_0000_0000` | `0xFFFF_D000_0000_0000` | Device registers (mapped uncached on request) |
| `0xFFFF_FFFF_8000_0000` | top | The kernel image |

The lower half belongs to programs (from M3). The kernel's lower-half page tables are empty,
and on ARM64 lower-half walks are switched off altogether (`TCR_EL1.EPD0`).

Device registers get their own area rather than living in the direct map. The direct map only
covers memory the firmware listed, it's mapped as normal memory, and a device mapping must
never be cached.

### 3. Physical memory

- **A bitmap**, one bit per 4 KiB page, covering everything up to the highest Usable
  address. It's stored in the first Usable region big enough to hold it.
- **Requests.** Single pages, or contiguous runs (with an alignment and an optional upper
  address limit, for things like the x86 start-up trampoline, which must be below 1 MiB).
  Page 0 is never handed out.
- **Locking.** One lock guards it. Per-core caches of free pages, and awareness of how memory
  is spread across sockets (NUMA), come later and don't change the interface.
- **Accounting.** It counts the total and free pages, for the start-up report and, later, for
  charging memory to programs (DESIGN.md section 6.6).

### 4. The kernel heap

`alloc` works through Rust's `GlobalAlloc` interface. Small requests (up to 2 KiB) come from
size classes of 16 bytes up to 2 KiB, each a free list of slots carved from whole pages.
Larger ones are contiguous runs of pages from the page allocator, reached through the direct
map. A lock that masks interrupts guards each size class, so the heap can be used anywhere,
including inside interrupt handlers.

### 5. Interrupts and exceptions

**x86-64:**
- A GDT and TSS for each core. The TSS has separate stacks (IST) for double faults, NMIs and
  machine checks, so a kernel stack overflow (which hits the guard page) is still reported
  cleanly.
- One shared IDT:
  - vectors 0 to 31 are exceptions;
  - 32 is the timer;
  - 0xF0 is the "reschedule" inter-core interrupt;
  - 0xF1 is "stop" (used when the kernel panics);
  - 0xFF is the APIC's spurious vector.
- The legacy PIC is masked. Each core's **local APIC** is used in x2APIC mode when the
  processor supports it, and through its registers in memory otherwise.

**AArch64:**
- One vector table (`VBAR_EL1`), saving all registers into a frame on the current stack.
- The interrupt controller is the **GIC**, version 2, as used by the Pi 5's GIC-400 and by
  QEMU's `virt` machine. GICv3 is added when a target needs it; the interrupt controller sits
  behind a small interface so the rest of the kernel doesn't care which one it is.
- Inter-core interrupts are software-generated interrupts (SGIs): 0 is "reschedule", 1 is
  "stop".

**On both:**
- **Exceptions in the kernel are bugs.** Page faults, undefined instructions and the like
  stop the system with a report of the registers and the faulting address. Two exceptions
  aren't fatal: the breakpoint instruction (used by the self-test) and, later, faults taken
  on behalf of programs.

### 6. Time

- **The clock.** The kernel's clock counts nanoseconds since start-up (`time::now()`), as
  DESIGN.md section 6.1 says:
  - x86-64: the TSC if it's invariant (it ticks at a constant rate), otherwise the HPET's
    counter. The TSC is calibrated against the HPET, or the PIT when there's no HPET.
  - AArch64: the generic timer's counter (`CNTVCT_EL0`), whose frequency is in `CNTFRQ_EL0`.
- **The timer interrupt.** Each core has one, ticking 1000 times a second for now:
  - x86-64: the local APIC timer, calibrated the same way.
  - AArch64: the virtual timer (`CNTV`), whose interrupt number comes from ACPI's GTDT.

  Turning the tick off when a core has nothing to do ("tickless") is a later change, inside
  the timer module.
- **Wall-clock time** is the boot time from the boot information plus `time::now()`.

### 7. Bringing up the other cores

**Per-core data.** Each core has a `PerCpu` structure:
- its number, and its hardware ID (APIC ID or MPIDR);
- its run queue and current thread;
- its timer state;
- its interrupt-nesting depth.

It's reached through `GS` (x86-64) or `TPIDR_EL1` (AArch64). Core numbers run from 0, with
the boot core as 0.

**x86-64:**
- The kernel copies a small **trampoline** into a page below 1 MiB, along with:
  - the kernel's page-table root (kept below 4 GiB for this reason);
  - a stack;
  - the core's `PerCpu` address;
  - the entry point.
- It sends INIT, then two start-up IPIs (INIT-SIPI-SIPI), with the waits the Intel and AMD
  manuals ask for.
- The trampoline goes from 16-bit to 64-bit mode and jumps into the kernel. The trampoline
  page is identity-mapped only while cores are starting.

**AArch64:**
- The kernel asks the firmware to start each core with PSCI `CPU_ON` (through `HVC` or `SMC`,
  as ACPI's FADT says). It passes the physical address of a small start-up record holding the
  page tables, the stack and the core's `PerCpu`.
- The new core starts with its MMU off, possibly at EL2:
  - it drops to EL1 the same way the loader does, with the kernel's own copy of the EL2 stub;
  - it turns its MMU on using a temporary identity mapping of the kernel image;
  - it jumps to the kernel's real address.

**On both:**
- Cores start one at a time. Each one reports in by setting a flag in its `PerCpu`, and a
  core that doesn't report within a second is left out and noted in the start-up report.

### 8. Threads and the scheduler

**A thread** is:
- a kernel stack;
- a saved stack pointer;
- a state (ready, running, sleeping, blocked or exited);
- a priority;
- an optional core affinity;
- a name for reports.

Threads are reference-counted (`Arc`), so a thread can be held by a run queue, a wait queue
and a joiner all at once.

**The context switch** saves only the callee-saved registers on the old thread's stack, then
loads the new thread's stack pointer and restores its registers. The architecture layer
provides this. Everything else, including what's saved on interrupt entry, stays on each
thread's own stack.

**The scheduler:**
- **Run queues.** Each core has its own run queue, guarded by its own interrupt-masking lock.
- **Priorities.** Higher priorities run first; within a priority, threads take turns
  (round-robin). Time slices are 10 ms, ended by the timer tick.
- **An idle thread** per core waits for interrupts (`hlt`, `wfi`) when there's nothing else
  to run.
- **New threads** go to the least busy core.
- **Stealing work.** A core that runs out of work takes it from the busiest other core. It
  only *tries* each other core's lock, so two cores stealing from each other can't deadlock.
- **Putting a thread back.** A thread that's switched away from goes back into a run queue
  only after the switch has finished saving it. This is done by the next thread to run, as
  its first action. Otherwise another core could start the thread before its registers were
  saved, a classic multi-core bug.
- **Preemption.** It happens when the timer interrupt sees the time slice is over: the
  interrupt handler calls the scheduler before returning. Kernel code can't be preempted
  while it holds a spin lock, because spin locks mask interrupts.
- **The real-time class** (DESIGN.md section 6.1) slots into the priority scheme later.

**Primitives:**
- `spawn` and `join`, and `yield`;
- `sleep`, on the per-core timer list, at the tick's resolution for now;
- wait queues, which sleeping locks and messages (M3) are built on.

### 9. Locks

- **`SpinLock`**: for data never touched by interrupt handlers.
- **`IrqSpinLock`**: masks interrupts on this core while held. Use it for anything an
  interrupt handler also touches: run queues, timers, the heap, the console.
- **Lock levels.** Every interrupt-masking lock is given a level. In debug builds, taking a
  lock whose level isn't higher than one already held on that core is reported as a bug,
  with both locks' names. This is how the lock order (DESIGN.md section 6.7) is enforced
  rather than just written down.

  The order, lowest first:
  0. the list of every thread (kept for diagnostics);
  1. wait queues;
  2. the scheduler's run queues;
  3. timers;
  4. the kernel's page tables and virtual areas;
  5. the heap (which takes pages from the page allocator);
  6. the page allocator;
  7. the console.

### 10. The self-test

When the command line contains `selftest`, the kernel runs a test of all this after start-up
and reports `Self-test passed` or the first failure. The boot tests (`cargo xtask qemu-test`)
run it with 4 cores on every configuration. It checks:
- **The heap:** thousands of allocations of random sizes, filled with patterns, checked and
  freed.
- **The page allocator:** pages handed out and returned balance exactly.
- **Exceptions:** a breakpoint is caught and the kernel carries on.
- **Time:** a 50 ms sleep takes about 50 ms.
- **The scheduler:**
  - 64 threads share counters, protected by atomics and by a lock, yielding and sleeping
    along the way;
  - every update is accounted for;
  - every core ran some of the work;
  - every thread is joined.

## Alternatives considered

- **A buddy allocator** for physical memory. It's faster for large contiguous requests, but
  more complicated. The bitmap is simple to get right, and its interface lets a buddy
  allocator replace it later without changes elsewhere.
- **One global run queue.** It's simpler, but every scheduling decision on every core would
  fight over one lock. Per-core queues with stealing are what serious kernels do, and what
  DESIGN.md D9 asked for.
- **Tickless from the start.** It saves power and wakes cores less often, but it's harder to
  get right, and the tick is hidden inside the timer module, so switching later is local.

## Open questions

- Stack overflow on AArch64. There's no equivalent of the x86 IST, so overflowing into the
  guard page faults again on the same stack. The standard fix is to run threads on `SP_EL0`
  and keep `SP_EL1` for exceptions. To be decided with programs (M3), which change how stacks
  are used anyway.
- GICv3, for newer Arm machines.

## Changes
- 29 October 2026: first version.
- 29 October 2026: built.
  - The heap is below the page allocator in the lock order, since it takes pages from it (the
    lock-order checker caught the first version's mistake).
  - Added the thread list for diagnostics.
  - Added the self-test's watchdog: if the test hasn't finished within 30 seconds, the timer
    interrupt prints every core's scheduler state and every thread.
