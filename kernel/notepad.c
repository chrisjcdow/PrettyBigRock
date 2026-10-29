/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - Notepad and the Files browser */
#include "kernel.h"
#include "rockfs.h"

/* ================================================================ Notepad */
#define NP_MAX 32000
#define NP_TOOL 32
#define NP_BTNS 4

static char text[NP_MAX + 1];
static int len, cur, top, rows = 20, modified, pressed;
static char path[256];  /* empty until the note is saved */
static char status[48];
static u32 status_until;
static window_t *nwin;
static const char *np_btn_label[NP_BTNS] = {"New", "Open", "Save", "Save as"};
static const int np_btn_x[NP_BTNS] = {6, 64, 122, 180};
static const int np_btn_w[NP_BTNS] = {54, 54, 54, 64};

static const char *base_name(const char *p) {
    const char *b = p;
    for (; *p; p++) if (*p == '/') b = p + 1;
    return b;
}

/* the folder the note lives in, or Documents for a new note */
static const char *np_folder(void) {
    static char dir[256];
    if (!path[0]) {
        rfs_stat_t st;
        strlcpy0(dir, rfs_stat("/Documents", &st) == RFS_OK && st.type == RFS_DIR ? "/Documents" : "/", sizeof dir);
        return dir;
    }
    strlcpy0(dir, path, sizeof dir);
    char *slash = (char *)base_name(dir);
    if (slash > dir + 1) slash[-1] = 0; else { dir[0] = '/'; dir[1] = 0; }
    return dir;
}

static void np_title(void) {
    char t[64];
    ksnprintf(t, sizeof t, "%s%s - Notepad", modified ? "*" : "", path[0] ? base_name(path) : "Untitled");
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
    if (nwin) win_invalidate(nwin);
}

static int np_write(const char *p) {
    int e = rfs_write(p, text, len);
    if (e) {
        char msg[200];
        ksnprintf(msg, sizeof msg, "Couldn't save %s. %s.", base_name(p), rfs_strerror(e));
        wm_message("Notepad", msg);
        return e;
    }
    strlcpy0(path, p, sizeof path);
    modified = 0;
    np_set_status(storage_is_ram() ? "Saved (in memory)" : "Saved");
    np_title();
    return 0;
}

static void save_as_answer(int ok, const char *answer, void *ctx) {
    (void)ctx;
    if (!ok || !answer[0]) return;
    char full[256];
    if (!rfs_path_join(np_folder(), answer, full, sizeof full)) { wm_message("Notepad", "That name is too long."); return; }
    rfs_stat_t st;
    if (rfs_stat(full, &st) == RFS_OK && strcmp(full, path)) {
        if (st.type == RFS_DIR) { wm_message("Notepad", "There's already a folder with that name."); return; }
    }
    np_write(full);
}

static void np_save_as(void) {
    char suggestion[128];
    strlcpy0(suggestion, path[0] ? base_name(path) : "Untitled.txt", sizeof suggestion);
    char msg[300];
    ksnprintf(msg, sizeof msg, "Save the note as (in %s):", np_folder());
    wm_dialog("Save as", msg, "Save", "Cancel", suggestion, save_as_answer, 0);
}

static void np_save(void) {
    if (!path[0]) { np_save_as(); return; }
    np_write(path);
}

static void np_new(void) {
    len = cur = top = 0;
    text[0] = 0;
    modified = 0;
    path[0] = 0;
    np_title();
    if (nwin) win_invalidate(nwin);
}

static void np_draw(window_t *w, int cx, int cy, int cw, int ch) {
    gfx_fill(cx, cy, cw, NP_TOOL, 0xebe7df);
    gfx_fill(cx, cy + NP_TOOL - 1, cw, 1, 0xc9c4ba);
    for (int i = 0; i < NP_BTNS; i++) ui_button(cx + np_btn_x[i], cy + 5, np_btn_w[i], 22, np_btn_label[i], pressed == i + 1);
    int l, c;
    np_pos(cur, &l, &c);
    char pos[32];
    ksnprintf(pos, sizeof pos, "Ln %d, Col %d", l + 1, c + 1);
    gfx_text(cx + cw - gfx_text_w(pos) - 10, cy + 12, pos, 0x5a5f6a);
    if (status[0] && ticks < status_until) gfx_text_bold(cx + 290, cy + 12, status, 0x2f6b55);

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
    else if (k == (KEY_CTRL | 'o')) files_open_at(np_folder());
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
    int hit = 0;
    if (y >= 5 && y < 27)
        for (int i = 0; i < NP_BTNS; i++) if (x >= np_btn_x[i] && x < np_btn_x[i] + np_btn_w[i]) hit = i + 1;
    if (ev == ME_DOWN && y < NP_TOOL) {
        pressed = hit;
    } else if (ev == ME_UP && pressed) {
        int p = pressed;
        pressed = 0;
        if (p == hit) {
            if (p == 1) np_new();
            if (p == 2) files_open_at(np_folder());
            if (p == 3) np_save();
            if (p == 4) np_save_as();
        }
    } else if ((ev == ME_DOWN || ev == ME_MOVE) && y >= NP_TOOL && !pressed) {
        int line = top + (y - NP_TOOL - 5) / LINE_H;
        int col = (x - 44 + CHAR_W / 2) / CHAR_W;
        if (line < 0) line = 0;
        if (col < 0) col = 0;
        cur = np_index_at(line, col);
    }
    if (nwin == w) win_invalidate(w);
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

void notepad_open_file(const char *p) {
    rfs_stat_t st;
    int e = rfs_stat(p, &st);
    if (e == RFS_OK && st.type == RFS_DIR) { files_open_at(p); return; }
    if (e == RFS_OK && st.size > NP_MAX) {
        char msg[200];
        ksnprintf(msg, sizeof msg, "%s is too big for Notepad, which opens files up to 31 KB.", base_name(p));
        wm_message("Notepad", msg);
        return;
    }
    u32 got = 0;
    if (e == RFS_OK) e = rfs_read(p, text, NP_MAX, &got);
    if (e == RFS_ENOENT) got = 0; /* a new file: it's created on the first save */
    else if (e) {
        char msg[200];
        ksnprintf(msg, sizeof msg, "Couldn't open %s. %s.", base_name(p), rfs_strerror(e));
        wm_message("Notepad", msg);
        return;
    }
    len = got;
    text[len] = 0;
    for (int i = 0; i < len; i++) if (text[i] == '\r' || text[i] == 0) text[i] = ' '; /* tolerate Windows line endings */
    strlcpy0(path, p, sizeof path);
    cur = top = 0;
    modified = 0;
    np_open_window();
    np_title();
    win_invalidate(nwin);
}

void notepad_open(void) {
    if (!nwin && !len && !path[0]) { notepad_open_file("/readme.txt"); return; }
    np_open_window();
}

/* ================================================================ Files */
#define FL_TOOL 34
#define FL_ROW 19
#define FL_MAX 512
#define FL_BTNS 6

typedef struct { char name[128]; int type; u64 size; } fentry_t;
static window_t *fwin;
static char fcwd[256] = "/";
static fentry_t *fents;
static int fn, fsel = -1, ftop, fvis = 10, fpress, fgen = -1, ferr;
static const char *fl_btn_label[FL_BTNS] = {"Up", "Open", "New file", "New folder", "Rename", "Delete"};
static const int fl_btn_x[FL_BTNS] = {6, 50, 104, 178, 262, 330};
static const int fl_btn_w[FL_BTNS] = {40, 50, 70, 80, 64, 58};

static int ci_cmp(const char *a, const char *b) {
    for (;; a++, b++) {
        char x = *a, y = *b;
        if (x >= 'A' && x <= 'Z') x += 32;
        if (y >= 'A' && y <= 'Z') y += 32;
        if (x != y || !x) return (u8)x - (u8)y;
    }
}

static int fl_collect(void *ctx, const char *name, u32 ino, int type) {
    (void)ctx; (void)ino;
    if (fn >= FL_MAX) return 1;
    fentry_t *e = &fents[fn++];
    strlcpy0(e->name, name, sizeof e->name);
    e->type = type;
    e->size = 0;
    return 0;
}

static void fl_child(const char *name, char *out, u32 n) { rfs_path_join(fcwd, name, out, n); }

static void fl_refresh(void) {
    if (!fents) fents = kmalloc(FL_MAX * sizeof(fentry_t));
    if (!fents) return;
    char keep[128] = "";
    if (fsel >= 0 && fsel < fn) strlcpy0(keep, fents[fsel].name, sizeof keep);
    fn = 0;
    ferr = rfs_list(fcwd, fl_collect, 0);
    if (ferr == RFS_ENOENT || ferr == RFS_ENOTDIR) { /* the folder went away: go to the top */
        strlcpy0(fcwd, "/", sizeof fcwd);
        fn = 0;
        ferr = rfs_list(fcwd, fl_collect, 0);
    }
    for (int i = 0; i < fn; i++) {
        char p[384];
        rfs_stat_t st;
        fl_child(fents[i].name, p, sizeof p);
        if (fents[i].type == RFS_FILE && rfs_stat(p, &st) == RFS_OK) fents[i].size = st.size;
    }
    /* folders first, then by name */
    for (int i = 1; i < fn; i++) {
        fentry_t e = fents[i];
        int j = i;
        while (j > 0 && ((fents[j - 1].type != RFS_DIR && e.type == RFS_DIR) ||
                         (fents[j - 1].type == e.type && ci_cmp(fents[j - 1].name, e.name) > 0))) {
            fents[j] = fents[j - 1];
            j--;
        }
        fents[j] = e;
    }
    fsel = -1;
    for (int i = 0; i < fn && keep[0]; i++) if (!ci_cmp(fents[i].name, keep)) fsel = i;
    fgen = fs_generation;
    if (fwin) {
        char t[300];
        ksnprintf(t, sizeof t, "Files - %s", fcwd);
        wm_set_title(fwin, t);
        win_invalidate(fwin);
    }
}

static int has_ext(const char *name, const char *ext) {
    int a = strlen(name), b = strlen(ext);
    return a > b && !ci_cmp(name + a - b, ext);
}

static void fl_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w;
    if (fgen != fs_generation) fl_refresh();
    gfx_fill(cx, cy, cw, FL_TOOL, 0xebe7df);
    gfx_fill(cx, cy + FL_TOOL - 1, cw, 1, 0xc9c4ba);
    for (int i = 0; i < FL_BTNS; i++) ui_button(cx + fl_btn_x[i], cy + 6, fl_btn_w[i], 22, fl_btn_label[i], fpress == i + 1);

    int y = cy + FL_TOOL;
    gfx_fill(cx, y, cw, ch - FL_TOOL, 0xffffff);
    gfx_fill(cx, y, cw, 20, 0xf3f1ec);
    rect_t old = gfx_get_clip();
    gfx_set_clip(rect_isect(old, (rect_t){cx, y, cw - 90, 20}));
    gfx_text_bold(cx + 10, y + 6, fcwd, 0x2f6b55);
    gfx_set_clip(old);
    gfx_text_bold(cx + cw - 80, y + 6, "Size", 0x4a4f5a);
    gfx_fill(cx, y + 20, cw, 1, 0xe0ddd5);
    y += 22;
    fvis = (ch - FL_TOOL - 22 - 22) / FL_ROW;
    if (fsel >= 0 && fsel < ftop) ftop = fsel;
    if (fsel >= ftop + fvis) ftop = fsel - fvis + 1;
    if (ftop > MAX(fn - fvis, 0)) ftop = MAX(fn - fvis, 0);
    if (ferr) gfx_text(cx + 34, y + 6, rfs_strerror(ferr), 0xb03a3a);
    else if (!fn) gfx_text(cx + 34, y + 6, "This folder is empty.", 0x8a8f98);
    for (int r = ftop; r < fn && r < ftop + fvis; r++, y += FL_ROW) {
        fentry_t *e = &fents[r];
        if (r == fsel) gfx_fill(cx + 2, y, cw - 4, FL_ROW, 0xf6dde6);
        int icon = e->type == RFS_DIR ? ICON_FILES : has_ext(e->name, ".bmp") ? ICON_PAINT : ICON_NOTEPAD;
        gfx_icon(icon, cx + 10, y + 1, 1);
        gfx_set_clip(rect_isect(old, (rect_t){cx, y, cw - 90, FL_ROW}));
        gfx_text(cx + 34, y + 5, e->name, 0x1f2229);
        gfx_set_clip(old);
        if (e->type == RFS_FILE) {
            char sz[24];
            fmt_size(sz, sizeof sz, e->size);
            gfx_text(cx + cw - 80, y + 5, sz, 0x5a5f6a);
        }
    }
    char foot[160], desc[120];
    storage_describe(desc, sizeof desc, 0);
    ksnprintf(foot, sizeof foot, "%d item%s - %s", fn, fn == 1 ? "" : "s", desc);
    gfx_fill(cx, cy + ch - 20, cw, 20, 0xf3f1ec);
    gfx_set_clip(rect_isect(old, (rect_t){cx, cy + ch - 20, cw - 4, 20}));
    gfx_text(cx + 8, cy + ch - 14, foot, 0x6a6f7a);
    gfx_set_clip(old);
}

static void fl_go(const char *folder) {
    char p[256];
    if (!rfs_path_join(fcwd, folder, p, sizeof p)) return;
    strlcpy0(fcwd, p, sizeof fcwd);
    fsel = -1;
    ftop = 0;
    fl_refresh();
}

static void fl_open_sel(void) {
    if (fsel < 0 || fsel >= fn) return;
    fentry_t *e = &fents[fsel];
    char p[384];
    fl_child(e->name, p, sizeof p);
    if (e->type == RFS_DIR) fl_go(e->name);
    else if (has_ext(e->name, ".bmp")) paint_open_file(p);
    else notepad_open_file(p);
}

static void fl_report(int e, const char *what) {
    if (!e) return;
    char msg[240];
    ksnprintf(msg, sizeof msg, "Couldn't %s. %s.", what, rfs_strerror(e));
    wm_message("Files", msg);
}

static void new_file_answer(int ok, const char *name, void *ctx) {
    (void)ctx;
    if (!ok || !name[0]) return;
    char p[384];
    rfs_stat_t st;
    fl_child(name, p, sizeof p);
    if (rfs_stat(p, &st) == RFS_OK) { fl_report(RFS_EEXIST, "make the file"); return; }
    int e = rfs_write(p, "", 0);
    fl_report(e, "make the file");
    if (!e) notepad_open_file(p);
}

static void new_folder_answer(int ok, const char *name, void *ctx) {
    (void)ctx;
    if (!ok || !name[0]) return;
    char p[384];
    fl_child(name, p, sizeof p);
    fl_report(rfs_mkdir(p), "make the folder");
}

static char rename_from[128];
static void rename_answer(int ok, const char *name, void *ctx) {
    (void)ctx;
    if (!ok || !name[0] || !strcmp(name, rename_from)) return;
    char a[384], b[384];
    fl_child(rename_from, a, sizeof a);
    fl_child(name, b, sizeof b);
    fl_report(rfs_rename(a, b), "rename it");
}

static char delete_name[128];
static void delete_answer(int ok, const char *text, void *ctx) {
    (void)text; (void)ctx;
    if (!ok) return;
    char p[384];
    fl_child(delete_name, p, sizeof p);
    fl_report(rfs_remove(p), "delete it");
}

static void fl_action(int b) {
    char msg[300];
    fentry_t *e = fsel >= 0 && fsel < fn ? &fents[fsel] : 0;
    switch (b) {
    case 1: if (strcmp(fcwd, "/")) fl_go(".."); break;
    case 2: fl_open_sel(); break;
    case 3: wm_dialog("New file", "Name for the new file:", "Make it", "Cancel", "Untitled.txt", new_file_answer, 0); break;
    case 4: wm_dialog("New folder", "Name for the new folder:", "Make it", "Cancel", "New folder", new_folder_answer, 0); break;
    case 5:
        if (!e) break;
        strlcpy0(rename_from, e->name, sizeof rename_from);
        ksnprintf(msg, sizeof msg, "New name for \"%s\":", e->name);
        wm_dialog("Rename", msg, "Rename", "Cancel", e->name, rename_answer, 0);
        break;
    case 6:
        if (!e) break;
        strlcpy0(delete_name, e->name, sizeof delete_name);
        ksnprintf(msg, sizeof msg, e->type == RFS_DIR ? "Delete the folder \"%s\"? It has to be empty." : "Delete \"%s\"? This can't be undone.", e->name);
        wm_dialog("Delete", msg, "Delete", "Keep it", 0, delete_answer, 0);
        break;
    }
}

static void fl_mouse(window_t *w, int ev, int x, int y, int b) {
    int hit = 0;
    if (y >= 6 && y < 28)
        for (int i = 0; i < FL_BTNS; i++) if (x >= fl_btn_x[i] && x < fl_btn_x[i] + fl_btn_w[i]) hit = i + 1;
    if (ev == ME_DOWN) {
        if (y < FL_TOOL) fpress = hit;
        else if (y >= FL_TOOL + 22) {
            int r = (y - FL_TOOL - 22) / FL_ROW;
            if (r < fvis) {
                r += ftop;
                fsel = r < fn ? r : -1;
                if (fsel >= 0 && (b & MB_DOUBLE)) { fl_open_sel(); return; }
            }
        }
    } else if (ev == ME_UP && fpress) {
        int p = fpress;
        fpress = 0;
        if (p == hit) { win_invalidate(w); fl_action(p); return; }
    }
    win_invalidate(w);
}

static void fl_key(window_t *w, int k) {
    if (k == KEY_UP && fsel > 0) fsel--;
    else if (k == KEY_DOWN && fsel < fn - 1) fsel++;
    else if (k == KEY_PGUP) fsel = MAX(fsel - fvis, 0);
    else if (k == KEY_PGDN) fsel = MIN(fsel + fvis, fn - 1);
    else if (k == '\n') { fl_open_sel(); return; }
    else if (k == '\b') fl_action(1);
    else if (k == KEY_DEL) fl_action(6);
    win_invalidate(w);
}

static void fl_tick(window_t *w) { if (fgen != fs_generation) win_invalidate(w); }
static void fl_close(window_t *w) { (void)w; fwin = 0; }

void files_open(void) {
    if (fwin) { wm_focus(fwin); return; }
    fwin = wm_create("Files", 460, 380, APP_FILES);
    if (!fwin) return;
    fwin->draw = fl_draw;
    fwin->mouse = fl_mouse;
    fwin->key = fl_key;
    fwin->tick = fl_tick;
    fwin->close = fl_close;
    fl_refresh();
}

void files_open_at(const char *folder) {
    strlcpy0(fcwd, folder, sizeof fcwd);
    fsel = -1;
    ftop = 0;
    files_open();
    fl_refresh();
}
