# 0031: The USB stick image, how M7 is tested, and the order of building

Status: Accepted · 26 November 2026, accepted 26 November 2026 · Related: DESIGN.md section 14; notes 0004, 0017, 0026 to
0030; `docs/m7-plan.md` (Q5, Q6, Q8, Q9, Q10, Q11, Q12); milestone M7

## The problem

Notes 0027 to 0030 say what M7 builds. This one says how it reaches Roxy and how we know it
works:
- **the stick image**: what is on it, how it is written to a stick from Windows or Linux, and
  how an old BIOS is persuaded to start it (Q6, Q12);
- **the test configurations**: which of the four QEMU machines gets EHCI, AHCI, a USB boot
  stick and a "foreign" disk, so that every new driver runs on every change (Q5, Q8), and the
  places notes 0027 to 0030 left to this note;
- **trying it on Roxy**: what is checked by hand, and when (Q9);
- **the order of building**, and who builds each part;
- **the release**, `2.0.0-alpha.2` (Q10), with Intel's microcode fetched at a pinned version
  (note 0027, section 1e).

It also settles one question note 0027 handed to note 0030 and 0030 didn't answer: what
happens when two disks hold the partition the machine started from, as two sticks written from
one image will.

## The design

### 1. The stick image

Today the build tool writes `prettybigrock.img`: a protective MBR, a GPT, the BIOS boot
partition and the EFI system partition (note 0004). The system volume is a separate disk
(`prettybigrock-data.img`, note 0026's Changes). A stick can only be one disk, so the stick
image is the boot disk with the ready-made system volume added as a third partition:

| Partition | Size | What |
|-----------|------|------|
| 1, BIOS boot | 64 MiB (note 0027 grows it from 32) | The BIOS loader's later stages, the payload, the microcode files |
| 2, EFI system | 96 MiB, FAT32 | The UEFI loader, the kernel, the start-up archive, `cmdline.txt`, microcode |
| 3, `Files` | 256 MiB, RockFS (GPT type `03971446-…`, ROCKFS.md) | The system volume, as in `prettybigrock-data.img`: one administrator, `owner`, and the Welcome window on first start |

With 1 MiB at the start and the backup GPT at the end, that is **418 MiB**, so it fits a
512 MB stick or larger. It is written by the same code as today's two images (`xtask/src/image.rs`
and `datadisk.rs`), as `prettybigrock-stick.img`, beside the existing images, which stay for QEMU
and the Windows launcher.

**A stick larger than the image.** Written with `dd` or Rufus onto a 16 GB stick, the GPT still
says the disk is 418 MiB and its backup copy is in the middle of the stick, not at its end.
That is allowed: everything that reads a GPT here (both loaders, `lib/partition`) uses the main
copy and looks at the last sector only when the main copy is damaged, and nothing in
PrettyBigRock writes a partition table. Linux and Windows notice and offer to "fix" it; that is
harmless, and declining is fine too. Using the rest of the stick is for later (open questions).

**Writing it to a stick.**
- **On Windows: Rufus**, choosing the `.img` file; Rufus writes it byte for byte ("DD image"
  mode, which it picks for a disk image on its own). Afterwards Windows may say a partition
  "needs to be formatted" (the BIOS boot and RockFS partitions mean nothing to it): **choose
  Cancel**, since formatting would wipe that partition. Windows doesn't show the EFI system
  partition at all, so `cmdline.txt` can't be edited there (section 4 has what to do instead).
- **On Linux: `dd`**, `sudo dd if=prettybigrock-stick.img of=/dev/sdX bs=4M conv=fsync`,
  with `lsblk` first to be sure of `sdX`.

The release notes give both, with the warning that the stick's contents are replaced.

**The partitions' GUIDs.** Today every image's partitions have the same fixed unique GUIDs
(`7B2E5C01-…` and so on, `image.rs`). The boot partition's GUID is how the storage service
finds the stick the machine started from (note 0030, section 7), so the stick image's GUIDs are
made from the version: each GUID is the first 16 bytes of a hash of the version string and the
partition's number, with GPT's "random" version bits set. A build is still repeatable (the same
source makes the same image), and a stick from one release can't be mistaken for one from
another. Sticks written from the **same** image are identical, though, so the storage service
must cope with two of them plugged in at once (section 2).

**Old BIOSes and the "active" mark (Q12; note 0004's open question).** An MBR marks one
partition "active" (byte `0x80`), and some BIOSes refuse to start a disk without one. A GPT
disk's protective MBR has a single entry covering the whole disk, which the UEFI
specification says must *not* be marked active: some UEFI firmware then treats the disk as an
old MBR disk and won't start it the UEFI way. The plan recommended marking it active anyway;
the specification's warning changes that. Instead:
1. the stick image follows the specification (not active), as today's image does;
2. stage 1 tries Roxy's BIOS route with it. If Roxy refuses, the build tool marks the entry
   active, and that image is tried on Roxy both ways and in OVMF before it becomes the
   default;
3. only if both fail does M7 build a hybrid MBR (real MBR entries for the first two
   partitions), with a note of its own.

Roxy's firmware also offers UEFI, so the stick starts on Roxy either way; the BIOS route is
proved because it is DESIGN.md's reason for keeping the BIOS loader.

### 2. Two disks holding the boot partition

Note 0027 left this to note 0030, which didn't settle it. A person with two sticks written from
one release image, both plugged in, gives the storage service two disks with a partition of the
boot partition's GUID. Their contents may differ (each has had its own files saved to it), and
nothing the loaders record can tell them apart: the loader read one of them, and both have the
same GUIDs.

**Neither is the boot disk.** The storage service's `boot_disk` (note 0030, section 5) gains a
fifth answer, **`TwoFound @5`**, and the system volume isn't chosen from either (or from any
other disk: falling back to "the first anywhere" would likely pick one of the two). The users
service starts with the system volume in memory and the desktop says:

> Two disks look like the one PrettyBigRock started from, so neither is being used for your
> files. Take out the one you didn't start from, then restart.

Both are still listed, `OtherDisk` and so read-only until a restart sorts them out (USB disks
are writable, but these two are exactly the ones whose writes could go to the wrong place).
If the second stick is plugged in after the system volume was chosen, nothing changes: the
first was chosen, and the newcomer is just another USB stick, writable, with a volume of the
same identity and label; `/Volumes` names it `Files 2` (note 0030, section 8). Note 0030 gets a Changes line saying so, since it
is an accepted note.

This is rare, and costs one message and one enum value. The alternatives (section
"Alternatives considered") either guess, and a wrong guess saves files to the wrong stick, or
need each stick to be made different when it is written, which Rufus and `dd` can't do.

### 3. The test configurations (Q8)

The four configurations stay four, with two boots each (note 0026, section 6). Two change:

| | x86-64 UEFI | x86-64 BIOS: **Roxy-like** | ARM64 UEFI (EL1) | ARM64 EL2 |
|---|---|---|---|---|
| Boot disk | virtio | **the stick image, as a USB stick on EHCI** (`usb-storage`, `bootindex=0`) | virtio | virtio |
| System volume | test GPT disk (virtio) | **the stick's third partition** | test GPT disk **on AHCI** (`ich9-ahci`) | test GPT disk (virtio) |
| USB | none | **two EHCI controllers** (`ich9-usb-ehci1`, no companions, as Roxy), keyboard and tablet on one, the stick on the other | **one EHCI controller**, keyboard and tablet (replacing today's `qemu-xhci` and `usb-kbd`) | none |
| AHCI | none | **q35's own controller at 00:1f.2, with the foreign disk** | `ich9-ahci`, the system volume's disk | none |
| Keyboard and mouse in the desktop boot | virtio and PS/2, as now | **USB only** (PS/2 is there but sent nothing) | virtio, as now | virtio, as now |
| Processor | `max` | **`SandyBridge`** (Roxy's: no SMEP or SMAP) | Cortex-A72 | `max` |
| `cmdline.txt` | as now | **`mitigations=on`** (note 0027, section 9) | as now | as now |
| Power off in the desktop boot | **the power button** (`system_powerdown`, note 0028) | the menu's Turn off | the menu | the menu |

The existing virtio test disks (the 2048-sector raw disk, the MBR and GPT test disks, the empty
disk, the classic disk) stay on every configuration, so the storage and driver tests that
already pass keep running; only the Roxy-like configuration's system volume moves to the stick.

**Why these two.** The Roxy-like configuration is as close to Roxy as QEMU gets: q35 has the
same ICH9 AHCI controller at the same address, SeaBIOS starts a USB stick on EHCI as Roxy's BIOS
will, and its keyboard and mouse are USB. ARM64 EL1 runs EHCI and AHCI on the other
architecture (note 0029's DMA below 4 GiB, interrupt lines through the device tree's map rather
than AML), and its AHCI disk is writable, which tests note 0030's second writable kind (the
system volume's disk). UEFI x86-64 keeps PS/2 tested; EL2 stays as it is.

**The shared interrupt line (note 0027, section 9).** The Roxy-like configuration's two EHCI
controllers, which have no MSI, are put in PCI slots that q35's tables route to the **same**
line (stage 4 chooses the slots, and checks through the ACPI service's `route` that they share
before relying on it). Both drivers then serve one line for every keystroke and every block read
from the stick.

**The foreign disk (Q5, second layer).** The Roxy-like configuration's AHCI disk,
`test-foreign.img`, is made afresh for each boot with a GPT and:
- a RockFS volume left as if the machine had lost power (its "cleanly shut down" mark unset);
- a FAT32 volume with its "dirty" bit set;
- a partition of a type PrettyBigRock doesn't know (an NTFS one, filled with a pattern).

It is attached **writable** (not `snapshot=on`, which would hide any write from us). The runner
keeps a copy of its bytes and, after each boot, **compares the whole disk with the copy**; any
difference fails the boot test, naming the first sector that differs. The self-test meanwhile
checks its volumes are listed as `OtherDisk`, mounted read-only, and readable.

**The boot stick.** Also writable, made afresh for the self-test boot and kept for the desktop
boot (as the test disks are, note 0026, section 6). Its third partition holds the desktop boot's
users (`xtask/src/testdisks.rs`'s `SYSTEM`) rather than the release's `owner`. The test GPT disk
keeps its own `Home` system volume too, as a decoy: the self-test checks the system volume is
the stick's, which is only true if the boot partition was followed (note 0030, section 7, rule
1). The two copies' tests (section 2) are on the host, as a function from the disks found to
the answer.

**Where notes 0029 and 0030's QEMU tests run.**
- On the **Roxy-like** configuration: everything with `device_add` and `device_del` (a
  keyboard pulled and plugged back, a FAT32 stick plugged in, written and pulled out during a
  write); the host driver crashed while a test class driver is using a device; the **system
  stick pulled out** as the very last step of the desktop boot, checking the desktop's message
  (note 0030, section 8). That boot has nothing to check after it.
- On **ARM64 EL1**: the self-test finds the keyboard, tablet and AHCI disk, and reads and
  writes the system volume through AHCI; one FAT32 stick is plugged in and read.
- Everywhere: `pbr.usb`'s and `pbr.devices`' refusals in the program self-test, which need no
  USB device.

**Using KVM on Impala** (`PBR_QEMU_KVM=1`): the Roxy-like configuration runs with
`-cpu SandyBridge` under KVM too, since Impala's processor has every feature that model asks
for. Without KVM, if QEMU's emulator can't provide the model, stage 1 uses the nearest model it
can and says which in the configuration's comment.

**How long CI takes.** The Roxy-like boots read their system volume through USB, the slowest
path. Stage 4 measures CI before and after adding the configuration; if its longest job grows by
more than a quarter, I report it to Chris with what could be cut before going on.

### 4. Seeing what happens on Roxy (Q9)

- **The serial log**: COM2's header, a header-to-DB9 bracket and a USB-serial adapter on
  **Impala** (Chris, 26 November), at 115200 baud (note 0027, section 3), where Claude can read
  it directly while Chris drives Roxy. From stage 1b the kernel writes there when `cmdline.txt`
  says `console=0x2f8`. Until the cable and adapter arrive, Roxy's visits rely on the screen
  (and a phone photo); stage 1a needs nothing more.
- **`cmdline.txt` can't be edited from Windows** (section 1), so the build tool takes extra words
  for it: `PBR_CMDLINE="console=0x2f8"` puts them on the stick image's `cmdline.txt`. I build
  Roxy's images on Impala; Chris writes them with Rufus on Harriet (or `dd` on Impala). The
  **release** stick has no `console=` word: a machine without a serial port then writes to
  COM1, if any, which harms nothing.
- **The self-test on Roxy** (`selftest` in `cmdline.txt`, a second image,
  `prettybigrock-stick-selftest.img`): the kernel's and the program self-tests are what count
  there. The later ones look for QEMU's devices (virtio, the test disks) and are expected to
  fail on Roxy; a self-test written for real machines isn't in M7.
- **The screen**: the kernel's report and its panic screen are on the firmware's framebuffer,
  for when the serial log isn't connected. A phone photo is enough.

**Each visit to Roxy**, with Roxy's own drives disconnected (Chris's rule) and the serial log
on:

| After stage | What Chris checks |
|-------------|-------------------|
| 1a | The stick starts through UEFI, and through the BIOS (section 1, the active mark); the kernel's report on screen, all four cores, the screen's size |
| 1b | The serial log; `Microcode: 0x28, updated to 0x2f` on every core; Meltdown, L1TF and MDS each reported on; the self-test's first two parts pass |
| 2 | The ACPI service's summary compared with Linux's (`lspci -vv`, `/proc/interrupts`); PS/2 and COM2 found through AML; the power button turns Roxy off |
| 3 | A spare SATA disk, **only** after the automated foreign-disk test passes: checksummed on another machine before and after a boot that lists and reads it (Q5's third layer) |
| 5 | The USB keyboard and mouse, behind the rate-matching hubs, at the desktop; plugging and unplugging them; an external hub if one is to hand |
| 6 | The desktop started from the stick with its files on the stick; a file saved, Roxy restarted, the file still there; a second stick plugged in, read, written and pulled out; the system stick pulled out (the message) |
| 7 | The release's stick, written with Rufus exactly as the release notes say, from start to Turn off |

I write each visit's checklist into STATUS.md before Chris goes, and what Chris saw into the
DEVLOG afterwards, with the serial log saved in `docs/roxy/` when it shows something we fixed.

### 5. The order of building

Each stage ends as M6's did: every test passing on all four configurations (on GitHub's CI,
with small runs locally), small commits pushed to `m7`, the notes, STATUS, DEVLOG and ROADMAP
brought up to date, and a report to Chris. Stress runs where marked.

| Stage | What | Who |
|-------|------|-----|
| 0 | Notes 0027 to 0031 accepted | Opus |
| 1a | **First start on Roxy**: the stick image (section 1), the boot partition's GUIDs from the version, `PBR_CMDLINE`; today's kernel started on Roxy from it, before any new driver. Whatever Roxy shows up is fixed here (timer calibration, starting cores, the memory map) | Opus |
| 1b | **Kernel 1.4** (note 0027): KPTI, MDS and L1TF, microcode and `cargo xtask microcode` (section 6), `console=`, the boot partition in the boot information, shared interrupt lines, `interrupt_configure`; the lock-order checker for plain spin locks (Q11). The Roxy-like configuration's processor and `mitigations=on`. Stress | Opus |
| 2 | **ACPI** (note 0028): `lib/aml` on the host first (with `acpiexec` cross-checks), then `services/acpi`; routing through `_PRT`; legacy devices and power off from AML; the device manager's built-in list and the power service's `\_S5` scan removed; the power button test on x86-64 UEFI | Opus |
| 3 | **AHCI** and **which disks are written** (note 0030, sections 4 to 6): `pbr.storage` version 2's `Access`, read-only filesystem servers, `pbr.driver.host`'s `child` and `decline`, `drivers/ahci`; the foreign disk on the Roxy-like configuration and AHCI on ARM64 EL1 | Opus |
| 4 | **USB** (note 0029): `lib/usb` with its simulated bus, `drivers/ehci`, hubs, `pbr.usb`, `pbr.devices` version 2, coming and going and `reconnect` in the device manager, `lib/driver`'s timers; EHCI on both configurations, the shared line, CI measured. Stress | Opus; the simulated devices' catalogue may be Sonnet's |
| 5 | **USB keyboards and mice** (note 0030, section 2): `lib/hid`, `drivers/usb-hid`, the key table, `decline`; the Roxy-like desktop boot on USB input only | Sonnet, from note 0030; review by Opus |
| 6 | **USB storage and disks that come and go** (note 0030, sections 3, 5, 7, 8): `drivers/usb-storage`; the storage service watching, `Gone`, identities, `boot_disk` (with section 2's `TwoFound`); the system volume from the boot disk; the live `/Volumes`; the system stick pulled out. The Roxy-like configuration boots from its stick. Stress | `usb-storage` by Sonnet; the rest by Opus |
| 7 | **The "done when"** on Roxy (section 4); the review of everything M7 added that hands out, derives, checks or revokes capabilities (the device manager's USB and child devices, `pbr.usb`'s rules, `pbr.storage`'s access, `/Volumes`, the ACPI service's regions); README, ROADMAP, DESIGN.md, every M7 note marked built; the release (section 6); Stress with 120 boots of each kind | Opus; Haiku summarises the stress logs |

`lib/hid` needs nothing but note 0030, so Sonnet can write it while stage 4 is built and
reviewed; likewise `lib/aml`'s test tables can be gathered while stage 1 runs on Roxy. Stage 3
doesn't need stage 2 (AHCI has MSI), so if Roxy holds stage 2 up, stage 3 goes first.

### 6. The release, `2.0.0-alpha.2` (Q10)

Still an alpha: the network is M8, and betas begin when it works (ROADMAP.md). The version goes
into `lib/manifest`'s `VERSION` in stage 7. The release on GitHub attaches, as alpha.1 did, the
version on the files rather than the build tool's names (note 0026's Changes):
- `prettybigrock-2.0.0-alpha.2-stick.zip`: **the stick image**, zipped (it is mostly empty
  space, so the zip is small), with a short `README.txt` saying how to write it (section 1);
- `prettybigrock-2.0.0-alpha.2.zip`: the launcher, the QEMU images and the ready-made data disk,
  as alpha.1's;
- the QEMU images on their own, and the self-test image.

**VirtualBox and VMware** (the plan's Q10): both can start from the stick image once it is
converted (VirtualBox's `VBoxManage convertfromraw`; VMware's `vmware-vdiskmanager`), and both
need what M7 adds (AHCI; and EHCI for VMware's keyboard and mouse when attached as USB). The
release notes give the steps, marked untested unless Chris has tried them; neither is a
condition of the release, since neither runs on Impala or in CI.

**Microcode in the release** (note 0027, section 1e). A new command, `cargo xtask microcode`,
fetches Intel's published release from
`github.com/intel/Intel-Linux-Processor-Microcode-Data-Files` at the tag named in
`xtask/src/microcode.rs`, checks the download's SHA-256 against the sum recorded beside the tag,
and unpacks the `intel-ucode` folder and Intel's licence into `build/microcode/` (outside git,
as `build/` already is). A sum that doesn't match is an error, and nothing is unpacked. The
release build runs it and passes the folder as `PBR_MICROCODE`; ordinary builds and CI don't,
since QEMU ignores microcode. It uses `curl` and `sha256sum` from the host, as the build already
uses `mtools` and Python for checks: they run on the machine that builds, and put nothing of
theirs into PrettyBigRock. The tag is pinned in stage 1b, when Roxy first needs the update; moving
to a newer one is a one-line change with a new sum, reviewed like any other. The release notes say
the microcode is Intel's, under Intel's licence, which is beside it on the stick.

Roxy's own images for stages 1b to 6 are built with the fetched microcode too, so the stick
Chris tries is the one the release will be.

### 7. Testing this note's own parts

- **On the host**: the stick image's layout (the three partitions where the table above says,
  both GPT copies intact and agreeing, the protective MBR as the specification says), read back
  through `lib/partition` and the RockFS library; `lib/partition` on a copy of the image placed at
  the start of a larger disk, with the backup copy in the middle (found, no problem reported);
  the GUIDs the same for the same version and different for another; section 2's choice as a
  function from the disks found to the answer, with zero, one, two and three matches and the
  second arriving late; `xtask/src/microcode.rs`'s check refusing a file with one byte changed
  (on a small made-up file, not Intel's).
- **In QEMU**: section 3.
- **On Roxy**: section 4.

## Alternatives considered

- **Marking the protective MBR active**, as the plan recommended. The UEFI specification says
  not to, and some UEFI firmware then won't start the disk the UEFI way; tried only if Roxy's
  BIOS needs it (section 1).
- **A hybrid MBR from the start.** It works on more old BIOSes, but a hybrid MBR is a second
  partition table that can disagree with the GPT, and tools that edit one leave the other wrong.
  Only if Roxy needs it.
- **The system volume using the rest of the stick**, grown at first start. That means writing a
  partition table on a disk, which PrettyBigRock doesn't do yet (note 0026), and a 418 MiB image
  is quick to write. For later.
- **Fixed GUIDs, as today.** Sticks from different releases would then be indistinguishable.
  **Fresh random GUIDs for every build** would make the build unrepeatable, and still not tell
  apart two sticks written from one image.
- **Picking one of two matching sticks** (the first found, or the one whose RockFS volume was
  saved to most recently). Either can be wrong, and the cost of wrong is files saved to the stick
  the person didn't start from, which they might then wipe. A message and a restart cost little.
- **A separate data stick**, keeping the boot image as it is. Two sticks for one machine, and
  the question of which data stick is the system's comes back.
- **Every configuration "Roxy-like".** Then virtio and PS/2 would no longer be tested, and every
  boot would read its files over emulated USB, slowly.
- **Our own HTTPS and SHA-256 in the build tool** for the microcode. DESIGN.md section 14's rule
  is about PrettyBigRock's own code; the build machine's tools put nothing into the system, and
  `curl` and `sha256sum` are already on every machine that builds it.

## Open questions

- Using the rest of a large stick for files: grow the RockFS volume, or add a fourth partition,
  at the owner's request.
- A self-test for real machines, which skips what isn't there rather than failing (after M7).
- A hybrid MBR, if Roxy's BIOS turns out to need one.
- Booting from a stick on xHCI (USB 3) controllers, which most machines since about 2015 have
  and Roxy doesn't (M11).

## Changes
- 26 November 2026: first draft, for discussion with Chris.
- 26 November 2026: accepted by Chris from the summary of its decisions. The serial adapter
  goes on Impala, not Harriet; the cable and adapter aren't here yet.
