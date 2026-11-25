# 0027: Kernel interface 1.4: real hardware

Status: Draft · 23 November 2026 · Related: DESIGN.md sections 6.1, 6.7 and 7.1; notes 0003,
0004, 0005, 0007, 0010, 0011, 0019, 0028 to 0031; `docs/m7-plan.md` (Q6, Q7, Q9, Q11, Q13);
milestone M7

## The problem

Until now PrettyBigRock has only run in QEMU and in virtual machines on Harriet. M7 runs it on
Roxy, a real PC from 2011 (`docs/m7-plan.md`, section 1): an Intel i5-2300 (Sandy Bridge) on a
Foxconn H61MX board with AMI firmware from 2013. Real hardware asks five things of the kernel
that QEMU never has:

1. **Protection from Meltdown and its relatives.** Since 2018 we know that many Intel
   processors, while guessing ahead ("speculative execution"), briefly let a program read
   memory it has no right to, and that the program can recover what it read from the timing of
   the caches. Three such flaws affect Roxy's processor:
   - **Meltdown** (2017): a program can read any memory the kernel has mapped, because the
     kernel's pages are in the program's page tables (marked kernel-only, which the guessing
     ignores). The kernel maps all of memory (the direct map, note 0005), so a program can read
     everything: other programs' files, keys, passwords.
   - **MDS** (2019): small buffers inside the processor (for stores, loads and cache fills)
     keep the kernel's recent data after it returns to a program, and a program can read them
     the same way.
   - **L1TF** (2018): a page-table entry marked "not present" is still followed while guessing,
     to whatever physical address its bits hold, if that memory is in the level 1 cache.

   PrettyBigRock's capabilities mean nothing if any program can read any memory, so these must
   be closed on processors that have them. Processors that say they're not affected (newer
   Intel ones, AMD's, and QEMU's on GitHub's runners) shouldn't pay for it.
2. **Shared interrupt lines.** Roxy's two USB controllers (EHCI) can't send message-signalled
   interrupts (MSI), so they use the old PCI interrupt lines, which several devices may share
   (Linux puts them on I/O APIC lines 16 and 23). Note 0010 allowed one driver per interrupt
   and left shared lines for M7.
3. **The serial port.** The kernel's log goes to COM1 (`0x3F8`). Roxy has no COM1; its only
   serial port is a header on the board, COM2 at `0x2F8`, interrupt 3. Without it we are blind
   on Roxy.
4. **Knowing which disk we started from.** Booting from a USB stick (Q6), the storage service
   must choose the system volume on the stick, not on whatever other disk is plugged in. Only
   the loader knows which disk it read.
5. **What real hardware does differently**: firmware leaving things half set up, clocks
   disturbed by the firmware's own interrupts (SMIs), video memory that is slow to read. Some
   of this we can see coming from Roxy's ACPI tables (section 5); the rest stage 1 will find,
   and this note will record it.

M7's plan also put **write-combining for framebuffers** here. It already exists: M4 built it
(note 0010, section 3; `perm::WRITE_COMBINING`), and the framebuffer driver uses it. On Roxy
the firmware's memory-type ranges (MTRRs) may mark video memory uncached, but a
write-combining page attribute wins over an uncached MTRR (Intel's manual, volume 3, table
"effective memory type"), so nothing needs to change. Section 5 deals with the part that does:
reading video memory.

The plan's question Q11, **a lock-order checker for plain spin locks**, is answered here too
(section 6), since real hardware's timing is where deadlocks first show.

## The design

The parts programs can see make **interface version 1.4**, a compatible minor version (note
0007, section 7): programs built for 1.0 to 1.3 run unchanged. The system page's minor version
becomes 4. The boot information becomes version **1.1** and the machine description **1.1**
(section 4).

### 1. Meltdown, MDS and L1TF (x86-64)

#### 1a. Which processors, and the `mitigations=` switch

At start-up the boot core decides, once, for every core:

| Flaw | Affected when |
|------|---------------|
| Meltdown | The vendor is `GenuineIntel`, unless `IA32_ARCH_CAPABILITIES` (MSR `0x10A`, present when CPUID leaf 7 says so in `EDX` bit 29) has `RDCL_NO` (bit 0) |
| L1TF | As Meltdown (the processors not affected by one aren't by the other) |
| MDS | The vendor is `GenuineIntel`, unless `IA32_ARCH_CAPABILITIES` has `MDS_NO` (bit 5) |

Other vendors (AMD, Hygon, and QEMU's own models) aren't affected by any of the three. Linux
also keeps a list of old Atom processors that aren't affected; we don't, and they get the
protections they don't need, which is harmless.

The command line (`cmdline.txt`) can override this, for testing and measuring:
- `mitigations=auto` (the default): as the table says;
- `mitigations=off`: none, whatever the processor;
- `mitigations=on`: all three, even on a processor that isn't affected. This is how QEMU on
  GitHub's runners (AMD processors, so "not affected") tests the code (section 9).

The start-up report says what was decided, one line each, for example:
`Meltdown: separate page tables (KPTI)`, `MDS: buffers cleared on return (VERW)` or
`MDS: affected, but the microcode can't clear buffers`, `L1TF: not-present entries are empty`.

ARM64 needs none of this in M7: QEMU's Cortex-A72 and the Raspberry Pi 5's Cortex-A76 aren't
affected by Meltdown, and MDS and L1TF are Intel's alone.

#### 1b. Separate page tables for programs (KPTI)

Meltdown can only read what is mapped. So while a program runs, the kernel is (almost) not
mapped: each process gets **two top-level page tables**.
- The **kernel copy** maps everything, as today's single table does: the program's half and
  the kernel's.
- The **user copy** maps the program's half identically, and of the kernel's half only the
  **entry area** (below): what the processor itself must reach to get from a program into the
  kernel.

The two top-level pages are allocated together, 8 KiB aligned, kernel copy first, so the user
copy's address is the kernel copy's plus 4096: entry code switches between them by setting or
clearing bit 12 of `CR3`, without reading memory. Both point to the same lower-level tables for
the program's half, so a mapping change made below the top level is seen by both; a change to a
top-level entry in the program's half is written to both. The extra page is charged to the
process's budget with the first (note 0006, section 9). Kernel threads, which never return to a
program, use the kernel's own table as today.

**The entry area** is a new region of the kernel's address space (note 0005, section 2,
gains a row): `0xFFFF_D000_0000_0000` to `0xFFFF_D000_4000_0000`, mapped identically in every
kernel copy and user copy, never executable except its code. It holds:
- the **entry code**: the interrupt stubs and the system call entry, in their own linker section
  (`.text.pbr_entry`), mapped read-and-execute;
- the shared **IDT**, read-only;
- for each core, a fixed-size slot with that core's **GDT** and **TSS** (which the processor
  reads on every entry from a program), a small **entry stack**, the three **IST stacks** (for
  double faults, NMIs and machine checks, note 0005, section 5) with guard pages, and an
  **entry block**: the few values the entry code needs before it can switch tables (a scratch
  slot for the program's stack pointer, the current thread's kernel stack top, and the address
  of the core's `PerCpu`).

`GS` points at the core's entry block instead of its `PerCpu`, and `percpu::this()` reads the
`PerCpu` address from it: one more load, so that `PerCpu`, with the run queues and everything
else, stays unmapped while programs run.

**Entering the kernel.** The TSS sends interrupts from a program to the core's entry stack.
The stub swaps `GS`, sets `CR3` to the kernel copy, copies the processor's saved frame to the
thread's kernel stack, switches to it, and carries on as today. The system call entry does the
same with the program's stack pointer in the scratch slot. **Leaving** goes the other way:
the registers are restored from the kernel stack, the return frame is copied to the entry
stack, `CR3` is set to the user copy, `GS` swapped back, then `iretq`.

**NMIs, machine checks and double faults** can arrive at any instruction, including between a
system call's `syscall` and its table switch, where the saved privilege level says "kernel"
but `GS` and `CR3` are still the program's. Their stubs therefore don't trust the saved
privilege level: they read `IA32_GS_BASE` and `CR3`, switch to the kernel's if either is a
program's, and on the way out restore exactly what they found.

**One code path.** When KPTI is off, the same entry area and stubs are used and only the two
`CR3` writes are skipped (a test of one byte in the entry block). So QEMU runs with KPTI off
still exercise the entry area, and the difference between on and off is only the table switch.

**Without PCID at first.** Every `CR3` write discards the processor's cached translations
(the TLB) for everything not marked global. With KPTI each system call writes it twice. Process
context identifiers (PCID: tagging cached translations with a 12-bit number, so they survive
the switch) remove most of that cost; Sandy Bridge has them but not the `INVPCID` instruction
that makes them simple to manage. M7 measures the cost first (section 9) and adds PCID only if
it is large.

**Two smaller protections come with it:**
- **On processors without SMEP** (Roxy's: SMEP arrived with Ivy Bridge), the kernel copy marks
  the program's half no-execute at the top level. SMEP stops the kernel running a program's
  code by mistake, a favourite step in attacks; this gives the same protection for free.
- The interrupt stubs' `swapgs`, which happens only when coming from a program, is followed by
  `lfence` on both paths, so the processor can't guess the wrong way and run kernel code with a
  program's `GS` (the "SWAPGS" variant of Spectre, 2019).

#### 1c. Clearing the processor's buffers (MDS)

When MDS is on, the very last thing before `iretq` to a program (after `CR3` is switched) is
`verw` on a memory word holding the kernel's data selector, kept in the entry area. On
processors whose microcode has been updated for MDS (CPUID leaf 7, `EDX` bit 10, `MD_CLEAR`),
this old instruction also empties the buffers. Without that microcode it does nothing useful;
the report says so (section 1a). **Roxy is in that state**: its firmware's microcode is
revision `0x28`, from before the fix, and Linux on Roxy is protected only because it loads
revision `0x2F` at start (Chris, 25 November: Linux reports `Updated early from: 0x00000028`,
and `mds: Mitigation: Clear CPU buffers`). So PrettyBigRock loads the update itself (section
1e).

Returns from the kernel to the kernel don't clear anything: only a program can be the reader.

MDS is also a danger between two hyper-threads of one core, which run at the same time and
share the buffers. Roxy's processor has no hyper-threading; PrettyBigRock's answer for
machines that do is an open question.

#### 1d. Harmless not-present entries (L1TF)

L1TF follows the address bits of a not-present entry. The rule: **a page-table entry that
isn't present is entirely zero**, so it points at physical page 0, which the page allocator
never hands out (note 0005, section 3) and nothing secret lives in. Today's code already
writes 0 when it unmaps; the rule makes it a promise:
- `lib/platform`'s page tables never write a not-present entry with any other bit set; nothing
  is ever stored in a not-present entry (no "swapped out" markers, no lazy mappings) without a
  change to this note;
- its host tests check, after every operation in their random sequences, that every
  not-present entry is zero.

L1TF's other half is virtual machines (a guest can aim the flaw at the host's memory); that
waits for virtual machines.

#### 1e. Microcode updates

A processor's **microcode** is the firmware inside it that carries out its more complicated
instructions. Intel fixes flaws by publishing new microcode, which the firmware or the
operating system loads into the processor at every start (it is forgotten when the power
goes). Roxy's firmware loads revision `0x28`; the fix for MDS (and the controls Spectre
variant 2 needs) came in `0x2F`. Chris agreed on 25 November 2026 that PrettyBigRock loads
Intel's updates, under the rule for firmware in DESIGN.md section 14: optional, never kept in
the repository, and the system works without them (with MDS open on processors like Roxy's).

- **Never in the repository** (Chris, 25 November): it would stay in git's history for ever,
  growing with each of Intel's releases (the full set is about 21 MB in 126 files), and a copy
  there goes stale unnoticed. The files come from outside at build time instead.
- **On the boot disk.** The build tool, given `PBR_MICROCODE=<directory>` (Ubuntu's
  `intel-microcode` package puts Intel's files in `/lib/firmware/intel-ucode/`), copies every
  file in it to `PBR/x86_64/microcode/` on the EFI system partition, and into the BIOS
  loader's payload, keeping Intel's names: family, model and stepping in hexadecimal, such as
  `06-2a-07` for Roxy's processor, with Intel's licence text beside them. Without it, there is
  no folder, and nothing else changes.
- **The UEFI loader** reads CPUID leaf 1 on the boot core, works out that name, and loads only
  that file, as a start-up module named `microcode` (note 0003's `modules`). A missing file is
  not an error.
- **The BIOS loader** can't read files: its second stage copies the whole payload to memory,
  through the BIOS, before anything else runs, and reading through a real BIOS can be slow (a
  USB stick through the firmware's emulation perhaps a few megabytes a second), so copying
  all 21 MB would add seconds to every start. So the microcode files go **after** the rest of
  the payload, with a table of their names, offsets and lengths in the payload's header; the
  second stage runs CPUID itself (it works in 16-bit mode), and reads only the matching file,
  after the rest. The BIOS boot partition grows from 32 MiB to 64 MiB to hold them (note 0004;
  the size is only a constant in the build tool).
- **The kernel** reads the module straight after its heap is ready (note 0005, section 1,
  step 6), before the hardware description, and well before the protections of sections 1a to
  1d are decided:
  - the file is a list of updates in Intel's format, each a 48-byte header (header version 1,
    revision, date, processor signature, checksum, loader version 1, processor flags, data
    size, total size) followed by its data and an optional table of further signatures. The
    kernel walks the list with every length checked against what is left, as `lib/devices`
    does configuration space, and skips anything that doesn't parse;
  - an update fits if its signature (or one in its extra table) equals CPUID leaf 1's `EAX`
    and its processor flags include the bit `IA32_PLATFORM_ID` (MSR `0x17`, bits 50 to 52)
    names, and its 32-bit words sum to zero. Of those, it takes the newest, and only if newer
    than the running revision (MSR `0x8B`, read after writing 0 to it and running CPUID):
    never older or the same;
  - it copies that update's data to a 16-byte aligned buffer, writes the buffer's address to
    `IA32_BIOS_UPDT_TRIG` (MSR `0x79`), and reads the revision again to see that it took;
  - each other core does the same with the same buffer as the first thing it does when it
    starts (note 0005, section 7), and the buffer is freed once every core has started;
  - the start-up report says `Microcode: 0x28, updated to 0x2f`, `Microcode: 0x2f (no newer
    update)`, or `Microcode: the update was refused`. A core that ends up on a different
    revision from the boot core is logged.
- **The processor checks Intel's signature** on the update itself and refuses anything else,
  so a damaged or hostile file can't put foreign code into the processor; the kernel's own
  checks only keep it from passing nonsense. Only Intel's updates are loaded in M7; AMD's
  (another format) can follow when a machine needs them. In QEMU nothing is loaded (QEMU
  ignores the MSR), so this is tested on the host and on Roxy (section 9).
- **Release images include them** (Chris, 25 November): every Intel machine whose firmware is
  out of date, as most are, gets the fixes. The release build downloads Intel's published
  release (`intel/Intel-Linux-Processor-Microcode-Data-Files` on GitHub) at a pinned version,
  checks it against a recorded SHA-256 sum, and passes it to the build tool as above; moving
  to a newer release is a one-line change, reviewed like any other. Note 0031 builds this.

### 2. Shared interrupt lines

**What sharing means.** A PCI device without MSI raises its interrupt by pulling a wire low
and holding it there until its driver has dealt with it ("level-triggered"). Several devices'
wires are joined into one line. When the line is asserted, the kernel can't tell which device
did it, so every driver on the line must look at its own device, and the line may only be
unmasked when all of them are done, or it fires again at once.

**Which interrupts may be shared:** line interrupts that are **level-triggered**. On x86-64
these are the I/O APIC pins above 15 (PCI's lines, active low and level-triggered unless the
firmware's MADT says otherwise) and any ISA line an override makes level (on Roxy, the ACPI
interrupt on line 9). On ARM64, the GIC's shared peripheral interrupts that aren't
message-signalled, all of which the kernel configures as level. Edge-triggered lines (the ISA
devices: serial ports, PS/2) and message-signalled interrupts are never shared: a second
binding still fails, as before.

**The rules** (changes to note 0010, section 4):
- `interrupt_bind` on a shareable line succeeds even if another process has bound it. **One
  binding per process per line**: binding again from the same process, through any
  capability, is `ALREADY_EXISTS` as before.
- When the line fires, the handler masks it and acknowledges it at the controller as today.
  The `irq` thread signals **every** binding's notification with its bits, and marks each
  binding as **owed** an acknowledgement.
- `interrupt_acknowledge(h, index)` now acts on **the calling process's own binding** of that
  interrupt (it fails with `BAD_STATE` if the process has none). It clears that binding's
  "owed"; when no binding on the line is owed, the line is unmasked. Acknowledging when
  nothing is owed is harmless, as before. (This settles note 0010's open question: a process
  can never acknowledge for another.)
- A binding released (by `interrupt_unbind`, its capability revoked, or its process ending)
  stops being owed. If other bindings remain and none is owed, the line is unmasked; if none
  remain, the line is masked and its vector given back, as before. So a driver that crashes in
  the middle of an interrupt doesn't hold its neighbours' line masked.
- A binding made while the line is masked waiting for others isn't owed anything. It hears of
  the next interrupt.
- Each binding is charged to the process that made it, as before. There is no fixed limit to
  how many share a line.

**Who may share.** The kernel doesn't decide who belongs on a line; whoever holds a capability
for it with `BIND` may bind it. The device manager gives a driver a one-line slice only for the
line its device is wired to, which ACPI's `_PRT` says (note 0028). Two drivers on one line hold
two slices of the same line, which was impossible before.

**What sharing costs.** A slow driver delays the interrupts of the others on its line, and a
driver that hangs without ending holds the line masked until it ends (hung drivers are an open
question of notes 0012 and 0018). A device whose wire is stuck asserted with no driver for it
would make the line fire again as soon as it is unmasked, for ever. To prevent that, **the
device manager sets each PCI function's "interrupt disable" bit** (bit 10 of the command
register) while no driver is running for it, from the start and whenever its driver ends, and
clears it just before starting a driver that will use a line (a change to note 0011, made with
note 0028).

**Trigger and polarity.** The kernel keeps choosing them as today: from the MADT's overrides
for ISA lines, active low and level for x86-64's other lines (which is what ACPI says of every
line `_PRT` names directly), level for ARM64's. A machine whose `_PRT` routes through
"interrupt link" devices with other settings would need a call to configure a line; Roxy's
firmware describes its links but names the lines directly when told the system uses APICs
(`\_PIC(1)`, note 0028), and QEMU's do the same, so that call waits until a machine needs it.

### 3. The serial console's port

The **loaders** choose the early console and put it in the boot information (note 0003,
`early_console`); the kernel uses whatever they say, and already treats that port as its own
(note 0010, section 1: slicing it is refused). So the change is in the x86-64 loaders, UEFI and
BIOS, which today always say COM1. They now choose, in order:

1. a **`console=` word in `cmdline.txt`**:
   - `console=0x2f8` (any I/O port, in hexadecimal): a 16550 UART on that port;
   - `console=none`: no serial console;
2. else the ACPI **SPCR** table, if it describes a 16550 on I/O ports or in memory (the same
   code the ARM64 loader already uses, `lib/platform`'s `spcr_console`);
3. else **COM1** at `0x3F8`, as today.

The command line comes first because it is the person's choice, and firmware tables are
sometimes wrong (the order in `docs/m7-plan.md` had SPCR first; this note changes it). The
loaders write their own messages to the same port. As today, the kernel checks that a UART
answers at the port (its scratch register) and has no serial console if not.

Roxy has no SPCR table, so the stick's `cmdline.txt` says `console=0x2f8`. The device
manager's check for COM2 then fails (it can't slice the kernel's port), and it logs
`com2: the kernel's console` instead of `not present`. When note 0028 replaces the built-in
list with AML, a serial port at the kernel's port is skipped the same way.

The ARM64 loader keeps its order (SPCR, then the device tree) and honours `console=none`.

### 4. The boot partition

**Boot information version 1.1** (note 0003) adds one field at the end:

| Offset | Size | Field | Meaning |
|-------:|-----:|-------|---------|
| 536 | 16 | `boot_partition` | The unique partition GUID of the GPT partition the loader read the kernel from, as its 16 bytes are stored in the GPT entry; all zero if not known |

`size` becomes 552. A kernel given version 1.0 information (`size` 536) treats the field as
zero, as note 0003's rules say.

- **The UEFI loader** takes it from the device path of the partition it was loaded from (the
  loaded image's device handle): the "hard drive" node with signature type 2 (GPT) holds the
  partition's unique GUID. That is the EFI system partition.
- **The BIOS loader** reads the GPT itself: its second stage, while the BIOS's disk services are
  still usable, reads the GPT header (sector 1) and the partition entries, and the third stage
  finds the entry whose first sector is the BIOS boot partition's (the one the first stage was
  told, note 0004) and takes its GUID. The header's and entries' checksums are checked; if
  anything doesn't match, the field is zero.
- **The machine description version 1.1** (note 0011, section 2) carries the same 16 bytes at
  offset 48, in what was a reserved field, so zero still means "not known". The storage service
  receives it from the root program and makes the system volume a RockFS volume on the disk
  holding that partition (note 0030 settles how, including what to do when two disks match,
  as two sticks written from one image will).

The kernel logs the GUID at start, and otherwise does nothing with it.

### 5. Things real hardware does differently

What we can foresee from Roxy's tables and from experience. Stage 1 adds what Roxy itself shows
under Changes.

- **The legacy interrupt controller (8259 PIC).** The kernel masks it, but doesn't move its
  vectors first; the firmware leaves them at 0x08 to 0x0F, where the processor's exceptions
  are, so a stray interrupt from it would look like a double fault. The kernel now moves its
  vectors to 0x20 to 0x2F before masking, ignores the two "spurious" vectors it can still send
  (0x27 and 0x2F, without telling the PIC it's done), and masks each local APIC's `LINT0`
  input, through which the firmware's "virtual wire" mode passes the PIC's interrupts on.
- **Calibrating the clock.** The TSC and the APIC timer are calibrated against the HPET (Roxy
  has one) over 20 ms. A firmware interrupt (SMI) during that time makes the result wrong for
  the whole run. Calibration now takes three 10 ms measurements and uses the middle one.
- **The TSC on every core.** Programs read the TSC themselves (note 0007, section 1), which is
  only right if every core's TSC agrees. Each core, as it starts, compares its TSC with the
  boot core's (a handshake through a shared variable, a few times); if they differ by more than
  the handshake's own time, the kernel logs it and the clock falls back to the HPET.
- **The APIC timer in idle.** Some processors stop the local APIC timer in their deeper sleep
  states. Roxy's says it doesn't (CPUID leaf 6, `ARAT`), and the kernel's idle loop only uses
  `hlt`. The kernel logs it if a processor lacks `ARAT`; doing more waits for a machine that
  needs it.
- **Reading video memory is slow.** Writing to write-combining memory is fast; reading it back
  is very slow (each read goes to the device, uncached). The kernel's text console scrolls by
  copying the screen upwards, which reads all of it: on a 1920 × 1080 screen that is 8 MiB read
  per line, perhaps a tenth of a second on Roxy, and seconds for a boot log. It will keep its
  text in ordinary memory and redraw from that instead, never reading the screen. (The display
  stack already never reads it.)
- **What Roxy's ACPI tables say** (`tests/data/acpi/roxy-h61mx/`): four cores with APIC IDs 0,
  2, 4 and 6; one I/O APIC with 24 lines; the ISA timer on line 2 and the ACPI interrupt on line
  9 (level, active high); an HPET; the FADT's reset register is port `0xCF9` with value 6, which
  the kernel already uses for restarting (note 0019). The FADT **does not** set the "8042
  present" flag, so the device manager won't start the PS/2 driver on Roxy until note 0028
  finds the keyboard controller in AML. Before then, a PS/2 keyboard on Roxy wouldn't work; a
  USB one does through the firmware's emulation (note 0029).

### 6. The lock-order checker for every spin lock (Q11)

Note 0005 (section 9) gives every interrupt-masking lock (`IrqSpinLock`) a level, and debug
builds report taking a lock whose level isn't above every lock already held. Plain `SpinLock`s
(a process's capability table and address space, endpoints, notifications, the interrupt
bindings and about twenty more) have no level, so an order mistake among them is found only
when it deadlocks, which on real hardware's timing may be rarely and late.

From M7 **every spin lock has a level and a name**, checked the same way in debug builds:
- one order covers both kinds; a plain lock may be held while taking an interrupt-masking one,
  so the plain locks' levels are all below the existing ones;
- locks of one kind on many objects (two processes' capability tables, two endpoints) share a
  level. Two of them may be held at once only when taken through a helper that takes them in
  order of address, which the checker recognises; taking a second one any other way is
  reported;
- the order is worked out by reading the code while building, and written into note 0005,
  section 9, as the existing levels are. Anything the self-test boots report is a bug to fix
  then, not a level to loosen.

Release builds don't check, so there's no cost where it matters.

### 7. In `granite`

Nothing new to call. `Interrupts::acknowledge`'s documentation says it acknowledges the
calling process's own binding, and `granite::sys::version()` returns (1, 4). The root program passes the
machine description's boot partition to the storage service (note 0030 says how).

### 8. What changes, in short

| What | Before | Now |
|------|--------|-----|
| `interrupt_bind` (call 84) on a level-triggered line bound by another process | `ALREADY_EXISTS` | Shared (section 2) |
| `interrupt_acknowledge` (call 86) | Unmasks the interrupt if anyone has bound it | Acts on the caller's own binding; unmasks when no binding is owed; `BAD_STATE` without one |
| `interrupt_trigger` (call 99, self-test runs only) | No result | Result 1: whether the line was masked at the controller when called (1) or not (0), so the self-test can see sharing work |
| Interface version (the system page and call 1) | 1.3 | 1.4 |
| Boot information | 1.0, 536 bytes | 1.1, 552 bytes, with `boot_partition` |
| Machine description | 1.0 | 1.1, with the boot partition at offset 48 |
| Command line | `selftest`, … | Also `mitigations=auto/on/off` (kernel) and `console=` (x86-64 loaders) |
| Start-up modules | `startup` | Also `microcode` (x86-64, optional, section 1e) |

No new calls, rights, object types or error codes.

### 9. Testing

- **Microcode.** On the host: the parser on synthetic files in Intel's format (several updates,
  extra signature tables, bad checksums, lengths running past the end, random corruption), and
  on the files in `/lib/firmware/intel-ucode/` when the machine has them (skipped otherwise, as
  the FAT32 tests are without `mtools`): every update in them parses, and Roxy's file
  (`06-2a-07`) gives revision `0x2F` for its signature. On Roxy in stage 1: the report says
  `0x28, updated to 0x2f` on every core, and MDS is cleared.
- **Host tests.** `lib/platform`: page tables never leave a non-zero not-present entry (section
  1d), in the existing random sequences; parsing `console=`; the UEFI device path's hard drive
  node; finding the boot partition in a GPT, with corrupted headers and entries. `lib/bootinfo`:
  version 1.1's offsets, at compile time as now. `lib/devices`: the machine description 1.1.
- **Mitigations in QEMU.** GitHub's runners are AMD processors, so `auto` turns everything off
  there. One configuration therefore always runs with `mitigations=on`: recommended, the x86-64
  BIOS one, which note 0031 makes "Roxy-like" (Roxy is affected). On Impala, with KVM, the
  processor (an i7-7700K) is affected, so `auto` turns everything on in every x86-64
  configuration. Between them, both paths run on every change.
- **In the kernel's self-test**, when KPTI is on: it walks a program's user copy and checks the
  kernel's half maps nothing but the entry area; on a processor without SMEP, that the kernel
  copy's program half is no-execute. With MDS on, that the report's line matches `MD_CLEAR`.
- **In the program self-test**, on all four configurations:
  - **Sharing:** a second process binds the level-triggered line the self-test already uses
    (the last one, which nothing is wired to); both are signalled when it is triggered;
    `interrupt_trigger` reports the line still masked after one has acknowledged and unmasked
    after both; one process ending while owed unmasks the line for the other; a second binding
    from the same process is still refused; acknowledging with no binding is `BAD_STATE`; an
    edge-triggered line (x86-64) or a message-signalled interrupt still refuses a second
    process.
  - **The cost of KPTI:** 100,000 calls of a cheap system call (`handle_info` on a bad handle),
    timed, and the result logged with the mitigations in force. This is reported, not checked:
    it is how we decide about PCID (section 1b).
- **The real thing**: two devices without MSI sharing a line, in a test configuration (note
  0031: QEMU's EHCI has no MSI, and its slot can be chosen so another device shares its line),
  and the serial log through COM2 on Roxy in stage 1.

## Alternatives considered

- **No KPTI; rely on the processor.** Roxy's processor will never be fixed, and it is the
  machine we are building for. Without KPTI, every program can read all of memory.
- **Mapping each thread's kernel stack, or all of `PerCpu`, in the user copy**, instead of an
  entry stack and entry block. Less copying on entry, but the stacks and `PerCpu` hold kernel
  pointers and data, which Meltdown would then read. The entry area holds nothing a program
  couldn't learn anyway.
- **Two separate page-table trees per process**, rather than two top-level pages sharing the
  lower levels. Double the work for every mapping, for no gain.
- **KPTI everywhere, always.** Simpler to test, but it would cost every unaffected processor
  (most made since 2019) for nothing.
- **Sharing only when the binder asks for it** (a flag on `interrupt_bind`). A driver of a
  device without MSI must already be written for a shared line (check its device's status,
  acknowledge even when it wasn't its device), and whether the line is shared depends on the
  board, which the driver can't know. The device manager's slicing already decides who may be
  on a line.
- **Acknowledgements that say whether the device was the cause**, so the kernel can notice a
  line nobody claims and turn it off (Linux does this). It needs every driver to answer
  correctly, and turning the line off silences the innocent drivers on it too. Disabling the
  interrupts of devices without a driver (section 2) stops the usual cause.
- **Polling Roxy's USB controllers** instead of sharing lines (Q13). Costs power and latency
  on every key press, and AML is needed anyway.
- **The kernel choosing the serial port**, reading `console=` itself. The boot information
  already has `early_console`, filled by the loaders, and the ARM64 loader already reads SPCR;
  keeping the choice there means one place, and the loaders' own messages go to the same port.
- **Identifying the boot disk by the disk's GPT GUID**, rather than a partition's. The UEFI
  loader would have to find the whole disk's handle and read its header through Block I/O; the
  partition's GUID is in the device path it already has. Either identifies the disk.

## Open questions

- **Spectre variant 2** (a program training the processor's guesses of indirect jumps to
  steer the kernel's): needs retpolines, which our pinned compiler offers only on nightly, or
  the IBRS and IBPB controls, which the microcode of section 1e brings to Roxy. Not in M7.
- **AMD's microcode**, when a machine needs it (section 1e).
- **Hyper-threading and MDS** (section 1c), and L1TF for virtual machines (section 1d).
- **PCID**, if the measurement (section 9) shows KPTI is costly.
- **Configuring a line's trigger and polarity** from AML (section 2), if a machine needs it.
- **Global pages for the kernel** when KPTI is off: today no kernel page is global, so every
  address-space switch discards the kernel's cached translations too. A later improvement.
- **Two disks with one boot partition GUID** (section 4): note 0030.

## Changes
- 25 November 2026: microcode never in the repository; release images fetch Intel's pinned
  release; the BIOS loader reads only the matching file, from a 64 MiB BIOS boot partition.
- 25 November 2026: Chris agreed that PrettyBigRock loads Intel's microcode updates; section
  1e, with DESIGN.md section 14's rule for firmware.
- 25 November 2026: Roxy's vulnerabilities and microcode, from Linux (Chris): Meltdown, L1TF
  and MDS affected, as the draft assumed; the firmware's microcode (`0x28`) can't clear
  buffers, Linux's update (`0x2F`) can. Sections 1c and the microcode question updated.
- 23 November 2026: first draft, for discussion with Chris.
