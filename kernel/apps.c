/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - app launcher plus Calculator, Paint, Snake and About */
#include "kernel.h"
#include "rockfs.h"

const char *app_names[APP_COUNT] = {"Terminal", "Notepad", "Files", "Calculator", "Paint", "Snake", "About"};
const int app_icons[APP_COUNT] = {ICON_TERMINAL, ICON_NOTEPAD, ICON_FILES, ICON_CALC, ICON_PAINT, ICON_SNAKE, ICON_ABOUT};

void term_open(void);
void notepad_open(void);
void files_open(void);
static void calc_open(void);
static void paint_open(void);
static void snake_open(void);
static void about_open(void);

void app_open(int app) {
    window_t *w = wm_find_app(app);
    if (w) { wm_focus(w); return; }
    switch (app) {
    case APP_TERMINAL: term_open(); break;
    case APP_NOTEPAD: notepad_open(); break;
    case APP_FILES: files_open(); break;
    case APP_CALC: calc_open(); break;
    case APP_PAINT: paint_open(); break;
    case APP_SNAKE: snake_open(); break;
    case APP_ABOUT: about_open(); break;
    }
}

/* ================================================================ Calculator */
static const char *cbtn[5][4] = {
    {"C", "+/-", "%", "/"},
    {"7", "8", "9", "*"},
    {"4", "5", "6", "-"},
    {"1", "2", "3", "+"},
    {"0", ".", "<", "="},
};
static char cdisp[24] = "0";
static double cacc;
static char cop;
static int cfresh = 1, cpr = -1, cprc = -1;

static double capply(double a, char op, double b) {
    switch (op) {
    case '+': return a + b;
    case '-': return a - b;
    case '*': return a * b;
    case '/': return b == 0 ? 0.0 / 0.0 : a / b;
    }
    return b;
}

static void cshow(double v) { fmt_double(cdisp, sizeof cdisp, v); }

static void cpress(const char *b) {
    char c = b[0];
    if (!strcmp(cdisp, "Error") && c != 'C') return;
    if ((c >= '0' && c <= '9') || c == '.') {
        if (cfresh) { strlcpy0(cdisp, c == '.' ? "0." : b, sizeof cdisp); cfresh = 0; return; }
        if (c == '.' && 0) return;
        for (const char *p = cdisp; *p && c == '.'; p++) if (*p == '.') return;
        int l = (int)strlen(cdisp);
        if (l >= 15) return;
        if (!strcmp(cdisp, "0") && c != '.') l = 0;
        cdisp[l] = c;
        cdisp[l + 1] = 0;
    } else if (c == 'C') {
        strlcpy0(cdisp, "0", sizeof cdisp);
        cacc = 0; cop = 0; cfresh = 1;
    } else if (!strcmp(b, "+/-")) {
        cshow(-parse_double(cdisp));
    } else if (c == '%') {
        cshow(parse_double(cdisp) / 100);
        cfresh = 1;
    } else if (c == '<') {
        if (cfresh) return;
        int l = (int)strlen(cdisp);
        if (l <= 1 || (l == 2 && cdisp[0] == '-')) strlcpy0(cdisp, "0", sizeof cdisp);
        else cdisp[l - 1] = 0;
    } else if (c == '=') {
        if (cop) { cshow(capply(cacc, cop, parse_double(cdisp))); cop = 0; }
        cfresh = 1;
    } else { /* operator */
        double v = parse_double(cdisp);
        if (cop && !cfresh) { cacc = capply(cacc, cop, v); cshow(cacc); }
        else if (!cop) cacc = v;
        cop = c;
        cfresh = 1;
    }
}

#define CB_W 50
#define CB_H 42
#define CB_G 8
#define CB_Y 86

static void calc_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w; (void)ch;
    gfx_fill(cx, cy, cw, ch, 0xe9e6e0);
    gfx_round(cx + 10, cy + 10, cw - 20, 64, 6, 0x1f2430);
    if (cop) {
        char e[40], a[30];
        fmt_double(a, sizeof a, cacc);
        ksnprintf(e, sizeof e, "%s %c", a, cop == '*' ? 'x' : cop);
        gfx_text(cx + cw - 20 - gfx_text_w(e), cy + 18, e, 0x9aa3b8);
    }
    int sc = gfx_text_w(cdisp) * 3 <= cw - 40 ? 3 : (gfx_text_w(cdisp) * 2 <= cw - 40 ? 2 : 1);
    gfx_text_scaled(cx + cw - 20 - gfx_text_w(cdisp) * sc, cy + 66 - 9 * sc, cdisp, 0xffffff, sc);
    for (int r = 0; r < 5; r++)
        for (int c = 0; c < 4; c++) {
            int x = cx + 10 + c * (CB_W + CB_G), y = cy + CB_Y + r * (CB_H + CB_G);
            const char *l = cbtn[r][c];
            int pr = cpr == r && cprc == c;
            if (c == 3 || r == 0) {
                u32 col = c == 3 ? (r == 4 ? 0xd2577a : 0xe0913a) : 0x9aa0ac;
                if (pr) col = color_lerp(col, 0x000000, 1, 4);
                gfx_round(x, y, CB_W, CB_H, 6, col);
                gfx_text_bold(x + (CB_W - (int)strlen(l) * 7) / 2, y + 17, l, 0xffffff);
            } else {
                ui_button(x, y, CB_W, CB_H, l, pr);
            }
        }
}

static void calc_mouse(window_t *w, int ev, int x, int y, int b) {
    (void)b;
    int c = (x - 10) / (CB_W + CB_G), r = (y - CB_Y) / (CB_H + CB_G);
    int inside = x >= 10 && y >= CB_Y && c < 4 && r < 5 && (x - 10) % (CB_W + CB_G) < CB_W && (y - CB_Y) % (CB_H + CB_G) < CB_H;
    if (ev == ME_DOWN && inside) { cpr = r; cprc = c; }
    else if (ev == ME_UP) {
        if (inside && cpr == r && cprc == c) cpress(cbtn[r][c]);
        cpr = cprc = -1;
    }
    win_invalidate(w);
}

static void calc_key(window_t *w, int k) {
    char s[2] = {(char)k, 0};
    if ((k >= '0' && k <= '9') || k == '.' || k == '+' || k == '-' || k == '*' || k == '/' || k == '%') cpress(s);
    else if (k == '=' || k == '\n') cpress("=");
    else if (k == '\b') cpress("<");
    else if (k == 27 || k == 'c' || k == 'C') cpress("C");
    else if (k == 'x') cpress("*");
    win_invalidate(w);
}

static void calc_open(void) {
    window_t *w = wm_create("Calculator", 4 * CB_W + 3 * CB_G + 20, CB_Y + 5 * CB_H + 4 * CB_G + 10, APP_CALC);
    if (!w) return;
    w->draw = calc_draw;
    w->mouse = calc_mouse;
    w->key = calc_key;
}

/* ================================================================ Paint */
#define PW 600
#define PH 400
#define PTOOL 38
static u32 *canvas;
static const u32 pcolors[] = {0x1b1b1f, 0xffffff, 0x8a8f98, 0xe05252, 0xf29a3a, 0xf5d04a,
                              0x5cc46a, 0x2f7d43, 0x3aa6a0, 0x4a8fe0, 0x8e5cc9, 0xe8678f, 0x8a5a35};
#define NPCOL (int)(sizeof pcolors / sizeof *pcolors)
static const int psizes[] = {2, 5, 10, 18};
static int pcol = 0, psize = 1, plx, ply, pdown;

static void p_stamp(int x, int y) {
    int r = psizes[psize];
    for (int dy = -r; dy <= r; dy++) {
        int yy = y + dy;
        if (yy < 0 || yy >= PH - PTOOL) continue;
        int dx = isqrt(r * r - dy * dy);
        for (int xx = x - dx; xx <= x + dx; xx++)
            if (xx >= 0 && xx < PW) canvas[yy * PW + xx] = pcolors[pcol];
    }
}

static void p_line(int x0, int y0, int x1, int y1) {
    int dx = ABS(x1 - x0), sx = x0 < x1 ? 1 : -1;
    int dy = -ABS(y1 - y0), sy = y0 < y1 ? 1 : -1;
    int err = dx + dy;
    for (;;) {
        p_stamp(x0, y0);
        if (x0 == x1 && y0 == y1) break;
        int e2 = 2 * err;
        if (e2 >= dy) { err += dy; x0 += sx; }
        if (e2 <= dx) { err += dx; y0 += sy; }
    }
}

#define PSWATCH 23
#define CW PW
#define CH (PH - PTOOL)
static int ppress;
static char ppath[256]; /* where the picture was last saved or opened from */
static window_t *pwin;
static const char *pbtn_label[3] = {"Open", "Save", "Clear"};
static const int pbtn_x[3] = {436, 486, 536};
static const int pbtn_w[3] = {46, 46, 56};

static const char *p_base(const char *p) {
    const char *b = p;
    for (; *p; p++) if (*p == '/') b = p + 1;
    return b;
}

static void p_title(void) {
    char t[80];
    ksnprintf(t, sizeof t, "%s - Paint", ppath[0] ? p_base(ppath) : "Untitled");
    if (pwin) wm_set_title(pwin, t);
}

static void paint_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w; (void)ch; (void)cw;
    gfx_fill(cx, cy, cw, PTOOL, 0xebe7df);
    gfx_fill(cx, cy + PTOOL - 1, cw, 1, 0xc9c4ba);
    for (int i = 0; i < NPCOL; i++) {
        int x = cx + 8 + i * PSWATCH;
        if (i == pcol) gfx_round(x - 3, cy + 6, 26, 26, 5, 0x2f6b55);
        gfx_round(x, cy + 9, 20, 20, 4, 0x6b717c);
        gfx_round(x + 1, cy + 10, 18, 18, 3, pcolors[i]);
    }
    int sx = cx + 8 + NPCOL * PSWATCH + 8;
    for (int i = 0; i < 4; i++) {
        int x = sx + i * 27;
        if (i == psize) gfx_round(x, cy + 6, 26, 26, 5, 0xd5d0c6);
        gfx_circle(x + 13, cy + 19, 1 + i * 2, 0x2a2e38);
    }
    for (int i = 0; i < 3; i++) ui_button(cx + pbtn_x[i], cy + 8, pbtn_w[i], 22, pbtn_label[i], ppress == i + 1);
    gfx_blit(cx, cy + PTOOL, PW, CH, canvas, PW);
}

/* ---- saving and opening Windows bitmaps (24-bit, uncompressed) ---- */
static void put16(u8 *p, u32 v) { p[0] = v; p[1] = v >> 8; }
static void put32(u8 *p, u32 v) { p[0] = v; p[1] = v >> 8; p[2] = v >> 16; p[3] = v >> 24; }
static u32 get16(const u8 *p) { return p[0] | (p[1] << 8); }
static u32 get32(const u8 *p) { return p[0] | (p[1] << 8) | (p[2] << 16) | ((u32)p[3] << 24); }

static void save_answer(int ok, const char *answer, void *ctx) {
    (void)ctx;
    if (!ok || !answer[0]) return;
    char full[256];
    if (!rfs_path_join("/Pictures", answer, full, sizeof full)) { wm_message("Paint", "That name is too long."); return; }
    u32 row = (CW * 3 + 3) & ~3u, size = 54 + row * CH;
    u8 *f = kmalloc(size);
    if (!f) { wm_message("Paint", "There isn't enough memory to save the picture."); return; }
    f[0] = 'B'; f[1] = 'M';
    put32(f + 2, size);
    put32(f + 10, 54);
    put32(f + 14, 40);
    put32(f + 18, CW);
    put32(f + 22, CH);
    put16(f + 26, 1);
    put16(f + 28, 24);
    put32(f + 34, row * CH);
    put32(f + 38, 2835); /* 72 dots per inch */
    put32(f + 42, 2835);
    for (int y = 0; y < CH; y++) { /* bitmaps are stored bottom row first */
        u8 *d = f + 54 + (u32)(CH - 1 - y) * row;
        for (int x = 0; x < CW; x++) {
            u32 c = canvas[y * PW + x];
            d[x * 3] = c; d[x * 3 + 1] = c >> 8; d[x * 3 + 2] = c >> 16;
        }
    }
    rfs_stat_t st;
    if (rfs_stat("/Pictures", &st)) rfs_mkdir("/Pictures");
    int e = rfs_write(full, f, size);
    kfree(f);
    if (e) {
        char msg[200];
        ksnprintf(msg, sizeof msg, "Couldn't save the picture. %s.", rfs_strerror(e));
        wm_message("Paint", msg);
        return;
    }
    strlcpy0(ppath, full, sizeof ppath);
    p_title();
}

static void p_save(void) {
    char suggestion[64];
    if (ppath[0]) strlcpy0(suggestion, p_base(ppath), sizeof suggestion);
    else {
        rfs_stat_t st;
        int n = 1;
        do { char t[96]; ksnprintf(suggestion, sizeof suggestion, "Drawing %d.bmp", n++); ksnprintf(t, sizeof t, "/Pictures/%s", suggestion); if (rfs_stat(t, &st)) break; } while (n < 1000);
    }
    wm_dialog("Save picture", "Save the picture in Pictures as:", "Save", "Cancel", suggestion, save_answer, 0);
}

void paint_open_file(const char *path) {
    rfs_stat_t st;
    int e = rfs_stat(path, &st);
    char msg[220];
    if (!e && st.size > 8 * 1024 * 1024) e = RFS_ENOMEM;
    u8 *f = e ? 0 : kmalloc((u32)st.size + 1);
    u32 got = 0;
    if (!e && !f) e = RFS_ENOMEM;
    if (!e) e = rfs_read(path, f, (u32)st.size, &got);
    if (e) {
        ksnprintf(msg, sizeof msg, "Couldn't open %s. %s.", p_base(path), rfs_strerror(e));
        wm_message("Paint", msg);
        kfree(f);
        return;
    }
    u32 off = got >= 54 ? get32(f + 10) : 0, bpp = got >= 54 ? get16(f + 28) : 0, comp = got >= 54 ? get32(f + 30) : 1;
    i32 w = got >= 54 ? (i32)get32(f + 18) : 0, h = got >= 54 ? (i32)get32(f + 22) : 0;
    int top_down = h < 0;
    if (h < 0) h = -h;
    u32 row = (((u32)w * (bpp / 8)) + 3) & ~3u;
    if (got < 54 || f[0] != 'B' || f[1] != 'M' || (bpp != 24 && bpp != 32) || comp != 0 || w <= 0 || h <= 0 ||
        off + row * (u32)h > got) {
        ksnprintf(msg, sizeof msg, "%s isn't a picture Paint can open. It opens uncompressed 24- or 32-bit .bmp files.", p_base(path));
        wm_message("Paint", msg);
        kfree(f);
        return;
    }
    if (!canvas) canvas = kmalloc(PW * CH * 4);
    memset32(canvas, 0xffffff, PW * CH);
    for (int y = 0; y < h && y < CH; y++) {
        const u8 *src = f + off + (u32)(top_down ? y : h - 1 - y) * row;
        for (int x = 0; x < w && x < CW; x++) {
            const u8 *px = src + x * (bpp / 8);
            canvas[y * PW + x] = px[0] | (px[1] << 8) | (px[2] << 16);
        }
    }
    kfree(f);
    strlcpy0(ppath, path, sizeof ppath);
    app_open(APP_PAINT);
    p_title();
    if (pwin) win_invalidate(pwin);
}

static void paint_mouse(window_t *w, int ev, int x, int y, int b) {
    (void)b;
    int hit = 0;
    if (y >= 8 && y < 30)
        for (int i = 0; i < 3; i++) if (x >= pbtn_x[i] && x < pbtn_x[i] + pbtn_w[i]) hit = i + 1;
    if (ev == ME_DOWN && y < PTOOL) {
        for (int i = 0; i < NPCOL; i++) if (x >= 8 + i * PSWATCH && x < 28 + i * PSWATCH) pcol = i;
        int sx = 8 + NPCOL * PSWATCH + 8;
        for (int i = 0; i < 4; i++) if (x >= sx + i * 27 && x < sx + i * 27 + 26) psize = i;
        ppress = hit;
    } else if (ev == ME_UP && ppress) {
        int p = ppress;
        ppress = 0;
        if (p == hit) {
            if (p == 1) files_open_at("/Pictures");
            if (p == 2) p_save();
            if (p == 3) { memset32(canvas, 0xffffff, PW * CH); ppath[0] = 0; p_title(); }
        }
    } else if (ev == ME_DOWN) {
        pdown = 1;
        plx = x; ply = y - PTOOL;
        p_stamp(plx, ply);
    } else if (ev == ME_MOVE && pdown) {
        p_line(plx, ply, x, y - PTOOL);
        plx = x; ply = y - PTOOL;
    } else if (ev == ME_UP) pdown = 0;
    win_invalidate(w);
}

static void paint_key(window_t *w, int k) {
    (void)w;
    if (k == (KEY_CTRL | 's')) p_save();
    if (k == (KEY_CTRL | 'o')) files_open_at("/Pictures");
}

static void paint_close(window_t *w) { (void)w; pwin = 0; }

static void paint_open(void) {
    if (!canvas) {
        canvas = kmalloc(PW * CH * 4);
        memset32(canvas, 0xffffff, PW * CH);
    }
    window_t *w = wm_create("Paint", PW, PH, APP_PAINT);
    if (!w) return;
    pwin = w;
    w->draw = paint_draw;
    w->mouse = paint_mouse;
    w->key = paint_key;
    w->close = paint_close;
    p_title();
}

/* ================================================================ Snake */
#define SG_W 28
#define SG_H 20
#define SG_C 15
#define SG_HEAD 30
typedef struct { i8 x, y; } pt_t;
static pt_t body[SG_W * SG_H];
static int slen, sdir, snext, sscore, sbest, sstate, spaused; /* state: 0 ready, 1 playing, 2 over */
static pt_t food;
static u32 slast;
static const int DX[4] = {1, 0, -1, 0}, DY[4] = {0, 1, 0, -1};

static void s_food(void) {
    for (;;) {
        food.x = krand() % SG_W;
        food.y = krand() % SG_H;
        int hit = 0;
        for (int i = 0; i < slen; i++) if (body[i].x == food.x && body[i].y == food.y) hit = 1;
        if (!hit) return;
    }
}

static void s_reset(void) {
    slen = 4;
    for (int i = 0; i < slen; i++) { body[i].x = 8 - i; body[i].y = SG_H / 2; }
    sdir = snext = 0;
    sscore = 0;
    spaused = 0;
    ksrand(ticks * 2654435761u + 7);
    s_food();
}

static void s_step(void) {
    if ((snext + 2) % 4 != sdir) sdir = snext;
    pt_t h = {(i8)(body[0].x + DX[sdir]), (i8)(body[0].y + DY[sdir])};
    if (h.x < 0 || h.y < 0 || h.x >= SG_W || h.y >= SG_H) { sstate = 2; return; }
    int grow = h.x == food.x && h.y == food.y;
    for (int i = 0; i < slen - (grow ? 0 : 1); i++)
        if (body[i].x == h.x && body[i].y == h.y) { sstate = 2; return; }
    if (grow && slen < SG_W * SG_H) slen++;
    memmove(body + 1, body, (slen - 1) * sizeof(pt_t));
    body[0] = h;
    if (grow) {
        sscore += 10;
        if (sscore > sbest) sbest = sscore;
        s_food();
    }
}

static void snake_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)ch;
    gfx_fill(cx, cy, cw, SG_HEAD, 0x1d2a22);
    char s[48];
    ksnprintf(s, sizeof s, "Score %d", sscore);
    gfx_text_bold(cx + 10, cy + 11, s, 0xffffff);
    ksnprintf(s, sizeof s, "Best %d", sbest);
    gfx_text_bold(cx + cw - 10 - (int)strlen(s) * 7, cy + 11, s, 0xf5d04a);
    int gy = cy + SG_HEAD;
    for (int y = 0; y < SG_H; y++)
        for (int x = 0; x < SG_W; x++)
            gfx_fill(cx + x * SG_C, gy + y * SG_C, SG_C, SG_C, ((x + y) & 1) ? 0x2b3d2f : 0x304434);
    gfx_circle(cx + food.x * SG_C + SG_C / 2, gy + food.y * SG_C + SG_C / 2, SG_C / 2 - 1, 0xe05252);
    gfx_fill(cx + food.x * SG_C + SG_C / 2, gy + food.y * SG_C, 2, 3, 0x5cc46a);
    for (int i = slen - 1; i >= 0; i--) {
        u32 col = i == 0 ? 0x9be89f : color_lerp(0x5cc46a, 0x2f7d43, i, slen);
        gfx_round(cx + body[i].x * SG_C + 1, gy + body[i].y * SG_C + 1, SG_C - 2, SG_C - 2, 4, col);
    }
    int hx = cx + body[0].x * SG_C, hy = gy + body[0].y * SG_C;
    int ex = DX[sdir] != 0 ? 0 : 3, ey = DY[sdir] != 0 ? 0 : 3;
    gfx_fill(hx + SG_C / 2 - 1 - ex + DX[sdir] * 3, hy + SG_C / 2 - 1 - ey + DY[sdir] * 3, 2, 2, 0x10140f);
    gfx_fill(hx + SG_C / 2 - 1 + ex + DX[sdir] * 3, hy + SG_C / 2 - 1 + ey + DY[sdir] * 3, 2, 2, 0x10140f);

    const char *msg = 0, *sub = 0;
    if (sstate == 0) { msg = "SNAKE"; sub = "Press Space to start - arrows or WASD to steer"; }
    else if (sstate == 2) { msg = "GAME OVER"; sub = "Press Space to play again"; }
    else if (spaused || !wm_is_focused(w)) { msg = "PAUSED"; sub = "Press P to carry on"; }
    if (msg) {
        gfx_fill_alpha(cx, gy, cw, SG_H * SG_C, 0x000000, 140);
        gfx_text_scaled(cx + (cw - gfx_text_w(msg) * 3) / 2, gy + 110, msg, 0xffffff, 3);
        gfx_text(cx + (cw - gfx_text_w(sub)) / 2, gy + 152, sub, 0xd8dce6);
    }
}

static void snake_key(window_t *w, int k) {
    if (k == KEY_RIGHT || k == 'd') snext = 0;
    else if (k == KEY_DOWN || k == 's') snext = 1;
    else if (k == KEY_LEFT || k == 'a') snext = 2;
    else if (k == KEY_UP || k == 'w') snext = 3;
    else if (k == ' ' || k == '\n') {
        if (sstate != 1) { s_reset(); sstate = 1; }
        else spaused = !spaused;
    } else if (k == 'p') spaused = !spaused;
    win_invalidate(w);
}

static void snake_tick(window_t *w) {
    if (sstate != 1 || spaused || !wm_is_focused(w)) return;
    u32 delay = sscore >= 250 ? 5 : 11 - sscore / 40;
    if (ticks - slast < delay) return;
    slast = ticks;
    s_step();
    win_invalidate(w);
}

static void snake_open(void) {
    if (sstate == 0) s_reset();
    window_t *w = wm_create("Snake", SG_W * SG_C, SG_H * SG_C + SG_HEAD, APP_SNAKE);
    if (!w) return;
    w->draw = snake_draw;
    w->key = snake_key;
    w->tick = snake_tick;
}

/* ================================================================ About */
static u32 about_last;

static void about_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w;
    gfx_vgrad(cx, cy, cw, 120, 0x2f6b55, 0x1d4234);
    gfx_icon(ICON_LOGO, cx + 22, cy + 12, 6);
    gfx_text_scaled(cx + 130, cy + 30, OS_NAME, 0xffffff, 3);
    gfx_text_bold(cx + 132, cy + 66, "Version " OS_VERSION, 0xcfe8dc);
    gfx_text(cx + 132, cy + 84, "Named for Table Mountain, a pretty big rock.", 0xa9cdbd);
    gfx_fill(cx, cy + 120, cw, ch - 120, 0xf5f2ec);
    char buf[64];
    u32 s = ticks / 100;
    int y = cy + 136;
    const char *labels[] = {"Display", "Memory", "Uptime", "Disk"};
    char vals[4][48];
    ksnprintf(vals[0], 48, "%d x %d, %d-bit colour", SW, SH, SBPP);
    ksnprintf(vals[1], 48, "%u MB", mem_total_mb());
    ksnprintf(vals[2], 48, "%u h %u min %u s", s / 3600, (s / 60) % 60, s % 60);
    storage_describe(vals[3], 48, 1);
    for (int i = 0; i < 4; i++, y += 18) {
        gfx_text_bold(cx + 24, y, labels[i], 0x2f6b55);
        gfx_text(cx + 110, y, vals[i], 0x2a2e38);
    }
    y += 10;
    ksnprintf(buf, sizeof buf, "A small hobby operating system, written");
    gfx_text(cx + 24, y, buf, 0x5a5f6a);
    gfx_text(cx + 24, y + 13, "from scratch in C and x86 assembly.", 0x5a5f6a);
}

static void about_tick(window_t *w) {
    if (ticks / 100 != about_last) { about_last = ticks / 100; win_invalidate(w); }
}

static void about_open(void) {
    window_t *w = wm_create("About PrettyBigRock", 400, 260, APP_ABOUT);
    if (!w) return;
    w->draw = about_draw;
    w->tick = about_tick;
}
