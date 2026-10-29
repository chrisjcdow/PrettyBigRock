/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - disks: an ATA (IDE) driver, a RAM disk, and a block cache */
#include "kernel.h"
#include "disk.h"

/* ---------------------------------------------------------------- ATA, polled PIO */
#define ATA_DATA     0
#define ATA_ERROR    1
#define ATA_COUNT    2
#define ATA_LBA0     3
#define ATA_LBA1     4
#define ATA_LBA2     5
#define ATA_DRIVE    6
#define ATA_STATUS   7
#define ATA_CMD      7
#define ST_ERR  0x01
#define ST_DRQ  0x08
#define ST_DF   0x20
#define ST_BSY  0x80

typedef struct { u16 io, ctl; int slave, lba48; } ata_t;
static ata_t ata_units[4];
static blkdev_t disks[5];
int disk_count;

static void ata_delay(ata_t *a) { for (int i = 0; i < 4; i++) inb(a->ctl); }

static int ata_wait(ata_t *a, int want_drq) {
    for (u32 i = 0; i < 2000000; i++) {
        u8 s = inb(a->io + ATA_STATUS);
        if (s == 0xFF) return -1;
        if (s & ST_BSY) continue;
        if (s & (ST_ERR | ST_DF)) return -1;
        if (!want_drq || (s & ST_DRQ)) return 0;
    }
    return -1;
}

static void ata_select(ata_t *a, u64 lba, u32 count) {
    if (a->lba48) {
        outb(a->io + ATA_DRIVE, 0x40 | (a->slave << 4));
        ata_delay(a);
        outb(a->io + ATA_COUNT, (count >> 8) & 0xFF);
        outb(a->io + ATA_LBA0, (lba >> 24) & 0xFF);
        outb(a->io + ATA_LBA1, (lba >> 32) & 0xFF);
        outb(a->io + ATA_LBA2, (lba >> 40) & 0xFF);
        outb(a->io + ATA_COUNT, count & 0xFF);
        outb(a->io + ATA_LBA0, lba & 0xFF);
        outb(a->io + ATA_LBA1, (lba >> 8) & 0xFF);
        outb(a->io + ATA_LBA2, (lba >> 16) & 0xFF);
    } else {
        outb(a->io + ATA_DRIVE, 0xE0 | (a->slave << 4) | ((lba >> 24) & 0x0F));
        ata_delay(a);
        outb(a->io + ATA_COUNT, count & 0xFF);
        outb(a->io + ATA_LBA0, lba & 0xFF);
        outb(a->io + ATA_LBA1, (lba >> 8) & 0xFF);
        outb(a->io + ATA_LBA2, (lba >> 16) & 0xFF);
    }
}

static int ata_rw(blkdev_t *d, u64 lba, u32 count, void *buf, int write) {
    ata_t *a = d->priv;
    u16 *p = buf;
    while (count) {
        u32 n = count > 128 ? 128 : count;
        if (ata_wait(a, 0)) return -1;
        ata_select(a, lba, n);
        outb(a->io + ATA_CMD, write ? (a->lba48 ? 0x34 : 0x30) : (a->lba48 ? 0x24 : 0x20));
        for (u32 s = 0; s < n; s++) {
            ata_delay(a);
            if (ata_wait(a, 1)) return -1;
            if (write) for (int i = 0; i < 256; i++) outw(a->io + ATA_DATA, *p++);
            else for (int i = 0; i < 256; i++) *p++ = inw(a->io + ATA_DATA);
        }
        if (write) {
            ata_delay(a);
            if (ata_wait(a, 0)) return -1;
        }
        lba += n;
        count -= n;
    }
    return 0;
}

static int ata_read(blkdev_t *d, u64 lba, u32 count, void *buf) { return ata_rw(d, lba, count, buf, 0); }
static int ata_write(blkdev_t *d, u64 lba, u32 count, const void *buf) { return ata_rw(d, lba, count, (void *)buf, 1); }

/* Waits until the disk has really stored what was written, so writes happen in the order
   RockFS depends on. */
static int ata_flush(blkdev_t *d) {
    ata_t *a = d->priv;
    if (ata_wait(a, 0)) return -1;
    outb(a->io + ATA_DRIVE, 0xE0 | (a->slave << 4));
    ata_delay(a);
    outb(a->io + ATA_CMD, a->lba48 ? 0xEA : 0xE7);
    ata_delay(a);
    return ata_wait(a, 0);
}

static int ata_identify(ata_t *a, u16 *id) {
    outb(a->ctl, 0x02); /* no interrupts: we poll */
    outb(a->io + ATA_DRIVE, 0xA0 | (a->slave << 4));
    ata_delay(a);
    outb(a->io + ATA_COUNT, 0);
    outb(a->io + ATA_LBA0, 0);
    outb(a->io + ATA_LBA1, 0);
    outb(a->io + ATA_LBA2, 0);
    outb(a->io + ATA_CMD, 0xEC);
    ata_delay(a);
    u8 s = inb(a->io + ATA_STATUS);
    if (s == 0 || s == 0xFF) return -1;
    for (u32 i = 0; i < 1000000 && (inb(a->io + ATA_STATUS) & ST_BSY); i++);
    if (inb(a->io + ATA_LBA1) || inb(a->io + ATA_LBA2)) return -1; /* a CD drive, not a disk */
    for (u32 i = 0; i < 1000000; i++) {
        s = inb(a->io + ATA_STATUS);
        if (s & ST_ERR) return -1;
        if (s & ST_DRQ) break;
    }
    if (!(s & ST_DRQ)) return -1;
    for (int i = 0; i < 256; i++) id[i] = inw(a->io + ATA_DATA);
    return 0;
}

void disk_init(void) {
    static const u16 bases[2][2] = {{0x1F0, 0x3F6}, {0x170, 0x376}};
    static const char *where[4] = {"IDE 1 (first)", "IDE 1 (second)", "IDE 2 (first)", "IDE 2 (second)"};
    u16 id[256];
    const u8 *boot_mbr = (const u8 *)0x7C00; /* the boot sector the BIOS loaded is still here */
    u8 sector[512];
    for (int u = 0; u < 4; u++) {
        ata_t *a = &ata_units[u];
        a->io = bases[u / 2][0];
        a->ctl = bases[u / 2][1];
        a->slave = u & 1;
        if (inb(a->io + ATA_STATUS) == 0xFF) continue; /* no controller */
        if (ata_identify(a, id)) continue;
        u64 sectors = id[60] | ((u32)id[61] << 16);
        if (id[83] & (1 << 10)) {
            u64 s48 = (u64)id[100] | ((u64)id[101] << 16) | ((u64)id[102] << 32) | ((u64)id[103] << 48);
            if (s48 > sectors) { sectors = s48; a->lba48 = 1; }
        }
        if (!sectors) continue;
        blkdev_t *d = &disks[disk_count];
        memset(d, 0, sizeof *d);
        d->sectors = sectors;
        d->read = ata_read;
        d->write = ata_write;
        d->flush = ata_flush;
        d->priv = a;
        strlcpy0(d->where, where[u], sizeof d->where);
        for (int i = 0; i < 20; i++) { /* model name: byte pairs are swapped */
            d->model[i * 2] = id[27 + i] >> 8;
            d->model[i * 2 + 1] = id[27 + i] & 0xFF;
        }
        d->model[40] = 0;
        for (int i = 39; i >= 0 && d->model[i] == ' '; i--) d->model[i] = 0;
        if (ata_read(d, 0, 1, sector) == 0 && memcmp(sector, boot_mbr, 512) == 0) d->is_boot = 1;
        disk_count++;
    }
}

blkdev_t *disk_get(int i) { return i >= 0 && i < disk_count ? &disks[i] : 0; }

/* ---------------------------------------------------------------- RAM disk */
static int ram_read(blkdev_t *d, u64 lba, u32 count, void *buf) {
    if (lba + count > d->sectors) return -1;
    memcpy(buf, (u8 *)d->priv + lba * 512, count * 512);
    return 0;
}
static int ram_write(blkdev_t *d, u64 lba, u32 count, const void *buf) {
    if (lba + count > d->sectors) return -1;
    memcpy((u8 *)d->priv + lba * 512, buf, count * 512);
    return 0;
}
static int ram_flush(blkdev_t *d) { (void)d; return 0; }

blkdev_t *ramdisk_create(u32 bytes) {
    blkdev_t *d = &disks[4];
    void *mem = d->priv && d->sectors * 512 == bytes ? d->priv : kmalloc(bytes);
    if (!mem) return 0;
    memset(mem, 0, bytes);
    memset(d, 0, sizeof *d);
    d->sectors = bytes / 512;
    d->read = ram_read;
    d->write = ram_write;
    d->flush = ram_flush;
    d->priv = mem;
    d->is_ram = 1;
    strlcpy0(d->where, "memory", sizeof d->where);
    strlcpy0(d->model, "RAM disk", sizeof d->model);
    return d;
}

/* ---------------------------------------------------------------- block cache
   Write-through: every change goes straight to the disk, in the order it's made, so the
   cache never changes the order RockFS relies on. It only saves re-reading. */
#define CACHE_N 128
typedef struct { blkdev_t *dev; u64 lba; u32 used; int valid; u8 *data; } centry_t;
static centry_t cache[CACHE_N];
static u32 cache_clock;

static centry_t *cache_slot(blkdev_t *dev, u64 lba) {
    centry_t *victim = 0;
    for (int i = 0; i < CACHE_N; i++) {
        centry_t *c = &cache[i];
        if (c->valid && c->dev == dev && c->lba == lba) { c->used = ++cache_clock; return c; }
        if (!victim || !c->valid || (victim->valid && c->used < victim->used)) victim = c;
    }
    if (!victim->data) victim->data = kmalloc(BLK_SIZE);
    victim->valid = 0;
    victim->dev = dev;
    victim->lba = lba;
    victim->used = ++cache_clock;
    return victim;
}

/* Returns the 4 KiB at sector `lba`, or 0 on a read error. The pointer stays valid only until
   the next cache call. */
u8 *cache_read(blkdev_t *dev, u64 lba) {
    centry_t *c = cache_slot(dev, lba);
    if (!c->valid) {
        if (dev->read(dev, lba, BLK_SIZE / 512, c->data)) return 0;
        c->valid = 1;
    }
    return c->data;
}

/* Writes bytes [off, off+len) of the 4 KiB at `lba`, sending only the sectors that change,
   then waits for the disk to store them. */
int cache_write(blkdev_t *dev, u64 lba, u32 off, const void *src, u32 len) {
    centry_t *c = cache_slot(dev, lba);
    if (!c->valid && (off || len != BLK_SIZE)) {
        if (dev->read(dev, lba, BLK_SIZE / 512, c->data)) return -1;
    }
    memcpy(c->data + off, src, len);
    c->valid = 1;
    u32 s0 = off / 512, s1 = (off + len + 511) / 512;
    if (dev->write(dev, lba + s0, s1 - s0, c->data + s0 * 512)) { c->valid = 0; return -1; }
    return dev->flush(dev);
}

void cache_forget(blkdev_t *dev) {
    for (int i = 0; i < CACHE_N; i++) if (cache[i].dev == dev) cache[i].valid = 0;
}
