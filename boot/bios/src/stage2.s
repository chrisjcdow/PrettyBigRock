# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
#
# The BIOS second stage, at 0x8000 in 16-bit real mode. It asks the BIOS everything that only
# the BIOS can answer, copies the payload (kernel and command line) above 1 MiB, switches to
# 64-bit mode and calls the Rust third stage, `bios_main`.
#
# Low memory it uses (see also boot/bios/src/main.rs, which reads these):
#   0x0500 boot drive (byte)          0x0504 next payload sector    0x0508 sectors left
#   0x050C payload destination        0x0600 int 15h/87h descriptor table
#   0x0700 disk address packet        0x1000 E820 count (word), entries of 24 bytes from 0x1008
#   0x2000 VBE controller info        0x2200 VBE mode info          0x2400 VBE mode list copy
#   0x2600 chosen VBE mode (word, 0 = none)   0x2604 best area so far (dword)
#   0x3000-0x6FFF temporary page tables       0x70000-0x7FFFF disk buffer, then the stack

.section .stage2, "ax"
.code16
.global stage2_start
stage2_start:
    mov si, offset msg_hello
    call print

    # --- A20 line on: through the BIOS, and through the "fast A20" port.
    mov ax, 0x2401
    int 0x15
    in al, 0x92
    or al, 2
    and al, 0xFE
    out 0x92, al

    # --- The memory map (E820).
    xor ebx, ebx
    mov di, 0x1008
    xor bp, bp
e820_next:
    mov eax, 0xE820
    mov edx, 0x534D4150         # "SMAP"
    mov ecx, 24
    mov dword ptr [di + 20], 1  # "valid" in case the BIOS doesn't write the extended field
    int 0x15
    jc e820_done
    cmp eax, 0x534D4150
    jne e820_done
    test ecx, ecx
    jz e820_skip
    inc bp
    add di, 24
    cmp bp, 120
    jae e820_done
e820_skip:
    test ebx, ebx
    jnz e820_next
e820_done:
    mov word ptr [0x1000], bp

    # --- Graphics: the largest 32-bit linear-framebuffer mode up to 1920x1080.
    mov word ptr [0x2600], 0
    mov dword ptr [0x2604], 0
    mov di, 0x2000
    mov dword ptr [di], 0x32454256  # "VBE2"
    mov ax, 0x4F00
    int 0x10
    cmp ax, 0x004F
    jne vbe_done
    push ds                     # copy the mode list: it may live in a buffer we reuse
    lds si, [0x200E]
    mov di, 0x2400
    mov cx, 255
copy_modes:
    lodsw
    cmp ax, 0xFFFF
    je copy_end
    stosw
    loop copy_modes
copy_end:
    pop ds
    mov word ptr es:[di], 0xFFFF
    mov si, 0x2400
scan:
    lodsw
    cmp ax, 0xFFFF
    je scan_done
    mov cx, ax
    push si
    push cx
    mov ax, 0x4F01
    mov di, 0x2200
    int 0x10
    pop cx
    pop si
    cmp ax, 0x004F
    jne scan
    mov ax, word ptr [0x2200]   # attributes: supported, colour, graphics, linear framebuffer
    and ax, 0x99
    cmp ax, 0x99
    jne scan
    cmp byte ptr [0x2219], 32   # bits per pixel
    jne scan
    cmp byte ptr [0x221B], 6    # direct colour
    jne scan
    movzx eax, word ptr [0x2212]
    cmp eax, 1920
    ja scan
    movzx ebx, word ptr [0x2214]
    cmp ebx, 1080
    ja scan
    imul eax, ebx
    cmp eax, dword ptr [0x2604]
    jbe scan
    mov dword ptr [0x2604], eax
    mov word ptr [0x2600], cx
    jmp scan
scan_done:
    mov cx, word ptr [0x2600]
    test cx, cx
    jz vbe_done
    mov ax, 0x4F01              # fetch the chosen mode's details again
    mov di, 0x2200
    int 0x10
    mov si, offset msg_loading  # last message: the screen is about to change mode
    call print
    mov ax, 0x4F02
    mov bx, word ptr [0x2600]
    or bx, 0x4000               # with its linear framebuffer
    int 0x10
    cmp ax, 0x004F
    je vbe_set
    mov word ptr [0x2600], 0
vbe_done:
    mov si, offset msg_loading
    call print
vbe_set:

    # --- The payload, from the sector after the loader, to 1 MiB.
    mov eax, dword ptr [0x7DB0]
    movzx edx, word ptr [0x7DB4]
    add eax, edx
    mov dword ptr [0x504], eax
    mov cx, 1
    mov bx, 0x7000
    call read_sectors           # its header, to read how long it is
    jc disk_error
    push ds
    mov ax, 0x7000
    mov ds, ax
    mov ecx, dword ptr [12]
    pop ds
    mov dword ptr [0x508], ecx
    mov dword ptr [0x50C], 0x100000
copy_next:
    mov ecx, dword ptr [0x508]
    test ecx, ecx
    jz copy_done
    cmp ecx, 64
    jbe 1f
    mov ecx, 64
1:
    mov eax, dword ptr [0x504]
    mov bx, 0x7000
    call read_sectors
    jc disk_error
    # Copy above 1 MiB with the BIOS's "move extended memory" call.
    mov di, 0x600
    push cx
    mov cx, 24
    xor ax, ax
    rep stosw
    pop cx
    mov word ptr [0x610], 0xFFFF    # source: 0x70000
    mov dword ptr [0x612], 0x93070000
    mov word ptr [0x618], 0xFFFF    # destination
    mov eax, dword ptr [0x50C]
    mov word ptr [0x61A], ax
    shr eax, 16
    mov byte ptr [0x61C], al
    mov byte ptr [0x61D], 0x93
    mov byte ptr [0x61F], ah
    push cx
    shl cx, 8                   # sectors to 16-bit words
    mov si, 0x600
    mov ah, 0x87
    int 0x15
    pop cx
    jc disk_error
    movzx ecx, cx
    add dword ptr [0x504], ecx
    sub dword ptr [0x508], ecx
    shl ecx, 9
    add dword ptr [0x50C], ecx
    jmp copy_next
copy_done:

    # --- 64-bit mode, with the first 2 GiB identity-mapped by 2 MiB pages.
    cli
    xor eax, eax
    mov di, 0x3000
    mov cx, 0x1000
    rep stosd
    mov dword ptr [0x3000], 0x4003
    mov dword ptr [0x4000], 0x5003
    mov dword ptr [0x4008], 0x6003
    mov di, 0x5000
    mov eax, 0x83
    mov cx, 1024
1:
    mov dword ptr [di], eax
    add eax, 0x200000
    add di, 8
    loop 1b
    lgdt [gdt_pointer]
    mov eax, cr4
    or eax, 0x20                # PAE
    mov cr4, eax
    mov eax, 0x3000
    mov cr3, eax
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x100               # long mode
    wrmsr
    mov eax, cr0
    or eax, 0x80000001          # paging and protection on together
    mov cr0, eax
    .byte 0x66, 0xEA            # jmp 0x08:long_mode
    .long long_mode
    .word 0x08

disk_error:
    mov si, offset msg_disk
    call print
1:
    cli
    hlt
    jmp 1b

# read_sectors: CX sectors (at most 64) from sector EAX to BX:0000. Carry set on failure.
read_sectors:
    mov byte ptr [0x700], 0x10
    mov byte ptr [0x701], 0
    mov word ptr [0x702], cx
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
    ret

# print: the zero-terminated text at SI, through the BIOS.
print:
    pushad
1:
    lodsb
    test al, al
    jz 2f
    mov ah, 0x0E
    mov bx, 7
    int 0x10
    jmp 1b
2:
    popad
    ret

.balign 8
gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF    # 0x08: 64-bit code
    .quad 0x00CF92000000FFFF    # 0x10: data
gdt_pointer:
    .word 23
    .long gdt

msg_hello:   .asciz "PrettyBigRock BIOS loader\r\n"
msg_loading: .asciz "Loading PrettyBigRock...\r\n"
msg_disk:    .asciz "PrettyBigRock can't read the boot disk.\r\n"

.code64
long_mode:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov rsp, 0x80000
    call bios_main
1:
    hlt
    jmp 1b
