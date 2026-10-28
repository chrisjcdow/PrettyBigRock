; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2026 Christopher Cameron-Dow
; PrettyBigRock - stage 2
; Picks the best VESA linear-framebuffer mode, enables A20,
; switches to 32-bit protected mode and jumps to the kernel at 0x10000.
;
; Mode info handed to the kernel at 0x5000:
;   +0 dd framebuffer address
;   +4 dw width   +6 dw height   +8 dw pitch (bytes)   +10 db bpp

[bits 16]
[org 0x7E00]

VBE_CTRL  equ 0x6000
VBE_MODE  equ 0x6400
BOOTINFO  equ 0x5000

stage2:
    mov si, msg_s2
    call print

    ; --- A20 (fast gate, plus BIOS call as a backup) ---
    in al, 0x92
    or al, 2
    and al, 0xFE
    out 0x92, al
    mov ax, 0x2401
    int 0x15

    ; --- VBE controller info ---
    xor ax, ax
    mov es, ax
    mov di, VBE_CTRL
    mov dword [es:di], 'VBE2'
    mov ax, 0x4F00
    int 0x10
    cmp ax, 0x004F
    jne no_vbe

    mov si, [VBE_CTRL + 14]
    mov ax, [VBE_CTRL + 16]
    mov fs, ax
    mov word [best_mode], 0xFFFF
    mov word [best_score], 0

.next_mode:
    mov cx, [fs:si]
    cmp cx, 0xFFFF
    je .scan_done
    add si, 2

    push si
    push fs
    push cx
    xor ax, ax
    mov es, ax
    mov di, VBE_MODE
    mov ax, 0x4F01
    int 0x10
    pop cx
    pop fs
    pop si
    cmp ax, 0x004F
    jne .next_mode

    mov ax, [VBE_MODE]          ; mode attributes
    test ax, 0x01               ; supported
    jz .next_mode
    test ax, 0x10               ; graphics
    jz .next_mode
    test ax, 0x80               ; linear framebuffer
    jz .next_mode
    cmp byte [VBE_MODE + 27], 6 ; direct colour
    jne .next_mode

    ; score = resolution rank * 2 + colour depth bonus
    xor bx, bx
    mov ax, [VBE_MODE + 18]
    mov dx, [VBE_MODE + 20]
    cmp ax, 1024
    jne .not1024
    cmp dx, 768
    jne .next_mode
    mov bx, 4
    jmp .depth
.not1024:
    cmp ax, 800
    jne .not800
    cmp dx, 600
    jne .next_mode
    mov bx, 2
    jmp .depth
.not800:
    cmp ax, 640
    jne .next_mode
    cmp dx, 480
    jne .next_mode
    mov bx, 0
.depth:
    mov al, [VBE_MODE + 25]
    cmp al, 32
    jne .not32
    add bx, 2
    jmp .score
.not32:
    cmp al, 24
    jne .next_mode
    add bx, 1
.score:
    cmp bx, [best_score]
    jbe .next_mode
    mov [best_score], bx
    mov [best_mode], cx
    jmp .next_mode

.scan_done:
    mov cx, [best_mode]
    cmp cx, 0xFFFF
    je no_vbe

    ; re-read the chosen mode's info
    xor ax, ax
    mov es, ax
    mov di, VBE_MODE
    mov ax, 0x4F01
    int 0x10

    mov eax, [VBE_MODE + 40]
    mov [BOOTINFO + 0], eax
    mov ax, [VBE_MODE + 18]
    mov [BOOTINFO + 4], ax
    mov ax, [VBE_MODE + 20]
    mov [BOOTINFO + 6], ax
    mov ax, [VBE_MODE + 16]
    mov [BOOTINFO + 8], ax
    mov al, [VBE_MODE + 25]
    mov [BOOTINFO + 10], al

    ; set the mode with the linear framebuffer bit
    mov bx, [best_mode]
    or bx, 0x4000
    mov ax, 0x4F02
    int 0x10
    cmp ax, 0x004F
    jne no_vbe

    ; --- protected mode ---
    cli
    lgdt [gdt_desc]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    jmp 0x08:pm_entry

no_vbe:
    mov si, msg_novbe
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

[bits 32]
pm_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x1FFFF0
    jmp 0x10000

align 8
gdt:
    dq 0
    dq 0x00CF9A000000FFFF       ; code, flat 4 GiB
    dq 0x00CF92000000FFFF       ; data, flat 4 GiB
gdt_end:
gdt_desc:
    dw gdt_end - gdt - 1
    dd gdt

best_mode  dw 0
best_score dw 0
msg_s2     db 13, 10, "Choosing a graphics mode...", 0
msg_novbe  db 13, 10, "No usable VESA graphics mode found.", 0

times (STAGE2_SECTORS * 512) - ($ - $$) db 0
