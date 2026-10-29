# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
#
# The BIOS first stage: the boot code in the disk's MBR. The BIOS loads it at 0x7C00 and starts
# it in 16-bit real mode with the boot drive in DL. It reads the rest of the loader from the
# BIOS boot partition to 0x8000 and jumps there. The build tool writes where to read from into
# the MBR: the first sector (32 bits) at offset 432 and the number of sectors (16 bits) at 436.

.section .stage1, "ax"
.code16
.global stage1_start
stage1_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    cld
    mov byte ptr [0x500], dl

    mov ah, 0x41                # are the BIOS's extended disk reads available?
    mov bx, 0x55AA
    int 0x13
    jc 9f
    cmp bx, 0xAA55
    jne 9f

    mov eax, dword ptr [0x7DB0] # first sector of the loader
    mov cx, word ptr [0x7DB4]   # how many sectors
    mov bx, 0x0800              # destination segment: 0x0800:0000 = 0x8000
1:
    test cx, cx
    jz 3f
    mov dx, cx
    cmp dx, 64
    jbe 2f
    mov dx, 64                  # at most 32 KiB at a time
2:
    mov byte ptr [0x700], 0x10  # the disk address packet
    mov byte ptr [0x701], 0
    mov word ptr [0x702], dx
    mov word ptr [0x704], 0
    mov word ptr [0x706], bx
    mov dword ptr [0x708], eax
    mov dword ptr [0x70C], 0
    pushad
    mov si, 0x700
    mov ah, 0x42
    mov dl, byte ptr [0x500]
    int 0x13
    popad
    jc 9f
    movzx edx, dx
    add eax, edx
    sub cx, dx
    shl dx, 5                   # sectors to 16-byte paragraphs
    add bx, dx
    jmp 1b
3:
    mov dl, byte ptr [0x500]
    .byte 0xEA                  # jmp 0x0000:0x8000
    .word 0x8000, 0x0000

9:
    mov si, offset stage1_message
4:
    lodsb
    test al, al
    jz 5f
    mov ah, 0x0E
    mov bx, 7
    int 0x10
    jmp 4b
5:
    cli
    hlt
    jmp 5b
stage1_message:
    .asciz "PrettyBigRock can't read the boot disk."
