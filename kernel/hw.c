/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - hardware: interrupts, timer, keyboard, mouse, clock, power */
#include "kernel.h"

/* ---------------------------------------------------------------- IDT */
struct idt_entry { u16 lo, sel; u8 zero, flags; u16 hi; } __attribute__((packed));
struct idt_ptr { u16 limit; u32 base; } __attribute__((packed));
struct regs { u32 edi, esi, ebp, esp, ebx, edx, ecx, eax, int_no, err, eip, cs, eflags; };

static struct idt_entry idt[48];
extern u32 isr_stub_table[48];
volatile u32 ticks;

static const char *exc_names[] = {
    "Divide by zero", "Debug", "NMI", "Breakpoint", "Overflow", "Bound range",
    "Invalid opcode", "Device not available", "Double fault", "Coprocessor overrun",
    "Invalid TSS", "Segment not present", "Stack fault", "General protection fault",
    "Page fault", "Reserved", "x87 error", "Alignment check", "Machine check", "SIMD error",
};

static void panic(struct regs *r) {
    char buf[96];
    gfx_set_clip((rect_t){0, 0, SW, SH});
    gfx_fill(0, 0, SW, SH, 0x6b1d2a);
    gfx_icon(ICON_LOGO, 40, 40, 4);
    gfx_text_scaled(120, 48, "PrettyBigRock stopped unexpectedly", 0xffffff, 2);
    ksnprintf(buf, sizeof buf, "%s (exception %u)", r->int_no < 20 ? exc_names[r->int_no] : "Reserved", r->int_no);
    gfx_text_scaled(120, 80, buf, 0xffd7de, 1);
    ksnprintf(buf, sizeof buf, "EIP=%08x  ERR=%08x  EAX=%08x  ESP=%08x", r->eip, r->err, r->eax, r->esp);
    gfx_text(120, 100, buf, 0xffd7de);
    gfx_text(120, 130, "Please restart the virtual machine.", 0xffffff);
    gfx_present((rect_t){0, 0, SW, SH});
    for (;;) { cli(); hlt(); }
}

/* keyboard and mouse ring buffers, filled from IRQ context */
#define KQ 128
static volatile u8 kq[KQ];
static volatile u32 kq_head, kq_tail;
#define MQ 64
static volatile int mq_dx[MQ], mq_dy[MQ], mq_b[MQ];
static volatile u32 mq_head, mq_tail;
static u8 mpkt[3];
static int mcycle;

static void mouse_byte(u8 b) {
    if (mcycle == 0 && !(b & 0x08)) return; /* resync: first byte always has bit 3 */
    mpkt[mcycle++] = b;
    if (mcycle < 3) return;
    mcycle = 0;
    if (mpkt[0] & 0xC0) return; /* overflow */
    int dx = (int)mpkt[1] - ((mpkt[0] << 4) & 0x100);
    int dy = (int)mpkt[2] - ((mpkt[0] << 3) & 0x100);
    u32 n = (mq_head + 1) % MQ;
    if (n == mq_tail) return;
    mq_dx[mq_head] = dx;
    mq_dy[mq_head] = -dy;
    mq_b[mq_head] = mpkt[0] & 7;
    mq_head = n;
}

void isr_handler(struct regs *r) {
    if (r->int_no < 32) panic(r);
    int irq = r->int_no - 32;
    if (irq == 0) {
        ticks++;
    } else if (irq == 1) {
        u8 sc = inb(0x60);
        u32 n = (kq_head + 1) % KQ;
        if (n != kq_tail) { kq[kq_head] = sc; kq_head = n; }
    } else if (irq == 12) {
        mouse_byte(inb(0x60));
    }
    if (irq >= 8) outb(0xA0, 0x20);
    outb(0x20, 0x20);
}

int kbd_pop(u8 *sc) {
    if (kq_tail == kq_head) return 0;
    *sc = kq[kq_tail];
    kq_tail = (kq_tail + 1) % KQ;
    return 1;
}

int mouse_pop(int *dx, int *dy, int *b) {
    if (mq_tail == mq_head) return 0;
    *dx = mq_dx[mq_tail];
    *dy = mq_dy[mq_tail];
    *b = mq_b[mq_tail];
    mq_tail = (mq_tail + 1) % MQ;
    return 1;
}

static void idt_set(int n, u32 h) {
    idt[n].lo = h & 0xFFFF;
    idt[n].hi = h >> 16;
    idt[n].sel = 0x08;
    idt[n].zero = 0;
    idt[n].flags = 0x8E;
}

void idt_init(void) {
    for (int i = 0; i < 48; i++) idt_set(i, isr_stub_table[i]);
    struct idt_ptr p = {sizeof(idt) - 1, (u32)idt};
    __asm__ volatile("lidt %0" ::"m"(p));

    /* remap the PICs to vectors 32-47 */
    outb(0x20, 0x11); io_wait();
    outb(0xA0, 0x11); io_wait();
    outb(0x21, 0x20); io_wait();
    outb(0xA1, 0x28); io_wait();
    outb(0x21, 0x04); io_wait();
    outb(0xA1, 0x02); io_wait();
    outb(0x21, 0x01); io_wait();
    outb(0xA1, 0x01); io_wait();
    outb(0x21, 0xF8); /* timer, keyboard, cascade */
    outb(0xA1, 0xEF); /* mouse (IRQ 12) */

    /* timer at 100 Hz */
    u32 div = 1193182 / 100;
    outb(0x43, 0x36);
    outb(0x40, div & 0xFF);
    outb(0x40, div >> 8);
}

/* ---------------------------------------------------------------- PS/2 mouse */
static void ps2_wait_write(void) { for (int i = 0; i < 100000 && (inb(0x64) & 2); i++); }
static void ps2_wait_read(void) { for (int i = 0; i < 100000 && !(inb(0x64) & 1); i++); }

static void mouse_cmd(u8 v) {
    ps2_wait_write(); outb(0x64, 0xD4);
    ps2_wait_write(); outb(0x60, v);
    ps2_wait_read(); inb(0x60); /* ACK */
}

void mouse_init(void) {
    ps2_wait_write(); outb(0x64, 0xA8); /* enable aux port */
    ps2_wait_write(); outb(0x64, 0x20);
    ps2_wait_read();
    u8 cfg = inb(0x60);
    cfg |= 0x02;          /* IRQ 12 on */
    cfg &= ~0x20;         /* aux clock on */
    ps2_wait_write(); outb(0x64, 0x60);
    ps2_wait_write(); outb(0x60, cfg);
    mouse_cmd(0xF6);      /* defaults */
    mouse_cmd(0xF3);      /* sample rate... */
    mouse_cmd(100);       /* ...100/s */
    mouse_cmd(0xF4);      /* start streaming */
    while (inb(0x64) & 1) inb(0x60);
}

/* ---------------------------------------------------------------- absolute pointer (VMware backdoor) */
#define VMW_MAGIC        0x564D5868u
#define VMW_PORT         0x5658
#define VMW_GETVERSION   10
#define VMW_ABS_DATA     39
#define VMW_ABS_STATUS   40
#define VMW_ABS_COMMAND  41
#define VMW_CMD_ENABLE   0x45414552u
#define VMW_CMD_DISABLE  0x000000F5u
#define VMW_CMD_ABSOLUTE 0x53424152u
#define VMW_VERSION_ID   0x3442554Au
#define VMW_ERROR        0xFFFF0000u
#define VMW_RELATIVE     0x00010000u

typedef struct { u32 a, b, c, d; } vmw_regs_t;
static int vmm_on;

static vmw_regs_t vmw_call(u32 cmd, u32 arg) {
    vmw_regs_t r = {VMW_MAGIC, arg, cmd, VMW_PORT};
    __asm__ volatile("inl %%dx, %%eax" : "+a"(r.a), "+b"(r.b), "+c"(r.c), "+d"(r.d) : : "memory");
    return r;
}

int vmmouse_init(void) {
    /* On real hardware nothing answers this port, so the check below fails safely. */
    vmw_regs_t r = vmw_call(VMW_GETVERSION, ~VMW_MAGIC);
    if (r.b != VMW_MAGIC || r.a == 0xFFFFFFFFu) return 0;
    vmw_call(VMW_ABS_COMMAND, VMW_CMD_ENABLE);
    r = vmw_call(VMW_ABS_STATUS, 0);
    if ((r.a & VMW_ERROR) == VMW_ERROR || (r.a & 0xFFFF) == 0) return 0;
    r = vmw_call(VMW_ABS_DATA, 1);
    if (r.a != VMW_VERSION_ID) { vmw_call(VMW_ABS_COMMAND, VMW_CMD_DISABLE); return 0; }
    vmw_call(VMW_ABS_COMMAND, VMW_CMD_ABSOLUTE);
    vmm_on = 1;
    return 1;
}

int vmmouse_active(void) { return vmm_on; }

/* Returns 1 and fills in one event if one is waiting. */
int vmmouse_poll(int *x, int *y, int *buttons, int *wheel) {
    if (!vmm_on) return 0;
    vmw_regs_t r = vmw_call(VMW_ABS_STATUS, 0);
    if ((r.a & VMW_ERROR) == VMW_ERROR) {
        /* the host reset the device: start again, or fall back to the PS/2 mouse */
        vmm_on = 0;
        vmmouse_init();
        return 0;
    }
    if ((r.a & 0xFFFF) < 4) return 0;
    r = vmw_call(VMW_ABS_DATA, 4);
    if (r.a & VMW_RELATIVE) return 0; /* only absolute packets are expected */
    *x = (int)(r.b & 0xFFFF);
    *y = (int)(r.c & 0xFFFF);
    *wheel = (int)(i8)(r.d & 0xFF);
    *buttons = ((r.a & 0x20) ? 1 : 0) | ((r.a & 0x10) ? 2 : 0) | ((r.a & 0x08) ? 4 : 0);
    return 1;
}

/* ---------------------------------------------------------------- keyboard (scan code set 1, US layout) */
static const char km_lo[] = "\0\x1b" "1234567890-=\b" "\tqwertyuiop[]\n" "\0" "asdfghjkl;'`" "\0"
                            "\\zxcvbnm,./" "\0" "*\0 ";
static const char km_hi[] = "\0\x1b" "!@#$%^&*()_+\b" "\tQWERTYUIOP{}\n" "\0" "ASDFGHJKL:\"~" "\0"
                            "|ZXCVBNM<>?" "\0" "*\0 ";
static int k_shift, k_ctrl, k_caps, k_ext;

int kbd_translate(u8 sc) {
    if (sc == 0xE0) { k_ext = 1; return -1; }
    int ext = k_ext;
    k_ext = 0;
    int rel = sc & 0x80;
    sc &= 0x7F;
    if (sc == 0x2A || sc == 0x36) { if (!ext) k_shift = !rel; return -1; }
    if (sc == 0x1D) { k_ctrl = !rel; return -1; }
    if (rel) return -1;
    if (sc == 0x3A) { k_caps = !k_caps; return -1; }
    if (ext && (sc == 0x5B || sc == 0x5C)) return KEY_SUPER;
    int ck = k_ctrl ? KEY_CTRL : 0;
    switch (sc) {
    case 0x47: return KEY_HOME | ck;
    case 0x4F: return KEY_END | ck;
    case 0x48: return KEY_UP;
    case 0x50: return KEY_DOWN;
    case 0x4B: return KEY_LEFT;
    case 0x4D: return KEY_RIGHT;
    case 0x53: return KEY_DEL;
    case 0x49: return KEY_PGUP;
    case 0x51: return KEY_PGDN;
    }
    if (ext && sc == 0x1C) return '\n';
    if (ext && sc == 0x35) return '/';
    if (sc == 0x4A) return '-';
    if (sc == 0x4E) return '+';
    if (sc >= sizeof(km_lo) - 1) return -1;
    char c = k_shift ? km_hi[sc] : km_lo[sc];
    if (!c) return -1;
    if (k_caps && ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'))) c ^= 0x20;
    if (k_ctrl && c == 27) return KEY_CTRL | 27;
    if (k_ctrl && c >= 'a' && c <= 'z') return KEY_CTRL | c;
    if (k_ctrl && c >= 'A' && c <= 'Z') return KEY_CTRL | (c | 0x20);
    return (u8)c;
}

/* ---------------------------------------------------------------- CMOS clock and memory */
int tz_offset = 0;

static u8 cmos(u8 r) { outb(0x70, r); return inb(0x71); }
static int bcd(int v) { return (v & 0x0F) + (v >> 4) * 10; }

void rtc_read(rtc_time_t *t) {
    for (int i = 0; i < 100000 && (cmos(0x0A) & 0x80); i++);
    int s = cmos(0x00), m = cmos(0x02), h = cmos(0x04);
    int d = cmos(0x07), mo = cmos(0x08), y = cmos(0x09);
    int b = cmos(0x0B);
    int pm = h & 0x80;
    h &= 0x7F;
    if (!(b & 0x04)) { s = bcd(s); m = bcd(m); h = bcd(h); d = bcd(d); mo = bcd(mo); y = bcd(y); }
    if (!(b & 0x02)) { if (h == 12) h = 0; if (pm) h += 12; }
    h += tz_offset;
    static const int mdays[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
    if (h >= 24) {
        h -= 24;
        if (++d > mdays[(mo - 1) % 12] + (mo == 2 && y % 4 == 0)) { d = 1; if (++mo > 12) { mo = 1; y++; } }
    } else if (h < 0) {
        h += 24;
        if (--d < 1) { if (--mo < 1) { mo = 12; y--; } d = mdays[mo - 1] + (mo == 2 && y % 4 == 0); }
    }
    t->sec = s; t->min = m; t->hour = h; t->day = d; t->mon = mo; t->year = 2000 + y;
}

u32 mem_total_mb(void) {
    u32 above16 = cmos(0x34) | (cmos(0x35) << 8);   /* 64 KiB blocks */
    if (above16) return 16 + above16 / 16;
    u32 above1 = cmos(0x30) | (cmos(0x31) << 8);    /* KiB */
    return 1 + above1 / 1024;
}

/* ---------------------------------------------------------------- power */
static int sig_is(const u8 *p, const char *s) {
    for (; *s; s++, p++) if (*p != (u8)*s) return 0;
    return 1;
}

static const u8 *find_rsdp(void) {
    u32 ebda = (u32)(*(volatile u16 *)0x40E) << 4;
    if (ebda >= 0x80000 && ebda < 0xA0000)
        for (u32 a = ebda; a < ebda + 1024; a += 16)
            if (sig_is((const u8 *)a, "RSD PTR ")) return (const u8 *)a;
    for (u32 a = 0xE0000; a < 0x100000; a += 16)
        if (sig_is((const u8 *)a, "RSD PTR ")) return (const u8 *)a;
    return 0;
}

/* Minimal ACPI soft-off: find the FADT and the \_S5 sleep type in the DSDT. */
static void acpi_poweroff(void) {
    const u8 *rsdp = find_rsdp();
    if (!rsdp) return;
    const u8 *rsdt = (const u8 *)*(u32 *)(rsdp + 16);
    if (!rsdt || !sig_is(rsdt, "RSDT")) return;
    u32 n = (*(u32 *)(rsdt + 4) - 36) / 4;
    const u8 *fadt = 0;
    for (u32 i = 0; i < n; i++) {
        const u8 *t = (const u8 *)*(u32 *)(rsdt + 36 + i * 4);
        if (t && sig_is(t, "FACP")) { fadt = t; break; }
    }
    if (!fadt) return;
    const u8 *dsdt = (const u8 *)*(u32 *)(fadt + 40);
    u32 smi_cmd = *(u32 *)(fadt + 48);
    u8 acpi_en = *(fadt + 52);
    u32 pm1a = *(u32 *)(fadt + 64);
    u32 pm1b = *(u32 *)(fadt + 68);
    if (!dsdt || !sig_is(dsdt, "DSDT") || !pm1a) return;
    u32 len = *(u32 *)(dsdt + 4);
    u16 typa = 0, typb = 0;
    int found = 0;
    for (u32 i = 36; i + 8 < len; i++) {
        const u8 *s = dsdt + i;
        if (!sig_is(s, "_S5_")) continue;
        if (!((s[-1] == 0x08) || (s[-2] == 0x08 && s[-1] == '\\')) || s[4] != 0x12) continue;
        const u8 *p = s + 5;
        p += ((*p & 0xC0) >> 6) + 2;  /* skip package length and element count */
        if (*p == 0x0A) p++;
        typa = (u16)(*p++) << 10;
        if (*p == 0x0A) p++;
        typb = (u16)(*p) << 10;
        found = 1;
        break;
    }
    if (!found) return;
    if (!(inw(pm1a) & 1) && smi_cmd && acpi_en) {
        outb(smi_cmd, acpi_en);
        for (int i = 0; i < 3000000 && !(inw(pm1a) & 1); i++) io_wait();
    }
    outw(pm1a, typa | (1 << 13));
    if (pm1b) outw(pm1b, typb | (1 << 13));
}

void power_off(void) {
    storage_shutdown(); /* marks the data disk as shut down cleanly */
    cli();
    acpi_poweroff();
    outw(0x604, 0x2000);  /* QEMU */
    outw(0xB004, 0x2000); /* Bochs and older QEMU */
    outw(0x4004, 0x3400); /* VirtualBox */
}

void reboot(void) {
    storage_shutdown();
    cli();
    ps2_wait_write();
    outb(0x64, 0xFE);
    struct idt_ptr p = {0, 0};
    __asm__ volatile("lidt %0; int $3" ::"m"(p));
    for (;;) hlt();
}
