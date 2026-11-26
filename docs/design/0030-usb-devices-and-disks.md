# 0030: USB keyboards, mice and storage; SATA; disks that come and go

Status: Accepted · 26 November 2026, accepted 26 November 2026 · Related: DESIGN.md sections 7.1, 8 and 10; notes 0011,
0014, 0015, 0016, 0017, 0021, 0024, 0027, 0029; `docs/m7-plan.md` (Q4, Q5, Q6, Q14); milestone M7

## The problem

Note 0029 gives each USB interface to a class driver, with nothing but a `pbr.usb` capability.
M7 needs three kinds of device behind that, and one more on PCI:
- **keyboards and mice**, so Roxy can be used at all;
- **USB storage**, because Roxy boots from a stick and its system volume is on it (Q6), and
  because a stick plugged in later should be readable;
- **SATA disks** through Roxy's AHCI controller, which QEMU also has, so that the "foreign disk
  is left alone" test (Q5) has something real behind it.

Disks have so far been found once, at start. Now a disk can arrive late (the boot stick appears
only once USB is running), arrive while the system runs, or be pulled out. Disks other than the
boot disk must not be written to (Q5). And the system volume must be the one on the stick the
machine started from, not whichever RockFS volume happens to be listed first (Q6).

This note settles:
- `drivers/usb-hid` and the report descriptors it reads (Q4);
- `drivers/usb-storage`: the bulk-only transport and the SCSI commands a stick needs;
- `drivers/ahci`, and how one driver serves several disks;
- disks and volumes coming and going in the storage service, and how a volume is recognised;
- which disks may be written (Q5), and how the system volume is chosen (Q6);
- what a session sees of a stick, and what happens when one is pulled out (Q14).

## The design

### 1. The parts

```
 drivers/ehci ──pbr.usb──▶ drivers/usb-hid      ──pbr.input──▶ the input service
              └─pbr.usb──▶ drivers/usb-storage  ──pbr.block──▶ the storage service
 drivers/ahci (one per controller) ──pbr.block, one per disk──▶ the storage service
                                         │
              the users service ◀─pbr.storage─┘ ──/Volumes──▶ administrators' sessions
```

- **`lib/hid`** (`pbr-hid`, GPL-3.0-or-later, `no_std`): reads HID report descriptors and
  turns reports into key presses, movements and buttons. Host-tested.
- **`drivers/usb-hid`**, **`drivers/usb-storage`**: class drivers (note 0029, section 1), one
  process per interface, holding only their `pbr.usb` capability.
- **`drivers/ahci`**: a PCI driver, with DMA, serving every disk on its controller.

### 2. Keyboards and mice: `drivers/usb-hid`

USB keyboards and mice are **HID** devices ("human interface devices", interface class 3). Each
describes its reports (the few bytes it sends at each change) in a **report descriptor**: a
small language of items saying "the next 8 bits are the modifier keys, one bit each", "then
six bytes, each a key's usage code", "16 bits of X movement, signed". Keyboards and mice also
offer a simpler fixed format, the **boot protocol**, meant for firmware.

**Report descriptors, falling back to the boot protocol** (Q4). The boot protocol has no scroll
wheel and no absolute pointer, and QEMU's tablet, used by three test configurations, has only
report descriptors. So:
1. The driver reads the interface's HID descriptor (from `describe`, note 0029) for the report
   descriptor's length, at most 4 KiB, then fetches it (`GET_DESCRIPTOR`, type Report, to the
   interface).
2. `lib/hid` parses it into a list of **fields**: for each report (by report ID, if the device
   uses them), each field's position, size, count, whether it is an array (a list of codes
   pressed) or variable (one value per usage), its logical range and its usages. Limits: 16
   nested collections, 8 levels of `Push`, 256 fields, fields of at most 32 bits, and reports
   of at most 64 bytes; a descriptor beyond them, or one that doesn't parse, is refused.
3. From the fields the driver decides what the interface is: a **keyboard** (usages on the
   keyboard page, 7), a **mouse** (relative X and Y on the generic desktop page), a **tablet**
   (absolute X and Y), or keyboard and mouse together (some wireless receivers put both on one
   interface). That's `pbr.input`'s `InputKind` (note 0021).
4. It sends `SET_IDLE(0)` (report only on change; a device that refuses is fine) and starts
   waiting for reports with `interrupt_in`.

If the descriptor is refused and the interface says it supports the boot protocol (sub-class
1, protocol 1 keyboard or 2 mouse), the driver uses the boot protocol instead
(`SET_PROTOCOL(0)`) and the fixed boot layout, and logs that it did. Otherwise, or if the
interface is neither keyboard nor mouse (a game pad, a UPS), the driver **declines** it
(section 4's `decline`), and the device is listed as unclaimed.

**Turning reports into events** (`pbr.input`, note 0011, section 8, unchanged):
- **keys**: the driver keeps the set of keys down. Each report gives the new set (array fields
  list codes; variable fields, such as the modifier byte or a keyboard that reports every key
  as a bit, give one bit per key); the difference is posted as presses and releases. A report
  saying "too many keys" (every code `ErrorRollOver`) is ignored. Keyboard-page usages become
  PrettyBigRock key codes through a new table in `lib/devices`' `keys` (`from_hid`, beside
  `from_evdev`), checked against the interface file as the others are. A few consumer-page keys
  (volume, mute, play) are mapped where `pbr.input` has codes for them; others are ignored,
  logged once. Key repeat stays in the input service (note 0021);
- **buttons**: buttons 1 to 5 as left, right, middle, back and forward;
- **movement**: relative X and Y as motion; the wheel (generic desktop `Wheel`) as scroll;
  horizontal scroll (consumer `AC Pan`);
- **absolute X and Y** scaled from the field's logical range to 0 to 65535, as virtio tablets
  are (note 0021, section 7).

A value outside its field's logical range is ignored, as the HID specification says.

**Keyboard lights** (Caps Lock and Num Lock) stay off in M7: no driver sets them today, PS/2's
included, because `pbr.input` has no way for the input service to say Caps Lock is on. It's an
open question for both.

**Restarts.** When the driver starts again, or reconnects after the host driver restarted (note
0029, section 6), it releases every key and button it last saw down before reading reports
again, so nothing stays held.

### 3. USB storage: `drivers/usb-storage`

Sticks, card readers and USB disks are **mass storage** devices (interface class 8). Nearly all
use the **bulk-only transport** (protocol `0x50`) carrying **SCSI** commands (sub-class 6), and
that is all M7 supports. Each command goes as three steps on the interface's two bulk
endpoints: a 31-byte **command block** (signature `USBC`, a tag, the length and direction of the
data, the SCSI command), the data, and a 13-byte **status** (signature `USBS`, the same tag, how
much data was *not* moved, and passed, failed or "phase error").

**Starting.**
1. `GET_MAX_LUN` (a device that refuses it has one). M7 uses **logical unit 0** only: a card
   reader with several slots is several units, an open question.
2. `INQUIRY`: the device type must be 0 (a disk; a CD drive is declined), and the
   **removable** bit, vendor and product are kept for the log and `details` (below).
3. `TEST UNIT READY` until the unit is ready, for up to 10 seconds (some sticks and disks take a
   while), asking `REQUEST SENSE` after each failure.
4. `READ CAPACITY (10)`, or `READ CAPACITY (16)` if the answer says the disk is bigger than
   2 TiB. A sector size other than 512 bytes is declined, logged, since the storage service
   supports only 512 (note 0015).
5. `MODE SENSE (6)` for the write-protect bit (an SD card's slider, or a stick with a switch);
   a device that refuses is taken as writable.

Then it serves `pbr.block`: `READ (10)` and `WRITE (10)` (the 16-byte forms beyond 2 TiB), at
most 64 KiB per call as the interface allows, and `SYNCHRONIZE CACHE (10)` for `flush`. A device
that says it doesn't know that command (many sticks) has nothing cached to write, so `flush`
succeeds.

**Checking every answer.** A status with the wrong signature or tag, or a residue larger than
the transfer, counts as a phase error. A failed command is followed by `REQUEST SENSE`:
- "not ready" and "unit attention" (the medium changed, or the device was reset): retried after
  `TEST UNIT READY`, for up to 10 seconds;
- "data protect": `ReadOnly`;
- anything else, a medium or hardware error: `DeviceError`.

**Recovery.** A phase error, an endpoint that stalls on the command block, or a time limit
passing (30 seconds for a command; writes to cheap sticks can be slow) is followed by the
specification's **reset recovery**: the class's reset request to the interface, then
`clear_halt` on both endpoints. The command is then tried once more, and if that fails the
request fails with `DeviceError`. A stall in the data step needs only `clear_halt` on that
endpoint, and the status is read as usual.

**Gone.** If the device is unplugged, every request fails with a new `BlockError` value,
**`Gone`** (a compatible addition to `pbr.block`'s flexible enum), and the driver ends once the
device manager stops it.

**`pbr.block` gains one operation** (compatible: a new operation), answered by every disk
driver:
```
/// What the disk is, for people and for the storage service's rules (note 0030, section 6).
@5 fn details() -> (model: string<64>, serial: string<64>, removable: bool, usb: bool)
    error BlockError;
```
`virtio-blk` answers with its device ID string and `usb` false.

### 4. SATA disks: `drivers/ahci`

**AHCI** is how SATA controllers have presented themselves since about 2006: memory-mapped
registers (BAR 5), up to 32 **ports**, one disk each, and for each port a list of up to 32
commands in memory, which the controller works through by DMA. Roxy's is at `00:1f.2`
(`8086:1c02`); QEMU's q35 has the same family of controller at the same address, and
`ich9-ahci` can be added to `virt`.

**Starting** (from reset, note 0011, section 7):
1. If the controller supports the firmware hand-over (`CAP2` bit 0), take ownership through
   `BOHC`, waiting up to 2 seconds.
2. Set AHCI mode (`GHC.AE`), reset the controller (`GHC.HR`, up to 1 second), set AHCI mode
   again.
3. For each port the controller implements (`PI`): make sure its command engine is stopped,
   give it its command list and received-information area in the driver's DMA memory, start
   receiving, spin the disk up if the controller needs that, and look at the link (`PxSSTS`): a
   device that is present and talking is waited for until it is no longer busy (up to 10
   seconds, as a disk spins up). Its **signature** says what it is: an ATA disk is used; a CD
   drive (ATAPI) or a port multiplier is skipped and logged.
4. Start the port's command engine and send **`IDENTIFY DEVICE`**. From its 512 bytes: 48-bit
   addressing must be there, the sector count, the logical sector size (512 only, as for USB),
   and the model and serial number (byte-swapped in pairs, as ATA stores them). If the last
   word carries ATA's checksum signature, the checksum must be right.

**Commands**: `READ DMA EXT`, `WRITE DMA EXT` and `FLUSH CACHE EXT`, one at a time per port
(command slot 0), with one 64 KiB buffer per port in the driver's DMA memory, copied as
`virtio-blk` does. No native command queuing: one disk at a time is plenty for M7. Each
command has a 30-second time limit.

**Errors**: a task-file error, an interface error or a time limit stops the port's command
engine, clears its error registers and starts it again (with a link reset, `PxSCTL`, if the
disk stays busy); the request fails with `DeviceError`. Every value read back (a port number, a
byte count in the received area) is checked before use.

**Interrupts**: AHCI controllers have MSI (Roxy's and QEMU's do), so the manifest asks for one;
a controller without MSI gets its line through note 0028's routing, with nothing to change.

**DMA memory**: `dma 2M low` (note 0029's `low`): per port 1 KiB of command list, 256 bytes of
received information, a command table and the 64 KiB buffer, for up to 30 ports.

**One driver, several disks.** Until now one driver has meant one device in the device
manager's list. An AHCI controller has several disks, and each should be listed, opened and
recognised on its own. `pbr.driver.host` gains (beside note 0029's `reconnect`):
```
/// Before `ready`: another device this driver serves itself, listed with this driver as its
/// parent, reached through `service` (a capability carrying a badge of the driver's choosing).
/// `place` is appended to the driver's path ("port 2").
@3 fn child(class: DeviceClass, place: string<16>, product: string<64>, serial: string<64>,
            service: handle<any>) -> (id: u32) error HostError;
/// The device isn't one this driver can drive after all (a HID interface that is neither
/// keyboard nor mouse, a USB disk with 4096-byte sectors). It is listed as unclaimed, with the
/// reason in the log, and the driver isn't started for it again.
@4 fn decline(reason: string<64>) -> () error HostError;
```
(`DeviceClass` is `pbr.devices`' version 2 enum, copied, as interfaces can't import each
other yet.) The AHCI driver calls `child` once per disk, then `ready`. Its own entry has a new
class, **`Controller`** (a compatible addition to version 2's flexible enum), which `open`
answers `NotAllowed` for, like `UsbHost`. Calls go from the driver to the device manager only,
as `ready` does, so nothing waits in a circle.

If the AHCI driver restarts, the device manager keeps its children's entries and ids, marked
`Restarting`, and pairs the new driver's `child` calls with them by place and serial number, as
note 0029, section 6 does for USB. The storage service's calls fail meanwhile, and it asks
`open` again as it already does after a restart (note 0015, section 6). SATA hot-plug is not in
M7: disks are found when the driver starts.

### 5. Disks that come and go: the storage service

**Finding disks** (note 0015, section 1, changes): the storage service calls `pbr.devices`'
`watch` (note 0029, section 7) and looks at the list whenever signalled, instead of once at
start. A new `Block` device is opened, its partitions read and its volumes started, as at
start today. The 30-second wait at start goes; section 7 says how the users service knows when
the system volume should have appeared.

**A disk goes** (its device leaves the list, or its driver answers `Gone`):
1. Its volumes' state becomes **`Gone`**.
2. Each of their filesystem servers is told to stop (its stop notification, note 0015,
   Changes). The server tries to save; its writes fail with `Gone`, and it ends.
3. Anything for that disk still waiting in the cache is dropped, and the log says how much:
   "12 KiB written to USB couldn't be saved: the disk was removed."
4. Programs holding files on it find their calls failing with `Gone` (`pbr.file` already has
   it; `lib/files` reports a server that has ended as `Gone` too).
5. Once the servers have ended, the volumes leave the list.

A volume plugged in again is a new volume with a new id, even if it's the same stick.

**Recognising a volume.** Each volume has an **identity**, which stays the same across boots,
disks being moved between sockets, and the device manager restarting:
- on a GPT disk, its partition's unique GUID;
- otherwise its filesystem's own: RockFS's `uuid` (ROCKFS.md), or FAT's volume serial number
  with the disk's serial number, if it has one.

The identity is how the system volume is found (section 7), how a disk is matched to its
volumes again after the device manager restarts (replacing note 0015's "same number"), and
what later work (remembered permissions for a stick) can rely on.

**`pbr.storage` goes to version 2**, because its enums aren't flexible and `VolumeInfo` must
grow; the users service and the self-tests are its only clients, changed in the same stage:
```
interface pbr.storage version 2;

flexible enum Filesystem { RockFs @1, Fat32 @2, Empty @3, Unknown @4 }
flexible enum VolumeState { Ready @1, Starting @2, Unsupported @3, Damaged @4, Failed @5,
                            Gone @6 }
/// Why a volume can or can't be written (section 6).
flexible enum Access { Writable @1, OtherDisk @2, WriteProtected @3 }
flexible enum BootDisk { NotExpected @1, Waiting @2, Found @3, NotFound @4, TwoFound @5 }

struct VolumeInfo {
    /// Never reused while the storage service runs.
    id: u32,
    disk: u32, partition: u32, filesystem: Filesystem, state: VolumeState,
    label: string<64>, size: u64, in_memory: bool,
    access: Access,
    /// It is on the disk the machine started from.
    boot_disk: bool,
    /// The disk is a USB device.
    usb: bool,
    /// The volume's identity: `gpt GUID`, `rockfs UUID` or `fat SERIAL DISKSERIAL`.
    identity: string<80>,
    /// The disk's model, for people ("SanDisk Cruzer Blade").
    disk_name: string<64>,
}

@1 to @6: as version 1 (`list`, `open`, `create_ram_volume`, `format`, `shutdown`, `check`).
/// Signals `bits` on `note` whenever the list changes.
@7 fn watch(note: handle<notification>, bits: u64) -> () error StorageError;
/// Whether the disk the machine started from has been found (section 7).
@8 fn boot_disk() -> (state: BootDisk) error StorageError;
```
`read_only` gives way to `access`. `StorageError` gains `ReadOnly @9` for `format` and `setup`
on a disk that may not be written.

### 6. Which disks are written to (Q5)

**The rule for M7: a disk may be written if it is**
- **the disk the machine started from** (the stick, on Roxy);
- **the disk holding the system volume**, wherever it is (section 7: a development or Windows
  launcher's separate data disk);
- **a USB disk**: the stick or drive someone has plugged in on purpose.

**Every other disk is read-only**: on Roxy, its own SATA disks. "Things you plug in can be
written; disks inside the machine can't, unless the system lives there." Writing to them
becomes possible later only by an administrator's explicit choice, in a note of its own (Q5's
fourth layer). A device's own write protection (section 3) makes a disk read-only whatever the
rule says.

**How it is kept**, in the storage service, which is the only program that talks to disk
drivers (note 0015):
- a read-only disk's volumes are served to their filesystem servers read-only (their
  `pbr.block` refuses writes and `info` says read-only), and the servers are started with
  `readonly` (as they can be already);
- the storage service itself never writes to such a disk: no partition table, no `format`, no
  `setup` (both answer `ReadOnly`), and nothing from the cache, which holds no changes for it;
- a filesystem server mounted read-only **writes nothing**, not even "mounted" or "clean"
  marks: RockFS's server reads the last complete state of a volume that wasn't shut down
  cleanly without repairing it (its copy-on-write design means the last committed state is
  whole), and FAT's leaves its "dirty" flag alone. Note 0016's servers are checked for this
  (section 10).

The automated test (section 10) checks a foreign disk's checksum is unchanged after a boot that
lists and reads it. On Roxy, a spare disk Chris doesn't care about is tried only after that
test passes (Q5's third layer).

### 7. The system volume (Q6)

Note 0027 gives the boot information the **boot partition**: the unique GUID of the partition
the loader was read from (boot information 1.1; machine description 1.1, offset 48). The root
program passes it to the storage service as an argument, `boot-partition=GUID`, or nothing if
it's zero (a loader that couldn't tell, or QEMU's direct kernel boot).

- The **boot disk** is the GPT disk with a partition of that GUID. Volumes on it are marked
  `boot_disk`.
- The **system volume** is a RockFS volume with `System/users.txt` (note 0017, section 2),
  chosen in this order:
  1. on the boot disk (the stick image's third partition, note 0031);
  2. if the boot disk has none, or there is no boot partition: the first anywhere, as today.
     This keeps development with a separate data disk (QEMU, and the Windows launcher's
     `data.img`) working.

**Waiting for the boot disk.** The stick appears only once the EHCI driver, the hubs and
`usb-storage` are running, several seconds after the storage service starts. So the users
service asks `boot_disk` and, while it says `Waiting`, waits (signalled through `watch`) before
choosing. The storage service says `NotFound` after 60 seconds, or `NotExpected` when there is
no boot partition, and the users service then uses rule 2, or a volume in memory with the
message it shows today.

### 8. Sticks in a session, and pulling them out

**`/Volumes` becomes live.** Today an administrator's session gets each volume's root at
`/Volumes/LABEL` when it starts (note 0024), and a volume going restarts the session. With sticks
that would mean nothing plugged in after login appears, and pulling one out ends everyone's
work. Instead:
- the users service serves a small **`/Volumes` folder of its own** (`pbr.file`, note 0014): its
  `list` shows the volumes that are ready now, by label (a repeated label gets " 2"); `open` of
  `LABEL/rest/of/path` opens that volume's root and passes the rest of the path on, so what comes
  back is the filesystem server's own capability, read-only on a read-only volume; it can't be
  written to itself;
- each administrator's session gets that folder as one entry, `/Volumes`, in place of an entry
  per volume. Files, Terminal and the shell then see a stick as soon as it's ready, and the
  file picker can offer it;
- the users service opens volumes for each session through a copy of the root of its own
  (`reduce`), and revokes the copies when the session ends or the volume goes;
- **a data volume going no longer restarts the session**: its files fail with `Gone` in the
  apps that had them open, and Files shows it gone from `/Volumes`.

Standard users still see only their home, as today (note 0024); a stick for them is an open
question.

**Pulling out the system stick** (Q14). The system volume can't be replaced while running, and
the machine can't restart from a stick that isn't there. So:
- saving stops: the system volume is `Gone`, and apps' saves fail with `Gone`, telling the user
  as they tell any failure;
- `pbr.users` gains, for the desktop:
  ```
  /// Signals `bits` on `note` when the system's state changes.
  @14 fn watch_system(note: handle<notification>, bits: u64) -> () error UsersError;
  /// Whether the system volume is there.
  @15 fn system() -> (state: SystemState) error UsersError;
  flexible enum SystemState { Normal @1, InMemory @2, SystemDiskGone @3 }
  ```
  and the desktop shows, over everything: **"The disk PrettyBigRock started from was removed,
  so nothing more can be saved. Put it back, then restart."**, with **Restart**. Putting the
  stick back doesn't bring the volume back by itself: the cache's lost changes and the stick's
  state can't be trusted to fit together. RockFS's copy-on-write keeps the volume whole as of
  its last completed save (a FAT volume pulled out mid-write may not be, as on every system).

### 9. Not in M7

- **Keyboard lights** (section 2), for USB and PS/2 alike.
- **Card readers' other slots** (logical units after 0), CD and DVD drives (USB and SATA),
  **UAS** (USB Attached SCSI: such devices also offer bulk-only, which is used), disks with
  4096-byte sectors, SATA hot-plug, port multipliers, native command queuing.
- **Writing to internal disks** (section 6), and **sticks for standard users**.
- **Game pads, touch screens beyond a tablet, pens**: other HID devices are declined.
- **Telling an SSD which blocks are free** (TRIM; note 0015's open question).

### 10. Testing

**On the host** (`cargo xtask test`):
- `lib/hid`: recorded report descriptors from QEMU's keyboard, mouse and tablet, and from real
  devices (a boot keyboard, a mouse with a wheel and side buttons, a wireless receiver with both
  on one interface, an NKRO keyboard; Roxy's own, if Chris copies them, note 0029's open
  question), each with reports and the events they must give; and every descriptor randomly
  corrupted, which must parse or fail cleanly within the limits.
- The bulk-only transport and SCSI logic in `drivers/usb-storage` written against a small
  trait for `pbr.usb`, so it runs on the host against a simulated stick: one that lies about
  tags and residues, stalls, answers "unit attention", takes 8 seconds to become ready, and
  disappears mid-transfer.
- The AHCI start-up and command logic against `lib/devices`' fake registers (`regs::Heap`),
  with a fake controller answering `IDENTIFY` from recorded data, including a bad checksum and
  a 4096-byte-sector disk.
- `pbr.storage`'s rules (section 6) as a function from a disk's facts to its `Access`; the
  system volume choice (section 7) as a function from the volumes to the one chosen.
- The RockFS and FAT servers mounted read-only on a volume that wasn't shut down cleanly: the
  image's bytes are compared before and after.

**In QEMU** (note 0031 places each in a configuration):
- **AHCI**: a disk with partitions and files on q35's controller (and `ich9-ahci` on ARM64
  EL1, for its system disk, Q8). Its volumes are listed as `OtherDisk`, read in the self-test,
  and **the runner checks its checksum is unchanged after the boot**.
- **USB keyboard and tablet**: the desktop boot on the Roxy-like configuration has only USB ones
  (q35's PS/2 is there but given nothing), and the runner sends keys and pointer movements to
  them by name (QMP's `input-send-event` with `device`).
- **USB storage**: the Roxy-like configuration boots from a USB stick on EHCI, with its system
  volume on the stick, chosen through the boot partition (section 7).
- **Coming and going**: the runner plugs a stick with a FAT32 volume in during the self-test
  (`device_add`); it appears in `/Volumes`, is read and written (it's USB), and is pulled out
  (`device_del`) during a write: the volume goes, the write fails with `Gone`, nothing else
  is disturbed. The keyboard is pulled and plugged back, and typing carries on.
- **The system stick pulled out**: once, on the Roxy-like configuration's desktop boot, the
  runner removes the boot stick and checks the desktop shows section 8's message.

**On Roxy** (stages 5 and 6): the keyboard and mouse; booting the desktop from the stick with
its system volume; a second stick plugged in, read, written and pulled out; then, once the
automated test has passed, a spare SATA disk, checksummed on another machine before and after.

### 11. Who builds it

Claude Opus writes the storage service's changes (sections 5 to 7), the users service's
`/Volumes` and system state (section 8), the device manager's `child` and `decline`, and
`drivers/ahci` (it has DMA, and its correctness is what keeps other disks safe). Sonnet writes
`lib/hid`, `drivers/usb-hid` and `drivers/usb-storage` from sections 2 and 3, following
`drivers/virtio-input` and `drivers/virtio-blk`, with their host tests; Opus reviews them.

## Alternatives considered

- **Boot protocol only** for keyboards and mice: simpler, but no wheel, no tablet (so not
  QEMU's), and some keyboards' media keys lost (Q4).
- **Every disk writable**, trusting the storage service never to write by mistake: Q5 asks for
  more than that on someone else's machine. **Every disk but the boot disk read-only**: then a
  stick plugged in couldn't be saved to, which is most of what sticks are for. **Removable
  media writable**, by SCSI's removable bit: many USB disks say they aren't removable, and
  some sticks say so too; "plugged in by USB" is what the user can see.
- **Several AHCI drivers**, one per port: each would need the controller's shared registers.
- **The system volume chosen by label or by RockFS UUID written into the boot partition**: the
  boot partition's GUID is already in the boot information, and a disk is identified by what
  the loader actually read.
- **Static `/Volumes` entries, re-sent to running apps**: there is no way to change a running
  program's file tree from outside, and adding one would let a sandbox manager rearrange apps'
  trees under them.
- **Bringing the system volume back when its stick returns**: tempting, but the cache's
  unsaved changes are gone and the stick may have been changed elsewhere meanwhile.

## Open questions

- **Keyboard lights**: an operation for the input service to tell keyboards which lights to
  show (PS/2 and USB).
- **Standard users and sticks**: whether a standard user's session gets removable volumes, and
  with what rules.
- Card readers' further slots, and medium changes (a card swapped while the reader stays in).
- Writing to internal disks by an administrator's choice (Q5's fourth layer).
- Disks with 4096-byte sectors, through the storage service (note 0015).

## Changes
- 26 November 2026: first draft, for discussion with Chris.
- 26 November 2026: accepted by Chris from the summary of its decisions.
- 26 November 2026: two disks holding the boot partition (two sticks written from one image):
  `BootDisk` gains `TwoFound @5`, neither is the boot disk, no system volume is chosen, and the
  desktop asks for one to be taken out; a second copy plugged in later is just another USB stick.
  Settled in note 0031, section 2.
