; long_mode_init.asm — маленький 64-битный "трамплин" перед вызовом Rust-ядра.

global long_mode_start
extern kernel_main

section .text
bits 64
long_mode_start:
    ; обнуляем сегментные регистры данных (в long mode они почти не используются,
    ; но обнулить их — хороший тон)
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    call kernel_main

    hlt
.hang:
    jmp .hang
