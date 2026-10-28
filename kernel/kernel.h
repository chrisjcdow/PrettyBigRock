/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* FynbosOS - shared kernel header */
#pragma once
#include <stdint.h>
#include <stddef.h>
#include <stdarg.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef int8_t i8;
typedef int32_t i32;
typedef uint64_t u64;

#define OS_NAME    "FynbosOS"
#define OS_VERSION "1.0"

/* ---------------------------------------------------------------- port I/O */
static inline void outb(u16 p, u8 v) { __asm__ volatile("outb %0,%1" ::"a"(v), "Nd"(p)); }
static inline void outw(u16 p, u16 v) { __asm__ volatile("outw %0,%1" ::"a"(v), "Nd"(p)); }
static inline u8 inb(u16 p) { u8 r; __asm__ volatile("inb %1,%0" : "=a"(r) : "Nd"(p)); return r; }
static inline u16 inw(u16 p) { u16 r; __asm__ volatile("inw %1,%0" : "=a"(r) : "Nd"(p)); return r; }
static inline void io_wait(void) { outb(0x80, 0); }
static inline void cli(void) { __asm__ volatile("cli"); }
static inline void sti(void) { __asm__ volatile("sti"); }
static inline void hlt(void) { __asm__ volatile("hlt"); }

#define MIN(a, b) ((a) < (b) ? (a) : (b))
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define ABS(a) ((a) < 0 ? -(a) : (a))

/* ---------------------------------------------------------------- lib */
void *memset(void *d, int c, size_t n);
void *memcpy(void *d, const void *s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void memset32(u32 *d, u32 v, size_t n);
void memcpy32(u32 *d, const u32 *s, size_t n);
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
void strlcpy0(char *d, const char *s, size_t n);
int ksnprintf(char *buf, size_t n, const char *fmt, ...);
int kvsnprintf(char *buf, size_t n, const char *fmt, va_list ap);
int katoi(const char *s);
void *kmalloc(u32 size);
u32 heap_used(void);
u32 krand(void);
void ksrand(u32 s);
double eval_expr(const char *s, int *ok);
double parse_double(const char *s);
void fmt_double(char *buf, size_t n, double v);
int d2i(double v);

/* ---------------------------------------------------------------- interrupts / devices */
extern volatile u32 ticks; /* 100 Hz */
void idt_init(void);
int kbd_pop(u8 *sc);
int mouse_pop(int *dx, int *dy, int *buttons);
void mouse_init(void);

#define KEY_UP     0x100
#define KEY_DOWN   0x101
#define KEY_LEFT   0x102
#define KEY_RIGHT  0x103
#define KEY_HOME   0x104
#define KEY_END    0x105
#define KEY_DEL    0x106
#define KEY_PGUP   0x107
#define KEY_PGDN   0x108
#define KEY_SUPER  0x109
#define KEY_CTRL   0x1000 /* flag OR-ed with a lowercase letter */
int kbd_translate(u8 sc); /* returns key code or -1 */

typedef struct { int sec, min, hour, day, mon, year; } rtc_time_t;
void rtc_read(rtc_time_t *t);
extern int tz_offset;
u32 mem_total_mb(void);

void power_off(void);
void reboot(void);

/* ---------------------------------------------------------------- graphics */
typedef struct { int x, y, w, h; } rect_t;
extern int SW, SH, SBPP;
void gfx_init(void);
rect_t rect_isect(rect_t a, rect_t b);
rect_t rect_union(rect_t a, rect_t b);
int rect_has(rect_t r, int x, int y);
void gfx_set_clip(rect_t r);
rect_t gfx_get_clip(void);
void gfx_pixel(int x, int y, u32 c);
void gfx_fill(int x, int y, int w, int h, u32 c);
void gfx_fill_alpha(int x, int y, int w, int h, u32 c, int a);
void gfx_rect(int x, int y, int w, int h, u32 c);
void gfx_vgrad(int x, int y, int w, int h, u32 top, u32 bot);
void gfx_line(int x0, int y0, int x1, int y1, u32 c);
void gfx_circle(int cx, int cy, int r, u32 c);
void gfx_round(int x, int y, int w, int h, int r, u32 c);
void gfx_char(int x, int y, char ch, u32 c, int scale);
void gfx_text(int x, int y, const char *s, u32 c);
void gfx_text_bold(int x, int y, const char *s, u32 c);
void gfx_text_scaled(int x, int y, const char *s, u32 c, int scale);
int gfx_text_w(const char *s);
void gfx_blit(int x, int y, int w, int h, const u32 *src, int stride);
void gfx_icon(int id, int x, int y, int scale);
void gfx_cursor(int x, int y);
void gfx_present(rect_t r);
u32 color_lerp(u32 a, u32 b, int t, int tmax);
u32 *gfx_backbuffer(void);
int isqrt(int v);

#define CHAR_W 6
#define LINE_H 12

extern const u8 font_data[95][9];
extern const u32 icon_palette[];
extern const u8 icon_data[][256];
extern const char *cursor_data[17];

enum { ICON_TERMINAL, ICON_NOTEPAD, ICON_FILES, ICON_CALC, ICON_PAINT, ICON_SNAKE,
       ICON_ABOUT, ICON_PROTEA, ICON_POWER, ICON_RESTART };

/* ---------------------------------------------------------------- window manager */
#define TITLE_H 26
#define TASKBAR_H 38

enum { ME_DOWN = 1, ME_UP, ME_MOVE };
#define MB_DOUBLE 0x100

typedef struct window window_t;
struct window {
    int used, x, y, w, h, minimized, app;
    char title[48];
    void (*draw)(window_t *w, int cx, int cy, int cw, int ch);
    void (*key)(window_t *w, int key);
    void (*mouse)(window_t *w, int ev, int x, int y, int b);
    void (*tick)(window_t *w);
    void (*close)(window_t *w);
};

window_t *wm_create(const char *title, int cw, int ch, int app);
void wm_close(window_t *w);
void wm_focus(window_t *w);
window_t *wm_top(void);
window_t *wm_find_app(int app);
void win_invalidate(window_t *w);
void wm_dirty_all(void);
void wm_run(void);
void wm_init(void);
void wm_set_title(window_t *w, const char *t);
int win_cx(window_t *w);
int win_cy(window_t *w);
int win_cw(window_t *w);
int win_ch(window_t *w);
int wm_is_focused(window_t *w);
void wm_refresh_clock(void);

/* common widgets */
void ui_button(int x, int y, int w, int h, const char *label, int pressed);
void ui_panel(int x, int y, int w, int h);

/* ---------------------------------------------------------------- apps */
enum { APP_TERMINAL, APP_NOTEPAD, APP_FILES, APP_CALC, APP_PAINT, APP_SNAKE, APP_ABOUT, APP_COUNT };
extern const char *app_names[APP_COUNT];
extern const int app_icons[APP_COUNT];
void app_open(int app);
void notepad_open_file(const char *name);
void apps_init(void);

/* ---------------------------------------------------------------- ram filesystem */
#define FS_MAX_FILES 32
#define FS_MAX_SIZE 16000
#define FS_NAME_LEN 28
void fs_init(void);
int fs_find(const char *name);
int fs_create(const char *name);
int fs_write(int i, const char *data, int len);
int fs_append(int i, const char *data, int len);
int fs_delete(int i);
int fs_valid(int i);
const char *fs_name(int i);
const char *fs_data(int i);
int fs_size(int i);
int fs_count(void);
extern int fs_generation; /* bumps on any change, so views can refresh */
