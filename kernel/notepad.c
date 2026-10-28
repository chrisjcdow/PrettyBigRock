/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - Notepad and the Files browser */
#include "kernel.h"

/* ================================================================ Notepad */
#define NP_MAX FS_MAX_SIZE
#define NP_TOOL 32

static char text[NP_MAX + 1];
static int len, cur, top, rows = 20, modified, pressed;
static char fname[FS_NAME_LEN] = "untitled.txt";
static char status[48];
static u32 status_until;
static window_t *nwin;

static void np_title(void) {
    char t[64];
    ksnprintf(t, sizeof t, "%s%s - Notepad", modified ? "*" : "", fname);
    if (nwin) wm_set_title(nwin, t);
}

static void np_pos(int idx, int *line, int *col) {
    int l = 0, c = 0;
    for (int i = 0; i < idx && i < len; i++) {
        if (text[i] == '\n') { l++; c = 0; } else c++;
    }
    *line = l;
    *col = c;
}

static int np_index_at(int line, int col) {
    int i = 0, l = 0;
    while (i < len && l < line) { if (text[i] == '\n') l++; i++; }
    if (l < line) return len;
    int c = 0;
    while (i < len && text[i] != '\n' && c < col) { i++; c++; }
    return i;
}

static void np_scroll_to_cursor(void) {
    int l, c;
    np_pos(cur, &l, &c);
    if (l < top) top = l;
    if (l >= top + rows) top = l - rows + 1;
}

static void np_set_status(const char *s) {
    strlcpy0(status, s, sizeof status);
    status_until = ticks + 300;
}

static void np_save(void) {
    int i = fs_create(fname);
    if (i < 0) { np_set_status("Couldn't save: disk full"); return; }
    fs_write(i, text, len);
    modified = 0;
    np_set_status("Saved");
    np_title();
}

static void np_new(void) {
    len = cur = top = 0;
    text[0] = 0;
    modified = 0;
    int n = 1;
    do ksnprintf(fname, sizeof fname, "note%d.txt", n++); while (fs_find(fname) >= 0);
    np_title();
}

static void np_draw(window_t *w, int cx, int cy, int cw, int ch) {
    gfx_fill(cx, cy, cw, NP_TOOL, 0xebe7df);
    gfx_fill(cx, cy + NP_TOOL - 1, cw, 1, 0xc9c4ba);
    ui_button(cx + 6, cy + 5, 54, 22, "New", pressed == 1);
    ui_button(cx + 64, cy + 5, 54, 22, "Save", pressed == 2);
    ui_button(cx + 122, cy + 5, 54, 22, "Files", pressed == 3);
    int l, c;
    np_pos(cur, &l, &c);
    char pos[32];
    ksnprintf(pos, sizeof pos, "Ln %d, Col %d", l + 1, c + 1);
    gfx_text(cx + cw - gfx_text_w(pos) - 10, cy + 12, pos, 0x5a5f6a);
    if (status[0] && ticks < status_until) gfx_text_bold(cx + 190, cy + 12, status, 0x2f6b55);

    int ty = cy + NP_TOOL;
    gfx_fill(cx, ty, cw, ch - NP_TOOL, 0xffffff);
    gfx_fill(cx, ty, 36, ch - NP_TOOL, 0xf3f1ec);
    gfx_fill(cx + 36, ty, 1, ch - NP_TOOL, 0xe0ddd5);
    rows = (ch - NP_TOOL - 12) / LINE_H;
    int x0 = cx + 44, y0 = ty + 7;
    int cols = (cw - 52) / CHAR_W;
    int line = 0, col = 0, cl = -1, cc = 0;
    char num[8];
    for (int i = 0; i <= len; i++) {
        if (i == cur) { cl = line; cc = col; }
        if (col == 0 && line >= top && line < top + rows) {
            ksnprintf(num, sizeof num, "%3d", line + 1);
            gfx_text(cx + 8, y0 + (line - top) * LINE_H, num, 0xa3a7b0);
        }
        if (i == len) break;
        if (text[i] == '\n') { line++; col = 0; continue; }
        if (line >= top && line < top + rows && col < cols)
            gfx_char(x0 + col * CHAR_W, y0 + (line - top) * LINE_H, text[i], 0x1f2229, 1);
        col++;
    }
    if (cl >= top && cl < top + rows && wm_is_focused(w) && (ticks / 50) % 2 == 0)
        gfx_fill(x0 + cc * CHAR_W - 1, y0 + (cl - top) * LINE_H - 2, 2, 12, 0xd2577a);
}

static void np_insert(char ch) {
    if (len >= NP_MAX) return;
    memmove(text + cur + 1, text + cur, len - cur);
    text[cur++] = ch;
    text[++len] = 0;
    if (!modified) { modified = 1; np_title(); }
}

static void np_key(window_t *w, int k) {
    int l, c;
    if (k == (KEY_CTRL | 's')) np_save();
    else if (k == (KEY_CTRL | 'n')) np_new();
    else if (k == '\b') {
        if (cur > 0) {
            memmove(text + cur - 1, text + cur, len - cur);
            cur--;
            text[--len] = 0;
            if (!modified) { modified = 1; np_title(); }
        }
    } else if (k == KEY_DEL) {
        if (cur < len) {
            memmove(text + cur, text + cur + 1, len - cur - 1);
            text[--len] = 0;
            if (!modified) { modified = 1; np_title(); }
        }
    } else if (k == KEY_LEFT) { if (cur > 0) cur--; }
    else if (k == KEY_RIGHT) { if (cur < len) cur++; }
    else if (k == KEY_UP) { np_pos(cur, &l, &c); if (l > 0) cur = np_index_at(l - 1, c); }
    else if (k == KEY_DOWN) { np_pos(cur, &l, &c); cur = np_index_at(l + 1, c); }
    else if (k == KEY_PGUP) { np_pos(cur, &l, &c); cur = np_index_at(MAX(l - rows, 0), c); }
    else if (k == KEY_PGDN) { np_pos(cur, &l, &c); cur = np_index_at(l + rows, c); }
    else if (k == (KEY_CTRL | KEY_HOME)) cur = 0;
    else if (k == (KEY_CTRL | KEY_END)) cur = len;
    else if (k == KEY_HOME) { np_pos(cur, &l, &c); cur = np_index_at(l, 0); }
    else if (k == KEY_END) { np_pos(cur, &l, &c); cur = np_index_at(l, 100000); }
    else if (k == '\t') { for (int i = 0; i < 4; i++) np_insert(' '); }
    else if (k == '\n' || (k >= 32 && k < 127)) np_insert((char)k);
    np_scroll_to_cursor();
    win_invalidate(w);
}

static void np_mouse(window_t *w, int ev, int x, int y, int b) {
    (void)b;
    if (ev == ME_DOWN && y < NP_TOOL) {
        if (y >= 5 && y < 27) {
            if (x >= 6 && x < 60) pressed = 1;
            else if (x >= 64 && x < 118) pressed = 2;
            else if (x >= 122 && x < 176) pressed = 3;
        }
    } else if (ev == ME_UP && pressed) {
        int p = pressed;
        pressed = 0;
        if (y >= 5 && y < 27) {
            if (p == 1 && x >= 6 && x < 60) np_new();
            if (p == 2 && x >= 64 && x < 118) np_save();
            if (p == 3 && x >= 122 && x < 176) app_open(APP_FILES);
        }
    } else if ((ev == ME_DOWN || ev == ME_MOVE) && y >= NP_TOOL && !pressed) {
        int line = top + (y - NP_TOOL - 5) / LINE_H;
        int col = (x - 44 + CHAR_W / 2) / CHAR_W;
        if (line < 0) line = 0;
        if (col < 0) col = 0;
        cur = np_index_at(line, col);
    }
    win_invalidate(w);
}

static u32 np_blink;
static void np_tick(window_t *w) {
    u32 b = ticks / 50;
    if (b != np_blink) { np_blink = b; win_invalidate(w); }
}

static void np_close(window_t *w) { (void)w; nwin = 0; }

static void np_open_window(void) {
    if (nwin) { wm_focus(nwin); return; }
    nwin = wm_create("Notepad", 560, 400, APP_NOTEPAD);
    if (!nwin) return;
    nwin->draw = np_draw;
    nwin->key = np_key;
    nwin->mouse = np_mouse;
    nwin->tick = np_tick;
    nwin->close = np_close;
    np_title();
}

void notepad_open_file(const char *name) {
    strlcpy0(fname, name, sizeof fname);
    int i = fs_find(name);
    if (i >= 0) {
        len = fs_size(i);
        memcpy(text, fs_data(i), len);
    } else len = 0;
    text[len] = 0;
    cur = top = 0;
    modified = 0;
    np_open_window();
    np_title();
    win_invalidate(nwin);
}

void notepad_open(void) {
    if (!nwin && len == 0 && fs_find(fname) >= 0) { notepad_open_file(fname); return; }
    np_open_window();
}

/* ================================================================ Files */
#define FL_TOOL 34
#define FL_ROW 19
static window_t *fwin;
static int fsel = -1, fpress, fgen, ftop, fvis = 10;
static int frow[FS_MAX_FILES], fnrows;

static void fl_refresh(void) {
    fnrows = 0;
    for (int i = 0; i < FS_MAX_FILES; i++) if (fs_valid(i)) frow[fnrows++] = i;
    if (fsel >= fnrows) fsel = fnrows - 1;
    fgen = fs_generation;
}

static void fl_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w;
    if (fgen != fs_generation) fl_refresh();
    gfx_fill(cx, cy, cw, FL_TOOL, 0xebe7df);
    gfx_fill(cx, cy + FL_TOOL - 1, cw, 1, 0xc9c4ba);
    ui_button(cx + 6, cy + 6, 60, 22, "Open", fpress == 1);
    ui_button(cx + 70, cy + 6, 60, 22, "New", fpress == 2);
    ui_button(cx + 134, cy + 6, 60, 22, "Delete", fpress == 3);

    int y = cy + FL_TOOL;
    gfx_fill(cx, y, cw, ch - FL_TOOL, 0xffffff);
    gfx_fill(cx, y, cw, 20, 0xf3f1ec);
    gfx_text_bold(cx + 34, y + 6, "Name", 0x4a4f5a);
    gfx_text_bold(cx + cw - 80, y + 6, "Size", 0x4a4f5a);
    gfx_fill(cx, y + 20, cw, 1, 0xe0ddd5);
    y += 22;
    fvis = (ch - FL_TOOL - 22 - 22) / FL_ROW;
    if (fsel >= 0 && fsel < ftop) ftop = fsel;
    if (fsel >= ftop + fvis) ftop = fsel - fvis + 1;
    if (ftop > MAX(fnrows - fvis, 0)) ftop = MAX(fnrows - fvis, 0);
    for (int r = ftop; r < fnrows && r < ftop + fvis; r++, y += FL_ROW) {
        int i = frow[r];
        if (r == fsel) gfx_fill(cx + 2, y, cw - 4, FL_ROW, 0xf6dde6);
        gfx_icon(ICON_NOTEPAD, cx + 10, y + 1, 1);
        gfx_text(cx + 34, y + 5, fs_name(i), 0x1f2229);
        char sz[16];
        ksnprintf(sz, sizeof sz, "%d B", fs_size(i));
        gfx_text(cx + cw - 80, y + 5, sz, 0x5a5f6a);
    }
    char foot[48];
    ksnprintf(foot, sizeof foot, "%d of %d files%s", fnrows, FS_MAX_FILES, fnrows > fvis ? " - arrow keys scroll" : " - double-click to open");
    gfx_fill(cx, cy + ch - 20, cw, 20, 0xf3f1ec);
    gfx_text(cx + 8, cy + ch - 14, foot, 0x6a6f7a);
}

static void fl_open_sel(void) {
    if (fsel >= 0 && fsel < fnrows) notepad_open_file(fs_name(frow[fsel]));
}

static void fl_mouse(window_t *w, int ev, int x, int y, int b) {
    if (ev == ME_DOWN) {
        if (y < FL_TOOL) {
            if (y >= 6 && y < 28) {
                if (x >= 6 && x < 66) fpress = 1;
                else if (x >= 70 && x < 130) fpress = 2;
                else if (x >= 134 && x < 194) fpress = 3;
            }
        } else if (y >= FL_TOOL + 22) {
            int r = (y - FL_TOOL - 22) / FL_ROW;
            if (r >= fvis) return;
            r += ftop;
            fsel = r < fnrows ? r : -1;
            if (fsel >= 0 && (b & MB_DOUBLE)) fl_open_sel();
        }
    } else if (ev == ME_UP && fpress) {
        int p = fpress;
        fpress = 0;
        if (y >= 6 && y < 28) {
            if (p == 1 && x >= 6 && x < 66) fl_open_sel();
            if (p == 2 && x >= 70 && x < 130) {
                char n[FS_NAME_LEN];
                int k = 1;
                do ksnprintf(n, sizeof n, "new%d.txt", k++); while (fs_find(n) >= 0);
                if (fs_create(n) >= 0) { fl_refresh(); fsel = fnrows - 1; }
            }
            if (p == 3 && x >= 134 && x < 194 && fsel >= 0 && fsel < fnrows) { fs_delete(frow[fsel]); fl_refresh(); }
        }
    }
    win_invalidate(w);
}

static void fl_key(window_t *w, int k) {
    if (k == KEY_UP && fsel > 0) fsel--;
    else if (k == KEY_DOWN && fsel < fnrows - 1) fsel++;
    else if (k == KEY_PGUP) fsel = MAX(fsel - fvis, 0);
    else if (k == KEY_PGDN) fsel = MIN(fsel + fvis, fnrows - 1);
    else if (k == '\n') fl_open_sel();
    else if (k == KEY_DEL && fsel >= 0) { fs_delete(frow[fsel]); fl_refresh(); }
    win_invalidate(w);
}

static void fl_tick(window_t *w) { if (fgen != fs_generation) win_invalidate(w); }
static void fl_close(window_t *w) { (void)w; fwin = 0; }

void files_open(void) {
    fwin = wm_create("Files", 380, 380, APP_FILES);
    if (!fwin) return;
    fwin->draw = fl_draw;
    fwin->mouse = fl_mouse;
    fwin->key = fl_key;
    fwin->tick = fl_tick;
    fwin->close = fl_close;
    fl_refresh();
}
