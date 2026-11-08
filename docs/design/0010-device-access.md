# 0010: Device access: device memory, interrupts, I/O ports and DMA

Status: Accepted · 31 October 2026, built 2 November 2026 · Related: DESIGN.md sections 6.1, 6.3 and 7.1;
notes 0005 to 0009, 0011, 0012; milestone M4

## The problem

A driver is an ordinary program (D4, principle 2). To drive a device it must still do four
things that only the kernel can allow:
- read and write the device's registers, which are at physical addresses (or, on x86, I/O
  ports);
- be told when the device raises an interrupt;
- give the device memory to read and write by itself (DMA);
- do all of that without being able to touch any other device, or the kernel.

Note 0006 reserved object types 8 (interrupt), 9 (device memory) and 10 (I/O ports) for this,
and note 0009 said the root program would get them "from M4". This note settles what they
are, the system calls on them, what the kernel keeps for itself, and what happens to DMA.
Notes 0011 (the device manager and drivers) and 0012 (crashing and restarting drivers) build
on it.

These are long-term contracts: every driver is built on them.

## The design

### 1. What the kernel keeps

The kernel does not know what devices exist. It keeps only what it cannot do without:
- the **interrupt controllers** (the local APIC and IOAPICs on x86-64; the GIC on AArch64),
  because the scheduler's timer depends on them;
- the **timers** and, on x86-64, the HPET, which it uses to calibrate the clock;
- the **early console** (serial port and screen text), which stays the log and panic channel
  for good and never goes through a driver (section 7).

These are listed as **kernel-owned** and can never be turned into capabilities: the kernel
image, every page the page allocator manages, the local APIC page, IOAPIC pages, the GIC's
distributor, CPU interface and MSI frame, the HPET, the early console's UART registers
(`0x3F8`–`0x3FF` when it is COM1), and, on x86-64, the legacy interrupt controller's ports.

### 2. Ranges

All three new object types are **ranges**: a device memory range of physical addresses, a
range of I/O ports, or a range of interrupt numbers. They all work the same way:
- the root program receives one capability of each kind covering everything (section 8), with the
  kernel-owned parts (section 1) refused at the moment of slicing, binding or use;
- **`*_slice`** makes a capability for a smaller range inside the one you hold. It's a copy
  in the sense of note 0006: a new child in the derivation tree, with the rights masked
  down. It needs `DUPLICATE`. Slicing never needs a new kind of right, and a slice can never
  reach outside its parent;
- revoking a capability destroys all slices of it, and with them any mappings, bindings and
  other uses made through them (note 0006, section 5).

The device manager (note 0011) slices the root program's big capabilities down to one device
at a time and gives each driver only its own device's slices.

The logic (checking that a slice lies inside its parent, subtracting the kernel-owned set,
the pool of message-signalled vectors) is ordinary code, so it lives in a small library,
`lib/ranges` (`pbr-ranges`), built for the kernel and the host and tested on the host with
random sequences (note 0002).

### 3. Device memory (type 9)

A **device memory** capability is a range of physical addresses, page-aligned in start and
length. It is not RAM: the kernel refuses to make one over any page it manages as memory or
that is kernel-owned, so a driver can never map the kernel's or another program's memory.

- **Mapping.** `memory_map` (note 0007) accepts a device memory capability in place of a
  memory object. It needs `MAP` on the capability, and `READ` and/or `WRITE` for the
  permissions asked for. **Execute is always refused** (`INVALID_ARGUMENT`).
- **Caching.** The permission word gains a cache mode in bits 8 and 9:

  | Mode | Value | Meaning |
  |------|-------|---------|
  | Device | 0 | Uncached, no reordering or merging: for registers. x86-64: uncached; AArch64: Device-nGnRE. This is the default. |
  | Write-combining | 1 | Writes may be merged and delayed: for framebuffers. x86-64: write-combining (through the PAT); AArch64: Normal non-cacheable. |

  Other values are refused. Memory objects only accept mode 0 (their own, ordinary caching).
- **Rights.** The common rights (note 0006): `DUPLICATE`, `TRANSFER`, `REVOKE`, `INSPECT`;
  `READ`, `WRITE` and `MAP` as for memory. The type-specific bit 8 is **`DMA`**: it allows
  creating DMA memory for this device (section 6).
- **Calls.**
  - `device_memory_slice(h, offset, length, rights mask) → new h`.
  - `device_memory_info(h) → physical address, length`. Needs `INSPECT`. (The device manager
    needs the address to match it against BARs. Drivers are not given `INSPECT`.)
- Pages are mapped with a hidden child node held by the address space, exactly like memory
  objects: revoking the capability unmaps the registers, and a later access faults.

### 4. Interrupts (type 8)

An **interrupt** capability is a range of interrupt numbers:
- **x86-64:** global system interrupts, the numbers the firmware gives IOAPIC pins (the first
  16 are the old ISA interrupts, with the polarity and trigger the ACPI tables' interrupt
  source overrides say);
- **AArch64:** GIC interrupt IDs of shared peripheral interrupts (32 and up).

There is also a special kind, the **message-signalled pool**, from which message-signalled
interrupts (MSI and MSI-X, which every PCI Express device supports) are allocated (below).

**Delivering an interrupt to a driver.** The driver binds an interrupt to one of its
notifications:

- `interrupt_bind(h, index, notification h, bits)`: needs `BIND` (type bit 8) on `h`, and
  `SIGNAL` on the notification. `index` is the position within the range (0 for a
  one-interrupt range). Fails with `ALREADY_EXISTS` if the interrupt is already bound by
  anyone. Unmasks the interrupt.
- When the interrupt fires, the kernel's handler **masks** the interrupt at the controller (a
  level-triggered line would otherwise fire again at once), tells the controller it's been
  taken, and records the interrupt in a bitmap. A kernel thread, `irq`, at the highest
  priority, then **signals `bits`** on the notification. Nothing runs driver code. (Signalling
  takes the notification's wait queue, which isn't safe to take inside an interrupt handler,
  so the handler only sets a bit and wakes the thread: see Changes.)
- The driver handles the device, then calls `interrupt_acknowledge(h, index)` (needs
  `ACKNOWLEDGE`, type bit 9), which unmasks it. Edge-triggered and message-signalled
  interrupts need no masking, because signalling an already-set bit changes nothing; the
  acknowledge is still the driver's "I've seen it", and costs nothing.
- Because a notification can be bound to an endpoint (note 0008, section 2), **one driver
  thread can wait for requests and interrupts in a single `endpoint_receive`**, and
  interrupts are delivered first.
- `interrupt_acknowledge` checks the capability and that the interrupt is bound, not which
  process bound it. Only the device manager ever holds a range that overlaps another
  driver's, so this is harmless in M4 (open questions).
- `interrupt_unbind(h, index)` releases a binding. `interrupt_info(h)` says what kind of
  range it is (ordinary or message-signalled pool), its first number and its count (needs
  `INSPECT`).
- **The binding belongs to the process that made it**, like a mapping: it's released when
  the process ends, when `interrupt_unbind` is called, or when the capability it was made
  through (or any ancestor of that) is revoked. The kernel **masks a released level
  interrupt** so that a driver that crashes mid-interrupt can't leave a storm behind. This
  is what lets a crashed driver be replaced (note 0012).
- An interrupt that fires with nobody bound is masked, and logged once.

**Message-signalled interrupts.** Two calls on the pool capability:
- `interrupt_msi_allocate(pool h) → interrupt h` makes a one-interrupt capability (a child of
  the pool) for a fresh vector.
- `interrupt_msi_info(h) → address, data` says what to program into the device's MSI or
  MSI-X table so that it raises that interrupt.

On x86-64 the kernel picks an unused vector (0x30 to 0xEF) and a target core, spreading them
across cores, and the address and data are the local APIC's message format. On AArch64 they
come from the GIC's **GICv2m MSI frame**: the address is the frame's doorbell register and
the data is an interrupt ID from the frame's range. QEMU's `virt` machine has one with GICv2
(the device tree shows `v2m@8020000`); the kernel finds it in the ACPI MADT. A machine
without an MSI frame has no pool, and `interrupt_msi_allocate` fails with `NOT_SUPPORTED`.
(GICv3's ITS is the equivalent for GICv3, added with GICv3 support, note 0005.)

A released message-signalled vector goes to the **back of the queue** of free vectors, and is
reused only when all the others have been, so a device that is still firing at a vector its
dead driver owned won't disturb the next driver that gets it. (A *restarted* driver keeps its
device's vectors: the device manager holds them, note 0012, section 2. The queue matters when a
vector really is given up.)

The handler takes only the interrupt controllers' lock (to mask a line), which is a new level in
note 0005's lock order, between the timers and the kernel's page tables. The table of bindings
is an ordinary lock, taken by the `irq` thread and by the calls, never by the handler. Nothing in
the handler allocates memory.

**Not in M4:** shared legacy PCI interrupt lines (INTx). On x86-64 they need the routing
tables that only ACPI's bytecode (AML) describes. Every device M4 and M7 need supports MSI.

### 5. I/O ports (type 10, x86-64 only)

An **I/O ports** capability is a range of port numbers (0 to 65535).
- Access is by system call, checked by the kernel against the range:
  `ioport_in(h, offset, width) → value` and `ioport_out(h, offset, width, value)`, with
  `width` 1, 2 or 4 and `offset` naturally aligned to it. Needs `READ` or `WRITE`.
- `ioport_slice(h, first, count, rights mask)` and `ioport_info(h) → first, count` as for the
  others.
- On AArch64 the type does not exist: the root program has none, and creating one fails with
  `NOT_SUPPORTED`.

The only drivers that need ports in M4 are the legacy serial port and PS/2, which are slow
devices, so a system call per access is fine. The processor's per-process permission bitmap
(in the task state segment) would let a driver do port access directly, but it costs work on
every process switch and is harder to get right; it can be added behind these same calls if a
fast device ever needs it.

### 6. DMA

**The problem.** Devices read and write memory by themselves. To tell a device where, a
driver needs a **bus address**: the address the device uses. Today that's the physical
address; a machine with an IOMMU (Intel VT-d, AMD-Vi, Arm SMMU) can translate and restrict
what a device sees.

**What this design gives, and doesn't.** Until the IOMMU support exists (a later milestone),
**a device can read and write all of memory, so whoever controls a DMA-capable device
controls the machine.** The design therefore holds two levels apart:
- **Crash containment** holds. A driver that crashes loses its device, not the system, and
  note 0012 makes sure a restarted driver never finds the device still writing to memory that
  has been given to someone else.
- **Protection from a driver that is itself hostile** does not hold for drivers of
  DMA-capable devices until the IOMMU is used. DESIGN.md section 7.1 says the same in other
  words ("where the machine has one"). The calls below are shaped so that adding an IOMMU
  changes only the kernel.

**The calls.**
- `memory_create_dma(device h, size, flags) → memory h`: makes a memory object of **physically
  contiguous** pages, zeroed, charged to the caller's budget, and up to **4 MiB** (`TOO_BIG`
  beyond that). Needs the `DMA` right on `device h`, a device memory capability. `flags`
  bit 0, `BELOW_4G`, puts the pages below 4 GiB, for devices with only 32-bit addressing.
  The memory gets the memory-type right **`BUS`** (type bit 8), which can be removed from
  copies.
- `memory_bus_address(memory h, device h, offset) → bus address`: needs `BUS` on the memory
  object and `DMA` on the device. `offset` is rounded down to a page. Because it works one
  page at a time and takes the device, scatter-gather lists and per-device IOMMU mappings can
  be added without changing it. Today the device argument is checked and otherwise ignored.
- DMA memory is mapped like any memory object. DMA is assumed to be **cache-coherent** (true
  on x86-64 and in QEMU); a platform where it is not would add a mapping attribute for DMA
  memory later.

**Who creates DMA memory.** Only holders of a device capability with `DMA`. The device manager
keeps those: it creates each driver's DMA memory itself, works out the bus address, and hands
the driver a *copy without `BUS`* together with that address in the device record (note 0011,
section 6). The memory is contiguous, so the driver only needs the address plus an offset, and
never needs the right to ask. This is so that a dying driver can't free memory under a device
that is still running (note 0012, section 2).

### 7. The early console, and the screen

The kernel's serial output and on-screen text console stay in the kernel for good:
- the log and panic path must work when every driver is dead (it's how we see that they are);
- the boot tests read the kernel's serial output.

So user-space drivers drive **other** devices: the second serial port, not the console's.
The one shared resource is the screen. The display stack (the framebuffer driver in M4, the
compositor in M6) needs the framebuffer to itself:
- `console_screen_release(log h)` stops the kernel drawing text on the screen. Needs the new
  kernel-log right **`CONTROL`** (type bit 8). The root program holds it and gives it only to
  the display stack. Serial output continues.
- A **panic still draws** on the screen regardless, over whatever is there, because seeing the
  panic matters more than the picture.

### 8. What the root program receives

Replacing note 0009's "capabilities for every device memory range, interrupt and I/O port
range" (which would have meant the kernel knowing every device) with one capability of each
kind. New start-up block roles (note 0009, section 4):

| Role | What the handle is |
|------|-------------------|
| 7 | Device memory: the whole physical address space as one range. Slicing or mapping anything that touches RAM or a kernel-owned page is refused. Rights: all, with `DMA` |
| 8 | Interrupts: every interrupt number the interrupt controllers have, as one range; binding a kernel-owned one is refused. All rights |
| 9 | The message-signalled interrupt pool (not present if the machine has none) |
| 10 | I/O ports: 0 to 65535 as one range; slicing or using a kernel-owned port is refused (x86-64 only) |
| 11 | The machine description: a read-only memory object (note 0011, section 2) |

The kernel log capability (role 3) gains `CONTROL`.

### 9. System calls and versions

These additions are **interface version 1.1** (a compatible minor version: note 0007,
section 7). Numbers are never reused.

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 37 | `memory_create_dma` | device h, size, flags | memory h | `DMA` on device |
| 38 | `memory_bus_address` | memory h, device h, offset | address | `BUS`; `DMA` on device |
| 72 | `console_screen_release` | log h | | `CONTROL` |
| 80 | `device_memory_slice` | h, offset, length, rights mask | new h | `DUPLICATE` |
| 81 | `device_memory_info` | h | address, length | `INSPECT` |
| 82 | `interrupt_slice` | h, first, count, rights mask | new h | `DUPLICATE` |
| 83 | `interrupt_info` | h | kind (low 32 bits) and count (high 32 bits), first number | `INSPECT` |
| 84 | `interrupt_bind` | h, index, notification h, bits | | `BIND`; `SIGNAL` on the notification |
| 85 | `interrupt_unbind` | h, index | | `BIND` |
| 86 | `interrupt_acknowledge` | h, index | | `ACKNOWLEDGE` |
| 87 | `interrupt_msi_allocate` | pool h | interrupt h | `DUPLICATE` |
| 88 | `interrupt_msi_info` | h | address, data | `INSPECT` |
| 90 | `ioport_slice` | h, first, count, rights mask | new h | `DUPLICATE` |
| 91 | `ioport_in` | h, offset, width | value | `READ` |
| 92 | `ioport_out` | h, offset, width, value | | `WRITE` |
| 93 | `ioport_info` | h | first, count | `INSPECT` |
| 99 | `interrupt_trigger` | h, index | | `ACKNOWLEDGE`; only when the kernel was started with `selftest`, otherwise `NOT_SUPPORTED` |

`memory_map` also accepts device memory (section 3). Call 99 makes a given interrupt pending
by software, so the self-test can exercise binding, masking and release without a device
(on x86-64 by sending the vector to its own core, on AArch64 by setting the interrupt
pending in the GIC). It exists only in self-test runs.

Rights changes (note 0006's table): the interrupt type's bit 8 is now **`BIND`** (it was
`WAIT`, which nothing used); bit 9 stays `ACKNOWLEDGE`. Device memory's bit 8 is `DMA`; I/O
ports' bits 8 and 9 are not used. Memory objects' bit 8 is `BUS`. The kernel log's bit 8 is
`CONTROL`. No new error codes are needed: wrong ranges are `INVALID_ARGUMENT`, a second bind
is `ALREADY_EXISTS`, ports on AArch64 or a missing MSI pool are `NOT_SUPPORTED`.

**Budgets.** A slice, a bound interrupt and an I/O port capability are charged like a
notification (note 0006, section 9); DMA memory is charged by its pages. The `process_info`
record doesn't change.

## Alternatives considered

- **The kernel scans PCI and creates one capability per device.** The kernel would then need a
  PCI scanner, the table of devices and their resources, and eventually ACPI's bytecode
  interpreter, all of which are only needed to find drivers. They belong in a program (note
  0011). The cost of this choice is that the root program's capability is broad, but it only
  ever goes to the device manager, which narrows it.
- **Interrupt controllers as user-space drivers.** The scheduler's own timer interrupt needs
  them.
- **Interrupts as messages from a kernel thread** (L4's approach). It would add a second
  delivery mechanism; notifications already exist and bind to endpoints.
- **Device memory as ordinary memory objects with a "device" flag.** Then "never RAM" and
  "never executable" would be flags to check everywhere memory is used, instead of a type
  that cannot be confused.
- **A permission bitmap for ports.** Faster, but extra work on every process switch for devices
  that don't need it (section 5).
- **Pinning any memory for DMA, with scatter-gather.** What zero-copy disk and network I/O will
  eventually want. It needs accounting for pinned pages and a way to stop a client unpinning
  memory under a device. The contiguous-memory design is simple to get right now, and
  `memory_bus_address` already works page by page, so this can be added later.
- **Letting any process create DMA memory.** Then the right to learn bus addresses would not
  be tied to any device, which an IOMMU needs.

## Open questions

- An IOMMU (VT-d, AMD-Vi, SMMU): a later milestone. It makes `memory_bus_address`'s device
  argument real and closes the gap described in section 6.
- Shared legacy interrupt lines, with AML (M7).
- Non-coherent DMA (some Arm boards) needs a mapping attribute (section 6).
- Whether very large DMA buffers need more than 4 MiB of contiguous memory; scatter-gather
  would remove the limit.
- GICv3 and its ITS for message-signalled interrupts (with GICv3 support).
- `interrupt_acknowledge` could also check that the caller's process made the binding, as
  `interrupt_unbind` does. Not needed while ranges never overlap between drivers.

## Changes
- 31 October 2026: first version, draft for approval; approved by Chris the same day.
- 2 November 2026: built (M4 stages 1 to 4).
  - **Delivery goes through a kernel thread.** The handler masks, acknowledges and sets a bit;
    the `irq` thread signals the notification. Signalling from the handler itself would have
    meant making every wait queue safe to take with interrupts masked.
  - The interrupt controllers' lock is level 4 in note 0005's order (the draft put an
    interrupt-table lock below the wait queues; there is no such lock in the handler now).
  - QEMU's ACPI MADT on `virt` does describe the GICv2m frame (entry type 0xD), so the kernel
    takes it from there; if the entry doesn't give the interrupt range, the frame's `MSI_TYPER`
    register does. The ARM program self-test fails if there is no pool, since `virt` always has
    one.
  - The machine description gained the FADT's 8042 flag (the kernel reads it, x86-64 only), and
    the kernel reads the MADT's interrupt source overrides for ISA lines' polarity and trigger.
  - A restarted driver keeps its interrupt vectors (above, and note 0012).
- 8 November 2026, M5 stage 8: **the screen hand-over as built.** Only the display driver
  holds `CONTROL` on the kernel log. `Spawn::log` in `granite` now gives a child the log
  without it (until then every service, driver and filesystem server got all rights, so any of
  them could have taken the screen). The root program gives the device manager a copy with
  only `CONTROL` at its role 1001; the device manager passes it to the firmware framebuffer's
  driver alone, at role 1050 (and only to a driver whose manifest says `class display`). The
  driver calls `console_screen_release` on its first `framebuffer()` call, since from then on a
  client (the console) draws; it lets go of the capability afterwards. The kernel's text console
  and the console service share one font, `lib/font`.
