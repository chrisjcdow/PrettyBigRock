/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - RockFS, the PrettyBigRock filesystem (see docs/ROCKFS.md) */
#pragma once
#include "kernel.h"
#include "disk.h"

enum {
    RFS_OK = 0,
    RFS_ENOENT = -1,     /* not found */
    RFS_EEXIST = -2,     /* already exists */
    RFS_ENOSPC = -3,     /* not enough space */
    RFS_ENOTDIR = -4,    /* not a folder */
    RFS_EISDIR = -5,     /* is a folder */
    RFS_ENOTEMPTY = -6,  /* folder not empty */
    RFS_EINVAL = -7,     /* bad name or request */
    RFS_EIO = -8,        /* the disk reported an error */
    RFS_EROFS = -9,      /* read-only */
    RFS_ENODEV = -10,    /* nothing mounted */
    RFS_ECORRUPT = -11,  /* damaged structure */
    RFS_ENOMEM = -12,    /* out of memory */
};

#define RFS_FILE 1
#define RFS_DIR  2

typedef struct { u32 ino; int type; u64 size, created, modified; } rfs_stat_t;

typedef struct {
    char label[64];
    u8 uuid[16];
    u64 total_bytes, free_bytes, reserve_bytes;
    u32 items, mount_count;
    int readonly, was_dirty;
} rfs_info_t;

typedef int (*rfs_dir_cb)(void *ctx, const char *name, u32 ino, int type);

/* finding volumes on a disk */
enum { RFS_VOL_NONE, RFS_VOL_FOUND, RFS_VOL_BLANK };
int rfs_find_volume(blkdev_t *dev, u64 *start, u64 *sectors);
int rfs_partition_and_format(blkdev_t *dev, const char *label);
int rfs_format(blkdev_t *dev, u64 start, u64 sectors, const char *label);

int rfs_mount(blkdev_t *dev, u64 start, u64 sectors);
void rfs_unmount(void);
int rfs_mounted(void);
void rfs_info(rfs_info_t *out);
const char *rfs_strerror(int e);
int rfs_valid_name(const char *name);

int rfs_stat(const char *path, rfs_stat_t *st);
int rfs_read(const char *path, void *buf, u32 max, u32 *got);
int rfs_write(const char *path, const void *buf, u32 len); /* creates or replaces a file */
int rfs_mkdir(const char *path);
int rfs_remove(const char *path);
int rfs_rename(const char *from, const char *to);
int rfs_list(const char *path, rfs_dir_cb cb, void *ctx);
int rfs_check(int repair, char *report, u32 report_len, int *problems, int *only_leftovers);

/* turns "a/../b" relative to cwd into a clean absolute path; returns 0 if it's too long */
int rfs_path_join(const char *cwd, const char *rel, char *out, u32 n);

extern int fs_generation; /* changes whenever anything on the disk changes */
