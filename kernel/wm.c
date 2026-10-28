/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* FynbosOS - window manager, desktop, taskbar and start menu */
#include "kernel.h"

#define MAXWIN 16

static window_t wins[MAXWIN];
static window_t *zord[MAXWIN];
static int nz;
static rect_t dirty;
static u32 *wall;

static int mx, my, mbtn;
static window_t *drag_win, *cap_win;
static int drag_dx, drag_dy;
static int menu_open, menu_hover = -1;
static int sel_icon = -1;
static u32 last_click_t;
static int last_click_x = -100, last_click_y = -100;
static rtc_time_t now;

/* colours */
#define C_BODY      0xf5f2ec
#define C_TITLE_A   0x2f6b55
#define C_TITLE_B   0x22503e
#define C_TITLE_IA  0xaeb2ba
#define C_TITLE_IB  0x989da7
#define C_ACCENT    0xd2577a
#define C_BAR_A     0x2a2e3d
#define C_BAR_B     0x171a24

/* ---------------------------------------------------------------- geometry helpers */
int win_cx(window_t *w) { return w->x + 1; }
int win_cy(window_t *w) { return w->y + TITLE_H; }
int win_cw(window_t *w) { return w->w - 2; }
int win_ch(window_t *w) { return w->h - TITLE_H - 1; }

static rect_t win_outer(window_t *w) { return (rect_t){w->x - 2, w->y - 2, w->w + 10, w->h + 12}; }
static void mark(rect_t r) { dirty = rect_union(dirty, r); }
void wm_dirty_all(void) { dirty = (rect_t){0, 0, SW, SH}; }
void win_invalidate(window_t *w) { if (!w->minimized) mark(win_outer(w)); }

static rect_t cursor_rect(void) { return (rect_t){mx, my, 13, 18}; }
static rect_t taskbar_rect(void) { return (rect_t){0, SH - TASKBAR_H, SW, TASKBAR_H}; }

/* ---------------------------------------------------------------- start menu layout */
#define MENU_W 236
#define MENU_ITEM_H 30
#define MENU_BANNER 30
#define MENU_ITEMS (APP_COUNT + 2)

static int menu_h(void) { return MENU_ITEMS * MENU_ITEM_H + 10 + 12; }
static rect_t menu_rect(void) { return (rect_t){4, SH - TASKBAR_H - 4 - menu_h(), MENU_W, menu_h()}; }

static int menu_item_y(int i) {
    rect_t m = menu_rect();
    int y = m.y + 6 + i * MENU_ITEM_H;
    if (i >= APP_COUNT) y += 10; /* separator */
    return y;
}

static int menu_hit(int x, int y) {
    rect_t m = menu_rect();
    if (!rect_has(m, x, y) || x < m.x + MENU_BANNER) return -1;
    for (int i = 0; i < MENU_ITEMS; i++) {
        int iy = menu_item_y(i);
        if (y >= iy && y < iy + MENU_ITEM_H) return i;
    }
    return -1;
}

/* ---------------------------------------------------------------- desktop icons */
#define ICON_CELL_W 84
#define ICON_CELL_H 82

static rect_t icon_cell(int i) { return (rect_t){16, 16 + i * ICON_CELL_H, ICON_CELL_W, ICON_CELL_H - 6}; }

/* ---------------------------------------------------------------- wallpaper: Table Mountain at dusk */
static inline void wset(int x, int y, u32 c) { if (x >= 0 && y >= 0 && x < SW && y < SH) wall[y * SW + x] = c; }
static inline void wblend(int x, int y, u32 c, int a) {
    if (x < 0 || y < 0 || x >= SW || y >= SH) return;
    u32 d = wall[y * SW + x];
    wall[y * SW + x] = color_lerp(d, c, a, 255);
}

static const short ridge[][2] = {
    {0, 90}, {60, 120}, {130, 190}, {180, 270}, {205, 330}, {220, 345}, {240, 318}, {265, 330},
    {290, 372}, {315, 396}, {332, 402}, {598, 400}, {618, 382}, {650, 322}, {680, 252}, {710, 205},
    {735, 214}, {758, 258}, {772, 276}, {788, 270}, {805, 236}, {830, 182}, {870, 142}, {920, 120},
    {970, 102}, {1000, 96},
};
static const short far_ridge[][2] = {
    {0, 150}, {90, 170}, {160, 150}, {230, 175}, {330, 140}, {450, 160}, {560, 130}, {700, 155},
    {820, 120}, {900, 140}, {1000, 115},
};

static int profile(const short (*pts)[2], int n, int x) {
    int fx = x * 1000 / SW;
    for (int i = 0; i < n - 1; i++) {
        if (fx >= pts[i][0] && fx <= pts[i + 1][0]) {
            int span = pts[i + 1][0] - pts[i][0];
            int t = span ? (fx - pts[i][0]) * 1000 / span : 0;
            int h = pts[i][1] + (pts[i + 1][1] - pts[i][1]) * t / 1000;
            return h * SH / 1000;
        }
    }
    return 0;
}

static void make_wallpaper(void) {
    wall = kmalloc(SW * SH * 4);
    int hz = SH * 76 / 100;
    ksrand(20261028);

    /* sky */
    for (int y = 0; y < hz; y++) {
        u32 c;
        int mid = hz * 55 / 100;
        if (y < mid) c = color_lerp(0x161a36, 0x6a4a7c, y, mid);
        else c = color_lerp(0x6a4a7c, 0xf2a35e, y - mid, hz - mid);
        memset32(wall + y * SW, c, SW);
    }
    /* stars */
    for (int i = 0; i < SW / 6; i++) {
        int x = krand() % SW, y = krand() % (hz * 40 / 100);
        int a = 80 + krand() % 160;
        wblend(x, y, 0xffffff, a * (hz * 40 / 100 - y) / (hz * 40 / 100));
    }
    /* setting sun with a soft glow */
    int sx = SW * 91 / 100, sy = hz - SH * 15 / 100, sr = SH * 45 / 1000;
    for (int ring = sr * 4; ring > sr; ring -= 2) {
        int a = 6;
        for (int dy = -ring; dy <= ring; dy++) {
            int dx = isqrt(ring * ring - dy * dy);
            for (int x = sx - dx; x <= sx + dx; x++) if (sy + dy < hz) wblend(x, sy + dy, 0xffc27a, a);
        }
    }
    for (int dy = -sr; dy <= sr; dy++) {
        int dx = isqrt(sr * sr - dy * dy);
        for (int x = sx - dx; x <= sx + dx; x++) if (sy + dy < hz) wset(x, sy + dy, color_lerp(0xffe6b0, 0xffb066, dy + sr, sr * 2));
    }
    /* distant range */
    for (int x = 0; x < SW; x++) {
        int top = hz - profile(far_ridge, 11, x) * 55 / 100;
        for (int y = top; y < hz; y++) wset(x, y, color_lerp(0x8a5a7e, 0x9c6a80, y - top, hz - top));
    }
    /* the mountain, with a warm rim of light along the ridge */
    for (int x = 0; x < SW; x++) {
        int top = hz - profile(ridge, sizeof(ridge) / sizeof(ridge[0]), x);
        for (int y = top; y < hz; y++) wset(x, y, color_lerp(0x342a4c, 0x1d1a30, y - top, hz - top));
        wset(x, top, 0xc98a8a);
        wblend(x, top + 1, 0xc98a8a, 120);
        wblend(x, top + 2, 0xc98a8a, 50);
    }
    /* the "tablecloth" of cloud resting on the flat top, with soft wisps spilling over */
    int tx0 = SW * 325 / 1000, tx1 = SW * 605 / 1000;
    int ttop = hz - SH * 401 / 1000;
    int bump[64];
    for (int i = 0; i < 64; i++) bump[i] = 12 + (int)(krand() % 14);
    for (int x = tx0; x < tx1; x++) {
        int seg = (x - tx0) / 22, t = (x - tx0) % 22;
        int b = bump[seg % 64] + (bump[(seg + 1) % 64] - bump[seg % 64]) * t / 22;   /* puffy outline */
        int edge = MIN(x - tx0, tx1 - x);
        int taper = MIN(edge * 256 / 40, 256);
        int h = b * taper / 256;
        int spill = 8 * taper / 256;
        for (int k = -h; k <= spill; k++) {
            int a;
            if (k < 0) a = MIN(230, 230 * (h + k) * 3 / (h * 2 + 1)); /* dense body, soft top */
            else a = 230 - k * 210 / (spill + 1);                   /* wisps over the edge */
            a = a * taper / 256;
            if (a > 0) wblend(x, ttop + k, 0xf4ecf0, a);
        }
    }
    /* sea with reflections */
    for (int y = hz; y < SH; y++) memset32(wall + y * SW, color_lerp(0x37406a, 0x0e1226, y - hz, SH - hz), SW);
    for (int i = 0; i < SW / 2; i++) {
        int y = hz + 2 + krand() % (SH - hz - TASKBAR_H);
        int depth = y - hz;
        int len = 6 + krand() % 30;
        int cx = sx + (int)(krand() % (60 + depth * 3)) - (30 + depth * 3 / 2);
        if (krand() % 3 == 0) cx = krand() % SW;
        int a = 170 - depth * 170 / (SH - hz);
        for (int x = cx; x < cx + len; x++) wblend(x, y, 0xf7b56c, a);
    }
    wset(0, 0, wall[0]);
}

/* ---------------------------------------------------------------- windows */
window_t *wm_top(void) {
    for (int i = nz - 1; i >= 0; i--) if (!zord[i]->minimized) return zord[i];
    return 0;
}

int wm_is_focused(window_t *w) { return wm_top() == w; }

window_t *wm_find_app(int app) {
    for (int i = 0; i < MAXWIN; i++) if (wins[i].used && wins[i].app == app) return &wins[i];
    return 0;
}

void wm_set_title(window_t *w, const char *t) {
    strlcpy0(w->title, t, sizeof w->title);
    win_invalidate(w);
    mark(taskbar_rect());
}

window_t *wm_create(const char *title, int cw, int ch, int app) {
    window_t *w = 0;
    for (int i = 0; i < MAXWIN; i++) if (!wins[i].used) { w = &wins[i]; break; }
    if (!w) return 0;
    memset(w, 0, sizeof *w);
    w->used = 1;
    w->app = app;
    w->w = cw + 2;
    w->h = ch + TITLE_H + 1;
    int slot = nz % 6;
    w->x = (SW - w->w) / 2 - 90 + slot * 30;
    w->y = (SH - TASKBAR_H - w->h) / 2 - 50 + slot * 26;
    if (w->x < 110) w->x = 110;
    if (w->y < 8) w->y = 8;
    strlcpy0(w->title, title, sizeof w->title);
    zord[nz++] = w;
    wm_dirty_all();
    return w;
}

static void z_remove(window_t *w) {
    int j = 0;
    for (int i = 0; i < nz; i++) if (zord[i] != w) zord[j++] = zord[i];
    nz = j;
}

void wm_focus(window_t *w) {
    w->minimized = 0;
    z_remove(w);
    zord[nz++] = w;
    wm_dirty_all();
}

void wm_close(window_t *w) {
    if (w->close) w->close(w);
    if (drag_win == w) drag_win = 0;
    if (cap_win == w) cap_win = 0;
    z_remove(w);
    w->used = 0;
    wm_dirty_all();
}

static void wm_minimize(window_t *w) {
    w->minimized = 1;
    if (cap_win == w) cap_win = 0;
    wm_dirty_all();
}

/* ---------------------------------------------------------------- widgets */
void ui_button(int x, int y, int w, int h, const char *label, int pressed) {
    gfx_round(x, y, w, h, 4, pressed ? 0x8d929c : 0xb4b8c0);
    if (pressed) gfx_round(x + 1, y + 1, w - 2, h - 2, 3, 0xd9dbe0);
    else {
        rect_t old = gfx_get_clip();
        gfx_round(x + 1, y + 1, w - 2, h - 2, 3, 0xfdfdfd);
        gfx_set_clip(rect_isect(old, (rect_t){x, y + h / 2, w, h}));
        gfx_round(x + 1, y + 1, w - 2, h - 2, 3, 0xeceef1);
        gfx_set_clip(old);
    }
    int tw = gfx_text_w(label);
    gfx_text(x + (w - tw) / 2 + (pressed ? 1 : 0), y + (h - 9) / 2 + (pressed ? 1 : 0), label, 0x22252c);
}

void ui_panel(int x, int y, int w, int h) {
    gfx_fill(x, y, w, h, 0xffffff);
    gfx_rect(x, y, w, h, 0xa9adb6);
}

/* ---------------------------------------------------------------- drawing */
static void draw_window(window_t *w, int focused) {
    for (int i = 1; i <= 5; i++) gfx_fill_alpha(w->x + i, w->y + i + 2, w->w, w->h, 0x000000, 16);
    gfx_fill(w->x, w->y, w->w, w->h, C_BODY);
    if (focused) gfx_vgrad(w->x, w->y, w->w, TITLE_H, C_TITLE_A, C_TITLE_B);
    else gfx_vgrad(w->x, w->y, w->w, TITLE_H, C_TITLE_IA, C_TITLE_IB);
    gfx_fill(w->x + 1, w->y + 1, w->w - 2, 1, focused ? 0x4b8a72 : 0xc4c7cd);
    gfx_rect(w->x, w->y, w->w, w->h, focused ? 0x1d3b30 : 0x80858f);
    gfx_icon(app_icons[w->app], w->x + 6, w->y + 5, 1);

    rect_t old = gfx_get_clip();
    gfx_set_clip(rect_isect(old, (rect_t){w->x, w->y, w->w - 50, TITLE_H}));
    gfx_text_bold(w->x + 28, w->y + 9, w->title, focused ? 0xffffff : 0x3d414a);
    gfx_set_clip(old);

    int bx = w->x + w->w - 22, by = w->y + 5;
    gfx_round(bx, by, 16, 16, 8, focused ? 0xe25c5c : 0xc5c8ce);
    gfx_line(bx + 5, by + 5, bx + 10, by + 10, 0xffffff);
    gfx_line(bx + 10, by + 5, bx + 5, by + 10, 0xffffff);
    gfx_line(bx + 6, by + 5, bx + 10, by + 9, 0xffffff);
    gfx_line(bx + 9, by + 5, bx + 5, by + 9, 0xffffff);
    bx -= 22;
    gfx_round(bx, by, 16, 16, 8, focused ? 0xe7b44a : 0xc5c8ce);
    gfx_fill(bx + 4, by + 7, 8, 2, 0xffffff);

    rect_t cr = {win_cx(w), win_cy(w), win_cw(w), win_ch(w)};
    gfx_set_clip(rect_isect(old, cr));
    if (w->draw) w->draw(w, cr.x, cr.y, cr.w, cr.h);
    gfx_set_clip(old);
}

static void draw_icons(void) {
    for (int i = 0; i < APP_COUNT; i++) {
        rect_t c = icon_cell(i);
        if (i == sel_icon) {
            gfx_fill_alpha(c.x, c.y, c.w, c.h, 0xffffff, 60);
            gfx_rect(c.x, c.y, c.w, c.h, 0xf3d9e2);
        }
        gfx_icon(app_icons[i], c.x + (c.w - 48) / 2, c.y + 6, 3);
        const char *l = app_names[i];
        int tx = c.x + (c.w - gfx_text_w(l)) / 2;
        gfx_text(tx + 1, c.y + 61, l, 0x101018);
        gfx_text(tx, c.y + 60, l, 0xffffff);
    }
}

static void draw_taskbar(void) {
    int y = SH - TASKBAR_H;
    gfx_vgrad(0, y, SW, TASKBAR_H, C_BAR_A, C_BAR_B);
    gfx_fill(0, y, SW, 1, 0x464c63);

    /* start button */
    gfx_round(6, y + 5, 104, TASKBAR_H - 10, 6, menu_open ? 0xa83d5d : C_ACCENT);
    gfx_icon(ICON_PROTEA, 12, y + 11, 1);
    gfx_text_bold(34, y + 15, "Fynbos", 0xffffff);

    /* one button per open window */
    int bx = 120;
    int bw = 160;
    int count = 0;
    for (int i = 0; i < MAXWIN; i++) if (wins[i].used) count++;
    int avail = SW - 120 - 120;
    if (count && count * (bw + 6) > avail) bw = avail / count - 6;
    window_t *top = wm_top();
    for (int i = 0; i < MAXWIN; i++) {
        window_t *w = &wins[i];
        if (!w->used) continue;
        u32 bg = w == top ? 0x48506c : (w->minimized ? 0x232633 : 0x323748);
        gfx_round(bx, y + 5, bw, TASKBAR_H - 10, 5, bg);
        if (w == top) gfx_fill(bx + 8, y + TASKBAR_H - 7, bw - 16, 2, C_ACCENT);
        gfx_icon(app_icons[w->app], bx + 7, y + 11, 1);
        rect_t old = gfx_get_clip();
        gfx_set_clip(rect_isect(old, (rect_t){bx, y, bw - 6, TASKBAR_H}));
        gfx_text(bx + 29, y + 15, w->title, w->minimized ? 0x8d93a6 : 0xeef0f6);
        gfx_set_clip(old);
        bx += bw + 6;
    }

    /* clock */
    char t[16], d[24];
    static const char *mons[] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    ksnprintf(t, sizeof t, "%02d:%02d", now.hour, now.min);
    ksnprintf(d, sizeof d, "%d %s %d", now.day, mons[(now.mon + 11) % 12], now.year);
    int cx = SW - 104;
    gfx_text_bold(cx + (98 - (int)strlen(t) * 7) / 2, y + 8, t, 0xffffff);
    gfx_text(cx + (98 - gfx_text_w(d)) / 2, y + 22, d, 0xa9afc2);
}

static void draw_menu(void) {
    rect_t m = menu_rect();
    for (int i = 1; i <= 5; i++) gfx_fill_alpha(m.x + i, m.y + i + 2, m.w, m.h, 0, 16);
    gfx_round(m.x, m.y, m.w, m.h, 6, 0xf7f4ef);
    gfx_vgrad(m.x, m.y + 4, MENU_BANNER, m.h - 8, 0xd2577a, 0x6a2a52);
    gfx_fill(m.x + 4, m.y, MENU_BANNER - 4, 4, 0xd2577a);
    gfx_fill(m.x + 4, m.y + m.h - 4, MENU_BANNER - 4, 4, 0x6a2a52);
    gfx_round(m.x, m.y, 10, 10, 5, 0xd2577a);
    gfx_round(m.x, m.y + m.h - 10, 10, 10, 5, 0x6a2a52);
    /* vertical "FYNBOS" on the banner */
    const char *v = "FYNBOS";
    int vl = (int)strlen(v);
    for (int i = 0; i < vl; i++) gfx_char(m.x + 12, m.y + m.h - 14 - (vl - i) * 12, v[i], 0xffffff, 1);
    gfx_icon(ICON_PROTEA, m.x + 7, m.y + 8, 1);
    for (int i = 0; i < MENU_ITEMS; i++) {
        int iy = menu_item_y(i);
        int ix = m.x + MENU_BANNER + 4;
        if (i == menu_hover) gfx_round(ix, iy + 1, m.w - MENU_BANNER - 8, MENU_ITEM_H - 2, 4, 0xe9dde3);
        int icon;
        const char *label;
        if (i < APP_COUNT) { icon = app_icons[i]; label = app_names[i]; }
        else if (i == APP_COUNT) { icon = ICON_RESTART; label = "Restart"; }
        else { icon = ICON_POWER; label = "Shut down"; }
        gfx_icon(icon, ix + 6, iy + 7, 1);
        gfx_text(ix + 30, iy + 11, label, 0x23262e);
    }
    int sy = menu_item_y(APP_COUNT) - 6;
    gfx_fill(m.x + MENU_BANNER + 10, sy, m.w - MENU_BANNER - 20, 1, 0xd5d0d2);
}

static void render(void) {
    rect_t d = rect_isect(dirty, (rect_t){0, 0, SW, SH});
    dirty = (rect_t){0, 0, 0, 0};
    if (!d.w) return;
    gfx_set_clip(d);
    gfx_blit(0, 0, SW, SH, wall, SW);
    draw_icons();
    window_t *top = wm_top();
    for (int i = 0; i < nz; i++) if (!zord[i]->minimized) draw_window(zord[i], zord[i] == top);
    draw_taskbar();
    if (menu_open) draw_menu();
    gfx_cursor(mx, my);
    gfx_present(d);
}

/* ---------------------------------------------------------------- input */
static void shutdown_screen(void) {
    gfx_set_clip((rect_t){0, 0, SW, SH});
    gfx_fill(0, 0, SW, SH, 0x12141c);
    gfx_icon(ICON_PROTEA, SW / 2 - 48, SH / 2 - 120, 6);
    const char *a = "FynbosOS has shut down.";
    const char *b = "You can close the virtual machine window now.";
    gfx_text_scaled((SW - gfx_text_w(a) * 2) / 2, SH / 2 + 2, a, 0xffffff, 2);
    gfx_text((SW - gfx_text_w(b)) / 2, SH / 2 + 36, b, 0xa9afc2);
    gfx_present((rect_t){0, 0, SW, SH});
}

static void menu_activate(int i) {
    menu_open = 0;
    menu_hover = -1;
    wm_dirty_all();
    if (i < 0) return;
    if (i < APP_COUNT) { app_open(i); return; }
    if (i == APP_COUNT) { reboot(); return; }
    shutdown_screen();
    power_off();
    for (;;) { cli(); hlt(); }
}

static void toggle_menu(void) {
    menu_open = !menu_open;
    menu_hover = -1;
    wm_dirty_all();
}

static void taskbar_click(int dbl) {
    (void)dbl;
    int y = SH - TASKBAR_H;
    if (mx >= 6 && mx < 110 && my >= y + 5) { toggle_menu(); return; }
    int bx = 120, bw = 160, count = 0;
    for (int i = 0; i < MAXWIN; i++) if (wins[i].used) count++;
    int avail = SW - 120 - 120;
    if (count && count * (bw + 6) > avail) bw = avail / count - 6;
    for (int i = 0; i < MAXWIN; i++) {
        window_t *w = &wins[i];
        if (!w->used) continue;
        if (mx >= bx && mx < bx + bw) {
            if (w == wm_top()) wm_minimize(w);
            else wm_focus(w);
            return;
        }
        bx += bw + 6;
    }
}

static void on_press(void) {
    int dbl = (ticks - last_click_t) < 45 && ABS(mx - last_click_x) < 5 && ABS(my - last_click_y) < 5;
    last_click_t = ticks;
    last_click_x = mx;
    last_click_y = my;
    if (dbl) last_click_t = 0; /* a third click starts over */

    if (menu_open) {
        if (rect_has(menu_rect(), mx, my)) { menu_activate(menu_hit(mx, my)); return; }
        int on_start = my >= SH - TASKBAR_H && mx < 110;
        menu_open = 0;
        wm_dirty_all();
        if (on_start) return;
    }
    if (my >= SH - TASKBAR_H) { taskbar_click(dbl); return; }

    for (int i = nz - 1; i >= 0; i--) {
        window_t *w = zord[i];
        if (w->minimized || !rect_has((rect_t){w->x, w->y, w->w, w->h}, mx, my)) continue;
        if (w != wm_top()) wm_focus(w);
        if (my < w->y + TITLE_H) {
            int bx = w->x + w->w - 22;
            if (mx >= bx && mx < bx + 16 && my >= w->y + 5 && my < w->y + 21) { wm_close(w); return; }
            bx -= 22;
            if (mx >= bx && mx < bx + 16 && my >= w->y + 5 && my < w->y + 21) { wm_minimize(w); return; }
            drag_win = w;
            drag_dx = mx - w->x;
            drag_dy = my - w->y;
        } else if (rect_has((rect_t){win_cx(w), win_cy(w), win_cw(w), win_ch(w)}, mx, my)) {
            cap_win = w;
            if (w->mouse) w->mouse(w, ME_DOWN, mx - win_cx(w), my - win_cy(w), 1 | (dbl ? MB_DOUBLE : 0));
        }
        return;
    }

    int hit = -1;
    for (int i = 0; i < APP_COUNT; i++) if (rect_has(icon_cell(i), mx, my)) hit = i;
    if (hit != sel_icon) { sel_icon = hit; mark((rect_t){0, 0, 120, SH}); }
    if (hit >= 0 && dbl) app_open(hit);
}

static void on_release(void) {
    drag_win = 0;
    if (cap_win) {
        window_t *w = cap_win;
        cap_win = 0;
        if (w->mouse) w->mouse(w, ME_UP, mx - win_cx(w), my - win_cy(w), 0);
    }
}

static void on_mouse(int dx, int dy, int b) {
    /* gentle acceleration for fast movements */
    if (ABS(dx) > 6) dx = dx * 3 / 2;
    if (ABS(dy) > 6) dy = dy * 3 / 2;
    int ox = mx, oy = my;
    mark(cursor_rect());
    mx = MIN(MAX(mx + dx, 0), SW - 1);
    my = MIN(MAX(my + dy, 0), SH - 1);
    mark(cursor_rect());
    if (mx != ox || my != oy) {
        if (drag_win) {
            mark(win_outer(drag_win));
            drag_win->x = MIN(MAX(mx - drag_dx, 60 - drag_win->w), SW - 60);
            drag_win->y = MIN(MAX(my - drag_dy, 0), SH - TASKBAR_H - TITLE_H);
            mark(win_outer(drag_win));
        } else if (cap_win) {
            if (cap_win->mouse) cap_win->mouse(cap_win, ME_MOVE, mx - win_cx(cap_win), my - win_cy(cap_win), b);
        } else if (menu_open) {
            int h = menu_hit(mx, my);
            if (h != menu_hover) { menu_hover = h; mark(menu_rect()); }
        }
    }
    int pressed = (b & 1) && !(mbtn & 1);
    int released = !(b & 1) && (mbtn & 1);
    mbtn = b;
    if (pressed) on_press();
    if (released) on_release();
}

static void on_key(int k) {
    if (k == KEY_SUPER || k == (KEY_CTRL | 27)) { toggle_menu(); return; }
    if (menu_open) {
        if (k == 27) toggle_menu();
        else if (k == KEY_UP) { menu_hover = menu_hover <= 0 ? MENU_ITEMS - 1 : menu_hover - 1; mark(menu_rect()); }
        else if (k == KEY_DOWN) { menu_hover = (menu_hover + 1) % MENU_ITEMS; mark(menu_rect()); }
        else if (k == '\n' && menu_hover >= 0) menu_activate(menu_hover);
        return;
    }
    window_t *w = wm_top();
    if (k == (KEY_CTRL | 'w') && w) { wm_close(w); return; }
    if (w && w->key) w->key(w, k);
}

/* ---------------------------------------------------------------- main loop */
void wm_init(void) {
    make_wallpaper();
    mx = SW / 2;
    my = SH / 2;
    rtc_read(&now);
    wm_dirty_all();
}

void wm_run(void) {
    u32 last_tick = ticks, last_rtc = 0;
    for (;;) {
        u8 sc;
        while (kbd_pop(&sc)) {
            int k = kbd_translate(sc);
            if (k >= 0) on_key(k);
        }
        int dx, dy, b;
        while (mouse_pop(&dx, &dy, &b)) on_mouse(dx, dy, b);
        if (ticks != last_tick) {
            last_tick = ticks;
            for (int i = 0; i < MAXWIN; i++)
                if (wins[i].used && wins[i].tick) wins[i].tick(&wins[i]);
            if (ticks - last_rtc >= 50) {
                last_rtc = ticks;
                rtc_time_t t;
                rtc_read(&t);
                if (t.min != now.min || t.hour != now.hour || t.day != now.day) mark(taskbar_rect());
                now = t;
            }
        }
        render();
        hlt();
    }
}

void wm_refresh_clock(void) {
    rtc_read(&now);
    mark(taskbar_rect());
}
