/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - dialog boxes: messages, questions and "type a name" prompts */
#include "kernel.h"

#define DLG_W 400
#define DLG_TEXT_MAX 255
#define DLG_LINES 14

typedef struct {
    char lines[DLG_LINES][64];
    int nlines;
    char ok[24], cancel[24];
    int has_input, has_cancel;
    char text[DLG_TEXT_MAX + 1];
    int len, sel_all;
    int pressed; /* 1 = ok, 2 = cancel */
    int done;
    dialog_cb cb;
    void *ctx;
} dialog_t;

static void dlg_layout(dialog_t *d, const char *msg) {
    /* word-wrap into lines of at most 60 characters, honouring newlines */
    const char *p = msg;
    while (*p && d->nlines < DLG_LINES) {
        int n = 0, last_space = -1;
        while (p[n] && p[n] != '\n' && n < 60) { if (p[n] == ' ') last_space = n; n++; }
        int take = n;
        if (p[n] && p[n] != '\n' && last_space > 0) take = last_space;
        memcpy(d->lines[d->nlines], p, take);
        d->lines[d->nlines][take] = 0;
        d->nlines++;
        p += take;
        if (*p == ' ' || *p == '\n') p++;
    }
}

static int dlg_btn_x(window_t *w, int which) { /* 1 = ok, 2 = cancel */
    int cw = win_cw(w);
    if (which == 1) return cw - 16 - 96;
    return cw - 16 - 96 - 8 - 96;
}

static void dlg_draw(window_t *w, int cx, int cy, int cw, int ch) {
    dialog_t *d = w->data;
    gfx_fill(cx, cy, cw, ch, 0xf5f2ec);
    int y = cy + 16;
    for (int i = 0; i < d->nlines; i++, y += 14) gfx_text(cx + 16, y, d->lines[i], 0x23262e);
    if (d->has_input) {
        y += 6;
        gfx_fill(cx + 16, y, cw - 32, 24, 0xffffff);
        gfx_rect(cx + 16, y, cw - 32, 24, wm_is_focused(w) ? 0x2f6b55 : 0xa9adb6);
        int maxc = (cw - 44) / CHAR_W;
        int start = d->len > maxc ? d->len - maxc : 0;
        if (d->sel_all && d->len) gfx_fill(cx + 21, y + 5, (d->len - start) * CHAR_W + 2, 14, 0xf6dde6);
        gfx_text(cx + 22, y + 8, d->text + start, 0x1f2229);
        if (wm_is_focused(w) && (ticks / 50) % 2 == 0) gfx_fill(cx + 22 + (d->len - start) * CHAR_W, y + 5, 2, 14, 0xd2577a);
    }
    int by = cy + ch - 16 - 26;
    ui_button(cx + dlg_btn_x(w, 1), by, 96, 26, d->ok, d->pressed == 1);
    if (d->has_cancel) ui_button(cx + dlg_btn_x(w, 2), by, 96, 26, d->cancel, d->pressed == 2);
}

static void dlg_finish(window_t *w, int ok) {
    dialog_t *d = w->data;
    if (d->done) return;
    d->done = 1;
    static char text[DLG_TEXT_MAX + 1];
    strlcpy0(text, d->text, sizeof text);
    dialog_cb cb = d->cb;
    void *ctx = d->ctx;
    wm_close(w); /* frees d */
    if (cb) cb(ok, text, ctx);
}

static void dlg_close(window_t *w) {
    dialog_t *d = w->data;
    if (!d->done) { /* closed with the X: same as cancelling */
        d->done = 1;
        if (d->cb) d->cb(0, d->text, d->ctx);
    }
    kfree(d);
    w->data = 0;
}

static void dlg_key(window_t *w, int k) {
    dialog_t *d = w->data;
    if (k == '\n') { dlg_finish(w, 1); return; }
    if (k == 27) { dlg_finish(w, 0); return; }
    if (d->has_input) {
        if (k == '\b') {
            if (d->sel_all) { d->len = 0; d->text[0] = 0; }
            else if (d->len) d->text[--d->len] = 0;
            d->sel_all = 0;
        } else if (k >= 32 && k < 127) {
            if (d->sel_all) { d->len = 0; d->text[0] = 0; d->sel_all = 0; }
            if (d->len < DLG_TEXT_MAX) { d->text[d->len++] = (char)k; d->text[d->len] = 0; }
        } else if (k == KEY_LEFT || k == KEY_RIGHT || k == KEY_END || k == KEY_HOME) {
            d->sel_all = 0;
        }
    }
    win_invalidate(w);
}

static void dlg_mouse(window_t *w, int ev, int x, int y, int b) {
    (void)b;
    dialog_t *d = w->data;
    int by = win_ch(w) - 16 - 26;
    int hit = 0;
    if (y >= by && y < by + 26) {
        if (x >= dlg_btn_x(w, 1) && x < dlg_btn_x(w, 1) + 96) hit = 1;
        else if (d->has_cancel && x >= dlg_btn_x(w, 2) && x < dlg_btn_x(w, 2) + 96) hit = 2;
    }
    if (ev == ME_DOWN) d->pressed = hit;
    else if (ev == ME_UP) {
        int p = d->pressed;
        d->pressed = 0;
        if (p && p == hit) { dlg_finish(w, p == 1); return; }
    }
    win_invalidate(w);
}

static u32 dlg_blink;
static void dlg_tick(window_t *w) {
    if (ticks / 50 != dlg_blink) { dlg_blink = ticks / 50; win_invalidate(w); }
}

window_t *wm_dialog(const char *title, const char *message, const char *ok, const char *cancel,
                    const char *initial, dialog_cb cb, void *ctx) {
    dialog_t *d = kmalloc(sizeof *d);
    if (!d) return 0;
    dlg_layout(d, message);
    strlcpy0(d->ok, ok ? ok : "OK", sizeof d->ok);
    if (cancel) { strlcpy0(d->cancel, cancel, sizeof d->cancel); d->has_cancel = 1; }
    if (initial) {
        d->has_input = 1;
        strlcpy0(d->text, initial, sizeof d->text);
        d->len = strlen(d->text);
        d->sel_all = 1; /* typing replaces the suggestion */
    }
    d->cb = cb;
    d->ctx = ctx;
    int h = 16 + d->nlines * 14 + (d->has_input ? 6 + 24 + 8 : 0) + 16 + 26 + 16;
    window_t *w = wm_create(title, DLG_W, h, APP_DIALOG);
    if (!w) { kfree(d); return 0; }
    w->data = d;
    w->draw = dlg_draw;
    w->key = dlg_key;
    w->mouse = dlg_mouse;
    w->close = dlg_close;
    if (d->has_input) w->tick = dlg_tick;
    w->x = (SW - w->w) / 2;
    w->y = (SH - TASKBAR_H - w->h) / 2 - 40;
    wm_dirty_all();
    return w;
}

void wm_message(const char *title, const char *message) {
    wm_dialog(title, message, "OK", 0, 0, 0, 0);
}
