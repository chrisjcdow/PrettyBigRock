# Roxy: what PrettyBigRock looked like on real hardware

Photos and serial logs from M7's visits to Roxy (docs/design/0031-stick-tests-and-stages.md,
section 4), kept small: photos are scaled to 1600 pixels wide. Chris keeps the full-size
originals outside the repository.

- **Visit 1a, 26 November 2026** (`2026-11-26-visit-1a-desktop-a.jpg`, `-b.jpg`): the first
  start of PrettyBigRock on Roxy, from `prettybigrock-stick.img` (a release build of stage 1a,
  written with Rufus). Both photos were taken 8 seconds apart, so they show the same start. The
  desktop is as in QEMU; the files are in memory, since nothing reads the stick until USB
  storage (stage 6). The clock shows UTC: with no system volume there is no user setting for
  the time zone. The keyboard and mouse didn't work (USB input is stage 5).
- **Visit 1a, through UEFI** (`2026-11-26-visit-1a-uefi.jpg`): started from the boot menu (F7).
  The UEFI loader kept the firmware's small mode, which the monitor stretched; the Welcome
  window sits off-centre, over the corner where the clock is, as if placed for a bigger screen
  (not reproduced in QEMU at 1024x768 or 1280x720). The loader now picks the largest mode up
  to 1920x1080 (note 0004, Changes), so the next visit shows whether that was all it was.
