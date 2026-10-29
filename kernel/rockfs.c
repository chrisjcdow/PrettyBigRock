/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - RockFS version 1, following docs/ROCKFS.md */
#include "kernel.h"
#include "disk.h"
#include "rockfs.h"

#define BS        4096u
#define SPB       8u          /* sectors per block */
#define IN_INODE  7
#define PER_XB    170
#define INO_PER_B 16
#define TABLE_INO 0xFFFFFFFFu /* the inode table's own inode, kept in the superblock */
#define NAME_MAX_BYTES 1020

typedef struct __attribute__((packed)) {
    u16 type, flags;
    u32 parent;
    u64 size, created, modified;
    u32 generation;
    u16 count, depth;
    u8 res1[24];
    u8 entries[168];
    u8 res2[20];
    u32 checksum;
} rinode_t;

typedef struct __attribute__((packed)) {
    char magic[8];
    u16 version, block_shift, name_rule, pad;
    u64 block_count, bitmap_start, bitmap_blocks, data_start, free_blocks, reserve_blocks;
    u32 inode_count, free_inodes, root_inode, state;
    u32 f_compat, f_ro_compat, f_incompat, mount_count;
    u64 created, mounted;
    u8 uuid[16];
    char label[64];
    u8 res1[320];
    rinode_t table;
    u8 res2[252];
    u32 checksum;
} rsb_t;

typedef struct __attribute__((packed)) { u64 fb, db; u32 len, flags; } rext_t;
typedef struct __attribute__((packed)) { u64 fb, child; u8 res[8]; } ridx_t;
typedef struct __attribute__((packed)) { char magic[4]; u16 count, depth; u32 res, checksum; } rxhdr_t;
typedef struct __attribute__((packed)) { u32 inode; u16 elen, nlen; u8 type; u8 res[7]; } rdent_t;

_Static_assert(sizeof(rinode_t) == 256, "inode size");
_Static_assert(sizeof(rsb_t) == 1024, "superblock size");
_Static_assert(sizeof(rext_t) == 24 && sizeof(ridx_t) == 24, "entry size");
_Static_assert(sizeof(rxhdr_t) == 16 && sizeof(rdent_t) == 16, "header size");

/* an extent in memory; for index entries `db` holds the child block and len is 0 */
typedef struct { u64 fb, db; u32 len; } ext_t;
typedef struct { ext_t *v; u32 n, cap; } elist_t;
typedef struct { u64 *v; u32 n, cap; } blist_t;

int fs_generation;

static struct {
    int mounted, readonly, used_backup, was_dirty;
    blkdev_t *dev;
    u64 start, sectors;
    rsb_t sb;
    elist_t tab; /* where the inode table lives */
    u64 alloc_hint;
} V;

/* ---------------------------------------------------------------- small helpers */
static u32 crc_table[256];
static u32 crc32(const void *data, u32 len) {
    if (!crc_table[1])
        for (u32 i = 0; i < 256; i++) {
            u32 c = i;
            for (int k = 0; k < 8; k++) c = (c & 1) ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            crc_table[i] = c;
        }
    u32 c = 0xFFFFFFFFu;
    const u8 *p = data;
    while (len--) c = crc_table[(c ^ *p++) & 0xFF] ^ (c >> 8);
    return c ^ 0xFFFFFFFFu;
}

static int el_push(elist_t *l, u64 fb, u64 db, u32 len) {
    if (l->n == l->cap) {
        u32 cap = l->cap ? l->cap * 2 : 16;
        ext_t *v = kmalloc(cap * sizeof(ext_t));
        if (!v) return RFS_ENOMEM;
        if (l->v) { memcpy(v, l->v, l->n * sizeof(ext_t)); kfree(l->v); }
        l->v = v;
        l->cap = cap;
    }
    l->v[l->n].fb = fb;
    l->v[l->n].db = db;
    l->v[l->n].len = len;
    l->n++;
    return RFS_OK;
}
static int bl_push(blist_t *l, u64 b) {
    if (l->n == l->cap) {
        u32 cap = l->cap ? l->cap * 2 : 16;
        u64 *v = kmalloc(cap * sizeof(u64));
        if (!v) return RFS_ENOMEM;
        if (l->v) { memcpy(v, l->v, l->n * sizeof(u64)); kfree(l->v); }
        l->v = v;
        l->cap = cap;
    }
    l->v[l->n++] = b;
    return RFS_OK;
}
static void el_free(elist_t *l) { kfree(l->v); l->v = 0; l->n = l->cap = 0; }
static void bl_free(blist_t *l) { kfree(l->v); l->v = 0; l->n = l->cap = 0; }

static void el_sort_merge(elist_t *l) {
    for (u32 i = 1; i < l->n; i++) { /* insertion sort: lists are nearly sorted already */
        ext_t e = l->v[i];
        u32 j = i;
        while (j > 0 && l->v[j - 1].fb > e.fb) { l->v[j] = l->v[j - 1]; j--; }
        l->v[j] = e;
    }
    u32 o = 0;
    for (u32 i = 0; i < l->n; i++) {
        if (o && l->v[o - 1].fb + l->v[o - 1].len == l->v[i].fb && l->v[o - 1].db + l->v[o - 1].len == l->v[i].db
            && (u64)l->v[o - 1].len + l->v[i].len < 0xFFFFFFFFu)
            l->v[o - 1].len += l->v[i].len;
        else
            l->v[o++] = l->v[i];
    }
    l->n = o;
}

static u64 now_unix(void) {
    rtc_time_t t;
    rtc_read(&t);
    int y = t.year, m = t.mon, d = t.day;
    y -= m <= 2;
    int era = (y >= 0 ? y : y - 399) / 400;
    int yoe = y - era * 400;
    int doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    int doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i32 days = era * 146097 + doe - 719468;
    i32 secs = t.hour * 3600 + t.min * 60 + t.sec - tz_offset * 3600; /* the clock chip keeps UTC */
    return (u64)((long long)days * 86400 + secs);
}

static u32 tree_blocks_needed(u32 n) {
    if (n <= IN_INODE) return 0;
    u32 level = (n + PER_XB - 1) / PER_XB, total = level;
    while (level > IN_INODE) { level = (level + PER_XB - 1) / PER_XB; total += level; }
    return total;
}

const char *rfs_strerror(int e) {
    switch (e) {
    case RFS_OK: return "OK";
    case RFS_ENOENT: return "Not found";
    case RFS_EEXIST: return "Something with that name already exists";
    case RFS_ENOSPC: return "There isn't enough free space to save this safely. Delete something you don't need and try again";
    case RFS_ENOTDIR: return "That isn't a folder";
    case RFS_EISDIR: return "That's a folder";
    case RFS_ENOTEMPTY: return "The folder isn't empty";
    case RFS_EINVAL: return "Names can't contain / \\ : * ? \" < > | or end with a space or dot";
    case RFS_EIO: return "The disk reported an error";
    case RFS_EROFS: return "The disk can only be read, not changed";
    case RFS_ENODEV: return "There's no disk to save to";
    case RFS_ECORRUPT: return "Part of the disk is damaged. Run 'check' in the Terminal";
    case RFS_ENOMEM: return "Out of memory";
    }
    return "Unknown error";
}

/* ---------------------------------------------------------------- block access */
static u8 *blk_read(u64 b) {
    if (b >= V.sb.block_count) return 0;
    return cache_read(V.dev, V.start + b * SPB);
}
static int blk_write(u64 b, u32 off, const void *src, u32 len) {
    if (b >= V.sb.block_count) return RFS_ECORRUPT;
    return cache_write(V.dev, V.start + b * SPB, off, src, len) ? RFS_EIO : RFS_OK;
}

static int sb_write(int backup) {
    V.sb.checksum = crc32(&V.sb, 1020);
    if (backup) { /* the backup first, so one copy is always complete */
        int e = blk_write(V.sb.block_count - 1, 0, &V.sb, sizeof V.sb);
        if (e) return e;
    }
    return blk_write(0, 0, &V.sb, sizeof V.sb);
}

/* ---------------------------------------------------------------- bitmap */
static int bm_get(u64 n) {
    u8 *b = blk_read(V.sb.bitmap_start + (n >> 15));
    if (!b) return 1;
    return (b[(n >> 3) & 4095] >> (n & 7)) & 1;
}

/* Marks [start, start+len) used or free, writing each changed bitmap block once. */
static int bm_set(u64 start, u64 len, int val) {
    u8 *tmp = kmalloc(BS);
    if (!tmp) return RFS_ENOMEM;
    int err = RFS_OK;
    while (len && !err) {
        u64 bb = start >> 15;
        u64 first = start & 32767, n = 32768 - first;
        if (n > len) n = len;
        u8 *b = blk_read(V.sb.bitmap_start + bb);
        if (!b) { err = RFS_EIO; break; }
        memcpy(tmp, b, BS);
        for (u64 i = first; i < first + n; i++) {
            u8 m = 1 << (i & 7);
            if (!!(tmp[i >> 3] & m) == val) { err = RFS_ECORRUPT; break; }
            if (val) tmp[i >> 3] |= m; else tmp[i >> 3] &= ~m;
        }
        if (err) break;
        u32 lo = first >> 3, hi = ((first + n - 1) >> 3) + 1;
        err = blk_write(V.sb.bitmap_start + bb, lo, tmp + lo, hi - lo);
        if (val) V.sb.free_blocks -= n; else V.sb.free_blocks += n;
        start += n;
        len -= n;
    }
    kfree(tmp);
    return err;
}

static u64 count_free(void) {
    u64 free = 0;
    for (u64 bb = 0; bb < V.sb.bitmap_blocks; bb++) {
        u8 *b = blk_read(V.sb.bitmap_start + bb);
        if (!b) return 0;
        for (u32 i = 0; i < BS; i++) {
            u64 base = (bb << 15) + (i << 3);
            if (base >= V.sb.block_count) break;
            u8 v = ~b[i];
            for (int k = 0; k < 8 && base + k < V.sb.block_count; k++) free += (v >> k) & 1;
        }
    }
    return free;
}

/* Finds `count` free blocks, preferring one run, and marks them used. */
static int alloc_blocks(u64 count, int priv, elist_t *runs) {
    u64 avail = V.sb.free_blocks > (priv ? 0 : V.sb.reserve_blocks) ? V.sb.free_blocks - (priv ? 0 : V.sb.reserve_blocks) : 0;
    if (count > avail) return RFS_ENOSPC;
    u64 lo = V.sb.data_start, hi = V.sb.block_count - 1;
    u64 n = V.alloc_hint >= lo && V.alloc_hint < hi ? V.alloc_hint : lo;
    u64 need = count, scanned = 0;
    while (need && scanned < hi - lo) {
        if (!bm_get(n)) {
            u64 s = n;
            while (n < hi && !bm_get(n) && n - s < need) { n++; scanned++; }
            int e = el_push(runs, 0, s, (u32)(n - s));
            if (e) return e;
            need -= n - s;
            if (n >= hi) n = lo;
        } else {
            n++;
            scanned++;
            if (n >= hi) n = lo;
        }
    }
    if (need) { runs->n = 0; return RFS_ENOSPC; }
    for (u32 i = 0; i < runs->n; i++) {
        int e = bm_set(runs->v[i].db, runs->v[i].len, 1);
        if (e) return e;
    }
    V.alloc_hint = n;
    return RFS_OK;
}

static void free_runs(elist_t *runs) {
    for (u32 i = 0; i < runs->n; i++) bm_set(runs->v[i].db, runs->v[i].len, 0);
}

/* ---------------------------------------------------------------- extents */
static int read_xblock(u64 b, ext_t *out, u32 *count, u32 *depth) {
    u8 *raw = blk_read(b);
    if (!raw) return RFS_EIO;
    rxhdr_t h;
    memcpy(&h, raw, sizeof h);
    if (memcmp(h.magic, "RXTB", 4) || h.count > PER_XB) return RFS_ECORRUPT;
    u32 want = h.checksum;
    u8 *tmp = kmalloc(BS);
    if (!tmp) return RFS_ENOMEM;
    memcpy(tmp, raw, BS);
    ((rxhdr_t *)tmp)->checksum = 0;
    u32 got = crc32(tmp, BS);
    for (u32 i = 0; i < h.count; i++) {
        if (h.depth == 0) {
            rext_t e;
            memcpy(&e, tmp + 16 + i * 24, 24);
            out[i].fb = e.fb; out[i].db = e.db; out[i].len = e.len;
        } else {
            ridx_t e;
            memcpy(&e, tmp + 16 + i * 24, 24);
            out[i].fb = e.fb; out[i].db = e.child; out[i].len = 0;
        }
    }
    kfree(tmp);
    if (got != want) return RFS_ECORRUPT;
    *count = h.count;
    *depth = h.depth;
    return RFS_OK;
}

static void inode_entries(const rinode_t *in, ext_t *out) {
    for (u32 i = 0; i < in->count && i < IN_INODE; i++) {
        if (in->depth == 0) {
            rext_t e;
            memcpy(&e, in->entries + i * 24, 24);
            out[i].fb = e.fb; out[i].db = e.db; out[i].len = e.len;
        } else {
            ridx_t e;
            memcpy(&e, in->entries + i * 24, 24);
            out[i].fb = e.fb; out[i].db = e.child; out[i].len = 0;
        }
    }
}

static int walk_extents(const ext_t *ents, u32 count, u32 depth, elist_t *out, blist_t *tree, int level) {
    if (level > 8) return RFS_ECORRUPT;
    if (depth == 0) {
        for (u32 i = 0; i < count; i++) {
            int e = el_push(out, ents[i].fb, ents[i].db, ents[i].len);
            if (e) return e;
        }
        return RFS_OK;
    }
    ext_t *child = kmalloc(PER_XB * sizeof(ext_t));
    if (!child) return RFS_ENOMEM;
    int err = RFS_OK;
    for (u32 i = 0; i < count && !err; i++) {
        u32 cn, cd;
        if (tree && (err = bl_push(tree, ents[i].db))) break;
        if ((err = read_xblock(ents[i].db, child, &cn, &cd))) break;
        if (cd != depth - 1) { err = RFS_ECORRUPT; break; }
        err = walk_extents(child, cn, cd, out, tree, level + 1);
    }
    kfree(child);
    return err;
}

static int load_extents(const rinode_t *in, elist_t *out, blist_t *tree) {
    if (in->count > IN_INODE) return RFS_ECORRUPT;
    ext_t top[IN_INODE];
    inode_entries(in, top);
    return walk_extents(top, in->count, in->depth, out, tree, 0);
}

/* Where file block k lives, or 0 for a gap. */
static u64 map_block(const rinode_t *in, u64 k) {
    ext_t ents[PER_XB];
    u32 count = in->count, depth = in->depth;
    if (count > IN_INODE) return 0;
    inode_entries(in, ents);
    for (int level = 0; level < 9; level++) {
        int pick = -1;
        for (u32 i = 0; i < count; i++) {
            if (ents[i].fb <= k) pick = i; else break;
        }
        if (pick < 0) return 0;
        if (depth == 0) {
            ext_t *e = &ents[pick];
            return k < e->fb + e->len ? e->db + (k - e->fb) : 0;
        }
        u32 cn, cd;
        if (read_xblock(ents[pick].db, ents, &cn, &cd)) return 0;
        count = cn;
        depth = cd;
    }
    return 0;
}

/* ---------------------------------------------------------------- inodes */
static int inode_loc(u32 n, u64 *blk, u32 *off) {
    u64 fb = n / INO_PER_B;
    for (u32 i = 0; i < V.tab.n; i++) {
        ext_t *e = &V.tab.v[i];
        if (fb >= e->fb && fb < e->fb + e->len) {
            *blk = e->db + (fb - e->fb);
            *off = (n % INO_PER_B) * 256;
            return RFS_OK;
        }
    }
    return RFS_ECORRUPT;
}

static int reload_table(void) {
    el_free(&V.tab);
    return load_extents(&V.sb.table, &V.tab, 0);
}

static int read_inode(u32 n, rinode_t *out, int verify) {
    if (n == TABLE_INO) { *out = V.sb.table; return RFS_OK; }
    if (n == 0 || n >= V.sb.inode_count) return RFS_ECORRUPT;
    u64 b;
    u32 off;
    int e = inode_loc(n, &b, &off);
    if (e) return e;
    u8 *raw = blk_read(b);
    if (!raw) return RFS_EIO;
    memcpy(out, raw + off, 256);
    if (verify && out->type && crc32(out, 252) != out->checksum) return RFS_ECORRUPT;
    if (verify && out->count > IN_INODE) return RFS_ECORRUPT;
    return RFS_OK;
}

/* The commit point of every change. Inodes are 256 bytes and never straddle a sector,
   so this is a single sector write. */
static int write_inode(u32 n, rinode_t *in) {
    in->checksum = crc32(in, 252);
    if (n == TABLE_INO) {
        V.sb.table = *in;
        int e = sb_write(1);
        return e ? e : reload_table();
    }
    u64 b;
    u32 off;
    int e = inode_loc(n, &b, &off);
    if (e) return e;
    fs_generation++;
    return blk_write(b, off, in, 256);
}

/* ---------------------------------------------------------------- writing contents (copy-on-write) */
static int build_tree(elist_t *exts, int priv, rinode_t *in) {
    memset(in->entries, 0, sizeof in->entries);
    if (exts->n <= IN_INODE) {
        for (u32 i = 0; i < exts->n; i++) {
            rext_t e = {exts->v[i].fb, exts->v[i].db, exts->v[i].len, 0};
            memcpy(in->entries + i * 24, &e, 24);
        }
        in->count = exts->n;
        in->depth = 0;
        return RFS_OK;
    }
    u32 need = tree_blocks_needed(exts->n);
    elist_t runs = {0};
    int err = alloc_blocks(need, priv, &runs);
    if (err) { el_free(&runs); return err; }
    u32 ri = 0, rk = 0;
    elist_t level = *exts, upper = {0};
    int owned = 0;
    u32 depth = 0;
    u8 *buf = kmalloc(BS);
    if (!buf) { free_runs(&runs); el_free(&runs); return RFS_ENOMEM; }
    while (level.n > IN_INODE && !err) {
        upper.n = 0;
        for (u32 i = 0; i < level.n && !err; i += PER_XB) {
            u32 cnt = level.n - i < PER_XB ? level.n - i : PER_XB;
            u64 blk = runs.v[ri].db + rk;
            if (++rk == runs.v[ri].len) { ri++; rk = 0; }
            memset(buf, 0, BS);
            rxhdr_t h = {{'R', 'X', 'T', 'B'}, (u16)cnt, (u16)depth, 0, 0};
            for (u32 j = 0; j < cnt; j++) {
                ext_t *x = &level.v[i + j];
                if (depth == 0) {
                    rext_t e = {x->fb, x->db, x->len, 0};
                    memcpy(buf + 16 + j * 24, &e, 24);
                } else {
                    ridx_t e = {x->fb, x->db, {0}};
                    memcpy(buf + 16 + j * 24, &e, 24);
                }
            }
            memcpy(buf, &h, sizeof h);
            h.checksum = crc32(buf, BS);
            memcpy(buf, &h, sizeof h);
            err = blk_write(blk, 0, buf, BS);
            if (!err) err = el_push(&upper, level.v[i].fb, blk, 0);
        }
        if (owned) el_free(&level);
        level = upper;
        upper = (elist_t){0};
        owned = 1;
        depth++;
    }
    kfree(buf);
    if (!err) {
        for (u32 i = 0; i < level.n; i++) {
            ridx_t e = {level.v[i].fb, level.v[i].db, {0}};
            memcpy(in->entries + i * 24, &e, 24);
        }
        in->count = level.n;
        in->depth = depth;
    } else {
        free_runs(&runs);
    }
    if (owned) el_free(&level);
    el_free(&runs);
    return err;
}

/* Writes `len` bytes at `off` and sets the size to `new_size`, never overwriting a block the
   file is using: changed blocks go to new places, and writing the inode switches over. */
static int write_range(u32 n, rinode_t *in, u64 off, const u8 *data, u64 len, u64 new_size, int priv) {
    elist_t old = {0}, keep = {0}, freed = {0}, runs = {0};
    blist_t tree = {0};
    u8 *buf = 0;
    int err = load_extents(in, &old, &tree);
    if (err) goto out;
    u64 end_block = (new_size + BS - 1) / BS;
    int has = len > 0;
    u64 b0 = 0, b1 = 0;
    if (has) {
        b0 = off / BS;
        b1 = (off + len - 1) / BS;
        if (b0 >= end_block) has = 0;
        else if (b1 >= end_block) b1 = end_block - 1;
    }
    /* what survives: everything outside [b0, b1] and below the new end */
    for (u32 i = 0; i < old.n && !err; i++) {
        ext_t *e = &old.v[i];
        u64 s0 = e->fb, s1 = e->fb + e->len, pa[2], pb[2];
        int np = 0;
        if (has) {
            pa[0] = s0; pb[0] = s1 < b0 ? s1 : b0;
            pa[1] = s0 > b1 + 1 ? s0 : b1 + 1; pb[1] = s1;
            np = 2;
        } else {
            pa[0] = s0; pb[0] = s1;
            np = 1;
        }
        u64 cur = s0;
        for (int p = 0; p < np && !err; p++) {
            u64 a = pa[p], b = pb[p] < end_block ? pb[p] : end_block;
            if (b <= a) continue;
            if (a > cur) err = el_push(&freed, 0, e->db + (cur - s0), (u32)(a - cur));
            if (!err) err = el_push(&keep, a, e->db + (a - s0), (u32)(b - a));
            cur = b;
        }
        if (!err && cur < s1) err = el_push(&freed, 0, e->db + (cur - s0), (u32)(s1 - cur));
    }
    if (err) goto out;

    u64 count = has ? b1 - b0 + 1 : 0;
    if (count) {
        /* the tree size is only known once the blocks are found; build_tree fails cleanly if
           the real need doesn't fit */
        u64 est = count + tree_blocks_needed(keep.n + 1);
        u64 reserve = priv ? 0 : V.sb.reserve_blocks;
        if (V.sb.free_blocks < reserve || est > V.sb.free_blocks - reserve) { err = RFS_ENOSPC; goto out; }
        if ((err = alloc_blocks(count, priv, &runs))) goto out;
        buf = kmalloc(BS);
        if (!buf) { err = RFS_ENOMEM; goto undo; }
        u64 k = b0;
        for (u32 r = 0; r < runs.n && !err; r++) {
            for (u32 i = 0; i < runs.v[r].len && !err; i++, k++) {
                u64 lo = k * BS, hi = lo + BS;
                memset(buf, 0, BS);
                if (lo < off || hi > off + len) { /* partly changed: start from the old contents */
                    u64 ob = map_block(in, k);
                    if (ob) {
                        u8 *raw = blk_read(ob);
                        if (!raw) { err = RFS_EIO; break; }
                        memcpy(buf, raw, BS);
                    }
                }
                u64 s = lo > off ? lo : off, t = hi < off + len ? hi : off + len;
                memcpy(buf + (s - lo), data + (s - off), (u32)(t - s));
                if (hi > new_size) {
                    u64 cut = new_size > lo ? new_size - lo : 0;
                    memset(buf + cut, 0, (u32)(BS - cut));
                }
                u64 db = runs.v[r].db + i;
                if (!(err = blk_write(db, 0, buf, BS))) err = el_push(&keep, k, db, 1);
            }
        }
        if (err) goto undo;
    }
    el_sort_merge(&keep);
    rinode_t nin = *in;
    if ((err = build_tree(&keep, priv, &nin))) goto undo;
    nin.size = new_size;
    nin.modified = now_unix();
    if ((err = write_inode(n, &nin))) goto out;
    *in = nin;
    /* finally, release what the file no longer uses */
    for (u32 i = 0; i < freed.n; i++) bm_set(freed.v[i].db, freed.v[i].len, 0);
    for (u32 i = 0; i < tree.n; i++) bm_set(tree.v[i], 1, 0);
    goto out;
undo:
    free_runs(&runs);
out:
    kfree(buf);
    el_free(&old); el_free(&keep); el_free(&freed); el_free(&runs);
    bl_free(&tree);
    return err;
}

static int read_contents(const rinode_t *in, u8 *dst, u32 max) {
    u64 size = in->size < max ? in->size : max;
    for (u64 k = 0; k * BS < size; k++) {
        u32 n = size - k * BS < BS ? (u32)(size - k * BS) : BS;
        u64 db = map_block(in, k);
        if (!db) { memset(dst + k * BS, 0, n); continue; }
        u8 *raw = blk_read(db);
        if (!raw) return RFS_EIO;
        memcpy(dst + k * BS, raw, n);
    }
    return RFS_OK;
}

/* ---------------------------------------------------------------- names */
static int strchr_ok(const char *set, u8 c) {
    for (; *set; set++) if ((u8)*set == c) return 1;
    return 0;
}

int rfs_valid_name(const char *name) {
    u32 bytes = strlen(name), chars = 0;
    if (!bytes || bytes > NAME_MAX_BYTES) return 0;
    if (!strcmp(name, ".") || !strcmp(name, "..")) return 0;
    const u8 *p = (const u8 *)name;
    while (*p) {
        u8 c = *p;
        int extra = c < 0x80 ? 0 : (c & 0xE0) == 0xC0 ? 1 : (c & 0xF0) == 0xE0 ? 2 : (c & 0xF8) == 0xF0 ? 3 : -1;
        if (extra < 0) return 0;
        if (c < 32 || c == 127) return 0;
        if (c < 0x80 && strchr_ok("/\\:*?\"<>|", c)) return 0;
        p++;
        for (int i = 0; i < extra; i++, p++) if ((*p & 0xC0) != 0x80) return 0;
        chars++;
    }
    char last = name[bytes - 1];
    if (last == ' ' || last == '.') return 0;
    return chars <= 255;
}

static int name_eq(const char *a, u32 al, const char *b, u32 bl) {
    if (al != bl) return 0;
    for (u32 i = 0; i < al; i++) {
        u8 x = a[i], y = b[i];
        if (x >= 'A' && x <= 'Z') x += 32;
        if (y >= 'A' && y <= 'Z') y += 32;
        if (x != y) return 0;
    }
    return 1;
}

/* ---------------------------------------------------------------- folders */
typedef int (*dent_fn)(void *ctx, u64 k, u32 pos, const rdent_t *d, const char *name);

/* Calls fn for every name in folder `din`. fn returns nonzero to stop. */
static int dir_iter(const rinode_t *din, dent_fn fn, void *ctx, int *stopped) {
    if (din->type != RFS_DIR) return RFS_ENOTDIR;
    u8 *tmp = kmalloc(BS);
    if (!tmp) return RFS_ENOMEM;
    int err = RFS_OK;
    if (stopped) *stopped = 0;
    for (u64 k = 0; k < din->size / BS && !err; k++) {
        u64 db = map_block(din, k);
        if (!db) continue;
        u8 *raw = blk_read(db);
        if (!raw) { err = RFS_EIO; break; }
        memcpy(tmp, raw, BS);
        u32 pos = 0;
        while (pos < BS) {
            rdent_t d;
            memcpy(&d, tmp + pos, sizeof d);
            if (d.elen == 0 && pos == 0) break; /* never written */
            if (d.elen < 16 || (d.elen & 7) || pos + d.elen > BS || (d.inode && 16u + d.nlen > d.elen)) { err = RFS_ECORRUPT; break; }
            if (d.inode && fn(ctx, k, pos, &d, (const char *)tmp + pos + 16)) {
                if (stopped) *stopped = 1;
                kfree(tmp);
                return RFS_OK;
            }
            pos += d.elen;
        }
    }
    kfree(tmp);
    return err;
}

typedef struct { const char *name; u32 nlen; u32 ino; int type; } find_ctx_t;
static int find_fn(void *c, u64 k, u32 pos, const rdent_t *d, const char *name) {
    (void)k; (void)pos;
    find_ctx_t *f = c;
    if (!name_eq(f->name, f->nlen, name, d->nlen)) return 0;
    f->ino = d->inode;
    f->type = d->type;
    return 1;
}

static int dir_find(u32 d, const char *name, u32 *ino, int *type) {
    rinode_t din;
    int e = read_inode(d, &din, 1);
    if (e) return e;
    find_ctx_t f = {name, strlen(name), 0, 0};
    int stopped;
    if ((e = dir_iter(&din, find_fn, &f, &stopped))) return e;
    if (!stopped) return RFS_ENOENT;
    *ino = f.ino;
    if (type) *type = f.type;
    return RFS_OK;
}

static int dir_add(u32 d, const char *name, u32 ino, int type) {
    rinode_t din;
    int err = read_inode(d, &din, 1);
    if (err) return err;
    u32 nlen = strlen(name), need = (16 + nlen + 7) & ~7u;
    u8 *blk = kmalloc(BS);
    if (!blk) return RFS_ENOMEM;
    for (u64 k = 0; k < din.size / BS; k++) {
        u64 db = map_block(&din, k);
        memset(blk, 0, BS);
        if (db) {
            u8 *raw = blk_read(db);
            if (!raw) { kfree(blk); return RFS_EIO; }
            memcpy(blk, raw, BS);
        }
        u32 pos = 0;
        while (pos < BS) {
            rdent_t e;
            memcpy(&e, blk + pos, sizeof e);
            if (e.elen == 0) e.elen = BS - pos; /* a never-written block is one empty entry */
            if (e.elen < 16 || (e.elen & 7) || pos + e.elen > BS) { kfree(blk); return RFS_ECORRUPT; }
            u32 used = e.inode ? (16 + e.nlen + 7) & ~7u : 0, elen = e.elen;
            if (elen - used >= need) {
                if (e.inode) {
                    e.elen = used;
                    memcpy(blk + pos, &e, sizeof e);
                    pos += used;
                    elen -= used;
                }
                memset(blk + pos, 0, elen);
                rdent_t ne = {ino, (u16)elen, (u16)nlen, (u8)type, {0}};
                memcpy(blk + pos, &ne, sizeof ne);
                memcpy(blk + pos + 16, name, nlen);
                err = write_range(d, &din, k * BS, blk, BS, din.size, 0);
                kfree(blk);
                return err;
            }
            pos += elen;
        }
    }
    memset(blk, 0, BS);
    rdent_t ne = {ino, (u16)BS, (u16)nlen, (u8)type, {0}};
    memcpy(blk, &ne, sizeof ne);
    memcpy(blk + 16, name, nlen);
    err = write_range(d, &din, din.size, blk, BS, din.size + BS, 0);
    kfree(blk);
    return err;
}

static int dir_remove(u32 d, const char *name) {
    rinode_t din;
    int err = read_inode(d, &din, 1);
    if (err) return err;
    u32 nlen = strlen(name);
    u8 *blk = kmalloc(BS);
    if (!blk) return RFS_ENOMEM;
    for (u64 k = 0; k < din.size / BS; k++) {
        u64 db = map_block(&din, k);
        if (!db) continue;
        u8 *raw = blk_read(db);
        if (!raw) { kfree(blk); return RFS_EIO; }
        memcpy(blk, raw, BS);
        u32 pos = 0, prev = 0xFFFFFFFFu;
        while (pos < BS) {
            rdent_t e;
            memcpy(&e, blk + pos, sizeof e);
            if (e.elen == 0) break;
            if (e.elen < 16 || pos + e.elen > BS) { kfree(blk); return RFS_ECORRUPT; }
            if (e.inode && name_eq(name, nlen, (const char *)blk + pos + 16, e.nlen)) {
                u32 elen = e.elen;
                if (prev != 0xFFFFFFFFu) { /* join its space onto the entry before */
                    rdent_t p;
                    memcpy(&p, blk + prev, sizeof p);
                    p.elen += elen;
                    memcpy(blk + prev, &p, sizeof p);
                    memset(blk + pos, 0, elen);
                } else {
                    memset(blk + pos, 0, elen);
                    rdent_t empty = {0, (u16)elen, 0, 0, {0}};
                    memcpy(blk + pos, &empty, sizeof empty);
                }
                err = write_range(d, &din, k * BS, blk, BS, din.size, 1);
                kfree(blk);
                return err;
            }
            prev = pos;
            pos += e.elen;
        }
    }
    kfree(blk);
    return RFS_ENOENT;
}

/* ---------------------------------------------------------------- inode allocation */
static int alloc_inode(int type, u32 parent, u32 *out, rinode_t *in) {
    rinode_t t;
    u32 n = 0;
    for (u32 i = 2; i < V.sb.inode_count; i++) {
        if (read_inode(i, &t, 0)) continue;
        if (t.type == 0) { n = i; break; }
    }
    if (!n) { /* grow the inode table by one block of 16 */
        rinode_t tab = V.sb.table;
        u8 *zero = kmalloc(BS);
        if (!zero) return RFS_ENOMEM;
        int e = write_range(TABLE_INO, &tab, tab.size, zero, BS, tab.size + BS, 0);
        kfree(zero);
        if (e) return e;
        n = V.sb.inode_count;
        V.sb.inode_count += INO_PER_B;
        V.sb.free_inodes += INO_PER_B;
        if ((e = sb_write(1))) return e;
        if ((e = read_inode(n, &t, 0))) return e;
    }
    u32 gen = t.generation;
    memset(in, 0, sizeof *in);
    in->type = type;
    in->parent = parent;
    in->generation = gen;
    in->created = in->modified = now_unix();
    int e = write_inode(n, in);
    if (e) return e;
    V.sb.free_inodes--;
    *out = n;
    return RFS_OK;
}

static int free_inode(u32 n, rinode_t *in) {
    elist_t ex = {0};
    blist_t tree = {0};
    int err = load_extents(in, &ex, &tree);
    rinode_t dead;
    memset(&dead, 0, sizeof dead);
    dead.generation = in->generation + 1;
    if (!err) err = write_inode(n, &dead);
    if (!err) {
        for (u32 i = 0; i < ex.n; i++) bm_set(ex.v[i].db, ex.v[i].len, 0);
        for (u32 i = 0; i < tree.n; i++) bm_set(tree.v[i], 1, 0);
        V.sb.free_inodes++;
    }
    el_free(&ex);
    bl_free(&tree);
    return err;
}

/* ---------------------------------------------------------------- paths */
static int resolve(const char *path, u32 *out) {
    u32 n = V.sb.root_inode;
    char part[NAME_MAX_BYTES + 1];
    const char *p = path;
    while (*p) {
        while (*p == '/') p++;
        if (!*p) break;
        u32 len = 0;
        while (p[len] && p[len] != '/') len++;
        if (len > NAME_MAX_BYTES) return RFS_ENOENT;
        memcpy(part, p, len);
        part[len] = 0;
        p += len;
        if (!strcmp(part, ".")) continue;
        rinode_t in;
        int e = read_inode(n, &in, 1);
        if (e) return e;
        if (!strcmp(part, "..")) { n = in.parent; continue; }
        if (in.type != RFS_DIR) return RFS_ENOTDIR;
        if ((e = dir_find(n, part, &n, 0))) return e;
    }
    *out = n;
    return RFS_OK;
}

/* Splits "/a/b/name" into the folder's inode and "name". */
static int split(const char *path, u32 *parent, char *name) {
    u32 len = strlen(path);
    while (len && path[len - 1] == '/') len--;
    u32 s = len;
    while (s && path[s - 1] != '/') s--;
    if (s == len || len - s > NAME_MAX_BYTES) return RFS_EINVAL;
    memcpy(name, path + s, len - s);
    name[len - s] = 0;
    if (!rfs_valid_name(name)) return RFS_EINVAL;
    char dir[512];
    if (s >= sizeof dir) return RFS_EINVAL;
    memcpy(dir, path, s);
    dir[s] = 0;
    int e = resolve(dir, parent);
    if (e) return e;
    rinode_t in;
    if ((e = read_inode(*parent, &in, 1))) return e;
    return in.type == RFS_DIR ? RFS_OK : RFS_ENOTDIR;
}

int rfs_path_join(const char *cwd, const char *rel, char *out, u32 n) {
    char tmp[512];
    u32 len = 0;
    if (rel[0] != '/') {
        u32 cl = strlen(cwd);
        if (cl >= sizeof tmp - 2) return 0;
        memcpy(tmp, cwd, cl);
        len = cl;
        tmp[len++] = '/';
    }
    u32 rl = strlen(rel);
    if (len + rl >= sizeof tmp) return 0;
    memcpy(tmp + len, rel, rl + 1);
    /* rebuild, resolving "." and ".." */
    u32 o = 0;
    out[0] = 0;
    const char *p = tmp;
    while (*p) {
        while (*p == '/') p++;
        if (!*p) break;
        u32 l = 0;
        while (p[l] && p[l] != '/') l++;
        if (l == 1 && p[0] == '.') { p += l; continue; }
        if (l == 2 && p[0] == '.' && p[1] == '.') {
            while (o && out[o - 1] != '/') o--;
            if (o) o--;
            out[o] = 0;
            p += l;
            continue;
        }
        if (o + 1 + l + 1 > n) return 0;
        out[o++] = '/';
        memcpy(out + o, p, l);
        o += l;
        out[o] = 0;
        p += l;
    }
    if (!o) { if (n < 2) return 0; out[0] = '/'; out[1] = 0; }
    return 1;
}

/* ---------------------------------------------------------------- public file API */
#define NEED_MOUNT() do { if (!V.mounted) return RFS_ENODEV; } while (0)
#define NEED_WRITE() do { NEED_MOUNT(); if (V.readonly) return RFS_EROFS; } while (0)

int rfs_stat(const char *path, rfs_stat_t *st) {
    NEED_MOUNT();
    u32 n;
    int e = resolve(path, &n);
    if (e) return e;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    st->ino = n;
    st->type = in.type;
    st->size = in.size;
    st->created = in.created;
    st->modified = in.modified;
    return RFS_OK;
}

int rfs_read(const char *path, void *buf, u32 max, u32 *got) {
    NEED_MOUNT();
    u32 n;
    int e = resolve(path, &n);
    if (e) return e;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    if (in.type != RFS_FILE) return RFS_EISDIR;
    if ((e = read_contents(&in, buf, max))) return e;
    *got = in.size < max ? (u32)in.size : max;
    return RFS_OK;
}

static int create(const char *path, int type, const void *data, u32 len) {
    u32 parent, n, existing;
    char name[NAME_MAX_BYTES + 1];
    int e = split(path, &parent, name);
    if (e) return e;
    if (dir_find(parent, name, &existing, 0) == RFS_OK) return RFS_EEXIST;
    rinode_t in;
    if ((e = alloc_inode(type, parent, &n, &in))) return e;
    if (len) e = write_range(n, &in, 0, data, len, len, 0);
    if (!e) e = dir_add(parent, name, n, type);
    if (e) {
        rinode_t cur;
        if (!read_inode(n, &cur, 0)) free_inode(n, &cur);
        return e;
    }
    return RFS_OK;
}

int rfs_write(const char *path, const void *buf, u32 len) {
    NEED_WRITE();
    u32 n;
    int e = resolve(path, &n);
    if (e == RFS_ENOENT) return create(path, RFS_FILE, buf, len);
    if (e) return e;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    if (in.type != RFS_FILE) return RFS_EISDIR;
    return write_range(n, &in, 0, buf, len, len, 0);
}

int rfs_mkdir(const char *path) {
    NEED_WRITE();
    return create(path, RFS_DIR, 0, 0);
}

typedef struct { int any; } empty_ctx_t;
static int empty_fn(void *c, u64 k, u32 pos, const rdent_t *d, const char *name) {
    (void)k; (void)pos; (void)d; (void)name;
    ((empty_ctx_t *)c)->any = 1;
    return 1;
}

int rfs_remove(const char *path) {
    NEED_WRITE();
    u32 parent, n;
    char name[NAME_MAX_BYTES + 1];
    int e = split(path, &parent, name);
    if (e) return e;
    if ((e = dir_find(parent, name, &n, 0))) return e;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    if (in.type == RFS_DIR) {
        empty_ctx_t c = {0};
        int stopped;
        if ((e = dir_iter(&in, empty_fn, &c, &stopped))) return e;
        if (c.any) return RFS_ENOTEMPTY;
    }
    if ((e = dir_remove(parent, name))) return e;
    return free_inode(n, &in);
}

int rfs_rename(const char *from, const char *to) {
    NEED_WRITE();
    u32 sp, dp, n, x;
    char sname[NAME_MAX_BYTES + 1], dname[NAME_MAX_BYTES + 1];
    int e = split(from, &sp, sname);
    if (e) return e;
    if ((e = dir_find(sp, sname, &n, 0))) return e;
    if ((e = split(to, &dp, dname))) return e;
    int same = sp == dp && name_eq(sname, strlen(sname), dname, strlen(dname));
    if (!same && dir_find(dp, dname, &x, 0) == RFS_OK) return RFS_EEXIST;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    if (in.type == RFS_DIR) { /* a folder can't go inside itself */
        u32 p = dp;
        for (int guard = 0; guard < 4096; guard++) {
            if (p == n) return RFS_EINVAL;
            if (p == V.sb.root_inode) break;
            rinode_t pi;
            if ((e = read_inode(p, &pi, 1))) return e;
            p = pi.parent;
        }
    }
    if (same) { /* only the capitals change: rewrite the name in place */
        if ((e = dir_remove(sp, sname))) return e;
        return dir_add(dp, dname, n, in.type);
    }
    if ((e = dir_add(dp, dname, n, in.type))) return e;
    if ((e = read_inode(n, &in, 1))) return e;
    in.parent = dp;
    if ((e = write_inode(n, &in))) return e;
    return dir_remove(sp, sname);
}

typedef struct { rfs_dir_cb cb; void *ctx; } list_ctx_t;
static int list_fn(void *c, u64 k, u32 pos, const rdent_t *d, const char *name) {
    (void)k; (void)pos;
    list_ctx_t *l = c;
    char nm[NAME_MAX_BYTES + 1];
    u32 n = d->nlen > NAME_MAX_BYTES ? NAME_MAX_BYTES : d->nlen;
    memcpy(nm, name, n);
    nm[n] = 0;
    return l->cb(l->ctx, nm, d->inode, d->type);
}

int rfs_list(const char *path, rfs_dir_cb cb, void *ctx) {
    NEED_MOUNT();
    u32 n;
    int e = resolve(path, &n);
    if (e) return e;
    rinode_t in;
    if ((e = read_inode(n, &in, 1))) return e;
    list_ctx_t l = {cb, ctx};
    return dir_iter(&in, list_fn, &l, 0);
}

/* ---------------------------------------------------------------- checking and repair */
typedef struct {
    char *buf;
    u32 len, cap;
    int count, leaks; /* leaks: harmless leftovers from an unfinished save */
} report_t;

static void rep(report_t *r, const char *fmt, ...) {
    r->count++;
    if (!r->buf || r->len + 2 >= r->cap) return;
    va_list ap;
    va_start(ap, fmt);
    int n = kvsnprintf(r->buf + r->len, r->cap - r->len, fmt, ap);
    va_end(ap);
    if (n < 0) return;
    r->len += (u32)n < r->cap - r->len ? (u32)n : r->cap - r->len - 1;
    if (r->len + 1 < r->cap) { r->buf[r->len++] = '\n'; r->buf[r->len] = 0; }
}

typedef struct {
    u8 *used;    /* blocks really in use */
    u8 *ltype;   /* type of each inode that's in use (0 = free, 0xFF = damaged) */
    u32 *lparent;
    report_t *r;
    int quiet;
    u32 cap; /* room in ltype and lparent */
} scan_t;

static void claim(scan_t *s, u64 b, u32 who) {
    if (b < V.sb.data_start || b >= V.sb.block_count - 1) {
        if (!s->quiet) rep(s->r, "item %u uses block %u, outside the data area", who, (u32)b);
        return;
    }
    if (s->used[b >> 3] & (1 << (b & 7))) {
        if (!s->quiet) rep(s->r, "block %u is used by more than one item (one is %u)", (u32)b, who);
        return;
    }
    s->used[b >> 3] |= 1 << (b & 7);
}

static int claim_inode(scan_t *s, const rinode_t *in, u32 who) {
    elist_t ex = {0};
    blist_t tree = {0};
    int e = load_extents(in, &ex, &tree);
    if (!e) {
        u64 last = 0;
        for (u32 i = 0; i < ex.n; i++) {
            if (i && ex.v[i].fb < last && !s->quiet) rep(s->r, "item %u has overlapping extents", who);
            last = ex.v[i].fb + ex.v[i].len;
            for (u32 k = 0; k < ex.v[i].len; k++) claim(s, ex.v[i].db + k, who);
        }
        for (u32 i = 0; i < tree.n; i++) claim(s, tree.v[i], who);
    }
    el_free(&ex);
    bl_free(&tree);
    return e;
}

static void scan(scan_t *s) {
    u64 bytes = V.sb.bitmap_blocks * BS;
    memset(s->used, 0, bytes);
    s->used[0] |= 1;
    for (u64 i = 0; i < V.sb.bitmap_blocks; i++) { u64 b = V.sb.bitmap_start + i; s->used[b >> 3] |= 1 << (b & 7); }
    { u64 b = V.sb.block_count - 1; s->used[b >> 3] |= 1 << (b & 7); }
    for (u64 b = V.sb.block_count; b < bytes * 8; b++) s->used[b >> 3] |= 1 << (b & 7);
    claim_inode(s, &V.sb.table, 0);
    for (u32 n = 1; n < V.sb.inode_count && n < s->cap; n++) {
        rinode_t in;
        s->ltype[n] = 0;
        int e = read_inode(n, &in, 1);
        if (e == RFS_ECORRUPT) {
            if (!s->quiet) rep(s->r, "item %u is damaged", n);
            s->ltype[n] = 0xFF;
            continue;
        }
        if (e || in.type == 0) continue;
        if (in.type != RFS_FILE && in.type != RFS_DIR) {
            if (!s->quiet) rep(s->r, "item %u has an unknown type", n);
            s->ltype[n] = 0xFF;
            continue;
        }
        if (claim_inode(s, &in, n)) {
            if (!s->quiet) rep(s->r, "item %u has a damaged extent tree", n);
            s->ltype[n] = 0xFF;
            continue;
        }
        s->ltype[n] = in.type;
        s->lparent[n] = in.parent;
    }
}

static int write_bitmap(const u8 *used) {
    for (u64 i = 0; i < V.sb.bitmap_blocks; i++) {
        int e = blk_write(V.sb.bitmap_start + i, 0, used + i * BS, BS);
        if (e) return e;
    }
    V.sb.free_blocks = count_free();
    return RFS_OK;
}

typedef struct { u32 kind, dir, ino; char *name; } fix_t;
typedef struct {
    scan_t *s;
    u32 dir;
    u32 *queue, *qtail;
    u8 *seen;
    fix_t *fixes;
    u32 nfix, capfix;
} walk_t;

static void add_fix(walk_t *w, u32 kind, u32 dir, u32 ino, const char *name, u32 nlen) {
    if (w->nfix == w->capfix) {
        u32 cap = w->capfix ? w->capfix * 2 : 16;
        fix_t *v = kmalloc(cap * sizeof(fix_t));
        if (!v) return;
        if (w->fixes) { memcpy(v, w->fixes, w->nfix * sizeof(fix_t)); kfree(w->fixes); }
        w->fixes = v;
        w->capfix = cap;
    }
    fix_t *f = &w->fixes[w->nfix++];
    f->kind = kind; f->dir = dir; f->ino = ino; f->name = 0;
    if (name) {
        f->name = kmalloc(nlen + 1);
        if (f->name) { memcpy(f->name, name, nlen); f->name[nlen] = 0; }
    }
}

static int walk_fn(void *c, u64 k, u32 pos, const rdent_t *d, const char *name) {
    (void)k; (void)pos;
    walk_t *w = c;
    scan_t *s = w->s;
    u32 n = d->inode;
    if (n >= V.sb.inode_count || n >= s->cap || s->ltype[n] == 0 || s->ltype[n] == 0xFF) {
        rep(s->r, "a name in folder %u points at a missing or damaged item", w->dir);
        add_fix(w, 0, w->dir, n, name, d->nlen);
        return 0;
    }
    if (w->seen[n]) {
        rep(s->r, "item %u has a second name in folder %u", n, w->dir);
        add_fix(w, 0, w->dir, n, name, d->nlen);
        return 0;
    }
    w->seen[n] = 1;
    if (d->type != s->ltype[n]) rep(s->r, "a name in folder %u has the wrong type", w->dir);
    if (s->lparent[n] != w->dir) {
        rep(s->r, "item %u is in folder %u but says it's in %u", n, w->dir, s->lparent[n]);
        add_fix(w, 1, w->dir, n, 0, 0);
    }
    if (s->ltype[n] == RFS_DIR) *w->qtail++ = n;
    return 0;
}

/* Reports names in one folder that count as the same. */
typedef struct { char **names; u32 *lens; u32 n, cap; } dup_t;
static int dup_fn(void *c, u64 k, u32 pos, const rdent_t *d, const char *name) {
    (void)k; (void)pos;
    dup_t *u = c;
    if (u->n == u->cap) return 1;
    u->names[u->n] = kmalloc(d->nlen + 1);
    if (!u->names[u->n]) return 1;
    memcpy(u->names[u->n], name, d->nlen);
    u->lens[u->n++] = d->nlen;
    return 0;
}
static void dup_check(const rinode_t *din, u32 dir, report_t *r) {
    dup_t u = {kmalloc(4096 * sizeof(char *)), kmalloc(4096 * 4), 0, 4096};
    if (u.names && u.lens) {
        dir_iter(din, dup_fn, &u, 0);
        for (u32 i = 0; i < u.n; i++)
            for (u32 j = i + 1; j < u.n; j++)
                if (name_eq(u.names[i], u.lens[i], u.names[j], u.lens[j])) rep(r, "folder %u has two names that count as the same", dir);
        for (u32 i = 0; i < u.n; i++) kfree(u.names[i]);
    }
    kfree(u.names);
    kfree(u.lens);
}

int rfs_check(int repair, char *report, u32 report_len, int *problems, int *only_leftovers) {
    NEED_MOUNT();
    if (repair && V.readonly) return RFS_EROFS;
    report_t r = {report, 0, report_len, 0, 0};
    if (report && report_len) report[0] = 0;
    u32 ic = V.sb.inode_count;
    u32 cap = ic + 2 * INO_PER_B; /* making /lost+found can grow the inode table */
    scan_t s = {kmalloc(V.sb.bitmap_blocks * BS), kmalloc(cap), kmalloc(cap * 4), &r, 0, cap};
    u32 *queue = kmalloc(cap * 4);
    u8 *seen = kmalloc(cap);
    if (!s.used || !s.ltype || !s.lparent || !queue || !seen) {
        kfree(s.used); kfree(s.ltype); kfree(s.lparent); kfree(queue); kfree(seen);
        return RFS_ENOMEM;
    }
    if (V.used_backup) rep(&r, "the main superblock was damaged; the backup was used");
    scan(&s);
    walk_t w = {&s, 0, queue, queue, seen, 0, 0, 0};
    u32 root = V.sb.root_inode;
    int err = RFS_OK;
    if (root >= ic || s.ltype[root] != RFS_DIR) {
        rep(&r, "the top-level folder is missing or damaged");
        err = RFS_ECORRUPT;
        goto done;
    }
    seen[root] = 1;
    *w.qtail++ = root;
    for (u32 *q = queue; q < w.qtail; q++) {
        rinode_t din;
        w.dir = *q;
        if (read_inode(*q, &din, 1)) continue;
        if (dir_iter(&din, walk_fn, &w, 0)) rep(&r, "folder %u is damaged", *q);
        dup_check(&din, *q, &r);
    }
    u32 orphans = 0;
    for (u32 n = 1; n < ic; n++) if (s.ltype[n] && s.ltype[n] != 0xFF && !seen[n]) orphans++;
    if (orphans) { rep(&r, "%u item(s) belong to no folder", orphans); r.leaks++; }
    u32 wrong_used = 0, wrong_free = 0;
    for (u64 b = 0; b < V.sb.block_count; b++) {
        int a = (s.used[b >> 3] >> (b & 7)) & 1;
        if (a != bm_get(b)) { if (a) wrong_free++; else wrong_used++; }
    }
    if (wrong_free) rep(&r, "%u block(s) are in use but marked free", wrong_free);
    if (wrong_used) { rep(&r, "%u block(s) are marked used but belong to nothing", wrong_used); r.leaks++; }

    if (repair && r.count) {
        for (u32 n = 1; n < ic; n++) if (s.ltype[n] == 0xFF) { rinode_t z; memset(&z, 0, sizeof z); write_inode(n, &z); }
        for (u32 i = 0; i < w.nfix; i++) {
            fix_t *f = &w.fixes[i];
            if (f->kind == 0 && f->name) dir_remove(f->dir, f->name);
            if (f->kind == 1) {
                rinode_t in;
                if (!read_inode(f->ino, &in, 1)) { in.parent = f->dir; write_inode(f->ino, &in); }
            }
        }
        s.quiet = 1;
        scan(&s);
        write_bitmap(s.used);
        u32 keepers = 0;
        for (u32 n = 1; n < ic && n < cap; n++) {
            rinode_t oi;
            if (s.ltype[n] && s.ltype[n] != 0xFF && !seen[n] && read_inode(n, &oi, 1) == RFS_OK && oi.size) keepers++;
        }
        if (orphans) {
            u32 lf = 0;
            if (keepers && resolve("/lost+found", &lf) != RFS_OK) {
                create("/lost+found", RFS_DIR, 0, 0);
                resolve("/lost+found", &lf);
            }
            for (u32 n = 1; n < ic; n++) {
                if (!s.ltype[n] || s.ltype[n] == 0xFF || seen[n] || n == lf) continue;
                if (n >= cap) break;
                rinode_t oi;
                if (read_inode(n, &oi, 1) == RFS_OK && oi.size == 0) { /* an empty leftover: just remove it */
                    free_inode(n, &oi);
                    continue;
                }
                char nm[16];
                ksnprintf(nm, sizeof nm, "#%u", n);
                rinode_t in;
                if (dir_add(lf, nm, n, s.ltype[n]) == RFS_OK && read_inode(n, &in, 1) == RFS_OK) {
                    in.parent = lf;
                    write_inode(n, &in);
                }
            }
            scan(&s);
            write_bitmap(s.used);
        }
    }
    if (repair) {
        u32 free_inodes = 0;
        for (u32 n = 2; n < V.sb.inode_count; n++) {
            rinode_t in;
            if (!read_inode(n, &in, 0) && in.type == 0) free_inodes++;
        }
        V.sb.free_inodes = free_inodes;
        V.sb.free_blocks = count_free();
        V.sb.state = 1; /* still mounted; unmounting marks it clean */
        V.was_dirty = 0;
        sb_write(1);
        fs_generation++;
    }
done:
    for (u32 i = 0; i < w.nfix; i++) kfree(w.fixes[i].name);
    kfree(w.fixes);
    kfree(s.used); kfree(s.ltype); kfree(s.lparent); kfree(queue); kfree(seen);
    if (problems) *problems = r.count;
    if (only_leftovers) *only_leftovers = r.count && r.count == r.leaks;
    return err;
}

/* ---------------------------------------------------------------- format, find, mount */
static const u8 gpt_type[16] = {0x46, 0x14, 0x97, 0x03, 0x02, 0x6A, 0xDC, 0x47,
                                0xBB, 0x5F, 0x6A, 0x75, 0x72, 0xE5, 0x24, 0x7C};

int rfs_format(blkdev_t *dev, u64 start, u64 sectors, const char *label) {
    if (V.mounted && V.dev == dev) return RFS_EINVAL;
    u64 bc = sectors / SPB;
    if (bc < 64) return RFS_EINVAL;
    u64 bmb = (bc + 32767) / 32768, data_start = 1 + bmb, table_blk = data_start;
    u8 *b = kmalloc(BS);
    if (!b) return RFS_ENOMEM;
    cache_forget(dev);
    int err = 0;
    for (u64 i = 0; i < bmb && !err; i++) { /* the bitmap */
        memset(b, 0, BS);
        for (u64 k = 0; k < 32768; k++) {
            u64 n = i * 32768 + k;
            if (n < data_start || n == table_blk || n == bc - 1 || n >= bc) b[k >> 3] |= 1 << (k & 7);
        }
        err = cache_write(dev, start + (1 + i) * SPB, 0, b, BS);
    }
    u64 t = now_unix();
    rinode_t root;
    memset(&root, 0, sizeof root);
    root.type = RFS_DIR;
    root.parent = 1;
    root.created = root.modified = t;
    root.checksum = crc32(&root, 252);
    memset(b, 0, BS);
    memcpy(b + 256, &root, 256);
    if (!err) err = cache_write(dev, start + table_blk * SPB, 0, b, BS);
    rsb_t *sb = kmalloc(sizeof(rsb_t));
    if (!sb) { kfree(b); return RFS_ENOMEM; }
    memcpy(sb->magic, "ROCKFS!", 8);
    sb->version = 1;
    sb->block_shift = 12;
    sb->name_rule = 1;
    sb->block_count = bc;
    sb->bitmap_start = 1;
    sb->bitmap_blocks = bmb;
    sb->data_start = data_start;
    sb->free_blocks = bc - (data_start + 2);
    sb->reserve_blocks = bc / 100 > 16 ? bc / 100 : 16;
    sb->inode_count = INO_PER_B;
    sb->free_inodes = INO_PER_B - 2;
    sb->root_inode = 1;
    sb->created = t;
    for (int i = 0; i < 16; i++) sb->uuid[i] = (u8)krand();
    sb->uuid[6] = (sb->uuid[6] & 0x0F) | 0x40;
    sb->uuid[8] = (sb->uuid[8] & 0x3F) | 0x80;
    strlcpy0(sb->label, label, sizeof sb->label);
    sb->table.type = RFS_FILE;
    sb->table.size = BS;
    sb->table.created = sb->table.modified = t;
    sb->table.count = 1;
    rext_t e = {0, table_blk, 1, 0};
    memcpy(sb->table.entries, &e, sizeof e);
    sb->table.checksum = crc32(&sb->table, 252);
    sb->checksum = crc32(sb, 1020);
    if (!err) err = cache_write(dev, start + (bc - 1) * SPB, 0, sb, sizeof *sb);
    if (!err) err = cache_write(dev, start, 0, sb, sizeof *sb);
    kfree(sb);
    kfree(b);
    return err ? RFS_EIO : RFS_OK;
}

int rfs_partition_and_format(blkdev_t *dev, const char *label) {
    if (dev->is_boot) return RFS_EINVAL;
    if (dev->sectors < 2048 + 512) return RFS_EINVAL;
    u64 count = dev->sectors - 2048;
    if (count > 0xFFFFFFFFull) count = 0xFFFFFFFFull; /* the MBR's limit; GPT can come later */
    u8 mbr[512];
    memset(mbr, 0, sizeof mbr);
    u8 *p = mbr + 446;
    p[1] = p[2] = p[3] = 0xFF;
    p[4] = 0x7F; /* RockFS */
    p[5] = p[6] = p[7] = 0xFF;
    u32 s = 2048, c = (u32)count;
    memcpy(p + 8, &s, 4);
    memcpy(p + 12, &c, 4);
    u32 sig = krand();
    memcpy(mbr + 440, &sig, 4);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    cache_forget(dev);
    if (dev->write(dev, 0, 1, mbr) || dev->flush(dev)) return RFS_EIO;
    return rfs_format(dev, 2048, count, label);
}

int rfs_find_volume(blkdev_t *dev, u64 *start, u64 *sectors) {
    u8 s[512];
    if (dev->read(dev, 0, 1, s)) return RFS_VOL_NONE;
    if (!memcmp(s, "ROCKFS!", 8)) { *start = 0; *sectors = dev->sectors; return RFS_VOL_FOUND; }
    if (s[510] == 0x55 && s[511] == 0xAA) {
        for (int i = 0; i < 4; i++) {
            u8 *e = s + 446 + i * 16;
            u32 st, ct;
            memcpy(&st, e + 8, 4);
            memcpy(&ct, e + 12, 4);
            if (e[4] == 0x7F && ct) { *start = st; *sectors = ct; return RFS_VOL_FOUND; }
            if (e[4] == 0xEE) {
                u8 h[512];
                if (dev->read(dev, 1, 1, h) || memcmp(h, "EFI PART", 8)) return RFS_VOL_NONE;
                u64 lba;
                u32 num, esz;
                memcpy(&lba, h + 72, 8);
                memcpy(&num, h + 80, 4);
                memcpy(&esz, h + 84, 4);
                if (esz < 128 || esz > 512 || num > 1024) return RFS_VOL_NONE;
                u8 sec[512];
                for (u32 k = 0; k < num; k++) {
                    u64 at = lba * 512 + (u64)k * esz;
                    if (dev->read(dev, at / 512, 1, sec)) return RFS_VOL_NONE;
                    u8 *ge = sec + (at % 512);
                    if (!memcmp(ge, gpt_type, 16)) {
                        u64 first, last;
                        memcpy(&first, ge + 32, 8);
                        memcpy(&last, ge + 40, 8);
                        *start = first;
                        *sectors = last - first + 1;
                        return RFS_VOL_FOUND;
                    }
                }
                return RFS_VOL_NONE;
            }
        }
        return RFS_VOL_NONE;
    }
    for (u32 k = 0; k < 34; k++) { /* a blank disk is all zeroes at the start */
        if (k && dev->read(dev, k, 1, s)) return RFS_VOL_NONE;
        for (int i = 0; i < 512; i++) if (s[i]) return RFS_VOL_NONE;
    }
    return RFS_VOL_BLANK;
}

static int sb_ok(const rsb_t *sb) {
    return !memcmp(sb->magic, "ROCKFS!", 8) && crc32(sb, 1020) == sb->checksum;
}

int rfs_mount(blkdev_t *dev, u64 start, u64 sectors) {
    if (V.mounted) rfs_unmount();
    memset(&V, 0, sizeof V);
    V.dev = dev;
    V.start = start;
    V.sectors = sectors;
    cache_forget(dev);
    u8 *raw = cache_read(dev, start);
    if (!raw) return RFS_EIO;
    memcpy(&V.sb, raw, sizeof V.sb);
    if (!sb_ok(&V.sb)) {
        u64 last = sectors / SPB - 1;
        raw = cache_read(dev, start + last * SPB);
        if (!raw) return RFS_EIO;
        memcpy(&V.sb, raw, sizeof V.sb);
        if (!sb_ok(&V.sb)) return RFS_ECORRUPT;
        V.used_backup = 1;
    }
    if (V.sb.version != 1 || V.sb.block_shift != 12 || V.sb.f_incompat) return RFS_ECORRUPT;
    if (V.sb.block_count > sectors / SPB || V.sb.block_count < 64) return RFS_ECORRUPT;
    if (V.sb.f_ro_compat || V.sb.name_rule != 1) V.readonly = 1;
    int e = reload_table();
    if (e) return e;
    V.sb.free_blocks = count_free();
    V.was_dirty = V.sb.state != 0;
    V.mounted = 1;
    if (!V.readonly) {
        V.sb.state = 1;
        V.sb.mount_count++;
        V.sb.mounted = now_unix();
        if ((e = sb_write(V.used_backup))) { V.mounted = 0; return e; }
    }
    fs_generation++;
    return RFS_OK;
}

void rfs_unmount(void) {
    if (!V.mounted) return;
    if (!V.readonly) {
        V.sb.state = 0;
        sb_write(1);
    }
    V.mounted = 0;
    el_free(&V.tab);
    cache_forget(V.dev);
    fs_generation++;
}

int rfs_mounted(void) { return V.mounted; }

void rfs_info(rfs_info_t *o) {
    memset(o, 0, sizeof *o);
    if (!V.mounted) return;
    strlcpy0(o->label, V.sb.label, sizeof o->label);
    memcpy(o->uuid, V.sb.uuid, 16);
    o->total_bytes = V.sb.block_count * BS;
    o->free_bytes = V.sb.free_blocks * BS;
    o->reserve_bytes = V.sb.reserve_blocks * BS;
    o->items = V.sb.inode_count - 1 - V.sb.free_inodes;
    o->mount_count = V.sb.mount_count;
    o->readonly = V.readonly;
    o->was_dirty = V.was_dirty;
}
