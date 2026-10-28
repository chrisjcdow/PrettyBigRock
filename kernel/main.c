/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - kernel entry point */
#include "kernel.h"

static void boot_splash(void) {
    gfx_fill(0, 0, SW, SH, 0x12141c);
    gfx_icon(ICON_LOGO, SW / 2 - 48, SH / 2 - 110, 6);
    gfx_text_scaled(SW / 2 - gfx_text_w(OS_NAME) * 3 / 2, SH / 2 + 6, OS_NAME, 0xffffff, 3);
    gfx_present((rect_t){0, 0, SW, SH});
}

static void splash_progress(int step, int of) {
    int w = 240, x = SW / 2 - w / 2, y = SH / 2 + 56;
    gfx_round(x, y, w, 8, 4, 0x2a2e3d);
    gfx_round(x, y, w * step / of, 8, 4, 0xd2577a);
    gfx_present((rect_t){x, y, w, 8});
}

void kmain(void) {
    gfx_init();
    boot_splash();
    idt_init();
    splash_progress(1, 5);
    mouse_init();
    splash_progress(2, 5);
    fs_init();
    splash_progress(3, 5);
    sti();
    wm_init();
    splash_progress(4, 5);
    u32 t0 = ticks;
    while (ticks - t0 < 40) hlt();
    splash_progress(5, 5);
    t0 = ticks;
    while (ticks - t0 < 20) hlt();
    app_open(APP_ABOUT);
    wm_run();
}
