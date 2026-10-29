/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - Terminal app with a small command shell */
#include "kernel.h"
#include "rockfs.h"

#define T_COLS 76
#define T_ROWS 25
#define T_HIST 300
#define T_INMAX 90

enum { K_NORMAL, K_GOOD, K_BAD, K_CMD, K_HEAD, K_SPUD, K_SUN, K_SEA };
static const u32 kind_color[] = {0xd8dce6, 0x8fd18a, 0xf08a8a, 0xffffff, 0xf2a0b8, 0xd9a066, 0xffc070, 0x6fa8e8};

static char lines[T_HIST][T_COLS + 1];
static u8 kinds[T_HIST][T_COLS];
static int scroll;
static int nlines = 1;
static char in[T_INMAX + 1];
static int inlen;
static char hist[16][T_INMAX + 1];
static int nhist, hpos;
static window_t *twin;

static char cwd[256] = "/";

static const char *prompt(void) {
    static char pr[48];
    int n = strlen(cwd);
    if (n <= 30) ksnprintf(pr, sizeof pr, "rock:%s> ", cwd);
    else ksnprintf(pr, sizeof pr, "rock:...%s> ", cwd + n - 27);
    return pr;
}

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
/* the next word, or "a quoted phrase" for names with spaces */
static char *next_word(char **p) {
    while (**p == ' ') (*p)++;
    char *w = *p;
    if (**p == '"') {
        w = ++(*p);
        while (**p && **p != '"') (*p)++;
    } else {
        while (**p && **p != ' ') (*p)++;
    }
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
    "The mountain was here first. It can wait for your build to finish.",
    "Big rock. Pretty rock. Pretty big rock.",
    "Interrupts are just the hardware asking for attention.",
};

static void fs_err(const char *what, int e) {
    t_printf(K_BAD, "%s: %s.\n", what, rfs_strerror(e));
}

static int abs_path(const char *rel, char *out) {
    if (rfs_path_join(cwd, rel, out, 256)) return 1;
    t_puts("That path is too long.\n", K_BAD);
    return 0;
}

#define LS_MAX 256
typedef struct { int n; char (*names)[80]; u8 *types; } ls_ctx_t;
static int ls_fn(void *ctx, const char *name, u32 ino, int type) {
    (void)ino;
    ls_ctx_t *c = ctx;
    if (c->n >= LS_MAX) return 1;
    strlcpy0(c->names[c->n], name, 80);
    c->types[c->n++] = type;
    return 0;
}

static int ls_before(ls_ctx_t *c, int a, int b) { /* folders first, then by name */
    if (c->types[a] != c->types[b]) return c->types[a] == RFS_DIR;
    for (const char *x = c->names[a], *y = c->names[b];; x++, y++) {
        char p = *x >= 'A' && *x <= 'Z' ? *x + 32 : *x, q = *y >= 'A' && *y <= 'Z' ? *y + 32 : *y;
        if (p != q || !p) return p < q;
    }
}

static void cmd_ls(const char *where) {
    char path[256];
    if (!abs_path(*where ? where : ".", path)) return;
    rfs_stat_t st;
    int e = rfs_stat(path, &st);
    if (e) { fs_err(*where ? where : path, e); return; }
    if (st.type == RFS_FILE) {
        char sz[24];
        fmt_size(sz, sizeof sz, st.size);
        t_printf(K_NORMAL, "  %s  %s\n", where, sz);
        return;
    }
    ls_ctx_t c = {0, kmalloc(LS_MAX * 80), kmalloc(LS_MAX)};
    int *order = kmalloc(LS_MAX * sizeof(int));
    if (!c.names || !c.types || !order) { kfree(c.names); kfree(c.types); kfree(order); return; }
    e = rfs_list(path, ls_fn, &c);
    if (e) fs_err(path, e);
    else if (!c.n) t_puts("  (empty)\n", K_NORMAL);
    for (int i = 0; i < c.n; i++) {
        int j = i;
        while (j > 0 && ls_before(&c, i, order[j - 1])) { order[j] = order[j - 1]; j--; }
        order[j] = i;
    }
    for (int i = 0; i < c.n; i++) {
        int k = order[i];
        if (c.types[k] == RFS_DIR) t_printf(K_HEAD, "  %s/\n", c.names[k]);
        else {
            char p2[256], sz[24];
            rfs_stat_t fst;
            sz[0] = 0;
            if (rfs_path_join(path, c.names[k], p2, sizeof p2) && !rfs_stat(p2, &fst)) fmt_size(sz, sizeof sz, fst.size);
            t_printf(K_NORMAL, "  %-40s %s\n", c.names[k], sz);
        }
    }
    if (c.n == LS_MAX) t_puts("  (and more)\n", K_NORMAL);
    kfree(c.names); kfree(c.types); kfree(order);
}

static void cmd_disk(void) {
    rfs_info_t info;
    rfs_info(&info);
    char d[160], tot[24], fr[24], res[24];
    storage_describe(d, sizeof d, 0);
    t_printf(K_GOOD, "%s\n", d);
    fmt_size(tot, sizeof tot, info.total_bytes);
    fmt_size(fr, sizeof fr, info.free_bytes);
    fmt_size(res, sizeof res, info.reserve_bytes);
    t_printf(K_NORMAL, "  Filesystem: RockFS 1, label \"%s\"%s\n", info.label, info.readonly ? " (read-only)" : "");
    t_printf(K_NORMAL, "  Size %s, free %s (%s kept back for tidying up)\n", tot, fr, res);
    t_printf(K_NORMAL, "  %u files and folders, mounted %u times\n", info.items, info.mount_count);
    const u8 *u = info.uuid;
    t_printf(K_NORMAL, "  ID %02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x\n",
             u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]);
}

static void cmd_help(void) {
    t_puts("Commands:\n", K_HEAD);
    t_puts("  help              this list\n"
           "  clear             clear the screen\n"
           "  about             about PrettyBigRock\n"
           "  fetch             system summary\n"
           "  time | date       show the clock\n"
           "  tz <hours>        set the clock offset from the hardware time\n"
           "  uptime            time since boot\n"
           "  mem               memory information\n"
           "  echo <text>       print text\n"
           "  calc <expr>       evaluate maths, e.g. calc (2+3)*4^2\n"
           "  ls [folder]       list a folder\n"
           "  cd <folder>       change folder (cd .. goes up)\n"
           "  mkdir <name>      make a folder\n"
           "  cat <file>        show a file\n"
           "  write <file> <t>  replace a file's contents\n"
           "  append <file> <t> add a line to a file\n"
           "  cp <from> <to>    copy a file\n"
           "  mv <from> <to>    move or rename\n"
           "  rm <name>         delete a file or an empty folder\n"
           "  edit <file>       open a file in Notepad\n"
           "  disk              data disk details\n"
           "  check [fix]       check the disk for problems\n"
           "  (\"quote names\" that have spaces)\n"
           "  open <app>        notepad, files, calculator, paint, snake, about\n"
           "  fortune           a little wisdom\n"
           "  (PgUp / PgDn scroll, Up / Down recall commands)\n"
           "  reboot | shutdown\n", K_NORMAL);
}

static void cmd_fetch(void) {
    rtc_time_t t;
    rtc_read(&t);
    u32 up = ticks / 100;
    /* Table Mountain at sunset: s = sun, c = cloud, m = mountain, w = water */
    const char *art[] = {
        "            \\|/  ",
        "     ~~~~~~~ -o- ",
        "  /\\/_______\\    ",
        " /            \\  ",
        "/              \\^",
        "~~~~~~~~~~~~~~~~~",
        "  ~   ~   ~   ~  ",
    };
    const char *paint[] = {
        "            sss  ",
        "     ccccccc sss ",
        "  mmmmmmmmmmm    ",
        " mmmmmmmmmmmmmm  ",
        "mmmmmmmmmmmmmmmmm",
        "wwwwwwwwwwwwwwwww",
        "wwwwwwwwwwwwwwwww",
    };
    char info[7][64];
    ksnprintf(info[0], 64, "%s %s", OS_NAME, OS_VERSION);
    ksnprintf(info[1], 64, "------------");
    ksnprintf(info[2], 64, "Kernel:  32-bit x86, written in C");
    ksnprintf(info[3], 64, "Display: %d x %d, %d-bit", SW, SH, SBPP);
    ksnprintf(info[4], 64, "Memory:  %u MB", mem_total_mb());
    ksnprintf(info[5], 64, "Uptime:  %u min %u s", up / 60, up % 60);
    char d[48];
    storage_describe(d, sizeof d, 1);
    ksnprintf(info[6], 64, "Disk:    %s", d);
    for (int i = 0; i < 7; i++) {
        for (int j = 0; art[i][j]; j++) {
            char ch[2] = {art[i][j], 0};
            char k = paint[i][j];
            t_puts(ch, k == 's' ? K_SUN : k == 'c' ? K_CMD : k == 'w' ? K_SEA : K_HEAD);
        }
        t_puts(" ", K_NORMAL);
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
        cmd_ls(next_word(&p));
    } else if (!strcmp(cmd, "pwd")) {
        t_printf(K_NORMAL, "%s\n", cwd);
    } else if (!strcmp(cmd, "cd")) {
        char *where = next_word(&p);
        char path[256];
        rfs_stat_t st;
        if (!*where) where = "/";
        if (!rfs_path_join(cwd, where, path, sizeof path)) { t_puts("That path is too long.\n", K_BAD); return; }
        int e = rfs_stat(path, &st);
        if (e) { fs_err(where, e); return; }
        if (st.type != RFS_DIR) { t_printf(K_BAD, "%s isn't a folder.\n", where); return; }
        strlcpy0(cwd, path, sizeof cwd);
    } else if (!strcmp(cmd, "mkdir") || !strcmp(cmd, "md")) {
        char *name = next_word(&p), path[256];
        if (!*name) { t_puts("Usage: mkdir <name>\n", K_BAD); return; }
        if (!abs_path(name, path)) return;
        int e = rfs_mkdir(path);
        if (e) fs_err(name, e); else t_printf(K_GOOD, "Made %s\n", path);
    } else if (!strcmp(cmd, "cat") || !strcmp(cmd, "type")) {
        char *name = next_word(&p), path[256];
        if (!*name) { t_puts("Usage: cat <file>\n", K_BAD); return; }
        if (!abs_path(name, path)) return;
        char *buf = kmalloc(32001);
        u32 got = 0;
        int e = buf ? rfs_read(path, buf, 32000, &got) : RFS_ENOMEM;
        if (e) { fs_err(name, e); kfree(buf); return; }
        buf[got] = 0;
        for (u32 i = 0; i < got; i++) if (buf[i] == '\r' || (buf[i] < 32 && buf[i] != '\n' && buf[i] != '\t')) buf[i] = '.';
        t_puts(buf, K_NORMAL);
        if (lines[nlines - 1][0]) t_puts("\n", K_NORMAL);
        rfs_stat_t st;
        if (!rfs_stat(path, &st) && st.size > got) t_printf(K_HEAD, "(showing the first %u bytes)\n", got);
        kfree(buf);
    } else if (!strcmp(cmd, "write") || !strcmp(cmd, "append")) {
        char *name = next_word(&p), path[256];
        if (!*name) { t_printf(K_BAD, "Usage: %s <file> <text>\n", cmd); return; }
        if (!abs_path(name, path)) return;
        char *buf = kmalloc(32002);
        if (!buf) return;
        u32 got = 0;
        if (cmd[0] == 'a') {
            int e = rfs_read(path, buf, 30000, &got);
            if (e && e != RFS_ENOENT) { fs_err(name, e); kfree(buf); return; }
            if (e) got = 0;
        }
        ksnprintf(buf + got, 32002 - got, "%s\n", p);
        int e = rfs_write(path, buf, strlen(buf));
        if (e) fs_err(name, e); else t_printf(K_GOOD, "%s: %u bytes\n", name, (u32)strlen(buf));
        kfree(buf);
    } else if (!strcmp(cmd, "cp") || !strcmp(cmd, "copy")) {
        char *a = next_word(&p), *b = next_word(&p), pa[256], pb[256];
        if (!*a || !*b) { t_puts("Usage: cp <from> <to>\n", K_BAD); return; }
        if (!abs_path(a, pa) || !abs_path(b, pb)) return;
        rfs_stat_t st;
        if (!rfs_stat(pb, &st) && st.type == RFS_DIR) { /* copying into a folder keeps the name */
            const char *base = a;
            for (const char *q = a; *q; q++) if (*q == '/') base = q + 1;
            if (!rfs_path_join(pb, base, pb, sizeof pb)) return;
        }
        int e = rfs_stat(pa, &st);
        if (e) { fs_err(a, e); return; }
        if (st.type != RFS_FILE) { t_puts("cp copies files, not folders.\n", K_BAD); return; }
        if (st.size > 4 * 1024 * 1024) { t_puts("That file is too big to copy here (4 MB at most).\n", K_BAD); return; }
        char *buf = kmalloc((u32)st.size + 1);
        u32 got;
        if (!buf) { fs_err(a, RFS_ENOMEM); return; }
        e = rfs_read(pa, buf, (u32)st.size, &got);
        if (!e) e = rfs_write(pb, buf, got);
        if (e) fs_err(b, e); else t_printf(K_GOOD, "Copied to %s\n", pb);
        kfree(buf);
    } else if (!strcmp(cmd, "mv") || !strcmp(cmd, "move") || !strcmp(cmd, "ren")) {
        char *a = next_word(&p), *b = next_word(&p), pa[256], pb[256];
        if (!*a || !*b) { t_puts("Usage: mv <from> <to>\n", K_BAD); return; }
        if (!abs_path(a, pa) || !abs_path(b, pb)) return;
        rfs_stat_t st;
        if (!rfs_stat(pb, &st) && st.type == RFS_DIR) {
            const char *base = a;
            for (const char *q = a; *q; q++) if (*q == '/') base = q + 1;
            if (!rfs_path_join(pb, base, pb, sizeof pb)) return;
        }
        int e = rfs_rename(pa, pb);
        if (e) fs_err(a, e); else t_printf(K_GOOD, "Moved to %s\n", pb);
    } else if (!strcmp(cmd, "rm") || !strcmp(cmd, "del") || !strcmp(cmd, "rmdir")) {
        char *name = next_word(&p), path[256];
        if (!*name) { t_puts("Usage: rm <name>\n", K_BAD); return; }
        if (!abs_path(name, path)) return;
        int e = rfs_remove(path);
        if (e) fs_err(name, e); else t_puts("Deleted.\n", K_GOOD);
    } else if (!strcmp(cmd, "edit")) {
        char *name = next_word(&p), path[256];
        if (!abs_path(*name ? name : "Untitled.txt", path)) return;
        notepad_open_file(path);
    } else if (!strcmp(cmd, "disk") || !strcmp(cmd, "df")) {
        cmd_disk();
    } else if (!strcmp(cmd, "check") || !strcmp(cmd, "fsck")) {
        int fix = !strcmp(next_word(&p), "fix");
        char *rep = kmalloc(4000);
        int problems = 0;
        t_puts(fix ? "Checking and repairing the disk...\n" : "Checking the disk...\n", K_NORMAL);
        int e = rfs_check(fix, rep, 4000, &problems, 0);
        if (e && !problems) fs_err("check", e);
        else if (!problems) t_puts("No problems found.\n", K_GOOD);
        else {
            t_puts(rep, fix ? K_NORMAL : K_BAD);
            t_printf(fix ? K_GOOD : K_BAD, fix ? "Fixed %d problem(s).\n" : "%d problem(s). Type 'check fix' to repair them.\n", problems);
        }
        kfree(rep);
    } else if (!strcmp(cmd, "format")) {
        if (strcmp(next_word(&p), "yes")) {
            t_puts(storage_is_ram() ? "This empties the memory disk, or sets up a blank data disk if there is one.\n"
                                    : "This erases everything on the data disk.\n", K_HEAD);
            t_puts("Type 'format yes' to go ahead.\n", K_HEAD);
            return;
        }
        int e = storage_format();
        if (e) fs_err("format", e);
        else { strlcpy0(cwd, "/", sizeof cwd); t_puts("Formatted.\n", K_GOOD); cmd_disk(); }
    } else if (!strcmp(cmd, "open")) {
        char *what = next_word(&p);
        for (int i = 0; i < APP_COUNT; i++)
            if (ieq_prefix(what, app_names[i])) { app_open(i); return; }
        t_puts("Unknown app. Try: notepad, files, calculator, paint, snake, about\n", K_BAD);
    } else if (!strcmp(cmd, "potato")) {
        /* not listed in 'help': you have to know about it */
        t_puts("        .-~~~~-.\n"
               "      .'  .     '.\n"
               "     /  .    .    \\\n"
               "    |      .    .  |\n"
               "     \\  .     .   /\n"
               "      '-.______.-'\n", K_SPUD);
        t_puts("Hello, potato. <3\n", K_GOOD);
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
            gfx_text(x, y, prompt(), 0x8fd18a);
            x += gfx_text_w(prompt());
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
        t_puts(prompt(), K_GOOD);
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
        t_puts(prompt(), K_GOOD);
        t_puts(in, K_CMD);
        t_puts("^C\n", K_BAD);
        inlen = 0;
        in[0] = 0;
    } else if (k >= 32 && k < 127 && inlen < T_INMAX - (int)strlen(prompt())) {
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
