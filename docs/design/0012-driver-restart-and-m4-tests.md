# 0012: Crashing and restarting drivers, and how M4 is tested

Status: Accepted · 31 October 2026, built 2 November 2026 · Related: DESIGN.md principle 6 and section 8;
notes 0006 to 0011; milestone M4

## The problem

M4 is "done when a driver can be deliberately crashed and is restarted while the system keeps
running" (ROADMAP.md). A driver crash is not like an application crash: the driver was in the
middle of talking to hardware. A restart has to deal with:
- a device that may be in any state, and may still be running (even writing to memory by DMA);
- an interrupt that may be stuck on;
- clients that were in the middle of a request;
- a driver that crashes again every time it starts.

This note settles what the device manager does, what clients see, and what the tests prove.

## The design

### 1. What counts as a crash

The device manager asks the kernel to tell it when each driver's process ends
(`process_watch`, note 0007), through one notification bound to its own endpoint (so it handles
requests and crashes in one loop). A process ends when a thread faults, when the driver exits,
or when something with the `STOP` right stops it (note 0006, section 2). All of these are
treated as a crash, whatever the exit status, because a driver is not supposed to end.

**A driver that hangs is not detected in M4.** Detecting it needs a liveness check (the device
manager asking the driver "are you there" at intervals, or the driver promising to answer within
a deadline), and a policy for when to give up on it. It is an open question, below.

### 2. What the device manager does when a driver ends

In this order:

1. **Record it.** Read the process record (`process_info`: the kind of fault, the address, the
   faulting instruction) and log it, with the driver's name.
2. **Revoke.** Revoke every capability it derived for the driver (`handle_revoke`, on the
   slices and interrupts it kept the originals of). Closing the dead driver's own table already
   removes its copies; revoking removes any it may have passed on, and the kernel has by now
   unbound its interrupts and masked any level-triggered one (note 0010, section 4).
3. **Stop the device.** The device manager still holds the device's configuration space. For a
   PCI function it clears the Command register's bus-master, memory and I/O enable bits, and
   the MSI and MSI-X enable bits in the capabilities, which stops the device writing to
   memory or raising interrupts. For a legacy device it writes the device's own quiet state
   (it knows the two: the UART's interrupt enable register, the 8042's disable-port commands).
   After this, nothing is in flight. (As built, a legacy device is not written to: its
   interrupt line is already masked, since the kernel masks a released level interrupt, and
   the next instance resets the device itself. The UART and 8042 quiet states can be added if
   a device ever misbehaves between instances.)
4. **Keep the DMA memory.** The driver's DMA memory is still held by the device manager (note
   0011, section 6), so a device that was writing into it when the driver died wrote into
   memory that still belonged to its device, never to anyone else's. It is **zeroed** now and
   **reused** by the next instance. It is never given back to the page allocator while the
   device might be running.
5. **Decide whether to restart** (section 4).
6. **Start the new instance** with the same budget, capabilities and DMA memory, and the
   same interrupt vectors (the device manager holds them; the device was switched off before
   the restart, so nothing is still in flight to them). The new
   instance finds the device switched off, and initialises it from reset like any start (note
   0011, section 7).
7. **Update the registry.** The device's state goes `Restarting`, then `Running` once the new
   driver says it is ready, and its restart count goes up.

If the new instance can't be started (no memory, a missing file), the device is `Disabled` and
the reason logged.

A function-level reset (the PCI Express FLR) would be a stronger version of step 3 for devices
that honour it. M4 relies on switching the device off and on the driver's own reset (for virtio,
writing 0 to the status register), which is enough for every device M4 drives.

### 3. What clients see

- **A client with an open session** (an endpoint to the driver) sees its calls fail with
  `PEER_GONE` (note 0008, section 2) the moment the driver ends. This is the kernel's behaviour
  and was tested in M3.
- **A client asks the device manager again** with `pbr.devices.open`. While the driver is
  starting, it gets `NotReady`, and tries again after a short delay. The client library offers
  `reconnect` which does exactly that, with a limit on the total wait. A `Disabled` device
  answers `Disabled`.
- **Nothing is replayed.** A request in flight when the driver died has failed; the client decides
  whether to repeat it. Each class interface says which of its calls are safe to repeat. Block
  reads and writes and flush are (they do the same thing if done twice). A write to a serial
  port might be sent twice or not at all. An input ring is gone with the driver, and the new
  one starts empty (some key presses may be lost).
- **No state survives in the driver.** The driver has no state worth keeping that cannot be
  rebuilt from the device (note 0011, section 7). Things a client set (such as a ring it opened)
  have to be set up again after `reconnect`.

### 4. When to give up

A driver that crashes every time it starts must not make the machine spin. The device manager
restarts a driver with a delay that grows, and counts the crashes in a window:

| Crash number within 30 seconds | Delay before restarting |
|:------------------------------:|-------------------------|
| 1 | at once |
| 2 | 100 ms |
| 3 | 1 s |
| 4 | none: the device is `Disabled`; no further restarts |

A driver that has run for 30 seconds since its last start has its count reset to zero.
`Disabled` is reported by `pbr.devices.list`, and logged once. Only a new device manager (that
is, a restart of the system, for now) enables it again; a way to re-enable it on request comes
with the tools that would use it.

All of the other drivers keep running throughout. A crash in one driver changes nothing for
another driver, since they share no capabilities.

### 5. If the device manager itself ends

The root program starts the device manager, and the drivers are its children. In M4 the root
program treats the device manager ending as **fatal to the device subsystem**: it logs the
fault record, and in self-test runs it fails the test. It does not restart it, because the
drivers it started are still running with their devices, and a new device manager would start
duplicates of them. Stopping all of a dead parent's children automatically needs a kernel
facility (a process flag that says "stop me when my creator ends") that doesn't exist yet. This
is the first open question below; it is the right problem to solve with the real design of
supervision for all services in M5, when there is more than one supervisor.

### 6. Fault injection for the tests

To crash a real driver, driver binaries need a way to be told to. When a driver's start-up
arguments contain `selftest`, the framework also serves a second interface on the driver's
endpoint, `pbr.driver.debug`:

```
/// Serves only when started with `selftest`. Crashes the driver on purpose.
interface pbr.driver.debug version 1;

enum CrashKind { Exit @1, NullWrite @2, IllegalInstruction @3, DivideByZero @4 }

@1 fn crash(kind: CrashKind) -> ();
```

The driver never replies (it is gone). The four kinds cover the ways a process ends: `Exit`
calls `process_exit`, `NullWrite` writes to address 0, `IllegalInstruction` executes an
undefined instruction, `DivideByZero` divides by zero. Without `selftest` the interface
is not served: a normal run has no way to crash a driver from outside, other than the `STOP`
right that only the device manager holds.

The device manager passes `selftest` to drivers only if it was started with it (note 0009:
the root program is given the kernel's command line). In self-test runs the device manager also
serves a small interface, `pbr.devices.debug` with `stop(id)`, which stops a driver from outside
through its `STOP` right, to test that path separately from the driver's own faults, and
`forgive(id)`, which clears a device's crash history so that the 20 cycles of the leak test
don't run into the storm limit.

On AArch64, integer division by zero gives 0 rather than faulting, so `DivideByZero` executes a
breakpoint (`brk`) instead, which ends the process the same way.

## The tests

Small, but covering each decision. The long runs go to GitHub Actions as before.

### On the host (ordinary and hostile inputs)

- `lib/ranges`: slicing, kernel-owned subtraction, message-signalled vector queue, against a
  simple model with random operations.
- `lib/devices`: configuration-space parsing (BARs 32 and 64-bit, memory and port, sizing with
  odd values; capability lists that loop, run off the end or point backwards; bridges with bad
  bus numbers), against synthetic spaces and corruption of them; manifest parsing, including
  every refusal. Matching order.
- `lib/virtio`: the transport and virtqueues against a software device, including a lying
  device (note 0011, section 7).
- `lib/devices`' `regs::access_ok`, the bounds and alignment rule `lib/driver`'s register
  wrappers use (`lib/driver` itself builds only for PrettyBigRock), and the host fake refusing
  exactly what it refuses. (There is no DMA arena in M4; note 0011, section 7.)
- The interface compiler: the new `device_memory` handle kind, the new interface files, and
  `tools/pbi_check.py` agreeing on every layout.

### In the kernel and the program self-test (QEMU, both architectures, 4 cores)

1. **Hardware misuse** (the existing test program `tests/programs/probe`, extended, run by the
   root program). Each
   fails as it should:
   - a slice outside its parent, or of zero length, or unaligned (`INVALID_ARGUMENT`);
   - a slice with more rights than its parent, or from a capability without `DUPLICATE`
     (`NO_RIGHT` or masked down);
   - mapping device memory executable, or with a bad cache mode (`INVALID_ARGUMENT`);
   - slicing device memory over RAM, the kernel image, the interrupt controller or the HPET
     (`INVALID_ARGUMENT`), and binding a kernel-owned interrupt or slicing a kernel-owned port;
   - using a mapping after its capability was revoked (the access faults and ends the
     program, not the system);
   - I/O ports outside the range or the wrong width (`INVALID_ARGUMENT`), and ports on AArch64
     (`NOT_SUPPORTED`);
   - binding a bound interrupt (`ALREADY_EXISTS`), or without `BIND` (`NO_RIGHT`);
   - `memory_create_dma` without `DMA`, over 4 MiB, and `BELOW_4G` really below 4 GiB.
2. **Interrupts without hardware** (with the self-test-only `interrupt_trigger`, note 0010,
   section 9): bind, trigger, see the notification bits arrive, see that the line stays masked
   until acknowledged, trigger again, unbind; a program that ends while holding a bound level
   interrupt leaves it masked and free for the next; message-signalled vectors come back in
   queue order; the same on every core.
3. **The scan.** The device manager finds exactly the devices the QEMU configuration has
   (virtio disk, keyboard, mouse; on x86-64 also the PS/2 controller and COM2) and starts
   one driver for each, and none for the others (the boot disk's controller, the host bridge).
4. **Each driver, through its class interface:**
   - *virtio-blk:* reads every sector of a 64 KiB test disk (`build/test-data.img`, which the
     build tool generates: sector *n* holds a pattern worked out from *n*) and compares; writes a
     sector, reads it back, flushes (the disk is a throwaway copy); an out-of-range read gives
     `OutOfRange`; a read of 128 sectors at once works.
   - *serial16550 (x86-64):* writes a known string to COM2; the build tool's second serial
     port captures it into a file that the test runner compares; a byte sent in (through the
     loopback mode) comes back from `read`, and the readable notification fires.
   - *virtio-input, ps2:* the test runner injects a key press and release (and a mouse
     movement) into the named QEMU device through its control socket when the self-test says
     it is waiting; the events come out of the ring in order, with the right codes.
   - *framebuffer:* after `console_screen_release`, the test maps the framebuffer
     write-combining, draws a pattern and reads it back; and a kernel message printed
     afterwards doesn't change the pixels in the drawn area.
5. **The crash test: the "done when".** On *virtio-blk* (a real DMA and interrupt driver):
   - read all sectors, and keep the answers;
   - for each `CrashKind`, and for `pbr.devices.debug.stop`: crash the driver; check the old endpoint's
     calls fail with `PEER_GONE`; check `open` says `NotReady` or works; `reconnect`; read all
     sectors again and compare with the answers (so DMA and its interrupt work in the new
     instance); check the restart count went up by one;
   - **the system keeps running:** throughout, `virtio-input` and the other drivers answer
     their calls, and the kernel's self-test threads keep making progress;
   - **no leaks:** after 20 crash and restart cycles the device manager's budget use is within
     64 KiB of where it started (a program can't see the kernel's free page count; the device
     manager's budget is where a restart's memory is charged);
   - **the storm limit:** crashing the driver four times in quick succession makes it
     `Disabled`; the other drivers still answer; `open` says `Disabled`;
   - **the timing:** each restart completes within 5 seconds, and the slowest is reported (the
     planned limit was 2 seconds, but a debug build on QEMU's emulation on the 2-core workspace
     took up to 2.8 seconds; release builds on real hardware are the place to hold a tighter
     limit). The storm test also checks the delays: the restarts after the second and third
     crashes come no sooner than the backoff allows, and the fourth disables the device.
6. **Load across a crash.** Four client threads make block reads in a loop while the driver
   is crashed ten times. Every call either succeeds with the right data or fails with
   `PEER_GONE` or `NotReady` within its deadline; none hangs, none returns wrong data.

`cargo xtask qemu-test` runs all of this on all four boot configurations (x86-64 UEFI and BIOS,
AArch64 UEFI and EL2), adding to the QEMU command line: the test disk, `virtio-keyboard-pci`
and `virtio-mouse-pci` with names, a second serial port (x86-64), and a control socket the
runner talks to. It waits for `Device self-test passed` after `Program self-test passed`.

## Order of building

Each stage ends with everything passing on all four configurations, and a commit; I report
after each, since each is something that can be redirected.

1. **Device access in the kernel** (note 0010): `lib/ranges`, the three object types,
   the calls, the interrupt table, MSI on both architectures, the machine description and the
   new start-up roles, `hwprobe` and the interrupt tests. First thing to check: the ACPI MADT
   on QEMU `virt` does describe the GICv2m frame (the device tree does); if it doesn't, the
   frame's address can come from the device tree instead, and the note is updated.
2. **The device manager and the first driver:** `lib/devices`, `lib/driver`, `lib/virtio`,
   `services/devmgr`, `virtio-blk`, the block test.
3. **Crash and restart,** and the whole crash test. This is the milestone's "done when"; it is
   proved on one real driver before the others are written.
4. **The other drivers:** `virtio-input`, `framebuffer`, `ps2`, `serial16550`, and their tests.
5. **Documents:** notes 0001, 0002, 0005 to 0011 brought up to date with what building taught
   us, ROADMAP, DEVLOG, status.md, and the stress runs on GitHub.

Commits go on a branch `m4` and reach GitHub the way M3's did (a bundle, then Chris pushes),
unless GitHub is linked by then.

## Alternatives considered

- **Restarting a driver without stopping the device.** Quicker, but then a restarted driver
  would have to cope with a device in any state. Stopping it first is cheap and makes every
  restart start from the same place.
- **Letting the driver keep its DMA memory.** The driver would free it when it ended, while the
  device might still write. The device manager owning it is a small rule that removes the
  problem.
- **Unlimited restarts.** A driver with a bug at start-up would spin forever, and log forever.
- **A separate supervisor program for restarts.** More parts, with the device manager still
  needing to talk to it for every capability. The general supervision of all services comes
  in M5 when there is more than one kind.
- **Replaying requests transparently after a restart.** The only safe replay needs the driver
  to say which requests it had finished, which it cannot say after a crash. The client knows
  whether its request is repeatable.
- **Testing the crash only with an injected message.** It tests the restart but not the
  fault path. The four crash kinds and `stop` cover both.

## Open questions

- **A driver that hangs** (section 1): liveness checks, and what deadline to promise.
- **Stopping a dead parent's children** (section 5): a kernel process flag, to be decided with
  general supervision in M5.
- **Function-level reset** for devices that support it.
- **Re-enabling a `Disabled` device** without restarting the system.
- **Crash reports** kept somewhere that survives (the logging service, DESIGN.md section 8).

## Changes
- 31 October 2026: first version, draft for approval; approved by Chris the same day.
- 2 November 2026: built (M4 stages 3 and 4), and the tests pass on all four configurations.
  - Restarted drivers keep their interrupt vectors (section 2, step 6); legacy devices are not
    written to when quieted (step 3).
  - The restart limit is 5 seconds (section "The tests", 5), the leak check uses the device
    manager's budget, and `pbr.devices.debug` gained `forgive`.
  - `probe` gained the hardware misuse tests instead of a new `hwprobe` program.
  - Building it found two kernel problems, both fixed: a timed wait kept its timer (and with it
    the waiting thread, its process and the process's whole budget) until the deadline, so a
    dead driver's budget stayed charged for seconds and the device manager ran out after about
    six restarts; and emptied page tables were never freed, a slow leak in any long-lived
    program. Review afterwards found that the second fix freed the tables before the TLB flush
    that made them unreachable; they are now freed after it (note 0005).
  - A rare ARM kernel crash during the crash test (about one boot in ten to twenty) was a
    scheduler race that let two cores run one thread (note 0005), found and fixed in stage 5.
    The boot-test runner now asks QEMU for every core's registers and stack when the kernel
    stops or a run times out, since a kernel on a trampled stack can't finish its own
    report.
- 8 November 2026, M5 stage 7 (note 0018): the restart policy moved to `lib/supervise`, where
  `Disable` is now `GiveUp`; drivers are *tied* to the device manager (note 0013), so they end
  with it; switching a function off also switches MSI off (`pbr_devices::pci::quiet`), and a
  new device manager does it to every device it is about to drive before starting any driver.

