# M7 plan: real hardware I, Roxy (draft, 23 November 2026)

This is the plan for Chris and Claude to agree before M7's design notes are written, as
`docs/m6-plan.md` was for M6. Once the notes are accepted they settle everything here; where they
differ, the notes win. **Status: draft, not yet discussed.** Branch `m7` started at `20a8936`
(same as main).

M7's aim (ROADMAP.md): **PrettyBigRock boots on Roxy from a USB stick, without touching Roxy's
own disks, and runs the desktop with a USB keyboard and mouse.**

## 1. What Roxy is

From `lspci -nn` on Roxy (23 November 2026):

| Address | Device | IDs | What M7 does with it |
|---------|--------|-----|----------------------|
| 00:02.0 | Intel HD Graphics 2000 (Sandy Bridge) | 8086:0102 | The firmware's framebuffer (VESA from the BIOS loader, or GOP), through the existing framebuffer driver |
| 00:1a.0, 00:1d.0 | Two EHCI USB 2 controllers (6 Series) | 8086:1c2d, 8086:1c26 | The EHCI driver. **No UHCI/OHCI companions**: each controller has a rate-matching hub built in, so every keyboard and mouse (full or low speed) is behind a hub, and hubs must work from the start |
| 00:1f.2 | 6-port SATA AHCI controller | 8086:1c02 | The AHCI driver. Roxy's own disks are on it (section 2, Q5) |
| 02:00.0 | Realtek RTL8111/8168 Gigabit Ethernet, rev 06 | 10ec:8168 | Q1. QEMU has no model of it |
| 00:1f.0 | H61 LPC bridge | 8086:1c5c | PS/2 and serial ports, if the board has them, described in AML |
| 00:1b.0, 00:16.0, 00:1f.3, 03:00.0 | HD audio, ME, SMBus, a PCIe-to-PCI bridge (IT8893E) | | Nothing in M7 |

The processor is an i5-2300 (Sandy Bridge, 4 cores, no hyper-threading). It is affected by
Meltdown, L1TF and MDS (Q7). It has VT-x but **no VT-d, so there is no IOMMU**: on Roxy a
driver that programs DMA can reach any memory. Note 0010 already treats DMA drivers as trusted
on machines without one; M7 doesn't change that, but the notes should say it plainly.

**Serial port** (Chris, 23 November): the board has a 10-pin header labelled **COM2** (9 pins and
a key), and no port on the back panel. A header-to-DB9 bracket and a USB-serial adapter on
Harriet give us the serial log (Q9).

**Roxy's disks** (Chris, 23 November): Chris will **disconnect all of Roxy's drives** before
booting PrettyBigRock, and may connect a spare drive he doesn't care about to check that
PrettyBigRock leaves disks alone (Q5).

**Still to find out** (Chris to run on Roxy): whether the firmware offers UEFI or only BIOS
(`sudo dmidecode -t baseboard -t bios`), which I/O address the BIOS gives the COM2 header (its
setup screen, or `dmesg | grep ttyS`: `0x3F8` or `0x2F8`), whether the EHCI controllers have MSI
(`sudo lspci -vv -s 00:1d.0 | grep -i msi`, and `/proc/interrupts`), and a copy of its ACPI
tables (`sudo acpidump > roxy-acpi.dat`) for the interpreter's host tests.

## 2. The design notes M7 needs

**0027 Kernel interface 1.4: real hardware.** Opus.
- Meltdown and friends (Q7): separate page tables for user space on affected x86-64 processors
  (KPTI), clearing the processor's buffers on the way back to user space (MDS), and keeping
  not-present page table entries harmless (L1TF). Off where the processor says it isn't
  affected, so QEMU's cost is measurable both ways.
- **Shared legacy interrupt lines (INTx)**, if Roxy's EHCI has no MSI (likely; Q13): a
  level-triggered line, masked when it fires and unmasked when every driver sharing it has said
  it is done. QEMU's EHCI has no MSI either, so the tests use it.
- Write-combining for framebuffers (the PAT): without it, drawing to Roxy's real video memory is
  many times slower than in QEMU, where the framebuffer is ordinary memory.
- Whatever booting on Roxy shows up: timer calibration against the HPET or PIT, starting cores
  with real-hardware delays, the firmware's memory map. Stage 1 finds out.
- The serial console's port on x86-64 taken from the ACPI SPCR table when there is one, else
  a `console=` line in `cmdline.txt` (for example `console=0x2f8`), else COM1 as today. Roxy's
  only port is a header labelled COM2, which its BIOS may put at `0x2F8`; the device manager
  must then not also start a driver for it.
- The boot information gains **the identity of the disk the system started from** (Q6), from
  the GPT header the loaders already read; note 0003 goes to version 1.1.

**0028 ACPI: the AML interpreter and the ACPI service.** Opus.
- ACPI is how a PC describes the devices PCI doesn't announce (the keyboard controller, serial
  ports), how its interrupt lines are wired (`_PRT`), and how to turn it off (`\_S5`). Its
  descriptions are programs in a bytecode, AML, which an interpreter has to run.
- Our own interpreter in `lib/aml`, host-tested on QEMU's tables (q35 and `virt`), Roxy's, and
  Harriet's, cross-checked against ACPICA's `acpiexec` as RockFS is against `tools/rockfs.py`.
  Distrusting the bytecode as `lib/devices` distrusts configuration space (Q2).
- A new service, `services/acpi`, runs it, holding only the I/O ports, memory and PCI
  configuration space the tables' operation regions name. It calls `\_PIC(1)` (switching the
  firmware's answers to APIC mode), answers the device manager's questions (legacy devices,
  interrupt routing) and the power service's (sleep types).
- The device manager's built-in list (note 0011, section 4) and the power service's temporary
  `\_S5` scan (note 0024) are removed.

**0029 USB: the EHCI driver, hubs, `pbr.usb` and devices that come and go.** Opus.
- The host controller driver (`drivers/ehci`) takes the controller from the firmware (the
  firmware has been pretending USB keyboards are PS/2 ones until then), owns its DMA, drives
  hubs itself (the built-in rate-matching hub first), and gives each USB interface it finds to
  the device manager (Q3).
- `pbr.usb`: a class driver's view of one interface: its descriptors, control, bulk and
  interrupt transfers. Class drivers never program DMA; the host driver copies.
- Hot-plug in the device manager (note 0011's open question): devices appearing and going,
  drivers started and stopped for them; a USB device's stable identity (port path plus serial).
- How the first keyboard works before USB is up: the firmware's PS/2 emulation, until the EHCI
  driver takes over.

**0030 USB keyboards, mice and storage; disks that come and go.** Opus for storage, Sonnet for
the class drivers once the note is accepted.
- `drivers/usb-hid`: keyboards and mice, reading HID report descriptors (Q4), into the input
  service as PS/2 and virtio-input already do.
- `drivers/usb-storage`: bulk-only transport and the SCSI commands a stick needs, behind
  `pbr.block`.
- `drivers/ahci`: SATA disks behind `pbr.block` (written following `drivers/virtio-blk`).
- The storage service: disks and volumes appearing and going (note 0015's open question), the
  system volume chosen by the boot disk's identity (note 0017's open question; Q6), and **disks
  other than the boot disk read-only** (Q5).

**0031 The USB stick image, how M7 is tested, and the order of building.** Opus.
- One image for a stick: today's boot disk plus the ready-made system volume as a third
  partition, written with Rufus on Windows or `dd` (Q6, Q12).
- The test configurations (Q8), the automated check that a "foreign" disk is untouched (Q5),
  the test on Roxy by hand (Q9), the stages, and the release (Q10).

## 3. Questions, with recommendations

**Q1. The Ethernet chip: M7 or M8?** The roadmap puts Roxy's Ethernet driver in M7, but
without M8's network stack it can only send and receive raw frames, and QEMU can't emulate an
RTL8168, so nothing could check it automatically. *Recommend:* move it to M8, where note 0032
designs the network interface with two drivers at once (virtio-net, tested in QEMU, and the
RTL8168, tested on Roxy by getting an address from your router). M7 is already the largest
milestone so far. The roadmap's lines would move with it.

**Q2. The AML interpreter: ours, or borrowed?** The choices are our own, ACPICA (C, the
reference, used by most operating systems; needs a C library, which is M9), or the Rust `aml`
crate (MIT/Apache, rust-osdev, being rewritten). *Recommend:* our own (DESIGN.md section 14's
rule), in its own service, built only as far as M7 needs (the namespace, methods, the common
operation regions, `_PRT`, `_CRS`, `_STA`, `_S5`, `\_PIC`), using `acpiexec` only as a host-side
cross-check, never in the system. Batteries, lids, sleep and embedded controllers wait for M11.
It is the biggest single piece of M7.

**Q3. How USB is split into programs.** *Recommend:* one host controller driver per controller
(it owns the controller, its DMA and every hub), and a separate class driver process for each
interface it finds (keyboard, mouse, stick), started by the device manager and talking to the
host driver over `pbr.usb`. A hostile USB stick's answers are then parsed by a program that
can't program DMA or touch the other devices. The alternative, one program for all of USB, is
simpler but puts parsing of anything plugged in next to the DMA engine.

**Q4. Keyboards and mice: boot protocol or report descriptors?** USB keyboards and mice offer
a simple fixed "boot protocol", but it has no scroll wheel and no absolute pointer, and QEMU's
tablet (which three test configurations use) needs report descriptors. *Recommend:* parse
report descriptors (a host-tested library, with recorded descriptors from real devices and
random corruption), falling back to the boot protocol when a descriptor can't be understood.

**Q5. Not touching Roxy's disks.** **Chris will disconnect Roxy's own drives** before every PrettyBigRock boot, so Roxy's data
isn't at risk however M7 goes; what follows is so that PrettyBigRock deserves trust on other
people's machines. *Recommend:* four layers. (1) In M7 the storage service
gives filesystem servers **read-only** access to every volume not on the boot disk, and never
writes a partition table or formats anything it wasn't asked to (note 0026 already says so).
(2) A test configuration has an AHCI disk with partitions and data on it, and the runner checks
its checksum is unchanged after the boot. (3) On Roxy, AHCI is first tried with a spare drive Chris doesn't
care about, holding partitions and files, whose checksum is taken on another machine before and
after; only once the automated test has passed. (4) Writing to other disks becomes possible later only by an
administrator's explicit choice, in its own note.

**Q6. The system volume when booting from a stick.** Today the system volume is the first RockFS
volume with `System/users.txt`, and the boot disk and the system disk are separate images.
*Recommend:* the stick image carries the ready-made system volume as partition 3 (sized like
today's 256 MiB `prettybigrock-data.img`; using the rest of the stick can come later), the
loaders put the boot disk's GPT identity in the boot information, and the storage service makes
only a volume on that disk the system volume. A stick plugged in later is data, never the
system. QEMU keeps its separate data disk as an option for development.

**Q7. Meltdown, L1TF and MDS.** The i5-2300 is affected by all three; QEMU's processors and
GitHub's runners say whether they are. *Recommend:* in kernel 1.4, KPTI (without PCID at first,
measured; Sandy Bridge has PCID but not `INVPCID`), `VERW` on return to user space, and safe
not-present entries, each switched on by what the processor reports. Spectre variant 2 (needs
retpolines, which the pinned compiler doesn't offer on stable, or microcode support) stays an
open question. ARM64 needs nothing for M7: the Pi 5's Cortex-A76 isn't affected by Meltdown.

**Q8. Test configurations.** QEMU can model Roxy closely: q35 already has the same ICH9 AHCI
controller at 00:1f.2, and it has EHCI, USB hubs, keyboard, mouse, tablet and storage; SeaBIOS
boots from a USB stick on EHCI. *Recommend:* keep four configurations, but make **x86-64 BIOS
"Roxy-like"**: booting from a USB stick on EHCI (no virtio disk), a USB keyboard and mouse, and
an AHCI "foreign" disk (Q5). ARM64 EL1 gets a USB keyboard and tablet on EHCI and its system
disk on AHCI, so both drivers are tested on both architectures. x86-64 UEFI and ARM64 EL2 stay
on virtio (and PS/2 stays tested through the keyboard on UEFI). CI grows somewhat; I'd measure
it in the first stage that adds a configuration.

**Q9. Seeing what happens on Roxy.** Settled: Roxy has a COM2 header. A 10-pin header-to-DB9
bracket (check its pinout is the common "Intel" one, which most are) and a USB-serial adapter
on Harriet, read with PuTTY at 115200 baud, give us the same log as QEMU. The kernel takes the
port from SPCR or `cmdline.txt` (note 0027). Also: a `selftest` choice in `cmdline.txt`, so the
stick can run the self-test boot on Roxy, and the kernel's panic screen on the framebuffer as a
fallback.

**Q10. A release at the end of M7?** *Recommend:* `2.0.0-alpha.2`, adding the USB stick image
(and VirtualBox and VMware, which also need AHCI and, for VMware, EHCI). Betas begin when the
network works (M8), as the roadmap allows.

**Q11. The lock-order checker for plain `SpinLock`s.** Left for you to decide "before M7".
*Recommend:* yes, in stage 1 with the kernel work: real hardware is where timing-dependent
deadlocks first appear, and a checker in test builds finds them in QEMU instead.

**Q12. Booting the stick on Roxy's BIOS.** Some older BIOSes boot only a disk with an "active"
partition, which a GPT's protective MBR doesn't have (note 0004's open question). *Recommend:*
mark the protective MBR's entry active (harmless to UEFI, and what several Linux installers do),
and build a hybrid MBR only if Roxy still refuses. If Roxy's firmware has UEFI too, the stick
already starts that way.

**Q13. Legacy interrupt lines.** Note 0010 said every M7 device has MSI; Intel's EHCI
controllers of that age almost certainly don't (to confirm on Roxy, section 1). *Recommend:*
shared level-triggered INTx in kernel 1.4 (note 0027), routed from AML's `_PRT` (note 0028).
Polling EHCI instead would work without either, but costs power and latency for every keyboard
press, and AML is needed anyway for power off and the legacy devices.

**Q14. A stick pulled out.** *Recommend:* a data stick's volume goes; its open files' calls fail
with `Gone`, and RockFS's copy-on-write keeps it consistent (a FAT32 stick may not be, as on
every system). Pulling the **system** stick out stops saving: the desktop says so and offers to
restart once it's back. Nothing more ambitious in M7.

## 4. Open questions from earlier notes: settle in M7, or wait

**Settle in M7** (M7 needs them):
- Which volume is the system volume, before USB storage (0017): Q6.
- Hot-plug of devices (0011) and disks, stable volume names (0015): notes 0029 and 0030.
- AML instead of the built-in list; shared interrupt lines (0010, 0011): Q2, Q13.
- KPTI (0007): Q7. PCID: measured in M7, done if KPTI costs much.
- Hybrid MBR (0004): Q12.
- Reassigning PCI resources the firmware didn't (0011): only if Roxy needs it.
- The lock-order checker (status): Q11.

**Can wait:**
- The users service waiting during a `check` (0026), checking a volume while it stays mounted
  (0026).
- Secure Boot (0004): Roxy predates it.
- Services and drivers that hang; crash reports that survive (0012, 0018).
- Everything in M6's "can wait" list that M7 doesn't touch.

## 5. Order of building (a first sketch; note 0031 settles it)

0. This plan and notes 0027 to 0031, walked through and accepted.
1. **First boot on Roxy**: the stick image (Q6, Q12) and the kernel booting on Roxy as it is
   today, with the self-test, before any new driver. Kernel 1.4 (Q7, Q11, write-combining, the
   boot disk's identity) and whatever Roxy shows up.
2. **ACPI**: `lib/aml` on the host first, then `services/acpi`; INTx routing; legacy devices and
   power off from AML.
3. **AHCI** and the read-only rule for other disks, with the untouched-disk test.
4. **USB**: the EHCI driver, hubs, `pbr.usb`, hot-plug in the device manager.
5. **USB keyboards and mice.**
6. **USB storage**, disks coming and going, the system volume from the boot disk; the Roxy-like
   configuration complete.
7. **Roxy by hand**, the review, and the release (Q10).

Roxy is tried at the end of stages 1, 3 (still without writing), 5 and 6, so each driver meets
the real hardware soon after it works in QEMU.
