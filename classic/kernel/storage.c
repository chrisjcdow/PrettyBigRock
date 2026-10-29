/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - storage: finds the data disk and mounts RockFS on it, or falls back to a
   RAM disk so the apps always have somewhere to save. */
#include "kernel.h"
#include "disk.h"
#include "rockfs.h"

#define RAMDISK_BYTES (8u * 1024 * 1024)

static blkdev_t *data_dev;   /* the disk RockFS is mounted from */
static blkdev_t *blank_dev;  /* a blank disk we could format */
static int on_ram, other_disks;
static char notice[600];     /* shown after the desktop appears */

void fmt_size(char *buf, int n, u64 b) {
    if (b < 1024) { ksnprintf(buf, n, "%u bytes", (u32)b); return; }
    const char *units[] = {"KB", "MB", "GB", "TB"};
    int u = 0;
    u64 whole = b / 1024, rem = b % 1024;
    while (whole >= 1024 && u < 3) { rem = whole % 1024; whole /= 1024; u++; }
    u32 tenth = (u32)(rem * 10 / 1024);
    if (whole >= 100 || !tenth) ksnprintf(buf, n, "%u %s", (u32)whole, units[u]);
    else ksnprintf(buf, n, "%u.%u %s", (u32)whole, tenth, units[u]);
}

static const char *WELCOME =
    "Welcome to PrettyBigRock!\n"
    "\n"
    "PrettyBigRock is a small hobby operating system written from\n"
    "scratch: its own boot loader, kernel, drivers, font, window\n"
    "manager, apps, and its own filesystem, RockFS.\n"
    "\n"
    "Things to try:\n"
    "  - Double-click the icons on the desktop.\n"
    "  - Drag windows around by their title bars.\n"
    "  - Open the Terminal and type 'help'.\n"
    "  - Write something in Notepad and press Ctrl+S.\n"
    "  - Draw in Paint and save it as a picture.\n"
    "  - Make folders in Files.\n"
    "\n";

static const char *SHORTCUTS =
    "Keyboard shortcuts\n"
    "------------------\n"
    "Windows key or Ctrl+Esc   open the start menu\n"
    "Ctrl+W                    close the active window\n"
    "Ctrl+S (Notepad)          save the file\n"
    "Up / Down (Terminal)      command history\n"
    "PgUp / PgDn (Terminal)    scroll back\n"
    "Arrows or WASD (Snake)    steer, P to pause\n";

static void seed_files(void) {
    char text[1200];
    ksnprintf(text, sizeof text, "%s%s", WELCOME,
              on_ram ? "There's no data disk, so files are kept in memory and are\n"
                       "gone after a restart. Add a second disk to the virtual\n"
                       "machine and PrettyBigRock will offer to set it up.\n"
                     : "Files are saved on the data disk using RockFS, so they're\n"
                       "still here after a restart.\n");
    rfs_write("/readme.txt", text, strlen(text));
    rfs_write("/shortcuts.txt", SHORTCUTS, strlen(SHORTCUTS));
    rfs_mkdir("/Documents");
    rfs_mkdir("/Pictures");
    const char *todo = "- water the proteas\n- teach the kernel to multitask\n- go for a run up Lion's Head\n";
    rfs_write("/Documents/todo.txt", todo, strlen(todo));
}

static int use_ram_disk(void) {
    blkdev_t *ram = ramdisk_create(RAMDISK_BYTES);
    if (!ram) return 0;
    if (rfs_partition_and_format(ram, "Memory")) return 0;
    u64 s, n;
    if (rfs_find_volume(ram, &s, &n) != RFS_VOL_FOUND || rfs_mount(ram, s, n)) return 0;
    on_ram = 1;
    data_dev = ram;
    seed_files();
    return 1;
}

void storage_init(void) {
    disk_init();
    for (int i = 0; i < disk_count; i++) {
        blkdev_t *d = disk_get(i);
        if (d->is_boot) continue;
        u64 start, sectors;
        int r = rfs_find_volume(d, &start, &sectors);
        if (r == RFS_VOL_FOUND && !data_dev) {
            int e = rfs_mount(d, start, sectors);
            if (e) {
                ksnprintf(notice, sizeof notice, "The data disk has a RockFS volume, but it couldn't be opened: %s.", rfs_strerror(e));
                continue;
            }
            data_dev = d;
            rfs_info_t info;
            rfs_info(&info);
            if (info.was_dirty) {
                static char report[400];
                int problems = 0, leftovers = 0;
                rfs_check(1, report, sizeof report, &problems, &leftovers);
                if (leftovers)
                    ksnprintf(notice, sizeof notice, "PrettyBigRock didn't shut down properly last time, so it checked the data disk. "
                              "Something was being saved when it stopped; that unfinished save was cleared away. Everything else is as it was.");
                else if (problems)
                    ksnprintf(notice, sizeof notice, "PrettyBigRock didn't shut down properly last time, so it checked the data disk and fixed %d problem(s):\n\n%s", problems, report);
            }
        } else if (r == RFS_VOL_BLANK && !blank_dev) {
            blank_dev = d;
        } else if (r == RFS_VOL_NONE) {
            other_disks++;
        }
    }
    if (!data_dev) use_ram_disk();
}

static void format_answer(int ok, const char *text, void *ctx) {
    (void)text; (void)ctx;
    if (!ok) return;
    int e = storage_format();
    if (e) {
        char msg[200];
        ksnprintf(msg, sizeof msg, "The disk couldn't be formatted: %s.", rfs_strerror(e));
        wm_message("Data disk", msg);
    } else {
        char size[24], msg[200];
        fmt_size(size, sizeof size, data_dev->sectors * 512);
        ksnprintf(msg, sizeof msg, "The data disk is ready (%s). Your files will now be kept after a restart.", size);
        wm_message("Data disk", msg);
    }
}

void storage_after_desktop(void) {
    if (notice[0]) wm_message("Data disk", notice);
    if (blank_dev && on_ram) {
        char size[24], msg[300];
        fmt_size(size, sizeof size, blank_dev->sectors * 512);
        ksnprintf(msg, sizeof msg, "PrettyBigRock found a blank disk (%s, %s). Set it up with RockFS so files are kept after a restart?\n\n"
                  "Anything in memory now will be left behind.", size, blank_dev->model[0] ? blank_dev->model : blank_dev->where);
        wm_dialog("Data disk", msg, "Set it up", "Not now", 0, format_answer, 0);
    }
}

/* Formats the blank disk if there is one, otherwise re-formats the current data disk. */
int storage_format(void) {
    blkdev_t *target = blank_dev ? blank_dev : (on_ram ? 0 : data_dev);
    if (!target) { /* only the RAM disk: start it afresh */
        rfs_unmount();
        on_ram = 0;
        data_dev = 0;
        return use_ram_disk() ? RFS_OK : RFS_ENOMEM;
    }
    rfs_unmount();
    int e = rfs_partition_and_format(target, "Data");
    u64 s, n;
    if (!e && rfs_find_volume(target, &s, &n) != RFS_VOL_FOUND) e = RFS_ECORRUPT;
    if (!e) e = rfs_mount(target, s, n);
    if (e) { use_ram_disk(); return e; }
    data_dev = target;
    blank_dev = 0;
    on_ram = 0;
    seed_files();
    return RFS_OK;
}

void storage_shutdown(void) { rfs_unmount(); }

int storage_is_ram(void) { return on_ram; }

void storage_describe(char *buf, int n, int brief) {
    rfs_info_t info;
    rfs_info(&info);
    if (!rfs_mounted()) { strlcpy0(buf, "No disk", n); return; }
    char fr[24], tot[24];
    fmt_size(fr, sizeof fr, info.free_bytes > info.reserve_bytes ? info.free_bytes - info.reserve_bytes : 0);
    fmt_size(tot, sizeof tot, info.total_bytes);
    if (on_ram) ksnprintf(buf, n, brief ? "%s free (in memory)" : "In memory: %s free of %s. Files are lost at restart.", fr, tot);
    else ksnprintf(buf, n, brief ? "%s free of %s" : "Data disk \"%s\": %s free of %s", brief ? fr : info.label, brief ? tot : fr, tot);
}
