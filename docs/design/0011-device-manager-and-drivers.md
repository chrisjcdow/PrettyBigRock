# 0011: The device manager, drivers and their interfaces

Status: Accepted · 31 October 2026, built 2 November 2026 · Related: DESIGN.md section 8; notes 0006 to
0010, 0012; milestone M4

## The problem

Note 0010 lets a program touch one device and nothing else. Something has to find the devices,
decide which program drives each, give that program exactly its device's capabilities, and
give everyone else a way to reach the driver. This note settles:
- what reads the hardware description, and how much of it M4 reads;
- how PCI Express devices are found, and the legacy devices that are not on PCI;
- how a driver is matched, packaged and started, and what it starts with;
- the shared driver framework;
- the interfaces drivers offer (serial, input, display, block);
- which drivers M4 contains.

Crashing and restarting is in note 0012.

## The design

### 1. The parts

```
 root ──starts──▶ devmgr ──scans──▶ PCI Express, legacy devices
                    │
                    ├──starts──▶ drivers/virtio-blk  ──▶ pbr.block
                    ├──starts──▶ drivers/virtio-input ─▶ pbr.input
                    ├──starts──▶ drivers/ps2          ─▶ pbr.input
                    ├──starts──▶ drivers/serial16550  ─▶ pbr.serial
                    └──starts──▶ drivers/framebuffer  ─▶ pbr.display
 clients ──ask──▶ devmgr (pbr.devices) ──hands out──▶ a driver's endpoint
```

- **`services/devmgr`** is the **device manager**: an ordinary program (GPL-3.0-or-later). The
  root program starts it, giving it the start-up archive and everything note 0010, section 8
  lists. It finds devices, starts and watches drivers, and keeps the list of what is running.
  The root program stays small and knows no hardware.
- **Drivers** are programs in the start-up archive, one per kind of device, each started with
  only its device's capabilities.
- **Clients** (the file layer in M5, the input and display services in M6) ask the device
  manager for a device by class, and get an endpoint to talk to the driver directly. The
  device manager is not in the data path.

### 2. The machine description

The kernel already reads the ACPI tables it needs (note 0005). It passes the root program a
read-only page, the **machine description** (start-up role 11), so the device manager need not
find things again:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | Magic `"PBRMACH\0"` |
| 8 | 2 | Version, major (1) |
| 10 | 2 | Version, minor (0) |
| 12 | 4 | Flags: bit 0 the legacy PS/2 controller is present (x86-64, from ACPI's FADT), bit 1 the message-signalled interrupt pool is present |
| 16 | 8 | Physical address of the ACPI root pointer, or 0 |
| 24 | 8 | Physical address of the device tree, or 0 |
| 32 | 8 | Length of the device tree |
| 40 | 4 | The number of interrupt numbers the interrupt controllers have |
| 44 | 4 | The number of framebuffers (0 to 8) |
| 48 | 16 | Reserved, 0 |
| 64 | | The framebuffers, each a copy of the boot information's `Framebuffer` record (note 0003) |

The page is versioned like the others; later versions only add fields after the end.

The ACPI tables themselves are in memory the kernel doesn't manage, so the device manager reads
them through the device memory capability. The ACPI parsing code is in `lib/platform` already,
host-tested; it gains the **MCFG** table (where PCI Express configuration space is). **ACPI's
bytecode (AML) is not interpreted in M4**; that is its own project in M7 (DESIGN.md section 11).

**Device trees.** `lib/platform/src/fdt.rs` can already read them, but no M4 device is described
only by one: QEMU's `virt` machine boots through UEFI with ACPI, and every device M4 drives is
on PCI Express. The device manager reads device trees from M10 (Raspberry Pi 5), where it
becomes necessary. ROADMAP.md's M4 line is adjusted to say so.

### 3. Finding PCI Express devices

The device manager:
1. reads the MCFG table, which lists the **configuration space** regions (one per segment
   group and bus range). QEMU's q35 and `virt` machines have one; so do essentially all PCs
   since 2007. A machine without one is not supported until a machine needs it (the older
   port-based access would be a small addition);
2. slices the ECAM region (1 MiB per bus) from its device memory capability and maps it one bus
   at a time;
3. walks the buses, following bridges (using the bus numbers the firmware assigned), and for
   each function reads its identity (vendor, device, class, sub-class, programming interface,
   revision), its **BARs** (sizing each by the usual write-all-ones procedure, restoring it
   afterwards), and its capability list (MSI, MSI-X, PCI Express, and the vendor-specific
   capabilities virtio uses);
4. **uses the resources the firmware assigned**. It never moves a BAR. A function whose BARs
   are unassigned is skipped with a log line. (Reassigning resources is a project for the
   machines that need it: M7 and M11.)

There is no hot-plug in M4: devices are found once at start-up.

**Distrusting the bus.** Configuration space is read from hardware, so garbage is possible:
a capability list that loops, an offset past the end, a BAR size that wraps. The parsing is
therefore in a library, `lib/devices` (`pbr-devices`, no_std), which never indexes, loops or
adds without checking, and is tested on the host with synthetic and recorded configuration
spaces and random corruption of them (note 0002, testing). A function that cannot be parsed is
reported and skipped; it never stops the scan.

### 4. Legacy devices that are not on PCI

PS/2 and the second serial port are not on PCI Express and are described only in AML. Until M7
the device manager has a **short built-in list** for x86-64:
- **COM2:** ports `0x2F8` to `0x2FF`, interrupt 3. Present if a loopback self-check on its
  registers works. (COM1 is the kernel's, note 0010, section 1.)
- **The PS/2 controller:** ports `0x60` and `0x64`, interrupts 1 (keyboard) and 12 (mouse).
  Present only if the FADT's boot flags say an 8042 exists (the machine description's flag).

Each is checked before a driver is started, so a machine without them just doesn't get them.
M7 replaces this list with what AML says. On AArch64 there is no built-in list.

### 5. Matching and starting drivers

**Packaging.** A driver is a program in the start-up archive named `drivers/<name>`, with a
text file **`drivers/<name>.drv`** beside it (the archive's names are at most 47 bytes). The
manifest has one `key value…` line each, ASCII, at most 1 KiB, `#` starting a comment:

```
# drivers/virtio-blk.drv
class block
match pci 1af4:1001
match pci 1af4:1042
budget 8M
dma 1M
irqs 1
```

| Key | Meaning |
|-----|---------|
| `class` | The class of interface it offers: `block`, `input`, `serial`, `display` |
| `match pci VENDOR:DEVICE` / `match pci class CLASS:SUBCLASS[:PROGIF]` | Which PCI functions it drives (hex). Any number of lines |
| `match legacy NAME` | Which built-in legacy device (`com2`, `ps2`, `framebuffer`) |
| `budget SIZE` | The memory budget for the process (suffixes `K`, `M`) |
| `dma SIZE` | The size of its DMA memory (`0` if none; at most 4 MiB) |
| `irqs N` | How many message-signalled interrupts it wants (`0` if none) |
| `args TEXT` | Its start-up arguments |

The parser lives in `lib/devices` with the PCI code, and is tested with the usual ordinary and
hostile inputs: unknown keys, repeated keys, bad numbers, too-long lines and values that would
overflow are all refused and the driver is skipped and logged. Manifests are small text so they
can be read, edited and checked in by hand; they become the model for application manifests
(DESIGN.md section 7.1).

**Matching.** For each device found, the first driver whose `match` lines fit it (exact IDs
before class matches) is started. Each device has at most one driver. A device with no driver
is listed as unclaimed.

**Starting.** The device manager creates a process with the manifest's budget plus nothing
else: the driver's image is charged to the device manager (note 0009, section 2). It loads the
program with `granite`'s loader, as the root program does, then gives it the capabilities in
section 6 and starts it.

### 6. What a driver starts with

Roles from 1000 are launcher-specific (note 0009, section 4). The device manager's:

| Role | What the handle is |
|------|-------------------|
| 1 to 3, 5 | As for every program (note 0009); role 5 is an endpoint to the device manager, with the driver's badge |
| 1000 | The **device record** (a read-only memory object, below) |
| 1001 | The device's configuration space: its function's 4 KiB of ECAM (PCI only) |
| 1010 + *i* | BAR *i* as a device memory slice (rights `READ`, `WRITE`, `MAP`; no `DMA`, no `INSPECT`, no `DUPLICATE`) |
| 1020 + *i* | Message-signalled interrupt *i* (`BIND`, `ACKNOWLEDGE`) |
| 1030 | The driver's **DMA memory**, if the manifest asks for any (a copy without `BUS`: the driver is told the bus address, below, and the memory is contiguous, so a bus address is that address plus an offset) |
| 1040 | I/O ports, for legacy devices (a slice) |
| 1041 + *i* | An interrupt number for a legacy device |
| 1050 | The kernel log with `CONTROL`, for the display driver only |

The **device record** says what these are, so the driver needs no table of its own:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | Magic `"PBRD"` |
| 4 | 2 | Version (1) |
| 6 | 2 | Kind: 1 PCI, 2 legacy, 3 framebuffer |
| 8 | 2 | Vendor ID (PCI) |
| 10 | 2 | Device ID |
| 12 | 4 | Class, sub-class, programming interface and revision, one byte each |
| 16 | 1 | The number of BARs given (0 to 6) |
| 17 | 1 | The number of interrupts given |
| 18 | 6 | Reserved, 0 |
| 24 | 8 × 6 | For each BAR: its size in bytes (0 if not given) |
| 72 | 16 × 8 | For each interrupt: the address (8 bytes) and data (4 bytes) to program into the device, 4 bytes padding. A legacy interrupt has address 0 and its number as data |
| 200 | 8 | The DMA memory's bus address (0 if none), and its size at 208 |
| 216 | 16 | For kind 3, the framebuffer: width, height, stride (`u32` each) and format (`u32`, as `pbr.display`'s `PixelFormat`) |

**DMA memory belongs to the device manager.** It creates the driver's DMA memory in its own
budget, and gives the driver a *copy*. That keeps the memory alive when the driver dies, until
the device has been stopped (note 0012, section 2). The driver tells the device addresses
inside it, and nothing else.

A driver learns about the world only through these. It has no way to find any other device,
nor any other program but the device manager.

### 7. The driver framework

`lib/driver` (`pbr-driver`, GPL-3.0-or-later; drivers are system code, not application-facing
library code, so it is not MPL) is what every driver is built on:
- **Start-up:** reads the start-up block and the device record into a typed `Device`; reports
  anything missing before doing anything.
- **Registers:** `Mmio`, a mapped register block with bounds-checked, volatile `read` and `write`
  (8, 16, 32 and 64 bits) and no other way to touch the memory. Both it and `Dma` implement
  `lib/devices`' `Registers` trait, so code that knows a device's protocol (virtio, MSI-X) is
  written against the trait and tested on the host against a fake (`regs::Heap`). The bounds
  rule itself (`regs::access_ok`) is in `lib/devices` too, so it is host-tested. I/O ports are
  `granite`'s `IoPorts` (one system call per access); they needed no wrapper of their own. This
  confines the `unsafe` code to one small place.
- **DMA:** `Dma`, the driver's DMA memory behind the same trait, with its bus address. The
  first-built drivers needed no allocator inside it: `lib/virtio` lays out its queues and
  request buffers at fixed, aligned offsets.
- **The event loop:** `serve` waits on the driver's endpoint (with its interrupts bound to a
  notification bound to the endpoint, note 0010, section 4) and calls the driver's handler for
  each request or interrupt. It sends the endpoint to the device manager when the driver says
  it's ready (`host_ready`, `pbr.driver.host.ready`).
- **Rules, as code where possible:**
  - a driver **initialises its device from reset** and never assumes what state the device is in
    when it starts, because it may be starting again after a crash (note 0012);
  - a driver **validates everything a device tells it**: lengths, indices and addresses a device
    writes are hostile input, and an impossible value is an error, not a crash or an overrun;
  - a driver keeps **no state that it cannot rebuild**.

Drivers for virtio devices share a second library, `lib/virtio` (`pbr-virtio`):
- the modern PCI transport (finding the common, notify, interrupt and device configuration
  structures through the vendor capabilities; feature negotiation, requiring
  `VIRTIO_F_VERSION_1`; the status handshake);
- split virtqueues: descriptor chains, the available and used rings, and notifications;
- every index or length read back from the device is checked, and an impossible one fails the
  request.

QEMU's virtio PCI devices are "transitional" (they show up under old and new IDs and offer
both interfaces); the driver uses only the modern one. A device without it is skipped.
`lib/virtio` is tested on the host with a software virtio *device* model that writes into the
rings, including a device that lies (lengths too big, indices out of range, a used entry for
a request that was never made).

### 8. The interfaces drivers offer

All are `.pbi` files (note 0008) in `interfaces/`, MIT-licensed like the others. The first
two belong to the device manager and are specified in full here; the class interfaces are
specified as far as M4 builds them.

```
/// What a driver says to its device manager.
interface pbr.driver.host version 1;

/// The driver is ready: here is the endpoint clients should use.
@1 fn ready(service: handle<any>) -> () error HostError;

flexible enum HostError { BadState @1 }
```

```
/// Finding devices.
interface pbr.devices version 1;

enum DeviceClass { Block @1, Input @2, Serial @3, Display @4 }
enum DeviceState { Starting @1, Running @2, Restarting @3, Disabled @4 }

struct DeviceInfo {
    id: u32,
    kind: DeviceClass,
    state: DeviceState,
    name: string<48>,
    restarts: u32,
}

/// The devices that have a driver, or had one.
@1 fn list() -> (devices: list<DeviceInfo, 64>) error DevicesError;

/// An endpoint for the driver of the device with this id, to be used through the
/// interface for its class. `NotReady` while the driver is starting or restarting:
/// ask again.
@2 fn open(id: u32) -> (service: handle<any>) error DevicesError;

flexible enum DevicesError { NoSuchDevice @1, NotReady @2, Disabled @3 }
```

`handle<any>` is an endpoint capability with `SEND` whose interface the client knows from the
device's class. Typed variants can come once interfaces can import each other (note 0008, open
questions).

**`pbr.serial`** (the second serial port, x86-64):

```
interface pbr.serial version 1;

/// Sends bytes. May accept fewer than offered; the answer says how many.
@1 fn write(data: bytes<4096>) -> (written: u32) error SerialError;
/// Takes the bytes received so far, up to `max`. Never waits.
@2 fn read(max: u32) -> (data: bytes<4096>) error SerialError;
/// A notification that gets bit 0 set whenever bytes are waiting.
@3 fn events() -> (readable: handle<notification>) error SerialError;

flexible enum SerialError { Overrun @1, BadArgument @2 }
```

**`pbr.input`** (keyboards and mice): events travel in a ring (note 0008, section 6), the
first real use of one. The ring is created by the driver, holds fixed 16-byte entries, and has
one consumer at a time.

```
interface pbr.input version 1;

enum InputKind { Keyboard @1, Mouse @2 }

/// What this device is.
@1 fn info() -> (kind: InputKind, name: string<64>) error InputError;
/// Opens the event ring for reading: the ring, and a notification that gets bit 0 set when
/// the ring goes from empty to not empty. Only one opener at a time.
@2 fn open() -> (ring: handle<memory>, doorbell: handle<notification>) error InputError;

flexible enum InputError { AlreadyOpen @1 }
```

An event is 16 bytes, little-endian: `kind: u16` (1 key, 2 relative movement, 3 button, 4 scroll),
`code: u16` (a key's code, or the axis, or the button), `value: i32` (1 pressed, 0 released,
2 repeated for keys; the amount for movement), and `time: u64` (the monotonic clock in
nanoseconds). Key codes are PrettyBigRock's own table (positions on the keyboard, not
characters; layouts are the input service's job in M6), defined in the interface file as
constants. If the ring is full the driver drops the oldest events and counts the loss, and
the next event after a loss carries kind 0 and the count; a consumer that is slow loses
input, but the driver never waits for it.

**`pbr.display`** (the firmware's framebuffer):

```
interface pbr.display version 1;

/// Four bytes per pixel, eight bits per colour, the fourth byte unused.
enum PixelFormat { Bgrx @1, Rgbx @2 }
struct DisplayInfo { width: u32, height: u32, stride: u32, format: PixelFormat }

@1 fn info() -> (info: DisplayInfo) error DisplayError;
/// The framebuffer's memory, to be mapped write-combining.
@2 fn framebuffer() -> (memory: handle<device_memory>) error DisplayError;

flexible enum DisplayError { NoScreen @1 }
```

This needs a new kind of handle in the language, `device_memory`, next to `memory` and
`notification` (note 0008, section 7's `hkind`). It's a compatible addition (note 0008,
"Compatibility"): the compiler and `tools/pbi_check.py` both learn it, and the check
that they agree on every layout covers it. Several screens (DESIGN.md section 8) are one
`DisplayInfo` each from several devices; the display driver is the first and simplest.

**`pbr.block`** (disks): in M4, calls only. A disk's driver offers the simple path, with the
data in the attached buffer (up to 64 KiB, so up to 128 sectors of 512 bytes at a time):

```
interface pbr.block version 1;

struct BlockInfo { sector_size: u32, sector_count: u64, read_only: bool }

@1 fn info() -> (info: BlockInfo) error BlockError;
@2 fn read(sector: u64, count: u32) -> (data: bytes<65536>) error BlockError;
@3 fn write(sector: u64, data: bytes<65536>) -> () error BlockError;
@4 fn flush() -> () error BlockError;

flexible enum BlockError { OutOfRange @1, ReadOnly @2, DeviceError @3, BadArgument @4 }
```

Reads and writes are **idempotent** (doing one twice has the same effect as once), which is why
a client may simply retry after a driver restart (note 0012, section 3). A ring-based path for
speed, with requests and completions in shared memory, comes with the file layer in M5, which
will say what it needs from it; that needs the driver to read and write the client's memory
without a copy, which in turn needs the scatter-gather work of note 0010's open questions. M4
copies through the driver's own DMA memory.

### 9. The drivers in M4

| Driver | x86-64 | AArch64 | Notes |
|--------|:------:|:-------:|-------|
| `virtio-blk` | yes | yes | virtio disk, over `lib/virtio` |
| `virtio-input` | yes | yes | virtio keyboard and mouse |
| `framebuffer` | yes | yes | the boot framebuffer (UEFI's, or the BIOS loader's) |
| `ps2` | yes | | keyboard and mouse on the 8042 controller |
| `serial16550` | yes | | COM2 |

There is no AArch64 serial driver in M4. The `virt` machine has only the one UART, and it is
the kernel's console (note 0010, section 7), so there is no second port to test one on. The
Raspberry Pi 5 has several (M10), which is when it's written and tested. ROADMAP.md's M4 line is
adjusted.

An AHCI (SATA) driver for the x86-64 test machine's disk, USB, network and graphics drivers
are later milestones (M7 to M11).

## Alternatives considered

- **Drivers started by the root program.** It would have to learn about PCI, manifests and
  restarting. A separate device manager keeps the root program a launcher (note 0009).
- **A driver per bus, scanning for its own devices.** Then every driver repeats the scan, and
  two drivers could claim one device. One scanner that hands out exactly one device's
  capabilities is easier to reason about and to restrict.
- **Matching tables inside the device manager** instead of manifests. Adding a driver would
  then mean changing a program that every driver depends on. With manifests, a driver is one
  program and one small text file.
- **Giving drivers a way to ask for more devices.** Not needed, and it would let a compromised
  driver reach for its neighbours. A driver gets its capabilities at start and nothing else.
- **A binary manifest, or a section in the ELF file.** More to parse in more places, and
  harder to read when something goes wrong.
- **Ring-based block I/O from the start.** It would be designed before the file layer has said
  what it needs.
- **Keyboard layouts and key repeat in the input driver.** They belong in the input service
  (M6); the driver reports positions and presses.

## Open questions

- AML, to replace the legacy device list and find devices PCI doesn't (M7).
- Hot-plug and removal of devices (USB, M7; PCI Express hot-plug later).
- Reassigning PCI resources when the firmware didn't (M7, M11).
- Whether devices should have stable names across boots (a serial number or path), for when
  there are several disks (M5).
- Typed `handle<…>` results from `pbr.devices.open`, with interface imports.

## Changes
- 31 October 2026: first version, draft for approval; approved by Chris the same day.
- 2 November 2026: built (M4 stages 2 and 4).
  - The boot framebuffer is the third built-in legacy device (`match legacy framebuffer`, device
    record kind 3), on both architectures.
  - No DMA arena (section 7): `lib/virtio` places its queues and buffers itself. An arena comes
    with the first driver that needs one.
  - No `Ports` wrapper: drivers use `granite::device::IoPorts`. The register bounds rule moved to
    `lib/devices` (`regs::access_ok`) so it could be tested on the host, since `lib/driver`
    builds only for PrettyBigRock.
  - The interface compiler gained the `device_memory` handle kind (note 0008).
  - Drivers take one to two seconds from start to ready in a debug build on QEMU's emulation;
    nothing in M4 waits on that, but it's worth measuring in a release build before M6.
