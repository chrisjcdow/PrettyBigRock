/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow
 *
 * The kernel is linked in the top 2 GiB of the address space, the same on every architecture.
 * Each part starts on its own 4 KiB page so it can be mapped with the right permissions:
 * code is read-and-execute, read-only data is read-only, data is read-write and never
 * executable. See docs/design/0003-boot-information.md. The kernel maps itself again from
 * these symbols when it builds its own page tables (docs/design/0005-kernel-core.md). */
ENTRY(_start)

KERNEL_BASE = 0xFFFFFFFF80000000;

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* read + execute */
    rodata PT_LOAD FLAGS(4);   /* read */
    data   PT_LOAD FLAGS(6);   /* read + write */
}

SECTIONS
{
    . = KERNEL_BASE;
    __kernel_start = .;

    .text : ALIGN(4K) { __text_start = .; *(.text._start) *(.text .text.*) } :text
    . = ALIGN(4K);
    __text_end = .;

    .rodata : { __rodata_start = .; *(.rodata .rodata.*) } :rodata
    .eh_frame_hdr : { *(.eh_frame_hdr) } :rodata
    .eh_frame : { *(.eh_frame) } :rodata
    . = ALIGN(4K);
    __rodata_end = .;

    .data : { __data_start = .; *(.data .data.*) } :data
    .got : { *(.got .got.*) } :data
    .bss : { __bss_start = .; *(.bss .bss.*) *(COMMON) __bss_end = .; } :data

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note.*) }
}
