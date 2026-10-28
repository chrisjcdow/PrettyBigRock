/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* FynbosOS - Terminal app with a small command shell */
#include "kernel.h"

#define T_COLS 76
#define T_ROWS 25
#define T_HIST 300
#define T_INMAX 90

enum { K_NORMAL, K_GOOD, K_BAD, K_CMD, K_HEAD };
static const u32 kind_color[] = {0xd8dce6, 0x8fd18a, 0xf08a8a, 0xffffff, 0xf2a0b8};

static char lines[T_HIST][T_COLS + 1];
static u8 kinds[T_HIST][T_COLS];
static int scroll;
static int nlines = 1;
static char in[T_INMAX + 1];
static int inlen;
static char hist[16][T_INMAX + 1];
static int nhist, hpos;
static window_t *twin;

static const char *PROMPT = "fynbos> ";

static void t_newline(void) {
    if (nlines == T_HIST) {
        memmove(lines[0], lines[1], (T_HIST - 1) * (T_COLS + 1));
        memmove(kinds[0], kinds[1], (T_HIST - 1) * T_COLS);
    } else nlines++;
    lines[nlines - 1][0] = 0;
}

static void t_puts(const char *s, int kind) {
    for (; *s; s++) {
        char *l = lines[nlines - 1];
        int len = (int)strlen(l);
        if (*s == '\n') { t_newline(); continue; }
        if (len >= T_COLS) { t_newline(); l = lines[nlines - 1]; len = 0; }
        kinds[nlines - 1][len] = kind;
        l[len] = *s;
        l[len + 1] = 0;
    }
}

static void t_printf(int kind, const char *fmt, ...) {
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    kvsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    t_puts(buf, kind);
}

static void t_clear(void) {
    nlines = 1;
    lines[0][0] = 0;
}

/* ---------------------------------------------------------------- commands */
static char *next_word(char **p) {
    while (**p == ' ') (*p)++;
    char *w = *p;
    while (**p && **p != ' ') (*p)++;
    if (**p) *(*p)++ = 0;
    while (**p == ' ') (*p)++;
    return w;
}

static int ieq_prefix(const char *a, const char *b) {
    /* is lowercase 'a' a prefix of 'b', ignoring case? */
    if (!*a) return 0;
    for (; *a; a++, b++) {
        char x = *a, y = *b;
        if (y >= 'A' && y <= 'Z') y |= 0x20;
        if (x >= 'A' && x <= 'Z') x |= 0x20;
        if (x != y) return 0;
    }
    return 1;
}

static const char *fortunes[] = {
    "Every kernel starts with a single boot sector.",
    "Fynbos grows back stronger after a fire. So do rebooted machines.",
    "There is no cloud, only the tablecloth over the mountain.",
    "A bug in the hand is worth two in the backlog.",
    "The south-easter clears the sky; the reset button clears the RAM.",
    "Small programs have small bugs. Mostly.",
    "When in doubt, print it out.",
    "Interrupts are just the hardware asking for attention.",
};

static void cmd_help(void) {
    t_puts("Commands:\n", K_HEAD);
    t_puts("  help              this list\n"
           "  clear             clear the screen\n"
           "  about             about FynbosOS\n"
           "  fetch             system summary\n"
           "  time | date       show the clock\n"
           "  tz <hours>        set the clock offset from the hardware time\n"
           "  uptime            time since boot\n"
           "  mem               memory information\n"
           "  echo <text>       print text\n"
           "  calc <expr>       evaluate maths, e.g. calc (2+3)*4^2\n"
           "  ls                list files\n"
           "  cat <file>        show a file\n"
           "  write <file> <t>  replace a file's contents\n"
           "  append <file> <t> add a line to a file\n"
           "  rm <file>         delete a file\n"
           "  edit <file>       open a file in Notepad\n"
           "  open <app>        notepad, files, calculator, paint, snake, about\n"
           "  fortune           a little wisdom\n"
           "  (PgUp / PgDn scroll, Up / Down recall commands)\n"
           "  reboot | shutdown\n", K_NORMAL);
}

static void cmd_fetch(void) {
    rtc_time_t t;
    rtc_read(&t);
    u32 up = ticks / 100;
    const char *art[] = {
        "    .  *  .   ",
        "  * \\ | / *   ",
        "   -( @@ )-   ",
        "  * / | \\ *   ",
        "      |       ",
        "    \\ | /     ",
        "     \\|/      ",
    };
    char info[7][64];
    ksnprintf(info[0], 64, "%s %s", OS_NAME, OS_VERSION);
    ksnprintf(info[1], 64, "------------");
    ksnprintf(info[2], 64, "Kernel:  32-bit x86, written in C");
    ksnprintf(info[3], 64, "Display: %d x %d, %d-bit", SW, SH, SBPP);
    ksnprintf(info[4], 64, "Memory:  %u MB", mem_total_mb());
    ksnprintf(info[5], 64, "Uptime:  %u min %u s", up / 60, up % 60);
    ksnprintf(info[6], 64, "Files:   %d in RAM", fs_count());
    for (int i = 0; i < 7; i++) {
        t_puts(art[i], K_HEAD);
        t_puts(info[i], i == 0 ? K_GOOD : K_NORMAL);
        t_puts("\n", K_NORMAL);
    }
}

static void run(char *line) {
    char *p = line;
    char *cmd = next_word(&p);
    if (!*cmd) return;
    if (!strcmp(cmd, "help") || !strcmp(cmd, "?")) cmd_help();
    else if (!strcmp(cmd, "clear") || !strcmp(cmd, "cls")) t_clear();
    else if (!strcmp(cmd, "about") || !strcmp(cmd, "ver")) {
        t_printf(K_GOOD, "%s %s\n", OS_NAME, OS_VERSION);
        t_puts("A small hobby operating system, written from scratch in C and\n"
               "x86 assembly: boot loader, kernel, drivers, window manager and apps.\n", K_NORMAL);
    } else if (!strcmp(cmd, "fetch") || !strcmp(cmd, "neofetch")) cmd_fetch();
    else if (!strcmp(cmd, "time") || !strcmp(cmd, "date")) {
        rtc_time_t t;
        rtc_read(&t);
        t_printf(K_NORMAL, "%04d-%02d-%02d %02d:%02d:%02d (offset %d h)\n", t.year, t.mon, t.day, t.hour, t.min, t.sec, tz_offset);
    } else if (!strcmp(cmd, "tz")) {
        if (!*p) t_printf(K_NORMAL, "Clock offset is %d hours. Use e.g. 'tz 2' for SAST.\n", tz_offset);
        else {
            int v = katoi(p);
            if (v < -12 || v > 14) t_puts("Offset must be between -12 and 14.\n", K_BAD);
            else { tz_offset = v; wm_refresh_clock(); t_printf(K_GOOD, "Clock offset set to %d hours.\n", v); }
        }
    } else if (!strcmp(cmd, "uptime")) {
        u32 s = ticks / 100;
        t_printf(K_NORMAL, "Up %u h %u min %u s\n", s / 3600, (s / 60) % 60, s % 60);
    } else if (!strcmp(cmd, "mem")) {
        t_printf(K_NORMAL, "Installed memory: %u MB\n", mem_total_mb());
        t_printf(K_NORMAL, "Kernel heap used: %u KB (screen buffers, files, canvases)\n", heap_used() / 1024);
    } else if (!strcmp(cmd, "echo")) {
        t_printf(K_NORMAL, "%s\n", p);
    } else if (!strcmp(cmd, "calc")) {
        int ok;
        double v = eval_expr(p, &ok);
        char buf[48];
        if (!ok) t_puts("Couldn't work that out. Try: calc 12*(3+4)/2\n", K_BAD);
        else { fmt_double(buf, sizeof buf, v); t_printf(K_GOOD, "= %s\n", buf); }
    } else if (!strcmp(cmd, "ls") || !strcmp(cmd, "dir")) {
        int n = 0, total = 0;
        for (int i = 0; i < FS_MAX_FILES; i++) {
            if (!fs_valid(i)) continue;
            t_printf(K_NORMAL, "  %-28s %6d bytes\n", fs_name(i), fs_size(i));
            n++;
            total += fs_size(i);
        }
        t_printf(K_GOOD, "%d files, %d bytes\n", n, total);
    } else if (!strcmp(cmd, "cat") || !strcmp(cmd, "type")) {
        int i = fs_find(next_word(&p));
        if (i < 0) t_puts("No such file.\n", K_BAD);
        else {
            t_puts(fs_data(i), K_NORMAL);
            if (lines[nlines - 1][0]) t_puts("\n", K_NORMAL);
        }
    } else if (!strcmp(cmd, "write") || !strcmp(cmd, "append")) {
        char *name = next_word(&p);
        if (!*name) { t_printf(K_BAD, "Usage: %s <file> <text>\n", cmd); return; }
        int i = fs_create(name);
        if (i < 0) { t_puts("Can't create that file (name too long, or the disk is full).\n", K_BAD); return; }
        char buf[T_INMAX + 2];
        ksnprintf(buf, sizeof buf, "%s\n", p);
        if (cmd[0] == 'w') fs_write(i, buf, (int)strlen(buf));
        else fs_append(i, buf, (int)strlen(buf));
        t_printf(K_GOOD, "%s: %d bytes\n", name, fs_size(i));
    } else if (!strcmp(cmd, "rm") || !strcmp(cmd, "del")) {
        int i = fs_find(next_word(&p));
        if (i < 0) t_puts("No such file.\n", K_BAD);
        else { fs_delete(i); t_puts("Deleted.\n", K_GOOD); }
    } else if (!strcmp(cmd, "edit")) {
        char *name = next_word(&p);
        notepad_open_file(*name ? name : "untitled.txt");
    } else if (!strcmp(cmd, "open")) {
        char *what = next_word(&p);
        for (int i = 0; i < APP_COUNT; i++)
            if (ieq_prefix(what, app_names[i])) { app_open(i); return; }
        t_puts("Unknown app. Try: notepad, files, calculator, paint, snake, about\n", K_BAD);
    } else if (!strcmp(cmd, "fortune")) {
        t_printf(K_HEAD, "%s\n", fortunes[krand() % (sizeof fortunes / sizeof *fortunes)]);
    } else if (!strcmp(cmd, "reboot")) reboot();
    else if (!strcmp(cmd, "shutdown")) {
        power_off();
        t_puts("Power off isn't supported on this machine.\n", K_BAD);
    } else t_printf(K_BAD, "Unknown command '%s'. Type 'help' for a list.\n", cmd);
}

/* ---------------------------------------------------------------- window callbacks */
static void t_draw(window_t *w, int cx, int cy, int cw, int ch) {
    (void)w;
    gfx_fill(cx, cy, cw, ch, 0x14161d);
    int first = nlines - T_ROWS - scroll;
    if (first < 0) first = 0;
    int last = MIN(nlines, first + T_ROWS);
    int y = cy + 8;
    for (int i = first; i < last; i++, y += LINE_H) {
        for (int c = 0; lines[i][c]; c++) gfx_char(cx + 8 + c * CHAR_W, y, lines[i][c], kind_color[kinds[i][c]], 1);
        if (i == nlines - 1) {
            int x = cx + 8 + gfx_text_w(lines[i]);
            gfx_text(x, y, PROMPT, 0x8fd18a);
            x += gfx_text_w(PROMPT);
            gfx_text(x, y, in, 0xffffff);
            x += gfx_text_w(in);
            if ((ticks / 50) % 2 == 0 && wm_is_focused(w)) gfx_fill(x, y - 1, CHAR_W, 11, 0x8fd18a);
        }
    }
    if (scroll) {
        const char *s = "-- scrolled back: PgDn to return --";
        gfx_fill(cx + cw - gfx_text_w(s) - 16, cy + 2, gfx_text_w(s) + 12, 14, 0x3a3f52);
        gfx_text(cx + cw - gfx_text_w(s) - 10, cy + 5, s, 0xf2a0b8);
    }
}

static void t_key(window_t *w, int k) {
    if (k == KEY_PGUP || k == KEY_PGDN) {
        int maxs = MAX(nlines - T_ROWS, 0);
        scroll += k == KEY_PGUP ? T_ROWS - 2 : -(T_ROWS - 2);
        scroll = MIN(MAX(scroll, 0), maxs);
        win_invalidate(w);
        return;
    }
    scroll = 0;
    if (k == '\n') {
        char cmdline[T_INMAX + 1];
        strlcpy0(cmdline, in, sizeof cmdline);
        t_puts(PROMPT, K_GOOD);
        t_puts(in, K_CMD);
        t_puts("\n", K_NORMAL);
        if (inlen) {
            if (nhist == 16) { memmove(hist[0], hist[1], 15 * (T_INMAX + 1)); nhist--; }
            strlcpy0(hist[nhist++], in, T_INMAX + 1);
        }
        hpos = nhist;
        inlen = 0;
        in[0] = 0;
        run(cmdline);
        if (lines[nlines - 1][0]) t_newline();
        if (!twin) return; /* the command may have closed us */
    } else if (k == '\b') {
        if (inlen) in[--inlen] = 0;
    } else if (k == KEY_UP) {
        if (hpos > 0) { hpos--; strlcpy0(in, hist[hpos], sizeof in); inlen = (int)strlen(in); }
    } else if (k == KEY_DOWN) {
        if (hpos < nhist - 1) { hpos++; strlcpy0(in, hist[hpos], sizeof in); }
        else { hpos = nhist; in[0] = 0; }
        inlen = (int)strlen(in);
    } else if (k == (KEY_CTRL | 'l')) {
        t_clear();
    } else if (k == (KEY_CTRL | 'c')) {
        t_puts(PROMPT, K_GOOD);
        t_puts(in, K_CMD);
        t_puts("^C\n", K_BAD);
        inlen = 0;
        in[0] = 0;
    } else if (k >= 32 && k < 127 && inlen < T_INMAX - (int)strlen(PROMPT)) {
        in[inlen++] = (char)k;
        in[inlen] = 0;
    }
    win_invalidate(w);
}

static u32 last_blink;
static void t_tick(window_t *w) {
    if (ticks / 50 != last_blink) { last_blink = ticks / 50; win_invalidate(w); }
}

static void t_close(window_t *w) { (void)w; twin = 0; }

void term_open(void) {
    twin = wm_create("Terminal", T_COLS * CHAR_W + 16, T_ROWS * LINE_H + 10, APP_TERMINAL);
    if (!twin) return;
    twin->draw = t_draw;
    twin->key = t_key;
    twin->tick = t_tick;
    twin->close = t_close;
    if (nlines == 1 && !lines[0][0]) {
        t_printf(K_HEAD, "%s %s terminal\n", OS_NAME, OS_VERSION);
        t_puts("Type 'help' to see what I can do.\n\n", K_NORMAL);
    }
}
