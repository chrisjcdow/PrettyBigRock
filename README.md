# PrettyBigRock

A small 32-bit x86 operating system with a graphical desktop, written from scratch:
boot loader, kernel, drivers, bitmap font, icons, window manager and apps.
It's named after Table Mountain, a pretty big rock, which is also its wallpaper: drawn
procedurally at boot, at dusk, tablecloth and all.

![PrettyBigRock desktop](screenshot.png)

## How this was made

The first version was written by **Claude** (Anthropic's AI model) from a single prompt by Chris:
*"Could you write me an operating system that I could boot in a VM. This OS should have a GUI.
Try not to plagiarise too much."* Claude wrote every line of code, the font and the icons.
It was called **FynbosOS** at first; that original is kept in this repository's history under
the tag `fynbos-1.0`, and was renamed to PrettyBigRock in version 1.1.
From here on, Chris and Claude are developing it together, and [DEVLOG.md](DEVLOG.md)
records who did what in each session.

## What's inside

- **Boot loader** (`boot/`): a 512-byte MBR that loads the rest with BIOS LBA reads, then a
  second stage that scans the VESA mode list for the best linear-framebuffer mode
  (1024x768x32 preferred, falls back to 800x600 or 640x480, 32- or 24-bit), enables A20
  and switches to protected mode.
- **Kernel** (`kernel/`): IDT and remapped PICs, 100 Hz PIT timer, PS/2 keyboard and mouse,
  CMOS clock, ACPI soft power-off (parses the FADT and DSDT `\_S5`), reboot, and a crash screen.
- **Graphics**: double-buffered drawing with clipping and dirty-rectangle updates, alpha
  blending, gradients, rounded shapes, and a hand-drawn 5x9 font.
- **Window manager**: overlapping draggable windows with shadows, focus, minimise/close,
  taskbar with clock, start menu (Windows key or Ctrl+Esc), desktop icons.
- **Apps**: Terminal (20-odd commands, history, scrollback), Notepad, Files, Calculator,
  Paint, Snake and About. Files live in a RAM filesystem shared by all apps.

## Running it

Download the disk image for your hypervisor from the **Releases** page (or build it yourself,
below), and put it in a `build/` folder:

| Hypervisor | File | Notes |
|---|---|---|
| QEMU / Proxmox | `prettybigrock.img` | `qemu-system-i386 -m 128 -drive file=prettybigrock.img,format=raw`, or on Windows double-click `run-prettybigrock.bat` |
| VirtualBox | `prettybigrock.vdi` | New VM, type *Other*, version *Other/Unknown*, 64 MB+ RAM, attach as the hard disk |
| VMware | `prettybigrock.vmdk` | New VM, guest *Other*, use an existing disk, BIOS firmware |

Requirements: **legacy BIOS boot** (not UEFI) and a **PS/2 mouse**. In VirtualBox set
System > Pointing Device to *PS/2 Mouse*; with the default USB tablet the pointer won't move.
The VM captures the mouse when you click inside it (host key releases it).

The clock shows the VM's hardware time, which is usually UTC. In the Terminal, `tz 2` switches it to SAST.

## Building

Needs `gcc` (with `-m32` support), `nasm`, `ld`, `objcopy` and Python 3; `qemu-img` is optional
(for the .vdi/.vmdk conversions).

```
./build.sh
```

## Licence

Copyright (C) 2026 Christopher Cameron-Dow

PrettyBigRock is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3
of the License, or (at your option) any later version. It is distributed in the hope that it
will be useful, but WITHOUT ANY WARRANTY. See [LICENSE](LICENSE) for the full text.

## Known limits

Single-tasking, no disk filesystem (files vanish on restart), no networking, US keyboard layout,
no mouse wheel.
