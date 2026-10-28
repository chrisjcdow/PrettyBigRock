; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2026 Christopher Cameron-Dow
; FynbosOS kernel entry and interrupt stubs

[bits 32]
section .text.entry
global _start
extern kmain
extern __bss_start
extern __bss_end

_start:
    ; clear .bss
    mov edi, __bss_start
    mov ecx, __bss_end
    sub ecx, edi
    xor eax, eax
    cld
    rep stosb

    ; enable the FPU (clear EM, set MP and NE)
    mov eax, cr0
    and eax, ~(1 << 2)
    or eax, (1 << 1) | (1 << 5)
    mov cr0, eax
    fninit

    call kmain
.hang:
    cli
    hlt
    jmp .hang

section .text
extern isr_handler
global isr_stub_table

%macro ISR_NOERR 1
isr%1:
    push dword 0
    push dword %1
    jmp isr_common
%endmacro

%macro ISR_ERR 1
isr%1:
    push dword %1
    jmp isr_common
%endmacro

%assign i 0
%rep 48
  %if i == 8 || i == 10 || i == 11 || i == 12 || i == 13 || i == 14 || i == 17 || i == 21 || i == 29 || i == 30
    ISR_ERR i
  %else
    ISR_NOERR i
  %endif
  %assign i i+1
%endrep

isr_common:
    pushad
    cld
    push esp
    call isr_handler
    add esp, 4
    popad
    add esp, 8
    iretd

section .data
isr_stub_table:
%assign i 0
%rep 48
    dd isr %+ i
  %assign i i+1
%endrep
