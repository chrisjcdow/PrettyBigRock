/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow
 *
 * The BIOS loader's memory layout (docs/design/0004-loaders-and-boot-disk.md):
 *   0x7C00  the first stage, which the BIOS loads from the MBR (at most 432 bytes)
 *   0x8000  the second stage and the Rust third stage, which the first stage loads
 * The build tool turns this into a flat binary starting at 0x7C00. */
ENTRY(stage1_start)

PHDRS
{
    loader PT_LOAD FLAGS(7);
}

SECTIONS
{
    . = 0x7C00;
    .stage1 : { KEEP(*(.stage1)) } :loader
    ASSERT(SIZEOF(.stage1) <= 432, "the BIOS first stage must fit in 432 bytes")

    . = 0x8000;
    __loader_start = .;
    .stage2 : { KEEP(*(.stage2)) }
    .text : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    /* .bss goes inside .data, so the flat binary includes it as zeros. */
    .data : { *(.data .data.*) *(.got .got.*) *(.bss .bss.*) *(COMMON) . = ALIGN(512); }
    __loader_end = .;
    ASSERT(__loader_end <= 0x68000, "the BIOS loader must end below 0x68000")

    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame*) }
}
