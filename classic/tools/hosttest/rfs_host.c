/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* Runs PrettyBigRock's RockFS code as an ordinary program, on a disk image file, so it can be
   tested quickly and checked by the independent Python tool (tools/rockfs.py).

   rfs_host IMAGE format
   rfs_host IMAGE stress SEED ROUNDS MANIFEST [CUT_AFTER_WRITES]
   rfs_host IMAGE verify MANIFEST
   rfs_host IMAGE check [repair] */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <fcntl.h>
#include "kernel.h"
#include "disk.h"
#include "rockfs.h"

/* ---- what the kernel normally provides ---- */
void *kmalloc(u32 size) { return calloc(1, size ? size : 1); }
void kfree(void *p) { free(p); }
int tz_offset = 0;
void rtc_read(rtc_time_t *t) { t->year = 2026; t->mon = 10; t->day = 29; t->hour = 12; t->min = 0; t->sec = 0; }
static unsigned long long rs = 88172645463325252ull;
u32 krand(void) { rs ^= rs << 13; rs ^= rs >> 7; rs ^= rs << 17; return (u32)rs; }
void strlcpy0(char *d, const char *s, size_t n) { if (!n) return; size_t i = 0; while (s[i] && i + 1 < n) { d[i] = s[i]; i++; } d[i] = 0; }
int kvsnprintf(char *b, size_t n, const char *f, va_list ap) { return vsnprintf(b, n, f, ap); }
int ksnprintf(char *b, size_t n, const char *f, ...) { va_list ap; va_start(ap, f); int r = vsnprintf(b, n, f, ap); va_end(ap); return r; }

/* ---- a disk image file, with an optional "power cut" ---- */
static int fd;
static long cut_after = -1, writes;
static int f_read(blkdev_t *d, u64 lba, u32 n, void *buf) { (void)d; return pread(fd, buf, n * 512, lba * 512) == (ssize_t)(n * 512) ? 0 : -1; }
static int f_write(blkdev_t *d, u64 lba, u32 n, const void *buf) {
    (void)d;
    for (u32 i = 0; i < n; i++) { /* sector by sector, like a real disk */
        if (cut_after >= 0 && writes >= cut_after) return -1;
        if (pwrite(fd, (const u8 *)buf + i * 512, 512, (lba + i) * 512) != 512) return -1;
        writes++;
    }
    return 0;
}
static int f_flush(blkdev_t *d) { (void)d; return 0; }

static u32 crc32b(const u8 *p, u32 n) {
    u32 c = 0xFFFFFFFFu;
    while (n--) { c ^= *p++; for (int k = 0; k < 8; k++) c = (c & 1) ? 0xEDB88320u ^ (c >> 1) : c >> 1; }
    return c ^ 0xFFFFFFFFu;
}

typedef struct { char path[96]; u8 *data; u32 len; int dir; int live; } item_t;
static item_t items[400];
static int nitems;
static u32 rnd(u32 n) { return krand() % n; }

static void fill(u8 *p, u32 n, u32 seed) { for (u32 i = 0; i < n; i++) { seed = seed * 1103515245u + 12345u; p[i] = seed >> 16; } }

static item_t *find(const char *p) { for (int i = 0; i < nitems; i++) if (items[i].live && !strcasecmp(items[i].path, p)) return &items[i]; return 0; }

static void die(const char *what, int e) { fprintf(stderr, "FAIL: %s: %s (%d)\n", what, rfs_strerror(e), e); exit(1); }

static void verify_all(void) {
    static u8 buf[300000];
    for (int i = 0; i < nitems; i++) {
        if (!items[i].live) continue;
        rfs_stat_t st;
        int e = rfs_stat(items[i].path, &st);
        if (e) die(items[i].path, e);
        if (items[i].dir) { if (st.type != RFS_DIR) die("expected a folder", 0); continue; }
        u32 got;
        if ((e = rfs_read(items[i].path, buf, sizeof buf, &got))) die(items[i].path, e);
        if (got != items[i].len || memcmp(buf, items[i].data, got)) { fprintf(stderr, "FAIL: contents differ: %s\n", items[i].path); exit(1); }
    }
}

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    fd = open(argv[1], O_RDWR);
    if (fd < 0) { perror(argv[1]); return 2; }
    blkdev_t dev = {0};
    dev.sectors = lseek(fd, 0, SEEK_END) / 512;
    dev.read = f_read; dev.write = f_write; dev.flush = f_flush;
    const char *cmd = argv[2];
    if (!strcmp(cmd, "format")) {
        int e = rfs_partition_and_format(&dev, "Data");
        if (e) die("format", e);
        return 0;
    }
    u64 start, sectors;
    if (rfs_find_volume(&dev, &start, &sectors) != RFS_VOL_FOUND) { fprintf(stderr, "no volume\n"); return 1; }
    int e = rfs_mount(&dev, start, sectors);
    if (e) die("mount", e);
    if (!strcmp(cmd, "check")) {
        static char rep[20000];
        int problems;
        e = rfs_check(argc > 3, rep, sizeof rep, &problems, 0);
        printf("%d problem(s)\n%s", problems, rep);
        rfs_unmount();
        return problems && argc <= 3 ? 3 : 0;
    }
    if (!strcmp(cmd, "verify")) { /* files listed by the Python tool */
        FILE *m = fopen(argv[3], "r");
        char path[300];
        unsigned len, crc;
        int n = 0;
        static u8 buf[2000000];
        while (fscanf(m, "%u %x %299[^\n]\n", &len, &crc, path) == 3) {
            u32 got;
            if ((e = rfs_read(path, buf, sizeof buf, &got))) die(path, e);
            if (got != len || crc32b(buf, got) != crc) { fprintf(stderr, "FAIL: %s differs\n", path); return 1; }
            n++;
        }
        printf("verified %d files written by the Python tool\n", n);
        /* and change some, for Python to check afterwards */
        rfs_mkdir("/From PrettyBigRock");
        rfs_write("/From PrettyBigRock/hello.txt", "Hello from the C side\n", 22);
        rfs_unmount();
        return 0;
    }
    if (!strcmp(cmd, "stress")) {
        u32 seed = atoi(argv[3]);
        int rounds = atoi(argv[4]);
        rs ^= seed * 2654435761u;
        if (argc > 6) cut_after = atol(argv[6]);
        static const u32 sizes[] = {0, 1, 100, 4095, 4096, 4097, 9000, 30000, 70000, 200000};
        char folders[12][96] = {""};
        int nf = 1;
        for (int r = 0; r < rounds; r++) {
            u32 op = rnd(100);
            int nfiles = 0;
            for (int i = 0; i < nitems; i++) nfiles += items[i].live && !items[i].dir;
            char path[96];
            if (op < 35 || !nfiles) {
                snprintf(path, sizeof path, "%s/File %u.txt", folders[rnd(nf)], rnd(60));
                item_t *it = find(path);
                if (it && it->dir) continue;
                u32 len = sizes[rnd(10)];
                u8 *d = malloc(len + 1);
                fill(d, len, krand());
                e = rfs_write(path, d, len);
                if (e == RFS_ENOSPC) { free(d); continue; }
                if (e) { free(d); if (cut_after >= 0) break; die(path, e); }
                if (!it) { it = &items[nitems++]; strcpy(it->path, path); it->live = 1; } else free(it->data);
                it->data = d; it->len = len;
            } else if (op < 55) { /* remove */
                int k; do k = rnd(nitems); while (!items[k].live || items[k].dir);
                e = rfs_remove(items[k].path);
                if (e) { if (cut_after >= 0) break; die(items[k].path, e); }
                items[k].live = 0;
            } else if (op < 65 && nf < 12) {
                snprintf(path, sizeof path, "%s/Folder %u", folders[rnd(nf)], rnd(30));
                if (find(path)) continue;
                e = rfs_mkdir(path);
                if (e == RFS_ENOSPC) continue;
                if (e) { if (cut_after >= 0) break; die(path, e); }
                item_t *it = &items[nitems++];
                strcpy(it->path, path); it->dir = 1; it->live = 1;
                strcpy(folders[nf++], path);
            } else if (op < 80) { /* rename into a random folder */
                int k; do k = rnd(nitems); while (!items[k].live || items[k].dir);
                snprintf(path, sizeof path, "%s/Moved %u.TXT", folders[rnd(nf)], rnd(40));
                if (find(path)) continue;
                e = rfs_rename(items[k].path, path);
                if (e == RFS_ENOSPC) continue;
                if (e) { if (cut_after >= 0) break; die(path, e); }
                strcpy(items[k].path, path);
            } else { /* overwrite part of a file is covered by rewriting; lookups ignore case */
                int k; do k = rnd(nitems); while (!items[k].live || items[k].dir);
                char up[96];
                strcpy(up, items[k].path);
                for (char *c = up; *c; c++) if (*c >= 'a' && *c <= 'z') *c -= 32;
                rfs_stat_t a, b;
                if (rfs_stat(up, &a) || rfs_stat(items[k].path, &b) || a.ino != b.ino) die("case-insensitive lookup", 0);
            }
            if (cut_after < 0 && r % 100 == 99) {
                verify_all();
                static char rep[4000];
                int problems;
                rfs_check(0, rep, sizeof rep, &problems, 0);
                if (problems) { fprintf(stderr, "FAIL: check after %d rounds:\n%s", r + 1, rep); return 1; }
            }
        }
        if (cut_after < 0) verify_all();
        FILE *m = fopen(argv[5], "w");
        for (int i = 0; i < nitems; i++)
            if (items[i].live) fprintf(m, "%s %u %08x %s\n", items[i].dir ? "D" : "F", items[i].len, items[i].dir ? 0 : crc32b(items[i].data, items[i].len), items[i].path);
        fclose(m);
        if (cut_after < 0) rfs_unmount(); /* a power cut leaves it "in use" */
        return 0;
    }
    return 2;
}
