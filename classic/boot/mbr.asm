; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2026 Christopher Cameron-Dow
; PrettyBigRock - stage 1 boot sector
; Loads stage 2 to 0x7E00 and the kernel to 0x10000 using BIOS LBA reads,
; then hands over to stage 2.

[bits 16]
[org 0x7C00]

    jmp 0x0000:start

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    mov [drive], dl

    mov si, msg_hello
    call print

    ; ---- stage 2 ----
    mov word [dap_count], STAGE2_SECTORS
    mov word [dap_off], 0x7E00
    mov word [dap_seg], 0
    mov dword [dap_lba], 1
    call read

    ; ---- kernel, in chunks of up to 64 sectors ----
    mov word [remaining], KERNEL_SECTORS
    mov word [dap_off], 0
    mov word [dap_seg], 0x1000
    mov dword [dap_lba], 1 + STAGE2_SECTORS
.loop:
    mov ax, [remaining]
    test ax, ax
    jz .done
    cmp ax, 64
    jbe .ok
    mov ax, 64
.ok:
    mov [dap_count], ax
    push ax
    call read
    pop ax
    sub [remaining], ax
    movzx eax, ax
    add [dap_lba], eax
    shl ax, 5                   ; sectors * 512 / 16 = paragraphs
    add [dap_seg], ax
    mov si, msg_dot
    call print
    jmp .loop
.done:
    mov dl, [drive]
    jmp 0x0000:0x7E00

read:
    mov si, dap
    mov ah, 0x42
    mov dl, [drive]
    int 0x13
    jc .err
    ret
.err:
    mov si, msg_err
    call print
.hang:
    hlt
    jmp .hang

print:
    lodsb
    test al, al
    jz .end
    mov ah, 0x0E
    mov bx, 0x0007
    int 0x10
    jmp print
.end:
    ret

msg_hello db "PrettyBigRock loader", 0
msg_dot   db ".", 0
msg_err   db 13, 10, "Disk read failed.", 0

drive     db 0
remaining dw 0

align 4
dap:
          db 16, 0
dap_count dw 0
dap_off   dw 0
dap_seg   dw 0
dap_lba   dq 0

times 510 - ($ - $$) db 0
dw 0xAA55
