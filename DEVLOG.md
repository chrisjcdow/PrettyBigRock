# PrettyBigRock development log

PrettyBigRock (originally FynbosOS) is developed by Chris together with Claude, Anthropic's AI model.
This log records what was built in each session and who did what.

## Session 1: 28 October 2026, from one prompt to a booting desktop (FynbosOS 1.0)

**The prompt (Chris):** "Could you write me an operating system that I could boot in a VM.
This OS should have a GUI. Try not to plagiarise too much."

**What Claude built, in about 24 minutes:**
- A two-stage boot loader that picks a VESA graphics mode and enters 32-bit protected mode
- A kernel with interrupts, timer, PS/2 keyboard and mouse, CMOS clock and ACPI power-off
- Double-buffered graphics, a hand-drawn 5x9 font, pixel-art icons and a mouse cursor
- A window manager with draggable windows, taskbar, start menu and desktop icons
- A procedurally drawn wallpaper of Table Mountain at sunset, tablecloth included
- Seven apps: Terminal, Notepad, Files, Calculator, Paint, Snake and About
- Disk images for QEMU, VirtualBox and VMware

Claude tested it in QEMU by scripting mouse and keyboard input and reading screenshots,
fixing a few things along the way (a hidden sun, a spiky cloud, Ctrl+Esc, scrollback).

**Chris:** booted it on Harriet (Windows 11 laptop) in QEMU, and decided to publish it on GitHub
and keep developing it together.

## Session 2: 28 October 2026, a new name (PrettyBigRock 1.1)

**Chris** chose to publish under GPL-3.0-or-later so the project stays open source, and started
rethinking the name. He'd wondered about "FynbOS", but "fynbos" is hard to search for and is
already a registered software trademark (the Fynbos Money app). Chris has a habit of pointing at
Table Mountain and saying "big rock" or "pretty rock", so he suggested **PrettyBigRock**.

**Claude:**
- Renamed the OS everywhere: boot loader, splash, start button and menu, About, Terminal
  (prompt is now `rock>`), crash and shutdown screens, docs, disk images and launcher
- Drew a new 16x16 pixel-art logo of Table Mountain with its tablecloth, the sun and the sea
- Hid an Easter egg in the Terminal for Chris and his wife, at Chris's request. It isn't in `help`.
- Kept the original FynbosOS 1.0 safe, as tag `fynbos-1.0` and as Chris's own copy
