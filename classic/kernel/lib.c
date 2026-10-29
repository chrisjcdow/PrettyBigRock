/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - small C library: memory, strings, formatting, maths */
#include "kernel.h"

void *memset(void *d, int c, size_t n) {
    void *r = d;
    __asm__ volatile("rep stosb" : "+D"(d), "+c"(n) : "a"(c) : "memory");
    return r;
}

void *memcpy(void *d, const void *s, size_t n) {
    void *r = d;
    __asm__ volatile("rep movsb" : "+D"(d), "+S"(s), "+c"(n) : : "memory");
    return r;
}

void *memmove(void *d, const void *s, size_t n) {
    u8 *dp = d;
    const u8 *sp = s;
    if (dp == sp || n == 0) return d;
    if (dp < sp) return memcpy(d, s, n);
    dp += n - 1;
    sp += n - 1;
    __asm__ volatile("std; rep movsb; cld" : "+D"(dp), "+S"(sp), "+c"(n) : : "memory");
    return d;
}

void memset32(u32 *d, u32 v, size_t n) {
    __asm__ volatile("rep stosl" : "+D"(d), "+c"(n) : "a"(v) : "memory");
}

void memcpy32(u32 *d, const u32 *s, size_t n) {
    __asm__ volatile("rep movsl" : "+D"(d), "+S"(s), "+c"(n) : : "memory");
}

int memcmp(const void *a, const void *b, size_t n) {
    const u8 *x = a, *y = b;
    for (; n; n--, x++, y++) if (*x != *y) return *x - *y;
    return 0;
}

size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) { a++; b++; }
    return (u8)*a - (u8)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    while (n && *a && *a == *b) { a++; b++; n--; }
    if (!n) return 0;
    return (u8)*a - (u8)*b;
}

void strlcpy0(char *d, const char *s, size_t n) {
    size_t i = 0;
    if (!n) return;
    while (s[i] && i + 1 < n) { d[i] = s[i]; i++; }
    d[i] = 0;
}

int katoi(const char *s) {
    int neg = 0, v = 0;
    while (*s == ' ') s++;
    if (*s == '-') { neg = 1; s++; } else if (*s == '+') s++;
    while (*s >= '0' && *s <= '9') v = v * 10 + (*s++ - '0');
    return neg ? -v : v;
}

/* ---------------------------------------------------------------- formatting */
typedef struct { char *buf; size_t n, pos; } out_t;

static void out_c(out_t *o, char c) {
    if (o->pos + 1 < o->n) o->buf[o->pos] = c;
    o->pos++;
}

static void out_num(out_t *o, u32 v, int base, int neg, int width, char pad) {
    char tmp[16];
    int i = 0;
    const char *dig = "0123456789abcdef";
    do { tmp[i++] = dig[v % base]; v /= base; } while (v);
    if (neg) width--;
    if (neg && pad == '0') out_c(o, '-');
    while (width-- > i) out_c(o, pad);
    if (neg && pad != '0') out_c(o, '-');
    while (i) out_c(o, tmp[--i]);
}

int kvsnprintf(char *buf, size_t n, const char *fmt, va_list ap) {
    out_t o = {buf, n, 0};
    for (; *fmt; fmt++) {
        if (*fmt != '%') { out_c(&o, *fmt); continue; }
        fmt++;
        char pad = ' ';
        int width = 0, left = 0;
        if (*fmt == '-') { left = 1; fmt++; }
        if (*fmt == '0') { pad = '0'; fmt++; }
        while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        switch (*fmt) {
        case 'd': { int v = va_arg(ap, int); out_num(&o, v < 0 ? (u32)(-v) : (u32)v, 10, v < 0, width, pad); break; }
        case 'u': out_num(&o, va_arg(ap, u32), 10, 0, width, pad); break;
        case 'x': out_num(&o, va_arg(ap, u32), 16, 0, width, pad); break;
        case 'c': out_c(&o, (char)va_arg(ap, int)); break;
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            int len = (int)strlen(s);
            if (!left) while (width-- > len) out_c(&o, ' ');
            while (*s) out_c(&o, *s++);
            if (left) while (width-- > len) out_c(&o, ' ');
            break;
        }
        case '%': out_c(&o, '%'); break;
        default: out_c(&o, '%'); out_c(&o, *fmt); break;
        }
    }
    if (n) buf[o.pos < n ? o.pos : n - 1] = 0;
    return (int)o.pos;
}

int ksnprintf(char *buf, size_t n, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = kvsnprintf(buf, n, fmt, ap);
    va_end(ap);
    return r;
}

/* ---------------------------------------------------------------- heap
   A first-fit allocator. Blocks sit one after another from HEAP_START, each with a small
   header; freed neighbours are joined back together. */
#define HEAP_START 0x00200000u
#define HEAP_MAGIC 0x524F434Bu /* "ROCK" */
typedef struct hblk { u32 size; u32 free; struct hblk *next; u32 magic; } hblk_t;
static hblk_t *heap_head;
static u32 heap_top, heap_limit, heap_inuse;

static void heap_init(void) {
    heap_head = 0;
    heap_top = HEAP_START;
    heap_limit = mem_total_mb() * 1024u * 1024u - 0x10000u;
    if (heap_limit < HEAP_START + 0x800000u) heap_limit = HEAP_START + 0x800000u;
}

void *kmalloc(u32 size) {
    if (!heap_top) heap_init();
    size = (size + 15) & ~15u;
    hblk_t *prev = 0;
    for (hblk_t *b = heap_head; b; prev = b, b = b->next) {
        if (!b->free) continue;
        while (b->next && b->next->free && (u8 *)b + sizeof(hblk_t) + b->size == (u8 *)b->next) {
            b->size += sizeof(hblk_t) + b->next->size; /* join with a free neighbour */
            b->next = b->next->next;
        }
        if (b->size < size) continue;
        if (b->size >= size + sizeof(hblk_t) + 64) { /* split off the rest */
            hblk_t *rest = (hblk_t *)((u8 *)b + sizeof(hblk_t) + size);
            rest->size = b->size - size - sizeof(hblk_t);
            rest->free = 1;
            rest->next = b->next;
            rest->magic = HEAP_MAGIC;
            b->next = rest;
            b->size = size;
        }
        b->free = 0;
        heap_inuse += b->size;
        memset(b + 1, 0, b->size);
        return b + 1;
    }
    if (heap_top + sizeof(hblk_t) + size > heap_limit) return 0;
    hblk_t *b = (hblk_t *)heap_top;
    heap_top += sizeof(hblk_t) + size;
    b->size = size;
    b->free = 0;
    b->next = 0;
    b->magic = HEAP_MAGIC;
    if (prev) prev->next = b; else heap_head = b;
    heap_inuse += size;
    memset(b + 1, 0, size);
    return b + 1;
}

void kfree(void *p) {
    if (!p) return;
    hblk_t *b = (hblk_t *)p - 1;
    if (b->magic != HEAP_MAGIC || b->free) return; /* not ours, or freed twice */
    b->free = 1;
    heap_inuse -= b->size;
}

u32 heap_used(void) { return heap_inuse; }

/* ---------------------------------------------------------------- random */
static u32 rnd_state = 0x1234567;
void ksrand(u32 s) { rnd_state = s ? s : 1; }
u32 krand(void) {
    rnd_state ^= rnd_state << 13;
    rnd_state ^= rnd_state >> 17;
    rnd_state ^= rnd_state << 5;
    return rnd_state;
}

/* ---------------------------------------------------------------- doubles */
/* truncate toward zero with the x87, avoiding libgcc helpers */
static long long d2ll(double x) {
    long long r;
    u16 cw, ncw;
    __asm__ volatile("fnstcw %0" : "=m"(cw));
    ncw = cw | 0x0C00;
    __asm__ volatile("fldcw %0" ::"m"(ncw));
    __asm__ volatile("fistpll %0" : "=m"(r) : "t"(x) : "st");
    __asm__ volatile("fldcw %0" ::"m"(cw));
    return r;
}

int d2i(double v) { return (int)d2ll(v); }

static double dtrunc(double v) {
    if (v > 9e15 || v < -9e15) return v;
    return (double)d2ll(v);
}

double parse_double(const char *s) {
    double v = 0, scale = 1;
    int neg = 0, frac = 0;
    while (*s == ' ') s++;
    if (*s == '-') { neg = 1; s++; }
    for (; *s; s++) {
        if (*s == '.') { if (frac) break; frac = 1; continue; }
        if (*s < '0' || *s > '9') break;
        if (frac) { scale /= 10; v += (*s - '0') * scale; }
        else v = v * 10 + (*s - '0');
    }
    return neg ? -v : v;
}

void fmt_double(char *buf, size_t n, double v) {
    if (v != v) { strlcpy0(buf, "Error", n); return; }
    if (v > 1e300 || v < -1e300) { strlcpy0(buf, "Error", n); return; }
    int neg = v < 0;
    if (neg) v = -v;
    if (v >= 1e15) {
        int e = 0;
        while (v >= 10) { v /= 10; e++; }
        u32 m = (u32)d2ll(v * 1e6 + 0.5);
        ksnprintf(buf, n, "%s%u.%06ue+%d", neg ? "-" : "", m / 1000000, m % 1000000, e);
        return;
    }
    double ip = dtrunc(v);
    double fr = v - ip;
    u32 fd = (u32)d2ll(fr * 1e9 + 0.5);
    if (fd >= 1000000000u) { fd -= 1000000000u; ip += 1; }
    u32 hi = (u32)d2ll(ip / 1e9);
    u32 lo = (u32)d2ll(ip - (double)hi * 1e9);
    char ibuf[32], fbuf[16];
    if (hi) ksnprintf(ibuf, sizeof ibuf, "%u%09u", hi, lo);
    else ksnprintf(ibuf, sizeof ibuf, "%u", lo);
    if (fd) {
        ksnprintf(fbuf, sizeof fbuf, "%09u", fd);
        int k = 8;
        while (k >= 0 && fbuf[k] == '0') fbuf[k--] = 0;
        ksnprintf(buf, n, "%s%s.%s", neg ? "-" : "", ibuf, fbuf);
    } else {
        ksnprintf(buf, n, "%s%s", (neg && (hi || lo)) ? "-" : "", ibuf);
    }
}

/* recursive-descent expression evaluator: + - * / % ^ and brackets */
typedef struct { const char *p; int ok; } ex_t;
static double ex_sum(ex_t *e);

static void ex_ws(ex_t *e) { while (*e->p == ' ') e->p++; }

static double ex_atom(ex_t *e) {
    ex_ws(e);
    if (*e->p == '-') { e->p++; return -ex_atom(e); }
    if (*e->p == '+') { e->p++; return ex_atom(e); }
    if (*e->p == '(') {
        e->p++;
        double v = ex_sum(e);
        ex_ws(e);
        if (*e->p == ')') e->p++; else e->ok = 0;
        return v;
    }
    if ((*e->p >= '0' && *e->p <= '9') || *e->p == '.') {
        double v = parse_double(e->p);
        while ((*e->p >= '0' && *e->p <= '9') || *e->p == '.') e->p++;
        return v;
    }
    e->ok = 0;
    return 0;
}

static double ex_pow(ex_t *e) {
    double b = ex_atom(e);
    ex_ws(e);
    if (*e->p == '^') {
        e->p++;
        double x = ex_pow(e);
        int n = d2i(x);
        if ((double)n != x) { e->ok = 0; return 0; }
        double r = 1;
        int k = n < 0 ? -n : n;
        while (k--) r *= b;
        return n < 0 ? 1 / r : r;
    }
    return b;
}

static double ex_prod(ex_t *e) {
    double v = ex_pow(e);
    for (;;) {
        ex_ws(e);
        char op = *e->p;
        if (op != '*' && op != '/' && op != '%' && op != 'x') return v;
        e->p++;
        double r = ex_pow(e);
        if (op == '*' || op == 'x') v *= r;
        else if (r == 0) { e->ok = 0; return 0; }
        else if (op == '/') v /= r;
        else v = v - r * dtrunc(v / r);
    }
}

static double ex_sum(ex_t *e) {
    double v = ex_prod(e);
    for (;;) {
        ex_ws(e);
        char op = *e->p;
        if (op != '+' && op != '-') return v;
        e->p++;
        double r = ex_prod(e);
        v = op == '+' ? v + r : v - r;
    }
}

double eval_expr(const char *s, int *ok) {
    ex_t e = {s, 1};
    double v = ex_sum(&e);
    ex_ws(&e);
    if (*e.p) e.ok = 0;
    *ok = e.ok;
    return v;
}
