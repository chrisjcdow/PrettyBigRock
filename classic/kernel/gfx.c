/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - 2D graphics on a double-buffered linear framebuffer */
#include "kernel.h"

struct bootinfo { u32 fb; u16 w, h, pitch; u8 bpp; } __attribute__((packed));

int SW, SH, SBPP;
static u8 *fb;
static int pitch;
static u32 *back;
static rect_t clip;

u32 *gfx_backbuffer(void) { return back; }

void gfx_init(void) {
    struct bootinfo *bi = (struct bootinfo *)0x5000;
    fb = (u8 *)bi->fb;
    SW = bi->w;
    SH = bi->h;
    pitch = bi->pitch;
    SBPP = bi->bpp;
    back = kmalloc(SW * SH * 4);
    clip = (rect_t){0, 0, SW, SH};
}

rect_t rect_isect(rect_t a, rect_t b) {
    int x0 = MAX(a.x, b.x), y0 = MAX(a.y, b.y);
    int x1 = MIN(a.x + a.w, b.x + b.w), y1 = MIN(a.y + a.h, b.y + b.h);
    if (x1 <= x0 || y1 <= y0) return (rect_t){0, 0, 0, 0};
    return (rect_t){x0, y0, x1 - x0, y1 - y0};
}

rect_t rect_union(rect_t a, rect_t b) {
    if (a.w <= 0 || a.h <= 0) return b;
    if (b.w <= 0 || b.h <= 0) return a;
    int x0 = MIN(a.x, b.x), y0 = MIN(a.y, b.y);
    int x1 = MAX(a.x + a.w, b.x + b.w), y1 = MAX(a.y + a.h, b.y + b.h);
    return (rect_t){x0, y0, x1 - x0, y1 - y0};
}

int rect_has(rect_t r, int x, int y) { return x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h; }

void gfx_set_clip(rect_t r) { clip = rect_isect(r, (rect_t){0, 0, SW, SH}); }
rect_t gfx_get_clip(void) { return clip; }

int isqrt(int v) {
    if (v <= 0) return 0;
    int r = 0, b = 1 << 30;
    while (b > v) b >>= 2;
    while (b) {
        if (v >= r + b) { v -= r + b; r = (r >> 1) + b; }
        else r >>= 1;
        b >>= 2;
    }
    return r;
}

u32 color_lerp(u32 a, u32 b, int t, int tmax) {
    if (tmax <= 0) return a;
    int r = ((a >> 16) & 255) + (((int)((b >> 16) & 255) - (int)((a >> 16) & 255)) * t) / tmax;
    int g = ((a >> 8) & 255) + (((int)((b >> 8) & 255) - (int)((a >> 8) & 255)) * t) / tmax;
    int bl = (a & 255) + (((int)(b & 255) - (int)(a & 255)) * t) / tmax;
    return (r << 16) | (g << 8) | bl;
}

static inline u32 blend(u32 d, u32 s, u32 a) {
    u32 rb = (((s & 0xFF00FF) * a + (d & 0xFF00FF) * (256 - a)) >> 8) & 0xFF00FF;
    u32 g = (((s & 0x00FF00) * a + (d & 0x00FF00) * (256 - a)) >> 8) & 0x00FF00;
    return rb | g;
}

void gfx_pixel(int x, int y, u32 c) {
    if (rect_has(clip, x, y)) back[y * SW + x] = c;
}

void gfx_fill(int x, int y, int w, int h, u32 c) {
    rect_t r = rect_isect((rect_t){x, y, w, h}, clip);
    for (int j = 0; j < r.h; j++) memset32(back + (r.y + j) * SW + r.x, c, r.w);
}

void gfx_fill_alpha(int x, int y, int w, int h, u32 c, int a) {
    rect_t r = rect_isect((rect_t){x, y, w, h}, clip);
    for (int j = 0; j < r.h; j++) {
        u32 *p = back + (r.y + j) * SW + r.x;
        for (int i = 0; i < r.w; i++) p[i] = blend(p[i], c, a);
    }
}

void gfx_rect(int x, int y, int w, int h, u32 c) {
    gfx_fill(x, y, w, 1, c);
    gfx_fill(x, y + h - 1, w, 1, c);
    gfx_fill(x, y, 1, h, c);
    gfx_fill(x + w - 1, y, 1, h, c);
}

void gfx_vgrad(int x, int y, int w, int h, u32 top, u32 bot) {
    for (int j = 0; j < h; j++) gfx_fill(x, y + j, w, 1, color_lerp(top, bot, j, h - 1));
}

void gfx_line(int x0, int y0, int x1, int y1, u32 c) {
    int dx = ABS(x1 - x0), sx = x0 < x1 ? 1 : -1;
    int dy = -ABS(y1 - y0), sy = y0 < y1 ? 1 : -1;
    int err = dx + dy;
    for (;;) {
        gfx_pixel(x0, y0, c);
        if (x0 == x1 && y0 == y1) break;
        int e2 = 2 * err;
        if (e2 >= dy) { err += dy; x0 += sx; }
        if (e2 <= dx) { err += dx; y0 += sy; }
    }
}

void gfx_circle(int cx, int cy, int r, u32 c) {
    for (int dy = -r; dy <= r; dy++) {
        int dx = isqrt(r * r - dy * dy);
        gfx_fill(cx - dx, cy + dy, dx * 2 + 1, 1, c);
    }
}

void gfx_round(int x, int y, int w, int h, int r, u32 c) {
    if (r * 2 > h) r = h / 2;
    if (r * 2 > w) r = w / 2;
    for (int j = 0; j < h; j++) {
        int inset = 0;
        int d = -1;
        if (j < r) d = r - j;
        else if (j >= h - r) d = j - (h - r - 1);
        if (d > 0) inset = r - isqrt(r * r - d * d);
        gfx_fill(x + inset, y + j, w - inset * 2, 1, c);
    }
}

void gfx_char(int x, int y, char ch, u32 c, int scale) {
    if (ch < 32 || ch > 126) ch = '?';
    const u8 *g = font_data[ch - 32];
    for (int row = 0; row < 9; row++) {
        u8 bits = g[row];
        if (!bits) continue;
        for (int col = 0; col < 5; col++) {
            if (!(bits & (0x10 >> col))) continue;
            if (scale == 1) gfx_pixel(x + col, y + row, c);
            else gfx_fill(x + col * scale, y + row * scale, scale, scale, c);
        }
    }
}

void gfx_text_scaled(int x, int y, const char *s, u32 c, int scale) {
    for (; *s; s++, x += CHAR_W * scale) gfx_char(x, y, *s, c, scale);
}

void gfx_text(int x, int y, const char *s, u32 c) { gfx_text_scaled(x, y, s, c, 1); }

void gfx_text_bold(int x, int y, const char *s, u32 c) {
    for (; *s; s++, x += CHAR_W + 1) {
        gfx_char(x, y, *s, c, 1);
        gfx_char(x + 1, y, *s, c, 1);
    }
}

int gfx_text_w(const char *s) { return (int)strlen(s) * CHAR_W; }

void gfx_blit(int x, int y, int w, int h, const u32 *src, int stride) {
    rect_t r = rect_isect((rect_t){x, y, w, h}, clip);
    for (int j = 0; j < r.h; j++)
        memcpy32(back + (r.y + j) * SW + r.x, src + (r.y - y + j) * stride + (r.x - x), r.w);
}

void gfx_icon(int id, int x, int y, int scale) {
    const u8 *d = icon_data[id];
    for (int j = 0; j < 16; j++)
        for (int i = 0; i < 16; i++) {
            u8 c = d[j * 16 + i];
            if (c) gfx_fill(x + i * scale, y + j * scale, scale, scale, icon_palette[c]);
        }
}

void gfx_cursor(int x, int y) {
    for (int j = 0; j < 17; j++)
        for (int i = 0; i < 12; i++) {
            char c = cursor_data[j][i];
            if (c == 'X') gfx_pixel(x + i, y + j, 0x101014);
            else if (c == '#') gfx_pixel(x + i, y + j, 0xffffff);
        }
}

void gfx_present(rect_t r) {
    r = rect_isect(r, (rect_t){0, 0, SW, SH});
    if (!r.w) return;
    for (int j = 0; j < r.h; j++) {
        u32 *src = back + (r.y + j) * SW + r.x;
        u8 *dst = fb + (r.y + j) * pitch;
        if (SBPP == 32) {
            memcpy32((u32 *)dst + r.x, src, r.w);
        } else {
            dst += r.x * 3;
            for (int i = 0; i < r.w; i++) {
                u32 c = src[i];
                dst[0] = c; dst[1] = c >> 8; dst[2] = c >> 16;
                dst += 3;
            }
        }
    }
}
