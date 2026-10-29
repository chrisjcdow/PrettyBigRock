/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - block devices */
#pragma once
#include "kernel.h"

#define BLK_SIZE 4096

typedef struct blkdev blkdev_t;
struct blkdev {
    u64 sectors;
    int (*read)(blkdev_t *d, u64 lba, u32 count, void *buf);
    int (*write)(blkdev_t *d, u64 lba, u32 count, const void *buf);
    int (*flush)(blkdev_t *d);
    void *priv;
    int is_boot, is_ram;
    char where[24];
    char model[41];
};

extern int disk_count;
void disk_init(void);
blkdev_t *disk_get(int i);
blkdev_t *ramdisk_create(u32 bytes);

u8 *cache_read(blkdev_t *dev, u64 lba);
int cache_write(blkdev_t *dev, u64 lba, u32 off, const void *src, u32 len);
void cache_forget(blkdev_t *dev);
